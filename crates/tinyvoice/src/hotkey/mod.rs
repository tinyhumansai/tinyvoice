//! Global hotkey listener using `rdev` (the `hotkey` feature).
//!
//! Monitors keyboard events system-wide and reports when a configurable key
//! combination is pressed or released. Supports two activation modes: **tap**
//! (toggle on press) and **push** (hold to record, release to stop).
//!
//! Nothing here records audio or knows about a voice pipeline; it turns raw key
//! events into [`HotkeyEvent`]s and leaves what they start to the host.
//!
//! macOS caveat: `rdev`'s `CGEventTap` callback calls
//! `TSMGetInputSourceProperty` off the main thread, which crashes on macOS 26.
//! A host targeting macOS should not start this listener there.

mod keys;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use log::{debug, info, warn};
use parking_lot::Mutex;
pub use rdev::Key;
use rdev::{Event, EventType, listen};
use tokio::sync::mpsc;

const LOG_PREFIX: &str = "[voice_hotkey]";
static LISTENER_STARTED: OnceLock<AtomicBool> = OnceLock::new();

/// Activation mode for the voice hotkey.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    /// Single press toggles recording on/off.
    Tap,
    /// Hold to record, release to stop.
    #[default]
    Push,
}

/// Events emitted by the hotkey listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// The hotkey was pressed (start recording).
    Pressed,
    /// The hotkey was released (stop recording — only relevant in Push mode).
    Released,
}

/// Parsed hotkey combination (e.g. Ctrl+Shift+Space).
#[derive(Debug, Clone)]
pub struct HotkeyCombination {
    /// Modifier keys that must be held.
    pub modifiers: HashSet<Key>,
    /// The primary trigger key.
    pub trigger: Key,
}

/// Handle to a running hotkey listener. Drop to stop.
#[derive(Debug)]
pub struct HotkeyListenerHandle {
    stop_flag: Arc<AtomicBool>,
    is_active: Arc<AtomicBool>,
    callback_lock: Arc<Mutex<()>>,
    event_sender: mpsc::UnboundedSender<HotkeyEvent>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyListenerHandle {
    #[cfg(test)]
    fn join_test_listener(&mut self) -> Option<std::thread::Result<()>> {
        self.thread.take().map(std::thread::JoinHandle::join)
    }

    /// Signal the listener to ignore further events.
    ///
    /// Note: this does **not** terminate the listener thread. `rdev::listen`
    /// blocks in the platform event loop and provides no cancellation API
    /// (rdev 0.5). The thread stays alive until the process exits; the
    /// stop flag merely causes the callback to discard all events.
    pub fn stop(&self) {
        let _callback_guard = self.callback_lock.lock();
        self.stop_flag.store(true, Ordering::SeqCst);
        if self.is_active.swap(false, Ordering::SeqCst) {
            let _ = self.event_sender.send(HotkeyEvent::Released);
        }
        info!("{LOG_PREFIX} hotkey listener signaled to skip events");
    }
}

impl Drop for HotkeyListenerHandle {
    fn drop(&mut self) {
        let _callback_guard = self.callback_lock.lock();
        self.stop_flag.store(true, Ordering::SeqCst);
        drop(self.thread.take());
        if self.is_active.swap(false, Ordering::SeqCst) {
            let _ = self.event_sender.send(HotkeyEvent::Released);
        }
    }
}

fn process_hotkey_event(
    event_type: EventType,
    hotkey: &HotkeyCombination,
    mode: ActivationMode,
    pressed_keys: &mut HashSet<Key>,
    is_active: &AtomicBool,
) -> Vec<HotkeyEvent> {
    let mut emitted = Vec::new();

    match event_type {
        EventType::KeyPress(key) => {
            let is_trigger = key == hotkey.trigger;
            let is_new_press = pressed_keys.insert(key);

            if !is_trigger || !is_new_press {
                return emitted;
            }

            if !hotkey.modifiers.iter().all(|m| pressed_keys.contains(m)) {
                return emitted;
            }

            let was_active = is_active.load(Ordering::SeqCst);
            debug!("{LOG_PREFIX} KeyPress trigger={key:?} was_active={was_active} mode={mode:?}");

            match mode {
                ActivationMode::Tap => {
                    if was_active {
                        is_active.store(false, Ordering::SeqCst);
                        info!("{LOG_PREFIX} tap → Released");
                        emitted.push(HotkeyEvent::Released);
                    } else {
                        is_active.store(true, Ordering::SeqCst);
                        info!("{LOG_PREFIX} tap → Pressed");
                        emitted.push(HotkeyEvent::Pressed);
                    }
                }
                ActivationMode::Push => {
                    if was_active {
                        is_active.store(false, Ordering::SeqCst);
                        info!("{LOG_PREFIX} push → Released (fallback, missed KeyRelease)");
                        emitted.push(HotkeyEvent::Released);
                    } else {
                        is_active.store(true, Ordering::SeqCst);
                        info!("{LOG_PREFIX} push → Pressed");
                        emitted.push(HotkeyEvent::Pressed);
                    }
                }
            }
        }
        EventType::KeyRelease(key) => {
            pressed_keys.remove(&key);

            if key != hotkey.trigger {
                if mode == ActivationMode::Push
                    && hotkey.modifiers.contains(&key)
                    && is_active.swap(false, Ordering::SeqCst)
                {
                    pressed_keys.remove(&hotkey.trigger);
                    info!("{LOG_PREFIX} push → Released (modifier released)");
                    emitted.push(HotkeyEvent::Released);
                }
                return emitted;
            }

            debug!(
                "{LOG_PREFIX} KeyRelease trigger={:?} is_active={}",
                key,
                is_active.load(Ordering::SeqCst)
            );

            if mode == ActivationMode::Push && is_active.swap(false, Ordering::SeqCst) {
                info!("{LOG_PREFIX} push → Released");
                emitted.push(HotkeyEvent::Released);
            }
        }
        _ => {}
    }

    emitted
}

/// Parse a hotkey string like "ctrl+shift+space" or "fn" into a `HotkeyCombination`.
///
/// Segments are `+`-separated, trimmed, and empty segments are ignored; the last
/// segment is the trigger and every earlier one a modifier.
///
/// # Errors
///
/// Returns a message when the string has no segments or names a key this crate
/// does not know.
pub fn parse_hotkey(hotkey_str: &str) -> crate::Result<HotkeyCombination> {
    let parts: Vec<&str> = hotkey_str
        .split('+')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return Err(crate::error::Error::EmptyHotkey);
    }

