//! Small owning boundary for the Windows low-level keyboard hook.
//!
//! This crate is intentionally separate so the pure voice library and TinyBus
//! module can keep `unsafe_code = "forbid"`. The hook callback runs on the
//! installing thread's message loop, reads the Windows-owned event pointer only
//! during the callback, and forwards bounded facts with `try_send`. Stop is a
//! message to that owner thread; success follows `UnhookWindowsHookEx` and join.
//! A per-listener process-owned reaper retains the owner after final Drop when
//! cleanup fails. TinyBus intentionally keeps mapped module images loaded until
//! process exit, so the callback and reaper code remain mapped during cleanup.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows_sys::Win32::Foundation::{HMODULE, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleHandleExW,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN,
    WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

mod lifecycle;

const STOP_MESSAGE: u32 = WM_APP + 0x41;
const STOP_REPLY_TIMEOUT: Duration = Duration::from_secs(1);
const REAPER_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Physical Windows virtual-key chord in the existing TinyVoice key vocabulary.
#[derive(Debug, Clone)]
pub struct Chord {
    /// Trigger virtual key.
    pub trigger: u32,
    /// Required modifier virtual keys, preserving left/right identity.
    pub modifiers: Vec<u32>,
    /// Push releases when a required modifier lifts before the trigger.
    pub push: bool,
}

/// A bounded hook startup or cleanup operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookError;

/// An owned hook thread and its bounded activation fact receiver.
///
/// Explicit stop returns an error while retaining the owner for retry. Final
/// drop transfers failed cleanup to a process-owned reaper instead of blocking
/// the caller or detaching the Windows hook thread.
#[derive(Debug)]
pub struct Listener {
    events: Option<mpsc::Receiver<bool>>,
    overflow: Arc<AtomicBool>,
    owner: Option<Box<dyn HookOwner>>,
    reaper: Reaper,
}

#[derive(Debug)]
struct Reaper {
    sender: mpsc::Sender<Box<dyn HookOwner>>,
    _worker: JoinHandle<()>,
}

trait HookOwner: std::fmt::Debug + Send {
    fn stop(&mut self) -> Result<(), HookError>;
}

#[derive(Debug)]
struct WindowsOwner {
    thread_id: u32,
    worker: Option<JoinHandle<Result<(), HookError>>>,
    replies: mpsc::Receiver<Result<(), HookError>>,
}

impl HookOwner for WindowsOwner {
    fn stop(&mut self) -> Result<(), HookError> {
        lifecycle::stop_worker(
            &mut self.worker,
            || unsafe { PostThreadMessageW(self.thread_id, STOP_MESSAGE, 0, 0) != 0 },
            || {
                self.replies
                    .recv_timeout(STOP_REPLY_TIMEOUT)
                    .map_err(|_| ())
            },
        )
    }
}

fn spawn_reaper() -> Result<Reaper, HookError> {
    let (sender, receiver) = mpsc::channel::<Box<dyn HookOwner>>();
    let worker = thread::Builder::new()
        .name("tinyvoice-hotkey-reaper".into())
        .spawn(move || {
            while let Ok(mut owner) = receiver.recv() {
                loop {
                    let stopped =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.stop()))
                            .is_ok_and(|result| result.is_ok());
                    if stopped {
                        break;
                    }
                    thread::sleep(REAPER_RETRY_DELAY);
                }
            }
        })
        .map_err(|_| HookError)?;
    Ok(Reaper {
        sender,
        _worker: worker,
    })
}

impl Listener {
    /// Receives the bounded activation facts observed by the hook callback.
    pub fn take_events(&mut self) -> Option<mpsc::Receiver<bool>> {
        self.events.take()
    }
    /// Shared overflow bit, set when bounded callback handoff fills.
    pub fn overflow(&self) -> Arc<AtomicBool> {
        self.overflow.clone()
    }

    /// Unhooks on the owner thread and joins it before returning success.
    pub fn stop(&mut self) -> Result<(), HookError> {
        self.owner.as_mut().map_or(Ok(()), |owner| owner.stop())
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take()
            && let Err(error) = self.reaper.sender.send(owner)
        {
            // The per-listener reaper is created before hook installation, so
            // this is only a defensive fallback for an unexpectedly lost
            // reaper thread. Preserve the owner and never silently detach it.
            let mut owner = error.0;
            while owner.stop().is_err() {
                thread::sleep(REAPER_RETRY_DELAY);
            }
        }
    }
}

#[derive(Debug)]
struct CallbackState {
    chord: Chord,
    pressed: [bool; 256],
    sender: mpsc::SyncSender<bool>,
    overflow: Arc<AtomicBool>,
}

thread_local! { static CALLBACK: RefCell<Option<CallbackState>> = const { RefCell::new(None) }; }

