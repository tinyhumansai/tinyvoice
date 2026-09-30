//! A one-shot recording: start it, later stop it, get the raw samples.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use log::{debug, info};
use tokio::sync::oneshot;

use super::PermissionCheck;

const LOG_PREFIX: &str = "[voice_capture]";

/// Most raw samples one recording may accumulate.
///
/// Five minutes of 48 kHz stereo (the widest common device format) is 57.6M
/// `f32`, so the cap is expressed in samples and bounds memory at about 230 MiB
/// worst case. The callback accumulates every raw interleaved sample and gating
/// happens later, so without a cap a recording that is started and never
/// stopped would grow without limit.
pub(crate) const MAX_RAW_SAMPLES: usize = 48_000 * 2 * 60 * 5;

/// What the capture thread produces: the device's own samples, untouched.
#[derive(Debug)]
pub struct RawRecording {
    /// Interleaved `f32` samples at the device's own rate.
    pub samples: Vec<f32>,
    /// Device sample rate.
    pub source_rate: u32,
    /// Interleaved channel count.
    pub channels: usize,
}

/// The lifecycle body of one recording thread.
///
/// Receives the stop flag to poll and the channel on which to report whether
/// setup succeeded; returns the raw recording once stopped. Exactly what
/// [`super::device_recording::record_on_thread`] is, abstracted so the handle
/// logic can be exercised without an audio device.
pub(crate) type RecordingBody = Box<
    dyn FnOnce(
            Arc<AtomicBool>,
            std::sync::mpsc::SyncSender<Result<(), String>>,
        ) -> Result<RawRecording, String>
        + Send,
>;

/// Handle to a recording in progress. Call [`stop`](Self::stop) to end it.
#[derive(Debug)]
pub struct RecordingHandle {
    stop_flag: Arc<AtomicBool>,
    result_rx: oneshot::Receiver<Result<RawRecording, String>>,
}

impl RecordingHandle {
    /// Signal the recording to stop and wait for the raw capture.
    ///
    /// # Errors
    ///
    /// The capture error if the recording itself failed (for example, no
    /// samples were captured), or a message if the capture task vanished.
    pub async fn stop(mut self) -> crate::Result<RawRecording> {
        self.stop_flag.store(true, Ordering::SeqCst);
        debug!("{LOG_PREFIX} stop signal sent");
        (&mut self.result_rx)
            .await
            .map_err(|_| crate::Error::Capture("recording task dropped before completing".into()))?
            .map_err(crate::Error::Capture)
    }
}

impl Drop for RecordingHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }
}

/// Start recording from the default microphone.
///
/// `permission` runs on the capture thread before the device is opened. The
/// recording runs on a dedicated OS thread because `cpal::Stream` is `!Send`
/// (it must be created and dropped on the same thread).
///
/// # Errors
///
/// The reason capture could not start: the permission check's message, a
/// missing device, an unsupported format, or the thread failing to spawn.
pub fn start_recording(permission: PermissionCheck) -> crate::Result<RecordingHandle> {
    spawn_recording(Box::new(move |stop, setup| {
        super::device_recording::record_on_thread(permission, stop, setup)
    }))
    .map_err(crate::Error::Capture)
}

/// Run `body` on the recording thread and wait for its readiness report.
pub(crate) fn spawn_recording(body: RecordingBody) -> Result<RecordingHandle, String> {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_clone = stop_flag.clone();
    let (result_tx, result_rx) = oneshot::channel();

    // Use a channel to report whether stream setup succeeded.
    let (setup_tx, setup_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);

    std::thread::Builder::new()
        .name("voice-capture".into())
        .spawn(move || {
            // All cpal objects are created and used on this thread.
            let result = body(stop_flag_clone, setup_tx);
            let _ = result_tx.send(result);
        })
        .map_err(|e| format!("failed to spawn capture thread: {e}"))?;

    // Wait for the stream to be set up (or an error).
    match setup_rx.recv() {
        Ok(Ok(())) => {
            info!("{LOG_PREFIX} recording started");
            Ok(RecordingHandle {
                stop_flag,
                result_rx,
            })
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err("capture thread exited before signalling readiness".to_string()),
    }
}

/// Append `samples` to `buffer`, stopping at [`MAX_RAW_SAMPLES`].
///
/// Truncates rather than dropping the whole chunk, so a recording that reaches
/// the cap keeps its first five minutes instead of losing the chunk that
/// crossed the line. Silent about it by design: this runs in the audio
/// callback, where logging on every chunk past the cap would be its own problem.
pub(crate) fn append_capped(buffer: &parking_lot::Mutex<Vec<f32>>, samples: &[f32]) {
    let mut guard = buffer.lock();
    let remaining = MAX_RAW_SAMPLES.saturating_sub(guard.len());
    if remaining == 0 {
        return;
    }
    guard.extend_from_slice(&samples[..samples.len().min(remaining)]);
}

#[cfg(test)]
#[path = "recording_test.rs"]
mod tests;
