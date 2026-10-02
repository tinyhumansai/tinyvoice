//! The `cpal` half of a one-shot recording: open the device, build the stream,
//! poll for stop.
//!
//! Everything here touches a real audio device, so it is the one part of the
//! capture module that cannot run in CI; the handle logic, the sample cap, the
//! sample-format conversion and the config choice around it are all tested.
//! It is excluded from the per-file coverage gate for that reason.

// The device flow is one linear sequence (permission, host, device, config,
// stream, play) whose error arms each carry a distinct, user-visible message.
// Splitting it would scatter that sequence without making any arm testable.
#![allow(
    clippy::too_many_lines,
    clippy::manual_let_else,
    clippy::single_match_else,
    clippy::needless_pass_by_value
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use log::{debug, error, info, warn};

use super::config::find_best_config;
use super::convert::{i16_to_f32, u16_to_f32};
use super::recording::{RawRecording, append_capped};
use super::{PermissionCheck, TARGET_SAMPLE_RATE};

const LOG_PREFIX: &str = "[voice_capture]";

fn stream_error_callback(sender: Sender<String>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |err| {
        let message = format!("audio stream error: {err}");
        warn!("{LOG_PREFIX} {message}");
        let _ = sender.send(message);
    }
}

/// Runs the entire recording lifecycle on a single thread (cpal requirement).
///
/// Every setup failure is also sent on `setup_tx`, so the caller of
/// `start_recording` sees the real reason instead of the generic "thread exited
/// before signalling readiness" fallback that fires when `setup_tx` is dropped.
pub(crate) fn record_on_thread(
    permission: PermissionCheck,
    stop_flag: Arc<AtomicBool>,
    setup_tx: std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<RawRecording, String> {
    // Cross-platform microphone permission pre-check, decided by the host.
    if let Err(msg) = permission() {
        let _ = setup_tx.send(Err(msg.clone()));
        return Err(msg);
    }

    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            // Forward via setup_tx so `start_recording`'s caller sees the
            // real reason instead of the generic "capture thread exited
            // before signalling readiness" fallback that fires when
            // setup_tx is dropped (OPENHUMAN-TAURI-AE). Without this, the
            // user — and Sentry — gets no signal about *which* audio
            // failure occurred.
            let msg = "no default audio input device found".to_string();
            warn!("{LOG_PREFIX} {msg}");
            let _ = setup_tx.send(Err(msg.clone()));
            return Err(msg);
        }
    };

    let device_name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    info!("{LOG_PREFIX} using input device: {device_name}");

    let config = match device.supported_input_configs() {
        Ok(supported) => match find_best_config(supported) {
            Ok(cfg) => cfg,
            Err(e) => {
                warn!("{LOG_PREFIX} find_best_config failed ({e}), falling back to default");
                match device.default_input_config() {
                    Ok(cfg) => cfg,
                    Err(e2) => {
                        // Replaces a `.expect()` that would have panicked
                        // and dropped setup_tx — see OPENHUMAN-TAURI-AE.
                        let msg = format!(
                            "no default input config available (best-config failed: {e}; default lookup: {e2})"
                        );
                        error!("{LOG_PREFIX} {msg}");
                        let _ = setup_tx.send(Err(msg.clone()));
                        return Err(msg);
                    }
                }
            }
        },
        Err(e) => {
            warn!("{LOG_PREFIX} failed to query input configs ({e}), using default");
            match device.default_input_config() {
                Ok(cfg) => cfg,
                Err(e2) => {
                    // Forward via setup_tx so callers see the real cpal
                    // error rather than the generic dropped-tx fallback
                    // (OPENHUMAN-TAURI-AE).
                    let msg = format!(
                        "no default input config: {e2} (supported-configs query failed: {e})"
                    );
                    error!("{LOG_PREFIX} {msg}");
                    let _ = setup_tx.send(Err(msg.clone()));
                    return Err(msg);
                }
            }
        }
    };
    let source_sample_rate = config.sample_rate();
    let source_channels = config.channels() as usize;

    debug!(
        "{LOG_PREFIX} recording config: rate={source_sample_rate} channels={source_channels} format={:?}",
        config.sample_format()
    );

    let samples: Arc<parking_lot::Mutex<Vec<f32>>> = Arc::new(parking_lot::Mutex::new(
        Vec::with_capacity(TARGET_SAMPLE_RATE as usize * 30),
    ));

    let sample_format = config.sample_format();
    let stream_config: StreamConfig = config.into();
    let (stream_error_tx, stream_error_rx) = std::sync::mpsc::channel();

    let stream = {
        let samples_writer = samples.clone();
        match sample_format {
            SampleFormat::F32 => device
                .build_input_stream(
                    stream_config.clone(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        append_capped(&samples_writer, data);
                    },
                    stream_error_callback(stream_error_tx.clone()),
                    None,
                )
                .map_err(|e| format!("failed to build f32 input stream: {e}")),
            SampleFormat::I16 => device
                .build_input_stream(
                    stream_config.clone(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        append_capped(&samples_writer, &i16_to_f32(data));
                    },
                    stream_error_callback(stream_error_tx.clone()),
                    None,
                )
                .map_err(|e| format!("failed to build i16 input stream: {e}")),
            SampleFormat::U16 => device
                .build_input_stream(
                    stream_config.clone(),
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        append_capped(&samples_writer, &u16_to_f32(data));
                    },
                    stream_error_callback(stream_error_tx.clone()),
                    None,
                )
                .map_err(|e| format!("failed to build u16 input stream: {e}")),
            other => Err(format!("unsupported sample format: {other:?}")),
        }
    };

    // If the preferred config failed, retry with the device's default config.
    //
    // The channel count is bound here, not discarded. It used to be (`_source_channels`)
    // because each callback closed over its own `ch` and downmixed in place. Now
    // that downmixing happens at finalize against these values, a fallback stream
    // with a different channel count would otherwise be de-interleaved as if it
    // had the preferred config's — which turns stereo into garbage rather than mono.
    let (stream, source_sample_rate, source_channels) = match stream {
        Ok(s) => (s, source_sample_rate, source_channels),
        Err(ref preferred_err) => {
            warn!(
                "{LOG_PREFIX} preferred config failed ({preferred_err}), retrying with default config"
            );
            match device.default_input_config() {
                Ok(default_cfg) => {
                    let sr = default_cfg.sample_rate();
                    let ch = default_cfg.channels() as usize;
                    let fmt = default_cfg.sample_format();
                    info!("{LOG_PREFIX} fallback config: rate={sr} channels={ch} format={fmt:?}");
                    let sc: StreamConfig = default_cfg.into();
                    let sw = samples.clone();
                    let fallback_stream = match fmt {
                        SampleFormat::F32 => device
                            .build_input_stream(
                                sc.clone(),
                                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                                    append_capped(&sw, data);
                                },
                                stream_error_callback(stream_error_tx.clone()),
                                None,
                            )
                            .map_err(|e| format!("fallback f32 stream failed: {e}")),
                        SampleFormat::I16 => device
                            .build_input_stream(
                                sc.clone(),
                                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                                    append_capped(&sw, &i16_to_f32(data));
                                },
                                stream_error_callback(stream_error_tx.clone()),
                                None,
                            )
                            .map_err(|e| format!("fallback i16 stream failed: {e}")),
                        SampleFormat::U16 => device
                            .build_input_stream(
                                sc.clone(),
                                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                                    append_capped(&sw, &u16_to_f32(data));
                                },
                                stream_error_callback(stream_error_tx.clone()),
                                None,
                            )
                            .map_err(|e| format!("fallback u16 stream failed: {e}")),
                        other => Err(format!("unsupported fallback format: {other:?}")),
                    };
                    match fallback_stream {
                        Ok(s) => (s, sr, ch),
                        Err(e2) => {
                            let msg = format!(
                                "both preferred ({preferred_err}) and fallback ({e2}) configs failed"
                            );
                            let _ = setup_tx.send(Err(msg.clone()));
                            return Err(msg);
                        }
                    }
                }
                Err(e2) => {
                    let msg = format!(
                        "preferred config failed ({preferred_err}) and no default available ({e2})"
                    );
                    let _ = setup_tx.send(Err(msg.clone()));
                    return Err(msg);
                }
            }
        }
    };

    if let Err(e) = stream.play() {
        let msg = format!("failed to start audio stream: {e}");
        let _ = setup_tx.send(Err(msg.clone()));
        return Err(msg);
    }

    // Signal success so start_recording() returns.
    let _ = setup_tx.send(Ok(()));

    // Poll stop flag while keeping the stream alive on this thread.
    while !stop_flag.load(Ordering::SeqCst) {
        if let Some(error) = receive_stream_error(&stream_error_rx) {
            drop(stream);
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    debug!("{LOG_PREFIX} stop flag detected, finalizing recording");
    drop(stream);

    if let Some(err) = receive_stream_error(&stream_error_rx) {
        return Err(err);
    }

    let samples = std::mem::take(&mut *samples.lock());
    if samples.is_empty() {
        warn!("{LOG_PREFIX} no audio samples captured");
        return Err("no audio samples captured".to_string());
    }
    debug!(
        "{LOG_PREFIX} captured {} raw interleaved samples at {source_sample_rate}Hz x{source_channels}",
        samples.len()
    );
    Ok(RawRecording {
        samples,
        source_rate: source_sample_rate,
        channels: source_channels,
    })
}

fn receive_stream_error(receiver: &Receiver<String>) -> Option<String> {
    receiver.try_recv().ok()
}

/// List available input devices.
///
/// # Errors
///
/// A message when the platform cannot enumerate input devices.
pub fn list_input_devices() -> crate::Result<Vec<String>> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|e| crate::Error::Capture(format!("failed to enumerate input devices: {e}")))?;

    let names: Vec<String> = devices
        .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
        .collect();

    debug!("{LOG_PREFIX} found {} input devices", names.len());
    Ok(names)
}
