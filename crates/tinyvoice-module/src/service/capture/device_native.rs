//! Native device I/O bridge. Fixtures exercise the manager through its backend seam.
//! Real device enumeration/open/stop require hardware and permission and use
//! the repository's established device-file coverage exclusion.
use super::{Backend, Recording, RecordingFuture, Stream, StreamFuture};
use tinyvoice::capture::RecordingHandle;
impl Recording for RecordingHandle {
    fn finish(self: Box<Self>) -> RecordingFuture {
        Box::pin(async move { self.stop().await.map_err(|error| error.to_string()) })
    }
}
#[derive(Debug)]
pub(super) struct Native;
impl Backend for Native {
    fn devices(&self) -> Result<Vec<String>, String> {
        tinyvoice::capture::list_input_devices().map_err(|error| error.to_string())
    }
    fn stream(&self) -> Result<(tinyvoice_bus::capture::CaptureFormat, Box<dyn Stream>), String> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let (format, handle) = tinyvoice::capture::start_capture_stream(tx, || Ok(()))
            .map_err(|error| error.to_string())?;
        Ok((format, Box::new(NativeStream { rx, handle })))
    }
    fn start(&self) -> Result<Box<dyn Recording>, String> {
        // The manager refuses anything except the host's computer-module grant
        // before entering this backend. No permission decision is made locally.
        tinyvoice::capture::start_recording(|| Ok(()))
            .map(|recording| Box::new(recording) as Box<dyn Recording>)
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug)]
struct NativeStream {
    rx: tokio::sync::mpsc::Receiver<tinyvoice::capture::RawChunk>,
    handle: tinyvoice::capture::CaptureStreamHandle,
}
impl Stream for NativeStream {
    fn poll(
        &mut self,
        max_chunks: usize,
    ) -> tinyvoice_bus::capture::CaptureResult<tinyvoice_bus::capture::CaptureBatch> {
        Ok(super::poll_chunks(&mut self.rx, max_chunks))
    }
    fn stop(self: Box<Self>) -> StreamFuture {
        Box::pin(async move { self.handle.stop().await.map_err(|error| error.to_string()) })
    }
}
