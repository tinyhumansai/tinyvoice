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
/// Start a recording using an explicit permission decision.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecordingStartRequest {
    /// Permission granted through the computer module.
    pub permission: MicrophonePermission,
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
    /// Lease released with `ReleaseAudioOutput`.
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
