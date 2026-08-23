//! Sample-level audio work: framing, energy, rate conversion, silence gating.
//!
//! Everything here operates on plain slices. Nothing opens a device, spawns a
//! thread, or allocates a runtime — a host captures however it likes and hands
//! the samples over.
//!
//! # Why the WAV writer is 40 lines rather than a dependency
//!
//! Wrapping a PCM buffer in a canonical 44-byte RIFF/WAVE header is the whole
//! job. `hound` would do it too, and `OpenHuman` used to call it from exactly one
//! function — but it was a gated dependency being reached from an ungated part
//! of that tree, and pulling a crate in to push 44 bytes is a poor trade when
//! the format has been frozen since 1991.

// Sample arithmetic moves between three domains on purpose: buffer lengths and
// indices (`usize`), resampling positions (`f64`, for the precision a long
// buffer needs), and the samples themselves (`f32`). Every conversion between
// them is lossy in principle and exact in practice at audio sizes — a buffer
// would have to exceed 2^24 samples (about 17 minutes at 16 kHz) before an
// index stopped being representable in `f32`, and the `f64` positions are wider
// still. The one conversion where precision genuinely matters is the sample
// itself, and `f32_mono_to_wav` clamps before casting rather than relying on it.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

#[cfg(test)]
mod test;

use crate::{Error, Result};

/// The sample rate every hosted STT endpoint in use expects: 16 kHz.
///
/// Exposed because a host doing its own capture needs to request this rate
/// from the device, and a second copy of the constant is a second thing to get
/// wrong.
pub const STT_SAMPLE_RATE: u32 = 16_000;

/// Root-mean-square energy of a block of mono samples.
///
/// Returns `0.0` for an empty slice rather than `NaN`: callers compare the
/// result against a threshold, and a `NaN` would silently answer `false` to
/// every comparison, which reads as "not silent" — the opposite of the truth.
#[must_use]
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Average interleaved multi-channel samples down to mono.
///
/// # Errors
///
/// [`Error::ZeroChannels`] if `channels` is zero, and [`Error::RaggedFrames`]
/// if `samples` is not a whole number of frames. A ragged tail means the caller
/// and the device disagree about the channel count, which is worth reporting
/// rather than silently truncating — the samples that follow would all be
/// assigned to the wrong channel.
pub fn to_mono(samples: &[f32], channels: u16) -> Result<Vec<f32>> {
    if channels == 0 {
        return Err(Error::ZeroChannels);
    }
    let channels_usize = channels as usize;
    if channels == 1 {
        return Ok(samples.to_vec());
    }
    if !samples.len().is_multiple_of(channels_usize) {
        return Err(Error::RaggedFrames {
            samples: samples.len(),
            channels,
        });
    }
    Ok(samples
        .chunks_exact(channels_usize)
        .map(|frame| frame.iter().sum::<f32>() / f32::from(channels))
        .collect())
}

/// Resample mono samples from `source_rate` to `target_rate` by linear
/// interpolation.
///
/// Linear interpolation is not a good anti-aliasing resampler and is not
/// pretending to be one. It is what a speech pipeline feeding a hosted STT
/// model needs: the models are robust to the artefacts, and the alternative is
/// a windowed-sinc implementation plus its dependency for no measurable
/// transcription gain.
///
/// # Errors
///
/// [`Error::ZeroSampleRate`] if either rate is zero.
pub fn resample(samples: &[f32], source_rate: u32, target_rate: u32) -> Result<Vec<f32>> {
    if source_rate == 0 || target_rate == 0 {
        return Err(Error::ZeroSampleRate);
    }
    if source_rate == target_rate || samples.is_empty() {
        return Ok(samples.to_vec());
    }

    let ratio = f64::from(source_rate) / f64::from(target_rate);
    let output_len = ((samples.len() as f64) / ratio).ceil() as usize;
    let last = samples.len().saturating_sub(1);
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_idx = i as f64 * ratio;
        let idx0 = (src_idx.floor() as usize).min(last);
        let idx1 = (idx0 + 1).min(last);
        let frac = (src_idx - src_idx.floor()) as f32;
        output.push(samples[idx0].mul_add(1.0 - frac, samples[idx1] * frac));
    }

    Ok(output)
}

