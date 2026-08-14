//! The bus interface, its setup, and the ABI v1 exports.

#[cfg(test)]
mod test;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use tinybus::{Connection, Result as TinyBusResult};

use tinyvoice::audio::{self, SilenceGateConfig};
use tinyvoice::intent;
use tinyvoice::transcript::{self, Mode};
use tinyvoice::vad::{VadConfig, VadEvent, VadSegmenter};

/// Well-known bus name this module claims.
pub const BUS_NAME: &str = "ai.tinyhumans.tinyvoice.Voice";

/// Object path the interface is served at.
pub const OBJECT_PATH: &str = "/ai/tinyhumans/tinyvoice/Voice";

/// Largest audio payload accepted or produced in a single call.
///
/// A bus frame caps at 16 MiB and base64 costs 1.34x, so this leaves ample room
/// for the JSON envelope around the payload. In wall-clock terms it is roughly
/// eight minutes of 16 kHz mono PCM — far longer than the utterances this path
/// exists to carry, and short enough that a caller cannot turn one frame into
/// an allocation failure.
pub const MAX_AUDIO_BYTES: usize = 8 * 1024 * 1024;

/// The served object.
///
/// Zero-sized: every method is a pure function of its arguments. There is no
/// per-caller state to hold, which is what lets a single instance serve every
/// caller without any locking, and what makes a call safe to retry.
#[derive(Debug, Default)]
pub struct VoiceService;

/// Decode a base64 payload, refusing anything over [`MAX_AUDIO_BYTES`].
///
/// The length is checked *before* decoding. A declared size is a number the
/// caller sent us; allocating for it first and validating after is how a wrong
/// or hostile value becomes an allocation failure, which aborts the process
/// rather than returning an error a caller can see.
fn decode_audio(encoded: &str) -> TinyBusResult<Vec<u8>> {
    // Base64 expands by 4/3, so this bounds the decoded size without decoding.
    if encoded.len() / 4 * 3 > MAX_AUDIO_BYTES {
        return Err(tinybus::Error::failed(format!(
            "audio payload exceeds the {MAX_AUDIO_BYTES} byte limit"
        )));
    }
    BASE64
        .decode(encoded)
        .map_err(|e| tinybus::Error::failed(format!("audio payload is not valid base64: {e}")))
}

