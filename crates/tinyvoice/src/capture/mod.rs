//! Microphone capture on `cpal` (the `capture` feature).
//!
//! Two shapes of capture, both returning the device's own samples untouched:
//!
//! * [`start_recording`]: a one-shot recording. Records until
//!   [`RecordingHandle::stop`] is awaited, then yields a [`RawRecording`].
//! * [`spawn_capture_thread`]: a continuous stream. Forwards each device
//!   callback's buffer as a [`RawChunk`] into a bounded channel, dropping the
//!   newest chunk when the consumer falls behind.
//!
//! Neither does any signal processing. The callback converts the sample format
//! and accumulates or forwards; downmixing, resampling, silence gating and WAV
//! framing are the [`crate::audio`] functions' job, run off the audio thread by
//! whoever holds the samples.
//!
//! # Permission
//!
//! Whether the process may open the microphone differs per operating system
//! (a macOS TCC prompt, Windows privacy settings) and per host. This module
//! does not decide it: each entry point takes a [`PermissionCheck`] and runs it
//! on the capture thread before the device is touched.
//!
//! # Threads
//!
//! `cpal::Stream` is `!Send`, so it is created, played and dropped on one
//! dedicated OS thread. The entry points block only until that thread reports
//! that setup succeeded or failed, and surface the real failure reason rather
//! than a generic "thread exited".

mod chunks;
mod config;
mod convert;
mod device_recording;
mod device_stream;
mod recording;

pub use chunks::{
    CaptureFormat, CaptureStreamHandle, MAX_CHUNK_SAMPLES, RawChunk, spawn_capture_thread,
    start_capture_stream,
};
pub use device_recording::list_input_devices;
pub use recording::{RawRecording, RecordingHandle, start_recording};

/// Sample rate the STT path wants (16 kHz).
pub const TARGET_SAMPLE_RATE: u32 = crate::audio::STT_SAMPLE_RATE;

/// A host-supplied microphone permission check.
///
/// Run on the capture thread before the device is opened. `Ok(())` means
/// capture may proceed; `Err(message)` is surfaced to the caller verbatim as
/// the reason capture could not start.
pub type PermissionCheck = fn() -> Result<(), String>;