/// Wrap 16-bit little-endian PCM samples in a canonical 44-byte WAV header.
///
/// # Errors
///
/// [`Error::ZeroSampleRate`] or [`Error::ZeroChannels`] — a header declaring
/// either as zero describes a file no decoder can play, and the byte-rate field
/// would be zero too.
pub fn pcm16_to_wav(samples: &[i16], sample_rate: u32, channels: u16) -> Result<Vec<u8>> {
    if sample_rate == 0 {
        return Err(Error::ZeroSampleRate);
    }
    if channels == 0 {
        return Err(Error::ZeroChannels);
    }

    let bits_per_sample: u16 = 16;
    let block_align = channels * bits_per_sample / 8;
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = (samples.len() * 2) as u32;

    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    // RIFF chunk size = 36 + data (everything after this field).
    out.extend_from_slice(&36u32.saturating_add(data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(out)
}

/// Encode mono `f32` samples in `-1.0..=1.0` as a 16-bit PCM WAV file.
///
/// Samples outside the range are clamped rather than allowed to wrap: a value
/// of `1.5` scaled and cast would land at a large negative `i16`, turning a
/// loud passage into an audible crack.
///
/// # Errors
///
/// [`Error::ZeroSampleRate`] if `sample_rate` is zero.
pub fn f32_mono_to_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let pcm: Vec<i16> = samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect();
    pcm16_to_wav(&pcm, sample_rate, 1)
}

/// How long a run of silence must last before [`SilenceGate`] starts dropping
/// it, and how much audio it keeps to avoid clipping the next word.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SilenceGateConfig {
    /// RMS below which a block counts as silence.
    pub threshold: f32,
    /// Continuous silence required before the gate closes.
    pub gate_ms: u32,
    /// How much audio to hold back while gated, so speech onset is not clipped.
    pub lookahead_ms: u32,
}

impl Default for SilenceGateConfig {
    /// The tuning `OpenHuman`'s push-to-talk recorder shipped with.
    fn default() -> Self {
        Self {
            threshold: 0.002,
            gate_ms: 500,
            lookahead_ms: 100,
        }
    }
}

/// Drops long runs of silence from a stream while preserving speech onset.
///
/// An STT upload is billed and rate-limited by duration, so dead air is worth
/// removing. Removing it naively clips the start of the next word, because by
/// the time energy crosses the threshold the first phoneme is already past —
/// hence the look-ahead: while gated, the most recent `lookahead_ms` is
/// retained and flushed ahead of the block that reopened the gate.
#[derive(Debug)]
pub struct SilenceGate {
    config: SilenceGateConfig,
    gate_samples: usize,
    lookahead_samples: usize,
    silent_samples: usize,
    gating: bool,
    lookahead: Vec<f32>,
}

impl SilenceGate {
    /// Build a gate for a stream at `sample_rate`.
    ///
    /// # Errors
    ///
    /// [`Error::ZeroSampleRate`] if `sample_rate` is zero.
    pub fn new(config: SilenceGateConfig, sample_rate: u32) -> Result<Self> {
        if sample_rate == 0 {
            return Err(Error::ZeroSampleRate);
        }
        let per_ms = sample_rate as usize;
        let gate_samples = ((per_ms * config.gate_ms as usize) / 1000).max(1);
        let lookahead_samples = ((per_ms * config.lookahead_ms as usize) / 1000).max(1);
        Ok(Self {
            config,
            gate_samples,
            lookahead_samples,
            silent_samples: 0,
            gating: false,
            lookahead: Vec::with_capacity(lookahead_samples),
        })
    }

    /// True while the gate is suppressing audio.
    #[must_use]
    pub fn is_gating(&self) -> bool {
        self.gating
    }

    /// Feed a block of mono samples; returns what should be kept.
    ///
    /// The returned buffer may be empty (the block was gated silence), the same
    /// length as the input (ordinary audio), or *longer* than the input — that
    /// is the look-ahead flushing on the transition back to speech, and a
    /// caller that assumed a length-preserving filter would drop it.
    pub fn push(&mut self, mono: &[f32]) -> Vec<f32> {
        if rms(mono) >= self.config.threshold {
            // Speech: reset, and flush any retained look-ahead ahead of it.
            self.silent_samples = 0;
            if self.gating {
                self.gating = false;
                let mut flushed = core::mem::take(&mut self.lookahead);
                flushed.extend_from_slice(mono);
                return flushed;
            }
            return mono.to_vec();
        }

        self.silent_samples += mono.len();
        if self.silent_samples < self.gate_samples {
            // Not yet a long enough run to be worth dropping.
            return mono.to_vec();
        }

        self.gating = true;
        self.lookahead.extend_from_slice(mono);
        if self.lookahead.len() > self.lookahead_samples {
            let excess = self.lookahead.len() - self.lookahead_samples;
            self.lookahead.drain(..excess);
        }
        Vec::new()
    }
}
