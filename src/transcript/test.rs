//! Ported from `OpenHuman`'s `inference::voice::hallucination` tests, which
//! encode transcripts observed in the field rather than invented ones.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use super::{Mode, is_hallucinated};

// --- Exact match, both modes ---

#[test]
fn blank_audio_markers_are_caught() {
    assert!(is_hallucinated("[blank_audio]", Mode::Conversation));
    assert!(is_hallucinated("[ blank_audio ]", Mode::Conversation));
    assert!(is_hallucinated("(blank audio)", Mode::Conversation));
}

#[test]
fn subtitle_phrases_are_caught() {
    assert!(is_hallucinated(
        "Thank you for watching",
        Mode::Conversation
    ));
    assert!(is_hallucinated("please subscribe", Mode::Conversation));
    assert!(is_hallucinated("See you next time.", Mode::Conversation));
}

#[test]
fn punctuation_only_is_caught() {
    assert!(is_hallucinated("...", Mode::Conversation));
    assert!(is_hallucinated(".", Mode::Conversation));
}

// --- The mode split, which is the whole reason `Mode` exists ---

#[test]
fn dictation_drops_single_filler_words() {
    for text in ["you", "okay", "yes", "Thank you."] {
        assert!(
            is_hallucinated(text, Mode::Dictation),
            "{text:?} should be filtered in dictation"
        );
    }
}

#[test]
fn conversation_keeps_the_same_words_as_real_replies() {
    for text in ["yes", "no", "okay", "thank you", "goodbye"] {
        assert!(
            !is_hallucinated(text, Mode::Conversation),
            "{text:?} is a legitimate chat reply and must pass"
        );
    }
}

// --- Repetition layers ---

#[test]
fn uniform_repetition_is_caught_with_and_without_punctuation() {
    assert!(is_hallucinated("you you you you", Mode::Conversation));
    assert!(is_hallucinated("the the the the the", Mode::Conversation));
    assert!(is_hallucinated("it... it... it...", Mode::Conversation));
    assert!(is_hallucinated("it, it, it, it", Mode::Conversation));
}

#[test]
fn repeating_ngram_is_caught_when_no_single_word_dominates() {
    // "thank" and "you" are 50% each — layer 4 cannot see this, layer 3 can.
    assert!(is_hallucinated(
        "Thank you. Thank you. Thank you.",
        Mode::Conversation
    ));
}

#[test]
fn dominant_word_loop_is_caught() {
    // "it" is 8/10 = 80%, count 8.
    assert!(is_hallucinated(
        "it it it it it it it it hello world",
        Mode::Conversation
    ));
}

// --- The false-positive guards. These are the ones that matter: over-eager
// filtering silently deletes real speech, which is worse than letting an
// artefact through. ---

#[test]
fn emphatic_repetition_is_left_alone() {
    // 3/6 = 50%, count 3 — under both bounds.
    assert!(!is_hallucinated(
        "no no no don't do that",
        Mode::Conversation
    ));
    // 3/5 = 60%, which does not *exceed* the ratio, and count 3 < 5.
    assert!(!is_hallucinated("go go go turn left", Mode::Conversation));
}

#[test]
fn moderate_repetition_is_left_alone() {
    // "thank" is 3/7 = 43%.
    assert!(!is_hallucinated(
        "thank you thank you thank you hello",
        Mode::Conversation
    ));
}

#[test]
fn ordinary_sentences_pass() {
    assert!(!is_hallucinated(
        "Can you check the latest price of Bitcoin?",
        Mode::Conversation
    ));
    assert!(!is_hallucinated(
        "I went to the store and the park today",
        Mode::Conversation
    ));
    assert!(!is_hallucinated(
        "Hey team, let's discuss the new feature implementation plan for next sprint",
        Mode::Conversation
    ));
}

#[test]
fn empty_is_not_a_hallucination() {
    // "nothing was said" is a different outcome the host handles separately.
    assert!(!is_hallucinated("", Mode::Conversation));
    assert!(!is_hallucinated("   ", Mode::Conversation));
    assert!(!is_hallucinated("", Mode::Dictation));
}

#[test]
fn two_words_are_below_the_repetition_layers() {
    assert!(!is_hallucinated("hello world", Mode::Conversation));
}

#[test]
fn mode_round_trips_as_json() {
    let json = serde_json::to_string(&Mode::Dictation).expect("serialize");
    assert_eq!(json, "\"dictation\"");
    let back: Mode = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, Mode::Dictation);
}
