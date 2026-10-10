#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tinyvoice_bus::{HostKeyFact, SequencedHostFact};

fn started(mode: ActivationMode) -> (Hotkeys, HotkeyHandle, u64) {
    let hotkeys = Hotkeys::default();
    let handle = hotkeys
        .reserve(HotkeyReserveRequest {
            request: HotkeyRequest {
                key: "Fn".into(),
                mode,
                source: HotkeySource::Host,
            },
        })
        .unwrap();
    let status = hotkeys
        .start(&HotkeyHandleRequest {
            handle: handle.clone(),
        })
        .unwrap();
    (hotkeys, handle, status.generation.unwrap())
}

fn feed(
    hotkeys: &Hotkeys,
    handle: &HotkeyHandle,
    generation: u64,
    facts: &[(u64, HostKeyFact)],
) -> HotkeyReply {
    hotkeys
        .feed(&HotkeyFeedRequest {
            handle: handle.clone(),
            generation,
            facts: facts
                .iter()
                .map(|(sequence, fact)| SequencedHostFact {
                    sequence: *sequence,
                    fact: *fact,
                })
                .collect(),
            overflow: false,
        })
        .unwrap()
}

#[test]
fn start_retry_and_duplicate_host_feed_are_idempotent() {
    let (hotkeys, handle, generation) = started(ActivationMode::Push);
    let retry = hotkeys
        .start(&HotkeyHandleRequest {
            handle: handle.clone(),
        })
        .unwrap();
    assert_eq!(retry.generation, Some(generation));
    assert!(feed(&hotkeys, &handle, generation, &[(1, HostKeyFact::Down)]).active);
    assert!(feed(&hotkeys, &handle, generation, &[(1, HostKeyFact::Down)]).active);
}

#[test]
fn expired_reservation_cannot_be_started_and_releases_capacity() {
    let hotkeys = Hotkeys::default();
    let request = HotkeyRequest {
        key: "Fn".into(),
        mode: ActivationMode::Push,
        source: HotkeySource::Host,
    };
    let handle = hotkeys
        .reserve(HotkeyReserveRequest {
            request: request.clone(),
        })
        .unwrap();
    hotkeys
        .leases
        .lock()
        .unwrap()
        .get_mut(&handle)
        .unwrap()
        .created = std::time::Instant::now()
        .checked_sub(RESERVATION_TTL)
        .expect("test TTL fits in the monotonic clock range");
    assert_eq!(
        hotkeys.start(&HotkeyHandleRequest {
            handle: handle.clone()
        }),
        Err(HotkeyError::UnknownHandle)
    );
    assert!(hotkeys.reserve(HotkeyReserveRequest { request }).is_ok());
}

#[derive(Debug, Default)]
struct FailOnceBackend(AtomicUsize);

impl native::Backend for FailOnceBackend {
    fn start(&self, _request: &HotkeyRequest) -> Result<native::NativeListener, HotkeyError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(HotkeyError::Unsupported);
        }
        let (sender, events) = std::sync::mpsc::sync_channel(1);
        drop(sender);
        Ok(native::NativeListener::new(
            events,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Box::new(NoopOwner),
        ))
    }
}

#[derive(Debug)]
struct NoopOwner;

impl native::NativeOwner for NoopOwner {
    fn stop(&mut self) -> Result<(), HotkeyError> {
        Ok(())
    }
}

#[test]
fn failed_native_start_restores_the_reservation_for_retry() {
    let hotkeys = Hotkeys::with_backend(Arc::new(FailOnceBackend::default()));
    let handle = hotkeys
        .reserve(HotkeyReserveRequest {
            request: HotkeyRequest {
                key: "ctrl+space".into(),
                mode: ActivationMode::Push,
                source: HotkeySource::Native,
            },
        })
        .unwrap();
    let request = HotkeyHandleRequest {
        handle: handle.clone(),
    };
    assert_eq!(hotkeys.start(&request), Err(HotkeyError::Unsupported));
    assert_eq!(
        hotkeys.leases.lock().unwrap()[&handle].state,
        HotkeyState::Reserved
    );
    assert_eq!(hotkeys.start(&request).unwrap().state, HotkeyState::Running);
    assert_eq!(hotkeys.stop(&request).unwrap().state, HotkeyState::Stopped);
}

