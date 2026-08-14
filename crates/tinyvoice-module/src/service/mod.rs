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

/// Most sessions a caller may hold open at once.
///
/// A session is opened by `VadOpen` and only released by `VadClose`, and a
/// method receives no caller identity — so there is nobody to attribute a leak
/// to and nothing to clean up when a caller goes away. The cap is what stops a
/// host that forgets to close from growing this map without bound; it refuses
/// the *new* session rather than evicting an old one, because evicting would
/// silently truncate an utterance somebody is still recording.
///
/// Generous relative to real use: a host runs one always-on loop.
pub const MAX_SESSIONS: usize = 64;

/// The served object.
///
/// Almost every method is a pure function of its arguments. The exception is
/// the VAD, which is a state machine over successive frames and therefore needs
/// somewhere to live between calls — hence its one field.
///
/// # Why the VAD gets a session and nothing else does
///
/// A stateless `Segment` exists too, and it is the right call for a recording
/// that is already complete. It cannot serve a live capture loop: a batch that
/// cuts an utterance in half loses the open segment, and a loop that re-sent
/// everything each time would do quadratic work to avoid holding one enum.
///
/// The state here is deliberately tiny — a `VadSegmenter` is two `u32`s and a
/// config — so a session costs a map entry, not a buffer. The audio itself
/// stays with the host, which is already accumulating it.
#[derive(Debug, Default)]
pub struct VoiceService {
    /// Live segmenters, keyed by the id `VadOpen` handed out.
    ///
    /// A `std::sync::Mutex` rather than an async one: every critical section is
    /// a map lookup and a few integer comparisons, with no await inside, so an
    /// async mutex would add a scheduling hop to a lock that is never contended
    /// for long. It is deliberately never held across an `.await`.
    sessions: std::sync::Mutex<Sessions>,
}

/// The session table and the counter that names its entries.
#[derive(Debug, Default)]
struct Sessions {
    /// Next id to hand out. Monotonic, never reused.
    ///
    /// Reusing a freed id would let a stale handle from one utterance land on
    /// the segmenter of the next, which is the kind of bug that shows up as a
    /// transcript with somebody else's sentence stapled to the front.
    next_id: u64,
    /// Open segmenters.
    live: std::collections::HashMap<u64, VadSegmenter>,
}

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

/// Base64 little-endian `f32`, the shape samples take on this interface.
fn encode_samples(samples: &[f32]) -> String {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    BASE64.encode(bytes)
}

/// The error for a session id that is not open.
///
/// Its own function so the wording is identical across the four methods that
/// can produce it — a host matching on the message should not have to care
/// which call it came from.
fn unknown_session(session: u64) -> tinybus::Error {
    tinybus::Error::failed(format!(
        "VAD session {session} is not open; it was closed, or never opened in this process"
    ))
}

