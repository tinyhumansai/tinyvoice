//! Native recording/stream ownership and bounded prepared audio outputs.
use super::BASE64;
use base64::Engine as _;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tinyvoice::capture::RawRecording;
use tinyvoice_bus::capture::{
    AudioOutput, CaptureBatch, CaptureError, CaptureHandle, CapturePollRequest, CaptureResult,
    CaptureStream, MicrophonePermission, ReadAudioRequest, RecordingFinishRequest,
    RecordingStartRequest,
};

const MAX_RESERVATIONS: usize = 16;
const RESERVATION_TTL: std::time::Duration = std::time::Duration::from_secs(60);
const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_OUTPUTS: usize = 4;
const MAX_READ_BYTES: usize = 256 * 1024;
type StreamFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
pub(super) trait Stream: std::fmt::Debug + Send {
    fn poll(&mut self, max_chunks: usize) -> CaptureResult<CaptureBatch>;
    fn stop(self: Box<Self>) -> StreamFuture;
}
type RecordingFuture = Pin<Box<dyn Future<Output = Result<RawRecording, String>> + Send>>;
pub(super) trait Recording: std::fmt::Debug + Send {
    fn finish(self: Box<Self>) -> RecordingFuture;
}
pub(super) trait Backend: std::fmt::Debug + Send + Sync {
    fn devices(&self) -> Result<Vec<String>, String>;
    fn prepare(&self, raw: &RawRecording, threshold: f32) -> CaptureResult<Vec<u8>> {
        prepare(raw, threshold)
    }
    fn start(&self) -> Result<Box<dyn Recording>, String>;
    fn stream(&self) -> Result<(tinyvoice_bus::capture::CaptureFormat, Box<dyn Stream>), String>;
}
#[derive(Debug)]
struct BusyGuard(Arc<AtomicBool>, tokio::sync::watch::Sender<bool>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
        self.1.send_replace(true);
    }
}
#[derive(Debug)]
struct Lease {
    recording: Box<dyn Recording>,
    busy: BusyGuard,
}
#[derive(Debug)]
struct StreamLease {
    stream: Box<dyn Stream>,
    busy: BusyGuard,
}
#[derive(Debug)]
struct PendingStart {
    canceled: AtomicBool,
    completed: tokio::sync::watch::Sender<bool>,
}
#[derive(Debug)]
struct Reservation {
    created: std::time::Instant,
    pending: Option<Arc<PendingStart>>,
}
#[derive(Debug)]
pub(super) enum Started {
    Recording(CaptureHandle),
    Stream(CaptureStream),
}
#[derive(Debug)]
pub(super) struct Capture {
    backend: Arc<dyn Backend>,
    busy: Arc<AtomicBool>,
    idle: tokio::sync::watch::Sender<bool>,
    closed: AtomicBool,
    recordings: Mutex<HashMap<CaptureHandle, Lease>>,
    outputs: Arc<Mutex<HashMap<CaptureHandle, Vec<u8>>>>,
    finishing: Arc<Mutex<HashMap<CaptureHandle, Arc<AtomicBool>>>>,
    reservations: Mutex<HashMap<CaptureHandle, Reservation>>,
    streams: Mutex<HashMap<CaptureHandle, StreamLease>>,
}
impl Default for Capture {
    fn default() -> Self {
        Self::new(Arc::new(device_native::Native))
    }
}
impl Capture {
    pub(super) fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            busy: Arc::new(AtomicBool::new(false)),
            idle: tokio::sync::watch::channel(true).0,
            closed: AtomicBool::new(false),
            recordings: Mutex::new(HashMap::new()),
            outputs: Arc::new(Mutex::new(HashMap::new())),
            finishing: Arc::new(Mutex::new(HashMap::new())),
            reservations: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
        }
    }
    pub(super) async fn shutdown(self: &Arc<Self>) -> CaptureResult<()> {
        // Freeze publication under the same lock used by startup workers.
        let pending = {
            let mut reservations = self
                .reservations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.closed.store(true, Ordering::SeqCst);
            let pending: Vec<_> = reservations
                .values()
                .filter_map(|reservation| reservation.pending.clone())
                .collect();
            for start in &pending {
                start.canceled.store(true, Ordering::SeqCst);
            }
            reservations.retain(|_, reservation| reservation.pending.is_some());
            pending
        };
        {
            let finishing = self
                .finishing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for canceled in finishing.values() {
                canceled.store(true, Ordering::SeqCst);
            }
            self.outputs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
        let capture = self.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let recordings: Vec<_> = capture
                .recordings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain()
                .map(|(_, lease)| lease)
                .collect();
            let streams: Vec<_> = capture
                .streams
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain()
                .map(|(_, lease)| lease)
                .collect();
            for Lease { recording, busy } in recordings {
                let _ = runtime.block_on(recording.finish());
                drop(busy);
            }
            for StreamLease { stream, busy } in streams {
                let _ = runtime.block_on(stream.stop());
                drop(busy);
            }
            runtime.block_on(wait_for_idle(&capture.busy, capture.idle.subscribe()))?;
            for start in pending {
                let mut completed = start.completed.subscribe();
                while !*completed.borrow_and_update() {
                    if runtime.block_on(completed.changed()).is_err() {
                        return Err(state_error());
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| state_error())?
    }
    pub(super) fn devices(&self) -> CaptureResult<Vec<String>> {
        self.backend.devices().map_err(CaptureError::Device)
    }
    #[cfg(test)]
    pub(super) fn start(&self, permission: MicrophonePermission) -> CaptureResult<CaptureHandle> {
        if permission != MicrophonePermission::Granted {
            return Err(CaptureError::PermissionDenied);
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(CaptureError::Closed);
        }
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| CaptureError::Busy)?;
        self.idle.send_replace(false);
        let busy = BusyGuard(self.busy.clone(), self.idle.clone());
        let handle = handle()?;
        let recording = self.backend.start().map_err(CaptureError::Device)?;
        self.recordings
            .lock()
            .map_err(|_| CaptureError::Device("capture state unavailable".into()))?
            .insert(handle.clone(), Lease { recording, busy });
        Ok(handle)
    }
    fn take(&self, handle: &CaptureHandle) -> CaptureResult<Lease> {
        self.recordings
            .lock()
            .map_err(|_| CaptureError::Device("capture state unavailable".into()))?
            .remove(handle)
            .ok_or(CaptureError::UnknownHandle)
    }
    pub(super) async fn finish(
        &self,
        request: RecordingFinishRequest,
    ) -> CaptureResult<AudioOutput> {
        let (recording, busy, canceled) = {
            let mut finishing = self.finishing.lock().map_err(|_| state_error())?;
            if self.closed.load(Ordering::SeqCst) {
                return Err(CaptureError::Closed);
            }
            if !request.gate_threshold.is_finite() || request.gate_threshold < 0.0 {
                return Err(CaptureError::InvalidParameters);
            }
            let Lease { recording, busy } = self.take(&request.handle)?;
            let canceled = Arc::new(AtomicBool::new(false));
            finishing.insert(request.handle.clone(), canceled.clone());
            (recording, busy, canceled)
        };
        let backend = self.backend.clone();
        let outputs = self.outputs.clone();
        let finishing = self.finishing.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let result = (|| {
                let raw = runtime
                    .block_on(recording.finish())
                    .map_err(CaptureError::Device)?;
                let wav = backend.prepare(&raw, request.gate_threshold)?;
                let _finish_lock = finishing.lock().map_err(|_| state_error())?;
                if canceled.load(Ordering::SeqCst) {
                    return Err(CaptureError::Cancelled);
                }
                let mut outputs = outputs.lock().map_err(|_| state_error())?;
                let total: usize = outputs.values().map(Vec::len).sum();
                if outputs.len() >= MAX_OUTPUTS
                    || wav.len() > MAX_OUTPUT_BYTES.saturating_sub(total)
                {
                    return Err(CaptureError::LimitExceeded);
                }
                let length = wav.len();
                outputs.insert(request.handle.clone(), wav);
                Ok(AudioOutput {
                    handle: request.handle.clone(),
                    length,
                })
            })();
            if let Ok(mut finishing) = finishing.lock() {
                finishing.remove(&request.handle);
            }
            drop(busy);
            result
        })
        .await
        .map_err(|_| state_error())?
    }
    pub(super) async fn cancel(&self, handle: &CaptureHandle) -> CaptureResult<()> {
        let was_pending = self.cancel_pending(handle).await?;
        let lease = match self.take(handle) {
            Ok(lease) => lease,
            Err(CaptureError::UnknownHandle) if was_pending => return Ok(()),
            Err(error) => return Err(error),
        };
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let Lease { recording, busy } = lease;
            let _ = runtime.block_on(recording.finish());
            drop(busy);
        })
        .await
        .map_err(|_| state_error())?;
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn stream_start(
        &self,
        permission: MicrophonePermission,
    ) -> CaptureResult<CaptureStream> {
        if permission != MicrophonePermission::Granted {
            return Err(CaptureError::PermissionDenied);
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(CaptureError::Closed);
        }
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| CaptureError::Busy)?;
        self.idle.send_replace(false);
        let busy = BusyGuard(self.busy.clone(), self.idle.clone());
        let handle = handle()?;
        let (format, stream) = self.backend.stream().map_err(CaptureError::Device)?;
        self.streams
            .lock()
            .map_err(|_| CaptureError::Device("capture stream state unavailable".into()))?
            .insert(handle.clone(), StreamLease { stream, busy });
        Ok(CaptureStream { handle, format })
    }
    pub(super) fn stream_poll(&self, request: &CapturePollRequest) -> CaptureResult<CaptureBatch> {
        if !(1..=2).contains(&request.max_chunks) {
            return Err(CaptureError::LimitExceeded);
        }
        self.streams
            .lock()
            .map_err(|_| CaptureError::Device("capture stream state unavailable".into()))?
            .get_mut(&request.handle)
            .ok_or(CaptureError::UnknownHandle)?
            .stream
            .poll(request.max_chunks)
    }
    pub(super) async fn stream_stop(&self, handle: &CaptureHandle) -> CaptureResult<()> {
        let was_pending = self.cancel_pending(handle).await?;
        let lease = self
            .streams
            .lock()
            .map_err(|_| state_error())?
            .remove(handle);
        let Some(lease) = lease else {
            return if was_pending {
                Ok(())
            } else {
                Err(CaptureError::UnknownHandle)
            };
        };
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let StreamLease { stream, busy } = lease;
            let result = runtime
                .block_on(stream.stop())
                .map_err(CaptureError::Device);
            drop(busy);
            result
        })
        .await
        .map_err(|_| state_error())?
    }
    pub(super) fn read(&self, request: &ReadAudioRequest) -> CaptureResult<String> {
        if request.length == 0 || request.length > MAX_READ_BYTES {
            return Err(CaptureError::LimitExceeded);
        }
        let outputs = self
            .outputs
            .lock()
            .map_err(|_| CaptureError::Device("audio output state unavailable".into()))?;
        let wav = outputs
            .get(&request.handle)
            .ok_or(CaptureError::UnknownHandle)?;
        if request.offset > wav.len() {
            return Err(CaptureError::InvalidParameters);
        }
        let end = request.offset.saturating_add(request.length).min(wav.len());
        Ok(BASE64.encode(&wav[request.offset..end]))
    }
    pub(super) fn release(&self, handle: &CaptureHandle) -> CaptureResult<()> {
        let finishing = self.finishing.lock().map_err(|_| state_error())?;
        if let Some(canceled) = finishing.get(handle) {
            canceled.store(true, Ordering::SeqCst);
            self.outputs
                .lock()
                .map_err(|_| state_error())?
                .remove(handle);
            return Ok(());
        }
        self.outputs
            .lock()
            .map_err(|_| state_error())?
            .remove(handle)
            .map(|_| ())
            .ok_or(CaptureError::UnknownHandle)
    }
}
/// Notifications can arrive out of order across owners; only the slot is authoritative.
async fn wait_for_idle(
    busy: &AtomicBool,
    mut idle: tokio::sync::watch::Receiver<bool>,
) -> CaptureResult<()> {
    loop {
        // Register this notification generation before reading the predicate,
        // so completion between the read and changed().await cannot be missed.
        let _ = idle.borrow_and_update();
        if !busy.load(Ordering::SeqCst) {
            return Ok(());
        }
        idle.changed().await.map_err(|_| state_error())?;
    }
}