#[test]
fn host_source_accepts_only_the_function_key_aliases() {
    let hotkeys = Hotkeys::default();
    let invalid = Hotkeys::default().reserve(HotkeyReserveRequest {
        request: HotkeyRequest {
            key: "ctrl+space".into(),
            mode: ActivationMode::Push,
            source: HotkeySource::Host,
        },
    });
    assert_eq!(invalid, Err(HotkeyError::InvalidRequest));
    assert!(
        hotkeys
            .reserve(HotkeyReserveRequest {
                request: HotkeyRequest {
                    key: "fn".into(),
                    mode: ActivationMode::Push,
                    source: HotkeySource::Host,
                }
            })
            .is_ok()
    );
}

#[test]
fn reservations_validate_keys_and_closed_leases_before_native_start() {
    let hotkeys = Hotkeys::default();
    for key in [String::new(), "x".repeat(MAX_KEY_BYTES + 1)] {
        assert_eq!(
            hotkeys.reserve(HotkeyReserveRequest {
                request: HotkeyRequest {
                    key,
                    mode: ActivationMode::Push,
                    source: HotkeySource::Host,
                },
            }),
            Err(HotkeyError::InvalidRequest)
        );
    }

    let native = hotkeys
        .reserve(HotkeyReserveRequest {
            request: HotkeyRequest {
                key: "ctrl+space".into(),
                mode: ActivationMode::Push,
                source: HotkeySource::Native,
            },
        })
        .unwrap();
    assert_eq!(
        hotkeys.feed(&HotkeyFeedRequest {
            handle: native.clone(),
            generation: 0,
            facts: vec![],
            overflow: false,
        }),
        Err(HotkeyError::InvalidRequest)
    );
    assert_eq!(
        hotkeys.read(&HotkeyReadRequest {
            handle: native.clone(),
            acknowledged_batch: None,
        }),
        Err(HotkeyError::Closed)
    );

    hotkeys.closed.store(true, Ordering::SeqCst);
    assert_eq!(
        hotkeys.start(&HotkeyHandleRequest { handle: native }),
        Err(HotkeyError::Closed)
    );
    assert_eq!(
        hotkeys.reserve(HotkeyReserveRequest {
            request: HotkeyRequest {
                key: "Fn".into(),
                mode: ActivationMode::Push,
                source: HotkeySource::Host,
            },
        }),
        Err(HotkeyError::Closed)
    );
}

#[test]
fn oversized_handles_are_rejected_before_lookup() {
    let hotkeys = Hotkeys::default();
    let request = HotkeyHandleRequest {
        handle: HotkeyHandle("x".repeat(MAX_HANDLE_BYTES + 1)),
    };
    assert_eq!(hotkeys.start(&request), Err(HotkeyError::LimitExceeded));
    assert_eq!(hotkeys.stop(&request), Err(HotkeyError::LimitExceeded));
    assert_eq!(
        hotkeys.read(&HotkeyReadRequest {
            handle: request.handle.clone(),
            acknowledged_batch: None
        }),
        Err(HotkeyError::LimitExceeded)
    );
    assert_eq!(
        hotkeys.feed(&HotkeyFeedRequest {
            handle: request.handle,
            generation: 0,
            facts: vec![],
            overflow: false
        }),
        Err(HotkeyError::LimitExceeded)
    );
}

#[test]
fn read_replays_identical_batch_until_exact_acknowledgment() {
    let (hotkeys, handle, generation) = started(ActivationMode::Push);
    feed(&hotkeys, &handle, generation, &[(1, HostKeyFact::Down)]);
    let first = hotkeys
        .read(&HotkeyReadRequest {
            handle: handle.clone(),
            acknowledged_batch: None,
        })
        .unwrap();
    let replay = hotkeys
        .read(&HotkeyReadRequest {
            handle: handle.clone(),
            acknowledged_batch: None,
        })
        .unwrap();
    assert_eq!(first, replay);
    let next = hotkeys
        .read(&HotkeyReadRequest {
            handle,
            acknowledged_batch: Some(first.batch),
        })
        .unwrap();
    assert!(next.events.is_empty());
    assert!(next.active);
}

