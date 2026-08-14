//! Wake-word gating and fast-path command classification.
//!
//! Two independent steps a host applies to a transcript, in this order:
//!
//! 1. [`extract_command`] — was this utterance addressed to the agent at all?
//!    In an always-on microphone every passing conversation arrives here, so
//!    the wake word is what separates a command from the room.
//! 2. [`route`] — is it something that can be executed directly, without
//!    spending an LLM turn on it?
//!
//! Routing can only ever *shortcut*, never *block*: anything not confidently
//! recognised comes back as [`VoiceIntent::Unknown`] and the host hands it to
//! the agent. That asymmetry is the whole safety argument for the fast path —
//! a classifier that guessed would silently mishandle real requests, whereas
//! one that declines merely costs a round trip.

#[cfg(test)]
mod test;

mod wake;

pub use wake::{extract_command, wake_word_present};

use serde::{Deserialize, Serialize};

/// A recognised fast-path voice command, or [`Unknown`](VoiceIntent::Unknown)
/// when the transcript should go to the agent instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case")]
pub enum VoiceIntent {
    /// "play &lt;song/artist&gt;" — a media search and play.
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
    /// Go back to the previous track.
    Previous,
    /// "open/launch/start &lt;app&gt;".
    OpenApp {
        /// The cleaned application name.
        app: String,
    },
    /// "set volume to N" — absolute, `0..=100`.
    SetVolume {
        /// Target volume percentage.
        percent: u8,
    },
    /// Raise the volume by the host's step.
    VolumeUp,
    /// Lower the volume by the host's step.
    VolumeDown,
    /// Mute audio output.
    Mute,
    /// Unmute audio output.
    Unmute,
    /// Not a confident fast command — defer to the agent.
    Unknown,
}

impl VoiceIntent {
    /// A stable, **non-PII** variant name, for logs and metrics.
    ///
    /// Never includes the transcript-derived `query` / `app` payloads. This
    /// path is fed by an always-on microphone, so those fields can contain
    /// anything that was said in the room; a log line naming the variant is
    /// useful, and one naming the query is a recording.
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

/// Normalise: lowercase, replace punctuation with spaces, collapse whitespace.
fn norm(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() || c == '%' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strip a leading politeness/filler prefix so "please pause" and "can you open
/// slack" route the same as the bare forms.
fn strip_filler(s: &str) -> &str {
    const FILLERS: &[&str] = &[
        "please ",
        "can you please ",
        "can you ",
        "could you please ",
        "could you ",
        "would you ",
        "i want to ",
        "i want you to ",
        "go ahead and ",
    ];
    let mut cur = s;
    // Applied repeatedly so "please can you …" also reduces.
    loop {
        let mut matched = false;
        for f in FILLERS {
            if let Some(rest) = cur.strip_prefix(f) {
                cur = rest;
                matched = true;
                break;
            }
        }
        if !matched {
            return cur;
        }
    }
}

/// Classify a command transcript into a [`VoiceIntent`].
///
/// The input should already have had the wake word removed by
/// [`extract_command`].
#[must_use]
pub fn route(transcript: &str) -> VoiceIntent {
    let normalized = norm(transcript);
    let s = strip_filler(&normalized).trim();
    if s.is_empty() {
        return VoiceIntent::Unknown;
    }

    // Transport controls, as exact phrases.
    match s {
        "pause" | "pause music" | "pause the music" | "pause the song" | "stop" | "stop music"
        | "stop the music" => return VoiceIntent::Pause,
        "resume" | "resume music" | "continue" | "continue playing" | "unpause" => {
            return VoiceIntent::Resume;
        }
        "next" | "next song" | "next track" | "skip" | "skip song" | "skip this" => {
            return VoiceIntent::Next;
        }
        "previous" | "previous song" | "previous track" | "go back a song" | "last song" => {
            return VoiceIntent::Previous;
        }
        "mute" | "mute it" | "mute the volume" | "mute audio" => return VoiceIntent::Mute,
        "unmute" | "unmute it" | "unmute the volume" => return VoiceIntent::Unmute,
        "volume up" | "turn it up" | "turn up the volume" | "louder" | "turn the volume up" => {
            return VoiceIntent::VolumeUp;
        }
        "volume down"
        | "turn it down"
        | "turn down the volume"
        | "quieter"
        | "turn the volume down"
        | "lower the volume" => return VoiceIntent::VolumeDown,
        _ => {}
    }

    if let Some(percent) = parse_set_volume(s) {
        return VoiceIntent::SetVolume { percent };
    }

    if let Some(rest) = s.strip_prefix("play ") {
        let query = clean_media_query(rest);
        if !query.is_empty() && !is_ambiguous_media_query(&query) {
            return VoiceIntent::Play { query };
        }
    }

    for verb in ["open ", "launch ", "start ", "fire up "] {
        if let Some(rest) = s.strip_prefix(verb) {
            let app = clean_app_name(rest);
            if !app.is_empty() {
                return VoiceIntent::OpenApp { app };
            }
        }
    }

    VoiceIntent::Unknown
}

/// Parse "set volume to 40", "volume 40", "set the volume to 40 percent".
fn parse_set_volume(s: &str) -> Option<u8> {
    let candidates = [
        s.strip_prefix("set volume to "),
        s.strip_prefix("set the volume to "),
        s.strip_prefix("change volume to "),
        s.strip_prefix("change the volume to "),
        s.strip_prefix("volume to "),
        s.strip_prefix("volume "),
        s.strip_prefix("set volume "),
    ];
    let rest = candidates.into_iter().flatten().next()?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let value: u32 = digits.parse().ok()?;
    Some(value.min(100) as u8)
}

/// Drop trailing app-locator words ("in apple music", "on spotify") and treat
/// "by" as a separator so "Numb by Linkin Park" becomes one query.
fn clean_media_query(rest: &str) -> String {
    let mut q = rest.trim().to_string();
    for suffix in [
        " in apple music",
        " on apple music",
        " in music",
        " on spotify",
        " on music",
    ] {
        if q.ends_with(suffix) {
            q.truncate(q.len() - suffix.len());
            break;
        }
    }
    for filler in ["the song ", "the track ", "song ", "track ", "me "] {
        if let Some(r) = q.strip_prefix(filler) {
            q = r.to_string();
            break;
        }
    }
    q.replace(" by ", " ").trim().to_string()
}

/// Strip "up "/"the "/"my " noise from an app name ("open up slack" → "slack").
fn clean_app_name(rest: &str) -> String {
    let mut a = rest.trim();
    for filler in ["up ", "the ", "my "] {
        if let Some(r) = a.strip_prefix(filler) {
            a = r.trim();
            break;
        }
    }
    a.to_string()
}

/// Bare pronouns and generic nouns carry no song, so they must defer to the
/// agent rather than take the local route with a meaningless query.
fn is_ambiguous_media_query(q: &str) -> bool {
    matches!(
        q,
        "it" | "this" | "that" | "them" | "something" | "music" | "some music" | "a song" | "songs"
    )
}