/// Reinterpret a little-endian byte buffer as `f32` samples.
fn f32_samples(bytes: &[u8]) -> TinyBusResult<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return Err(tinybus::Error::failed(format!(
            "{} bytes is not a whole number of f32 samples",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Map a `tinyvoice` error onto the wire.
fn failed(error: &tinyvoice::Error) -> tinybus::Error {
    tinybus::Error::failed(error.to_string())
}

/// Serialise a value that is known to be representable as JSON.
///
/// The inputs are this crate's own plain data types, so a failure here would be
/// a bug in the type rather than anything a caller did — but it is still
/// reported rather than unwrapped, because a module that panics takes the
/// host's address space with it.
fn to_json<T: serde::Serialize>(value: &T) -> TinyBusResult<String> {
    serde_json::to_string(value)
        .map_err(|e| tinybus::Error::failed(format!("could not encode result: {e}")))
}

// Every operation underneath is synchronous — this crate exists to move pure
// functions across a bus, not to do I/O. `#[tinybus::interface]` still requires
// `async fn` signatures because dispatch awaits them, so the methods are async
// without awaiting anything. Making them genuinely async would mean inventing
// work for them to wait on.
#[allow(clippy::unused_async)]
#[tinybus::interface(name = "ai.tinyhumans.tinyvoice.Voice")]
impl VoiceService {
    /// Classify a command transcript, returning a JSON `VoiceIntent`.
    ///
    /// The transcript should already have had any wake word removed by
    /// [`Self::extract_command`].
    async fn route(&self, transcript: String) -> TinyBusResult<String> {
        to_json(&intent::route(&transcript))
    }

    /// Apply the wake-word gate, returning the command that followed it.
    ///
    /// An empty string means the utterance was not addressed to the agent, or
    /// the wake word arrived with no command after it. Those are the same
    /// outcome for a caller — there is nothing to route either way — so they
    /// share one representation rather than needing a nullable wire type.
    async fn extract_command(
        &self,
        transcript: String,
        wake_word: String,
    ) -> TinyBusResult<String> {
        Ok(intent::extract_command(&transcript, &wake_word).unwrap_or_default())
    }

    /// Whether the wake word appears near the start of a transcript.
    async fn wake_word_present(
        &self,
        transcript: String,
        wake_word: String,
    ) -> TinyBusResult<bool> {
        Ok(intent::wake_word_present(&transcript, &wake_word))
    }

    /// Whether an STT transcript looks like a hallucination.
    ///
    /// `mode` is `"dictation"` or `"conversation"`. An unrecognised value is an
    /// error rather than a silent fallback: the two modes disagree about
    /// whether a bare "okay" is real speech, so guessing would either delete
    /// legitimate chat replies or leak artefacts into dictated text.
    async fn is_hallucinated(&self, text: String, mode: String) -> TinyBusResult<bool> {
        let mode = match mode.as_str() {
            "dictation" => Mode::Dictation,
            "conversation" => Mode::Conversation,
            other => {
                return Err(tinybus::Error::failed(format!(
                    "unknown hallucination mode `{other}`, expected `dictation` or `conversation`"
                )));
            }
        };
        Ok(transcript::is_hallucinated(&text, mode))
    }

    /// Replay a batch of frame energies through a fresh VAD segmenter.
    ///
    /// `config` is a JSON `VadConfig`. Returns a JSON array of `VadEvent`,
    /// each tagged with the index of the frame that produced it.
    ///
    /// The segmenter starts idle on every call and is dropped afterwards, so a
    /// caller streaming a long recording must submit whole utterances rather
    /// than arbitrary slices — a batch that cuts an utterance in half loses the
    /// open segment. That is the cost of a stateless interface, and it is the
    /// reason a realtime host should link the library instead.
    async fn segment(
        &self,
        config: String,
        frame_ms: u32,
        energies: Vec<f32>,
    ) -> TinyBusResult<String> {
        let config: VadConfig = serde_json::from_str(&config)
            .map_err(|e| tinybus::Error::failed(format!("invalid VAD config: {e}")))?;
        if frame_ms == 0 {
            return Err(tinybus::Error::failed(
                "frame_ms must be greater than zero".to_string(),
            ));
        }

        let mut segmenter = VadSegmenter::new(config);
        let events: Vec<IndexedEvent> = energies
            .into_iter()
            .enumerate()
            .filter_map(|(frame, rms)| {
                segmenter
                    .push_frame(rms, frame_ms)
                    .map(|event| IndexedEvent { frame, event })
            })
            .collect();
        to_json(&events)
    }

    /// Wrap base64 little-endian `f32` mono samples in a 16-bit PCM WAV file.
    ///
    /// Returns the base64 WAV bytes.
    async fn encode_wav(&self, samples: String, sample_rate: u32) -> TinyBusResult<String> {
        let samples = f32_samples(&decode_audio(&samples)?)?;
        let wav = audio::f32_mono_to_wav(&samples, sample_rate).map_err(|e| failed(&e))?;
        if wav.len() > MAX_AUDIO_BYTES {
            return Err(tinybus::Error::failed(format!(
                "encoded WAV exceeds the {MAX_AUDIO_BYTES} byte limit"
            )));
        }
        Ok(BASE64.encode(wav))
    }

    /// Downmix, resample, and silence-gate a captured buffer, returning WAV.
    ///
    /// This is the whole capture-side pipeline in one call, because a host that
    /// made three round trips would ship the same audio across the bus three
    /// times to do work that is a few microseconds of arithmetic.
    ///
    /// `gate_threshold` of zero disables the silence gate.
    async fn prepare_capture(
        &self,
        samples: String,
        source_rate: u32,
        channels: u16,
        gate_threshold: f32,
    ) -> TinyBusResult<String> {
        let raw = f32_samples(&decode_audio(&samples)?)?;
        let mono = audio::to_mono(&raw, channels).map_err(|e| failed(&e))?;
        let resampled =
            audio::resample(&mono, source_rate, audio::STT_SAMPLE_RATE).map_err(|e| failed(&e))?;

        let gated = if gate_threshold > 0.0 {
            let config = SilenceGateConfig {
                threshold: gate_threshold,
                ..SilenceGateConfig::default()
            };
            let mut gate =
                audio::SilenceGate::new(config, audio::STT_SAMPLE_RATE).map_err(|e| failed(&e))?;
            // One block: the gate is a streaming filter, but a caller handing
            // over a finished recording has no blocks to give it.
            gate.push(&resampled)
        } else {
            resampled
        };

        let wav = audio::f32_mono_to_wav(&gated, audio::STT_SAMPLE_RATE).map_err(|e| failed(&e))?;
        if wav.len() > MAX_AUDIO_BYTES {
            return Err(tinybus::Error::failed(format!(
                "encoded WAV exceeds the {MAX_AUDIO_BYTES} byte limit"
            )));
        }
        Ok(BASE64.encode(wav))
    }
}

/// A [`VadEvent`] paired with the frame that produced it.
///
/// The index is what makes a batch reply actionable: a host needs to know
/// *where* an utterance ended in order to cut the audio there, and a bare event
/// list would only say that it did.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct IndexedEvent {
    /// Zero-based index into the submitted energies.
    frame: usize,
    /// What the segmenter reported at that frame.
    #[serde(flatten)]
    event: VadEvent,
}

/// Claim the bus name and serve the interface.
async fn setup(connection: Connection) -> TinyBusResult<()> {
    connection
        .serve_at(OBJECT_PATH.try_into()?, VoiceService)
        .await?;
    connection.request_name(BUS_NAME).await?;
    Ok(())
}

tinybus_module::module_export! {
    setup = setup,
    worker_threads = 1,
    provides = ["ai.tinyhumans.tinyvoice.Voice"],
    methods = [
        "Route",
        "ExtractCommand",
        "WakeWordPresent",
        "IsHallucinated",
        "Segment",
        "EncodeWav",
        "PrepareCapture",
    ],
    signals = [],
    requires = [],
    optional = [],
    lazy = false,
}
