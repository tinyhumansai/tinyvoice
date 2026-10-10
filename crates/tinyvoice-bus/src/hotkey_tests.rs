use super::*;

#[test]
fn activation_mode_wire_names_remain_compatible() {
    let tap = serde_json::to_string(&ActivationMode::Tap).unwrap_or_default();
    let push = serde_json::to_string(&ActivationMode::Push).unwrap_or_default();
    assert_eq!(tap, "\"tap\"");
    assert_eq!(push, "\"push\"");
}

#[test]
fn activation_mode_defaults_to_push() {
    assert_eq!(ActivationMode::default(), ActivationMode::Push);
}

#[test]
fn host_facts_have_a_generic_stable_wire_shape() {
    let fact = SequencedHostFact {
        sequence: 9,
        fact: HostKeyFact::Down,
    };
    let encoded = serde_json::to_string(&fact).unwrap_or_default();
    assert_eq!(encoded, r#"{"sequence":9,"fact":"down"}"#);
}

#[test]
fn hotkey_errors_never_carry_native_detail() {
    let encoded = serde_json::to_string(&HotkeyError::CleanupFailed).unwrap_or_default();
    assert_eq!(encoded, "\"cleanup_failed\"");
}
