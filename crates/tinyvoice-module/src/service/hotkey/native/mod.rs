//! Platform owner boundary for cancellable native listeners.
use std::sync::{Arc, atomic::AtomicBool, mpsc};
use tinyvoice_bus::{HotkeyError, HotkeyRequest};

#[derive(Debug)]
pub(super) struct NativeListener {
    pub events: mpsc::Receiver<bool>,
    pub overflow: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
    owner: Box<dyn NativeOwner>,
}

pub(super) trait NativeOwner: std::fmt::Debug + Send {
    fn stop(&mut self) -> Result<(), HotkeyError>;
}

pub(super) trait Backend: std::fmt::Debug + Send + Sync {
    fn start(&self, request: &HotkeyRequest) -> Result<NativeListener, HotkeyError>;
}

impl NativeListener {
    pub(super) fn new(
        events: mpsc::Receiver<bool>,
        overflow: Arc<AtomicBool>,
        closing: Arc<AtomicBool>,
        owner: Box<dyn NativeOwner>,
    ) -> Self {
        Self {
            events,
            overflow,
            closing,
            owner,
        }
    }
}

impl NativeListener {
    pub(super) fn stop(&mut self) -> Result<(), HotkeyError> {
        self.closing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.owner.stop()
    }
}

impl Drop for NativeListener {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Debug, Default)]
struct PlatformBackend;

impl Backend for PlatformBackend {
    fn start(&self, request: &HotkeyRequest) -> Result<NativeListener, HotkeyError> {
        start_platform(request)
    }
}

pub(super) fn backend() -> Arc<dyn Backend> {
    Arc::new(PlatformBackend)
}

fn start_platform(request: &HotkeyRequest) -> Result<NativeListener, HotkeyError> {
    #[cfg(target_os = "linux")]
    {
        linux::start(request)
    }
    #[cfg(target_os = "windows")]
    {
        windows::start(request)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = request;
        Err(HotkeyError::Unsupported)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = request;
        Err(HotkeyError::Unsupported)
    }
}
