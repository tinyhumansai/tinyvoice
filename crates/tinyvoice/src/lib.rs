//! Host-agnostic voice primitives.
//!
//! This crate owns the parts of a voice pipeline that are the same for every
//! host: turning captured samples into a container an STT endpoint accepts,
//! deciding where one utterance ends and the next begins, classifying a
//! transcript into a fast-path command, and recognising the stock phrases a
//! Whisper-family model emits when it is fed silence.
//!
//! # What is deliberately *not* here
//!
//! The split follows one rule, and it is the same rule `tinydocs` and
//! `tinywallet` follow: **a crate owns what is identical for every host; the
//! host owns what depends on its own runtime, config, or threat model.**
//!
//! So by default this crate is synchronous, I/O-free and runtime-free. It does
//! not call an STT or TTS endpoint, or know what a `Config` is; the off-by-default
//! `capture` and `hotkey` features are the only parts that touch the machine (a
//! microphone, a keyboard hook). Those are the host's:
//!
//! | Stays with the host | Why |
//! | --- | --- |
//! | Microphone permission | Differs per OS and per host; the `capture` feature takes a check as a parameter |
//! | STT / TTS transport | Endpoint choice, credentials and retry are host policy |
//! | Hotkeys, text injection | Platform input APIs, and a host's own accessibility posture |
//! | Config and RPC shapes | The host's wire contract, not this crate's |
//!
//! The consequence worth knowing: [`vad::VadConfig`] has no constructor that
//! reads a config file. A host builds one from whatever it persists. A crate
//! that guessed at that shape would be wrong for every host that guessed
//! differently.
//!
//! # Layout
//!
//! - [`audio`] — WAV framing, RMS energy, resampling, downmixing, and the
//!   silence gate that keeps dead air out of an STT upload.
//! - [`vad`] — the voice-activity state machine that carves a continuous
//!   stream into utterances.
//! - [`intent`] — transcript to [`intent::VoiceIntent`], the fast-path
//!   classifier that lets a host skip an LLM turn.
//! - [`transcript`] — STT hallucination detection.
//! - `capture` (cargo feature `capture`, off by default) — microphone input on
//!   `cpal`: a one-shot recording and a continuous chunk stream, both returning
//!   raw device samples for the functions in [`audio`] to process.
//! - `hotkey` (cargo feature `hotkey`, off by default) — a system-wide
//!   push-to-talk / toggle key listener on `rdev`, for hosts that gate
//!   recording on a key.
//!
//! # Example
//!
//! ```
//! use tinyvoice::{intent::{route, VoiceIntent}, transcript::{is_hallucinated, Mode}};
//!
//! assert_eq!(route("please pause the music"), VoiceIntent::Pause);
//! assert!(is_hallucinated("Thank you for watching", Mode::Conversation));
//! ```

mod error;

pub mod audio;
#[cfg(feature = "capture")]
pub mod capture;
#[cfg(feature = "hotkey")]
pub mod hotkey;
pub mod intent;
pub mod transcript;
pub mod vad;

pub use error::{Error, Result};
// The transport-free vocabulary is defined once in `tinyvoice-bus`. Re-export
// it so the historical `tinyvoice::{vad, intent, transcript}` paths keep
// resolving to the exact types a TinyBus host uses.
pub use tinyvoice_bus;