/// Start a dedicated owner thread, wait for hook registration, and return its lease.
pub fn start(chord: Chord) -> Result<Listener, HookError> {
    // Establish an owned cleanup path before installing a callback that could
    // outlive the caller if synchronous cleanup later fails.
    let reaper = spawn_reaper()?;
    let (events_tx, events) = mpsc::sync_channel(256);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (reply_tx, replies) = mpsc::sync_channel(4);
    let overflow = Arc::new(AtomicBool::new(false));
    let worker_overflow = overflow.clone();
    let (thread_id_tx, thread_id_rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("tinyvoice-hotkey-win".into())
        .spawn(move || {
            owner_loop(
                chord,
                events_tx,
                worker_overflow,
                ready_tx,
                reply_tx,
                thread_id_tx,
            )
        })
        .map_err(|_| HookError)?;
    let thread_id = match thread_id_rx.recv() {
        Ok(thread_id) => thread_id,
        Err(_) => {
            let _ = worker.join();
            return Err(HookError);
        }
    };
    match ready_rx.recv() {
        Ok(Ok(())) => {}
        _ => {
            let _ = worker.join();
            return Err(HookError);
        }
    }
    Ok(Listener {
        events: Some(events),
        overflow,
        owner: Some(Box::new(WindowsOwner {
            thread_id,
            worker: Some(worker),
            replies,
        })),
        reaper,
    })
}

fn owner_loop(
    chord: Chord,
    sender: mpsc::SyncSender<bool>,
    overflow: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<(), HookError>>,
    replies: mpsc::SyncSender<Result<(), HookError>>,
    thread_id_sender: mpsc::SyncSender<u32>,
) -> Result<(), HookError> {
    let thread_id = unsafe { GetCurrentThreadId() };
    let mut message = MSG::default();
    // Force creation of this thread's message queue before the handle is published.
    unsafe {
        PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }
    let _ = thread_id_sender.send(thread_id);
    let Ok(module) = callback_module() else {
        let _ = ready.send(Err(HookError));
        return Err(HookError);
    };
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_callback), module, 0) };
    if hook.is_null() {
        let _ = ready.send(Err(HookError));
        return Err(HookError);
    }
    CALLBACK.with(|target| {
        *target.borrow_mut() = Some(CallbackState {
            chord,
            pressed: [false; 256],
            sender,
            overflow: overflow.clone(),
        })
    });
    if ready.send(Ok(())).is_err() {
        let result = unsafe { UnhookWindowsHookEx(hook) };
        CALLBACK.with(|target| *target.borrow_mut() = None);
        return if result != 0 { Ok(()) } else { Err(HookError) };
    }
    loop {
        let message_result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if message_result <= 0 {
            lifecycle::mark_unexpected_exit(message_result, &overflow);
            let unhooked = unsafe { UnhookWindowsHookEx(hook) };
            if unhooked != 0 {
                CALLBACK.with(|target| *target.borrow_mut() = None);
                let _ = replies.try_send(Ok(()));
                return Ok(());
            }
            let _ = replies.try_send(Err(HookError));
            if message_result < 0 {
                thread::sleep(Duration::from_millis(50));
            }
            continue;
        }
        if message.message == STOP_MESSAGE {
            let unhooked = unsafe { UnhookWindowsHookEx(hook) };
            if unhooked != 0 {
                CALLBACK.with(|target| *target.borrow_mut() = None);
                let _ = replies.try_send(Ok(()));
                return Ok(());
            }
            let _ = replies.try_send(Err(HookError));
        } else if message.message == WM_QUIT {
            lifecycle::mark_unexpected_exit(0, &overflow);
            let unhooked = unsafe { UnhookWindowsHookEx(hook) };
            if unhooked != 0 {
                CALLBACK.with(|target| *target.borrow_mut() = None);
                let _ = replies.try_send(Ok(()));
                return Ok(());
            }
            let _ = replies.try_send(Err(HookError));
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

fn callback_module() -> Result<HMODULE, HookError> {
    resolve_callback_module(|flags, address, module| unsafe {
        GetModuleHandleExW(flags, address, module)
    })
}

fn resolve_callback_module(
    lookup: impl FnOnce(u32, *const u16, *mut HMODULE) -> i32,
) -> Result<HMODULE, HookError> {
    let mut module = std::ptr::null_mut();
    // FROM_ADDRESS makes this pointer an address inside the module, not a
    // string. The hook must be attributed to the DLL containing its callback,
    // rather than the process executable that GetModuleHandleW(NULL) returns.
    let address = keyboard_callback as *const () as *const u16;
    let flags =
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if lookup(flags, address, &mut module) == 0 || module.is_null() {
        return Err(HookError);
    }
    Ok(module)
}

unsafe extern "system" fn keyboard_callback(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // Windows owns KBDLLHOOKSTRUCT for this callback invocation. We only
        // read the keycode synchronously and never retain the pointer.
        let event = unsafe { *(data as *const KBDLLHOOKSTRUCT) };
        let down = message == WM_KEYDOWN as usize || message == WM_SYSKEYDOWN as usize;
        let up = message == WM_KEYUP as usize || message == WM_SYSKEYUP as usize;
        if down || up {
            CALLBACK.with(|target| {
                if let Some(state) = target.borrow_mut().as_mut() {
                    let key = event.vkCode;
                    let index = key as usize;
                    if index >= state.pressed.len() {
                        return;
                    }
                    if down {
                        let fresh = !state.pressed[index];
                        state.pressed[index] = true;
                        if fresh
                            && key == state.chord.trigger
                            && state.chord.modifiers.iter().all(|modifier| {
                                state
                                    .pressed
                                    .get(*modifier as usize)
                                    .copied()
                                    .unwrap_or(false)
                            })
                            && state.sender.try_send(true).is_err()
                        {
                            state.overflow.store(true, Ordering::SeqCst);
                        }
                    } else {
                        state.pressed[index] = false;
                        if key == state.chord.trigger
                            || (state.chord.push
                                && state.chord.modifiers.contains(&key)
                                && state
                                    .pressed
                                    .get(state.chord.trigger as usize)
                                    .copied()
                                    .unwrap_or(false))
                        {
                            if state.chord.push && state.chord.modifiers.contains(&key) {
                                state.pressed[state.chord.trigger as usize] = false;
                            }
                            if state.sender.try_send(false).is_err() {
                                state.overflow.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
            });
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, message, data) }
}
