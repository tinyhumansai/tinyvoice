//! Testable worker completion and continuity handling for the native hook.
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use crate::HookError;

pub(crate) fn mark_unexpected_exit(message_result: i32, overflow: &AtomicBool) {
    if message_result <= 0 {
        overflow.store(true, Ordering::SeqCst);
    }
}

fn join_finished(
    worker: &mut Option<JoinHandle<Result<(), HookError>>>,
) -> Option<Result<(), HookError>> {
    if !worker.as_ref().is_some_and(JoinHandle::is_finished) {
        return None;
    }
    let worker = worker.take()?;
    Some(worker.join().map_err(|_| HookError).and_then(|result| result))
}

pub(crate) fn stop_worker(
    worker: &mut Option<JoinHandle<Result<(), HookError>>>,
    mut post_stop: impl FnMut() -> bool,
    mut receive_reply: impl FnMut() -> Result<Result<(), HookError>, ()>,
) -> Result<(), HookError> {
    if worker.is_none() { return Ok(()); }
    if let Some(result) = join_finished(worker) {
        return result;
    }
    if !post_stop() {
        return join_finished(worker).unwrap_or(Err(HookError));
    }
    match receive_reply() {
        Ok(Ok(())) => {
            let Some(worker) = worker.take() else { return Ok(()); };
            worker.join().map_err(|_| HookError)?
        }
        Ok(Err(error)) => Err(error),
        Err(()) => join_finished(worker).unwrap_or(Err(HookError)),
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
