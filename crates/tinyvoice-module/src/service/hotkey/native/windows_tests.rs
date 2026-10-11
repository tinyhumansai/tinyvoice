//! Pure key-map checks for the Windows native adapter.
use super::virtual_key;
use tinyvoice::hotkey::Key;

#[test]
fn left_and_right_modifier_keys_keep_distinct_virtual_codes() {
    assert_eq!(virtual_key(Key::ControlLeft), Some(0xA2));
    assert_eq!(virtual_key(Key::ControlRight), Some(0xA3));
    assert_eq!(virtual_key(Key::ShiftLeft), Some(0xA0));
    assert_eq!(virtual_key(Key::ShiftRight), Some(0xA1));
    assert_eq!(virtual_key(Key::Alt), Some(0xA4));
    assert_eq!(virtual_key(Key::AltGr), Some(0xA5));
}

#[test]
fn printable_and_keypad_keys_map_to_their_windows_codes() {
    assert_eq!(virtual_key(Key::KeyA), Some(u32::from(b'A')));
    assert_eq!(virtual_key(Key::Num0), Some(0x30));
    assert_eq!(virtual_key(Key::Kp0), Some(0x60));
    assert_eq!(virtual_key(Key::Return), Some(0x0D));
    assert_eq!(virtual_key(Key::KpReturn), Some(0x0D));
}

#[test]
fn function_keys_without_a_windows_code_are_rejected() {
    assert_eq!(virtual_key(Key::Function), None);
}