#[test]
fn future_acknowledgment_does_not_mutate_the_retained_batch() {
    let (hotkeys, handle, generation) = started(ActivationMode::Push);
    feed(&hotkeys, &handle, generation, &[(1, HostKeyFact::Down)]);
    let request = HotkeyReadRequest {
        handle: handle.clone(),
        acknowledged_batch: None,
    };
    let first = hotkeys.read(&request).unwrap();
    assert_eq!(
        hotkeys.read(&HotkeyReadRequest {
            handle: handle.clone(),
            acknowledged_batch: Some(first.batch + 1),
        }),
        Err(HotkeyError::InvalidRequest)
    );
    assert_eq!(hotkeys.read(&request), Ok(first));
}

#[test]
fn output_overflow_resets_inactive_and_rearms_only_after_release() {
    let (hotkeys, handle, generation) = started(ActivationMode::Push);
    let mut facts = Vec::new();
    for sequence in 1..=257 {
        facts.push((
            sequence,
            if sequence % 2 == 1 {
                HostKeyFact::Down
            } else {
                HostKeyFact::Up
            },
        ));
    }
    for batch in facts.chunks(MAX_FEED_FACTS) {
        feed(&hotkeys, &handle, generation, batch);
    }
    let reset = hotkeys
        .read(&HotkeyReadRequest {
            handle: handle.clone(),
            acknowledged_batch: None,
        })
        .unwrap();
    assert!(reset.reset);
    assert!(!reset.active);
    assert!(reset.events.is_empty());
    assert!(!feed(&hotkeys, &handle, generation, &[(258, HostKeyFact::Up)]).active);
    assert!(feed(&hotkeys, &handle, generation, &[(259, HostKeyFact::Down)]).active);
}

#[test]
fn sequence_gap_resets_push_state_until_a_fresh_release() {
    let (hotkeys, handle, generation) = started(ActivationMode::Push);
    assert!(feed(&hotkeys, &handle, generation, &[(1, HostKeyFact::Down)]).active);
    let gap = hotkeys
        .feed(&HotkeyFeedRequest {
            handle: handle.clone(),
            generation,
            facts: vec![SequencedHostFact {
                sequence: 3,
                fact: HostKeyFact::Down,
            }],
            overflow: false,
        })
        .unwrap_err();
    assert_eq!(gap, HotkeyError::SequenceGap);
    let reset = hotkeys
        .read(&HotkeyReadRequest {
            handle: handle.clone(),
            acknowledged_batch: None,
        })
        .unwrap();
    assert!(reset.reset);
    assert!(!reset.active);
    assert!(!feed(&hotkeys, &handle, generation, &[(4, HostKeyFact::Down)]).active);
    assert!(!feed(&hotkeys, &handle, generation, &[(5, HostKeyFact::Up)]).active);
    assert!(feed(&hotkeys, &handle, generation, &[(6, HostKeyFact::Down)]).active);
}

#[test]
fn acknowledging_prior_batch_after_host_gap_returns_reset_snapshot() {
    let (hotkeys, handle, generation) = started(ActivationMode::Push);
    feed(&hotkeys, &handle, generation, &[(1, HostKeyFact::Down)]);
    let prior = hotkeys
        .read(&HotkeyReadRequest {
            handle: handle.clone(),
            acknowledged_batch: None,
        })
        .unwrap();
    assert!(prior.active);
    assert_eq!(prior.events.len(), 1);

    assert_eq!(
        hotkeys.feed(&HotkeyFeedRequest {
            handle: handle.clone(),
            generation,
            facts: vec![SequencedHostFact {
                sequence: 3,
                fact: HostKeyFact::Down
            }],
            overflow: false,
        }),
        Err(HotkeyError::SequenceGap)
    );

    let reset = hotkeys
        .read(&HotkeyReadRequest {
            handle: handle.clone(),
            acknowledged_batch: Some(prior.batch),
        })
        .expect("the outstanding batch acknowledgment remains valid across a gap");
    assert!(reset.reset);
    assert!(!reset.active);
    assert!(reset.events.is_empty());
}

