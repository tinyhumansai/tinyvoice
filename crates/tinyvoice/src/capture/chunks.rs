//! A continuous capture stream: raw device chunks into a bounded channel.

use std::sync::atomic::{AtomicU64, Ordering};

use super::PermissionCheck;

const LOG_PREFIX: &str = "[voice::always_on]";

/// One chunk of raw capture, exactly as the device delivered it.
///
/// Interleaved and at the device's own rate: the callback converts the sample
/// format and nothing else, so [`CaptureFormat`] travels separately for the
/// consumer to hand to [`crate::audio`].
#[derive(Debug)]
pub struct RawChunk {
    /// Interleaved `f32` samples.
    pub samples: Vec<f32>,
}

/// The device format, learned once when the stream is built.
#[derive(Debug, Clone, Copy)]
pub struct CaptureFormat {
    /// Device sample rate, before resampling to [`super::TARGET_SAMPLE_RATE`].
    pub source_rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
}

/// Chunks the capture callback had to drop because the queue was full.
///
/// A process-wide counter rather than closure state: the callback is built once
/// per sample format and each closure must stay `Fn`, so the count cannot live
/// in a captured local. One always-on stream exists per process, so a single
/// counter is not an aggregation of unrelated streams.
static DROPPED_CHUNKS: AtomicU64 = AtomicU64::new(0);

/// The stream thread's body: build and play the stream, report the format, and
/// then keep the stream alive.
pub(crate) type StreamBody = Box<
    dyn FnOnce(
            tokio::sync::mpsc::Sender<RawChunk>,
            &std::sync::mpsc::SyncSender<Result<CaptureFormat, String>>,
        ) -> Result<(), String>
        + Send,
>;

/// Spawn the dedicated cpal capture thread. Blocks until the stream is set up
/// (or fails).
///
/// `permission` runs on the capture thread before the device is opened; each
/// callback then forwards its buffer through `tx` with `try_send`, never
/// blocking the realtime audio thread.
///
/// # Errors
///
/// The reason the stream could not start: the permission check's message, a
/// missing device or unsupported format, or the thread failing to spawn.
pub fn spawn_capture_thread(
    tx: tokio::sync::mpsc::Sender<RawChunk>,
    permission: PermissionCheck,
) -> Result<CaptureFormat, String> {
    spawn_stream_thread(
        tx,
        Box::new(move |tx, setup| super::device_stream::capture_on_thread(permission, tx, setup)),
    )
}

/// Run `body` on the stream thread and wait for its readiness report.
pub(crate) fn spawn_stream_thread(
    tx: tokio::sync::mpsc::Sender<RawChunk>,
    body: StreamBody,
) -> Result<CaptureFormat, String> {
    let (setup_tx, setup_rx) = std::sync::mpsc::sync_channel::<Result<CaptureFormat, String>>(1);
    std::thread::Builder::new()
        .name("voice-always-on".into())
        .spawn(move || {
            if let Err(e) = body(tx, &setup_tx) {
                log::warn!("{LOG_PREFIX} capture thread error: {e}");
                let _ = setup_tx.send(Err(e));
            }
        })
        .map_err(|e| format!("failed to spawn always-on capture thread: {e}"))?;
    match setup_rx.recv() {
        Ok(Ok(format)) => Ok(format),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("always-on capture thread exited before signalling readiness".to_string()),
    }
}

/// Forward one raw interleaved chunk per callback.
///
/// `try_send`, never `send`: this runs on a realtime audio thread where
/// blocking is a dropout, so a full queue drops the chunk rather than waiting
/// for the consumer to catch up. Dropping the newest chunk is the right end to
/// lose: the queue ahead of it is older speech that is closer to being
/// transcribed.
///
/// A send error also covers the consumer being gone (shutdown), which is why
/// neither case is fatal here.
pub(crate) fn forward(tx: &tokio::sync::mpsc::Sender<RawChunk>, samples: Vec<f32>) {
    if tx.try_send(RawChunk { samples }).is_err() {
        let dropped = DROPPED_CHUNKS.fetch_add(1, Ordering::Relaxed) + 1;
        // Log on a power-of-two schedule: a persistently overloaded consumer
        // should be visible without logging inside every callback once it
        // starts.
        if dropped.is_power_of_two() {
            log::warn!("{LOG_PREFIX} capture queue full; dropped {dropped} chunk(s) so far");
        }
    }
}

#[cfg(test)]
#[path = "chunks_test.rs"]
mod tests;
