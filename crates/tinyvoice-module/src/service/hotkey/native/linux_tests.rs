//! Tests for X11 key mapping and listener cleanup.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::collections::HashSet;

#[test]
fn local_xrecord_listener_cancels_blocked_reader_and_joins() {
    if std::env::var_os("DISPLAY").is_none() {
        return;
    }
    let request = HotkeyRequest {
        key: "ctrl+space".to_owned(),
        mode: tinyvoice_bus::ActivationMode::Push,
        source: tinyvoice_bus::HotkeySource::Native,
    };
    let mut listener = super::super::backend()
        .start(&request)
        .expect("Xvfb X RECORD startup");
    assert!(listener.events.try_recv().is_err());
    listener
        .stop()
        .expect("X RECORD stop disables context and joins reader");
}

#[test]
fn xrecord_stop_releases_context_after_control_connection_loss() {
    let mut owner = owner_with(FakeRecordControl {
        disable: Arc::new(std::sync::Mutex::new(Err(
            RecordCleanupError::ConnectionLost,
        ))),
        free: Arc::new(std::sync::Mutex::new(Ok(()))),
    });

    assert_eq!(owner.stop(), Ok(()));
    assert_eq!(owner.context, None);
}

#[test]
fn xrecord_stop_keeps_context_on_protocol_error() {
    let mut owner = owner_with(FakeRecordControl {
        disable: Arc::new(std::sync::Mutex::new(Err(RecordCleanupError::Protocol))),
        free: Arc::new(std::sync::Mutex::new(Ok(()))),
    });

    assert_eq!(owner.stop(), Err(HotkeyError::CleanupFailed));
    assert_eq!(owner.context, Some(1));
}

#[test]
fn xrecord_stop_releases_context_if_free_finds_connection_lost() {
    let mut owner = owner_with(FakeRecordControl {
        disable: Arc::new(std::sync::Mutex::new(Ok(()))),
        free: Arc::new(std::sync::Mutex::new(Err(
            RecordCleanupError::ConnectionLost,
        ))),
    });

    assert_eq!(owner.stop(), Ok(()));
    assert_eq!(owner.context, None);
}

#[test]
fn uncertain_x11_connection_errors_do_not_mean_the_context_was_released() {
    let timeout = x11rb::errors::ReplyError::ConnectionError(
        x11rb::errors::ConnectionError::IoError(std::io::Error::from(std::io::ErrorKind::TimedOut)),
    );
    let unknown =
        x11rb::errors::ReplyError::ConnectionError(x11rb::errors::ConnectionError::UnknownError);

    assert_eq!(
        classify_reply_error(&timeout),
        RecordCleanupError::Uncertain
    );
    assert_eq!(
        classify_reply_error(&unknown),
        RecordCleanupError::Uncertain
    );

    let closed =
        x11rb::errors::ReplyError::ConnectionError(x11rb::errors::ConnectionError::IoError(
            std::io::Error::from(std::io::ErrorKind::BrokenPipe),
        ));
    assert_eq!(
        classify_reply_error(&closed),
        RecordCleanupError::ConnectionLost
    );
}

#[test]
fn xrecord_stop_retains_context_after_timeout_and_retries_cleanup() {
    let control = FakeRecordControl {
        disable: Arc::new(std::sync::Mutex::new(Err(RecordCleanupError::Uncertain))),
        free: Arc::new(std::sync::Mutex::new(Ok(()))),
    };
    let mut owner = owner_with(control.clone());

    assert_eq!(owner.stop(), Err(HotkeyError::CleanupFailed));
    assert_eq!(owner.context, Some(1));

    *control.disable.lock().unwrap() = Ok(());
    assert_eq!(owner.stop(), Ok(()));
    assert_eq!(owner.context, None);
}

#[derive(Clone, Debug)]
struct FakeRecordControl {
    disable: Arc<std::sync::Mutex<Result<(), RecordCleanupError>>>,
    free: Arc<std::sync::Mutex<Result<(), RecordCleanupError>>>,
}

