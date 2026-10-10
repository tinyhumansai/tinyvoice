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
