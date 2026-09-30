//! Sample-format conversion, the one transform that has to stay in the audio
//! callback.

/// Convert interleaved signed `i16` PCM samples to normalised `f32` in
/// `[-1.0, 1.0)` (`s / 32768`).
pub(crate) fn i16_to_f32(data: &[i16]) -> Vec<f32> {
    data.iter().map(|&s| f32::from(s) / 32768.0).collect()
}

/// Convert interleaved unsigned `u16` PCM samples (mid-scale 32768) to
/// normalised `f32` in `[-1.0, 1.0)` (`(s - 32768) / 32768`).
pub(crate) fn u16_to_f32(data: &[u16]) -> Vec<f32> {
    data.iter()
        .map(|&s| (f32::from(s) - 32768.0) / 32768.0)
        .collect()
}

#[cfg(test)]
#[path = "convert_test.rs"]
mod tests;
