//! Regression tests for owner exit and retryable Windows hook cleanup.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{keyboard_callback, resolve_callback_module};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::Duration;
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};

#[test]
fn unexpected_message_loop_exit_marks_continuity_lost() {
    let overflow = AtomicBool::new(false);
    mark_unexpected_exit(1, &overflow);
    assert!(!overflow.load(Ordering::SeqCst));
    mark_unexpected_exit(0, &overflow);
    assert!(overflow.load(Ordering::SeqCst));
}

#[test]
fn hook_registration_resolves_the_module_containing_its_callback() {
    let callback_address = keyboard_callback as *const () as *const u16;
    let expected_module = std::ptr::NonNull::<std::ffi::c_void>::dangling().as_ptr();
    let module = resolve_callback_module(|flags, address, output| {
        assert_eq!(address, callback_address);
        assert_eq!(
            flags,
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT
        );
        // SAFETY: the helper provides a valid out pointer and the fixture only
        // writes a non-null sentinel HMODULE value to verify the lookup result.
        unsafe { *output = expected_module };
        1
    });

    assert_eq!(module, Ok(expected_module));
}

#[test]
fn stop_joins_an_owner_that_already_finished_after_unhook() {
    let mut worker = Some(std::thread::spawn(|| Ok(())));
    while !worker.as_ref().unwrap().is_finished() {
        std::thread::yield_now();
    }

    let result = stop_worker(
        &mut worker,
        || false,
        || panic!("finished owner has no reply"),
    );

    assert_eq!(result, Ok(()));
    assert!(worker.is_none());
}

#[test]
fn failed_unhook_reply_retains_worker_until_retry_completes_cleanup() {
    let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
    let mut worker = Some(std::thread::spawn(move || {
        release_rx
            .recv()
            .expect("fixture keeps the owner alive after unhook failure");
        Ok(())
    }));

    assert_eq!(
        stop_worker(&mut worker, || true, || Ok(Err(HookError))),
        Err(HookError)
    );
    assert!(
        worker.is_some(),
        "failed unhook cannot consume owner ownership"
    );

    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while !worker.as_ref().unwrap().is_finished() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(
        stop_worker(
            &mut worker,
            || false,
            || panic!("finished owner has no reply")
        ),
        Ok(())
    );
    assert!(worker.is_none());
}

#[test]
fn failed_stop_post_retains_live_owner() {
    let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
    let mut worker = Some(std::thread::spawn(move || {
        release_rx
            .recv()
            .expect("fixture keeps the owner alive after failed post");
        Ok(())
    }));

    assert_eq!(
        stop_worker(&mut worker, || false, || panic!("failed post has no reply")),
        Err(HookError)
    );
    assert!(worker.is_some());

    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while !worker.as_ref().unwrap().is_finished() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(
        stop_worker(
            &mut worker,
            || false,
            || panic!("finished owner has no reply")
        ),
        Ok(())
    );
}

#[test]
fn successful_unhook_reply_joins_owner_before_acknowledging_stop() {
    let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
    let mut worker = Some(std::thread::spawn(move || {
        release_rx
            .recv()
            .expect("fixture waits for the successful unhook reply");
        Ok(())
    }));

    let result = stop_worker(
        &mut worker,
        || true,
        || {
            release_tx.send(()).unwrap();
            Ok(Ok(()))
        },
    );
    assert_eq!(result, Ok(()));
    assert!(worker.is_none());
}

#[test]
fn reply_timeout_retains_live_owner_until_later_join() {
    let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
    let mut worker = Some(std::thread::spawn(move || {
        release_rx
            .recv()
            .expect("fixture keeps the owner alive after reply timeout");
        Ok(())
    }));

    assert_eq!(
        stop_worker(&mut worker, || true, || Err(())),
        Err(HookError)
    );
    assert!(worker.is_some());

    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while !worker.as_ref().unwrap().is_finished() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(
        stop_worker(
            &mut worker,
            || false,
            || panic!("finished owner has no reply")
        ),
        Ok(())
    );
}
