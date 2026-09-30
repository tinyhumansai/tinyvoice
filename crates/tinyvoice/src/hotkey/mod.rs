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
use std::sync::atomic::{AtomicBool, Ordering};

use log::{debug, info, warn};
use parking_lot::Mutex;
pub use rdev::Key;
use rdev::{Event, EventType, listen};
use tokio::sync::mpsc;

const LOG_PREFIX: &str = "[voice_hotkey]";

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
    _thread: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyListenerHandle {
    /// Signal the listener to ignore further events.
    ///
    /// Note: this does **not** terminate the listener thread. `rdev::listen`
    /// blocks in the platform event loop and provides no cancellation API
    /// (rdev 0.5). The thread stays alive until the process exits; the
    /// stop flag merely causes the callback to discard all events.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        info!("{LOG_PREFIX} hotkey listener signaled to skip events");
    }
}

impl Drop for HotkeyListenerHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
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
            pressed_keys.insert(key);

            if !is_trigger {
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
pub fn parse_hotkey(hotkey_str: &str) -> Result<HotkeyCombination, String> {
    let parts: Vec<&str> = hotkey_str
        .split('+')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return Err("hotkey string is empty".to_string());
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

    let trigger = trigger.ok_or_else(|| "no trigger key specified".to_string())?;

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
) -> Result<(HotkeyListenerHandle, mpsc::UnboundedReceiver<HotkeyEvent>), String> {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::unbounded_channel();

    let stop_flag_clone = stop_flag.clone();
    let pressed_keys: Arc<Mutex<HashSet<Key>>> = Arc::new(Mutex::new(HashSet::new()));
    let is_active = Arc::new(AtomicBool::new(false));

    info!(
        "{LOG_PREFIX} starting hotkey listener, mode={mode:?}, trigger={:?}, modifiers={:?}",
        hotkey.trigger, hotkey.modifiers
    );

    let thread = std::thread::Builder::new()
        .name("voice-hotkey".into())
        .spawn(move || {
            let callback = move |event: Event| {
                if stop_flag_clone.load(Ordering::SeqCst) {
                    return;
                }
                let emitted = {
                    let mut keys = pressed_keys.lock();
                    process_hotkey_event(event.event_type, &hotkey, mode, &mut keys, &is_active)
                };
                for event in emitted {
                    let _ = tx.send(event);
                }
            };

            if let Err(e) = listen(callback) {
                warn!("{LOG_PREFIX} rdev listen error: {e:?}");
            }
        })
        .map_err(|e| format!("failed to spawn hotkey listener thread: {e}"))?;

    Ok((
        HotkeyListenerHandle {
            stop_flag,
            _thread: Some(thread),
        },
        rx,
    ))
}

#[cfg(test)]
mod test;
