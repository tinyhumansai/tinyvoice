//! Shared fast-path command routing values.

use serde::{Deserialize, Serialize};

/// A recognised fast-path voice command, or an unknown command for the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case")]
pub enum VoiceIntent {
    /// Play a media query.
    Play {
        /// The cleaned search query.
        query: String,
    },
    /// Pause playback.
    Pause,
    /// Resume playback.
    Resume,
    /// Skip to the next track.
    Next,
    /// Go to the previous track.
    Previous,
    /// Open an application.
    OpenApp {
        /// The cleaned application name.
        app: String,
    },
    /// Set absolute volume.
    SetVolume {
        /// Target percentage, `0..=100`.
        percent: u8,
    },
    /// Raise volume.
    VolumeUp,
    /// Lower volume.
    VolumeDown,
    /// Mute audio output.
    Mute,
    /// Unmute audio output.
    Unmute,
    /// Defer an unrecognised command to the host.
    ///
    /// `#[serde(other)]` is the forward-compatibility rule, not a detail.
    /// [`is_compatible`](crate::is_compatible) lets a host bind a module whose
    /// minor version is *ahead* of its own, so a host can be handed an intent
    /// this build has never heard of. Degrading that to `Unknown` sends the
    /// utterance to the agent, which is what an unrecognised command is
    /// supposed to do; without it the decode fails instead, and a
    /// forward-compatible addition upstream becomes a broken call downstream.
    #[serde(other)]
    Unknown,
}

impl VoiceIntent {
    /// Returns a stable, non-PII variant name for logs and metrics.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Play { .. } => "play",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::OpenApp { .. } => "open_app",
            Self::SetVolume { .. } => "set_volume",
            Self::VolumeUp => "volume_up",
            Self::VolumeDown => "volume_down",
            Self::Mute => "mute",
            Self::Unmute => "unmute",
            Self::Unknown => "unknown",
        }
    }
}
