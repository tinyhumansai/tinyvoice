//! The crate-wide error type.

#[cfg(test)]
mod test;

/// Why a `tinyvoice` operation could not produce a result.
///
/// Deliberately small. Most of this crate is total — a silence gate or a VAD
/// frame cannot fail — so the variants here cover only the places where an
/// input can genuinely be out of contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A sample rate of zero was supplied.
    ///
    /// Every duration in this crate is derived by dividing by the sample rate,
    /// so zero is rejected at the boundary rather than producing a division by
    /// zero or a silently absurd millisecond count deeper in.
    #[error("sample rate must be greater than zero")]
    ZeroSampleRate,

    /// A channel count of zero was supplied to a downmix or WAV writer.
    #[error("channel count must be greater than zero")]
    ZeroChannels,

    /// Interleaved input was not a whole number of frames for its channel
    /// count.
    #[error("{samples} samples is not a whole number of {channels}-channel frames")]
    RaggedFrames {
        /// How many samples were supplied.
        samples: usize,
        /// How many channels they were said to be interleaved across.
        channels: u16,
    },
}

/// The result type returned by every fallible function in this crate.
pub type Result<T> = core::result::Result<T, Error>;