    let mut modifiers = HashSet::new();
    let mut trigger = None;

    for (i, part) in parts.iter().enumerate() {
        let key = keys::string_to_key(part)?;
        if i < parts.len() - 1 {
            modifiers.insert(key);
        } else {
            trigger = Some(key);
        }
    }

    let trigger = trigger.ok_or(crate::error::Error::EmptyHotkey)?;

    debug!("{LOG_PREFIX} parsed hotkey: modifiers={modifiers:?} trigger={trigger:?}");

    Ok(HotkeyCombination { modifiers, trigger })
}

/// Start the global hotkey listener.
///
/// Returns a handle (drop to stop) and a receiver for hotkey events.
/// The listener runs on a dedicated OS thread since `rdev::listen` is blocking.
///
/// # Errors
///
/// Returns a message when the listener thread cannot be spawned.
pub fn start_listener(
    hotkey: HotkeyCombination,
    mode: ActivationMode,
) -> crate::Result<(HotkeyListenerHandle, mpsc::UnboundedReceiver<HotkeyEvent>)> {
    start_listener_with(hotkey, mode, |callback| {
        if let Err(error) = listen(callback) {
            warn!("{LOG_PREFIX} rdev listen error: {error:?}");
        }
    })
}

fn start_listener_with(
    hotkey: HotkeyCombination,
    mode: ActivationMode,
    listen_events: impl FnOnce(Box<dyn Fn(Event) + Send>) + Send + 'static,
) -> crate::Result<(HotkeyListenerHandle, mpsc::UnboundedReceiver<HotkeyEvent>)> {
    let listener_started = LISTENER_STARTED.get_or_init(|| AtomicBool::new(false));
    listener_started
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .map_err(|_| crate::error::Error::HotkeyListenerAlreadyStarted)?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::unbounded_channel();
    let handle_tx = tx.clone();

    let stop_flag_clone = stop_flag.clone();
    let callback_lock = Arc::new(Mutex::new(()));
    let callback_lock_clone = callback_lock.clone();
    let pressed_keys: Arc<Mutex<HashSet<Key>>> = Arc::new(Mutex::new(HashSet::new()));
    let is_active = Arc::new(AtomicBool::new(false));
    let callback_is_active = is_active.clone();

    info!(
        "{LOG_PREFIX} starting hotkey listener, mode={mode:?}, trigger={:?}, modifiers={:?}",
        hotkey.trigger, hotkey.modifiers
    );

    let thread = match std::thread::Builder::new()
        .name("voice-hotkey".into())
        .spawn(move || {
            let callback = Box::new(move |event: Event| {
                let _callback_guard = callback_lock_clone.lock();
                if stop_flag_clone.load(Ordering::SeqCst) {
                    return;
                }
                let emitted = {
                    let mut keys = pressed_keys.lock();
                    process_hotkey_event(
                        event.event_type,
                        &hotkey,
                        mode,
                        &mut keys,
                        &callback_is_active,
                    )
                };
                for event in emitted {
                    let _ = tx.send(event);
                }
            });
            listen_events(callback);
        }) {
        Ok(thread) => thread,
        Err(error) => {
            listener_started.store(false, Ordering::SeqCst);
            return Err(crate::error::Error::HotkeyListenerSpawn(error.to_string()));
        }
    };

    Ok((
        HotkeyListenerHandle {
            stop_flag,
            is_active,
            callback_lock,
            event_sender: handle_tx,
            thread: Some(thread),
        },
        rx,
    ))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
