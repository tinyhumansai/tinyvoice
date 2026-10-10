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
};

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
    fn start(&self) -> Result<Box<dyn Recording>, String>;
    fn stream(&self) -> Result<(tinyvoice_bus::capture::CaptureFormat, Box<dyn Stream>), String>;
}
#[derive(Debug)]
struct BusyGuard(Arc<AtomicBool>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
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
pub(super) struct Capture {
    backend: Arc<dyn Backend>,
    busy: Arc<AtomicBool>,
    recordings: Mutex<HashMap<CaptureHandle, Lease>>,
    outputs: Mutex<HashMap<CaptureHandle, Vec<u8>>>,
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
            recordings: Mutex::new(HashMap::new()),
            outputs: Mutex::new(HashMap::new()),
            streams: Mutex::new(HashMap::new()),
        }
    }
    pub(super) fn devices(&self) -> CaptureResult<Vec<String>> {
        self.backend.devices().map_err(CaptureError::Device)
    }
    pub(super) fn start(&self, permission: MicrophonePermission) -> CaptureResult<CaptureHandle> {
        if permission != MicrophonePermission::Granted {
            return Err(CaptureError::PermissionDenied);
        }
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| CaptureError::Busy)?;
        let busy = BusyGuard(self.busy.clone());
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
        if !request.gate_threshold.is_finite() || request.gate_threshold < 0.0 {
            return Err(CaptureError::InvalidParameters);
        }
        let Lease { recording, busy } = self.take(&request.handle)?;
        let raw = recording.finish().await.map_err(CaptureError::Device)?;
        let wav = tokio::task::spawn_blocking(move || prepare(&raw, request.gate_threshold))
            .await
            .map_err(|_| CaptureError::Device("audio preparation worker failed".into()))??;
        let mut outputs = self
            .outputs
            .lock()
            .map_err(|_| CaptureError::Device("audio output state unavailable".into()))?;
        let total: usize = outputs.values().map(Vec::len).sum();
        if outputs.len() >= MAX_OUTPUTS || wav.len() > MAX_OUTPUT_BYTES.saturating_sub(total) {
            return Err(CaptureError::LimitExceeded);
        }
        let handle = handle()?;
        let length = wav.len();
        outputs.insert(handle.clone(), wav);
        drop(busy);
        Ok(AudioOutput { handle, length })
    }
    pub(super) async fn cancel(&self, handle: &CaptureHandle) -> CaptureResult<()> {
        let Lease { recording, busy } = self.take(handle)?;
        // A canceled empty/failed recording is discarded, but the finish await
        // still waits for its device thread to drop the native stream.
        let _ = recording.finish().await;
        drop(busy);
        Ok(())
    }
    pub(super) fn stream_start(
        &self,
        permission: MicrophonePermission,
    ) -> CaptureResult<CaptureStream> {
        if permission != MicrophonePermission::Granted {
            return Err(CaptureError::PermissionDenied);
        }
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| CaptureError::Busy)?;
        let busy = BusyGuard(self.busy.clone());
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
        let StreamLease { stream, busy } = self
            .streams
            .lock()
            .map_err(|_| CaptureError::Device("capture stream state unavailable".into()))?
            .remove(handle)
            .ok_or(CaptureError::UnknownHandle)?;
        let result = stream.stop().await.map_err(CaptureError::Device);
        drop(busy);
        result
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
        self.outputs
            .lock()
            .map_err(|_| CaptureError::Device("audio output state unavailable".into()))?
            .remove(handle)
            .map(|_| ())
            .ok_or(CaptureError::UnknownHandle)
    }
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
        gate.push(&samples)
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