impl VoiceService {
    /// Take the session lock, turning a poisoned mutex into a wire error.
    ///
    /// A poisoned lock means a previous call panicked while holding it. The
    /// panic was already caught by tinybus rather than taking the host down,
    /// but the map may be inconsistent, so this refuses rather than reaching
    /// into it — and says so, because "poisoned" is a fact worth seeing in a
    /// log rather than a generic failure.
    fn lock_sessions(&self) -> TinyBusResult<std::sync::MutexGuard<'_, Sessions>> {
        self.sessions.lock().map_err(|_| {
            tinybus::Error::failed(
                "the VAD session table is poisoned by an earlier panic; restart the host"
                    .to_string(),
            )
        })
    }
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

    /// Open a VAD session and return its id.
    ///
    /// `config` is a JSON `VadConfig`. The caller must `VadClose` the id when
    /// the capture loop stops; see [`MAX_SESSIONS`] for what happens if it
    /// does not.
    async fn vad_open(&self, config: String) -> TinyBusResult<u64> {
        let config: VadConfig = serde_json::from_str(&config)
            .map_err(|e| tinybus::Error::failed(format!("invalid VAD config: {e}")))?;

        let mut sessions = self.lock_sessions()?;
        if sessions.live.len() >= MAX_SESSIONS {
            return Err(tinybus::Error::failed(format!(
                "at the {MAX_SESSIONS} open VAD session limit; close a session before opening another"
            )));
        }
        let id = sessions.next_id;
        sessions.next_id += 1;
        sessions.live.insert(id, VadSegmenter::new(config));
        Ok(id)
    }

    /// Push frame energies into an open session.
    ///
    /// Returns a JSON array of `VadEvent`, each tagged with the index of the
    /// frame *within this call* that produced it — not a running total, which
    /// the module would have to track and the caller already knows.
    async fn vad_push(
        &self,
        session: u64,
        frame_ms: u32,
        energies: Vec<f32>,
    ) -> TinyBusResult<String> {
        if frame_ms == 0 {
            return Err(tinybus::Error::failed(
                "frame_ms must be greater than zero".to_string(),
            ));
        }

        let events = {
            let mut sessions = self.lock_sessions()?;
            let segmenter = sessions
                .live
                .get_mut(&session)
                .ok_or_else(|| unknown_session(session))?;
            energies
                .into_iter()
                .enumerate()
                .filter_map(|(frame, rms)| {
                    segmenter
                        .push_frame(rms, frame_ms)
                        .map(|event| IndexedEvent { frame, event })
                })
                .collect::<Vec<_>>()
        };
        to_json(&events)
    }

    /// Whether an open session is currently inside an utterance.
    async fn vad_is_speaking(&self, session: u64) -> TinyBusResult<bool> {
        let sessions = self.lock_sessions()?;
        sessions
            .live
            .get(&session)
            .map(VadSegmenter::is_speaking)
            .ok_or_else(|| unknown_session(session))
    }

    /// Abort any in-flight utterance without emitting an event.
    ///
    /// This is the privacy hook: a host calls it when the screen locks or
    /// capture is revoked, and the partial utterance is dropped rather than
    /// completed and transcribed.
    async fn vad_reset(&self, session: u64) -> TinyBusResult<()> {
        let mut sessions = self.lock_sessions()?;
        sessions
            .live
            .get_mut(&session)
            .map(VadSegmenter::reset)
            .ok_or_else(|| unknown_session(session))
    }

    /// Release a session.
    ///
    /// Closing an id that is already gone is **not** an error. A host tearing
    /// down after a failure should not have to track whether it got far enough
    /// to open one, and turning cleanup into something that can itself fail
    /// invites the leak this is meant to prevent.
    async fn vad_close(&self, session: u64) -> TinyBusResult<()> {
        self.lock_sessions()?.live.remove(&session);
        Ok(())
    }

    /// Downmix and resample a captured buffer, returning `f32` mono samples.
    ///
    /// The sibling of `PrepareCapture` for callers that need samples rather
    /// than a container — a live loop measuring energy per frame and
    /// accumulating an utterance, rather than one finishing a recording.
    ///
    /// Exists so this work can leave a host's audio callback: `cpal` delivers
    /// on a realtime thread where the correct amount of work is as little as
    /// possible, and a host that forwards raw interleaved samples to its own
    /// worker does strictly less there than one that downmixes in place.
    async fn prepare_frames(
        &self,
        samples: String,
        source_rate: u32,
        channels: u16,
    ) -> TinyBusResult<String> {
        let raw = f32_samples(&decode_audio(&samples)?)?;
        let mono = audio::to_mono(&raw, channels).map_err(|e| failed(&e))?;
        let resampled =
            audio::resample(&mono, source_rate, audio::STT_SAMPLE_RATE).map_err(|e| failed(&e))?;
        Ok(encode_samples(&resampled))
    }

    /// Root-mean-square energy of each fixed-size frame in a buffer.
    ///
    /// Paired with `VadPush`: a host slices its resampled audio into frames and
    /// needs one number per frame. Returning them together keeps the framing
    /// rule — including what happens to a short trailing frame — in one place
    /// rather than reimplemented by every caller.
    ///
    /// A trailing partial frame is measured as-is rather than dropped or padded.
    /// Dropping it would lose the end of an utterance; padding with silence
    /// would dilute its energy and could turn speech into a below-threshold
    /// frame.
    async fn frame_energies(&self, samples: String, frame_len: u32) -> TinyBusResult<Vec<f32>> {
        if frame_len == 0 {
            return Err(tinybus::Error::failed(
                "frame_len must be greater than zero".to_string(),
            ));
        }
        let samples = f32_samples(&decode_audio(&samples)?)?;
        Ok(samples.chunks(frame_len as usize).map(audio::rms).collect())
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
        .serve_at(OBJECT_PATH.try_into()?, VoiceService::default())
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
        "VadOpen",
        "VadPush",
        "VadIsSpeaking",
        "VadReset",
        "VadClose",
        "PrepareFrames",
        "FrameEnergies",
        "EncodeWav",
        "PrepareCapture",
    ],
    signals = [],
    requires = [],
    optional = [],
    lazy = false,
}
