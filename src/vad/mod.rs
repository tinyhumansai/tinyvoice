//! Voice-activity segmentation: carving a continuous stream into utterances.
//!
//! [`VadSegmenter`] is a pure state machine over per-frame energy. It never
//! sees a sample — a host computes [`crate::audio::rms`] for each fixed-size
//! frame and pushes the number. That is what makes it exhaustively testable
//! without an audio backend, and it is why the type carries no buffer: the host
//! already holds the audio it is accumulating, and a second copy in here would
//! be both wasted memory and a second thing to keep in sync.

#[cfg(test)]
mod test;

use serde::{Deserialize, Serialize};

/// Tuning for [`VadSegmenter`].
///
/// There is deliberately no `from_config`-style constructor. A host builds this
/// from whatever it persists; a crate that guessed at that shape would be wrong
/// for every host that guessed differently.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VadConfig {
    /// Peak-RMS energy above which a frame counts as speech.
    pub onset_threshold: f32,
    /// How long energy must stay below `onset_threshold` before the current
    /// utterance is closed. Bridges natural mid-sentence pauses.
    pub hangover_ms: u32,
    /// Minimum voiced duration for a segment to be emitted; shorter blips
    /// (a cough, a door) are dropped.
    pub min_speech_ms: u32,
    /// Hard ceiling on a single utterance, so a continuous noise source cannot
    /// grow an unbounded recording.
    pub max_utterance_ms: u32,
}

impl Default for VadConfig {
    /// The tuning `OpenHuman`'s always-on listener shipped with.
    fn default() -> Self {
        Self {
            onset_threshold: 0.01,
            hangover_ms: 800,
            min_speech_ms: 300,
            max_utterance_ms: 30_000,
        }
    }
}

/// An event emitted as the stream is consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VadEvent {
    /// Energy crossed the onset threshold — an utterance has begun.
    SpeechStart,
    /// An utterance closed.
    SpeechEnd {
        /// Accumulated speech duration, excluding the trailing silence.
        voiced_ms: u32,
        /// False when `voiced_ms` fell below `min_speech_ms` — the host should
        /// discard the audio rather than transcribe it.
        emit: bool,
        /// True when the close was forced by `max_utterance_ms` rather than by
        /// a silence hangover. A host may want to keep the microphone hot
        /// across a forced close, since the speaker probably has not stopped.
        forced: bool,
    },
}

#[derive(Debug, Clone, Copy)]
enum State {
    /// No active utterance — waiting for energy to cross the onset threshold.
    Silent,
    /// Inside an utterance.
    Speaking {
        /// Elapsed time since the utterance opened (voiced + silence).
        total_ms: u32,
        /// Accumulated voiced time (frames above onset).
        voiced_ms: u32,
        /// Consecutive below-onset time since the last voiced frame.
        silence_run_ms: u32,
    },
}

/// Segments a stream of frame energies into utterances.
///
/// Drive it by calling [`push_frame`](Self::push_frame) once per fixed-size
/// audio frame; it returns at most one [`VadEvent`] per frame.
#[derive(Debug)]
pub struct VadSegmenter {
    config: VadConfig,
    state: State,
}

impl VadSegmenter {
    /// Build a segmenter with the given tuning.
    #[must_use]
    pub fn new(config: VadConfig) -> Self {
        Self {
            config,
            state: State::Silent,
        }
    }

    /// True while inside an utterance (between `SpeechStart` and `SpeechEnd`).
    #[must_use]
    pub fn is_speaking(&self) -> bool {
        matches!(self.state, State::Speaking { .. })
    }

    /// Abort any in-flight utterance and return to idle without emitting an
    /// event.
    ///
    /// This is the privacy hook: a host calls it when the screen locks or
    /// capture is revoked, and the partial utterance is dropped rather than
    /// completed and transcribed.
    pub fn reset(&mut self) {
        self.state = State::Silent;
    }

    /// Feed one frame's RMS energy and its duration in milliseconds.
    pub fn push_frame(&mut self, rms: f32, frame_ms: u32) -> Option<VadEvent> {
        let above = rms >= self.config.onset_threshold;
        match self.state {
            State::Silent => {
                if !above {
                    return None;
                }
                self.state = State::Speaking {
                    total_ms: frame_ms,
                    voiced_ms: frame_ms,
                    silence_run_ms: 0,
                };
                Some(VadEvent::SpeechStart)
            }
            State::Speaking {
                mut total_ms,
                mut voiced_ms,
                mut silence_run_ms,
            } => {
                total_ms = total_ms.saturating_add(frame_ms);
                if above {
                    voiced_ms = voiced_ms.saturating_add(frame_ms);
                    silence_run_ms = 0;
                } else {
                    silence_run_ms = silence_run_ms.saturating_add(frame_ms);
                }

                // Close on a silence hangover, or on the hard ceiling. The
                // hangover is checked first: when a frame satisfies both, the
                // speaker has actually stopped, and reporting that as `forced`
                // would tell the host to keep listening when it need not.
                let forced = if silence_run_ms >= self.config.hangover_ms {
                    false
                } else if total_ms >= self.config.max_utterance_ms {
                    true
                } else {
                    self.state = State::Speaking {
                        total_ms,
                        voiced_ms,
                        silence_run_ms,
                    };
                    return None;
                };

                self.state = State::Silent;
                Some(VadEvent::SpeechEnd {
                    voiced_ms,
                    emit: voiced_ms >= self.config.min_speech_ms,
                    forced,
                })
            }
        }
    }
}
