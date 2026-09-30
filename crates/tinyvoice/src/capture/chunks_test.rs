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
        Box::new(|_tx, setup| {
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
        Box::new(|_, _| Err("no default audio input device".into())),
    )
    .unwrap_err();
    assert_eq!(error, "no default audio input device");
}

#[test]
fn a_reported_setup_failure_is_returned_as_is() {
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let error = spawn_stream_thread(
        tx,
        Box::new(|_, setup| {
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
    let error = spawn_stream_thread(tx, Box::new(|_, _| Ok(()))).unwrap_err();
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
    assert_eq!(error, "microphone permission denied");
}
