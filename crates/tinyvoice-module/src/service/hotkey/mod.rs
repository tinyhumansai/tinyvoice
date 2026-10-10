//! Owned hotkey leases, activation sequencing, and replayable event batches.
use std::collections::{HashMap, VecDeque};
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tinyvoice_bus::{
    ActivationMode, HotkeyBatch, HotkeyError, HotkeyEvent, HotkeyFeedRequest, HotkeyHandle,
    HotkeyHandleRequest, HotkeyReadRequest, HotkeyReply, HotkeyRequest, HotkeyReserveRequest,
    HotkeyResult, HotkeySource, HotkeyState, HotkeyStatus, SequencedHotkeyEvent,
};

mod native;

const MAX_RESERVATIONS: usize = 16;
const RESERVATION_TTL: std::time::Duration = std::time::Duration::from_secs(60);
const MAX_KEY_BYTES: usize = 128;
const MAX_HANDLE_BYTES: usize = 32;
const MAX_FEED_FACTS: usize = 64;
const MAX_BATCH_EVENTS: usize = 64;
const MAX_EVENTS: usize = 256;

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // These are independent key, activation, and continuity facts.
struct Lease {
    request: HotkeyRequest,
    created: std::time::Instant,
    state: HotkeyState,
    generation: Option<u64>,
    source_sequence: u64,
    last_feed: Option<(
        Vec<tinyvoice_bus::SequencedHostFact>,
        bool,
        Option<HotkeyError>,
    )>,
    active: bool,
    armed: bool,
    source_down: bool,
    continuity_lost: bool,
    event_sequence: u64,
    next_batch: u64,
    events: VecDeque<SequencedHotkeyEvent>,
    pending_batch: Option<HotkeyBatch>,
    reset_pending: bool,
    native: Option<native::NativeListener>,
}

#[derive(Debug)]
pub(super) struct Hotkeys {
    closed: AtomicBool,
    next_generation: std::sync::atomic::AtomicU64,
    starts_in_progress: Mutex<usize>,
    start_changed: Condvar,
    leases: Mutex<HashMap<HotkeyHandle, Lease>>,
    backend: std::sync::Arc<dyn native::Backend>,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self::with_backend(native::backend())
    }
}

impl Hotkeys {
    fn with_backend(backend: std::sync::Arc<dyn native::Backend>) -> Self {
        Self {
            closed: AtomicBool::new(false),
            next_generation: std::sync::atomic::AtomicU64::new(0),
            starts_in_progress: Mutex::new(0),
            start_changed: Condvar::new(),
            leases: Mutex::new(HashMap::new()),
            backend,
        }
    }
    pub(super) fn reserve(&self, request: HotkeyReserveRequest) -> HotkeyResult<HotkeyHandle> {
        let request = request.request;
        if request.key.is_empty() || request.key.len() > MAX_KEY_BYTES {
            return Err(HotkeyError::InvalidRequest);
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(if self.closed.load(Ordering::SeqCst) {
                HotkeyError::Closed
            } else {
                HotkeyError::InvalidRequest
            });
        }
        // Use the existing parser so aliases and key spelling stay one contract.
        if request.source == HotkeySource::Native {
            #[cfg(feature = "native-hotkeys")]
            tinyvoice::hotkey::parse_hotkey(&request.key)
                .map_err(|_| HotkeyError::InvalidRequest)?;
            #[cfg(not(feature = "native-hotkeys"))]
            return Err(HotkeyError::Unsupported);
        } else if request.key != "Fn" && request.key != "fn" {
            return Err(HotkeyError::InvalidRequest);
        }
        self.reserve_validated(request)
    }

    fn reserve_validated(&self, request: HotkeyRequest) -> HotkeyResult<HotkeyHandle> {
        let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
        if self.closed.load(Ordering::SeqCst) {
            return Err(HotkeyError::Closed);
        }
        leases.retain(|_, lease| match lease.state {
            HotkeyState::Reserved => lease.created.elapsed() < RESERVATION_TTL,
            HotkeyState::Stopped => false,
            _ => true,
        });
        if leases.len() >= MAX_RESERVATIONS {
            return Err(HotkeyError::LimitExceeded);
        }
        let handle = new_handle()?;
        leases.insert(
            handle.clone(),
            Lease {
                request,
                created: std::time::Instant::now(),
                state: HotkeyState::Reserved,
                generation: None,
                source_sequence: 0,
                last_feed: None,
                active: false,
                armed: true,
                source_down: false,
                continuity_lost: false,
                event_sequence: 0,
                next_batch: 1,
                events: VecDeque::new(),
                pending_batch: None,
                reset_pending: false,
                native: None,
            },
        );
        Ok(handle)
    }

