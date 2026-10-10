#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::collections::HashSet;

#[test]
fn local_xrecord_listener_cancels_blocked_reader_and_joins() {
    if std::env::var_os("DISPLAY").is_none() { return; }
    let request = HotkeyRequest {
        key: "ctrl+space".to_owned(),
        mode: tinyvoice_bus::ActivationMode::Push,
        source: tinyvoice_bus::HotkeySource::Native,
    };
    let mut listener = super::super::backend()
        .start(&request)
        .expect("Xvfb X RECORD startup");
    assert!(listener.events.try_recv().is_err());
    listener.stop().expect("X RECORD stop disables context and joins reader");
}

#[test]
fn autorepeat_release_press_pair_does_not_end_or_retoggle_activation() {
    let combo = tinyvoice::hotkey::parse_hotkey("space").expect("known key");
    let mut bytes = Vec::new();
    bytes.extend(key_event(2, 65, 10));
    bytes.extend(key_event(3, 65, 11));
    bytes.extend(key_event(2, 65, 11));
    bytes.extend(key_event(3, 65, 12));
    let (sender, receiver) = mpsc::sync_channel(8);
    let mut down = HashSet::new();
    let overflow = AtomicBool::new(false);
    process_record_bytes(&bytes, &combo, ActivationMode::Push, &mut down, &sender, &overflow);
    assert_eq!(receiver.try_iter().collect::<Vec<_>>(), vec![true, false]);
    assert!(!overflow.load(Ordering::SeqCst));
}

#[test]
fn physical_x_keycodes_keep_the_existing_key_vocabulary() {
    let mappings = [
        (64, Key::Alt), (37, Key::ControlLeft), (9, Key::Escape), (68, Key::F2),
        (110, Key::Home), (117, Key::PageDown), (50, Key::ShiftLeft), (49, Key::BackQuote),
        (10, Key::Num1), (20, Key::Minus), (24, Key::KeyQ), (34, Key::LeftBracket),
        (41, Key::KeyF), (47, Key::SemiColon), (52, Key::KeyZ), (118, Key::Insert),
        (104, Key::KpReturn), (85, Key::Kp6), (91, Key::KpDelete),
    ];
    for (keycode, expected) in mappings {
        assert_eq!(key_from_x_keycode(keycode), Some(expected));
    }
    assert_eq!(key_from_x_keycode(0), None);
}

#[test]
fn push_modifier_release_ends_activation_before_trigger_release() {
    let combo = tinyvoice::hotkey::parse_hotkey("ctrl+space").unwrap();
    let mut down = HashSet::new();
    assert_eq!(process_event(Key::ControlLeft, true, &combo, ActivationMode::Push, &mut down), None);
    assert_eq!(process_event(Key::Space, true, &combo, ActivationMode::Push, &mut down), Some(true));
    assert_eq!(process_event(Key::ControlLeft, false, &combo, ActivationMode::Push, &mut down), Some(false));
    assert_eq!(process_event(Key::Space, false, &combo, ActivationMode::Push, &mut down), Some(false));
}

#[test]
fn a_full_native_event_queue_reports_continuity_loss() {
    let combo = tinyvoice::hotkey::parse_hotkey("space").unwrap();
    let mut bytes = Vec::new();
    bytes.extend(key_event(2, 65, 1));
    bytes.extend(key_event(3, 65, 2));
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut down = HashSet::new();
    let overflow = AtomicBool::new(false);
    process_record_bytes(&bytes, &combo, ActivationMode::Push, &mut down, &sender, &overflow);
    assert!(overflow.load(Ordering::SeqCst));
    assert_eq!(receiver.try_iter().collect::<Vec<_>>(), vec![true]);
}

fn key_event(kind: u8, keycode: u8, timestamp: u32) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    bytes[0] = kind;
    bytes[1] = keycode;
    bytes[4..8].copy_from_slice(&timestamp.to_ne_bytes());
    bytes
}
