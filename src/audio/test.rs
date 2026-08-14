//! Ported from `OpenHuman`'s `inference::voice::wav` and `voice::audio_capture`
//! tests, plus the boundary cases the originals left to their callers.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use super::{
    STT_SAMPLE_RATE, SilenceGate, SilenceGateConfig, f32_mono_to_wav, pcm16_to_wav, resample, rms,
    to_mono,
};
use crate::Error;

// --- WAV framing ---

#[test]
fn writes_a_canonical_44_byte_header() {
    let wav = pcm16_to_wav(&[1, -1, 32767], 16_000, 1).expect("valid");
    assert_eq!(wav.len(), 44 + 6, "header + 3 samples x 2 bytes");
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(&wav[12..16], b"fmt ");
    assert_eq!(&wav[36..40], b"data");
    // RIFF size = 36 + data bytes.
    assert_eq!(
        u32::from_le_bytes(wav[4..8].try_into().expect("4 bytes")),
        42
    );
    assert_eq!(
        u32::from_le_bytes(wav[40..44].try_into().expect("4 bytes")),
        6
    );
    // Sample rate and derived byte rate.
    assert_eq!(
        u32::from_le_bytes(wav[24..28].try_into().expect("4 bytes")),
        16_000
    );
    assert_eq!(
        u32::from_le_bytes(wav[28..32].try_into().expect("4 bytes")),
        32_000
    );
    // Samples are little-endian.
    assert_eq!(&wav[44..46], &1i16.to_le_bytes());
}

#[test]
fn empty_input_still_produces_a_valid_header() {
    let wav = pcm16_to_wav(&[], 16_000, 1).expect("valid");
    assert_eq!(wav.len(), 44);
    assert_eq!(
        u32::from_le_bytes(wav[40..44].try_into().expect("4 bytes")),
        0
    );
}

#[test]
fn a_header_that_no_decoder_could_play_is_refused() {
    assert_eq!(pcm16_to_wav(&[1], 0, 1), Err(Error::ZeroSampleRate));
    assert_eq!(pcm16_to_wav(&[1], 16_000, 0), Err(Error::ZeroChannels));
}

#[test]
fn f32_encoding_clamps_rather_than_wrapping() {
    // 1.5 scaled and cast would wrap to a large negative i16 — an audible
    // crack exactly where the audio was loudest.
    let wav = f32_mono_to_wav(&[1.5, -1.5], STT_SAMPLE_RATE).expect("valid");
    let a = i16::from_le_bytes(wav[44..46].try_into().expect("2 bytes"));
    let b = i16::from_le_bytes(wav[46..48].try_into().expect("2 bytes"));
    assert_eq!(a, 32767);
    assert_eq!(b, -32767);
}

// --- Energy ---

#[test]
fn rms_of_empty_is_zero_not_nan() {
    // A NaN would compare false against every threshold, which reads as
    // "not silent" — the opposite of the truth.
    assert_eq!(rms(&[]), 0.0);
    assert!(!rms(&[]).is_nan());
}

