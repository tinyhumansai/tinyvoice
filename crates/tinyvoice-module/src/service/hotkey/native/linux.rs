//! Linux/X11 listener backed by an owned X RECORD context and data socket.
use super::{NativeListener, NativeOwner};
use std::collections::HashSet;
use std::os::fd::AsFd;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use tinyvoice::hotkey::{ActivationMode, HotkeyCombination, Key};
use tinyvoice_bus::{HotkeyError, HotkeyRequest};
use x11rb::{
    connection::Connection,
    protocol::record::{self, ConnectionExt as _},
    rust_connection::RustConnection,
};
const CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);
const MAX_RECORD_FRAMES: usize = 256;

pub(super) fn start(request: &HotkeyRequest) -> Result<NativeListener, HotkeyError> {
    let combo =
        tinyvoice::hotkey::parse_hotkey(&request.key).map_err(|_| HotkeyError::InvalidRequest)?;
    if combo.trigger == Key::Function || combo.modifiers.contains(&Key::Function) {
        return Err(HotkeyError::Unsupported);
    }
    let (control, _) = x11rb::connect(None).map_err(|_| HotkeyError::Unsupported)?;
    rustix::net::sockopt::set_socket_timeout(
        control.stream(),
        rustix::net::sockopt::Timeout::Recv,
        Some(CONTROL_TIMEOUT),
    )
    .map_err(|_| HotkeyError::Unsupported)?;
    rustix::net::sockopt::set_socket_timeout(
        control.stream(),
        rustix::net::sockopt::Timeout::Send,
        Some(CONTROL_TIMEOUT),
    )
    .map_err(|_| HotkeyError::Unsupported)?;
    let (data, _) = x11rb::connect(None).map_err(|_| HotkeyError::Unsupported)?;
    let context = control
        .generate_id()
        .map_err(|_| HotkeyError::Unsupported)?;
    create_record_context(&control, context)?;
    let wake = rustix::io::dup(data.stream().as_fd()).map_err(|_| HotkeyError::Unsupported)?;
    let (events_tx, events) = mpsc::sync_channel(256);
    let overflow = Arc::new(AtomicBool::new(false));
    let worker_overflow = overflow.clone();
    let closing = Arc::new(AtomicBool::new(false));
    let worker_closing = closing.clone();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let mode = match request.mode {
        tinyvoice_bus::ActivationMode::Tap => ActivationMode::Tap,
        tinyvoice_bus::ActivationMode::Push => ActivationMode::Push,
    };
    let combo_for_thread = combo.clone();
    let worker = thread::Builder::new()
        .name("tinyvoice-hotkey-x11".into())
        .spawn(move || {
            let Ok(mut replies) = data.record_enable_context(context) else {
                let _ = ready_tx.send(Err(()));
                return;
            };
            let Ok(_first_reply) = take_start_reply(replies.next(), |reply| reply.category) else {
                let _ = ready_tx.send(Err(()));
                return;
            };
            let _ = ready_tx.send(Ok(()));
            let mut down = HashSet::new();
            while let Some(Ok(reply)) = replies.next() {
                if reply.category == 5 {
                    break;
                }
                process_record_bytes(
                    &reply.data,
                    &combo_for_thread,
                    mode,
                    &mut down,
                    &events_tx,
                    &worker_overflow,
                );
            }
            if !worker_closing.load(Ordering::SeqCst) {
                worker_overflow.store(true, Ordering::SeqCst);
            }
        })
        .map_err(|_| HotkeyError::Unsupported)?;
    let Ok(Ok(())) = ready_rx.recv() else {
        let mut owner = XRecordOwner {
            control: Box::new(control),
            context: Some(context),
            disable_sent: false,
            wake,
            worker: Some(worker),
        };
        let _ = owner.stop();
        return Err(HotkeyError::Unsupported);
    };
    let owner = XRecordOwner {
        control: Box::new(control),
        context: Some(context),
        disable_sent: false,
        wake,
        worker: Some(worker),
    };
    Ok(NativeListener::new(
        events,
        overflow,
        closing,
        Box::new(owner),
    ))
}

fn take_start_reply<T, E>(
    first: Option<Result<T, E>>,
    category: impl FnOnce(&T) -> u8,
) -> Result<T, ()> {
    // X RECORD reports StartOfData only after the server accepted the enable
    // request. Do not publish a running lease on a failed, ended, or unexpected
    // first reply.
    let Some(Ok(reply)) = first else {
        return Err(());
    };
    if category(&reply) == 4 {
        Ok(reply)
    } else {
        Err(())
    }
}

