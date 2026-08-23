//! Tests that pin the shared `TinyVoice` vocabulary and compatibility rule.

#![allow(clippy::unwrap_used)]

use super::{
    CONTRACT_VERSION, IndexedVadEvent, Mode, VadConfig, VadEvent, VoiceIntent, is_compatible,
};

#[test]
fn the_contract_accepts_its_own_version_and_newer_minors() {
    assert!(is_compatible(CONTRACT_VERSION));
    assert!(is_compatible((CONTRACT_VERSION.0, CONTRACT_VERSION.1 + 1)));
    assert!(!is_compatible((CONTRACT_VERSION.0 + 1, 0)));
}

#[test]
fn voice_values_keep_their_json_contract() {
    let event = IndexedVadEvent {
        frame: 4,
        event: VadEvent::SpeechEnd {
            voiced_ms: 120,
            emit: true,
            forced: false,
        },
    };
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"frame":4,"kind":"speech_end","voiced_ms":120,"emit":true,"forced":false}"#
    );
    assert_eq!(
        VoiceIntent::Play {
            query: "Kind of Blue".into()
        }
        .kind(),
        "play"
    );
    assert_eq!(VoiceIntent::Unknown.kind(), "unknown");
    assert_eq!(
        serde_json::to_string(&Mode::Dictation).unwrap(),
        r#""dictation""#
    );
    assert_eq!(serde_json::from_str::<VadConfig>(r#"{"onset_threshold":0.1,"hangover_ms":100,"min_speech_ms":60,"max_utterance_ms":5000}"#).unwrap().hangover_ms, 100);
}
