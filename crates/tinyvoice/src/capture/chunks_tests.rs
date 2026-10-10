//! Unit tests for the chunk stream plumbing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;

use super::{CaptureFormat, DROPPED_CHUNKS, forward, spawn_stream_thread};

#[tokio::test]
async fn a_forwarded_chunk_arrives_untouched() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    forward(&tx, vec![0.1, 0.2, 0.3]);
    assert_eq!(rx.recv().await.unwrap().samples, vec![0.1, 0.2, 0.3]);
}

#[tokio::test]
async fn a_full_queue_drops_the_newest_chunk_and_counts_it() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let before = DROPPED_CHUNKS.load(Ordering::Relaxed);
    forward(&tx, vec![1.0]);
    // The queue is full: this one is dropped, and nothing blocks.
    forward(&tx, vec![2.0]);
    assert!(DROPPED_CHUNKS.load(Ordering::Relaxed) > before);
    assert_eq!(rx.recv().await.unwrap().samples, vec![1.0]);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn a_consumer_that_is_gone_is_not_fatal() {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    drop(rx);
    forward(&tx, vec![1.0]);
}

#[test]
fn a_stream_that_comes_up_reports_its_format() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let format = spawn_stream_thread(
        tx,
        Box::new(|_stop, _tx, setup| {
            setup
                .send(Ok(CaptureFormat {
                    source_rate: 44_100,
                    channels: 2,
                }))
                .unwrap();
            Ok(())
        }),
    )
    .unwrap();
    assert_eq!(format.source_rate, 44_100);
    assert_eq!(format.channels, 2);
}

#[test]
fn a_body_error_reaches_the_caller_with_its_reason() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let error = spawn_stream_thread(
        tx,
        Box::new(|_, _, _| Err("no default audio input device".into())),
    )
    .unwrap_err();
    assert_eq!(error, "no default audio input device");
}

#[test]
fn a_reported_setup_failure_is_returned_as_is() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let error = spawn_stream_thread(
        tx,
        Box::new(|_stop, _, setup| {
            setup.send(Err("permission denied".to_string())).unwrap();
            Ok(())
        }),
    )
    .unwrap_err();
    assert_eq!(error, "permission denied");
}

#[test]
fn a_thread_that_dies_before_reporting_is_named_as_such() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let error = spawn_stream_thread(tx, Box::new(|_, _, _| Ok(()))).unwrap_err();
    assert_eq!(
        error,
        "always-on capture thread exited before signalling readiness"
    );
}

#[test]
fn a_denied_permission_stops_the_stream_before_any_device_is_touched() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let error = super::spawn_capture_thread(tx, || Err("microphone permission denied".to_string()))
        .unwrap_err();
    assert_eq!(
        error,
        crate::Error::Capture("microphone permission denied".into())
    );
}

#[test]
fn continuous_stream_exits_when_its_consumer_closes() {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let (errors, error_rx) = std::sync::mpsc::channel();
    errors
        .send("late device error after consumer closure".to_string())
        .unwrap();
    drop(rx);
    assert!(
        super::wait_for_stream_end(&tx, &std::sync::atomic::AtomicBool::new(false), &error_rx)
            .is_ok()
    );
}

#[tokio::test]
async fn tracked_stop_waits_for_thread_cleanup_and_drop_requests_shutdown() {
    for explicit in [true, false] {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let (cleaned_tx, cleaned_rx) = tokio::sync::oneshot::channel();
        let (format, handle) = super::spawn_tracked_stream(
            tx,
            Box::new(move |stop, _tx, setup| {
                setup
                    .send(Ok(CaptureFormat {
                        source_rate: 48_000,
                        channels: 2,
                    }))
                    .unwrap();
                while !stop.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
                let _ = cleaned_tx.send(());
                Ok(())
            }),
        )
        .unwrap();
        assert_eq!(format.source_rate, 48_000);
        if explicit {
            handle.stop().await.unwrap();
        } else {
            drop(handle);
        }
        cleaned_rx.await.unwrap();
    }
}

#[tokio::test]
async fn tracked_stream_surfaces_terminal_and_vanished_thread_errors() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let (_, handle) = super::spawn_tracked_stream(
        tx,
        Box::new(|_stop, _tx, setup| {
            setup
                .send(Ok(CaptureFormat {
                    source_rate: 16_000,
                    channels: 1,
                }))
                .unwrap();
            Err("device disconnected".into())
        }),
    )
    .unwrap();
    assert_eq!(
        handle.stop().await.unwrap_err(),
        crate::Error::Capture("device disconnected".into())
    );
    let (done, completed) = tokio::sync::oneshot::channel();
    drop(done);
    let handle = super::CaptureStreamHandle {
        stop_flag: Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        ))),
        completed,
    };
    assert!(
        handle
            .stop()
            .await
            .unwrap_err()
            .to_string()
            .contains("task dropped")
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    assert_eq!(
        super::start_capture_stream(tx, || Err("denied".into())).unwrap_err(),
        crate::Error::Capture("denied".into())
    );
}

#[test]
fn stream_lifetime_surfaces_device_errors_and_accepts_explicit_stop() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let (errors, rx) = std::sync::mpsc::channel();
    errors.send("device disconnected".into()).unwrap();
    let stop = std::sync::atomic::AtomicBool::new(false);
    assert_eq!(
        super::wait_for_stream_end(&tx, &stop, &rx).unwrap_err(),
        "device disconnected"
    );
    stop.store(true, Ordering::SeqCst);
    assert!(super::wait_for_stream_end(&tx, &stop, &rx).is_ok());
    drop(errors);
    stop.store(false, Ordering::SeqCst);
    assert!(super::wait_for_stream_end(&tx, &stop, &rx).is_ok());
}

#[tokio::test]
async fn oversized_native_buffers_are_dropped_before_queueing() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(2);
    forward(&tx, vec![0.0; super::MAX_CHUNK_SAMPLES + 1]);
    assert!(rx.try_recv().is_err());
    forward(&tx, vec![0.0; super::MAX_CHUNK_SAMPLES]);
    assert_eq!(
        rx.try_recv().unwrap().samples.len(),
        super::MAX_CHUNK_SAMPLES
    );
}