#[test]
fn upstream_overflow_resets_active_listener_and_requires_release() {
    let (hotkeys, handle, generation) = started(ActivationMode::Push);
    assert!(feed(&hotkeys, &handle, generation, &[(1, HostKeyFact::Down)]).active);
    let status = hotkeys
        .feed(&HotkeyFeedRequest {
            handle: handle.clone(),
            generation,
            facts: vec![],
            overflow: true,
        })
        .unwrap();
    assert!(!status.active);
    assert!(status.continuity_lost);
    assert!(!feed(&hotkeys, &handle, generation, &[(2, HostKeyFact::Down)]).active);
    assert!(!feed(&hotkeys, &handle, generation, &[(3, HostKeyFact::Up)]).active);
    assert!(feed(&hotkeys, &handle, generation, &[(4, HostKeyFact::Down)]).active);
}

#[derive(Debug)]
struct EndedReaderBackend;

impl native::Backend for EndedReaderBackend {
    fn start(&self, _request: &HotkeyRequest) -> Result<native::NativeListener, HotkeyError> {
        let (events_tx, events) = std::sync::mpsc::sync_channel(8);
        events_tx
            .send(true)
            .expect("fixture queue accepts Down before EOF");
        drop(events_tx);
        let overflow = Arc::new(AtomicBool::new(true));
        Ok(native::NativeListener::new(
            events,
            overflow,
            Arc::new(AtomicBool::new(false)),
            Box::new(EndedReaderOwner),
        ))
    }
}

#[derive(Debug)]
struct EndedReaderOwner;

impl native::NativeOwner for EndedReaderOwner {
    fn stop(&mut self) -> Result<(), HotkeyError> {
        Ok(())
    }
}

#[test]
fn native_reader_eof_after_down_returns_inactive_reset_snapshot() {
    let hotkeys = Hotkeys::with_backend(Arc::new(EndedReaderBackend));
    let handle = hotkeys
        .reserve(HotkeyReserveRequest {
            request: HotkeyRequest {
                key: "ctrl+space".into(),
                mode: ActivationMode::Push,
                source: HotkeySource::Native,
            },
        })
        .unwrap();
    hotkeys
        .start(&HotkeyHandleRequest {
            handle: handle.clone(),
        })
        .unwrap();
    let snapshot = hotkeys
        .read(&HotkeyReadRequest {
            handle: handle.clone(),
            acknowledged_batch: None,
        })
        .unwrap();
    assert!(snapshot.reset, "EOF reports a continuity reset");
    assert!(
        !snapshot.active,
        "queued Down cannot survive an ended reader"
    );
    assert!(
        snapshot.events.is_empty(),
        "the incomplete event stream is discarded"
    );
    let after_ack = hotkeys
        .read(&HotkeyReadRequest {
            handle,
            acknowledged_batch: Some(snapshot.batch),
        })
        .unwrap();
    assert!(
        !after_ack.active,
        "stale queued Down is discarded with the reset snapshot"
    );
    assert!(after_ack.events.is_empty());
}

#[test]
fn tap_mode_repeats_toggle_without_changing_activation_alias_semantics() {
    let (hotkeys, handle, generation) = started(ActivationMode::Tap);
    assert!(feed(&hotkeys, &handle, generation, &[(1, HostKeyFact::Down)]).active);
    assert!(feed(&hotkeys, &handle, generation, &[(2, HostKeyFact::Down)]).active);
    assert!(feed(&hotkeys, &handle, generation, &[(3, HostKeyFact::Up)]).active);
    assert!(!feed(&hotkeys, &handle, generation, &[(4, HostKeyFact::Down)]).active);
}

#[derive(Debug, Default)]
struct NativeFixtureState {
    stop_calls: AtomicUsize,
    joined: AtomicBool,
}

#[derive(Debug)]
struct FakeBackend(Arc<NativeFixtureState>);

