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
