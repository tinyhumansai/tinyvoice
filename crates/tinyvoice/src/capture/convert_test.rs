//! Unit tests for sample-format conversion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{i16_to_f32, u16_to_f32};

#[test]
fn i16_to_f32_normalises_to_unit_range() {
    assert_eq!(i16_to_f32(&[0]), vec![0.0]);
    assert_eq!(i16_to_f32(&[16384]), vec![0.5]);
    assert_eq!(i16_to_f32(&[-32768]), vec![-1.0]);
    // Interleaved frames are converted element-wise, order preserved.
    assert_eq!(i16_to_f32(&[0, 16384, -16384]), vec![0.0, 0.5, -0.5]);
}

#[test]
fn u16_to_f32_centers_on_midscale() {
    // Unsigned PCM is offset-binary: 32768 is silence (0.0), the endpoints map
    // to the extremes.
    assert_eq!(u16_to_f32(&[32768]), vec![0.0]);
    assert_eq!(u16_to_f32(&[49152]), vec![0.5]);
    assert_eq!(u16_to_f32(&[0]), vec![-1.0]);
    assert_eq!(u16_to_f32(&[32768, 49152, 16384]), vec![0.0, 0.5, -0.5]);
}

#[test]
fn the_offset_binary_form_matches_the_shifted_form_for_every_value() {
    // The always-on path used `s / 32768 - 1` and the recorder used
    // `(s - 32768) / 32768`. Both are exact in f32, so unifying on one changes
    // no sample; this proves it over the whole domain.
    let all: Vec<u16> = (0..=u16::MAX).collect();
    let shifted = u16_to_f32(&all);
    for (s, got) in all.iter().zip(shifted) {
        assert_eq!(got, f32::from(*s) / 32768.0 - 1.0);
    }
}
