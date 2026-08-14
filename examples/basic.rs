//! Ordinary library use: gate an utterance, route it, and screen the result.

use tinyvoice::{
    intent::{VoiceIntent, extract_command, route},
    transcript::{Mode, is_hallucinated},
};

fn main() {
    // An always-on microphone hears everything in the room, so the wake word
    // is what separates an instruction from a passing conversation.
    let heard = "um, hey tiny, play Numb by Linkin Park";
    let Some(command) = extract_command(heard, "Hey Tiny") else {
        println!("not addressed to the agent");
        return;
    };

    // A model fed near-silence returns stock phrases rather than nothing.
    if is_hallucinated(&command, Mode::Conversation) {
        println!("discarded a hallucinated transcript");
        return;
    }

    match route(&command) {
        VoiceIntent::Play { query } => println!("play: {query}"),
        VoiceIntent::Unknown => println!("no fast path; hand {command:?} to the agent"),
        other => println!("fast path: {}", other.kind()),
    }
}
