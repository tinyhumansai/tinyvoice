//! Loadable `TinyBus` module adapter for `TinyVoice`.
//!
//! This private workspace crate keeps the vendored `TinyBus` dependency out of
//! the independently publishable `tinyvoice` crate. Its `cdylib` output is the
//! target-specific binary distributed in GitHub releases.
//!
//! # Measured call cost
//!
//! Measured in-process with `examples/bench_call.rs`, on the real loaded
//! module: **13.3 µs per round trip**, against a 20 ms audio frame — 0.066% of
//! the budget. Re-run it before trusting that number on other hardware:
//!
//! ```sh
//! cargo run --manifest-path crates/tinyvoice-module/Cargo.toml --release --example bench_call -- \
//!   crates/tinyvoice-module/target/release/libtinyvoice_module.so
//! ```
//!
//! This benchmark measures one in-process method round trip on one machine. It
//! is an example measurement, not a reason to call the module for every audio
//! sample or frame.
//!
//! # Capture and hotkey ownership
//!
//! The module owns bounded capture and recording leases, device callbacks and
//! their worker handoff, plus native hotkey listener lifetimes. Hosts reserve,
//! start, poll or read, and stop these leases through the bus. Audio callbacks
//! keep to the realtime-safe capture handoff; host code does not forward raw
//! samples through a per-sample bus loop. Hotkey reads return bounded batches
//! with sequence and continuity facts so a host can recover from a gap.
//!
//! The host owns product policy and composition. In particular, macOS hotkey
//! feeds can be composed from approved Computer accessibility facts by the
//! host; TinyVoice does not link Computer or own accessibility algorithms.
//!
//! # Bounded session state
//!
//! `VoiceService` retains capture, recording, VAD and hotkey owners between bus
//! calls. Their leases, payloads, queues and replay windows are bounded, and
//! identifiers are not reused within a module instance. Shutdown stops new
//! work and joins native owners before the module is unloaded.

mod service;

#[cfg(feature = "static-link")]
pub use service::linked_module;

pub use service::{MAX_AUDIO_BYTES, MAX_SESSIONS, VoiceService};
pub use tinyvoice_bus::{BUS_NAME, OBJECT_PATH};
