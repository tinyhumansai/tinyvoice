# TinyVoice TinyBus Module

This package contains the native `tinyvoice` module for TinyBus module ABI v1.
Install only the archive matching the host operating system and architecture.

The module claims `ai.tinyhumans.tinyvoice.Voice`, serves the object at
`/ai/tinyhumans/tinyvoice/Voice`, and provides seven methods.

## Methods

| Method | Arguments | Returns |
| --- | --- | --- |
| `Route` | `transcript` | JSON `VoiceIntent`, tagged by an `intent` field |
| `ExtractCommand` | `transcript`, `wake_word` | The command after the wake word, or `""` |
| `WakeWordPresent` | `transcript`, `wake_word` | `bool` |
| `IsHallucinated` | `text`, `mode` | `bool` |
| `Segment` | `config`, `frame_ms`, `energies` | JSON array of frame-indexed VAD events |
| `EncodeWav` | `samples`, `sample_rate` | base64 WAV |
| `PrepareCapture` | `samples`, `source_rate`, `channels`, `gate_threshold` | base64 WAV |

Notes on the contract:

- `mode` is `"dictation"` or `"conversation"`. An unrecognised value is an
  **error**, not a silent fallback: the two modes disagree about whether a bare
  "okay" is real speech, so guessing would either delete legitimate chat replies
  or leak artefacts into dictated text.
- `ExtractCommand` returns `""` both when the utterance was not addressed to the
  agent and when the wake word arrived with nothing after it. Those are the same
  outcome for a caller — there is nothing to route either way.
- `Route` never fails on an unrecognised transcript; it answers
  `{"intent":"unknown"}`. Routing may only ever shortcut, never block.
- `samples` are base64 little-endian `f32`. Payloads are capped at 8 MiB, which
  is roughly eight minutes of 16 kHz mono, and the cap is checked *before*
  decoding so an oversized value returns an error rather than an allocation
  failure.
- `Segment` starts a fresh segmenter per call and drops it afterwards, so submit
  whole utterances — a batch that cuts one in half loses the open segment. A
  host in a realtime capture loop should link the `tinyvoice` rlib and hold its
  own `VadSegmenter` instead of calling this per frame.

## Installing

The archive contains one `.so`, `.dylib`, or `.dll` plus `modules.toml`. Keep
those files together when copying them into a TinyBus module directory. The
allowlist binds the native library filename to its SHA-256 digest so TinyBus can
reject a missing, renamed, or modified artifact before initialization.

The GitHub release also publishes `checksum.toml` as a separate asset. TinyBus
checks that manifest before downloading and extracting the selected platform
archive. Install directly from a tagged release with:

```sh
tinybus modules load-github \
  https://github.com/tinyhumansai/tinyvoice/releases/tag/v0.1.0 \
  tinyvoice-module-0.1.0-ubuntu-24.04-x86_64.tar.gz \
  <archive-sha256>
```

TinyBus modules are trusted in-process code: a loaded module shares the address
space, the privileges and the crash domain, and TinyBus never unloads one.
Install release artifacts only from a trusted source, and restart the host after
replacing a loaded module.
