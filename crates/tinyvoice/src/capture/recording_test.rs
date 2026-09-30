//! Unit tests for the recording handle and the sample cap.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;

use super::{MAX_RAW_SAMPLES, RawRecording, append_capped, spawn_recording};

#[test]
fn append_capped_bounds_the_raw_buffer() {
    // The cap is what stops a recording that is started and never stopped from
    // growing without limit.
    let buffer = parking_lot::Mutex::new(Vec::new());

    // Fill to just under the cap.
    let chunk = vec![0.5f32; 4096];
    while buffer.lock().len() + chunk.len() <= MAX_RAW_SAMPLES {
        append_capped(&buffer, &chunk);
    }
    let before = buffer.lock().len();
    assert!(before > 0);

    // The chunk that crosses the line is truncated, not dropped whole: a
    // recording that hits the cap keeps its first five minutes.
    append_capped(&buffer, &vec![0.5f32; MAX_RAW_SAMPLES]);
    assert_eq!(buffer.lock().len(), MAX_RAW_SAMPLES);

    // Past the cap, further chunks are ignored rather than reallocating.
    append_capped(&buffer, &chunk);
    assert_eq!(buffer.lock().len(), MAX_RAW_SAMPLES);
}

#[tokio::test]
async fn a_started_recording_yields_its_samples_once_stopped() {
    let handle = spawn_recording(Box::new(|stop, setup| {
        setup.send(Ok(())).unwrap();
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(RawRecording {
            samples: vec![0.25, -0.25],
            source_rate: 48_000,
            channels: 2,
        })
    }))
    .unwrap();

    let raw = handle.stop().await.unwrap();
    assert_eq!(raw.samples, vec![0.25, -0.25]);
    assert_eq!(raw.source_rate, 48_000);
    assert_eq!(raw.channels, 2);
}

#[tokio::test]
async fn a_recording_that_captured_nothing_reports_why() {
    let handle = spawn_recording(Box::new(|stop, setup| {
        setup.send(Ok(())).unwrap();
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err("no audio samples captured".to_string())
    }))
    .unwrap();

    assert_eq!(
        handle.stop().await.unwrap_err(),
        "no audio samples captured"
    );
}

#[test]
fn a_setup_failure_reaches_the_caller_with_its_reason() {
    let error = spawn_recording(Box::new(|_, setup| {
        setup
            .send(Err("no default audio input device found".to_string()))
            .unwrap();
        Err("no default audio input device found".to_string())
    }))
    .unwrap_err();
    assert_eq!(error, "no default audio input device found");
}

#[test]
fn a_thread_that_dies_before_reporting_is_named_as_such() {
    let error = spawn_recording(Box::new(|_, _| Err("gone".to_string()))).unwrap_err();
    assert_eq!(error, "capture thread exited before signalling readiness");
}

#[tokio::test]
async fn a_capture_task_that_vanishes_is_reported() {
    let handle = spawn_recording(Box::new(|_, setup| {
        setup.send(Ok(())).unwrap();
        panic!("audio thread crashed");
    }))
    .unwrap();
    assert_eq!(
        handle.stop().await.unwrap_err(),
        "recording task dropped before completing"
    );
}

#[test]
fn a_denied_permission_stops_a_recording_before_any_device_is_touched() {
    let error = super::start_recording(|| Err("microphone access denied".to_string())).unwrap_err();
    assert_eq!(error, "microphone access denied");
}
