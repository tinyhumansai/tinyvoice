//! Known reservations and cancellation-safe native startup workers.
use super::{
    Arc, AtomicBool, BusyGuard, Capture, CaptureError, CaptureHandle, CaptureResult, CaptureStream,
    Lease, MAX_RESERVATIONS, MicrophonePermission, Ordering, PendingStart, RESERVATION_TTL,
    RecordingStartRequest, Reservation, Started, StreamLease, handle, state_error,
};

#[derive(Debug)]
enum NativeResource {
    Recording(Box<dyn super::Recording>),
    Stream(
        tinyvoice_bus::capture::CaptureFormat,
        Box<dyn super::Stream>,
    ),
}
impl NativeResource {
    fn discard(self, runtime: &tokio::runtime::Handle) {
        match self {
            Self::Recording(recording) => {
                let _ = runtime.block_on(recording.finish());
            }
            Self::Stream(_, stream) => {
                let _ = runtime.block_on(stream.stop());
            }
        }
    }
    fn publish(
        self,
        capture: &Capture,
        handle: &CaptureHandle,
        busy: BusyGuard,
    ) -> Result<Started, (Self, BusyGuard)> {
        match self {
            Self::Recording(recording) => {
                let Ok(mut recordings) = capture.recordings.lock() else {
                    return Err((Self::Recording(recording), busy));
                };
                recordings.insert(handle.clone(), Lease { recording, busy });
                Ok(Started::Recording(handle.clone()))
            }
            Self::Stream(format, stream) => {
                let Ok(mut streams) = capture.streams.lock() else {
                    return Err((Self::Stream(format, stream), busy));
                };
                streams.insert(handle.clone(), StreamLease { stream, busy });
                Ok(Started::Stream(CaptureStream {
                    handle: handle.clone(),
                    format,
                }))
            }
        }
    }
}

impl Capture {
    pub(in crate::service) fn reserve(
        &self,
        permission: MicrophonePermission,
    ) -> CaptureResult<CaptureHandle> {
        if permission != MicrophonePermission::Granted {
            return Err(CaptureError::PermissionDenied);
        }
        let mut reservations = self.reservations.lock().map_err(|_| state_error())?;
        if self.closed.load(Ordering::SeqCst) {
            return Err(CaptureError::Closed);
        }
        reservations.retain(|_, reservation| {
            reservation.pending.is_some() || reservation.created.elapsed() < RESERVATION_TTL
        });
        if reservations.len() >= MAX_RESERVATIONS {
            return Err(CaptureError::LimitExceeded);
        }
        let handle = handle()?;
        reservations.insert(
            handle.clone(),
            Reservation {
                created: std::time::Instant::now(),
                pending: None,
            },
        );
        Ok(handle)
    }
    pub(in crate::service) async fn start_reserved(
        self: &Arc<Self>,
        request: RecordingStartRequest,
        stream: bool,
    ) -> CaptureResult<Started> {
        if request.permission != MicrophonePermission::Granted {
            return Err(CaptureError::PermissionDenied);
        }
        let handle = request.handle.ok_or(CaptureError::InvalidParameters)?;
        let (pending, busy) = {
            let mut reservations = self.reservations.lock().map_err(|_| state_error())?;
            if self.closed.load(Ordering::SeqCst) {
                return Err(CaptureError::Closed);
            }
            let reservation = reservations
                .get_mut(&handle)
                .ok_or(CaptureError::UnknownHandle)?;
            if reservation.pending.is_some() {
                return Err(CaptureError::Busy);
            }
            if reservation.created.elapsed() >= RESERVATION_TTL {
                reservations.remove(&handle);
                return Err(CaptureError::UnknownHandle);
            }
            self.busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .map_err(|_| CaptureError::Busy)?;
            self.idle.send_replace(false);
            let busy = BusyGuard(self.busy.clone(), self.idle.clone());
            let (completed, _) = tokio::sync::watch::channel(false);
            let pending = Arc::new(PendingStart {
                canceled: AtomicBool::new(false),
                completed,
            });
            reservation.pending = Some(pending.clone());
            (pending, busy)
        };
        let capture = self.clone();
        let (reply, received) = tokio::sync::oneshot::channel();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                capture.run_start(&handle, &pending, busy, stream, &runtime, reply);
            }));
            // Notification follows all cleanup, even failed or abandoned setup.
            if let Ok(mut reservations) = capture.reservations.lock() {
                reservations.remove(&handle);
            }
            pending.completed.send_replace(true);
        });
        received.await.map_err(|_| state_error())?
    }
    fn run_start(
        &self,
        handle: &CaptureHandle,
        pending: &PendingStart,
        busy: BusyGuard,
        stream: bool,
        runtime: &tokio::runtime::Handle,
        reply: tokio::sync::oneshot::Sender<CaptureResult<Started>>,
    ) {
        let result = if stream {
            self.backend
                .stream()
                .map(|(format, resource)| NativeResource::Stream(format, resource))
        } else {
            self.backend.start().map(NativeResource::Recording)
        };
        let result = result.map_err(CaptureError::Device).and_then(|resource| {
            let Ok(mut reservations) = self.reservations.lock() else {
                resource.discard(runtime);
                return Err(state_error());
            };
            if pending.canceled.load(Ordering::SeqCst) || reply.is_closed() {
                drop(reservations);
                resource.discard(runtime);
                return Err(CaptureError::Cancelled);
            }
            match resource.publish(self, handle, busy) {
                Ok(started) => {
                    reservations.remove(handle);
                    Ok(started)
                }
                Err((resource, busy)) => {
                    drop(reservations);
                    resource.discard(runtime);
                    drop(busy);
                    Err(state_error())
                }
            }
        });
        if let Err(Ok(started)) = reply.send(result) {
            // The result receiver vanished after publication. Reclaim the live resource.
            self.discard_live(started, runtime);
        }
    }
    fn discard_live(&self, started: Started, runtime: &tokio::runtime::Handle) {
        match started {
            Started::Recording(handle) => {
                let lease = self
                    .recordings
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&handle);
                if let Some(Lease { recording, busy }) = lease {
                    let _ = runtime.block_on(recording.finish());
                    drop(busy);
                }
            }
            Started::Stream(stream) => {
                let lease = self
                    .streams
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&stream.handle);
                if let Some(StreamLease { stream, busy }) = lease {
                    let _ = runtime.block_on(stream.stop());
                    drop(busy);
                }
            }
        }
    }
    pub(super) async fn cancel_pending(&self, handle: &CaptureHandle) -> CaptureResult<bool> {
        let pending = {
            let mut reservations = self.reservations.lock().map_err(|_| state_error())?;
            let Some(reservation) = reservations.get(handle) else {
                return Ok(false);
            };
            if let Some(pending) = &reservation.pending {
                pending.canceled.store(true, Ordering::SeqCst);
                Some(pending.clone())
            } else {
                reservations.remove(handle);
                None
            }
        };
        if let Some(pending) = pending {
            let mut completed = pending.completed.subscribe();
            while !*completed.borrow_and_update() {
                completed.changed().await.map_err(|_| state_error())?;
            }
            // A cancel may arrive between the worker's check and publication.
            // Fall through to reclaim a published recording/stream if present.
            return Ok(true);
        }
        Ok(true)
    }
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
