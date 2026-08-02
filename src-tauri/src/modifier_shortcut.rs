use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::Shortcut;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParsedShortcut {
    Standard(Shortcut),
    Modifiers(ModifierChord),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModifierBindings {
    pub toggle: Option<ModifierChord>,
    pub cancel: Option<ModifierChord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModifierChord(u8);

impl ModifierChord {
    const CONTROL: u8 = 1 << 0;
    const ALT: u8 = 1 << 1;
    const SHIFT: u8 = 1 << 2;
    const SUPER: u8 = 1 << 3;

    fn contains_only(self, required: Self) -> bool {
        self.0 & !required.0 == 0
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl ParsedShortcut {
    pub fn modifier_chord(self) -> Option<ModifierChord> {
        match self {
            Self::Standard(_) => None,
            Self::Modifiers(chord) => Some(chord),
        }
    }
}

pub fn parse_shortcut(value: &str, label: &str) -> Result<ParsedShortcut, String> {
    let tokens = value
        .split('+')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(format!("Invalid {label} shortcut: shortcut is empty"));
    }

    let mut modifier_bits = 0_u8;
    let mut all_modifiers = true;
    for token in &tokens {
        let bit = match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(ModifierChord::CONTROL),
            "alt" | "option" => Some(ModifierChord::ALT),
            "shift" => Some(ModifierChord::SHIFT),
            "meta" | "super" | "win" | "windows" | "cmd" | "command" => Some(ModifierChord::SUPER),
            _ => None,
        };
        if let Some(bit) = bit {
            modifier_bits |= bit;
        } else {
            all_modifiers = false;
            break;
        }
    }

    if all_modifiers {
        if modifier_bits.count_ones() < 2 {
            return Err(format!(
                "Invalid {label} shortcut: use at least two different modifier keys"
            ));
        }
        return Ok(ParsedShortcut::Modifiers(ModifierChord(modifier_bits)));
    }

    value
        .parse::<Shortcut>()
        .map(ParsedShortcut::Standard)
        .map_err(|error| format!("Invalid {label} shortcut: {error}"))
}

pub fn shortcuts_are_equal(left: ParsedShortcut, right: ParsedShortcut) -> bool {
    match (left, right) {
        (ParsedShortcut::Standard(left), ParsedShortcut::Standard(right)) => left == right,
        (ParsedShortcut::Modifiers(left), ParsedShortcut::Modifiers(right)) => left == right,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct BindingSnapshot {
    generation: u64,
    bindings: ModifierBindings,
}

pub struct ModifierShortcutManager {
    snapshot: Arc<RwLock<BindingSnapshot>>,
    running: Arc<AtomicBool>,
}

impl ModifierShortcutManager {
    pub fn start(app: AppHandle) -> Result<Self, String> {
        let snapshot = Arc::new(RwLock::new(BindingSnapshot::default()));
        let running = Arc::new(AtomicBool::new(true));

        #[cfg(windows)]
        {
            let worker_snapshot = Arc::clone(&snapshot);
            let worker_running = Arc::clone(&running);
            thread::Builder::new()
                .name("voicel-modifier-shortcuts".to_owned())
                .spawn(move || modifier_loop(app, worker_snapshot, worker_running))
                .map_err(|error| format!("Start modifier shortcut listener: {error}"))?;
        }

        #[cfg(not(windows))]
        let _ = app;

        Ok(Self { snapshot, running })
    }

    pub fn configure(&self, bindings: ModifierBindings) {
        let mut snapshot = self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.generation = snapshot.generation.wrapping_add(1);
        snapshot.bindings = bindings;
    }
}

impl Drop for ModifierShortcutManager {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ChordTracker {
    gesture_active: bool,
    armed: bool,
    tainted: bool,
}

impl ChordTracker {
    fn update(
        &mut self,
        required: Option<ModifierChord>,
        held: ModifierChord,
        non_modifier_participated: bool,
    ) -> bool {
        let Some(required) = required else {
            *self = Self::default();
            return false;
        };

        if non_modifier_participated && self.gesture_active {
            self.tainted = true;
        }

        if held.is_empty() {
            let trigger = self.gesture_active && self.armed && !self.tainted;
            *self = Self::default();
            return trigger;
        }

        if !self.gesture_active {
            self.gesture_active = true;
        }
        if !held.contains_only(required) || non_modifier_participated {
            self.tainted = true;
        }
        if held == required && !self.tainted {
            self.armed = true;
        }
        false
    }
}

#[cfg(windows)]
fn modifier_loop(app: AppHandle, shared: Arc<RwLock<BindingSnapshot>>, running: Arc<AtomicBool>) {
    const POLL_INTERVAL: Duration = Duration::from_millis(10);

    let mut generation = u64::MAX;
    let mut toggle_tracker = ChordTracker::default();
    let mut cancel_tracker = ChordTracker::default();
    while running.load(Ordering::Acquire) {
        let snapshot = *shared
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if snapshot.generation != generation {
            generation = snapshot.generation;
            toggle_tracker = ChordTracker::default();
            cancel_tracker = ChordTracker::default();
        }

        if snapshot.bindings.toggle.is_none() && snapshot.bindings.cancel.is_none() {
            thread::sleep(POLL_INTERVAL);
            continue;
        }

        let sample = windows_keyboard_sample();
        if toggle_tracker.update(
            snapshot.bindings.toggle,
            sample.modifiers,
            sample.non_modifier_participated,
        ) {
            crate::dispatch_toggle(app.clone());
        }
        if cancel_tracker.update(
            snapshot.bindings.cancel,
            sample.modifiers,
            sample.non_modifier_participated,
        ) {
            crate::dispatch_cancel(app.clone());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct KeyboardSample {
    modifiers: ModifierChord,
    non_modifier_participated: bool,
}

#[cfg(windows)]
fn windows_keyboard_sample() -> KeyboardSample {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
        VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
    };

    fn state(vk: i32) -> i16 {
        unsafe { GetAsyncKeyState(vk) }
    }
    fn down(vk: i32) -> bool {
        state(vk) & i16::MIN != 0
    }

    let mut modifiers = 0_u8;
    if down(i32::from(VK_LCONTROL.0)) || down(i32::from(VK_RCONTROL.0)) {
        modifiers |= ModifierChord::CONTROL;
    }
    if down(i32::from(VK_LMENU.0)) || down(i32::from(VK_RMENU.0)) {
        modifiers |= ModifierChord::ALT;
    }
    if down(i32::from(VK_LSHIFT.0)) || down(i32::from(VK_RSHIFT.0)) {
        modifiers |= ModifierChord::SHIFT;
    }
    if down(i32::from(VK_LWIN.0)) || down(i32::from(VK_RWIN.0)) {
        modifiers |= ModifierChord::SUPER;
    }

    let excluded = [
        i32::from(VK_SHIFT.0),
        i32::from(VK_CONTROL.0),
        i32::from(VK_MENU.0),
        i32::from(VK_LSHIFT.0),
        i32::from(VK_RSHIFT.0),
        i32::from(VK_LCONTROL.0),
        i32::from(VK_RCONTROL.0),
        i32::from(VK_LMENU.0),
        i32::from(VK_RMENU.0),
        i32::from(VK_LWIN.0),
        i32::from(VK_RWIN.0),
    ];
    let non_modifier_participated =
        (0x08_i32..=0xfe).any(|vk| !excluded.contains(&vk) && state(vk) & (i16::MIN | 1) != 0);

    KeyboardSample {
        modifiers: ModifierChord(modifiers),
        non_modifier_participated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(bits: u8) -> ModifierChord {
        ModifierChord(bits)
    }

    #[test]
    fn parses_modifier_only_and_standard_shortcuts() {
        assert_eq!(
            parse_shortcut("Ctrl+Alt", "recording").unwrap(),
            ParsedShortcut::Modifiers(chord(ModifierChord::CONTROL | ModifierChord::ALT))
        );
        assert!(matches!(
            parse_shortcut("Ctrl+Shift+Space", "recording").unwrap(),
            ParsedShortcut::Standard(_)
        ));
        assert!(
            parse_shortcut("Ctrl", "recording")
                .unwrap_err()
                .contains("at least two")
        );
    }

    #[test]
    fn exact_chord_triggers_once_after_every_modifier_releases() {
        let required = chord(ModifierChord::CONTROL | ModifierChord::ALT);
        let mut tracker = ChordTracker::default();
        assert!(!tracker.update(Some(required), chord(ModifierChord::CONTROL), false));
        assert!(!tracker.update(Some(required), required, false));
        assert!(!tracker.update(Some(required), chord(ModifierChord::ALT), false));
        assert!(tracker.update(Some(required), chord(0), false));
        assert!(!tracker.update(Some(required), chord(0), false));
    }

    #[test]
    fn extra_modifier_or_main_key_taints_the_whole_gesture() {
        let required = chord(ModifierChord::CONTROL | ModifierChord::ALT);
        let mut tracker = ChordTracker::default();
        assert!(!tracker.update(Some(required), required, false));
        assert!(!tracker.update(Some(required), required, true));
        assert!(!tracker.update(Some(required), chord(0), false));

        assert!(!tracker.update(
            Some(required),
            chord(ModifierChord::CONTROL | ModifierChord::ALT | ModifierChord::SHIFT),
            false
        ));
        assert!(!tracker.update(Some(required), required, false));
        assert!(!tracker.update(Some(required), chord(0), false));
    }
}
