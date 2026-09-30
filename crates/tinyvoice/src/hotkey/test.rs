//! Behavioral tests for hotkey parsing and activation events.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use std::sync::atomic::AtomicBool;

fn combo() -> HotkeyCombination {
    parse_hotkey("ctrl+space").expect("test hotkey")
}

#[test]
fn parse_simple_hotkey() {
    let combo = parse_hotkey("ctrl+shift+space").unwrap();
    assert_eq!(combo.trigger, Key::Space);
    assert!(combo.modifiers.contains(&Key::ControlLeft));
    assert!(combo.modifiers.contains(&Key::ShiftLeft));
}

#[test]
fn parse_single_key() {
    let combo = parse_hotkey("f5").unwrap();
    assert_eq!(combo.trigger, Key::F5);
    assert!(combo.modifiers.is_empty());
}

#[test]
fn parse_cmd_key() {
    let combo = parse_hotkey("cmd+space").unwrap();
    assert_eq!(combo.trigger, Key::Space);
    assert!(combo.modifiers.contains(&Key::MetaLeft));
}

#[test]
fn parse_function_key() {
    let combo = parse_hotkey("fn").unwrap();
    assert_eq!(combo.trigger, Key::Function);
    assert!(combo.modifiers.is_empty());
}

#[test]
fn parse_empty_errors() {
    assert!(parse_hotkey("").is_err());
}

#[test]
fn parse_unknown_key_errors() {
    assert!(parse_hotkey("ctrl+unknownkey").is_err());
}

#[test]
fn activation_mode_default_is_push() {
    assert_eq!(ActivationMode::default(), ActivationMode::Push);
}

#[test]
fn parse_hotkey_trims_and_ignores_empty_segments() {
    let combo = parse_hotkey("  ctrl +  + shift + space ").unwrap();
    assert_eq!(combo.trigger, Key::Space);
    assert!(combo.modifiers.contains(&Key::ControlLeft));
    assert!(combo.modifiers.contains(&Key::ShiftLeft));
    assert_eq!(combo.modifiers.len(), 2);
}

#[test]
fn parse_hotkey_supports_aliases_and_right_side_modifiers() {
    let combo = parse_hotkey("rctrl+rshift+return").unwrap();
    assert_eq!(combo.trigger, Key::Return);
    assert!(combo.modifiers.contains(&Key::ControlRight));
    assert!(combo.modifiers.contains(&Key::ShiftRight));
}

#[test]
fn parse_hotkey_rejects_whitespace_only() {
    let err = parse_hotkey("   ").expect_err("whitespace-only hotkey should fail");
    assert_eq!(err, crate::error::Error::EmptyHotkey);
}

#[test]
fn process_hotkey_event_push_requires_modifier_then_releases() {
    let combo = combo();
    let is_active = AtomicBool::new(false);
    let mut pressed = HashSet::new();

    let no_emit = process_hotkey_event(
        EventType::KeyPress(Key::Space),
        &combo,
        ActivationMode::Push,
        &mut pressed,
        &is_active,
    );
    assert!(no_emit.is_empty());
    process_hotkey_event(
        EventType::KeyRelease(Key::Space),
        &combo,
        ActivationMode::Push,
        &mut pressed,
        &is_active,
    );

    process_hotkey_event(
        EventType::KeyPress(Key::ControlLeft),
        &combo,
        ActivationMode::Push,
        &mut pressed,
        &is_active,
    );
    let pressed_event = process_hotkey_event(
        EventType::KeyPress(Key::Space),
        &combo,
        ActivationMode::Push,
        &mut pressed,
        &is_active,
    );
    assert_eq!(pressed_event, vec![HotkeyEvent::Pressed]);

    let release_event = process_hotkey_event(
        EventType::KeyRelease(Key::Space),
        &combo,
        ActivationMode::Push,
        &mut pressed,
        &is_active,
    );
    assert_eq!(release_event, vec![HotkeyEvent::Released]);
}

#[test]
fn process_hotkey_event_push_repeat_does_not_release_while_held() {
    let combo = combo();
    let is_active = AtomicBool::new(false);
    let mut pressed = HashSet::from([Key::ControlLeft]);

    let first = process_hotkey_event(
        EventType::KeyPress(Key::Space),
        &combo,
        ActivationMode::Push,
        &mut pressed,
        &is_active,
    );
    let second = process_hotkey_event(
        EventType::KeyPress(Key::Space),
        &combo,
        ActivationMode::Push,
        &mut pressed,
        &is_active,
    );

    assert_eq!(first, vec![HotkeyEvent::Pressed]);
    assert!(second.is_empty());
}

