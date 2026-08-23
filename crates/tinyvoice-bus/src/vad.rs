//! Shared voice-activity configuration and event types.

use serde::{Deserialize, Serialize};

/// Tuning for a voice-activity segmenter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VadConfig {
    /// Peak-RMS energy above which a frame counts as speech.
    pub onset_threshold: f32,
    /// Silence needed to close an active utterance.
    pub hangover_ms: u32,
    /// Minimum voiced duration an utterance needs to be emitted.
    pub min_speech_ms: u32,
    /// Hard ceiling on one utterance.
    pub max_utterance_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            onset_threshold: 0.01,
            hangover_ms: 800,
            min_speech_ms: 300,
            max_utterance_ms: 30_000,
        }
    }
}

/// An event emitted while segmenting voice activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VadEvent {
    /// Energy crossed the onset threshold.
    SpeechStart,
    /// An utterance closed.
    SpeechEnd {
        /// Accumulated voiced duration excluding trailing silence.
        voiced_ms: u32,
        /// Whether the utterance was long enough to emit.
        emit: bool,
        /// Whether the maximum duration, rather than silence, closed it.
        forced: bool,
    },
}

/// A VAD event paired with the zero-based frame that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedVadEvent {
    /// Zero-based index in the submitted energy buffer.
    pub frame: usize,
    /// The event reported at that frame.
    #[serde(flatten)]
    pub event: VadEvent,
}