fn state_error() -> CaptureError {
    CaptureError::Device("capture state unavailable".into())
}
fn handle() -> CaptureResult<CaptureHandle> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| CaptureError::Device("capture entropy unavailable".into()))?;
    Ok(CaptureHandle(format!(
        "{:032x}",
        u128::from_le_bytes(bytes)
    )))
}
fn prepare(raw: &RawRecording, threshold: f32) -> CaptureResult<Vec<u8>> {
    use tinyvoice::audio::{self, SilenceGateConfig};
    let channels = u16::try_from(raw.channels).map_err(|_| CaptureError::InvalidParameters)?;
    let mono = audio::to_mono(&raw.samples, channels)
        .map_err(|error| CaptureError::Device(error.to_string()))?;
    if raw.source_rate == 0 {
        return Err(CaptureError::InvalidParameters);
    }
    let target_samples = mono
        .len()
        .checked_mul(audio::STT_SAMPLE_RATE as usize)
        .ok_or(CaptureError::LimitExceeded)?
        .div_ceil(raw.source_rate as usize);
    if target_samples > (MAX_OUTPUT_BYTES - 44) / 2 {
        return Err(CaptureError::LimitExceeded);
    }
    let samples = audio::resample(&mono, raw.source_rate, audio::STT_SAMPLE_RATE)
        .map_err(|error| CaptureError::Device(error.to_string()))?;
    let samples = if threshold > 0.0 {
        let mut gate = audio::SilenceGate::new(
            SilenceGateConfig {
                threshold,
                ..Default::default()
            },
            audio::STT_SAMPLE_RATE,
        )
        .map_err(|error| CaptureError::Device(error.to_string()))?;
        let frame_samples = (audio::STT_SAMPLE_RATE as usize / 100).max(1);
        let mut gated = Vec::with_capacity(samples.len());
        for frame in samples.chunks(frame_samples) {
            gated.extend(gate.push(frame));
        }
        gated
    } else {
        samples
    };
    let wav = audio::f32_mono_to_wav(&samples, audio::STT_SAMPLE_RATE)
        .map_err(|error| CaptureError::Device(error.to_string()))?;
    if wav.len() > MAX_OUTPUT_BYTES {
        return Err(CaptureError::LimitExceeded);
    }
    Ok(wav)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
pub(super) fn fixture_capture() -> Arc<Capture> {
    Arc::new(tests::fixture_capture())
}

mod device_native;
mod startup;

fn poll_chunks(
    rx: &mut tokio::sync::mpsc::Receiver<tinyvoice::capture::RawChunk>,
    max_chunks: usize,
) -> CaptureBatch {
    let mut batch = CaptureBatch::default();
    for _ in 0..max_chunks {
        match rx.try_recv() {
            Ok(chunk) => batch.chunks.push(chunk),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                batch.closed = true;
                break;
            }
        }
    }
    batch
}
