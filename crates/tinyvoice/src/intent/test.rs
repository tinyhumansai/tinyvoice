//! Ported from `OpenHuman`'s `voice::command_router` and `voice::always_on`
//! wake-word tests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use super::{VoiceIntent, extract_command, route, wake_word_present};

// --- Routing ---

#[test]
fn transport_controls() {
    assert_eq!(route("pause"), VoiceIntent::Pause);
    assert_eq!(route("Pause the music."), VoiceIntent::Pause);
    assert_eq!(route("please stop"), VoiceIntent::Pause);
    assert_eq!(route("resume"), VoiceIntent::Resume);
    assert_eq!(route("next song"), VoiceIntent::Next);
    assert_eq!(route("skip"), VoiceIntent::Next);
    assert_eq!(route("previous track"), VoiceIntent::Previous);
    assert_eq!(route("mute"), VoiceIntent::Mute);
}

#[test]
fn volume_controls() {
    assert_eq!(route("turn it up"), VoiceIntent::VolumeUp);
    assert_eq!(route("louder"), VoiceIntent::VolumeUp);
    assert_eq!(route("lower the volume"), VoiceIntent::VolumeDown);
    assert_eq!(
        route("set volume to 40"),
        VoiceIntent::SetVolume { percent: 40 }
    );
    assert_eq!(
        route("set the volume to 100 percent"),
        VoiceIntent::SetVolume { percent: 100 }
    );
    assert_eq!(route("volume 25"), VoiceIntent::SetVolume { percent: 25 });
}

#[test]
fn out_of_range_volume_is_clamped_not_rejected() {
    assert_eq!(
        route("set volume to 250"),
        VoiceIntent::SetVolume { percent: 100 }
    );
}

#[test]
fn play_intent_cleans_the_query() {
    assert_eq!(
        route("play Numb by Linkin Park"),
        VoiceIntent::Play {
            query: "numb linkin park".into()
        }
    );
    assert_eq!(
        route("please play the song Highway to Hell on spotify"),
        VoiceIntent::Play {
            query: "highway to hell".into()
        }
    );
}

#[test]
fn open_app_intent_strips_filler() {
    assert_eq!(
        route("open Slack"),
        VoiceIntent::OpenApp {
            app: "slack".into()
        }
    );
    assert_eq!(
        route("can you open up Slack"),
        VoiceIntent::OpenApp {
            app: "slack".into()
        }
    );
    assert_eq!(
        route("launch the calculator"),
        VoiceIntent::OpenApp {
            app: "calculator".into()
        }
    );
}

#[test]
fn ambiguous_play_defers_to_the_agent() {
    // A bare pronoun carries no song. Taking the local route with a
    // meaningless query would do the wrong thing confidently; deferring only
    // costs a round trip.
    for q in [
        "play it",
        "play them",
        "play a song",
        "play songs",
        "play music",
        "play",
    ] {
        assert_eq!(route(q), VoiceIntent::Unknown, "{q} should be Unknown");
    }
}

#[test]
fn anything_unrecognised_defers_to_the_agent() {
    assert_eq!(route("what's the weather in Tokyo"), VoiceIntent::Unknown);
    assert_eq!(
        route("message Steven on slack saying hi"),
        VoiceIntent::Unknown
    );
    assert_eq!(route(""), VoiceIntent::Unknown);
    assert_eq!(route("   "), VoiceIntent::Unknown);
}

#[test]
fn kind_is_stable_and_carries_no_transcript() {
    assert_eq!(route("pause").kind(), "pause");
    assert_eq!(route("turn it up").kind(), "volume_up");
    assert_eq!(VoiceIntent::Unknown.kind(), "unknown");

    let intent = VoiceIntent::Play {
        query: "secret song".into(),
    };
    assert_eq!(intent.kind(), "play");
    assert!(
        !intent.kind().contains("secret"),
        "kind() is used in logs and must never leak what was said"
    );
}

// --- Wake word ---

#[test]
fn wake_word_extracts_the_command_after_it() {
    assert_eq!(
        extract_command("Hey Tiny, play Numb by Linkin Park", "Hey Tiny").as_deref(),
        Some("play numb by linkin park")
    );
    assert_eq!(
        extract_command("hey tiny open slack", "Hey Tiny").as_deref(),
        Some("open slack")
    );
    assert_eq!(
        extract_command("um, hey tiny what time is it", "Hey Tiny").as_deref(),
        Some("what time is it"),
        "leading filler before the wake word is tolerated"
    );
}

#[test]
fn wake_word_tolerates_stt_homophones() {
    // STT mangles the greeting and the spelling; an exact match would reject
    // utterances a listener would call obviously correct.
    assert_eq!(
        extract_command("Hey Tony, play music", "Hey Tiny").as_deref(),
        Some("play music")
    );
    assert_eq!(
        extract_command("a tinny open slack", "Hey Tiny").as_deref(),
        Some("open slack")
    );
}

#[test]
fn an_early_anchor_can_trigger_and_that_is_a_known_trade_off() {
    // Pinning the documented behaviour, not endorsing it. Fuzzy-matching the
    // anchor anywhere in the leading three tokens is what makes "a tinny open
    // slack" work, and the same rule fires here on an ordinary sentence that
    // happens to start "the tiny …".
    //
    // Tightening it is a product decision with a real cost on the other side —
    // a missed wake word reads to the user as a dead microphone — so it is
    // recorded here rather than quietly "fixed" during the extraction.
    assert_eq!(
        extract_command("the tiny details matter here a lot", "Hey Tiny").as_deref(),
        Some("details matter here a lot")
    );
}

#[test]
fn an_anchor_past_the_window_does_not_trigger() {
    assert_eq!(
        extract_command("i think the details here are tiny in the end", "Hey Tiny"),
        None,
        "'tiny' beyond the leading window is a word, not an address"
    );
}

#[test]
fn absent_or_bare_wake_word_yields_no_command() {
    assert_eq!(extract_command("play some music", "Hey Tiny"), None);
    // A bare address is not an instruction — there is nothing to route.
    assert_eq!(extract_command("Hey Tiny", "Hey Tiny"), None);
    assert_eq!(extract_command("hey tiny!", "Hey Tiny"), None);
}

#[test]
fn empty_wake_word_disables_the_gate() {
    assert_eq!(
        extract_command("play some music", "").as_deref(),
        Some("play some music")
    );
    assert_eq!(extract_command("   ", ""), None);
}

#[test]
fn presence_detection_covers_bare_and_fuzzy_forms() {
    assert!(wake_word_present("Hey Tiny", "Hey Tiny"));
    assert!(wake_word_present("hey tiny!", "Hey Tiny"));
    assert!(wake_word_present("hey tony", "Hey Tiny"));
    assert!(wake_word_present("Hey Tiny, play music", "Hey Tiny"));
}

#[test]
fn presence_detection_is_false_when_absent() {
    assert!(!wake_word_present("play some music", "Hey Tiny"));
    assert!(!wake_word_present("", "Hey Tiny"));
    // An empty wake word has nothing to be present, which is deliberately not
    // the mirror of `extract_command`'s "gate disabled" reading.
    assert!(!wake_word_present("anything at all", ""));
}

#[test]
fn intent_round_trips_as_json() {
    let intent = VoiceIntent::SetVolume { percent: 40 };
    let json = serde_json::to_string(&intent).expect("serialize");
    assert_eq!(json, r#"{"intent":"set_volume","percent":40}"#);
    assert_eq!(intent, serde_json::from_str(&json).expect("deserialize"));
}