impl native::Backend for FakeBackend {
    fn start(&self, _request: &HotkeyRequest) -> Result<native::NativeListener, HotkeyError> {
        let (cancel, wait) = std::sync::mpsc::channel::<()>();
        let joined = self.0.clone();
        let worker = std::thread::spawn(move || {
            let _ = wait.recv();
            joined.joined.store(true, Ordering::SeqCst);
        });
        let (events_tx, events) = std::sync::mpsc::sync_channel(8);
        drop(events_tx);
        let owner = FakeOwner {
            state: self.0.clone(),
            cancel,
            worker: Some(worker),
        };
        Ok(native::NativeListener::new(
            events,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Box::new(owner),
        ))
    }
}

#[derive(Debug)]
struct FakeOwner {
    state: Arc<NativeFixtureState>,
    cancel: std::sync::mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl native::NativeOwner for FakeOwner {
    fn stop(&mut self) -> Result<(), HotkeyError> {
        let _ = self.cancel.send(());
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| HotkeyError::CleanupFailed)?;
        }
        if self.state.stop_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(HotkeyError::CleanupFailed);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct BlockingBackend {
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    starts: Arc<AtomicUsize>,
}

impl native::Backend for BlockingBackend {
    fn start(&self, _request: &HotkeyRequest) -> Result<native::NativeListener, HotkeyError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.entered.send(()).map_err(|_| HotkeyError::Closed)?;
        self.release
            .lock()
            .map_err(|_| HotkeyError::Closed)?
            .recv()
            .map_err(|_| HotkeyError::Closed)?;
        let (sender, events) = std::sync::mpsc::sync_channel(8);
        drop(sender);
        Ok(native::NativeListener::new(
            events,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Box::new(EndedReaderOwner),
        ))
    }
}

#[tokio::test]
async fn canceled_start_reply_leaves_running_lease_reachable_for_retry() {
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let starts = Arc::new(AtomicUsize::new(0));
    let hotkeys = Arc::new(Hotkeys::with_backend(Arc::new(BlockingBackend {
        entered: entered_tx,
        release: std::sync::Mutex::new(release_rx),
        starts: starts.clone(),
    })));
    let handle = hotkeys
        .reserve(HotkeyReserveRequest {
            request: HotkeyRequest {
                key: "ctrl+space".into(),
                mode: ActivationMode::Push,
                source: HotkeySource::Native,
            },
        })
        .unwrap();
    let request = HotkeyHandleRequest {
        handle: handle.clone(),
    };
    let first_hotkeys = hotkeys.clone();
    let canceled_waiter = tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || first_hotkeys.start(&request)).await;
    });
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    canceled_waiter.abort();
    release_tx.send(()).unwrap();

    let retry_hotkeys = hotkeys.clone();
    let retry_request = HotkeyHandleRequest { handle };
    let retried = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::task::spawn_blocking(move || retry_hotkeys.start(&retry_request)),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(retried.state, HotkeyState::Running);
    assert_eq!(retried.generation, Some(1));
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "retry must return the retained listener"
    );
}

#[test]
fn failed_native_cleanup_keeps_retryable_ownership_and_never_acknowledges_stop() {
    let state = Arc::new(NativeFixtureState::default());
    let hotkeys = Hotkeys::with_backend(Arc::new(FakeBackend(state.clone())));
    let handle = hotkeys
        .reserve(HotkeyReserveRequest {
            request: HotkeyRequest {
                key: "ctrl+space".into(),
                mode: ActivationMode::Push,
                source: HotkeySource::Native,
            },
        })
        .unwrap();
    hotkeys
        .start(&HotkeyHandleRequest {
            handle: handle.clone(),
        })
        .unwrap();
    assert_eq!(
        hotkeys.stop(&HotkeyHandleRequest {
            handle: handle.clone()
        }),
        Err(HotkeyError::CleanupFailed)
    );
    assert!(
        state.joined.load(Ordering::SeqCst),
        "stop waits for the native reader"
    );
    assert_eq!(
        hotkeys.stop(&HotkeyHandleRequest { handle }),
        Ok(HotkeyStatus {
            state: HotkeyState::Stopped,
            generation: Some(1),
            active: false,
            continuity_lost: true,
        })
    );
}
