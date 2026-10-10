//! Native capture lifecycle fixtures do not open devices or request permissions.
use super::*;

#[derive(Debug)]
struct Fixture {
    starts: AtomicUsize,
    drops: Arc<AtomicUsize>,
    fail: bool,
}
use std::sync::atomic::AtomicUsize;
#[derive(Debug)]
struct RecordingFixture(Arc<AtomicUsize>);
impl Drop for RecordingFixture {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
impl Recording for RecordingFixture {
    fn finish(self: Box<Self>) -> RecordingFuture {
        Box::pin(async move {
            let _owned = self;
            Ok(RawRecording {
                samples: vec![0.25; 320],
                source_rate: 16_000,
                channels: 1,
            })
        })
    }
}
impl Backend for Fixture {
    fn devices(&self) -> Result<Vec<String>, String> {
        if self.fail {
            Err("fixture device failure".into())
        } else {
            Ok(vec!["fixture".into()])
        }
    }
    fn stream(&self) -> Result<(tinyvoice_bus::capture::CaptureFormat, Box<dyn Stream>), String> {
        if self.fail {
            return Err("fixture setup failure".into());
        }
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tx.try_send(tinyvoice::capture::RawChunk {
            samples: vec![0.25; 16],
        })
        .map_err(|error| error.to_string())?;
        Ok((
            tinyvoice_bus::capture::CaptureFormat {
                source_rate: 16_000,
                channels: 1,
            },
            Box::new(StreamFixture {
                rx,
                drops: self.drops.clone(),
                fail_stop: false,
            }),
        ))
    }
    fn start(&self) -> Result<Box<dyn Recording>, String> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err("fixture setup failure".into())
        } else {
            Ok(Box::new(RecordingFixture(self.drops.clone())))
        }
    }
}
fn fixture(fail: bool) -> (Capture, Arc<Fixture>) {
    let backend = Arc::new(Fixture {
        starts: AtomicUsize::new(0),
        drops: Arc::new(AtomicUsize::new(0)),
        fail,
    });
    (Capture::new(backend.clone()), backend)
}
fn read(handle: &CaptureHandle, offset: usize, length: usize) -> ReadAudioRequest {
    ReadAudioRequest {
        handle: handle.clone(),
        offset,
        length,
    }
}

