//! Native device I/O bridge. Fixtures exercise the manager through its backend seam.
//! Real device enumeration/open/stop require hardware and permission and use
//! the repository's established device-file coverage exclusion.
use super::{Backend, Recording, RecordingFuture};
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
    fn start(&self) -> Result<Box<dyn Recording>, String> {
        // The manager refuses anything except the host's computer-module grant
        // before entering this backend. No permission decision is made locally.
        tinyvoice::capture::start_recording(|| Ok(()))
            .map(|recording| Box::new(recording) as Box<dyn Recording>)
            .map_err(|error| error.to_string())
    }
}
