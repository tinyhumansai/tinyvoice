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
    let (sender, receiver) = std::sync::mpsc::sync_channel(256);
    let overflow = listener.overflow();
    let bridge_overflow = overflow.clone();
    let bridge = std::thread::Builder::new()
        .name("tinyvoice-hotkey-win-bridge".into())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                if sender.try_send(event).is_err() {
                    bridge_overflow.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        })
        .map_err(|_| HotkeyError::Unsupported)?;
    Ok(NativeListener::new(
        receiver,
        overflow,
        Arc::new(AtomicBool::new(false)),
        Box::new(WindowsOwner {
            listener,
            bridge: Some(bridge),
        }),
    ))
}

#[derive(Debug)]
struct WindowsOwner {
    listener: Listener,
    bridge: Option<std::thread::JoinHandle<()>>,
}

impl NativeOwner for WindowsOwner {
    fn stop(&mut self) -> Result<(), HotkeyError> {
        self.listener
            .stop()
            .map_err(|_| HotkeyError::CleanupFailed)?;
        if let Some(bridge) = self.bridge.take() {
            bridge.join().map_err(|_| HotkeyError::CleanupFailed)?;
        }
        Ok(())
    }
}

impl Drop for WindowsOwner {
    fn drop(&mut self) {
        while self.stop().is_err() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

fn virtual_key(key: Key) -> Option<u32> {
    Some(match key {
        Key::Alt => 0xA4,
        Key::AltGr => 0xA5,
        Key::Backspace => 0x08,
        Key::CapsLock => 0x14,
        Key::ControlLeft => 0xA2,
        Key::ControlRight => 0xA3,
        Key::Delete => 0x2E,
        Key::DownArrow => 0x28,
        Key::End => 0x23,
        Key::Escape => 0x1B,
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
        Key::Home => 0x24,
        Key::LeftArrow => 0x25,
        Key::MetaLeft => 0x5B,
        Key::MetaRight => 0x5C,
        Key::PageDown => 0x22,
        Key::PageUp => 0x21,
        Key::Return | Key::KpReturn => 0x0D,
        Key::RightArrow => 0x27,
        Key::ShiftLeft => 0xA0,
        Key::ShiftRight => 0xA1,
        Key::Space => 0x20,
        Key::Tab => 0x09,
        Key::UpArrow => 0x26,
        Key::PrintScreen => 0x2C,
        Key::ScrollLock => 0x91,
        Key::Pause => 0x13,
        Key::NumLock => 0x90,
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
        Key::KeyA => b'A' as u32,
        Key::KeyB => b'B' as u32,
        Key::KeyC => b'C' as u32,
        Key::KeyD => b'D' as u32,
        Key::KeyE => b'E' as u32,
        Key::KeyF => b'F' as u32,
        Key::KeyG => b'G' as u32,
        Key::KeyH => b'H' as u32,
        Key::KeyI => b'I' as u32,
        Key::KeyJ => b'J' as u32,
        Key::KeyK => b'K' as u32,
        Key::KeyL => b'L' as u32,
        Key::KeyM => b'M' as u32,
        Key::KeyN => b'N' as u32,
        Key::KeyO => b'O' as u32,
        Key::KeyP => b'P' as u32,
        Key::KeyQ => b'Q' as u32,
        Key::KeyR => b'R' as u32,
        Key::KeyS => b'S' as u32,
        Key::KeyT => b'T' as u32,
        Key::KeyU => b'U' as u32,
        Key::KeyV => b'V' as u32,
        Key::KeyW => b'W' as u32,
        Key::KeyX => b'X' as u32,
        Key::KeyY => b'Y' as u32,
        Key::KeyZ => b'Z' as u32,
        Key::LeftBracket => 0xDB,
        Key::RightBracket => 0xDD,
        Key::SemiColon => 0xBA,
        Key::Quote => 0xDE,
        Key::BackSlash => 0xDC,
        Key::IntlBackslash => 0xE2,
        Key::Comma => 0xBC,
        Key::Dot => 0xBE,
        Key::Slash => 0xBF,
        Key::Insert => 0x2D,
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
        Key::Function | Key::Unknown(_) => return None,
    })
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
