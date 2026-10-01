//! Choosing an input configuration.

use cpal::{SampleRate, SupportedStreamConfig, SupportedStreamConfigRange};

use super::TARGET_SAMPLE_RATE;

/// Find the best input config: prefer one whose range includes 16 kHz, then
/// fewer channels; if none includes 16 kHz, take the best one's maximum rate
/// (it is resampled later).
///
/// # Errors
///
/// Returns a message when the device offers no configuration at all.
pub(crate) fn find_best_config(
    configs: impl Iterator<Item = SupportedStreamConfigRange>,
) -> Result<SupportedStreamConfig, String> {
    let mut configs_vec: Vec<SupportedStreamConfigRange> = configs.collect();
    if configs_vec.is_empty() {
        return Err("no supported audio input configurations found".to_string());
    }

    let has_target = |range: &SupportedStreamConfigRange| {
        range.min_sample_rate().0 <= TARGET_SAMPLE_RATE
            && range.max_sample_rate().0 >= TARGET_SAMPLE_RATE
    };

    // Sort: prefer configs whose range includes 16kHz, then by fewer channels.
    configs_vec.sort_by(|a, b| {
        has_target(b)
            .cmp(&has_target(a))
            .then(a.channels().cmp(&b.channels()))
    });

    let best = &configs_vec[0];
    let rate = if has_target(best) {
        SampleRate(TARGET_SAMPLE_RATE)
    } else {
        // Use the maximum supported rate and resample later.
        best.max_sample_rate()
    };

    Ok((*best).with_sample_rate(rate))
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
