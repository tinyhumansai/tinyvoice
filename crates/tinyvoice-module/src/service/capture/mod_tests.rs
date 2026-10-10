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
    assert!(poll_chunks(&mut rx, 2).chunks.is_empty());
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
                busy: BusyGuard(capture.busy.clone()),
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