fn create_record_context(control: &RustConnection, context: u32) -> Result<(), HotkeyError> {
    let empty = record::Range8 { first: 0, last: 0 };
    let range = record::Range {
        core_requests: empty,
        core_replies: empty,
        ext_requests: record::ExtRange {
            major: empty,
            minor: record::Range16 { first: 0, last: 0 },
        },
        ext_replies: record::ExtRange {
            major: empty,
            minor: record::Range16 { first: 0, last: 0 },
        },
        delivered_events: empty,
        device_events: record::Range8 { first: 2, last: 3 },
        errors: empty,
        client_started: false,
        client_died: false,
    };
    control
        .record_create_context(context, 0, &[record::CS::ALL_CLIENTS.into()], &[range])
        .map_err(|_| HotkeyError::Unsupported)?
        .check()
        .map_err(|_| HotkeyError::Unsupported)?;
    Ok(())
}

#[derive(Debug)]
pub(super) struct XRecordOwner {
    control: Box<dyn RecordControl>,
    context: Option<u32>,
    disable_sent: bool,
    wake: rustix::fd::OwnedFd,
    worker: Option<thread::JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordCleanupError {
    ConnectionLost,
    Protocol,
}

trait RecordControl: std::fmt::Debug + Send {
    fn disable_context(&self, context: u32) -> Result<(), RecordCleanupError>;
    fn free_context(&self, context: u32) -> Result<(), RecordCleanupError>;
}

impl RecordControl for RustConnection {
    fn disable_context(&self, context: u32) -> Result<(), RecordCleanupError> {
        self.record_disable_context(context)
            .map_err(|_| RecordCleanupError::ConnectionLost)?
            .check()
            .map_err(|error| classify_reply_error(&error))
    }

    fn free_context(&self, context: u32) -> Result<(), RecordCleanupError> {
        self.record_free_context(context)
            .map_err(|_| RecordCleanupError::ConnectionLost)?
            .check()
            .map_err(|error| classify_reply_error(&error))
    }
}

fn classify_reply_error(error: &x11rb::errors::ReplyError) -> RecordCleanupError {
    match error {
        x11rb::errors::ReplyError::ConnectionError(_) => RecordCleanupError::ConnectionLost,
        x11rb::errors::ReplyError::X11Error(_) => RecordCleanupError::Protocol,
    }
}

impl NativeOwner for XRecordOwner {
    fn stop(&mut self) -> Result<(), HotkeyError> {
        let disable_result = if let Some(context) = self.context.filter(|_| !self.disable_sent) {
            self.disable_sent = true;
            self.control.disable_context(context)
        } else {
            Ok(())
        };
        let _ = rustix::net::shutdown(&self.wake, rustix::net::Shutdown::Both);
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| HotkeyError::CleanupFailed)?;
        }
        match disable_result {
            Ok(()) => {}
            Err(RecordCleanupError::ConnectionLost) => {
                self.context = None;
                return Ok(());
            }
            Err(RecordCleanupError::Protocol) => return Err(HotkeyError::CleanupFailed),
        }
        if let Some(context) = self.context {
            match self.control.free_context(context) {
                Ok(()) | Err(RecordCleanupError::ConnectionLost) => self.context = None,
                Err(RecordCleanupError::Protocol) => return Err(HotkeyError::CleanupFailed),
            }
        }
        Ok(())
    }
}

fn process_record_bytes(
    bytes: &[u8],
    combo: &HotkeyCombination,
    mode: ActivationMode,
    down: &mut HashSet<Key>,
    events: &mpsc::SyncSender<bool>,
    overflow: &AtomicBool,
) {
    let (frames, remainder) = bytes.as_chunks::<32>();
    if frames.len() > MAX_RECORD_FRAMES || !remainder.is_empty() {
        overflow.store(true, Ordering::SeqCst);
        return;
    }
    let mut index = 0;
    while index < frames.len() {
        let frame = frames[index];
        let kind = frame[0] & 0x7f;
        let code = frame[1];
        if kind == 3
            && frames.get(index + 1).is_some_and(|next| {
                next[0] & 0x7f == 2 && next[1] == code && next[4..8] == frame[4..8]
            })
        {
            // X11 auto-repeat is a same-time release/press pair. Preserve the
            // held state so repeats do not toggle tap or release push mode.
            index += 2;
            continue;
        }
        if matches!(kind, 2 | 3)
            && let Some(key) = key_from_x_keycode(u32::from(code))
            && let Some(pressed) = process_event(key, kind == 2, combo, mode, down)
            && events.try_send(pressed).is_err()
        {
            overflow.store(true, Ordering::SeqCst);
        }
        index += 1;
    }
}

