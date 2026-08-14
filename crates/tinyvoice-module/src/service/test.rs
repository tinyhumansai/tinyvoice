//! Integration tests for the bus adapter, over a real in-memory `TinyBus`.
//!
//! These assert the *wire* contract — method names, payload shapes, error
//! wording — rather than the behaviour underneath, which the `tinyvoice` unit
//! tests already cover. A change that only moves logic should leave every
//! assertion here untouched; one that fires means the contract moved.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use super::{BUS_NAME, MAX_AUDIO_BYTES, OBJECT_PATH, VoiceService, setup};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use tinybus::broker::Broker;
use tinybus::transport::memory::MemoryBus;
use tinybus::{Connection, Interface};

/// A live bus with the interface served on it.
///
/// The service `Connection` is held here rather than dropped at the end of
/// setup: a peer that goes out of scope releases its bus name, and every call
/// then fails with `NameHasNoOwner` — which looks like a registration bug
/// rather than a lifetime one.
struct Served {
    proxy: tinybus::Proxy,
    _service: Connection,
}

impl std::ops::Deref for Served {
    type Target = tinybus::Proxy;

    fn deref(&self) -> &Self::Target {
        &self.proxy
    }
}

/// Stand up a broker, serve the interface, and hand back a client proxy.
async fn connect() -> tinybus::Result<Served> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());

    let service = Connection::connect(bus.connect().await?).await?;
    setup(service.clone()).await?;

    let client = Connection::connect(bus.connect().await?).await?;
    Ok(Served {
        proxy: client.proxy(BUS_NAME, OBJECT_PATH, BUS_NAME)?,
        _service: service,
    })
}

/// Little-endian `f32` samples, base64'd, as the wire carries them.
fn encode_samples(samples: &[f32]) -> String {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    BASE64.encode(bytes)
}

#[test]
fn declared_methods_match_the_dispatch_table() {
    // The `module_export!` manifest lists these names separately from the
    // `#[interface]` impl, so nothing but this test keeps the two in step. A
    // manifest that over-claims makes the host advertise a method that is not
    // there; one that under-claims hides a method that is.
    let mut methods = VoiceService
        .members()
        .into_iter()
        .map(|member| member.to_string())
        .collect::<Vec<_>>();
    methods.sort_unstable();

    assert_eq!(
        methods,
        [
            "EncodeWav",
            "ExtractCommand",
            "IsHallucinated",
            "PrepareCapture",
            "Route",
            "Segment",
            "WakeWordPresent",
        ]
    );
}

#[tokio::test]
async fn route_returns_a_tagged_intent() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let json: String = proxy.call("Route", ("please pause the music",)).await?;
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(value["intent"], "pause");
    Ok(())
}

#[tokio::test]
async fn route_carries_the_payload_for_a_parameterised_intent() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let json: String = proxy.call("Route", ("set volume to 40",)).await?;
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(value["intent"], "set_volume");
    assert_eq!(value["percent"], 40);
    Ok(())
}

#[tokio::test]
async fn an_unrecognised_transcript_comes_back_as_unknown_not_an_error() -> tinybus::Result<()> {
    // Routing may only ever shortcut. "I could not classify this" is a normal
    // answer the host acts on by calling its agent, not a fault.
    let proxy = connect().await?;
    let json: String = proxy
        .call("Route", ("what is the weather in Tokyo",))
        .await?;
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(value["intent"], "unknown");
    Ok(())
}

#[tokio::test]
async fn extract_command_strips_the_wake_word() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let command: String = proxy
        .call("ExtractCommand", ("hey tiny open slack", "Hey Tiny"))
        .await?;

    assert_eq!(command, "open slack");
    Ok(())
}

#[tokio::test]
async fn an_unaddressed_utterance_yields_an_empty_command() -> tinybus::Result<()> {
    let proxy = connect().await?;

    // Not addressed at all, and addressed with nothing after it, are the same
    // outcome for a caller: there is no command to route.
    let unaddressed: String = proxy
        .call("ExtractCommand", ("open slack", "Hey Tiny"))
        .await?;
    assert_eq!(unaddressed, "");

    let bare: String = proxy
        .call("ExtractCommand", ("hey tiny", "Hey Tiny"))
        .await?;
    assert_eq!(bare, "");
    Ok(())
}

#[tokio::test]
async fn wake_word_presence_distinguishes_a_bare_address() -> tinybus::Result<()> {
    let proxy = connect().await?;

    // This is what lets a host acknowledge "Hey Tiny" instead of staying
    // silent, which reads to the user as a dead microphone.
    let present: bool = proxy
        .call("WakeWordPresent", ("hey tiny", "Hey Tiny"))
        .await?;
    assert!(present);

    let absent: bool = proxy
        .call("WakeWordPresent", ("open slack", "Hey Tiny"))
        .await?;
    assert!(!absent);
    Ok(())
}

#[tokio::test]
async fn hallucination_modes_disagree_over_the_bus() -> tinybus::Result<()> {
    let proxy = connect().await?;

    let dictation: bool = proxy.call("IsHallucinated", ("okay", "dictation")).await?;
    let conversation: bool = proxy
        .call("IsHallucinated", ("okay", "conversation"))
        .await?;

    assert!(dictation, "a lone filler word is an artefact in dictation");
    assert!(!conversation, "the same word is a real reply in chat");
    Ok(())
}