    pub(super) fn start(&self, request: &HotkeyHandleRequest) -> HotkeyResult<HotkeyReply> {
        validate_handle(&request.handle)?;
        let mut starts = self
            .starts_in_progress
            .lock()
            .map_err(|_| HotkeyError::Closed)?;
        loop {
            let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
            if leases.get(&request.handle).is_some_and(|lease| {
                lease.state == HotkeyState::Reserved && lease.created.elapsed() >= RESERVATION_TTL
            }) {
                leases.remove(&request.handle);
                return Err(HotkeyError::UnknownHandle);
            }
            let lease = leases
                .get_mut(&request.handle)
                .ok_or(HotkeyError::UnknownHandle)?;
            if lease.state == HotkeyState::Running {
                return Ok(status(lease));
            }
            if lease.state == HotkeyState::Starting {
                drop(leases);
                starts = self
                    .start_changed
                    .wait(starts)
                    .map_err(|_| HotkeyError::Closed)?;
                continue;
            }
            if lease.state != HotkeyState::Reserved || self.closed.load(Ordering::SeqCst) {
                return Err(HotkeyError::Closed);
            }
            let generation = self
                .next_generation
                .fetch_add(1, Ordering::SeqCst)
                .checked_add(1)
                .ok_or(HotkeyError::LimitExceeded)?;
            lease.generation = Some(generation);
            if lease.request.source == HotkeySource::Native {
                lease.state = HotkeyState::Starting;
                let native_request = lease.request.clone();
                *starts += 1;
                drop(leases);
                drop(starts);
                let result = self.backend.start(&native_request);
                return self.finish_native_start(&request.handle, generation, result);
            }
            lease.state = HotkeyState::Running;
            return Ok(status(lease));
        }
    }

    fn finish_native_start(
        &self,
        handle: &HotkeyHandle,
        generation: u64,
        result: Result<native::NativeListener, HotkeyError>,
    ) -> HotkeyResult<HotkeyReply> {
        match result {
            Err(error) => {
                let mut starts = self
                    .starts_in_progress
                    .lock()
                    .map_err(|_| HotkeyError::Closed)?;
                let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
                let closed = self.closed.load(Ordering::SeqCst);
                if let Some(lease) = leases.get_mut(handle) {
                    if lease.generation == Some(generation)
                        && lease.state == HotkeyState::Starting
                        && !closed
                    {
                        lease.state = HotkeyState::Reserved;
                    } else {
                        lease.state = HotkeyState::Stopped;
                    }
                }
                Self::finish_startup(&mut starts, &self.start_changed);
                if closed {
                    Err(HotkeyError::Closed)
                } else {
                    Err(error)
                }
            }
            Ok(mut listener) => {
                let mut starts = self
                    .starts_in_progress
                    .lock()
                    .map_err(|_| HotkeyError::Closed)?;
                let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
                let publish = !self.closed.load(Ordering::SeqCst)
                    && leases.get(handle).is_some_and(|lease| {
                        lease.generation == Some(generation) && lease.state == HotkeyState::Starting
                    });
                if publish {
                    let lease = leases.get_mut(handle).ok_or(HotkeyError::Closed)?;
                    lease.native = Some(listener);
                    lease.state = HotkeyState::Running;
                    let status = status(lease);
                    Self::finish_startup(&mut starts, &self.start_changed);
                    return Ok(status);
                }
                if let Some(lease) = leases.get_mut(handle) {
                    lease.state = HotkeyState::Stopping;
                }
                drop(leases);
                drop(starts);

                let cleanup = listener.stop();
                let mut starts = self
                    .starts_in_progress
                    .lock()
                    .map_err(|_| HotkeyError::Closed)?;
                let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
                if let Some(lease) = leases.get_mut(handle) {
                    if cleanup.is_err() {
                        lease.native = Some(listener);
                        lease.state = HotkeyState::Stopping;
                    } else {
                        lease.state = HotkeyState::Stopped;
                    }
                }
                Self::finish_startup(&mut starts, &self.start_changed);
                cleanup.map_err(|_| HotkeyError::CleanupFailed)?;
                Err(HotkeyError::Closed)
            }
        }
    }

