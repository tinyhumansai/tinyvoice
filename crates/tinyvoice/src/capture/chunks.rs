//! A continuous capture stream: raw device chunks into a bounded channel.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::PermissionCheck;

const LOG_PREFIX: &str = "[voice::always_on]";

pub use tinyvoice_bus::capture::{CaptureFormat, RawChunk};

/// Maximum native samples retained in one callback buffer.
pub const MAX_CHUNK_SAMPLES: usize = 32_768;

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
            Arc<AtomicBool>,
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
) -> crate::Result<CaptureFormat> {
    spawn_stream_thread(
        tx,
        Box::new(move |stop, tx, setup| {
            super::device_stream::capture_on_thread(permission, stop, tx, setup)
        }),
    )
    .map_err(crate::Error::Capture)
}

/// Run `body` on the stream thread and wait for its readiness report.
pub(crate) fn spawn_stream_thread(
    tx: tokio::sync::mpsc::Sender<RawChunk>,
    body: StreamBody,
) -> Result<CaptureFormat, String> {
    let (format, mut handle) = spawn_tracked_stream(tx, body)?;
    // Compatibility API: receiver closure ends this stream, without returning
    // a handle. The tracked API additionally offers explicit stop and await.
    handle.stop_flag.take();
    Ok(format)
}

/// Owns a continuous capture thread. Dropping it requests shutdown.
#[derive(Debug)]
pub struct CaptureStreamHandle {
    stop_flag: Option<Arc<AtomicBool>>,
    completed: tokio::sync::oneshot::Receiver<Result<(), String>>,
}
impl CaptureStreamHandle {
    /// Request shutdown and wait until the native stream has been dropped.
    ///
    /// # Errors
    /// Returns a terminal device error or a vanished capture-thread error.
    pub async fn stop(mut self) -> crate::Result<()> {
        if let Some(stop) = &self.stop_flag {
            stop.store(true, Ordering::SeqCst);
        }
        (&mut self.completed)
            .await
            .map_err(|_| {
                crate::Error::Capture("capture stream task dropped before completing".into())
            })?
            .map_err(crate::Error::Capture)
    }
}
impl Drop for CaptureStreamHandle {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop_flag {
            stop.store(true, Ordering::SeqCst);
        }
    }
}
/// Start continuous capture with explicit shutdown ownership.
///
/// # Errors
/// Returns permission, device, format or thread startup errors.
pub fn start_capture_stream(
    tx: tokio::sync::mpsc::Sender<RawChunk>,
    permission: PermissionCheck,
) -> crate::Result<(CaptureFormat, CaptureStreamHandle)> {
    spawn_tracked_stream(
        tx,
        Box::new(move |stop, tx, setup| {
            super::device_stream::capture_on_thread(permission, stop, tx, setup)
        }),
    )
    .map_err(crate::Error::Capture)
}
fn spawn_tracked_stream(
    tx: tokio::sync::mpsc::Sender<RawChunk>,
    body: StreamBody,
) -> Result<(CaptureFormat, CaptureStreamHandle), String> {
    let (setup_tx, setup_rx) = std::sync::mpsc::sync_channel::<Result<CaptureFormat, String>>(1);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let worker_stop = stop_flag.clone();
    let (done_tx, completed) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("voice-always-on".into())
        .spawn(move || {
            let result = body(worker_stop, tx, &setup_tx);
            if let Err(error) = &result {
                let _ = setup_tx.send(Err(error.clone()));
            }
            let _ = done_tx.send(result);
        })
        .map_err(|error| format!("failed to spawn always-on capture thread: {error}"))?;
    let handle = CaptureStreamHandle {
        stop_flag: Some(stop_flag),
        completed,
    };
    match setup_rx.recv() {
        Ok(Ok(format)) => Ok((format, handle)),
        Ok(Err(error)) => Err(error),
        Err(_) => Err("always-on capture thread exited before signalling readiness".into()),
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
    if samples.len() > MAX_CHUNK_SAMPLES || tx.try_send(RawChunk { samples }).is_err() {
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
#[path = "chunks_tests.rs"]
mod tests;

/// Wait for native failure or closure. Device I/O remains on its owning thread.
pub(super) fn wait_for_stream_end(
    tx: &tokio::sync::mpsc::Sender<RawChunk>,
    stop: &std::sync::atomic::AtomicBool,
    errors: &std::sync::mpsc::Receiver<String>,
) -> Result<(), String> {
    loop {
        if stop.load(Ordering::SeqCst) {
            return match errors.try_recv() {
                Ok(error) => Err(error),
                Err(_) => Ok(()),
            };
        }
        if tx.is_closed() {
            return Ok(());
        }
        match errors.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(error) => return Err(error),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}
