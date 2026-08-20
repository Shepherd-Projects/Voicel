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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, SyncSender};
    use std::sync::{Mutex, Once};
    use std::thread;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, anyhow, bail};
    use log::{debug, error};
    use windows::Win32::Foundation::{
        GetLastError, GlobalFree, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, SetLastError,
        WIN32_ERROR, WPARAM,
    };
    use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardOwner,
        GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
    };
    use windows::Win32::System::Ole::{
        CF_BITMAP, CF_DSPBITMAP, CF_DSPENHMETAFILE, CF_DSPMETAFILEPICT, CF_DSPTEXT, CF_ENHMETAFILE,
        CF_METAFILEPICT, CF_OWNERDISPLAY, CF_PALETTE, CF_UNICODETEXT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CopyImage, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
        GetMessageW, GetWindowLongPtrW, HWND_MESSAGE, IMAGE_BITMAP, KillTimer, MSG,
        PostQuitMessage, RegisterClassW, SetTimer, SetWindowLongPtrW, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_DESTROY, WM_RENDERALLFORMATS, WM_RENDERFORMAT, WM_TIMER, WNDCLASSW,
    };
    use windows::core::{PCWSTR, w};

    use super::send_ctrl_v;

    const CLASS_NAME: PCWSTR = w!("VoicelPasteReceiptWindow");
    const TIMER_ID: usize = 1;
    const TIMER_INTERVAL_MS: u32 = 25;
    const RECEIPT_SETTLE: Duration = Duration::from_millis(125);
    const RECEIPT_TIMEOUT: Duration = Duration::from_millis(2500);
    const RESTORE_TIMEOUT: Duration = Duration::from_millis(3500);
    const MAX_FORMAT_BYTES: usize = 64 * 1024 * 1024;
    static REGISTER_CLASS: Once = Once::new();

    struct SavedFormat {
        format: u32,
        bytes: Vec<u8>,
    }

    enum SavedClipboardFormat {
        Global(SavedFormat),
        Bitmap(Option<OwnedBitmap>),
    }

    struct ClipboardSnapshot {
        sequence: u32,
        formats: Vec<SavedClipboardFormat>,
    }

    struct OwnedGlobal(Option<windows::Win32::Foundation::HGLOBAL>);

    impl OwnedGlobal {
        unsafe fn allocate(bytes: &[u8]) -> Result<Self, String> {
            let global = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))
                .map_err(|error| error.to_string())?;
            let pointer = GlobalLock(global).cast::<u8>();
            if pointer.is_null() {
                let _ = GlobalFree(Some(global));
                return Err("GlobalLock failed while restoring clipboard".into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            let _ = GlobalUnlock(global);
            Ok(Self(Some(global)))
        }

        fn handle(&self) -> HANDLE {
            HANDLE(self.0.expect("clipboard allocation already transferred").0)
        }

        fn forget(mut self) {
            self.0 = None;
        }
    }

    impl Drop for OwnedGlobal {
        fn drop(&mut self) {
            if let Some(global) = self.0.take() {
                unsafe {
                    let _ = GlobalFree(Some(global));
                }
            }
        }
    }

    struct OwnedBitmap(Option<HANDLE>);

    impl OwnedBitmap {
        fn handle(&self) -> HANDLE {
            self.0.expect("bitmap already transferred")
        }

        fn forget(mut self) {
            self.0 = None;
        }
    }

    impl Drop for OwnedBitmap {
        fn drop(&mut self) {
            if let Some(bitmap) = self.0.take() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(bitmap.0));
                }
            }
        }
    }

    struct Transaction {
        text: String,
        formats: Mutex<Vec<SavedClipboardFormat>>,
        started: Instant,
        receipt: Mutex<Option<Instant>>,
        transcript_published: AtomicBool,
        done: SyncSender<Result<(), String>>,
    }

    enum StartDisposition {
        Published,
        TypeInstead(String),
    }

    enum TransactionStart {
        Published(mpsc::Receiver<Result<(), String>>),
        TypeInstead(String),
    }

    enum RestoreOutcome {
        Restored,
        ClipboardChanged,
        ClipboardBusy,
    }

    pub(super) fn paste(text: String) -> Result<()> {
        match start_transaction(text)? {
            TransactionStart::Published(done) => {
                send_ctrl_v()?;
                wait_for_transaction(done)
            }
            TransactionStart::TypeInstead(text) => super::type_text(&text),
        }
    }

    fn start_transaction(text: String) -> Result<TransactionStart> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("voicel-clipboard".into())
            .spawn(move || {
                let ready_error = ready_tx.clone();
                if let Err(error) = unsafe { clipboard_thread(text, ready_tx, done_tx.clone()) } {
                    let _ = ready_error.send(Err(error.to_string()));
                    let _ = done_tx.try_send(Err(error.to_string()));
                }
            })
            .context("start clipboard transaction")?;

        let disposition = ready_rx
            .recv_timeout(Duration::from_secs(1))
            .context("publish transcript to clipboard")?
            .map_err(anyhow::Error::msg)?;
        Ok(match disposition {
            StartDisposition::Published => TransactionStart::Published(done_rx),
            StartDisposition::TypeInstead(text) => TransactionStart::TypeInstead(text),
        })
    }

    fn wait_for_transaction(done: mpsc::Receiver<Result<(), String>>) -> Result<()> {
        match done
            .recv_timeout(Duration::from_secs(4))
            .context("wait for target application to read transcript")?
        {
            Ok(()) => Ok(()),
            Err(message) => bail!(message),
        }
    }

    unsafe fn clipboard_thread(
        text: String,
        ready: SyncSender<Result<StartDisposition, String>>,
        done: SyncSender<Result<(), String>>,
    ) -> Result<()> {
        register_window_class()?;
        let snapshot = match snapshot_clipboard() {
            Ok(snapshot) => snapshot,
            Err(_) => {
                let _ = ready.send(Ok(StartDisposition::TypeInstead(text)));
                return Ok(());
            }
        };
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
        let ClipboardSnapshot { sequence, formats } = snapshot;
        let transaction = Box::new(Transaction {
            text,
            formats: Mutex::new(formats),
            started: Instant::now(),
            receipt: Mutex::new(None),
            transcript_published: AtomicBool::new(true),
            done,
        });
        let transaction_ptr = Box::into_raw(transaction);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, transaction_ptr as isize);

        if let Err(publish_error) = publish_promise(hwnd, sequence) {
            let recovery = restore_after_publish_failure(hwnd, &*transaction_ptr);
            (*transaction_ptr)
                .transcript_published
                .store(false, Ordering::Release);
            let _ = DestroyWindow(hwnd);
            drop(Box::from_raw(transaction_ptr));
            if let Err(restore_error) = recovery {
                bail!(
                    "{publish_error:#}; additionally failed to restore clipboard: {restore_error}"
                );
            }
            return Err(publish_error);
        }
        let _ = ready.send(Ok(StartDisposition::Published));

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
                    if transaction.transcript_published.load(Ordering::Acquire) {
                        if let Ok(mut receipt) = transaction.receipt.lock() {
                            *receipt = Some(Instant::now());
                        }
                        render_text(&transaction.text);
                    }
                }
                LRESULT(0)
            }
            WM_RENDERALLFORMATS => {
                if !pointer.is_null() && OpenClipboard(Some(hwnd)).is_ok() {
                    let transaction = &*pointer;
                    if transaction.transcript_published.load(Ordering::Acquire)
                        && GetClipboardOwner().is_ok_and(|owner| owner == hwnd)
                    {
                        render_text(&transaction.text);
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
                        let result = match restore_clipboard(hwnd, transaction) {
                            Ok(RestoreOutcome::ClipboardBusy)
                                if transaction.started.elapsed() < RESTORE_TIMEOUT =>
                            {
                                None
                            }
                            Ok(RestoreOutcome::ClipboardBusy) => Some(Err(format!(
                                "Clipboard remained busy for {RESTORE_TIMEOUT:?} while restoring original formats"
                            ))),
                            Ok(RestoreOutcome::Restored | RestoreOutcome::ClipboardChanged) => {
                                Some(Ok(()))
                            }
                            Err(message) => Some(Err(message)),
                        };
                        if let Some(result) = result {
                            if let Err(message) = &result {
                                error!("Clipboard restore failed after delayed paste: {message}");
                            }
                            let _ = KillTimer(Some(hwnd), TIMER_ID);
                            transaction
                                .transcript_published
                                .store(false, Ordering::Release);
                            // Destroying a delayed-render owner synchronously delivers
                            // WM_RENDERALLFORMATS. Finish that teardown before callers can
                            // observe the restored clipboard.
                            let completion = match DestroyWindow(hwnd) {
                                Ok(()) => result,
                                Err(error) => Err(format!(
                                    "destroy clipboard transaction window before completion: {error}"
                                )),
                            };
                            let _ = transaction.done.try_send(completion);
                        }
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

    unsafe fn publish_promise(hwnd: HWND, expected_sequence: u32) -> Result<()> {
        open_clipboard_with_retry(Some(hwnd))?;
        let result = (|| {
            if GetClipboardSequenceNumber() != expected_sequence {
                bail!("Clipboard changed before Voicel could publish the transcript");
            }
            EmptyClipboard().context("clear clipboard for transcript")?;
            publish_monitor_opt_out_formats();
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

    unsafe fn publish_monitor_opt_out_formats() {
        for (name, value) in [
            ("ExcludeClipboardContentFromMonitorProcessing", 1_u32),
            ("CanIncludeInClipboardHistory", 0_u32),
            ("CanUploadToCloudClipboard", 0_u32),
        ] {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let format = RegisterClipboardFormatW(PCWSTR(wide.as_ptr()));
            if format == 0 {
                continue;
            }
            let Ok(global) = OwnedGlobal::allocate(&value.to_ne_bytes()) else {
                continue;
            };
            if SetClipboardData(format, Some(global.handle())).is_ok() {
                global.forget();
            }
        }
    }

    unsafe fn snapshot_clipboard() -> Result<ClipboardSnapshot> {
        open_clipboard_with_retry(None)?;
        let result = (|| {
            let mut formats = Vec::new();
            let mut format = 0_u32;
            loop {
                SetLastError(WIN32_ERROR::default());
                format = EnumClipboardFormats(format);
                if format == 0 {
                    let error = GetLastError();
                    if error.0 != 0 {
                        bail!("EnumClipboardFormats failed: {error:?}");
                    }
                    break;
                }
                if format == CF_BITMAP.0 as u32 {
                    let handle = GetClipboardData(format)
                        .with_context(|| format!("read bitmap clipboard format {format}"))?;
                    let copy = CopyImage(handle, IMAGE_BITMAP, 0, 0, Default::default())
                        .with_context(|| format!("copy bitmap clipboard format {format}"))?;
                    formats.push(SavedClipboardFormat::Bitmap(Some(OwnedBitmap(Some(copy)))));
                    continue;
                }
                ensure_format_can_be_snapshotted(format)?;
                let handle = GetClipboardData(format)
                    .with_context(|| format!("read clipboard format {format}"))?;
                let global = windows::Win32::Foundation::HGLOBAL(handle.0);
                let size = GlobalSize(global);
                if size == 0 {
                    bail!("Clipboard format {format} does not expose duplicable global memory");
                }
                if size > MAX_FORMAT_BYTES {
                    bail!(
                        "Clipboard format {format} exceeds the {MAX_FORMAT_BYTES}-byte preservation limit"
                    );
                }
                let pointer = GlobalLock(global);
                if pointer.is_null() {
                    bail!("GlobalLock failed for clipboard format {format}");
                }
                let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec();
                let _ = GlobalUnlock(global);
                formats.push(SavedClipboardFormat::Global(SavedFormat { format, bytes }));
            }
            Ok(ClipboardSnapshot {
                sequence: GetClipboardSequenceNumber(),
                formats,
            })
        })();
        let _ = CloseClipboard();
        result
    }

    unsafe fn restore_clipboard(
        hwnd: HWND,
        transaction: &Transaction,
    ) -> Result<RestoreOutcome, String> {
        if OpenClipboard(Some(hwnd)).is_err() {
            return Ok(RestoreOutcome::ClipboardBusy);
        }
        let result = (|| {
            let current_owner = GetClipboardOwner().ok();
            let current_sequence = GetClipboardSequenceNumber();
            let owner_matches = clipboard_matches_transaction(current_owner, hwnd);
            debug!(
                "Clipboard restore classification: current_owner={:?}, transaction_owner={}, sequence={}, action={}",
                current_owner.map(|owner| owner.0 as usize),
                hwnd.0 as usize,
                current_sequence,
                if owner_matches {
                    "restore"
                } else {
                    "preserve_newer_copy"
                }
            );
            if !owner_matches {
                return Ok(RestoreOutcome::ClipboardChanged);
            }
            let mut formats = transaction
                .formats
                .lock()
                .map_err(|_| "Clipboard snapshot lock was poisoned".to_string())?;
            restore_open_clipboard(&mut formats)?;
            debug!("Clipboard restore outcome: restored");
            Ok(RestoreOutcome::Restored)
        })();
        let _ = CloseClipboard();
        result
    }

    unsafe fn restore_after_publish_failure(
        hwnd: HWND,
        transaction: &Transaction,
    ) -> Result<(), String> {
        open_clipboard_with_retry(Some(hwnd)).map_err(|error| error.to_string())?;
        let result = (|| {
            if GetClipboardOwner().ok() != Some(hwnd) {
                return Ok(());
            }
            let mut formats = transaction
                .formats
                .lock()
                .map_err(|_| "Clipboard snapshot lock was poisoned".to_string())?;
            restore_open_clipboard(&mut formats)
        })();
        let _ = CloseClipboard();
        result
    }

    unsafe fn restore_open_clipboard(
        formats: &mut Vec<SavedClipboardFormat>,
    ) -> Result<(), String> {
        let pending = prepare_clipboard_restore_formats(formats)?;
        EmptyClipboard().map_err(|error| error.to_string())?;
        for pending_format in pending {
            match pending_format {
                PendingClipboardFormat::Global { format, global } => {
                    if let Err(error) = SetClipboardData(format, Some(global.handle())) {
                        return Err(format!("restore clipboard format {format}: {error}"));
                    }
                    global.forget();
                }
                PendingClipboardFormat::Bitmap { bitmap } => {
                    if let Err(error) = SetClipboardData(CF_BITMAP.0 as u32, Some(bitmap.handle()))
                    {
                        return Err(format!("restore bitmap clipboard format: {error}"));
                    }
                    bitmap.forget();
                }
            }
        }
        formats.clear();
        Ok(())
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
            CF_DSPBITMAP.0,
            CF_DSPENHMETAFILE.0,
            CF_DSPMETAFILEPICT.0,
            CF_DSPTEXT.0,
            CF_ENHMETAFILE.0,
            CF_METAFILEPICT.0,
            CF_OWNERDISPLAY.0,
            CF_PALETTE.0,
        ]
        .iter()
        .any(|value| *value as u32 == format)
    }

    enum PendingClipboardFormat {
        Global { format: u32, global: OwnedGlobal },
        Bitmap { bitmap: OwnedBitmap },
    }

    fn prepare_clipboard_restore_formats(
        formats: &mut [SavedClipboardFormat],
    ) -> Result<Vec<PendingClipboardFormat>, String> {
        let mut pending = Vec::with_capacity(formats.len());
        for saved in formats.iter() {
            if let SavedClipboardFormat::Global(saved) = saved {
                let global = unsafe { OwnedGlobal::allocate(&saved.bytes) }?;
                pending.push(PendingClipboardFormat::Global {
                    format: saved.format,
                    global,
                });
            }
        }
        for saved in formats.iter_mut() {
            if let SavedClipboardFormat::Bitmap(bitmap) = saved {
                let bitmap = bitmap
                    .take()
                    .ok_or_else(|| "bitmap clipboard format was already transferred".to_string())?;
                pending.push(PendingClipboardFormat::Bitmap { bitmap });
            }
        }
        Ok(pending)
    }

    fn clipboard_matches_transaction(current_owner: Option<HWND>, expected_owner: HWND) -> bool {
        current_owner == Some(expected_owner)
    }

    fn ensure_format_can_be_snapshotted(format: u32) -> Result<()> {
        if is_non_memory_format(format) {
            bail!("Clipboard format {format} cannot be safely preserved by this transaction");
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use std::collections::BTreeMap;
        use std::sync::{Mutex, OnceLock};

        use anyhow::{Context, Result, anyhow, bail};
        use windows::Win32::Foundation::{HANDLE, HWND};
        use windows::Win32::Graphics::Gdi::{
            BITMAP, CreateBitmap, DeleteObject, GetBitmapBits, GetObjectW, HBITMAP, HGDIOBJ,
        };
        use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
        use windows::Win32::System::Ole::CF_DIB;
        use windows::core::w;

        use super::{
            CF_BITMAP, CF_DSPBITMAP, CF_UNICODETEXT, CloseClipboard, EmptyClipboard,
            EnumClipboardFormats, GetClipboardData, GetLastError, GlobalLock, GlobalSize,
            GlobalUnlock, OwnedGlobal, SavedClipboardFormat, SavedFormat, SetClipboardData,
            SetLastError, TransactionStart, WIN32_ERROR, clipboard_matches_transaction,
            ensure_format_can_be_snapshotted, open_clipboard_with_retry, restore_open_clipboard,
            snapshot_clipboard, start_transaction, wait_for_transaction,
        };

        const CF_HDROP_FORMAT: u32 = 15;
        const BITMAP_PIXELS: [u8; 4] = [0x11, 0x22, 0x33, 0x44];
        static CLIPBOARD_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

        #[derive(Debug, Eq, PartialEq)]
        struct ClipboardObservation {
            globals: BTreeMap<u32, Vec<u8>>,
            bitmap: Option<BitmapObservation>,
        }

        #[derive(Debug, Eq, PartialEq)]
        struct BitmapObservation {
            width: i32,
            height: i32,
            width_bytes: i32,
            planes: u16,
            bits_per_pixel: u16,
            pixels: Vec<u8>,
        }

        #[test]
        fn accepts_bitmap_and_hglobal_formats_but_rejects_unsupported_handles() {
            assert!(ensure_format_can_be_snapshotted(CF_BITMAP.0 as u32).is_ok());
            assert!(ensure_format_can_be_snapshotted(CF_DSPBITMAP.0 as u32).is_err());
            assert!(ensure_format_can_be_snapshotted(CF_UNICODETEXT.0 as u32).is_ok());
        }

        #[test]
        fn post_render_sequence_drift_does_not_block_restore() {
            let transaction_owner = HWND(0x1234_usize as *mut _);
            let sequence_before_render = 100;
            let sequence_after_render = 101;

            assert_ne!(sequence_before_render, sequence_after_render);
            assert!(clipboard_matches_transaction(
                Some(transaction_owner),
                transaction_owner
            ));
        }

        #[test]
        fn different_clipboard_owner_blocks_restore() {
            let transaction_owner = HWND(0x1234_usize as *mut _);
            let newer_owner = HWND(0x5678_usize as *mut _);

            assert!(!clipboard_matches_transaction(
                Some(newer_owner),
                transaction_owner
            ));
        }

        #[test]
        fn direct_delayed_render_restores_text_files_images_and_custom_formats() -> Result<()> {
            let _serial = CLIPBOARD_TEST_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .map_err(|_| anyhow!("clipboard transaction test lock was poisoned"))?;

            unsafe {
                let mut user_clipboard =
                    ClipboardGuard::capture().context("capture user's pre-test clipboard")?;
                let test_result = run_direct_transaction_test();
                let restore_result = user_clipboard
                    .restore()
                    .context("restore user's pre-test clipboard");

                match (test_result, restore_result) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(test_error), Ok(())) => Err(test_error),
                    (Ok(()), Err(restore_error)) => Err(restore_error),
                    (Err(test_error), Err(restore_error)) => Err(anyhow!(
                        "{test_error:#}; additionally failed to restore user's clipboard: {restore_error:#}"
                    )),
                }
            }
        }

        unsafe fn run_direct_transaction_test() -> Result<()> {
            let custom_format =
                RegisterClipboardFormatW(w!("Voicel.NativeClipboardAcceptance.Custom"));
            if custom_format == 0 {
                bail!("RegisterClipboardFormatW failed");
            }

            let fixture = acceptance_formats(custom_format);
            install_fixture(&fixture)?;
            let expected = observe_clipboard()?;
            assert_fixture_formats(&expected, custom_format);

            let done = match start_transaction("Dictated".to_owned())? {
                TransactionStart::Published(done) => done,
                TransactionStart::TypeInstead(_) => {
                    bail!("isolated fixture unexpectedly required typing fallback")
                }
            };
            let rendered = consume_unicode_text(std::time::Duration::from_millis(400))?;
            assert_eq!(rendered, "Dictated");
            wait_for_transaction(done)?;

            let actual = observe_clipboard()?;
            assert_eq!(actual, expected);

            let done = match start_transaction("Second dictation".to_owned())? {
                TransactionStart::Published(done) => done,
                TransactionStart::TypeInstead(_) => {
                    bail!("isolated fixture unexpectedly required typing fallback")
                }
            };
            assert_eq!(
                consume_unicode_text(std::time::Duration::ZERO)?,
                "Second dictation"
            );
            let later_clipboard = later_clipboard_formats(custom_format);
            install_fixture(&later_clipboard)?;
            let expected_later_clipboard = observe_clipboard()?;
            wait_for_transaction(done)?;
            assert_eq!(observe_clipboard()?, expected_later_clipboard);
            Ok(())
        }

        fn assert_fixture_formats(observation: &ClipboardObservation, custom_format: u32) {
            assert!(observation.globals.contains_key(&(CF_UNICODETEXT.0 as u32)));
            assert!(observation.globals.contains_key(&CF_HDROP_FORMAT));
            assert!(observation.globals.contains_key(&(CF_DIB.0 as u32)));
            assert!(observation.globals.contains_key(&custom_format));
            assert!(observation.bitmap.is_some());
        }

        struct ClipboardGuard {
            original: Option<Vec<SavedClipboardFormat>>,
        }

        impl ClipboardGuard {
            unsafe fn capture() -> Result<Self> {
                Ok(Self {
                    original: Some(snapshot_clipboard()?.formats),
                })
            }

            unsafe fn restore(&mut self) -> Result<()> {
                let Some(mut formats) = self.original.take() else {
                    return Ok(());
                };
                restore_formats(&mut formats).map_err(anyhow::Error::msg)
            }
        }

        impl Drop for ClipboardGuard {
            fn drop(&mut self) {
                let Some(mut formats) = self.original.take() else {
                    return;
                };
                unsafe {
                    let _ = restore_formats(&mut formats);
                }
            }
        }

        unsafe fn restore_formats(formats: &mut Vec<SavedClipboardFormat>) -> Result<(), String> {
            open_clipboard_with_retry(None).map_err(|error| error.to_string())?;
            let result = restore_open_clipboard(formats);
            let _ = CloseClipboard();
            result
        }

        unsafe fn install_fixture(formats: &[SavedFormat]) -> Result<()> {
            let globals = formats
                .iter()
                .map(|saved| {
                    OwnedGlobal::allocate(&saved.bytes)
                        .map(|global| (saved.format, global))
                        .map_err(anyhow::Error::msg)
                })
                .collect::<Result<Vec<_>>>()?;
            let bitmap = CreateBitmap(1, 1, 1, 32, Some(BITMAP_PIXELS.as_ptr().cast()));
            if bitmap.0.is_null() {
                bail!("CreateBitmap failed");
            }

            open_clipboard_with_retry(None)?;
            let result = (|| {
                EmptyClipboard().context("clear clipboard for isolated fixture")?;
                for (format, global) in globals {
                    SetClipboardData(format, Some(global.handle()))
                        .with_context(|| format!("set fixture clipboard format {format}"))?;
                    global.forget();
                }
                if let Err(error) = SetClipboardData(CF_BITMAP.0 as u32, Some(HANDLE(bitmap.0))) {
                    let _ = DeleteObject(HGDIOBJ(bitmap.0));
                    return Err(error).context("set fixture bitmap");
                }
                Ok(())
            })();
            let _ = CloseClipboard();
            result
        }

        unsafe fn consume_unicode_text(hold_open_for: std::time::Duration) -> Result<String> {
            open_clipboard_with_retry(None)?;
            let result = (|| {
                let handle = GetClipboardData(CF_UNICODETEXT.0 as u32)
                    .context("consume delayed CF_UNICODETEXT")?;
                let global = windows::Win32::Foundation::HGLOBAL(handle.0);
                let byte_len = GlobalSize(global);
                let pointer = GlobalLock(global).cast::<u16>();
                if pointer.is_null() {
                    bail!("GlobalLock failed for rendered transcript");
                }
                let units = std::slice::from_raw_parts(pointer, byte_len / 2);
                let text_len = units
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(units.len());
                let text =
                    String::from_utf16(&units[..text_len]).context("decode rendered transcript")?;
                let _ = GlobalUnlock(global);
                // A consumer may keep the clipboard open after reading. The restore must wait
                // for that legitimate read transaction instead of abandoning the snapshot.
                std::thread::sleep(hold_open_for);
                Ok(text)
            })();
            let _ = CloseClipboard();
            result
        }

        unsafe fn observe_clipboard() -> Result<ClipboardObservation> {
            open_clipboard_with_retry(None)?;
            let result = (|| {
                let mut globals = BTreeMap::new();
                let mut bitmap = None;
                let mut format = 0_u32;
                loop {
                    SetLastError(WIN32_ERROR::default());
                    format = EnumClipboardFormats(format);
                    if format == 0 {
                        let error = GetLastError();
                        if error.0 != 0 {
                            bail!("EnumClipboardFormats failed: {error:?}");
                        }
                        break;
                    }
                    if format == CF_BITMAP.0 as u32 {
                        let handle = GetClipboardData(format)
                            .context("read fixture bitmap clipboard format")?;
                        bitmap = Some(observe_bitmap(handle)?);
                        continue;
                    }
                    ensure_format_can_be_snapshotted(format)?;
                    let handle = GetClipboardData(format)
                        .with_context(|| format!("read fixture clipboard format {format}"))?;
                    let global = windows::Win32::Foundation::HGLOBAL(handle.0);
                    let size = GlobalSize(global);
                    if size > super::MAX_FORMAT_BYTES {
                        bail!("clipboard format {format} exceeds the preservation limit");
                    }
                    let pointer = GlobalLock(global);
                    if pointer.is_null() {
                        bail!("GlobalLock failed for clipboard format {format}");
                    }
                    let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec();
                    let _ = GlobalUnlock(global);
                    globals.insert(format, bytes);
                }
                Ok(ClipboardObservation { globals, bitmap })
            })();
            let _ = CloseClipboard();
            result
        }

        unsafe fn observe_bitmap(handle: HANDLE) -> Result<BitmapObservation> {
            let mut info = BITMAP::default();
            if GetObjectW(
                HGDIOBJ(handle.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some((&mut info as *mut BITMAP).cast()),
            ) == 0
            {
                bail!("GetObjectW failed for fixture bitmap");
            }

            let byte_len = (info.bmWidthBytes * info.bmHeight.abs()) as usize;
            let mut pixels = vec![0_u8; byte_len];
            if GetBitmapBits(
                HBITMAP(handle.0),
                byte_len as i32,
                pixels.as_mut_ptr().cast(),
            ) != byte_len as i32
            {
                bail!("GetBitmapBits failed for fixture bitmap");
            }

            Ok(BitmapObservation {
                width: info.bmWidth,
                height: info.bmHeight,
                width_bytes: info.bmWidthBytes,
                planes: info.bmPlanes,
                bits_per_pixel: info.bmBitsPixel,
                pixels,
            })
        }

        fn acceptance_formats(custom_format: u32) -> Vec<SavedFormat> {
            vec![
                SavedFormat {
                    format: CF_UNICODETEXT.0 as u32,
                    bytes: unicode_bytes("Hello."),
                },
                SavedFormat {
                    format: CF_HDROP_FORMAT,
                    bytes: hdrop_bytes(),
                },
                SavedFormat {
                    format: CF_DIB.0 as u32,
                    bytes: dib_bytes(),
                },
                SavedFormat {
                    format: custom_format,
                    bytes: b"custom clipboard payload\0".to_vec(),
                },
            ]
        }

        fn later_clipboard_formats(custom_format: u32) -> Vec<SavedFormat> {
            vec![
                SavedFormat {
                    format: CF_UNICODETEXT.0 as u32,
                    bytes: unicode_bytes("Copied after dictation"),
                },
                SavedFormat {
                    format: CF_HDROP_FORMAT,
                    bytes: hdrop_bytes(),
                },
                SavedFormat {
                    format: CF_DIB.0 as u32,
                    bytes: dib_bytes(),
                },
                SavedFormat {
                    format: custom_format,
                    bytes: b"later custom clipboard payload\0".to_vec(),
                },
            ]
        }

        fn unicode_bytes(text: &str) -> Vec<u8> {
            text.encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_le_bytes)
                .collect()
        }

        fn hdrop_bytes() -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&20_u32.to_le_bytes());
            bytes.extend_from_slice(&0_i32.to_le_bytes());
            bytes.extend_from_slice(&0_i32.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(&1_u32.to_le_bytes());
            for unit in "C:\\Voicel\\clipboard-acceptance.txt"
                .encode_utf16()
                .chain([0, 0])
            {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes
        }

        fn dib_bytes() -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&40_u32.to_le_bytes());
            bytes.extend_from_slice(&1_i32.to_le_bytes());
            bytes.extend_from_slice(&1_i32.to_le_bytes());
            bytes.extend_from_slice(&1_u16.to_le_bytes());
            bytes.extend_from_slice(&32_u16.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(&4_u32.to_le_bytes());
            bytes.extend_from_slice(&0_i32.to_le_bytes());
            bytes.extend_from_slice(&0_i32.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(&[0, 0, 255, 0]);
            bytes
        }
    }
}