    fn finish_startup(starts: &mut usize, changed: &Condvar) {
        *starts = starts.saturating_sub(1);
        changed.notify_all();
    }

    pub(super) fn feed(&self, request: &HotkeyFeedRequest) -> HotkeyResult<HotkeyReply> {
        validate_handle(&request.handle)?;
        if request.facts.len() > MAX_FEED_FACTS {
            return Err(HotkeyError::LimitExceeded);
        }
        let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
        let lease = leases
            .get_mut(&request.handle)
            .ok_or(HotkeyError::UnknownHandle)?;
        if lease.request.source != HotkeySource::Host {
            return Err(HotkeyError::InvalidRequest);
        }
        if lease.state != HotkeyState::Running {
            return Err(HotkeyError::Closed);
        }
        if lease.generation != Some(request.generation) {
            return Err(HotkeyError::StaleGeneration);
        }
        let signature = (request.facts.clone(), request.overflow);
        if let Some((prior, prior_overflow, prior_error)) = &lease.last_feed
            && prior == &request.facts
            && *prior_overflow == request.overflow
        {
            if let Some(error) = prior_error {
                return Err(*error);
            }
            return Ok(status(lease));
        }
        if request.overflow {
            reset(lease);
            lease.source_sequence = request.facts.last().map_or(lease.source_sequence, |fact| {
                lease.source_sequence.max(fact.sequence)
            });
            lease.last_feed = Some((signature.0, signature.1, None));
            return Ok(status(lease));
        }
        for fact in &request.facts {
            if fact.sequence <= lease.source_sequence {
                return Err(HotkeyError::SequenceGap);
            }
            if fact.sequence != lease.source_sequence.saturating_add(1) {
                reset(lease);
                lease.source_sequence = fact.sequence;
                lease.last_feed = Some((signature.0, signature.1, Some(HotkeyError::SequenceGap)));
                return Err(HotkeyError::SequenceGap);
            }
            lease.source_sequence = fact.sequence;
            apply_fact(lease, matches!(fact.fact, tinyvoice_bus::HostKeyFact::Down));
        }
        lease.last_feed = Some((signature.0, signature.1, None));
        Ok(status(lease))
    }

    pub(super) fn read(&self, request: &HotkeyReadRequest) -> HotkeyResult<HotkeyBatch> {
        validate_handle(&request.handle)?;
        let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
        let lease = leases
            .get_mut(&request.handle)
            .ok_or(HotkeyError::UnknownHandle)?;
        if lease.state != HotkeyState::Running {
            return Err(HotkeyError::Closed);
        }
        let overflow = lease
            .native
            .as_ref()
            .is_some_and(|listener| listener.overflow.swap(false, Ordering::SeqCst));
        if overflow {
            if let Some(listener) = lease.native.as_ref() {
                while listener.events.try_recv().is_ok() {}
            }
            reset(lease);
        } else if let Some(listener) = lease.native.as_mut() {
            let facts: Vec<_> = std::iter::from_fn(|| listener.events.try_recv().ok()).collect();
            for down in facts {
                apply_fact(lease, down);
            }
        }
        if let Some(batch) = &lease.pending_batch {
            match request.acknowledged_batch {
                Some(ack) if ack == batch.batch => lease.pending_batch = None,
                Some(ack) if ack > batch.batch => return Err(HotkeyError::InvalidRequest),
                None | Some(_) => return Ok(batch.clone()),
            }
        } else if request.acknowledged_batch.is_some() {
            return Err(HotkeyError::InvalidRequest);
        }
        let generation = lease.generation.ok_or(HotkeyError::Closed)?;
        let event_count = lease.events.len().min(MAX_BATCH_EVENTS);
        let events: Vec<_> = lease.events.drain(..event_count).collect();
        let batch = HotkeyBatch {
            generation,
            batch: lease.next_batch,
            events,
            reset: std::mem::take(&mut lease.reset_pending),
            active: lease.active,
        };
        lease.next_batch = lease.next_batch.saturating_add(1);
        lease.pending_batch = Some(batch.clone());
        Ok(batch)
    }

