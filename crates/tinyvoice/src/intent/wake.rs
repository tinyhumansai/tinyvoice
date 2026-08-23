//! The wake-word gate.
//!
//! # Why the match is fuzzy
//!
//! An always-on microphone feeds this an STT transcript, and STT mangles short
//! greetings reliably: "hey" arrives as "a" or "ok", and "tiny" as "tony" or
//! "tinny". An exact match would fail on utterances a listener would call
//! obviously correct, so matching anchors on the *longest* wake token — the
//! distinctive one — and allows a small edit distance around it.
//!
//! Two bounds keep that tolerance from becoming a false trigger: only the first
//! three tokens are considered (a wake word belongs at the start of an address,
//! not buried mid-sentence), and the permitted distance scales with the anchor's
//! length, so a four-character anchor allows one edit rather than two.

/// Tokenise into lowercase alphanumeric words.
///
/// Shared by both public functions so they normalise identically — a matcher
/// and a detector that disagreed about punctuation would answer differently for
/// the same transcript.
fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(String::from)
        .collect()
}

/// How many tokens from the start of a transcript may hold the wake word.
const ANCHOR_WINDOW: usize = 3;

/// The distinctive token to match on, and the edit distance allowed for it.
fn anchor(wake_word: &str) -> Option<(String, usize)> {
    let wake = tokens(wake_word);
    let anchor = wake.into_iter().max_by_key(String::len)?;
    let max_distance = usize::from(anchor.chars().count() > 4) + 1;
    Some((anchor, max_distance))
}

/// Position of the wake word within `transcript`'s leading tokens, if present.
fn anchor_position(transcript_tokens: &[String], wake_word: &str) -> Option<usize> {
    let (anchor, max_distance) = anchor(wake_word)?;
    (0..transcript_tokens.len().min(ANCHOR_WINDOW))
        .find(|&i| levenshtein(&transcript_tokens[i], &anchor) <= max_distance)
}

/// True when the wake word appears near the start of `transcript`, whether or
/// not a command follows.
///
/// Lets a host acknowledge a bare "Hey Tiny" instead of silently dropping it,
/// which otherwise reads to the user as the microphone being dead.
///
/// An empty `wake_word` returns `false` — there is no wake word to be present.
/// Note this is *not* the mirror of [`extract_command`], which treats an empty
/// wake word as "gate disabled, pass everything".
#[must_use]
pub fn wake_word_present(transcript: &str, wake_word: &str) -> bool {
    anchor_position(&tokens(transcript), wake_word).is_some()
}

/// Apply the wake-word gate, returning the command that followed it.
///
/// Returns `None` when the utterance was not addressed to the agent, and also
/// when the wake word appeared with nothing after it — a bare wake word is an
/// address, not an instruction, so there is no command to route.
///
/// An empty `wake_word` disables the gate: every non-empty utterance passes
/// through, normalised.
#[must_use]
pub fn extract_command(transcript: &str, wake_word: &str) -> Option<String> {
    let transcript_tokens = tokens(transcript);

    if tokens(wake_word).is_empty() {
        return if transcript_tokens.is_empty() {
            None
        } else {
            Some(transcript_tokens.join(" "))
        };
    }

    let position = anchor_position(&transcript_tokens, wake_word)?;
    let command = transcript_tokens[position + 1..].join(" ");
    if command.trim().is_empty() {
        None
    } else {
        Some(command)
    }
}

/// Classic Levenshtein edit distance.
///
/// Inputs are single wake-word tokens, so the quadratic table is a handful of
/// cells and the two-row form is only to avoid allocating a full matrix.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        core::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}