#[test]
fn process_hotkey_event_push_releases_if_modifier_is_released_first() {
    let combo = combo();
    let is_active = AtomicBool::new(true);
    let mut pressed = HashSet::from([Key::ControlLeft, Key::Space]);

    let released = process_hotkey_event(
        EventType::KeyRelease(Key::ControlLeft),
        &combo,
        ActivationMode::Push,
        &mut pressed,
        &is_active,
    );

    assert_eq!(released, vec![HotkeyEvent::Released]);
    assert!(!pressed.contains(&Key::Space));
    assert!(!is_active.load(Ordering::SeqCst));
}

#[test]
fn process_hotkey_event_tap_toggles_after_a_complete_key_cycle() {
    let combo = parse_hotkey("space").unwrap();
    let is_active = AtomicBool::new(false);
    let mut pressed = HashSet::new();

    let started = process_hotkey_event(
        EventType::KeyPress(Key::Space),
        &combo,
        ActivationMode::Tap,
        &mut pressed,
        &is_active,
    );
    process_hotkey_event(
        EventType::KeyRelease(Key::Space),
        &combo,
        ActivationMode::Tap,
        &mut pressed,
        &is_active,
    );
    let stopped = process_hotkey_event(
        EventType::KeyPress(Key::Space),
        &combo,
        ActivationMode::Tap,
        &mut pressed,
        &is_active,
    );

    assert_eq!(started, vec![HotkeyEvent::Pressed]);
    assert_eq!(stopped, vec![HotkeyEvent::Released]);
}

#[test]
fn process_hotkey_event_push_falls_back_when_active_without_cached_trigger() {
    let combo = parse_hotkey("space").unwrap();
    let is_active = AtomicBool::new(true);
    let mut pressed = HashSet::new();

    let released = process_hotkey_event(
        EventType::KeyPress(Key::Space),
        &combo,
        ActivationMode::Push,
        &mut pressed,
        &is_active,
    );

    assert_eq!(released, vec![HotkeyEvent::Released]);
    assert!(!is_active.load(Ordering::SeqCst));
}

#[test]
fn process_hotkey_event_tap_ignores_repeated_key_down_while_held() {
    let combo = combo();
    let is_active = AtomicBool::new(false);
    let mut pressed = HashSet::from([Key::ControlLeft]);

    let first = process_hotkey_event(
        EventType::KeyPress(Key::Space),
        &combo,
        ActivationMode::Tap,
        &mut pressed,
        &is_active,
    );
    let second = process_hotkey_event(
        EventType::KeyPress(Key::Space),
        &combo,
        ActivationMode::Tap,
        &mut pressed,
        &is_active,
    );

    assert_eq!(first, vec![HotkeyEvent::Pressed]);
    assert!(second.is_empty());
}

#[test]
fn stop_emits_release_for_active_listener() {
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
    let is_active = Arc::new(AtomicBool::new(true));
    let handle = HotkeyListenerHandle {
        stop_flag: Arc::new(AtomicBool::new(false)),
        is_active,
        event_sender,
        _thread: None,
    };

    handle.stop();

    assert_eq!(event_receiver.try_recv(), Ok(HotkeyEvent::Released));
}

#[test]
fn drop_emits_release_for_active_listener() {
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
    let handle = HotkeyListenerHandle {
        stop_flag: Arc::new(AtomicBool::new(false)),
        is_active: Arc::new(AtomicBool::new(true)),
        event_sender,
        _thread: None,
    };

    drop(handle);

    assert_eq!(event_receiver.try_recv(), Ok(HotkeyEvent::Released));
}

#[test]
fn listener_forwards_activation_events_and_stops_callback_processing() {
    let (mut handle, mut events) = start_listener_with(combo(), ActivationMode::Push, |callback| {
        for event_type in [
            EventType::KeyPress(Key::ControlLeft),
            EventType::KeyPress(Key::Space),
            EventType::KeyRelease(Key::Space),
        ] {
            callback(Event {
                time: std::time::SystemTime::now(),
                name: None,
                event_type,
            });
        }
    })
    .expect("test listener should start");

    handle.join_test_listener();
    assert_eq!(events.try_recv(), Ok(HotkeyEvent::Pressed));
    assert_eq!(events.try_recv(), Ok(HotkeyEvent::Released));

    handle.stop();
    assert!(matches!(
        start_listener_with(combo(), ActivationMode::Push, |_| {}),
        Err(crate::error::Error::HotkeyListenerAlreadyStarted)
    ));
}
