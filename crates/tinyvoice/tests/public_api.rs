//! Integration tests against the public API only.
//!
//! These exercise the crate the way a host does — through `tinyvoice::…`, with
//! no access to private helpers — so a re-export that goes missing fails here
//! even when the unit tests still pass.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use tinyvoice::audio::{self, SilenceGateConfig};
use tinyvoice::intent::{VoiceIntent, extract_command, route, wake_word_present};
use tinyvoice::transcript::{Mode, is_hallucinated};
use tinyvoice::vad::{VadConfig, VadEvent, VadSegmenter};
use tinyvoice::{Error, Result};

#[test]
fn the_always_on_pipeline_composes_end_to_end() {
    // The order a host applies these in: gate on the wake word, screen for a
    // hallucination, then try the fast path.
    let heard = "hey tony, pause the music";

    assert!(wake_word_present(heard, "Hey Tiny"));
    let command = extract_command(heard, "Hey Tiny").expect("a command follows the wake word");
    assert_eq!(command, "pause the music");
    assert!(!is_hallucinated(&command, Mode::Conversation));
    assert_eq!(route(&command), VoiceIntent::Pause);
}

#[test]
fn an_unaddressed_utterance_stops_at_the_gate() {
    assert_eq!(extract_command("pause the music", "Hey Tiny"), None);
}

#[test]
fn a_hallucination_is_screened_before_routing() {
    let heard = "hey tiny thank you for watching";
    let command = extract_command(heard, "Hey Tiny").expect("text follows the wake word");
    assert!(
        is_hallucinated(&command, Mode::Conversation),
        "the subtitle phrase must be caught before it reaches the agent"
    );
}

#[test]
fn a_capture_buffer_becomes_a_wav_file() -> Result<()> {
    // Stereo at 32 kHz, as a device might deliver it.
    let stereo: Vec<f32> = (0..2000).map(|i| ((i as f32) / 50.0).sin() * 0.5).collect();

    let mono = audio::to_mono(&stereo, 2)?;
    assert_eq!(mono.len(), 1000);

    let resampled = audio::resample(&mono, 32_000, audio::STT_SAMPLE_RATE)?;
    assert_eq!(resampled.len(), 500);

    let wav = audio::f32_mono_to_wav(&resampled, audio::STT_SAMPLE_RATE)?;
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(wav.len(), 44 + 500 * 2);
    Ok(())
}

#[test]
fn the_silence_gate_is_reachable_and_configurable() -> Result<()> {
    let mut gate = audio::SilenceGate::new(SilenceGateConfig::default(), audio::STT_SAMPLE_RATE)?;
    let loud = vec![0.5f32; 100];
    assert_eq!(gate.push(&loud), loud);
    assert!(!gate.is_gating());
    Ok(())
}

#[test]
fn a_segmenter_carves_an_utterance_out_of_a_stream() {
    let config = VadConfig {
        onset_threshold: 0.1,
        hangover_ms: 100,
        min_speech_ms: 60,
        max_utterance_ms: 5_000,
    };
    let mut seg = VadSegmenter::new(config);

    assert_eq!(seg.push_frame(0.5, 20), Some(VadEvent::SpeechStart));
    for _ in 0..5 {
        assert!(seg.push_frame(0.5, 20).is_none());
    }

    let mut ended = None;
    for _ in 0..10 {
        if let Some(event) = seg.push_frame(0.0, 20) {
            ended = Some(event);
            break;
        }
    }
    match ended {
        Some(VadEvent::SpeechEnd { emit, forced, .. }) => {
            assert!(emit);
            assert!(!forced);
        }
        other => panic!("expected SpeechEnd, got {other:?}"),
    }
}

#[test]
fn errors_are_public_and_matchable() {
    let err = audio::to_mono(&[1.0, 2.0, 3.0], 2).expect_err("ragged");
    assert!(matches!(
        err,
        Error::RaggedFrames {
            samples: 3,
            channels: 2
        }
    ));
}