#[test]
fn rms_of_a_constant_signal_is_its_magnitude() {
    assert!((rms(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < f32::EPSILON);
    assert_eq!(rms(&[0.0, 0.0]), 0.0);
}

// --- Downmixing ---

#[test]
fn stereo_averages_to_mono() {
    assert_eq!(
        to_mono(&[1.0, 0.0, 0.5, 0.5], 2).expect("even"),
        vec![0.5, 0.5]
    );
}

#[test]
fn mono_is_returned_unchanged() {
    assert_eq!(
        to_mono(&[0.1, 0.2, 0.3], 1).expect("mono"),
        vec![0.1, 0.2, 0.3]
    );
}

#[test]
fn a_ragged_tail_is_an_error_not_a_silent_truncation() {
    // A ragged tail means the caller and the device disagree about the channel
    // count, so every following sample would land on the wrong channel.
    assert_eq!(
        to_mono(&[1.0, 2.0, 3.0], 2),
        Err(Error::RaggedFrames {
            samples: 3,
            channels: 2
        })
    );
    assert_eq!(to_mono(&[1.0], 0), Err(Error::ZeroChannels));
}

// --- Resampling ---

#[test]
fn matching_rates_are_a_passthrough() {
    let input = [0.1, 0.2, 0.3];
    assert_eq!(
        resample(&input, 16_000, 16_000).expect("same"),
        input.to_vec()
    );
}

#[test]
fn downsampling_halves_the_length() {
    let input: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
    let out = resample(&input, 32_000, 16_000).expect("valid");
    assert_eq!(out.len(), 50);
}

#[test]
fn upsampling_doubles_the_length_and_interpolates() {
    let out = resample(&[0.0, 1.0], 8_000, 16_000).expect("valid");
    assert_eq!(out.len(), 4);
    assert!((out[0] - 0.0).abs() < 1e-6);
    // Midpoint between the two samples.
    assert!((out[1] - 0.5).abs() < 1e-6);
}

#[test]
fn resampling_never_reads_past_the_end() {
    // The final output index maps onto the last input sample; an off-by-one
    // here is an index panic on every real recording.
    for len in 1..64 {
        let input: Vec<f32> = (0..len).map(|i| i as f32).collect();
        assert!(resample(&input, 44_100, 16_000).is_ok());
        assert!(resample(&input, 16_000, 44_100).is_ok());
    }
}

#[test]
fn empty_and_zero_rates_are_handled() {
    assert_eq!(
        resample(&[], 44_100, 16_000).expect("empty"),
        Vec::<f32>::new()
    );
    assert_eq!(resample(&[0.1], 0, 16_000), Err(Error::ZeroSampleRate));
    assert_eq!(resample(&[0.1], 16_000, 0), Err(Error::ZeroSampleRate));
}

// --- Silence gate ---

fn gate() -> SilenceGate {
    // 1000 samples/sec keeps the arithmetic legible: gate_ms and lookahead_ms
    // map to sample counts one-for-one.
    SilenceGate::new(
        SilenceGateConfig {
            threshold: 0.01,
            gate_ms: 100,
            lookahead_ms: 20,
        },
        1000,
    )
    .expect("valid rate")
}

#[test]
fn speech_passes_through_untouched() {
    let mut g = gate();
    let loud = vec![0.5f32; 50];
    assert_eq!(g.push(&loud), loud);
    assert!(!g.is_gating());
}

#[test]
fn short_silence_is_kept() {
    let mut g = gate();
    let quiet = vec![0.0f32; 50]; // 50 < 100 sample gate
    assert_eq!(g.push(&quiet).len(), 50);
    assert!(!g.is_gating());
}

#[test]
fn sustained_silence_is_dropped() {
    let mut g = gate();
    g.push(&[0.0f32; 50]);
    // Crosses the 100-sample threshold.
    assert!(g.push(&[0.0f32; 60]).is_empty());
    assert!(g.is_gating());
    assert!(g.push(&[0.0f32; 60]).is_empty());
}

#[test]
fn lookahead_is_flushed_so_speech_onset_is_not_clipped() {
    let mut g = gate();
    g.push(&[0.0f32; 200]); // gate closes, look-ahead fills
    assert!(g.is_gating());

    let loud = vec![0.5f32; 10];
    let out = g.push(&loud);
    assert!(!g.is_gating());
    assert!(
        out.len() > loud.len(),
        "the gate returns more than it was given on the reopening block; \
         a caller assuming a length-preserving filter would drop the flush"
    );
    assert_eq!(out.len(), 20 + 10, "retained look-ahead + the new block");
    assert_eq!(&out[20..], &loud[..]);
}

#[test]
fn a_zero_sample_rate_is_refused_at_construction() {
    assert_eq!(
        SilenceGate::new(SilenceGateConfig::default(), 0).err(),
        Some(Error::ZeroSampleRate)
    );
}
