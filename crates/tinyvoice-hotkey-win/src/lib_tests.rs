//! Owner cleanup fixtures for the Windows hook boundary.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::sync::atomic::AtomicUsize;

#[derive(Debug, PartialEq, Eq)]
enum CleanupEvent {
    Failed,
    Stopped,
    Dropped,
}

#[derive(Debug)]
struct RetryOwner {
    attempts: usize,
    allow_cleanup: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    events: mpsc::Sender<CleanupEvent>,
    successful_cleanups: Arc<AtomicUsize>,
    joins: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}

impl HookOwner for RetryOwner {
    fn stop(&mut self) -> Result<(), HookError> {
        self.attempts += 1;
        if self.attempts < 3 || !self.allow_cleanup.load(Ordering::SeqCst) {
            let _ = self.events.send(CleanupEvent::Failed);
            return Err(HookError);
        }
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| HookError)?;
            self.joins.fetch_add(1, Ordering::SeqCst);
        }
        self.successful_cleanups.fetch_add(1, Ordering::SeqCst);
        let _ = self.events.send(CleanupEvent::Stopped);
        Ok(())
    }
}

impl Drop for RetryOwner {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        let _ = self.events.send(CleanupEvent::Dropped);
    }
}

#[test]
fn drop_transfers_persistently_failing_owner_to_retry_worker() {
    let reaper = spawn_reaper().unwrap();
    let (events_tx, events_rx) = mpsc::channel();
    let allow_cleanup = Arc::new(AtomicBool::new(false));
    let joins = Arc::new(AtomicUsize::new(0));
    let successful_cleanups = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let (release_worker_tx, release_worker_rx) = mpsc::channel();
    let worker = thread::spawn(move || release_worker_rx.recv().unwrap());
    let listener = Listener {
        events: None,
        overflow: Arc::new(AtomicBool::new(false)),
        owner: Some(Box::new(RetryOwner {
            attempts: 0,
            allow_cleanup: allow_cleanup.clone(),
            worker: Some(worker),
            events: events_tx,
            successful_cleanups: successful_cleanups.clone(),
            joins: joins.clone(),
            drops: drops.clone(),
        })),
        reaper,
    };

    // Drop returns even though the owner keeps failing cleanup. Ownership is
    // now held by the worker until the fixture permits successful cleanup.
    drop(listener);
    assert_eq!(events_rx.recv().unwrap(), CleanupEvent::Failed);
    assert_eq!(events_rx.recv().unwrap(), CleanupEvent::Failed);
    assert_eq!(successful_cleanups.load(Ordering::SeqCst), 0);
    assert_eq!(joins.load(Ordering::SeqCst), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    release_worker_tx.send(()).unwrap();
    allow_cleanup.store(true, Ordering::SeqCst);
    assert_eq!(events_rx.recv().unwrap(), CleanupEvent::Stopped);
    assert_eq!(events_rx.recv().unwrap(), CleanupEvent::Dropped);
    assert_eq!(successful_cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(joins.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
