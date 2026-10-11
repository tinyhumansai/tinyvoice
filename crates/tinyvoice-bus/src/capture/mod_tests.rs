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

#[test]
fn continuous_format_and_batches_preserve_native_samples() -> Result<(), serde_json::Error> {
    let stream = CaptureStream {
        handle: CaptureHandle("lease".into()),
        format: CaptureFormat {
            source_rate: 48_000,
            channels: 2,
        },
    };
    let wire = serde_json::to_value(&stream)?;
    let decoded: CaptureStream = serde_json::from_value(wire)?;
    assert_eq!(decoded.format, stream.format);
    let batch = CaptureBatch {
        chunks: vec![RawChunk {
            samples: vec![0.25, -0.25],
        }],
        closed: false,
    };
    let decoded: CaptureBatch = serde_json::from_value(serde_json::to_value(&batch)?)?;
    assert_eq!(decoded.chunks, batch.chunks);
    assert!(!decoded.closed);
    Ok(())
}
