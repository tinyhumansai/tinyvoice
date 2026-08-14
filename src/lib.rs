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
//! So this crate is synchronous, I/O-free and runtime-free. It does not open a
//! microphone, call an STT or TTS endpoint, own a hotkey, or know what a
//! `Config` is. Those are the host's:
//!
//! | Stays with the host | Why |
//! | --- | --- |
//! | Device capture (`cpal`) | A stream is `!Send`, needs the host's thread and its permission model |
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
pub mod intent;
pub mod transcript;
pub mod vad;

pub use error::{Error, Result};
