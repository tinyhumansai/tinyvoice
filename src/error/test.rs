//! The error type is part of the public surface: hosts match on it, so the
//! variants and their messages are a contract.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use super::Error;

#[test]
fn messages_say_what_was_wrong() {
    assert_eq!(
        Error::ZeroSampleRate.to_string(),
        "sample rate must be greater than zero"
    );
    assert_eq!(
        Error::ZeroChannels.to_string(),
        "channel count must be greater than zero"
    );
    assert_eq!(
        Error::RaggedFrames {
            samples: 3,
            channels: 2
        }
        .to_string(),
        "3 samples is not a whole number of 2-channel frames"
    );
}

#[test]
fn errors_are_comparable_so_tests_and_hosts_can_match_on_them() {
    assert_eq!(Error::ZeroSampleRate, Error::ZeroSampleRate);
    assert_ne!(Error::ZeroSampleRate, Error::ZeroChannels);
    assert_ne!(
        Error::RaggedFrames {
            samples: 3,
            channels: 2
        },
        Error::RaggedFrames {
            samples: 5,
            channels: 2
        }
    );
}
