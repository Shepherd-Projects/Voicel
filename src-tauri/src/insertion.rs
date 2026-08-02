use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
    VK_RWIN,
};

pub fn type_text(text: &str) -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default()).context("initialize keyboard injection")?;
    wait_for_modifiers_released()?;
    enigo.text(text).context("type transcript")?;
    Ok(())
}

pub fn paste_preserving_clipboard(text: String) -> Result<()> {
    #[cfg(windows)]
    {
        windows_paste::paste(text)
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        anyhow::bail!("Receipt-sequenced clipboard insertion is available on Windows only")
    }
}

fn send_ctrl_v() -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default()).context("initialize keyboard injection")?;
    wait_for_modifiers_released()?;
    enigo
        .key(Key::Control, Direction::Press)
        .context("press Control")?;
    let click = enigo
        .key(Key::Other(0x56), Direction::Click)
        .context("press V");
    thread::sleep(Duration::from_millis(8));
    let release = enigo
        .key(Key::Control, Direction::Release)
        .context("release Control");
    click?;
    release?;
    Ok(())
}

#[cfg(windows)]
const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(windows)]
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_secs(1);

fn wait_for_modifiers_released() -> Result<()> {
    #[cfg(windows)]
    {
        use std::time::Instant;

        let deadline = Instant::now() + MODIFIER_RELEASE_TIMEOUT;
        loop {
            if modifier_keys_are_released(&current_modifier_key_states()) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "timed out after {MODIFIER_RELEASE_TIMEOUT:?} waiting for Ctrl, Shift, Alt, and Win to be released"
                );
            }
            thread::sleep(MODIFIER_POLL_INTERVAL);
        }
    }

    #[cfg(not(windows))]
    {
        Ok(())
    }
}

fn modifier_keys_are_released(states: &[i16]) -> bool {
    states.iter().all(|state| (*state & i16::MIN) == 0)
}

