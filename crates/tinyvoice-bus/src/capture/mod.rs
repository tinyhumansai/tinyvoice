//! Native capture lifecycle vocabulary; no devices or audio processing live here.
use serde::{Deserialize, Serialize};
/// Permission decision obtained by the host through the computer module.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicrophonePermission {
    /// Capture may proceed.
    Granted,
    /// Capture is refused, including unknown or unsupported permission states.
    #[default]
    Denied,
}
/// Opaque module-owned recording or audio output handle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CaptureHandle(pub String);
/// Reserve or start capture using an explicit permission decision.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecordingStartRequest {
    /// Permission granted through the computer module.
    pub permission: MicrophonePermission,
    /// Module-generated reservation obtained before starting native setup.
    #[serde(default)]
    pub handle: Option<CaptureHandle>,
}
/// Finish capture and run the module's existing preparation pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingFinishRequest {
    /// Recording lease.
    pub handle: CaptureHandle,
    /// Existing silence-gate threshold; zero disables gating.
    pub gate_threshold: f32,
}
/// Prepared WAV held inside the module for bounded reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioOutput {
    /// Caller-known recording lease, released with `ReleaseAudioOutput`.
    pub handle: CaptureHandle,
    /// WAV bytes available.
    pub length: usize,
}
/// Read a bounded slice of a held audio output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadAudioRequest {
    /// Output lease.
    pub handle: CaptureHandle,
    /// Byte offset.
    pub offset: usize,
    /// Maximum bytes to read.
    pub length: usize,
}
/// Capture failure for product presentation, never a telemetry payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", content = "detail", rename_all = "snake_case")]
pub enum CaptureError {
    /// Permission was not explicitly granted.
    PermissionDenied,
    /// Startup was canceled before its resource was delivered.
    Cancelled,
    /// Capture shutdown has closed this module instance.
    Closed,
    /// A recording or finishing operation already owns the device slot.
    Busy,
    /// The recording or output handle is unknown or released.
    UnknownHandle,
    /// Output storage or read bounds were exceeded.
    LimitExceeded,
    /// Invalid processing parameters.
    InvalidParameters,
    /// Device or processing failure; contains product-facing native detail.
    Device(String),
}
/// Typed terminal result; provider detail must not enter telemetry.
pub type CaptureResult<T> = Result<T, CaptureError>;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

/// Native device format, reported once on stream startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureFormat {
    /// Native sample rate.
    pub source_rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
}
/// Bounded native callback buffer, processed through module audio operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawChunk {
    /// Interleaved native f32 samples.
    pub samples: Vec<f32>,
}
/// Opaque continuous capture lease and its native format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureStream {
    /// Lease consumed by `CaptureStop`.
    pub handle: CaptureHandle,
    /// Native device format.
    pub format: CaptureFormat,
}
/// Bounded batch read request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturePollRequest {
    /// Continuous capture lease.
    pub handle: CaptureHandle,
    /// At most two callback buffers per call.
    pub max_chunks: usize,
}
/// Ordered bounded chunks, with terminal channel status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaptureBatch {
    /// Retained native chunks in capture order.
    pub chunks: Vec<RawChunk>,
    /// Native capture ended; stop releases the lease and reports its result.
    pub closed: bool,
}
