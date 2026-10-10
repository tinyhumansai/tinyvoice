//! Bounded, transport-neutral hotkey lifecycle vocabulary.
use serde::{Deserialize, Serialize};

/// Opaque hotkey reservation, unique within one module process.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HotkeyHandle(pub String);

/// Existing tap-toggle or push-to-talk behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    /// A fresh down toggles activation.
    Tap,
    /// A down activates until its matching up.
    Push,
}

/// Source of activation facts for a listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeySource {
    /// Use a module-owned platform listener.
    Native,
    /// Accept sequenced generic facts from the host.
    Host,
}

/// The input chord and mode requested by the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyRequest {
    /// Existing `TinyVoice` key spelling, including supported aliases.
    pub key: String,
    /// Tap or push activation behavior.
    pub mode: ActivationMode,
    /// Native listener or host-fed facts (macOS).
    pub source: HotkeySource,
}

/// Reserve a hotkey lease before potentially fallible native startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyReserveRequest {
    /// Requested chord and source.
    pub request: HotkeyRequest,
}

/// Handle used by start, read, feed, and stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyHandleRequest {
    /// Reserved listener.
    pub handle: HotkeyHandle,
}

/// Start a reserved listener.
pub type HotkeyStartRequest = HotkeyHandleRequest;

/// Start or feed acknowledgment with the current listener status.
pub type HotkeyReply = HotkeyStatus;

/// Read retained events, acknowledging only the exact prior batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyReadRequest {
    /// Reserved listener.
    pub handle: HotkeyHandle,
    /// Batch previously returned, if the caller consumed it.
    pub acknowledged_batch: Option<u64>,
}

/// Whether the activation reducer is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyEvent {
    /// Begin activation.
    Pressed,
    /// End activation.
    Released,
}

/// Host-owned physical fact. No native key or Computer type crosses the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKeyFact {
    /// Configured source became pressed.
    Down,
    /// Configured source became released.
    Up,
}

/// One sequenced host fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencedHostFact {
    /// Monotonic source sequence.
    pub sequence: u64,
    /// Generic activation input.
    pub fact: HostKeyFact,
}

/// Feed a bounded batch of Computer-derived generic activation facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyFeedRequest {
    /// Reserved host-fed listener.
    pub handle: HotkeyHandle,
    /// Generation returned by start, fencing replaced source leases.
    pub generation: u64,
    /// At most 64 source facts.
    pub facts: Vec<SequencedHostFact>,
    /// Upstream reported that facts were lost.
    pub overflow: bool,
}

/// Lifecycle phase of one listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyState {
    /// Reserved and not started.
    Reserved,
    /// Native startup is in progress.
    Starting,
    /// Accepting source facts.
    Running,
    /// Cleanup is in progress.
    Stopping,
    /// Stopped and no longer accepting facts.
    Stopped,
}

/// Public status contains no native or permission details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyStatus {
    /// Current lifecycle phase.
    pub state: HotkeyState,
    /// Listener generation, when startup began.
    pub generation: Option<u64>,
    /// Current reducer state.
    pub active: bool,
    /// Continuity was lost and the reducer was reset.
    pub continuity_lost: bool,
}

/// One ordered activation fact emitted by the reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencedHotkeyEvent {
    /// Listener generation.
    pub generation: u64,
    /// Monotonic listener event sequence.
    pub sequence: u64,
    /// Pressed or Released activation.
    pub event: HotkeyEvent,
}

/// Replayable read batch. The same batch remains until acknowledged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyBatch {
    /// Listener generation.
    pub generation: u64,
    /// Batch identity acknowledged by the next read.
    pub batch: u64,
    /// Ordered retained activation events.
    pub events: Vec<SequencedHotkeyEvent>,
    /// Reader must treat activation as inactive and await a fresh release.
    pub reset: bool,
    /// Current active state after the batch.
    pub active: bool,
}

/// Fixed, content-free hotkey failure codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyError {
    /// Permission was not granted by the host.
    PermissionDenied,
    /// Module or listener is closed.
    Closed,
    /// Listener capacity or operation is busy.
    Busy,
    /// Handle is unknown, expired, or already released.
    UnknownHandle,
    /// Request is malformed or incompatible with the source.
    InvalidRequest,
    /// A bounded resource or payload limit was reached.
    LimitExceeded,
    /// Request belongs to an old listener generation.
    StaleGeneration,
    /// Source sequence is duplicate or discontinuous.
    SequenceGap,
    /// Native cleanup failed and may be retried.
    CleanupFailed,
    /// This platform/source is not supported.
    Unsupported,
}

/// Typed result returned by lifecycle operations.
pub type HotkeyResult<T> = Result<T, HotkeyError>;

#[cfg(test)]
#[path = "hotkey_tests.rs"]
mod tests;