#[tokio::test]
async fn permission_busy_cancel_and_drop_release_device_ownership() -> CaptureResult<()> {
    let (capture, backend) = fixture(false);
    assert_eq!(capture.devices()?, vec!["fixture"]);
    assert_eq!(
        capture.start(MicrophonePermission::Denied),
        Err(CaptureError::PermissionDenied)
    );
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
    let handle = capture.start(MicrophonePermission::Granted)?;
    assert_eq!(
        capture.start(MicrophonePermission::Granted),
        Err(CaptureError::Busy)
    );
    capture.cancel(&handle).await?;
    assert_eq!(backend.drops.load(Ordering::SeqCst), 1);
    assert_eq!(
        capture.cancel(&handle).await,
        Err(CaptureError::UnknownHandle)
    );
    let _handle = capture.start(MicrophonePermission::Granted)?;
    drop(capture);
    assert_eq!(backend.drops.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn capture_shutdown_takes_precedence_over_invalid_requests() -> CaptureResult<()> {
    let (capture, _) = fixture(false);
    let capture = Arc::new(capture);
    capture.shutdown().await?;

    assert_eq!(
        capture.reserve(MicrophonePermission::Denied),
        Err(CaptureError::Closed)
    );
    assert!(matches!(
        capture
            .start_reserved(
                RecordingStartRequest {
                    permission: MicrophonePermission::Denied,
                    handle: None,
                },
                false,
            )
            .await,
        Err(CaptureError::Closed)
    ));
    assert_eq!(
        capture
            .finish(RecordingFinishRequest {
                handle: CaptureHandle("unknown".into()),
                gate_threshold: f32::NAN,
            })
            .await,
        Err(CaptureError::Closed)
    );
    Ok(())
}

#[tokio::test]
async fn finish_holds_prepared_wav_for_bounded_reads_and_explicit_release() -> CaptureResult<()> {
    let (capture, _) = fixture(false);
    let handle = capture.start(MicrophonePermission::Granted)?;
    assert_eq!(
        capture
            .finish(RecordingFinishRequest {
                handle: handle.clone(),
                gate_threshold: f32::NAN
            })
            .await,
        Err(CaptureError::InvalidParameters)
    );
    let output = capture
        .finish(RecordingFinishRequest {
            handle: handle.clone(),
            gate_threshold: 0.0,
        })
        .await?;
    assert_eq!(output.length, 44 + 320 * 2);
    let encoded = capture.read(&read(&output.handle, 0, MAX_READ_BYTES))?;
    let wav = BASE64
        .decode(encoded)
        .map_err(|error| CaptureError::Device(error.to_string()))?;
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(capture.read(&read(&output.handle, output.length, 1))?, "");
    assert_eq!(
        capture.read(&read(&output.handle, output.length + 1, 1)),
        Err(CaptureError::InvalidParameters)
    );
    assert_eq!(
        capture.read(&read(&output.handle, 0, 0)),
        Err(CaptureError::LimitExceeded)
    );
    assert_eq!(
        capture.read(&read(&output.handle, 0, MAX_READ_BYTES + 1)),
        Err(CaptureError::LimitExceeded)
    );
    assert_eq!(
        capture
            .finish(RecordingFinishRequest {
                handle,
                gate_threshold: 0.0
            })
            .await,
        Err(CaptureError::UnknownHandle)
    );
    capture.release(&output.handle)?;
    assert_eq!(
        capture.read(&read(&output.handle, 0, 1)),
        Err(CaptureError::UnknownHandle)
    );
    assert_eq!(
        capture.release(&output.handle),
        Err(CaptureError::UnknownHandle)
    );
    Ok(())
}

#[test]
fn silence_gate_preserves_short_speech_before_a_long_silent_tail() -> CaptureResult<()> {
    let mut samples = vec![0.1; 1_000];
    samples.extend(vec![0.0; 16_000]);
    let wav = prepare(
        &RawRecording {
            samples,
            source_rate: 16_000,
            channels: 1,
        },
        0.03,
    )?;

    assert!(wav.len() > 44, "speech must survive silence gating");
    assert!(
        wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .any(|sample| *sample != [0, 0])
    );
    Ok(())
}

#[tokio::test]
async fn output_capacity_and_setup_faults_do_not_leak_recording_slots() -> CaptureResult<()> {
    let (capture, _) = fixture(false);
    for _ in 0..MAX_OUTPUTS {
        let handle = capture.start(MicrophonePermission::Granted)?;
        capture
            .finish(RecordingFinishRequest {
                handle,
                gate_threshold: 0.01,
            })
            .await?;
    }
    let handle = capture.start(MicrophonePermission::Granted)?;
    assert_eq!(
        capture
            .finish(RecordingFinishRequest {
                handle,
                gate_threshold: 0.0
            })
            .await,
        Err(CaptureError::LimitExceeded)
    );
    let handle = capture.start(MicrophonePermission::Granted)?;
    capture.cancel(&handle).await?;
    let (failed, backend) = fixture(true);
    assert_eq!(
        failed.devices(),
        Err(CaptureError::Device("fixture device failure".into()))
    );
    for _ in 0..2 {
        assert_eq!(
            failed.start(MicrophonePermission::Granted),
            Err(CaptureError::Device("fixture setup failure".into()))
        );
    }
    assert_eq!(backend.starts.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn invalid_device_formats_and_expansion_are_rejected_before_resampling() {
    assert_eq!(
        prepare(
            &RawRecording {
                samples: vec![0.0; 10],
                source_rate: 0,
                channels: 1
            },
            0.0
        ),
        Err(CaptureError::InvalidParameters)
    );
    assert_eq!(
        prepare(
            &RawRecording {
                samples: vec![0.0; 10],
                source_rate: 16_000,
                channels: usize::MAX
            },
            0.0
        ),
        Err(CaptureError::InvalidParameters)
    );
    assert_eq!(
        prepare(
            &RawRecording {
                samples: vec![0.0; 2_000],
                source_rate: 1,
                channels: 1
            },
            0.0
        ),
        Err(CaptureError::LimitExceeded)
    );
    assert!(matches!(
        prepare(
            &RawRecording {
                samples: vec![0.0; 10],
                source_rate: 16_000,
                channels: 0
            },
            0.0
        ),
        Err(CaptureError::Device(_))
    ));
}

pub(super) fn fixture_capture() -> Capture {
    fixture(false).0
}

#[test]
fn poisoned_resource_tables_return_closed_errors_without_recovery() {
    let (capture, _) = fixture(false);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = capture.recordings.lock();
        std::panic::resume_unwind(Box::new("fixture poison"));
    }));
    assert!(matches!(
        capture.start(MicrophonePermission::Granted),
        Err(CaptureError::Device(_))
    ));
    assert!(matches!(
        capture.take(&CaptureHandle("unknown".into())),
        Err(CaptureError::Device(_))
    ));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = capture.outputs.lock();
        std::panic::resume_unwind(Box::new("fixture poison"));
    }));
    assert!(matches!(
        capture.read(&read(&CaptureHandle("unknown".into()), 0, 1)),
        Err(CaptureError::Device(_))
    ));
    assert!(matches!(
        capture.release(&CaptureHandle("unknown".into())),
        Err(CaptureError::Device(_))
    ));
}

