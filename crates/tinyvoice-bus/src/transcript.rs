//! Shared hallucination-detection modes.

use serde::{Deserialize, Serialize};

/// How aggressively a transcript should be screened for hallucinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Aggressive screening for push-to-talk dictation.
    Dictation,
    /// Conservative screening for conversational input.
    Conversation,
}
