//! Ported from `OpenHuman`'s `voice::always_on` VAD tests.
//!
//! Every case drives the segmenter with bare energy numbers — no audio, no
//! device, no clock. That is the point of keeping the state machine pure.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use super::{VadConfig, VadEvent, VadSegmenter};

/// Tighter than the shipping default so a test utterance is a few frames
/// rather than a few hundred.
fn cfg() -> VadConfig {
    VadConfig {
        onset_threshold: 0.1,
        hangover_ms: 100,
        min_speech_ms: 60,
        max_utterance_ms: 1000,
    }
}

/// Push `n` frames at a fixed energy and collect whatever comes back.
fn drive(seg: &mut VadSegmenter, rms: f32, frame_ms: u32, n: u32) -> Vec<VadEvent> {
    (0..n)
        .filter_map(|_| seg.push_frame(rms, frame_ms))
        .collect()
}

#[test]
fn silence_emits_nothing() {
    let mut seg = VadSegmenter::new(cfg());
    assert!(drive(&mut seg, 0.0, 20, 50).is_empty());
    assert!(!seg.is_speaking());
}

#[test]
fn onset_then_hangover_emits_one_utterance() {
    let mut seg = VadSegmenter::new(cfg());

    assert_eq!(seg.push_frame(0.2, 20), Some(VadEvent::SpeechStart));
    assert!(seg.is_speaking());
    // 5 more voiced frames -> 120ms voiced total.
    assert!(drive(&mut seg, 0.2, 20, 5).is_empty());

    // Silence accumulates; the hangover is 100ms, so frames 1-4 stay quiet.
    for _ in 0..4 {
        assert!(seg.push_frame(0.0, 20).is_none());
    }
    match seg.push_frame(0.0, 20) {
        Some(VadEvent::SpeechEnd {
            voiced_ms,
            emit,
            forced,
        }) => {
            assert_eq!(voiced_ms, 120);
            assert!(emit, "120ms voiced clears the 60ms minimum");
            assert!(!forced, "closed by hangover, not by the ceiling");
        }
        other => panic!("expected SpeechEnd, got {other:?}"),
    }
    assert!(!seg.is_speaking());
}

#[test]
fn short_blip_is_reported_but_not_emitted() {
    let mut seg = VadSegmenter::new(cfg());
    assert_eq!(seg.push_frame(0.2, 20), Some(VadEvent::SpeechStart));

    let mut end = None;
    for _ in 0..6 {
        if let Some(e) = seg.push_frame(0.0, 20) {
            end = Some(e);
            break;
        }
    }
    match end {
        Some(VadEvent::SpeechEnd {
            voiced_ms, emit, ..
        }) => {
            assert_eq!(voiced_ms, 20);
            assert!(!emit, "20ms is under the 60ms minimum, so drop the audio");
        }
        other => panic!("expected SpeechEnd, got {other:?}"),
    }
}

#[test]
fn mid_utterance_pause_does_not_split() {
    let mut seg = VadSegmenter::new(cfg());
    seg.push_frame(0.2, 20);

    // 80ms of silence: under the 100ms hangover, so the utterance stays open.
    for _ in 0..4 {
        assert!(seg.push_frame(0.0, 20).is_none());
    }
    assert!(
        seg.is_speaking(),
        "a natural pause must not close the segment"
    );

    // Speech resumes and the silence run resets.
    assert!(drive(&mut seg, 0.2, 20, 3).is_empty());
    assert!(seg.is_speaking());
}

#[test]
fn max_utterance_forces_a_flush() {
    let mut seg = VadSegmenter::new(cfg());
    let mut forced_seen = false;

    // Loud throughout: only the ceiling can close this.
    for _ in 0..100 {
        if let Some(VadEvent::SpeechEnd { forced, emit, .. }) = seg.push_frame(0.5, 20) {
            assert!(forced, "a loud-throughout close must be the ceiling");
            assert!(emit);
            forced_seen = true;
            break;
        }
    }
    assert!(forced_seen, "should force-flush at max_utterance_ms");
    assert!(!seg.is_speaking());
}

#[test]
fn hangover_wins_over_the_ceiling_when_both_apply() {
    // A frame can satisfy both conditions. The speaker has actually stopped,
    // so reporting `forced` would tell the host to keep listening for a
    // continuation that is not coming.
    let mut seg = VadSegmenter::new(VadConfig {
        onset_threshold: 0.1,
        hangover_ms: 40,
        min_speech_ms: 10,
        max_utterance_ms: 60,
    });
    seg.push_frame(0.5, 20); // start, total 20
    seg.push_frame(0.0, 20); // total 40, silence 20
    match seg.push_frame(0.0, 20) {
        // total 60 (>= ceiling) and silence 40 (>= hangover)
        Some(VadEvent::SpeechEnd { forced, .. }) => {
            assert!(!forced, "a genuine stop must not be reported as forced");
        }
        other => panic!("expected SpeechEnd, got {other:?}"),
    }
}

#[test]
fn reset_aborts_without_an_event() {
    let mut seg = VadSegmenter::new(cfg());
    seg.push_frame(0.2, 20);
    assert!(seg.is_speaking());

    seg.reset();
    assert!(
        !seg.is_speaking(),
        "the privacy hook drops the partial utterance"
    );

    // And the segmenter is reusable afterwards.
    assert_eq!(seg.push_frame(0.2, 20), Some(VadEvent::SpeechStart));
}

#[test]
fn config_round_trips_as_json_and_rejects_unknown_keys() {
    let cfg = VadConfig::default();
    let json = serde_json::to_string(&cfg).expect("serialize");
    assert_eq!(
        VadConfig::default(),
        serde_json::from_str(&json).expect("deserialize")
    );

    let extra = r#"{"onset_threshold":0.01,"hangover_ms":800,"min_speech_ms":300,"max_utterance_ms":30000,"surprise":1}"#;
    assert!(
        serde_json::from_str::<VadConfig>(extra).is_err(),
        "an unknown key is a caller/host version mismatch, not something to ignore"
    );
}
