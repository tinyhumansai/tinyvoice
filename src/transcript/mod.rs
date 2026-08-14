//! Detecting the things an STT model says when nobody spoke.
//!
//! Whisper-family models do not return an empty string for silence. Trained
//! largely on subtitled video, they return what such video usually ends with:
//! `[BLANK_AUDIO]`, "Thank you for watching", "Please subscribe" — or they
//! latch onto a single token and loop it. A pipeline that treats those as
//! speech will happily transcribe an empty room and hand the result to an
//! agent as an instruction.
//!
//! # Why there are two modes
//!
//! The filter cannot be equally aggressive everywhere, because the same
//! utterance means different things in different pipelines. In push-to-talk
//! dictation, a lone "okay" is almost always a noise artefact — the user held a
//! key in order to say something, and one filler word is not it. In a
//! conversation, a lone "okay" is a complete and common reply. Filtering it in
//! both places would break chat; filtering it in neither would leak artefacts
//! into dictated text.
//!
//! So [`Mode`] picks which list applies. The blank markers and the
//! subtitle phrases in `ALWAYS` are filtered in both — those are never
//! legitimate speech in either setting.

// Word counts are compared as a ratio, so they cross from `usize` into `f64`.
// An utterance long enough for that to lose precision (2^53 words) cannot exist.
#![allow(clippy::cast_precision_loss)]

#[cfg(test)]
mod test;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How aggressively to filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Push-to-talk dictation. Aggressive: single-word noise artefacts and
    /// short conversational phrases are treated as hallucination.
    Dictation,
    /// Chat voice input. Conservative: only blank markers, subtitle phrases and
    /// repetition patterns are filtered, so "yes" or "okay" pass through.
    Conversation,
}

/// Blank-audio markers and subtitle-trained phrases, filtered in every mode.
const ALWAYS: &[&str] = &[
    // Blank markers emitted by whisper.cpp.
    "[blank_audio]",
    "[ blank_audio ]",
    "[blank audio]",
    "(blank audio)",
    // Common hallucinations from video-trained models.
    "thank you for watching",
    "thanks for watching",
    "thank you for listening",
    "thanks for listening",
    "thank you so much",
    "please subscribe",
    "like and subscribe",
    "see you next time",
    "see you in the next video",
    "bye bye",
    // Punctuation only.
    "...",
    ".",
    ",",
    "!",
    "?",
];

/// Short phrases and filler words filtered only in [`Mode::Dictation`].
const DICTATION_ONLY: &[&str] = &[
    "thank you",
    "thank you.",
    "thanks.",
    "bye.",
    "goodbye.",
    // Single-word noise artefacts.
    "you",
    "the",
    "i",
    "a",
    "so",
    "okay",
    "ok",
    "yeah",
    "yes",
    "no",
    "oh",
    "hmm",
    "huh",
    "ah",
];

/// A word must appear at least this many times, and account for more than
/// `DOMINANCE_RATIO` of the utterance, to count as a loop.
///
/// Both bounds are needed. The ratio alone would flag "no no no" (3/3); the
/// count alone would flag a long sentence that happens to use "the" five times.
const MIN_DOMINANT_OCCURRENCES: usize = 5;

/// Share of an utterance a single word must exceed to look like a loop.
///
/// Deliberately loose enough to let emphatic speech through: "no no no don't do
/// that" is 3 of 6 words, which does not exceed this.
const DOMINANCE_RATIO: f64 = 0.6;

/// The longest repeating phrase length checked by the n-gram layer.
const MAX_NGRAM: usize = 3;

/// Strip ASCII punctuation from a word, leaving its bare core.
fn strip_punctuation(word: &str) -> String {
    word.chars().filter(|c| !c.is_ascii_punctuation()).collect()
}

/// Whether an STT transcript looks like a hallucination rather than speech.
///
/// Returns `false` for an empty or whitespace-only transcript. That is not an
/// oversight: "nothing was said" and "something false was said" are different
/// outcomes with different handling, and a host distinguishes them by checking
/// for emptiness itself.
///
/// The layers, in order:
///
/// 1. **Exact match** against `ALWAYS`, plus `DICTATION_ONLY` in
///    [`Mode::Dictation`].
/// 2. **Uniform repetition** — every word identical once punctuation is
///    stripped, which catches both "you you you you" and "it... it... it...".
/// 3. **Repeating n-gram** — the whole utterance is a phrase of up to
///    `MAX_NGRAM` words looped, which catches "Thank you. Thank you. Thank
///    you." where no *single* word dominates.
/// 4. **Dominant word** — see `DOMINANCE_RATIO`.
///
/// Layers 2 to 4 apply only to transcripts of three or more words; below that
/// there is not enough signal to call a repetition, and short real utterances
/// would be caught.
#[must_use]
pub fn is_hallucinated(text: &str, mode: Mode) -> bool {
    let normalized = text.trim().to_lowercase();
    if normalized.is_empty() {
        return false;
    }

    // Engines often append a period; match with and without trailing marks.
    let stripped = normalized.trim_end_matches(|c: char| c.is_ascii_punctuation());

    let exact = |patterns: &[&str]| patterns.iter().any(|p| normalized == *p || stripped == *p);
    if exact(ALWAYS) || (mode == Mode::Dictation && exact(DICTATION_ONLY)) {
        return true;
    }

    if normalized.split_whitespace().count() < 3 {
        return false;
    }

    let words: Vec<String> = normalized
        .split_whitespace()
        .map(strip_punctuation)
        .filter(|w| !w.is_empty())
        .collect();
    let Some(first) = words.first() else {
        return false;
    };

    // Layer 2: every word the same.
    if words.iter().all(|w| w == first) {
        return true;
    }

    // Layer 3: the whole utterance is one short phrase, repeated.
    for len in 1..=MAX_NGRAM {
        if words.len() >= len * 2 && words.len().is_multiple_of(len) {
            let pattern = &words[..len];
            if words.chunks(len).all(|chunk| chunk == pattern) {
                return true;
            }
        }
    }

    // Layer 4: one word dominates.
    let total = words.len();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for w in &words {
        *counts.entry(w.as_str()).or_default() += 1;
    }
    counts.values().any(|&count| {
        count >= MIN_DOMINANT_OCCURRENCES && (count as f64 / total as f64) > DOMINANCE_RATIO
    })
}