#[tokio::test]
async fn preparing_with_a_poisoned_output_table_releases_the_recording() -> CaptureResult<()> {
    let (capture, _) = fixture(false);
    let handle = capture.start(MicrophonePermission::Granted)?;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = capture.outputs.lock();
        std::panic::resume_unwind(Box::new("fixture poison"));
    }));
    assert!(matches!(
        capture
            .finish(RecordingFinishRequest {
                handle,
                gate_threshold: 0.0
            })
            .await,
        Err(CaptureError::Device(_))
    ));
    let restarted = capture.start(MicrophonePermission::Granted)?;
    capture.cancel(&restarted).await?;
    Ok(())
}

#[derive(Debug)]
struct StreamFixture {
    fail_stop: bool,
    rx: tokio::sync::mpsc::Receiver<tinyvoice::capture::RawChunk>,
    drops: Arc<AtomicUsize>,
}
impl Drop for StreamFixture {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl Stream for StreamFixture {
    fn poll(&mut self, max_chunks: usize) -> CaptureResult<CaptureBatch> {
        Ok(poll_chunks(&mut self.rx, max_chunks))
    }
    fn stop(self: Box<Self>) -> StreamFuture {
        Box::pin(async move {
            let fail = self.fail_stop;
            drop(self);
            if fail {
                Err("fixture terminal failure".into())
            } else {
                Ok(())
            }
        })
    }
}
#[tokio::test]
async fn continuous_capture_is_exclusive_drains_ordered_chunks_and_stops() -> CaptureResult<()> {
    let manager = fixture_capture();
    assert_eq!(
        manager.stream_start(MicrophonePermission::Denied).err(),
        Some(CaptureError::PermissionDenied)
    );
    let stream = manager.stream_start(MicrophonePermission::Granted)?;
    assert_eq!(
        manager.start(MicrophonePermission::Granted).err(),
        Some(CaptureError::Busy)
    );
    assert_eq!(
        manager.stream_start(MicrophonePermission::Granted).err(),
        Some(CaptureError::Busy)
    );
    assert_eq!(
        manager
            .stream_poll(&CapturePollRequest {
                handle: stream.handle.clone(),
                max_chunks: 3
            })
            .err(),
        Some(CaptureError::LimitExceeded)
    );
    let batch = manager.stream_poll(&CapturePollRequest {
        handle: stream.handle.clone(),
        max_chunks: 2,
    })?;
    assert_eq!(batch.chunks[0].samples, vec![0.25; 16]);
    assert!(batch.closed);
    manager.stream_stop(&stream.handle).await?;
    assert_eq!(
        manager.stream_stop(&stream.handle).await.err(),
        Some(CaptureError::UnknownHandle)
    );
    assert_eq!(
        manager
            .stream_poll(&CapturePollRequest {
                handle: stream.handle,
                max_chunks: 1
            })
            .err(),
        Some(CaptureError::UnknownHandle)
    );
    let stream = manager.stream_start(MicrophonePermission::Granted)?;
    manager.stream_stop(&stream.handle).await?;
    Ok(())
}
#[test]
fn chunk_poll_preserves_order_and_limits_batch_size() -> Result<(), Box<dyn std::error::Error>> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    for i in [1.0, 2.0, 3.0] {
        tx.try_send(tinyvoice::capture::RawChunk { samples: vec![i] })?;
    }
    let batch = poll_chunks(&mut rx, 2);
    assert_eq!(batch.chunks.len(), 2);
    assert_eq!(batch.chunks[1].samples, vec![2.0]);
    assert!(!batch.closed);
    assert_eq!(poll_chunks(&mut rx, 1).chunks[0].samples, vec![3.0]);
    assert_eq!(poll_chunks(&mut rx, 2).chunks.len(), 0);
    drop(tx);
    assert!(poll_chunks(&mut rx, 2).closed);
    Ok(())
}

