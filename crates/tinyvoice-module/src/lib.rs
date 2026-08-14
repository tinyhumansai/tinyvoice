//! Loadable `TinyBus` module adapter for `TinyVoice`.
//!
//! This private workspace crate keeps the vendored `TinyBus` dependency out of
//! the independently publishable `tinyvoice` crate. Its `cdylib` output is the
//! target-specific binary distributed in GitHub releases.
//!
//! # A call costs about 13 microseconds
//!
//! Measured in-process with `examples/bench_call.rs`, on the real loaded
//! module: **13.3 µs per round trip**, against a 20 ms audio frame — 0.066% of
//! the budget. Re-run it before trusting that number on other hardware:
//!
//! ```sh
//! cargo run --release -p tinyvoice-module --example bench_call -- \
//!   target/release/libtinyvoice_module.so
//! ```
//!
//! This is worth stating plainly because an earlier revision of these docs
//! asserted the opposite — that a per-frame call was too expensive and a
//! realtime host should link the `tinyvoice` rlib instead. That claim was never
//! measured, and it is wrong. A `TinyBus` module shares the host's address
//! space; a call is a channel send and a JSON hop, not IPC.
//!
//! So a live capture loop **can** drive the VAD through this interface, and the
//! session methods exist for exactly that.
//!
//! # What still belongs on the host's side
//!
//! One thing, and it is about thread discipline rather than cost: whatever runs
//! inside the audio callback. `cpal` delivers on a realtime thread where the
//! correct amount of work is as little as possible and blocking is a dropout.
//! A host should forward raw interleaved samples from the callback to its own
//! worker and call `PrepareFrames` from there — which is less
//! work in the callback than downmixing in place, not more.
//!
//! # Session state
//!
//! The VAD is the only thing this module remembers between calls, because it is
//! the only thing that is a state machine over successive frames. See
//! [`VoiceService`] for why it is bounded and why ids are never reused.

mod service;

pub use service::{BUS_NAME, MAX_AUDIO_BYTES, MAX_SESSIONS, OBJECT_PATH, VoiceService};
