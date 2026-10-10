//! Windows adapter for the narrowly unsafe, owner-thread hook crate.
use super::{NativeListener, NativeOwner};
use std::sync::{Arc, atomic::AtomicBool};
use tinyvoice::hotkey::Key;
use tinyvoice_bus::{ActivationMode, HotkeyError, HotkeyRequest};
use tinyvoice_hotkey_win::{Chord, Listener};

pub(super) fn start(request: &HotkeyRequest) -> Result<NativeListener, HotkeyError> {
    let combination =
        tinyvoice::hotkey::parse_hotkey(&request.key).map_err(|_| HotkeyError::InvalidRequest)?;
    let trigger = virtual_key(combination.trigger).ok_or(HotkeyError::Unsupported)?;
    let modifiers = combination
        .modifiers
        .iter()
        .map(|key| virtual_key(*key).ok_or(HotkeyError::Unsupported))
        .collect::<Result<Vec<_>, _>>()?;
    let chord = Chord {
        trigger,
        modifiers,
        push: request.mode == ActivationMode::Push,
    };
    let mut listener = tinyvoice_hotkey_win::start(chord).map_err(|_| HotkeyError::Unsupported)?;
    let events = listener.take_events().ok_or(HotkeyError::Unsupported)?;
    let overflow = listener.overflow();
    Ok(NativeListener::new(
        events,
        overflow,
        Arc::new(AtomicBool::new(false)),
        Box::new(WindowsOwner { listener }),
    ))
}

#[derive(Debug)]
struct WindowsOwner {
    listener: Listener,
}

impl NativeOwner for WindowsOwner {
    fn stop(&mut self) -> Result<(), HotkeyError> {
        self.listener
            .stop()
            .map_err(|_| HotkeyError::CleanupFailed)?;
        Ok(())
    }
}

fn virtual_key(key: Key) -> Option<u32> {
    modifier_virtual_key(key)
        .or_else(|| navigation_virtual_key(key))
        .or_else(|| function_virtual_key(key))
        .or_else(|| printable_virtual_key(key))
        .or_else(|| keypad_virtual_key(key))
}

fn modifier_virtual_key(key: Key) -> Option<u32> {
    Some(match key {
        Key::Alt => 0xA4,
        Key::AltGr => 0xA5,
        Key::ControlLeft => 0xA2,
        Key::ControlRight => 0xA3,
        Key::MetaLeft => 0x5B,
        Key::MetaRight => 0x5C,
        Key::ShiftLeft => 0xA0,
        Key::ShiftRight => 0xA1,
        _ => return None,
    })
}

fn navigation_virtual_key(key: Key) -> Option<u32> {
    Some(match key {
        Key::Backspace => 0x08,
        Key::CapsLock => 0x14,
        Key::Delete => 0x2E,
        Key::DownArrow => 0x28,
        Key::End => 0x23,
        Key::Escape => 0x1B,
        Key::Home => 0x24,
        Key::LeftArrow => 0x25,
        Key::PageDown => 0x22,
        Key::PageUp => 0x21,
        Key::RightArrow => 0x27,
        Key::Space => 0x20,
        Key::Tab => 0x09,
        Key::UpArrow => 0x26,
        Key::PrintScreen => 0x2C,
        Key::ScrollLock => 0x91,
        Key::Pause => 0x13,
        Key::NumLock => 0x90,
        Key::Insert => 0x2D,
        Key::Return => 0x0D,
        _ => return None,
    })
}

fn function_virtual_key(key: Key) -> Option<u32> {
    Some(match key {
        Key::F1 => 0x70,
        Key::F2 => 0x71,
        Key::F3 => 0x72,
        Key::F4 => 0x73,
        Key::F5 => 0x74,
        Key::F6 => 0x75,
        Key::F7 => 0x76,
        Key::F8 => 0x77,
        Key::F9 => 0x78,
        Key::F10 => 0x79,
        Key::F11 => 0x7A,
        Key::F12 => 0x7B,
        _ => return None,
    })
}

fn printable_virtual_key(key: Key) -> Option<u32> {
    Some(match key {
        Key::BackQuote => 0xC0,
        Key::Num0 => 0x30,
        Key::Num1 => 0x31,
        Key::Num2 => 0x32,
        Key::Num3 => 0x33,
        Key::Num4 => 0x34,
        Key::Num5 => 0x35,
        Key::Num6 => 0x36,
        Key::Num7 => 0x37,
        Key::Num8 => 0x38,
        Key::Num9 => 0x39,
        Key::Minus => 0xBD,
        Key::Equal => 0xBB,
        Key::KeyA => u32::from(b'A'),
        Key::KeyB => u32::from(b'B'),
        Key::KeyC => u32::from(b'C'),
        Key::KeyD => u32::from(b'D'),
        Key::KeyE => u32::from(b'E'),
        Key::KeyF => u32::from(b'F'),
        Key::KeyG => u32::from(b'G'),
        Key::KeyH => u32::from(b'H'),
        Key::KeyI => u32::from(b'I'),
        Key::KeyJ => u32::from(b'J'),
        Key::KeyK => u32::from(b'K'),
        Key::KeyL => u32::from(b'L'),
        Key::KeyM => u32::from(b'M'),
        Key::KeyN => u32::from(b'N'),
        Key::KeyO => u32::from(b'O'),
        Key::KeyP => u32::from(b'P'),
        Key::KeyQ => u32::from(b'Q'),
        Key::KeyR => u32::from(b'R'),
        Key::KeyS => u32::from(b'S'),
        Key::KeyT => u32::from(b'T'),
        Key::KeyU => u32::from(b'U'),
        Key::KeyV => u32::from(b'V'),
        Key::KeyW => u32::from(b'W'),
        Key::KeyX => u32::from(b'X'),
        Key::KeyY => u32::from(b'Y'),
        Key::KeyZ => u32::from(b'Z'),
        Key::LeftBracket => 0xDB,
        Key::RightBracket => 0xDD,
        Key::SemiColon => 0xBA,
        Key::Quote => 0xDE,
        Key::BackSlash => 0xDC,
        Key::IntlBackslash => 0xE2,
        Key::Comma => 0xBC,
        Key::Dot => 0xBE,
        Key::Slash => 0xBF,
        _ => return None,
    })
}

fn keypad_virtual_key(key: Key) -> Option<u32> {
    Some(match key {
        Key::KpReturn => 0x0D,
        Key::KpMinus => 0x6D,
        Key::KpPlus => 0x6B,
        Key::KpMultiply => 0x6A,
        Key::KpDivide => 0x6F,
        Key::Kp0 => 0x60,
        Key::Kp1 => 0x61,
        Key::Kp2 => 0x62,
        Key::Kp3 => 0x63,
        Key::Kp4 => 0x64,
        Key::Kp5 => 0x65,
        Key::Kp6 => 0x66,
        Key::Kp7 => 0x67,
        Key::Kp8 => 0x68,
        Key::Kp9 => 0x69,
        Key::KpDelete => 0x6E,
        _ => return None,
    })
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
