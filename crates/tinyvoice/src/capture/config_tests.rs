//! Unit tests for input-config selection.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cpal::{SampleFormat, SampleRate, SupportedBufferSize, SupportedStreamConfigRange};

use super::{TARGET_SAMPLE_RATE, find_best_config};

#[test]
fn find_best_config_prefers_target_rate_and_fewer_channels() {
    let configs = vec![
        SupportedStreamConfigRange::new(
            2,
            SampleRate(8_000),
            SampleRate(48_000),
            SupportedBufferSize::Unknown,
            SampleFormat::F32,
        ),
        SupportedStreamConfigRange::new(
            1,
            SampleRate(16_000),
            SampleRate(16_000),
            SupportedBufferSize::Unknown,
            SampleFormat::I16,
        ),
    ];

    let best = find_best_config(configs.into_iter()).expect("best config");
    assert_eq!(best.channels(), 1);
    assert_eq!(best.sample_rate(), SampleRate(TARGET_SAMPLE_RATE));
    assert_eq!(best.sample_format(), SampleFormat::I16);
}

#[test]
fn find_best_config_falls_back_to_max_rate_when_target_missing() {
    let configs = vec![SupportedStreamConfigRange::new(
        1,
        SampleRate(22_050),
        SampleRate(44_100),
        SupportedBufferSize::Unknown,
        SampleFormat::F32,
    )];

    let best = find_best_config(configs.into_iter()).expect("best config");
    assert_eq!(best.sample_rate(), SampleRate(44_100));
}

#[test]
fn find_best_config_errors_when_empty() {
    let err = find_best_config(Vec::<SupportedStreamConfigRange>::new().into_iter())
        .expect_err("empty config list should fail");
    assert!(err.contains("no supported audio input configurations"));
}
