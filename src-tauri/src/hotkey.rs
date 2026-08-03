//! One key to show the pet and hide it again.
//!
//! Reaching for the mouse to find a tray icon defeats the point of an overlay
//! you are supposed to glance at. A global hotkey is the only way to summon it
//! without leaving whatever you are doing.
//!
//! `RegisterHotKey` is exclusive across the whole machine: whoever asks first
//! owns the chord, and everybody else is told no. So the binding is a list
//! rather than a single value, and if the preferred chord is already spoken
//! for we take the next one and report which one we actually got. Silently
//! failing would leave a key that does nothing and no way to find out why.
//!
//! Raw `user32`, matching `desktop.rs`, rather than a crate for four calls.

use crate::state::load_config;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::AppHandle;

/// Tried in order. `Ctrl+Alt+P` first because it is rarely claimed and easy to
/// hit one-handed; the rest are progressively more obscure so that a machine
/// with a busy keymap still ends up with something.
const FALLBACKS: [&str; 4] = ["Ctrl+Alt+P", "Ctrl+Alt+K", "Ctrl+Shift+F10", "Ctrl+Alt+F10"];

/// Whatever chord we ended up with, for the menus to display.
#[derive(Default)]
pub struct Binding(pub Mutex<String>);

/// Registers the hotkey and starts pumping messages for it.
///
/// Returns the chord that was actually registered. The message loop lives on
/// its own thread for the life of the process, because `RegisterHotKey`
/// delivers to the thread that asked.
pub fn start(app: AppHandle) -> Result<String, String> {
    let preferred = load_config().hotkey;
    if preferred.trim().eq_ignore_ascii_case("off") || preferred.trim().is_empty() {
        return Err("turned off".to_string());
    }

    let mut candidates: Vec<String> = vec![preferred.trim().to_string()];
    for fallback in FALLBACKS {
        if !candidates.iter().any(|c| c.eq_ignore_ascii_case(fallback)) {
            candidates.push(fallback.to_string());
        }
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for spec in &candidates {
            let Some((modifiers, key)) = parse(spec) else {
                continue;
            };
            if !platform::register(modifiers, key) {
                continue;
            }
            let _ = tx.send(Ok(spec.clone()));
            platform::pump(&app);
            return;
        }
        let _ = tx.send(Err("every candidate chord is already taken".to_string()));
    });

    // The thread answers as soon as it has tried the list; the timeout only
    // matters if something has gone very wrong.
    rx.recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|_| Err("the hotkey thread did not answer".to_string()))
}

/// `Ctrl+Alt+P` into the modifier mask and virtual key code Windows wants.
///
/// Returns `None` for anything it does not recognise, so a hand-edited config
/// falls through to the defaults instead of registering something surprising.
fn parse(spec: &str) -> Option<(u32, u32)> {
    const MOD_ALT: u32 = 0x0001;
    const MOD_CONTROL: u32 = 0x0002;
    const MOD_SHIFT: u32 = 0x0004;
    const MOD_WIN: u32 = 0x0008;
    // Without this, holding the chord down repeats it as fast as the key
    // repeat rate, which for a toggle means the window strobes.
    const MOD_NOREPEAT: u32 = 0x4000;

    let mut modifiers = MOD_NOREPEAT;
    let mut key = None;
    for part in spec.split('+') {
        let part = part.trim();
        match part.to_ascii_lowercase().as_str() {
            "" => continue,
            "ctrl" | "control" => modifiers |= MOD_CONTROL,
            "alt" => modifiers |= MOD_ALT,
            "shift" => modifiers |= MOD_SHIFT,
            "win" | "super" | "meta" | "cmd" => modifiers |= MOD_WIN,
            other => {
                let code = virtual_key(other)?;
                // Two keys in one chord is not a thing Windows can register.
                if key.replace(code).is_some() {
                    return None;
                }
            }
        }
    }
    // A bare key with no modifier would swallow that key everywhere.
    if modifiers == MOD_NOREPEAT {
        return None;
    }
    Some((modifiers, key?))
}

fn virtual_key(name: &str) -> Option<u32> {
    if let Some(number) = name.strip_prefix('f') {
        if let Ok(n) = number.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(0x6F + n); // VK_F1 is 0x70
            }
        }
    }
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return match name {
            "space" => Some(0x20),
            "tab" => Some(0x09),
            "escape" | "esc" => Some(0x1B),
            "insert" => Some(0x2D),
            "home" => Some(0x24),
            "end" => Some(0x23),
            _ => None,
        };
    }
    if first.is_ascii_alphanumeric() {
        Some(first.to_ascii_uppercase() as u32)
    } else {
        None
    }
}

#[cfg(windows)]
mod platform {
    use tauri::{AppHandle, Manager};

    const WM_HOTKEY: u32 = 0x0312;
    const HOTKEY_ID: i32 = 0xB17E;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Msg {
        hwnd: usize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        point: Point,
        private: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterHotKey(hwnd: usize, id: i32, modifiers: u32, key: u32) -> i32;
        fn GetMessageW(msg: *mut Msg, hwnd: usize, min: u32, max: u32) -> i32;
    }

    pub fn register(modifiers: u32, key: u32) -> bool {
        // A null window means "deliver to this thread's message queue", which
        // is why the pump below has to be on the same thread.
        unsafe { RegisterHotKey(0, HOTKEY_ID, modifiers, key) != 0 }
    }

    pub fn pump(app: &AppHandle) {
        let mut msg = Msg::default();
        loop {
            let result = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
            if result <= 0 {
                return;
            }
            if msg.message == WM_HOTKEY {
                toggle(app);
            }
        }
    }

    fn toggle(app: &AppHandle) {
        let Some(window) = app.get_webview_window("pet") else {
            return;
        };
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            // Summoning it counts as having seen whatever it was blinking about.
            crate::attention::clear(app);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use tauri::AppHandle;

    pub fn register(_modifiers: u32, _key: u32) -> bool {
        false
    }

    pub fn pump(_app: &AppHandle) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOREPEAT: u32 = 0x4000;

    #[test]
    fn the_default_chord_parses() {
        let (modifiers, key) = parse("Ctrl+Alt+P").unwrap();
        assert_eq!(modifiers, NOREPEAT | 0x0002 | 0x0001);
        assert_eq!(key, 'P' as u32);
    }

    #[test]
    fn spelling_and_spacing_are_forgiving() {
        assert_eq!(parse("ctrl + alt + p"), parse("Ctrl+Alt+P"));
        assert_eq!(parse("CONTROL+ALT+p"), parse("Ctrl+Alt+P"));
        assert_eq!(parse("Win+Shift+K"), parse("super + shift + k"));
    }

    #[test]
    fn function_keys_work() {
        assert_eq!(parse("Ctrl+Shift+F10").unwrap().1, 0x79);
        assert_eq!(parse("Ctrl+F1").unwrap().1, 0x70);
        assert!(parse("Ctrl+F25").is_none());
    }

    /// A chord with no modifier would take that key away from every other
    /// program on the machine.
    #[test]
    fn a_bare_key_is_refused() {
        assert!(parse("P").is_none());
        assert!(parse("F5").is_none());
    }

    #[test]
    fn nonsense_falls_through_rather_than_guessing() {
        assert!(parse("").is_none());
        assert!(parse("Ctrl+").is_none());
        assert!(parse("Ctrl+Alt+Banana").is_none());
        assert!(parse("Ctrl+A+B").is_none(), "two keys is not a chord");
    }
}