fn process_event(
    key: Key,
    pressed: bool,
    combo: &HotkeyCombination,
    mode: ActivationMode,
    down: &mut HashSet<Key>,
) -> Option<bool> {
    if pressed {
        let is_new = down.insert(key);
        if !is_new
            || key != combo.trigger
            || !combo
                .modifiers
                .iter()
                .all(|modifier| down.contains(modifier))
        {
            return None;
        }
        return Some(true);
    }
    down.remove(&key);
    if key == combo.trigger {
        return Some(false);
    }
    if mode == ActivationMode::Push
        && combo.modifiers.contains(&key)
        && down.contains(&combo.trigger)
    {
        down.remove(&combo.trigger);
        return Some(false);
    }
    None
}

fn key_from_x_keycode(code: u32) -> Option<Key> {
    modifier_key(code)
        .or_else(|| navigation_key(code))
        .or_else(|| function_key(code))
        .or_else(|| character_key(code))
        .or_else(|| keypad_key(code))
}

fn modifier_key(code: u32) -> Option<Key> {
    Some(match code {
        64 => Key::Alt,
        108 => Key::AltGr,
        37 => Key::ControlLeft,
        105 => Key::ControlRight,
        133 => Key::MetaLeft,
        134 => Key::MetaRight,
        50 => Key::ShiftLeft,
        62 => Key::ShiftRight,
        _ => return None,
    })
}

fn navigation_key(code: u32) -> Option<Key> {
    Some(match code {
        22 => Key::Backspace,
        66 => Key::CapsLock,
        119 => Key::Delete,
        116 => Key::DownArrow,
        115 => Key::End,
        9 => Key::Escape,
        110 => Key::Home,
        77 => Key::NumLock,
        127 => Key::Pause,
        107 => Key::PrintScreen,
        113 => Key::LeftArrow,
        117 => Key::PageDown,
        112 => Key::PageUp,
        36 => Key::Return,
        114 => Key::RightArrow,
        78 => Key::ScrollLock,
        65 => Key::Space,
        23 => Key::Tab,
        111 => Key::UpArrow,
        118 => Key::Insert,
        _ => return None,
    })
}

fn function_key(code: u32) -> Option<Key> {
    Some(match code {
        67 => Key::F1,
        68 => Key::F2,
        69 => Key::F3,
        70 => Key::F4,
        71 => Key::F5,
        72 => Key::F6,
        73 => Key::F7,
        74 => Key::F8,
        75 => Key::F9,
        76 => Key::F10,
        95 => Key::F11,
        96 => Key::F12,
        _ => return None,
    })
}

fn character_key(code: u32) -> Option<Key> {
    Some(match code {
        49 => Key::BackQuote,
        10 => Key::Num1,
        11 => Key::Num2,
        12 => Key::Num3,
        13 => Key::Num4,
        14 => Key::Num5,
        15 => Key::Num6,
        16 => Key::Num7,
        17 => Key::Num8,
        18 => Key::Num9,
        19 => Key::Num0,
        20 => Key::Minus,
        21 => Key::Equal,
        24 => Key::KeyQ,
        25 => Key::KeyW,
        26 => Key::KeyE,
        27 => Key::KeyR,
        28 => Key::KeyT,
        29 => Key::KeyY,
        30 => Key::KeyU,
        31 => Key::KeyI,
        32 => Key::KeyO,
        33 => Key::KeyP,
        34 => Key::LeftBracket,
        35 => Key::RightBracket,
        38 => Key::KeyA,
        39 => Key::KeyS,
        40 => Key::KeyD,
        41 => Key::KeyF,
        42 => Key::KeyG,
        43 => Key::KeyH,
        44 => Key::KeyJ,
        45 => Key::KeyK,
        46 => Key::KeyL,
        47 => Key::SemiColon,
        48 => Key::Quote,
        51 => Key::BackSlash,
        94 => Key::IntlBackslash,
        52 => Key::KeyZ,
        53 => Key::KeyX,
        54 => Key::KeyC,
        55 => Key::KeyV,
        56 => Key::KeyB,
        57 => Key::KeyN,
        58 => Key::KeyM,
        59 => Key::Comma,
        60 => Key::Dot,
        61 => Key::Slash,
        _ => return None,
    })
}

fn keypad_key(code: u32) -> Option<Key> {
    Some(match code {
        104 => Key::KpReturn,
        82 => Key::KpMinus,
        86 => Key::KpPlus,
        63 => Key::KpMultiply,
        106 => Key::KpDivide,
        90 => Key::Kp0,
        87 => Key::Kp1,
        88 => Key::Kp2,
        89 => Key::Kp3,
        83 => Key::Kp4,
        84 => Key::Kp5,
        85 => Key::Kp6,
        79 => Key::Kp7,
        80 => Key::Kp8,
        81 => Key::Kp9,
        91 => Key::KpDelete,
        _ => return None,
    })
}

#[cfg(test)]
#[path = "linux_tests.rs"]
mod tests;