    pub(super) fn stop(&self, request: &HotkeyHandleRequest) -> HotkeyResult<HotkeyReply> {
        validate_handle(&request.handle)?;
        let mut starts = self
            .starts_in_progress
            .lock()
            .map_err(|_| HotkeyError::Closed)?;
        let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
        let lease = leases
            .get_mut(&request.handle)
            .ok_or(HotkeyError::UnknownHandle)?;
        if lease.state == HotkeyState::Starting {
            lease.state = HotkeyState::Stopping;
            drop(leases);
            while *starts > 0 {
                starts = self
                    .start_changed
                    .wait(starts)
                    .map_err(|_| HotkeyError::Closed)?;
            }
            leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
        }
        let lease = leases
            .get_mut(&request.handle)
            .ok_or(HotkeyError::UnknownHandle)?;
        if lease.state == HotkeyState::Running {
            reset(lease);
            lease.state = HotkeyState::Stopping;
        }
        if lease.state == HotkeyState::Stopped {
            return Ok(status(lease));
        }
        if let Some(listener) = lease.native.as_mut() {
            listener.stop()?;
            lease.native = None;
        }
        lease.state = HotkeyState::Stopped;
        let status = status(lease);
        leases.remove(&request.handle);
        Ok(status)
    }

    pub(super) fn shutdown(&self) -> HotkeyResult<HotkeyReply> {
        let mut starts = self
            .starts_in_progress
            .lock()
            .map_err(|_| HotkeyError::Closed)?;
        self.closed.store(true, Ordering::SeqCst);
        let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
        for lease in leases.values_mut() {
            if matches!(lease.state, HotkeyState::Running | HotkeyState::Starting) {
                reset(lease);
                lease.state = HotkeyState::Stopping;
            }
        }
        drop(leases);
        while *starts > 0 {
            starts = self
                .start_changed
                .wait(starts)
                .map_err(|_| HotkeyError::Closed)?;
        }
        let mut leases = self.leases.lock().map_err(|_| HotkeyError::Closed)?;
        for lease in leases.values_mut() {
            if let Some(listener) = lease.native.as_mut() {
                listener.stop()?;
                lease.native = None;
            }
            lease.state = HotkeyState::Stopped;
        }
        leases.clear();
        Ok(HotkeyStatus {
            state: HotkeyState::Stopped,
            generation: None,
            active: false,
            continuity_lost: false,
        })
    }
}

fn apply_fact(lease: &mut Lease, down: bool) {
    if !down {
        lease.source_down = false;
        if !lease.armed {
            lease.armed = true;
            return;
        }
        if lease.request.mode == ActivationMode::Push && lease.active {
            emit(lease, HotkeyEvent::Released);
        }
        return;
    }
    if lease.source_down {
        return;
    }
    lease.source_down = true;
    if !lease.armed {
        return;
    }
    match lease.request.mode {
        ActivationMode::Tap => emit(
            lease,
            if lease.active {
                HotkeyEvent::Released
            } else {
                HotkeyEvent::Pressed
            },
        ),
        ActivationMode::Push if !lease.active => emit(lease, HotkeyEvent::Pressed),
        ActivationMode::Push => {}
    }
}

fn validate_handle(handle: &HotkeyHandle) -> HotkeyResult<()> {
    if handle.0.len() > MAX_HANDLE_BYTES {
        return Err(HotkeyError::LimitExceeded);
    }
    Ok(())
}

fn emit(lease: &mut Lease, event: HotkeyEvent) {
    if lease.events.len() >= MAX_EVENTS {
        reset(lease);
        return;
    }
    lease.active = event == HotkeyEvent::Pressed;
    lease.event_sequence = lease.event_sequence.saturating_add(1);
    lease.events.push_back(SequencedHotkeyEvent {
        generation: lease.generation.unwrap_or_default(),
        sequence: lease.event_sequence,
        event,
    });
}

fn reset(lease: &mut Lease) {
    lease.active = false;
    lease.armed = false;
    lease.source_down = true;
    lease.continuity_lost = true;
    lease.reset_pending = true;
    lease.events.clear();
}

fn status(lease: &Lease) -> HotkeyStatus {
    HotkeyStatus {
        state: lease.state,
        generation: lease.generation,
        active: lease.active,
        continuity_lost: lease.continuity_lost,
    }
}

fn new_handle() -> HotkeyResult<HotkeyHandle> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| HotkeyError::LimitExceeded)?;
    Ok(HotkeyHandle(format!("{:032x}", u128::from_le_bytes(bytes))))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