#[tokio::test]
async fn failed_stream_shutdown_releases_device_capacity() -> CaptureResult<()> {
    let (capture, backend) = fixture(false);
    let handle = CaptureHandle("failing stream".into());
    let (_tx, rx) = tokio::sync::mpsc::channel(8);
    capture.busy.store(true, Ordering::SeqCst);
    capture
        .streams
        .lock()
        .map_err(|_| CaptureError::Device("fixture lock".into()))?
        .insert(
            handle.clone(),
            StreamLease {
                stream: Box::new(StreamFixture {
                    rx,
                    drops: backend.drops.clone(),
                    fail_stop: true,
                }),
                busy: BusyGuard(capture.busy.clone(), capture.idle.clone()),
            },
        );
    assert_eq!(
        capture.stream_stop(&handle).await,
        Err(CaptureError::Device("fixture terminal failure".into()))
    );
    assert_eq!(backend.drops.load(Ordering::SeqCst), 1);
    let stream = capture.stream_start(MicrophonePermission::Granted)?;
    capture.stream_stop(&stream.handle).await?;
    Ok(())
}

#[test]
fn dropping_the_manager_releases_its_stream_and_failed_setup_releases_capacity() -> CaptureResult<()>
{
    let (capture, backend) = fixture(false);
    let _stream = capture.stream_start(MicrophonePermission::Granted)?;
    drop(capture);
    assert_eq!(backend.drops.load(Ordering::SeqCst), 1);
    let (capture, _) = fixture(true);
    for _ in 0..2 {
        assert_eq!(
            capture.stream_start(MicrophonePermission::Granted).err(),
            Some(CaptureError::Device("fixture setup failure".into()))
        );
        assert!(!capture.busy.load(Ordering::SeqCst));
    }
    Ok(())
}

#[tokio::test]
async fn poisoned_stream_state_fails_closed_and_drops_unregistered_capture() -> CaptureResult<()> {
    let (capture, backend) = fixture(false);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = capture.streams.lock();
        std::panic::resume_unwind(Box::new("fixture poison"));
    }));
    let handle = CaptureHandle("unknown".into());
    assert!(matches!(
        capture.stream_start(MicrophonePermission::Granted),
        Err(CaptureError::Device(_))
    ));
    assert_eq!(backend.drops.load(Ordering::SeqCst), 1);
    assert!(!capture.busy.load(Ordering::SeqCst));
    assert!(matches!(
        capture.stream_poll(&CapturePollRequest {
            handle: handle.clone(),
            max_chunks: 1
        }),
        Err(CaptureError::Device(_))
    ));
    assert!(matches!(
        capture.stream_stop(&handle).await,
        Err(CaptureError::Device(_))
    ));
    Ok(())
}

