//! Capture vocabulary preserves explicit permission and opaque lease shapes.
use super::*;
#[test]
fn permission_defaults_to_denial_and_handle_is_opaque() -> Result<(), serde_json::Error> {
    assert_eq!(
        MicrophonePermission::default(),
        MicrophonePermission::Denied
    );
    assert_eq!(
        serde_json::to_value(MicrophonePermission::Granted)?,
        serde_json::json!("granted")
    );
    let handle = CaptureHandle("opaque".into());
    assert_eq!(serde_json::to_value(&handle)?, serde_json::json!("opaque"));
    assert_eq!(
        serde_json::from_value::<CaptureHandle>(serde_json::json!("opaque"))?,
        handle
    );
    Ok(())
}
#[test]
fn native_faults_are_structured_product_results() -> Result<(), serde_json::Error> {
    let fault = CaptureError::Device("fixture".into());
    let wire = serde_json::to_value(&fault)?;
    assert_eq!(
        wire,
        serde_json::json!({"code":"device","detail":"fixture"})
    );
    assert_eq!(serde_json::from_value::<CaptureError>(wire)?, fault);
    Ok(())
}
