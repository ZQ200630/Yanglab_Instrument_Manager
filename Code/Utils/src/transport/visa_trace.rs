//! Bounded, payload-free call evidence. This does not cancel VISA calls or
//! constitute a lifecycle/cleanup receipt. No sink lock spans a vendor call.
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::VecDeque,
    marker::PhantomData,
    rc::Rc,
    sync::{Mutex, OnceLock, TryLockError},
    time::Instant,
};

const CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Stage {
    #[serde(rename = "visa_library_load")]
    LibraryLoad,
    #[serde(rename = "viOpenDefaultRM")]
    OpenDefaultRm,
    #[serde(rename = "viParseRsrcEx")]
    ParseResource,
    #[serde(rename = "viOpen")]
    Open,
    #[serde(rename = "viFindRsrc")]
    FindResource,
    #[serde(rename = "viFindNext")]
    FindNext,
    #[serde(rename = "viSetAttribute(TMO_VALUE)")]
    SetTimeout,
    #[serde(rename = "viSetAttribute(TERMCHAR_EN)")]
    SetTermination,
    #[serde(rename = "viRead")]
    Read,
    #[serde(rename = "viWrite")]
    Write,
    #[serde(rename = "viClose")]
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pending,
    Returned,
    Error,
    Unwound,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Call {
    pub id: u64,
    pub stage: Stage,
    pub request_id: Option<String>,
    pub domain: Option<String>,
    pub started_monotonic_ms: u64,
    pub elapsed_ms: u64,
    pub status: Option<i32>,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub active: Vec<Call>,
    pub recent: Vec<Call>,
    /// Entries omitted when the active capacity was already occupied. Never
    /// evict a pending call merely because other calls completed or started.
    pub dropped: u64,
}

#[derive(Clone, Default)]
struct Context {
    request_id: Option<String>,
    domain: Option<String>,
}
thread_local! {
    static CONTEXT: RefCell<Context> = RefCell::new(Context::default());
}

/// Restores only this thread's prior context; deliberately cannot cross threads.
pub struct ScopeGuard {
    previous: Option<Context>,
    _thread: PhantomData<Rc<()>>,
}
/// Attach protocol identifiers, not resources or user data. Invalid identifiers
/// are omitted rather than truncated into a different, apparently valid owner.
/// Calls on cleanup threads without an explicit scope honestly have null IDs.
pub fn scope(request_id: &str, domain: &str) -> ScopeGuard {
    fn identifier(value: &str) -> Option<String> {
        (!value.is_empty()
            && value.len() <= 96
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)))
            .then(|| value.to_owned())
    }
    let context = Context { request_id: identifier(request_id), domain: identifier(domain) };
    ScopeGuard {
        previous: Some(CONTEXT.with(|slot| slot.replace(context))),
        _thread: PhantomData,
    }
}
impl Drop for ScopeGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            CONTEXT.with(|slot| { slot.replace(previous); });
        }
    }
}

struct Active {
    call: Call,
    started: Instant,
}
struct Sink {
    origin: Instant,
    next_id: u64,
    active: Vec<Active>,
    recent: VecDeque<Call>,
    dropped: u64,
}
fn sink() -> &'static Mutex<Sink> {
    static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();
    SINK.get_or_init(|| Mutex::new(Sink {
        origin: Instant::now(),
        next_id: 1,
        active: Vec::with_capacity(CAPACITY),
        recent: VecDeque::with_capacity(CAPACITY),
        dropped: 0,
    }))
}
fn millis(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
/// Never waits for diagnostic mutation, let alone native I/O. None means busy,
/// not an empty trace; consumers must preserve that distinction.
pub fn snapshot() -> Option<Snapshot> {
    let state = match sink().try_lock() {
        Ok(state) => state,
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
        Err(TryLockError::WouldBlock) => return None,
    };
    Some(Snapshot {
        active: state.active.iter().map(|entry| {
            let mut call = entry.call.clone();
            call.elapsed_ms = millis(entry.started.elapsed());
            call
        }).collect(),
        recent: state.recent.iter().cloned().collect(),
        dropped: state.dropped,
    })
}

pub(crate) struct CallGuard {
    id: Option<u64>,
}
pub(crate) fn enter(stage: Stage) -> CallGuard {
    let context = CONTEXT.with(|slot| slot.borrow().clone());
    // This short critical section publishes before FFI. A try_lock here could
    // omit the very call that subsequently hangs, so only readers are try-lock.
    let mut state = sink().lock().unwrap_or_else(|error| error.into_inner());
    if state.active.len() == CAPACITY {
        state.dropped = state.dropped.saturating_add(1);
        return CallGuard { id: None };
    }
    let id = state.next_id;
    state.next_id = state.next_id.saturating_add(1);
    let started = Instant::now();
    let started_monotonic_ms = millis(started.duration_since(state.origin));
    state.active.push(Active {
        call: Call {
            id, stage, request_id: context.request_id, domain: context.domain,
            started_monotonic_ms, elapsed_ms: 0, status: None, outcome: Outcome::Pending,
        },
        started,
    });
    CallGuard { id: Some(id) }
}
impl CallGuard {
    pub(crate) fn finish(mut self, status: Option<i32>, outcome: Outcome) {
        self.complete(status, outcome);
    }
    fn complete(&mut self, status: Option<i32>, outcome: Outcome) {
        let Some(id) = self.id.take() else { return; };
        let mut state = sink().lock().unwrap_or_else(|error| error.into_inner());
        if let Some(index) = state.active.iter().position(|entry| entry.call.id == id) {
            let entry = state.active.remove(index);
            let mut call = entry.call;
            call.elapsed_ms = millis(entry.started.elapsed());
            call.status = status;
            call.outcome = outcome;
            if state.recent.len() == CAPACITY { state.recent.pop_front(); }
            state.recent.push_back(call);
        }
    }
}
impl Drop for CallGuard {
    fn drop(&mut self) {
        self.complete(None, Outcome::Unwound);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Catches replacing try_lock with a reader that waits behind sink mutation.
    #[test]
    fn snapshot_reports_busy_without_waiting_for_the_sink_lock() {
        let held = sink().lock().unwrap_or_else(|error| error.into_inner());
        let (sent, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || sent.send(snapshot()).unwrap());
        let value = received.recv_timeout(std::time::Duration::from_millis(200));
        drop(held);
        worker.join().unwrap();
        assert!(matches!(value, Ok(None)), "busy is not an empty or delayed snapshot");
    }
}