#[derive(Debug)]
struct PendingRecording(tokio::sync::oneshot::Receiver<()>);
impl Recording for PendingRecording {
    fn finish(self: Box<Self>) -> RecordingFuture {
        Box::pin(async move {
            let _ = self.0.await;
            Ok(RawRecording {
                samples: vec![0.25; 320],
                source_rate: 16_000,
                channels: 1,
            })
        })
    }
}
#[tokio::test]
async fn dropping_cancel_future_keeps_device_busy_until_cleanup_completes() -> CaptureResult<()> {
    let (capture, _) = fixture(false);
    let handle = capture.start(MicrophonePermission::Granted)?;
    let (done, pending) = tokio::sync::oneshot::channel();
    capture
        .recordings
        .lock()
        .map_err(|_| state_error())?
        .get_mut(&handle)
        .ok_or(CaptureError::UnknownHandle)?
        .recording = Box::new(PendingRecording(pending));
    let mut cancel = Box::pin(capture.cancel(&handle));
    std::future::poll_fn(|cx| {
        assert!(cancel.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(cancel);
    assert_eq!(
        capture.start(MicrophonePermission::Granted),
        Err(CaptureError::Busy)
    );
    let _ = done.send(());
    while capture.busy.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    let next = capture.start(MicrophonePermission::Granted)?;
    capture.cancel(&next).await?;
    Ok(())
}

#[derive(Debug)]
struct DelayedBackend {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    proceed: Mutex<std::sync::mpsc::Receiver<()>>,
    drops: Arc<AtomicUsize>,
}
impl Backend for DelayedBackend {
    fn devices(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
    fn start(&self) -> Result<Box<dyn Recording>, String> {
        if let Some(entered) = self
            .entered
            .lock()
            .map_err(|error| error.to_string())?
            .take()
        {
            let _ = entered.send(());
        }
        self.proceed
            .lock()
            .map_err(|error| error.to_string())?
            .recv()
            .map_err(|error| error.to_string())?;
        Ok(Box::new(RecordingFixture(self.drops.clone())))
    }
    fn stream(&self) -> Result<(tinyvoice_bus::capture::CaptureFormat, Box<dyn Stream>), String> {
        Err("fixture stream unavailable".into())
    }
}
#[tokio::test]
async fn abandoned_start_has_a_known_cancel_handle_and_waits_for_native_cleanup()
-> CaptureResult<()> {
    let (entered, startup) = tokio::sync::oneshot::channel();
    let (proceed, blocked) = std::sync::mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let capture = Arc::new(Capture::new(Arc::new(DelayedBackend {
        entered: Mutex::new(Some(entered)),
        proceed: Mutex::new(blocked),
        drops: drops.clone(),
    })));
    let handle = capture.reserve(MicrophonePermission::Granted)?;
    let service = crate::service::VoiceService {
        capture: capture.clone(),
        ..Default::default()
    };
    let mut start = Box::pin(service.recording_start(RecordingStartRequest {
        permission: MicrophonePermission::Granted,
        handle: Some(handle.clone()),
    }));
    std::future::poll_fn(|cx| {
        assert!(start.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    startup.await.map_err(|_| state_error())?;
    drop(start);
    let mut cancel = Box::pin(capture.cancel(&handle));
    std::future::poll_fn(|cx| {
        assert!(cancel.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(capture.busy.load(Ordering::SeqCst));
    proceed.send(()).map_err(|_| state_error())?;
    cancel.await?;
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(!capture.busy.load(Ordering::SeqCst));
    assert!(
        capture
            .recordings
            .lock()
            .map_err(|_| state_error())?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn abandoned_finish_keeps_capacity_and_known_output_can_be_released_before_delivery()
-> CaptureResult<()> {
    let (capture, _) = fixture(false);
    let handle = capture.start(MicrophonePermission::Granted)?;
    let (done, pending) = tokio::sync::oneshot::channel();
    capture
        .recordings
        .lock()
        .map_err(|_| state_error())?
        .get_mut(&handle)
        .ok_or(CaptureError::UnknownHandle)?
        .recording = Box::new(PendingRecording(pending));
    let mut finish = Box::pin(capture.finish(RecordingFinishRequest {
        handle: handle.clone(),
        gate_threshold: 0.0,
    }));
    std::future::poll_fn(|cx| {
        assert!(finish.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(finish);
    assert_eq!(
        capture.start(MicrophonePermission::Granted),
        Err(CaptureError::Busy)
    );
    capture.release(&handle)?;
    let _ = done.send(());
    while capture.busy.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    assert!(
        capture
            .outputs
            .lock()
            .map_err(|_| state_error())?
            .is_empty()
    );
    assert!(
        capture
            .finishing
            .lock()
            .map_err(|_| state_error())?
            .is_empty()
    );
    let handle = capture.start(MicrophonePermission::Granted)?;
    let output = capture
        .finish(RecordingFinishRequest {
            handle: handle.clone(),
            gate_threshold: 0.0,
        })
        .await?;
    assert_eq!(output.handle, handle);
    capture.release(&handle)?;
    Ok(())
}
#[derive(Debug)]
struct PendingStream(tokio::sync::oneshot::Receiver<()>);
impl Stream for PendingStream {
    fn poll(&mut self, _: usize) -> CaptureResult<CaptureBatch> {
        Ok(CaptureBatch::default())
    }
    fn stop(self: Box<Self>) -> StreamFuture {
        Box::pin(async move {
            let _ = self.0.await;
            Ok(())
        })
    }
}
#[tokio::test]
async fn abandoned_stream_stop_keeps_device_busy_until_native_cleanup() -> CaptureResult<()> {
    let (capture, _) = fixture(false);
    let stream = capture.stream_start(MicrophonePermission::Granted)?;
    let (done, pending) = tokio::sync::oneshot::channel();
    capture
        .streams
        .lock()
        .map_err(|_| state_error())?
        .get_mut(&stream.handle)
        .ok_or(CaptureError::UnknownHandle)?
        .stream = Box::new(PendingStream(pending));
    let mut stop = Box::pin(capture.stream_stop(&stream.handle));
    std::future::poll_fn(|cx| {
        assert!(stop.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(stop);
    assert_eq!(
        capture.stream_start(MicrophonePermission::Granted).err(),
        Some(CaptureError::Busy)
    );
    let _ = done.send(());
    while capture.busy.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    let stream = capture.stream_start(MicrophonePermission::Granted)?;
    capture.stream_stop(&stream.handle).await?;
    Ok(())
}
#[tokio::test]
async fn abandoned_reservations_are_bounded_expire_and_never_hold_device_capacity()
-> CaptureResult<()> {
    let capture = Arc::new(fixture_capture());
    assert_eq!(
        capture.reserve(MicrophonePermission::Denied),
        Err(CaptureError::PermissionDenied)
    );
    let handle = capture.reserve(MicrophonePermission::Granted)?;
    let live = capture.start(MicrophonePermission::Granted)?;
    capture.cancel(&live).await?;
    for _ in 1..MAX_RESERVATIONS {
        capture.reserve(MicrophonePermission::Granted)?;
    }
    assert_eq!(
        capture.reserve(MicrophonePermission::Granted),
        Err(CaptureError::LimitExceeded)
    );
    capture
        .reservations
        .lock()
        .map_err(|_| state_error())?
        .values_mut()
        .for_each(|reservation| reservation.created -= RESERVATION_TTL);
    assert_eq!(
        capture
            .start_reserved(
                RecordingStartRequest {
                    permission: MicrophonePermission::Granted,
                    handle: Some(handle)
                },
                false
            )
            .await
            .err(),
        Some(CaptureError::UnknownHandle)
    );
    let handle = capture.reserve(MicrophonePermission::Granted)?;
    assert_eq!(
        capture
            .reservations
            .lock()
            .map_err(|_| state_error())?
            .len(),
        1
    );
    capture.cancel(&handle).await?;
    assert!(
        capture
            .reservations
            .lock()
            .map_err(|_| state_error())?
            .is_empty()
    );
    assert!(!capture.busy.load(Ordering::SeqCst));
    Ok(())
}
#[tokio::test]
async fn reserved_start_rejects_missing_unknown_and_repeated_handles_and_recovers_setup_errors()
-> CaptureResult<()> {
    let capture = Arc::new(fixture_capture());
    assert_eq!(
        capture
            .start_reserved(RecordingStartRequest::default(), false)
            .await
            .err(),
        Some(CaptureError::PermissionDenied)
    );
    assert_eq!(
        capture
            .start_reserved(
                RecordingStartRequest {
                    permission: MicrophonePermission::Granted,
                    handle: None
                },
                false
            )
            .await
            .err(),
        Some(CaptureError::InvalidParameters)
    );
    assert_eq!(
        capture
            .start_reserved(
                RecordingStartRequest {
                    permission: MicrophonePermission::Granted,
                    handle: Some(CaptureHandle("unknown".into()))
                },
                false
            )
            .await
            .err(),
        Some(CaptureError::UnknownHandle)
    );
    let handle = capture.reserve(MicrophonePermission::Granted)?;
    let request = RecordingStartRequest {
        permission: MicrophonePermission::Granted,
        handle: Some(handle.clone()),
    };
    let started = capture.start_reserved(request.clone(), false).await?;
    assert!(matches!(started, Started::Recording(_)));
    assert_eq!(
        capture.start_reserved(request, false).await.err(),
        Some(CaptureError::UnknownHandle)
    );
    capture.cancel(&handle).await?;
    let capture = Arc::new(fixture(true).0);
    for stream in [false, true] {
        let handle = capture.reserve(MicrophonePermission::Granted)?;
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
    }
    Ok(())
}

#[tokio::test]
async fn shutdown_waits_for_delayed_start_and_prevents_publication() -> CaptureResult<()> {
    let (entered, startup) = tokio::sync::oneshot::channel();
    let (proceed, blocked) = std::sync::mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let capture = Arc::new(Capture::new(Arc::new(DelayedBackend {
        entered: Mutex::new(Some(entered)),
        proceed: Mutex::new(blocked),
        drops: drops.clone(),
    })));
    let handle = capture.reserve(MicrophonePermission::Granted)?;
    let request = RecordingStartRequest {
        permission: MicrophonePermission::Granted,
        handle: Some(handle.clone()),
    };
    let mut start = Box::pin(capture.start_reserved(request.clone(), false));
    std::future::poll_fn(|cx| {
        assert!(start.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    startup.await.map_err(|_| state_error())?;
    assert_eq!(
        capture.start_reserved(request.clone(), false).await.err(),
        Some(CaptureError::Busy)
    );
    let other = capture.reserve(MicrophonePermission::Granted)?;
    assert_eq!(
        capture
            .start_reserved(
                RecordingStartRequest {
                    permission: MicrophonePermission::Granted,
                    handle: Some(other)
                },
                false
            )
            .await
            .err(),
        Some(CaptureError::Busy)
    );
    let mut shutdown = Box::pin(capture.shutdown());
    std::future::poll_fn(|cx| {
        assert!(shutdown.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(capture.busy.load(Ordering::SeqCst));
    assert_eq!(
        capture.reserve(MicrophonePermission::Granted),
        Err(CaptureError::Closed)
    );
    assert_eq!(
        capture.start_reserved(request, false).await.err(),
        Some(CaptureError::Closed)
    );
    proceed.send(()).map_err(|_| state_error())?;
    assert_eq!(start.await.err(), Some(CaptureError::Cancelled));
    shutdown.await?;
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(
        capture
            .recordings
            .lock()
            .map_err(|_| state_error())?
            .is_empty()
    );
    assert!(
        capture
            .reservations
            .lock()
            .map_err(|_| state_error())?
            .is_empty()
    );
    Ok(())
}
#[tokio::test]
async fn shutdown_waits_for_detached_finish_cancel_and_stop_workers() -> CaptureResult<()> {
    for operation in ["finish", "cancel", "stop"] {
        let capture = Arc::new(fixture_capture());
        let (done, pending) = tokio::sync::oneshot::channel();
        let handle = if operation == "stop" {
            let stream = capture.stream_start(MicrophonePermission::Granted)?;
            capture
                .streams
                .lock()
                .map_err(|_| state_error())?
                .get_mut(&stream.handle)
                .ok_or(CaptureError::UnknownHandle)?
                .stream = Box::new(PendingStream(pending));
            stream.handle
        } else {
            let handle = capture.start(MicrophonePermission::Granted)?;
            capture
                .recordings
                .lock()
                .map_err(|_| state_error())?
                .get_mut(&handle)
                .ok_or(CaptureError::UnknownHandle)?
                .recording = Box::new(PendingRecording(pending));
            handle
        };
        let mut work: Pin<Box<dyn Future<Output = CaptureResult<()>>>> = match operation {
            "finish" => Box::pin(async {
                capture
                    .finish(RecordingFinishRequest {
                        handle: handle.clone(),
                        gate_threshold: 0.0,
                    })
                    .await
                    .map(|_| ())
            }),
            "cancel" => Box::pin(capture.cancel(&handle)),
            _ => Box::pin(capture.stream_stop(&handle)),
        };
        std::future::poll_fn(|cx| {
            assert!(work.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(work);
        // Reproduce the stale notification from a predecessor guard.
        capture.idle.send_replace(true);
        let mut shutdown = Box::pin(capture.shutdown());
        std::future::poll_fn(|cx| {
            assert!(shutdown.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        done.send(()).map_err(|()| state_error())?;
        shutdown.await?;
        assert!(
            capture
                .outputs
                .lock()
                .map_err(|_| state_error())?
                .is_empty()
        );
        assert!(
            capture
                .finishing
                .lock()
                .map_err(|_| state_error())?
                .is_empty()
        );
        assert!(!capture.busy.load(Ordering::SeqCst));
        assert_eq!(
            capture
                .finish(RecordingFinishRequest {
                    handle,
                    gate_threshold: 0.0
                })
                .await,
            Err(CaptureError::Closed)
        );
    }
    Ok(())
}
#[tokio::test]
async fn shutdown_drains_live_resources_and_idle_reservations() -> CaptureResult<()> {
    for stream in [false, true] {
        let capture = Arc::new(fixture_capture());
        let handle = capture.reserve(MicrophonePermission::Granted)?;
        capture
            .start_reserved(
                RecordingStartRequest {
                    permission: MicrophonePermission::Granted,
                    handle: Some(handle),
                },
                stream,
            )
            .await?;
        capture.reserve(MicrophonePermission::Granted)?;
        capture.shutdown().await?;
        capture.shutdown().await?;
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
        assert!(
            capture
                .reservations
                .lock()
                .map_err(|_| state_error())?
                .is_empty()
        );
        assert_eq!(
            capture.start(MicrophonePermission::Granted),
            Err(CaptureError::Closed)
        );
        assert_eq!(
            capture.stream_start(MicrophonePermission::Granted).err(),
            Some(CaptureError::Closed)
        );
    }
    Ok(())
}

#[derive(Debug)]
struct DelayedPreparation {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    proceed: Mutex<std::sync::mpsc::Receiver<()>>,
    fixture: Arc<Fixture>,
}
impl Backend for DelayedPreparation {
    fn devices(&self) -> Result<Vec<String>, String> {
        self.fixture.devices()
    }
    fn start(&self) -> Result<Box<dyn Recording>, String> {
        self.fixture.start()
    }
    fn stream(&self) -> Result<(tinyvoice_bus::capture::CaptureFormat, Box<dyn Stream>), String> {
        self.fixture.stream()
    }
    fn prepare(&self, raw: &RawRecording, threshold: f32) -> CaptureResult<Vec<u8>> {
        if let Some(entered) = self.entered.lock().map_err(|_| state_error())?.take() {
            let _ = entered.send(());
        }
        self.proceed
            .lock()
            .map_err(|_| state_error())?
            .recv()
            .map_err(|_| state_error())?;
        prepare(raw, threshold)
    }
}
#[tokio::test]
async fn shutdown_waits_for_blocked_preparation_and_discards_its_late_output() -> CaptureResult<()>
{
    let (entered, preparing) = tokio::sync::oneshot::channel();
    let (proceed, blocked) = std::sync::mpsc::channel();
    let capture = Arc::new(Capture::new(Arc::new(DelayedPreparation {
        entered: Mutex::new(Some(entered)),
        proceed: Mutex::new(blocked),
        fixture: fixture(false).1,
    })));
    let handle = capture.start(MicrophonePermission::Granted)?;
    let mut finish = Box::pin(capture.finish(RecordingFinishRequest {
        handle: handle.clone(),
        gate_threshold: 0.0,
    }));
    std::future::poll_fn(|cx| {
        assert!(finish.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    preparing.await.map_err(|_| state_error())?;
    drop(finish);
    capture.release(&handle)?;
    let mut shutdown = Box::pin(capture.shutdown());
    std::future::poll_fn(|cx| {
        assert!(shutdown.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(capture.busy.load(Ordering::SeqCst));
    proceed.send(()).map_err(|_| state_error())?;
    shutdown.await?;
    assert!(
        capture
            .outputs
            .lock()
            .map_err(|_| state_error())?
            .is_empty()
    );
    assert!(
        capture
            .finishing
            .lock()
            .map_err(|_| state_error())?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn stale_predecessor_idle_notification_does_not_release_successor_cleanup()
-> CaptureResult<()> {
    // Old guard publishes busy=false; successor claims busy=true/idle=false;
    // old guard then publishes the stale idle=true notification.
    let busy = AtomicBool::new(true);
    let (changed, idle) = tokio::sync::watch::channel(true);
    let mut cleanup = Box::pin(wait_for_idle(&busy, idle));
    std::future::poll_fn(|cx| {
        assert!(cleanup.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    busy.store(false, Ordering::SeqCst);
    changed.send_replace(true);
    cleanup.await?;
    Ok(())
}

#[tokio::test]
async fn lost_cleanup_notifications_fail_closed() {
    let busy = AtomicBool::new(true);
    let (changed, idle) = tokio::sync::watch::channel(true);
    drop(changed);
    assert!(matches!(
        wait_for_idle(&busy, idle).await,
        Err(CaptureError::Device(_))
    ));
}
