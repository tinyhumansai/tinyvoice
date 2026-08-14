# TinyVoice

Host-agnostic voice primitives, extracted from [OpenHuman](https://github.com/tinyhumansai/openhuman): WAV framing, silence gating, voice-activity segmentation, wake-word gating, fast-path command routing, and STT hallucination detection.

Ships two things from one repository:

| Crate | Output | For |
| --- | --- | --- |
| `tinyvoice` (root) | `rlib` | A host that links the logic in-process |
| `tinyvoice-module` (`crates/`) | `cdylib` | A host that loads it over the TinyBus module ABI |

## What belongs here, and what does not

The split follows one rule, the same one `tinydocs` and `tinywallet` follow:
**a crate owns what is identical for every host; the host owns what depends on
its own runtime, config, or threat model.**

So this crate is synchronous, I/O-free and runtime-free. It does not open a
microphone, call an STT or TTS endpoint, own a hotkey, or know what a `Config`
is.

| Here | With the host |
| --- | --- |
| WAV framing, RMS, resampling, downmix, silence gate | Device capture (`cpal`) — a stream is `!Send` and needs the host's thread and permission model |
| VAD segmentation | The capture loop that drives it |
| Wake-word gate, intent routing | What to *do* with an intent |
| Hallucination detection | The STT transport, credentials, and retry policy |

A visible consequence: `VadConfig` has no constructor that reads a config file.
A host builds one from whatever it persists. A crate that guessed at that shape
would be wrong for every host that guessed differently.

## Use it as a library

```rust
use tinyvoice::{
    intent::{extract_command, route, VoiceIntent},
    transcript::{is_hallucinated, Mode},
};

// An always-on microphone hears the whole room, so the wake word is what
// separates an instruction from a passing conversation.
let Some(command) = extract_command("hey tiny, pause the music", "Hey Tiny") else {
    return; // not addressed to the agent
};

// A model fed near-silence returns stock phrases rather than nothing.
if is_hallucinated(&command, Mode::Conversation) {
    return;
}

assert_eq!(route(&command), VoiceIntent::Pause);
```

Run it: `cargo run --example basic`.

## Use it as a TinyBus module

The module claims `ai.tinyhumans.tinyvoice.Voice` and serves
`/ai/tinyhumans/tinyvoice/Voice`. See [`MODULE.md`](MODULE.md) for installation
and the method list.

**Every method is stateless and per-utterance** — a transcript in, a verdict
out; a recording in, a container out. That boundary is deliberate. The one piece
that does not fit it is the VAD: a segmenter is driven once per 20 ms frame, and
a bus round trip at that cadence costs more than the work it carries. `Segment`
therefore takes a *batch* of frame energies.

**A host in a hard-realtime capture loop should link the rlib and hold its own
`VadSegmenter`.** The library has no bus dependency precisely so that this is
possible.

## Layout

```text
src/
├── lib.rs              # crate docs + the public re-export surface
├── error/              # crate-wide `Error` and `Result<T>`
├── audio/              # WAV framing, RMS, resample, downmix, silence gate
├── vad/                # the voice-activity state machine
├── intent/             # wake-word gate (`wake.rs`) + command routing
└── transcript/         # STT hallucination detection
crates/tinyvoice-module/
├── src/service/        # bus interface, setup, ABI v1 exports
└── examples/           # local and tagged-release module verification
vendor/tinybus/         # pinned TinyBus submodule (build-time only)
```

After cloning: `git submodule update --init vendor/tinybus`.

## Development

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

CI additionally requires 90% line coverage in every source file and a clean
`rustdoc -D warnings`. See [`AGENTS.md`](AGENTS.md) for the conventions.

## License

GPL-3.0-only. See [`LICENSE`](LICENSE).
