//! Loadable `TinyBus` module adapter for `TinyVoice`.
//!
//! This private workspace crate keeps the vendored `TinyBus` dependency out of
//! the independently publishable `tinyvoice` crate. Its `cdylib` output is the
//! target-specific binary distributed in GitHub releases.
//!
//! # What crosses the bus, and what should not
//!
//! Every method here is **stateless and per-utterance**: a transcript in, a
//! verdict out; a recording in, a container out. That is a deliberate boundary,
//! not an incidental one.
//!
//! The one piece of `tinyvoice` that does *not* fit it is the VAD. A segmenter
//! is driven once per audio frame — every 20 ms — and a bus round trip at that
//! cadence is the wrong shape: the hop would cost more than the work. So
//! `VoiceService::segment` takes a **batch** of frame energies and
//! replays them through a fresh segmenter, which suits offline segmentation and
//! a host that can tolerate batching latency at the end of an utterance.
//!
//! **A host in a hard-realtime capture loop should link the `tinyvoice` rlib
//! directly and hold its own [`tinyvoice::vad::VadSegmenter`].** The crate is
//! host-agnostic and has no bus dependency precisely so that this is possible.
//! Reaching for the module there would trade a function call for an IPC hop and
//! add latency to the one path that is measured in milliseconds.

mod service;

pub use service::{BUS_NAME, MAX_AUDIO_BYTES, OBJECT_PATH, VoiceService};