impl RecordControl for FakeRecordControl {
    fn disable_context(&self, _context: u32) -> Result<(), RecordCleanupError> {
        *self.disable.lock().unwrap()
    }

    fn free_context(&self, _context: u32) -> Result<(), RecordCleanupError> {
        *self.free.lock().unwrap()
    }
}

fn owner_with(control: FakeRecordControl) -> XRecordOwner {
    XRecordOwner {
        control: Box::new(control),
        context: Some(1),
        disable_sent: false,
        wake: std::fs::File::open("/dev/null").unwrap().into(),
        worker: None,
    }
}

#[test]
fn record_listener_rejects_failed_ended_and_unexpected_start_replies() {
    assert!(take_start_reply::<u8, ()>(None, |reply| *reply).is_err());
    assert!(take_start_reply(Some(Err(())), |reply: &u8| *reply).is_err());
    assert!(take_start_reply(Some(Ok::<u8, ()>(0)), |reply| *reply).is_err());
    assert_eq!(
        take_start_reply(Some(Ok::<u8, ()>(4)), |reply| *reply),
        Ok(4)
    );
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
    process_record_bytes(
        &bytes,
        &combo,
        ActivationMode::Push,
        &mut down,
        &sender,
        &overflow,
    );
    assert_eq!(receiver.try_iter().collect::<Vec<_>>(), vec![true, false]);
    assert!(!overflow.load(Ordering::SeqCst));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the complete physical-key map is one fixture"
)]
fn physical_x_keycodes_keep_the_existing_key_vocabulary() {
    let mappings = [
        (64, Key::Alt),
        (108, Key::AltGr),
        (22, Key::Backspace),
        (66, Key::CapsLock),
        (37, Key::ControlLeft),
        (105, Key::ControlRight),
        (119, Key::Delete),
        (116, Key::DownArrow),
        (127, Key::Pause),
        (107, Key::PrintScreen),
        (77, Key::NumLock),
        (78, Key::ScrollLock),
        (115, Key::End),
        (9, Key::Escape),
        (67, Key::F1),
        (76, Key::F10),
        (95, Key::F11),
        (96, Key::F12),
        (68, Key::F2),
        (69, Key::F3),
        (70, Key::F4),
        (71, Key::F5),
        (72, Key::F6),
        (73, Key::F7),
        (74, Key::F8),
        (75, Key::F9),
        (110, Key::Home),
        (113, Key::LeftArrow),
        (133, Key::MetaLeft),
        (134, Key::MetaRight),
        (117, Key::PageDown),
        (112, Key::PageUp),
        (36, Key::Return),
        (114, Key::RightArrow),
        (50, Key::ShiftLeft),
        (62, Key::ShiftRight),
        (65, Key::Space),
        (23, Key::Tab),
        (111, Key::UpArrow),
        (49, Key::BackQuote),
        (10, Key::Num1),
        (11, Key::Num2),
        (12, Key::Num3),
        (13, Key::Num4),
        (14, Key::Num5),
        (15, Key::Num6),
        (16, Key::Num7),
        (17, Key::Num8),
        (18, Key::Num9),
        (19, Key::Num0),
        (20, Key::Minus),
        (21, Key::Equal),
        (24, Key::KeyQ),
        (25, Key::KeyW),
        (26, Key::KeyE),
        (27, Key::KeyR),
        (28, Key::KeyT),
        (29, Key::KeyY),
        (30, Key::KeyU),
        (31, Key::KeyI),
        (32, Key::KeyO),
        (33, Key::KeyP),
        (34, Key::LeftBracket),
        (35, Key::RightBracket),
        (38, Key::KeyA),
        (39, Key::KeyS),
        (40, Key::KeyD),
        (41, Key::KeyF),
        (42, Key::KeyG),
        (43, Key::KeyH),
        (44, Key::KeyJ),
        (45, Key::KeyK),
        (46, Key::KeyL),
        (47, Key::SemiColon),
        (48, Key::Quote),
        (51, Key::BackSlash),
        (94, Key::IntlBackslash),
        (52, Key::KeyZ),
        (53, Key::KeyX),
        (54, Key::KeyC),
        (55, Key::KeyV),
        (56, Key::KeyB),
        (57, Key::KeyN),
        (58, Key::KeyM),
        (59, Key::Comma),
        (60, Key::Dot),
        (61, Key::Slash),
        (118, Key::Insert),
        (104, Key::KpReturn),
        (82, Key::KpMinus),
        (86, Key::KpPlus),
        (63, Key::KpMultiply),
        (106, Key::KpDivide),
        (90, Key::Kp0),
        (87, Key::Kp1),
        (88, Key::Kp2),
        (89, Key::Kp3),
        (83, Key::Kp4),
        (84, Key::Kp5),
        (85, Key::Kp6),
        (79, Key::Kp7),
        (80, Key::Kp8),
        (81, Key::Kp9),
        (91, Key::KpDelete),
    ];
    assert_key_mappings(&mappings);
    assert_eq!(key_from_x_keycode(0), None);
}

