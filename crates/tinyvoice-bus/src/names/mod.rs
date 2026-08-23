//! The `TinyVoice` module's bus identity and member names.

/// The well-known interface name the module claims on the bus.
pub const BUS_NAME: &str = "ai.tinyhumans.tinyvoice.Voice";

/// The object path the module serves its interface at.
pub const OBJECT_PATH: &str = "/ai/tinyhumans/tinyvoice/Voice";

/// One constant per member of [`BUS_NAME`].
pub mod methods {
    /// Routes a wake-word-stripped transcript.
    pub const ROUTE: &str = "Route";
    /// Removes a leading wake word from a transcript.
    pub const EXTRACT_COMMAND: &str = "ExtractCommand";
    /// Detects whether a transcript contains its wake word.
    pub const WAKE_WORD_PRESENT: &str = "WakeWordPresent";
    /// Detects likely STT hallucinations.
    pub const IS_HALLUCINATED: &str = "IsHallucinated";
    /// Segments a complete batch of frame energies.
    pub const SEGMENT: &str = "Segment";
    /// Opens a stateful VAD session.
    pub const VAD_OPEN: &str = "VadOpen";
    /// Adds frame energies to a VAD session.
    pub const VAD_PUSH: &str = "VadPush";
    /// Reports whether a VAD session is inside an utterance.
    pub const VAD_IS_SPEAKING: &str = "VadIsSpeaking";
    /// Drops the partial utterance in a VAD session.
    pub const VAD_RESET: &str = "VadReset";
    /// Closes a VAD session.
    pub const VAD_CLOSE: &str = "VadClose";
    /// Downmixes and resamples samples for frame-based processing.
    pub const PREPARE_FRAMES: &str = "PrepareFrames";
    /// Calculates RMS values for a sample buffer's frames.
    pub const FRAME_ENERGIES: &str = "FrameEnergies";
    /// Encodes f32 mono samples as a WAV file.
    pub const ENCODE_WAV: &str = "EncodeWav";
    /// Encodes PCM16 samples as a WAV file without a lossy conversion.
    pub const ENCODE_WAV_PCM16: &str = "EncodeWavPcm16";
    /// Runs the capture preparation pipeline and produces a WAV file.
    pub const PREPARE_CAPTURE: &str = "PrepareCapture";
}

/// Every member of [`BUS_NAME`], in the interface's sorted dispatch order.
pub const METHODS: &[&str] = &[
    methods::ENCODE_WAV,
    methods::ENCODE_WAV_PCM16,
    methods::EXTRACT_COMMAND,
    methods::FRAME_ENERGIES,
    methods::IS_HALLUCINATED,
    methods::PREPARE_CAPTURE,
    methods::PREPARE_FRAMES,
    methods::ROUTE,
    methods::SEGMENT,
    methods::VAD_CLOSE,
    methods::VAD_IS_SPEAKING,
    methods::VAD_OPEN,
    methods::VAD_PUSH,
    methods::VAD_RESET,
    methods::WAKE_WORD_PRESENT,
];
