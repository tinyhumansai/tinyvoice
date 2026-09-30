//! The `cpal` half of the continuous stream: open the default input, forward
//! each callback's buffer, and keep the stream alive for the process lifetime.
//!
//! Touches a real audio device, so like [`super::device_recording`] it is
//! excluded from the per-file coverage gate; the forwarding, drop accounting and
//! thread handshake it relies on are tested in [`super::chunks`].

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

use super::chunks::{CaptureFormat, RawChunk, forward};
use super::{PermissionCheck, TARGET_SAMPLE_RATE};

const LOG_PREFIX: &str = "[voice::always_on]";

/// Owns the cpal stream for the process lifetime.
///
/// Each callback converts the device's sample format to `f32` and forwards the
/// interleaved buffer untouched, because this runs on a realtime audio thread
/// where the right amount of work is the least possible.
pub(crate) fn capture_on_thread(
    permission: PermissionCheck,
    tx: tokio::sync::mpsc::Sender<RawChunk>,
    setup_tx: &std::sync::mpsc::SyncSender<Result<CaptureFormat, String>>,
) -> Result<(), String> {
    // The host decides and logs the microphone permission state: a denied one
    // is the most common reason always-on "does nothing", and it differs per OS.
    permission()?;

    let host = cpal::default_host();
    log::info!("{LOG_PREFIX} audio host: {:?}", host.id());
    let device = host
        .default_input_device()
        .ok_or_else(|| "no default audio input device".to_string())?;
    let device_name = device.name().unwrap_or_else(|e| format!("<unknown: {e}>"));
    let supported = device
        .default_input_config()
        .map_err(|e| format!("no default input config: {e}"))?;
    let source_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let sample_format = supported.sample_format();
    let stream_config: StreamConfig = supported.into();
    // Name + source rate/channels/format vary across M-chip, Intel, and Windows
    // mics; capturing them makes a "wrong device" or "unsupported format" failure
    // obvious from the log alone. We resample everything to 16 kHz mono downstream.
    log::info!(
        "{LOG_PREFIX} capture device ready name='{device_name}' rate={source_rate}->{TARGET_SAMPLE_RATE} channels={channels} format={sample_format:?}"
    );

    let forward = move |samples: Vec<f32>| forward(&tx, samples);

    let err_fn = |e| log::warn!("{LOG_PREFIX} cpal stream error: {e}");
    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| forward(data.to_vec()),
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| {
                forward(data.iter().map(|&s| f32::from(s) / 32768.0).collect());
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            &stream_config,
            move |data: &[u16], _| {
                forward(data.iter().map(|&s| f32::from(s) / 32768.0 - 1.0).collect());
            },
            err_fn,
            None,
        ),
        other => return Err(format!("unsupported sample format: {other:?}")),
    }
    .map_err(|e| format!("failed to build input stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("failed to start stream: {e}"))?;
    let _ = setup_tx.send(Ok(CaptureFormat {
        source_rate,
        channels,
    }));
    log::info!("{LOG_PREFIX} microphone stream live");

    // Keep the stream (and thus this thread) alive for the process lifetime.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