fn assert_key_mappings(mappings: &[(u32, Key)]) {
    for (keycode, expected) in mappings {
        assert_eq!(key_from_x_keycode(*keycode), Some(*expected));
        let combo = HotkeyCombination {
            modifiers: HashSet::new(),
            trigger: *expected,
        };
        let mut down = HashSet::new();
        assert_eq!(
            process_event(*expected, true, &combo, ActivationMode::Tap, &mut down,),
            Some(true),
            "X keycode {keycode} should activate its mapped key"
        );
        assert_eq!(
            process_event(*expected, false, &combo, ActivationMode::Tap, &mut down,),
            Some(false),
            "X keycode {keycode} should release its mapped key"
        );
    }
}

#[test]
fn push_modifier_release_ends_activation_before_trigger_release() {
    let combo = tinyvoice::hotkey::parse_hotkey("ctrl+space").unwrap();
    let mut down = HashSet::new();
    assert_eq!(
        process_event(
            Key::ControlLeft,
            true,
            &combo,
            ActivationMode::Push,
            &mut down
        ),
        None
    );
    assert_eq!(
        process_event(Key::Space, true, &combo, ActivationMode::Push, &mut down),
        Some(true)
    );
    assert_eq!(
        process_event(
            Key::ControlLeft,
            false,
            &combo,
            ActivationMode::Push,
            &mut down
        ),
        Some(false)
    );
    assert_eq!(
        process_event(Key::Space, false, &combo, ActivationMode::Push, &mut down),
        Some(false)
    );
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
    process_record_bytes(
        &bytes,
        &combo,
        ActivationMode::Push,
        &mut down,
        &sender,
        &overflow,
    );
    assert!(overflow.load(Ordering::SeqCst));
    assert_eq!(receiver.try_iter().collect::<Vec<_>>(), vec![true]);
}

#[test]
fn oversized_record_reply_reports_continuity_loss_without_partial_events() {
    let combo = tinyvoice::hotkey::parse_hotkey("space").unwrap();
    let bytes: Vec<_> = (0..=MAX_RECORD_FRAMES)
        .flat_map(|timestamp| key_event(2, 65, u32::try_from(timestamp).unwrap_or_default()))
        .collect();
    let (sender, receiver) = mpsc::sync_channel(8);
    let mut down = HashSet::new();
    let overflow = AtomicBool::new(false);

    process_record_bytes(
        &bytes,
        &combo,
        ActivationMode::Push,
        &mut down,
        &sender,
        &overflow,
    );

    assert!(overflow.load(Ordering::SeqCst));
    assert!(receiver.try_iter().next().is_none());
}

fn key_event(kind: u8, keycode: u8, timestamp: u32) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    bytes[0] = kind;
    bytes[1] = keycode;
    bytes[4..8].copy_from_slice(&timestamp.to_ne_bytes());
    bytes
}