#[cfg(windows)]
fn current_modifier_key_states() -> [i16; 8] {
    unsafe {
        [
            GetAsyncKeyState(i32::from(VK_LCONTROL.0)),
            GetAsyncKeyState(i32::from(VK_RCONTROL.0)),
            GetAsyncKeyState(i32::from(VK_LSHIFT.0)),
            GetAsyncKeyState(i32::from(VK_RSHIFT.0)),
            GetAsyncKeyState(i32::from(VK_LMENU.0)),
            GetAsyncKeyState(i32::from(VK_RMENU.0)),
            GetAsyncKeyState(i32::from(VK_LWIN.0)),
            GetAsyncKeyState(i32::from(VK_RWIN.0)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::modifier_keys_are_released;

    #[test]
    fn modifier_keys_are_released_uses_the_currently_down_bit() {
        assert!(modifier_keys_are_released(&[0, 1, i16::MAX]));
        assert!(!modifier_keys_are_released(&[0, i16::MIN]));
        assert!(!modifier_keys_are_released(&[0, -1]));
    }
}

#[cfg(windows)]
#[allow(unsafe_op_in_unsafe_fn)] // Win32 clipboard ownership requires a message-window FFI boundary.
mod windows_paste {
    use std::sync::mpsc::{self, SyncSender};
    use std::sync::{Mutex, Once};
    use std::thread;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, anyhow, bail};
    use windows::Win32::Foundation::{
        GlobalFree, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardOwner,
        IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
    };
    use windows::Win32::System::Ole::{
        CF_BITMAP, CF_DSPBITMAP, CF_DSPENHMETAFILE, CF_DSPMETAFILEPICT, CF_ENHMETAFILE,
        CF_METAFILEPICT, CF_OWNERDISPLAY, CF_PALETTE, CF_UNICODETEXT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
        GetMessageW, GetWindowLongPtrW, HWND_MESSAGE, KillTimer, MSG, PostQuitMessage,
        RegisterClassW, SetTimer, SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY,
        WM_RENDERALLFORMATS, WM_RENDERFORMAT, WM_TIMER, WNDCLASSW,
    };
    use windows::core::{PCWSTR, w};

    use super::send_ctrl_v;

    const CLASS_NAME: PCWSTR = w!("VoicelPasteReceiptWindow");
    const TIMER_ID: usize = 1;
    const TIMER_INTERVAL_MS: u32 = 25;
    const RECEIPT_SETTLE: Duration = Duration::from_millis(125);
    const RECEIPT_TIMEOUT: Duration = Duration::from_millis(2500);
    const MAX_FORMAT_BYTES: usize = 64 * 1024 * 1024;
    static REGISTER_CLASS: Once = Once::new();

    struct SavedFormat {
        format: u32,
        bytes: Vec<u8>,
    }

    struct Transaction {
        text: String,
        formats: Mutex<Vec<SavedFormat>>,
        started: Instant,
        receipt: Mutex<Option<Instant>>,
        done: SyncSender<Result<(), String>>,
    }

    pub(super) fn paste(text: String) -> Result<()> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("voicel-clipboard".into())
            .spawn(move || {
                if let Err(error) = unsafe { clipboard_thread(text, ready_tx, done_tx.clone()) } {
                    let _ = done_tx.try_send(Err(error.to_string()));
                }
            })
            .context("start clipboard transaction")?;

        ready_rx
            .recv_timeout(Duration::from_secs(1))
            .context("publish transcript to clipboard")?
            .map_err(anyhow::Error::msg)?;
        send_ctrl_v()?;
        match done_rx
            .recv_timeout(Duration::from_secs(4))
            .context("wait for target application to read transcript")?
        {
            Ok(()) => Ok(()),
            Err(message) => bail!(message),
        }
    }

    unsafe fn clipboard_thread(
        text: String,
        ready: SyncSender<Result<(), String>>,
        done: SyncSender<Result<(), String>>,
    ) -> Result<()> {
        register_window_class()?;
        let formats = snapshot_clipboard()?;
        let transaction = Box::new(Transaction {
            text,
            formats: Mutex::new(formats),
            started: Instant::now(),
            receipt: Mutex::new(None),
            done,
        });
        let transaction_ptr = Box::into_raw(transaction);
        let instance = HINSTANCE(GetModuleHandleW(None).context("get application module")?.0);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
        .context("create clipboard receipt window")?;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, transaction_ptr as isize);

        let publish_result = publish_promise(hwnd).map_err(|error| error.to_string());
        let published = publish_result.is_ok();
        let _ = ready.send(publish_result);
        if !published {
            let _ = DestroyWindow(hwnd);
            drop(Box::from_raw(transaction_ptr));
            bail!("Could not publish transcript to the clipboard");
        }

        let _ = SetTimer(Some(hwnd), TIMER_ID, TIMER_INTERVAL_MS, None);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).into() {
            let _ = DispatchMessageW(&message);
        }
        drop(Box::from_raw(transaction_ptr));
        Ok(())
    }

    unsafe fn register_window_class() -> Result<()> {
        let mut registration_error = None;
        REGISTER_CLASS.call_once(|| {
            let instance = match GetModuleHandleW(None) {
                Ok(module) => HINSTANCE(module.0),
                Err(error) => {
                    registration_error = Some(error.to_string());
                    return;
                }
            };
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                registration_error = Some("RegisterClassW failed".into());
            }
        });
        if let Some(error) = registration_error {
            bail!(error);
        }
        Ok(())
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Transaction;
        match message {
            WM_RENDERFORMAT => {
                if !pointer.is_null() && wparam.0 as u32 == CF_UNICODETEXT.0 as u32 {
                    let transaction = &*pointer;
                    if let Ok(mut receipt) = transaction.receipt.lock() {
                        *receipt = Some(Instant::now());
                    }
                    render_text(&transaction.text);
                }
                LRESULT(0)
            }
            WM_RENDERALLFORMATS => {
                if !pointer.is_null() && OpenClipboard(Some(hwnd)).is_ok() {
                    if GetClipboardOwner().is_ok_and(|owner| owner == hwnd) {
                        render_text(&(*pointer).text);
                    }
                    let _ = CloseClipboard();
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if !pointer.is_null() {
                    let transaction = &*pointer;
                    let receipt = transaction.receipt.lock().ok().and_then(|value| *value);
                    let timed_out = transaction.started.elapsed() >= RECEIPT_TIMEOUT;
                    let settled = receipt.is_some_and(|at| at.elapsed() >= RECEIPT_SETTLE);
                    if settled || timed_out {
                        let result = if GetClipboardOwner().is_ok_and(|owner| owner == hwnd) {
                            restore_clipboard(hwnd, transaction).map_err(|error| error.to_string())
                        } else {
                            Err("Clipboard changed before Voicel could restore it".into())
                        };
                        let _ = transaction.done.try_send(result);
                        let _ = KillTimer(Some(hwnd), TIMER_ID);
                        let _ = DestroyWindow(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }

    unsafe fn publish_promise(hwnd: HWND) -> Result<()> {
        open_clipboard_with_retry(Some(hwnd))?;
        let result = (|| {
            EmptyClipboard().context("clear clipboard for transcript")?;
            if let Err(error) = SetClipboardData(CF_UNICODETEXT.0 as u32, None) {
                // Delayed rendering deliberately stores a null handle. The windows crate
                // maps that documented success shape to Err, so verify the two observable
                // Win32 postconditions before accepting it.
                let owns_clipboard = GetClipboardOwner().is_ok_and(|owner| owner == hwnd);
                let format_registered = IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok();
                if !owns_clipboard || !format_registered {
                    return Err(error).context("publish delayed transcript");
                }
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }

    unsafe fn snapshot_clipboard() -> Result<Vec<SavedFormat>> {
        open_clipboard_with_retry(None)?;
        let result = (|| {
            let mut formats = Vec::new();
            let mut format = 0_u32;
            loop {
                format = EnumClipboardFormats(format);
                if format == 0 {
                    break;
                }
                if is_non_memory_format(format) {
                    continue;
                }
                let handle = match GetClipboardData(format) {
                    Ok(handle) => handle,
                    Err(_) => continue,
                };
                let global = windows::Win32::Foundation::HGLOBAL(handle.0);
                let size = GlobalSize(global);
                if size == 0 || size > MAX_FORMAT_BYTES {
                    continue;
                }
                let pointer = GlobalLock(global);
                if pointer.is_null() {
                    continue;
                }
                let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec();
                let _ = GlobalUnlock(global);
                formats.push(SavedFormat { format, bytes });
            }
            Ok(formats)
        })();
        let _ = CloseClipboard();
        result
    }

    unsafe fn restore_clipboard(hwnd: HWND, transaction: &Transaction) -> Result<(), String> {
        open_clipboard_with_retry(Some(hwnd)).map_err(|error| error.to_string())?;
        let result = (|| {
            EmptyClipboard().map_err(|error| error.to_string())?;
            let mut formats = transaction
                .formats
                .lock()
                .map_err(|_| "Clipboard snapshot lock was poisoned".to_string())?;
            for saved in formats.drain(..) {
                let global = GlobalAlloc(GMEM_MOVEABLE, saved.bytes.len())
                    .map_err(|error| error.to_string())?;
                let pointer = GlobalLock(global).cast::<u8>();
                if pointer.is_null() {
                    let _ = GlobalFree(Some(global));
                    continue;
                }
                std::ptr::copy_nonoverlapping(saved.bytes.as_ptr(), pointer, saved.bytes.len());
                let _ = GlobalUnlock(global);
                if SetClipboardData(saved.format, Some(HANDLE(global.0))).is_err() {
                    let _ = GlobalFree(Some(global));
                }
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }

    unsafe fn render_text(text: &str) {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else {
            return;
        };
        let pointer = GlobalLock(global).cast::<u16>();
        if pointer.is_null() {
            let _ = GlobalFree(Some(global));
            return;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
        let _ = GlobalUnlock(global);
        if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(global.0))).is_err() {
            let _ = GlobalFree(Some(global));
        }
    }

    unsafe fn open_clipboard_with_retry(owner: Option<HWND>) -> Result<()> {
        for _ in 0..10 {
            if OpenClipboard(owner).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        Err(anyhow!("Clipboard is busy"))
    }

    fn is_non_memory_format(format: u32) -> bool {
        [
            CF_BITMAP.0,
            CF_DSPBITMAP.0,
            CF_DSPENHMETAFILE.0,
            CF_DSPMETAFILEPICT.0,
            CF_ENHMETAFILE.0,
            CF_METAFILEPICT.0,
            CF_OWNERDISPLAY.0,
            CF_PALETTE.0,
        ]
        .iter()
        .any(|value| *value as u32 == format)
    }
}
