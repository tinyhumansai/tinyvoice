//! Startup faults and reclamation use in-memory resources, never devices.
use super::*;

fn fixture() -> Arc<Capture> {
    Arc::new(super::super::tests::fixture_capture())
}
fn poison<T>(mutex: &std::sync::Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock();
        std::panic::resume_unwind(Box::new("fixture poison"));
    }));
}
#[tokio::test]
async fn lost_reply_reclamation_stops_both_native_modes() -> CaptureResult<()> {
    for stream in [false, true] {
        let capture = fixture();
        let handle = capture.reserve(MicrophonePermission::Granted)?;
        let started = capture
            .start_reserved(
                RecordingStartRequest {
                    permission: MicrophonePermission::Granted,
                    handle: Some(handle),
                },
                stream,
            )
            .await?;
        let worker_capture = capture.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || worker_capture.discard_live(started, &runtime))
            .await
            .map_err(|_| state_error())?;
        assert!(!capture.busy.load(Ordering::SeqCst));
        assert!(
            capture
                .recordings
                .lock()
                .map_err(|_| state_error())?
                .is_empty()
        );
        assert!(
            capture
                .streams
                .lock()
                .map_err(|_| state_error())?
                .is_empty()
        );
    }
    Ok(())
}
#[tokio::test]
async fn poisoned_live_tables_stop_unpublished_resources_and_free_capacity() -> CaptureResult<()> {
    for stream in [false, true] {
        let capture = fixture();
        let handle = capture.reserve(MicrophonePermission::Granted)?;
        if stream {
            poison(&capture.streams);
        } else {
            poison(&capture.recordings);
        }
        assert!(matches!(
            capture
                .start_reserved(
                    RecordingStartRequest {
                        permission: MicrophonePermission::Granted,
                        handle: Some(handle)
                    },
                    stream
                )
                .await,
            Err(CaptureError::Device(_))
        ));
        assert!(!capture.busy.load(Ordering::SeqCst));
        capture.shutdown().await?;
    }
    Ok(())
}
#[tokio::test]
async fn poisoned_reservations_fail_closed_and_cleanup_unpublished_native_resources()
-> CaptureResult<()> {
    for stream in [false, true] {
        let capture = fixture();
        poison(&capture.reservations);
        assert!(matches!(
            capture.reserve(MicrophonePermission::Granted),
            Err(CaptureError::Device(_))
        ));
        assert!(matches!(
            capture
                .start_reserved(
                    RecordingStartRequest {
                        permission: MicrophonePermission::Granted,
                        handle: Some(CaptureHandle("fixture".into()))
                    },
                    stream
                )
                .await,
            Err(CaptureError::Device(_))
        ));
        assert!(matches!(
            capture
                .cancel_pending(&CaptureHandle("fixture".into()))
                .await,
            Err(CaptureError::Device(_))
        ));
        let (reply, received) = tokio::sync::oneshot::channel();
        let pending = PendingStart {
            canceled: AtomicBool::new(false),
            completed: tokio::sync::watch::channel(false).0,
        };
        capture.busy.store(true, Ordering::SeqCst);
        let worker_capture = capture.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            worker_capture.run_start(
                &CaptureHandle("fixture".into()),
                &pending,
                BusyGuard(worker_capture.busy.clone(), worker_capture.idle.clone()),
                stream,
                &runtime,
                reply,
            );
        })
        .await
        .map_err(|_| state_error())?;
        assert!(matches!(
            received.await.map_err(|_| state_error())?,
            Err(CaptureError::Device(_))
        ));
        assert!(!capture.busy.load(Ordering::SeqCst));
    }
    Ok(())
}
#[tokio::test]
async fn canceled_and_undeliverable_startup_waits_for_native_discard() -> CaptureResult<()> {
    for stream in [false, true] {
        for canceled in [false, true] {
            let capture = fixture();
            let (reply, received) = tokio::sync::oneshot::channel();
            drop(received);
            let pending = PendingStart {
                canceled: AtomicBool::new(canceled),
                completed: tokio::sync::watch::channel(false).0,
            };
            capture.busy.store(true, Ordering::SeqCst);
            let worker_capture = capture.clone();
            let runtime = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                worker_capture.run_start(
                    &CaptureHandle("fixture".into()),
                    &pending,
                    BusyGuard(worker_capture.busy.clone(), worker_capture.idle.clone()),
                    stream,
                    &runtime,
                    reply,
                );
            })
            .await
            .map_err(|_| state_error())?;
            assert!(!capture.busy.load(Ordering::SeqCst));
            assert!(
                capture
                    .recordings
                    .lock()
                    .map_err(|_| state_error())?
                    .is_empty()
            );
            assert!(
                capture
                    .streams
                    .lock()
                    .map_err(|_| state_error())?
                    .is_empty()
            );
        }
    }
    Ok(())
}
#[derive(Debug)]
struct PanicBackend;
impl super::super::Backend for PanicBackend {
    fn devices(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
    fn start(&self) -> Result<Box<dyn super::super::Recording>, String> {
        std::panic::resume_unwind(Box::new("fixture setup panic"))
    }
    fn stream(
        &self,
    ) -> Result<
        (
            tinyvoice_bus::capture::CaptureFormat,
            Box<dyn super::super::Stream>,
        ),
        String,
    > {
        Err("fixture".into())
    }
}
#[tokio::test]
async fn panicking_setup_notifies_completion_and_shutdown_recovers() -> CaptureResult<()> {
    let capture = Arc::new(Capture::new(Arc::new(PanicBackend)));
    let handle = capture.reserve(MicrophonePermission::Granted)?;
    assert!(matches!(
        capture
            .start_reserved(
                RecordingStartRequest {
                    permission: MicrophonePermission::Granted,
                    handle: Some(handle)
                },
                false
            )
            .await,
        Err(CaptureError::Device(_))
    ));
    capture.shutdown().await?;
    assert!(!capture.busy.load(Ordering::SeqCst));
    Ok(())
}