#[tokio::test]
async fn an_unknown_mode_is_refused_rather_than_defaulted() -> tinybus::Result<()> {
    // The two modes disagree about real speech, so guessing would either
    // delete legitimate replies or leak artefacts into dictated text.
    let proxy = connect().await?;
    let result = proxy
        .call::<bool>("IsHallucinated", ("okay", "aggressive"))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "unknown mode unexpectedly succeeded",
        ));
    };
    assert!(
        error.to_string().contains("unknown hallucination mode"),
        "got: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn segment_reports_the_frame_each_event_landed_on() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let config = serde_json::json!({
        "onset_threshold": 0.1,
        "hangover_ms": 100,
        "min_speech_ms": 60,
        "max_utterance_ms": 5000,
    })
    .to_string();

    // Six loud frames (120ms voiced), then silence past the 100ms hangover.
    let mut energies = vec![0.5f32; 6];
    energies.extend(std::iter::repeat_n(0.0f32, 6));

    let json: String = proxy.call("Segment", (config, 20u32, energies)).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(events.len(), 2, "one start and one end: {events:?}");
    assert_eq!(events[0]["kind"], "speech_start");
    assert_eq!(events[0]["frame"], 0);
    assert_eq!(events[1]["kind"], "speech_end");
    assert_eq!(events[1]["emit"], true);
    assert_eq!(events[1]["forced"], false);
    // The index is what lets a host cut the audio in the right place.
    assert_eq!(events[1]["frame"], 10);
    Ok(())
}

#[tokio::test]
async fn a_malformed_vad_config_is_reported_not_defaulted() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let result = proxy
        .call::<String>(
            "Segment",
            (r#"{"onset_threshold":0.1}"#, 20u32, vec![0.5f32]),
        )
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "partial config unexpectedly succeeded",
        ));
    };
    assert!(
        error.to_string().contains("invalid VAD config"),
        "got: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn a_zero_frame_duration_is_refused() -> tinybus::Result<()> {
    // Every duration the segmenter reports is a multiple of this, so zero
    // would silently make every utterance 0ms long and fail `min_speech_ms`.
    let proxy = connect().await?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default()).expect("serialize");
    let result = proxy
        .call::<String>("Segment", (config, 0u32, vec![0.5f32]))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "zero frame_ms unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("frame_ms"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn encode_wav_round_trips_through_base64() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let samples = encode_samples(&[0.0, 0.5, -0.5]);

    let encoded: String = proxy.call("EncodeWav", (samples, 16_000u32)).await?;
    let wav = BASE64.decode(&encoded).expect("valid base64");

    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(wav.len(), 44 + 3 * 2);
    Ok(())
}

#[tokio::test]
async fn prepare_capture_downmixes_resamples_and_frames_in_one_call() -> tinybus::Result<()> {
    // Three round trips would ship the same audio across the bus three times
    // to do work that is microseconds of arithmetic.
    let proxy = connect().await?;
    // 400 interleaved samples = 200 stereo frames at 32 kHz
    //   -> 200 mono samples -> 100 samples at 16 kHz.
    let stereo: Vec<f32> = (0..400).map(|i| ((i as f32) / 20.0).sin() * 0.5).collect();

    let encoded: String = proxy
        .call(
            "PrepareCapture",
            (encode_samples(&stereo), 32_000u32, 2u16, 0.0f32),
        )
        .await?;
    let wav = BASE64.decode(&encoded).expect("valid base64");

    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(wav.len(), 44 + 100 * 2);
    // The header must declare the rate the samples were converted to, not the
    // one they arrived at.
    assert_eq!(
        u32::from_le_bytes(wav[24..28].try_into().expect("4 bytes")),
        16_000
    );
    Ok(())
}

#[tokio::test]
async fn a_ragged_channel_count_is_reported() -> tinybus::Result<()> {
    let proxy = connect().await?;
    // Three samples cannot be whole stereo frames.
    let result = proxy
        .call::<String>(
            "PrepareCapture",
            (encode_samples(&[0.1, 0.2, 0.3]), 32_000u32, 2u16, 0.0f32),
        )
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "ragged input unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("whole number"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn a_truncated_sample_buffer_is_refused() -> tinybus::Result<()> {
    let proxy = connect().await?;
    // Five bytes is not a whole number of f32 samples.
    let result = proxy
        .call::<String>("EncodeWav", (BASE64.encode([0u8; 5]), 16_000u32))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "truncated buffer unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("f32 samples"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn an_oversized_payload_is_refused_before_it_is_decoded() -> tinybus::Result<()> {
    // The length check runs on the encoded string. Allocating first and
    // validating after is how a hostile size becomes an allocation failure,
    // which aborts the host rather than returning an error it can handle.
    let proxy = connect().await?;
    let oversized = "A".repeat(MAX_AUDIO_BYTES / 3 * 4 + 8);
    let result = proxy
        .call::<String>("EncodeWav", (oversized, 16_000u32))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "oversized payload unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("exceeds"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn a_zero_sample_rate_is_refused() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let result = proxy
        .call::<String>("EncodeWav", (encode_samples(&[0.1]), 0u32))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "zero sample rate unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("sample rate"), "got: {error}");
    Ok(())
}
