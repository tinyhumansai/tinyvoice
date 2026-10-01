//! Key-name table for [`parse_hotkey`](super::parse_hotkey).

use rdev::Key;

/// Convert a string key name to an rdev Key.
pub(super) fn string_to_key(s: &str) -> crate::Result<Key> {
    match s.to_lowercase().as_str() {
        // Modifiers
        "ctrl" | "control" | "leftcontrol" => Ok(Key::ControlLeft),
        "rctrl" | "rightcontrol" => Ok(Key::ControlRight),
        "shift" | "leftshift" => Ok(Key::ShiftLeft),
        "rshift" | "rightshift" => Ok(Key::ShiftRight),
        "alt" | "option" | "leftalt" => Ok(Key::Alt),
        "ralt" | "rightaltoption" => Ok(Key::AltGr),
        "meta" | "super" | "cmd" | "command" | "leftmeta" => Ok(Key::MetaLeft),
        "rmeta" | "rsuper" | "rcmd" | "rightmeta" => Ok(Key::MetaRight),

        // Common keys
        "space" => Ok(Key::Space),
        "enter" | "return" => Ok(Key::Return),
        "tab" => Ok(Key::Tab),
        "escape" | "esc" => Ok(Key::Escape),
        "backspace" => Ok(Key::Backspace),
        "delete" | "del" => Ok(Key::Delete),
        "capslock" => Ok(Key::CapsLock),
        "fn" | "function" => Ok(Key::Function),

        // F-keys
        "f1" => Ok(Key::F1),
        "f2" => Ok(Key::F2),
        "f3" => Ok(Key::F3),
        "f4" => Ok(Key::F4),
        "f5" => Ok(Key::F5),
        "f6" => Ok(Key::F6),
        "f7" => Ok(Key::F7),
        "f8" => Ok(Key::F8),
        "f9" => Ok(Key::F9),
        "f10" => Ok(Key::F10),
        "f11" => Ok(Key::F11),
        "f12" => Ok(Key::F12),

        // Navigation
        "up" | "uparrow" => Ok(Key::UpArrow),
        "down" | "downarrow" => Ok(Key::DownArrow),
        "left" | "leftarrow" => Ok(Key::LeftArrow),
        "right" | "rightarrow" => Ok(Key::RightArrow),
        "home" => Ok(Key::Home),
        "end" => Ok(Key::End),
        "pageup" | "pgup" => Ok(Key::PageUp),
        "pagedown" | "pgdn" => Ok(Key::PageDown),
        "insert" | "ins" => Ok(Key::Insert),

        // Letters
        "a" => Ok(Key::KeyA),
        "b" => Ok(Key::KeyB),
        "c" => Ok(Key::KeyC),
        "d" => Ok(Key::KeyD),
        "e" => Ok(Key::KeyE),
        "f" => Ok(Key::KeyF),
        "g" => Ok(Key::KeyG),
        "h" => Ok(Key::KeyH),
        "i" => Ok(Key::KeyI),
        "j" => Ok(Key::KeyJ),
        "k" => Ok(Key::KeyK),
        "l" => Ok(Key::KeyL),
        "m" => Ok(Key::KeyM),
        "n" => Ok(Key::KeyN),
        "o" => Ok(Key::KeyO),
        "p" => Ok(Key::KeyP),
        "q" => Ok(Key::KeyQ),
        "r" => Ok(Key::KeyR),
        "s" => Ok(Key::KeyS),
        "t" => Ok(Key::KeyT),
        "u" => Ok(Key::KeyU),
        "v" => Ok(Key::KeyV),
        "w" => Ok(Key::KeyW),
        "x" => Ok(Key::KeyX),
        "y" => Ok(Key::KeyY),
        "z" => Ok(Key::KeyZ),

        // Numbers
        "0" => Ok(Key::Num0),
        "1" => Ok(Key::Num1),
        "2" => Ok(Key::Num2),
        "3" => Ok(Key::Num3),
        "4" => Ok(Key::Num4),
        "5" => Ok(Key::Num5),
        "6" => Ok(Key::Num6),
        "7" => Ok(Key::Num7),
        "8" => Ok(Key::Num8),
        "9" => Ok(Key::Num9),

        other => Err(crate::error::Error::UnknownHotkeyKey(other.to_owned())),
    }
}

#[cfg(test)]
#[path = "keys_test_tests.rs"]
mod test;
