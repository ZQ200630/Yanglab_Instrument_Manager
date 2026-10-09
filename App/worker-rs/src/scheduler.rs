use crate::{
    new_id,
    observations::Observation,
    safety::{self, SafetyIntent},
    DomainRegistry, DomainSnapshot, WorkerError,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, OnceLock, Weak,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use yang_drivers::{
    clock::Clock,
    lifecycle::{CleanupReport, DriverState},
    transport::Deadline,
};
use yang_protocol::{
    encode_outcome, parse_request, ContextV3, DomainRef, Limits, OutcomeV3, Phase, RequestV3,
    WireError,
};
pub trait Backend: Send + Sync {
    fn registry(&self) -> DomainRegistry;
    fn execute(&self, request: &RequestV3) -> OutcomeV3;
    fn observe(&self, context: &ContextV3) -> Observation;
    /// Bounded metadata-only cancellation/interlock notification. No native I/O.
    fn request_stop(&self, _context: &ContextV3) {}
    fn request_safety(&self, context: &ContextV3, _intent: SafetyIntent) {
        self.request_stop(context);
    }
    fn begin_shutdown(&self) {}
    /// One worker-owned spool, installed before admission. No instrument I/O.
    fn install_spool(&self, _spool: Arc<Mutex<crate::captures::CaptureSpool>>) {}
    fn cleanup_reports(&self) -> Vec<CleanupReport> {
        Vec::new()
    }
    /// Completed preserving cleanup evidence, consumed after connect quiescence.
    /// This is internal lifecycle metadata and never changes the wire schema.
    fn take_failed_connect_cleanup(&self, _context: &ContextV3) -> Option<CleanupReport> {
        None
    }
    fn auxiliary_responsibility(&self) -> bool {
        false
    }
    /// Cached only, safe while native exchanges are blocked on the owner thread.
    fn newport_resources_released(&self) -> bool {
        true
    }
    /// Called only after all scheduled native calls have settled.
    fn finish_shutdown(&self) -> Result<(), WorkerError> {
        Ok(())
    }
}
pub struct Scheduler {
    core: Arc<Core>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}
#[derive(Clone)]
pub struct PendingOutcome {
    promise: Arc<Promise>,
}
pub struct SchedulerSnapshot {
    pub domains: Vec<DomainSnapshot>,
    pub active: usize,
    pub pending_normal: usize,
    pub reply_slots: usize,
    pub responsibility_slots: usize,
    pub callback_errors: usize,
    pub closing: bool,
}
type Callback = Box<dyn FnOnce(OutcomeV3) + Send>;
struct Promise {
    value: Mutex<(Option<OutcomeV3>, Vec<Callback>)>,
    ready: Condvar,
    core: Weak<Core>,
    ticket: u64,
}
#[derive(Clone)]
struct Work {
    request: RequestV3,
    promise: Arc<Promise>,
    sequence: u64,
    normal: bool,
    counted: Arc<AtomicBool>,
    tracks_id: bool,
}
struct Group {
    intent: SafetyIntent,
    request: RequestV3,
    waiters: Vec<Work>,
    running: bool,
    attempt_id: String,
}
struct Lane {
    driver: String,
    queue: Option<Work>,
    active: bool,
    active_request_id: Option<String>,
    observing: bool,
    readback: Option<(Work, OutcomeV3)>,
    safety: Option<Group>,
    last_safety: Option<Value>,
    pending_notifications: usize,
    state: &'static str,
    status: Value,
    refresh: bool,
    next_refresh: Duration,
    healthy: Option<ContextV3>,
}
#[derive(Default)]
struct State {
    lanes: BTreeMap<String, Lane>,
    management: Option<Work>,
    management_active: bool,
    management_method: Option<String>,
    sequence: u64,
    outstanding: usize,
    normal: usize,
    active: usize,
    observing: usize,
    ids: HashSet<String>,
    recent: VecDeque<String>,
    replies: HashMap<u64, Arc<Promise>>,
    callback_errors: usize,
    closing: bool,
    terminated: bool,
}
struct Core {
    backend: Arc<dyn Backend>,
    registry: DomainRegistry,
    clock: Arc<dyn Clock>,
    limits: Limits,
    state: Mutex<State>,
    wake: Condvar,
}
fn key(domain: &DomainRef) -> String {
    format!("{}:{}", domain.kind, domain.id)
}
fn failure(context: Option<ContextV3>, phase: Phase, code: &str, message: &str) -> OutcomeV3 {
    OutcomeV3 {
        phase,
        context,
        result: None,
        error: Some(WireError {
            kind: code.into(),
            message: message.into(),
            attempt_id: None,
        }),
    }
}
fn global(registry: &DomainRegistry) -> ContextV3 {
    ContextV3 {
        session_id: registry.session_id().into(),
        domain: None,
        connection_id: None,
        epoch: 0,
    }
}
fn merge(target: &mut Value, delta: &Value) {
    if let (Some(target), Some(delta)) = (target.as_object_mut(), delta.as_object()) {
        target.extend(delta.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
}
fn stronger(new: SafetyIntent, old: SafetyIntent) -> bool {
    fn rank(intent: SafetyIntent) -> u8 {
        match intent {
            SafetyIntent::Zero | SafetyIntent::CurrentOff => 0,
            SafetyIntent::TecOff => 1,
            SafetyIntent::Disconnect => 2,
        }
    }
    rank(new) > rank(old)
}
impl Scheduler {
    pub fn new(
        backend: Arc<dyn Backend>,
        clock: Arc<dyn Clock>,
        limits: Limits,
    ) -> Result<Self, WorkerError> {
        let ceiling = Limits::default();
        if limits.max_domains == 0
            || limits.max_domains > ceiling.max_domains
            || limits.max_pending_normal == 0
            || limits.max_pending_normal > 31
            || limits.max_reply_slots < 2
            || limits.max_reply_slots > 226
            || limits.max_responsibility_slots == 0
            || limits.max_responsibility_slots > 225
            || limits.ordinary_slots == 0
            || limits.ordinary_slots > 4
            || limits.observation_slots == 0
            || limits.observation_slots > 4
        {
            return Err(WorkerError::new(
                "Limits",
                "invalid native scheduler capacity",
            ));
        }
        let registry = backend.registry();
        let core = Arc::new(Core {
            backend,
            registry,
            clock,
            limits,
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        core.sync_lanes(&mut core.state.lock().unwrap())?;
        let mut threads = Vec::new();
        for (count, pool) in [
            (limits.ordinary_slots, 0),
            (limits.observation_slots, 1),
            (limits.max_domains, 2),
            (1, 3),
        ] {
            for index in 0..count {
                let worker = core.clone();
                match thread::Builder::new()
                    .name(format!("yang-{pool}-{index}"))
                    .spawn(move || worker.run(pool))
                {
                    Ok(handle) => threads.push(handle),
                    Err(error) => {
                        core.begin_shutdown();
                        retain_threads(core, threads);
                        return Err(WorkerError::new("ThreadCreation", error.to_string()));
                    }
                }
            }
        }
        Ok(Self {
            core,
            threads: Mutex::new(threads),
        })
    }
    pub fn submit(&self, request: RequestV3) -> Result<PendingOutcome, WorkerError> {
        self.core.submit(request, false)
    }
    pub fn snapshot(&self) -> SchedulerSnapshot {
        let state = self.core.state.lock().unwrap_or_else(|e| e.into_inner());
        SchedulerSnapshot {
            domains: self.core.registry.snapshot(),
            active: state.active + usize::from(state.management_active),
            pending_normal: state.normal,
            reply_slots: state.replies.len(),
            responsibility_slots: state.outstanding + state.observing,
            callback_errors: state.callback_errors,
            closing: state.closing,
        }
    }
    pub fn begin_shutdown(&self) {
        self.core.begin_shutdown();
    }
    pub fn continue_shutdown(&self) {
        self.core.begin_shutdown();
        for snapshot in self.core.registry.snapshot() {
            if snapshot.responsibility && snapshot.context.connection_id.is_some() {
                if let Ok(id) = new_id() {
                    let _ = self.core.submit(
                        RequestV3 {
                            v: 3,
                            id,
                            method: "disconnect".into(),
                            params: json!({}),
                            context: Some(snapshot.context),
                        },
                        true,
                    );
                }
            }
        }
    }
    pub fn join_when_released(&self, deadline: Deadline) -> Result<(), WorkerError> {
        let mut state = self
            .core
            .state
            .lock()
            .map_err(|_| WorkerError::new("Scheduler", "state poisoned"))?;
        while !state.terminated {
            let timeout = deadline.remaining_millis().map_err(|_| {
                WorkerError::new(
                    "PendingResponsibility",
                    "scheduler has not confirmed release",
                )
            })?;
            state = self
                .core
                .wake
                .wait_timeout(state, Duration::from_millis(u64::from(timeout.min(50))))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(state);
        let mut handles = self
            .threads
            .lock()
            .map_err(|_| WorkerError::new("Scheduler", "thread registry poisoned"))?;
        while !handles.is_empty() {
            deadline.remaining_millis().map_err(|_| {
                WorkerError::new("PendingResponsibility", "native threads have not joined")
            })?;
            if handles
                .iter()
                .any(|h| h.thread().id() == thread::current().id())
            {
                return Err(WorkerError::new(
                    "PendingResponsibility",
                    "a callback cannot join its own thread",
                ));
            }
            if let Some(index) = handles.iter().position(JoinHandle::is_finished) {
                let handle = handles.swap_remove(index);
                handle
                    .join()
                    .map_err(|_| WorkerError::new("NativeThread", "worker thread panicked"))?;
            } else {
                drop(handles);
                let state = self.core.state.lock().unwrap();
                let _ = self
                    .core
                    .wake
                    .wait_timeout(state, Duration::from_millis(1))
                    .unwrap();
                handles = self.threads.lock().unwrap();
            }
        }
        Ok(())
    }
}
impl PendingOutcome {
    pub fn wait(&self, deadline: Deadline) -> Result<OutcomeV3, WorkerError> {
        let mut value = self.promise.value.lock().unwrap_or_else(|e| e.into_inner());
        while value.0.is_none() {
            let timeout = deadline.remaining_millis().map_err(|_| {
                WorkerError::new(
                    "TransportTimeout",
                    "operation is still retained; do not replay",
                )
            })?;
            value = self
                .promise
                .ready
                .wait_timeout(value, Duration::from_millis(u64::from(timeout)))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let outcome = value.0.clone().unwrap();
        drop(value);
        self.acknowledge();
        Ok(outcome)
    }
    pub fn acknowledge(&self) {
        if let Some(core) = self.promise.core.upgrade() {
            core.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .replies
                .remove(&self.promise.ticket);
            core.wake.notify_all();
        }
    }
    pub fn on_complete(&self, callback: impl FnOnce(OutcomeV3) + Send + 'static) {
        let mut value = self.promise.value.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(outcome) = value.0.clone() {
            drop(value);
            self.promise.callback(Box::new(callback), outcome);
        } else {
            value.1.push(Box::new(callback));
        }
    }
}
impl Promise {
    fn callback(&self, callback: Callback, outcome: OutcomeV3) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(outcome))).is_err() {
            if let Some(core) = self.core.upgrade() {
                core.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .callback_errors += 1;
                core.wake.notify_all();
            }
        }
    }
    fn complete(&self, outcome: OutcomeV3) {
        let mut value = self.value.lock().unwrap_or_else(|e| e.into_inner());
        if value.0.is_some() {
            return;
        }
        value.0 = Some(outcome.clone());
        let callbacks = std::mem::take(&mut value.1);
        drop(value);
        self.ready.notify_all();
        for callback in callbacks {
            self.callback(callback, outcome.clone());
        }
    }
}
impl Core {
    fn sync_lanes(&self, state: &mut State) -> Result<(), WorkerError> {
        let snapshots = self.registry.snapshot();
        if snapshots.len() > self.limits.max_domains {
            return Err(WorkerError::new("Capacity", "too many execution domains"));
        }
        let known: HashSet<_> = snapshots
            .iter()
            .map(|d| key(d.context.domain.as_ref().unwrap()))
            .collect();
        state.lanes.retain(|key, lane| {
            known.contains(key)
                || lane.active
                || lane.observing
                || lane.queue.is_some()
                || lane.safety.is_some()
        });
        for snapshot in snapshots {
            let domain = snapshot.context.domain.as_ref().unwrap();
            let driver = self.registry.config(domain)?.driver_kind;
            state.lanes.entry(key(domain)).or_insert(Lane {
                driver,
                queue: None,
                active: false,
                active_request_id: None,
                observing: false,
                readback: None,
                safety: None,
                last_safety: None,
                pending_notifications: 0,
                state: if snapshot.state == DriverState::Ready {
                    "READY"
                } else if snapshot.responsibility {
                    "RETAINED"
                } else {
                    "DISCONNECTED"
                },
                status: json!({}),
                refresh: false,
                next_refresh: self.clock.now() + Duration::from_millis(2500),
                healthy: None,
            });
        }
        Ok(())
    }
    fn obligation_done(&self, state: &mut State, work: &Work) {
        if work.counted.swap(false, Ordering::AcqRel) {
            state.outstanding = state.outstanding.saturating_sub(1);
            if work.normal {
                state.normal = state.normal.saturating_sub(1);
            }
            if let Some(domain) = work
                .request
                .context
                .as_ref()
                .and_then(|c| c.domain.as_ref())
            {
                self.registry.pending_finish(domain);
            }
        }
    }
    fn complete(&self, work: &Work, outcome: OutcomeV3) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            self.obligation_done(&mut state, work);
            if work.tracks_id {
                state.ids.remove(&work.request.id);
                state.recent.push_back(work.request.id.clone());
                if state.recent.len() > 256 {
                    state.recent.pop_front();
                }
            }
            self.maybe_terminate(&mut state);
        }
        self.wake.notify_all();
        work.promise.complete(outcome);
    }
    fn maybe_terminate(&self, state: &mut State) {
        if state.closing
            && state.outstanding == 0
            && state.observing == 0
            && !self.registry.snapshot().iter().any(|d| d.responsibility)
        {
            state.terminated = true;
        }
    }
    fn cached_status(&self, state: &State) -> Value {
        let mut domains = serde_json::Map::new();
        let mut devices = serde_json::Map::new();
        let mut newport_released = self.backend.newport_resources_released()
            && state.management_method.as_deref() != Some("scan_lasers")
            && !state.management.as_ref().is_some_and(|w| w.request.method == "scan_lasers");
        let now = self.clock.now().as_secs_f64();
        for snapshot in self.registry.snapshot() {
            let domain = snapshot.context.domain.as_ref().unwrap();
            let key = key(domain);
            if let Some(lane) = state.lanes.get(&key) {
                if lane.driver == "laser" && (snapshot.responsibility || snapshot.pending != 0) {
                    newport_released = false;
                }
                domains.insert(key.clone(), json!({"context":snapshot.context,"state":lane.state,"observing":lane.observing,"pending":snapshot.pending,"responsibility":snapshot.responsibility,
                    "active_request_id":lane.active_request_id,"pending_request_id":lane.queue.as_ref().map(|w| &w.request.id),
                    "readback_request_id":lane.readback.as_ref().map(|(w,_)| &w.request.id),
                    "safety_request_id":lane.safety.as_ref().map(|g| &g.request.id),"safety":lane.last_safety}));
                if lane.status.as_object().is_some_and(|s| !s.is_empty()) {
                    let mut status = lane.status.clone();
                    if lane.driver == "laser" {
                        for (sample, age) in [("laser", "sample_age_s"), ("motion", "motion_age_s")] {
                            status[age] = status[sample]["received_at"].as_f64()
                                .filter(|t| t.is_finite() && *t <= now)
                                .map(|t| json!(now - t)).unwrap_or(Value::Null);
                        }
                    }
                    devices.insert(key, status);
                }
            }
        }
        json!({
            "session_id":self.registry.session_id(),
            "domains":domains,"devices":devices,"closing":state.closing,
            "consumer_callback_errors":state.callback_errors,
            "newport_resources_released":newport_released,
        })
    }
    fn submit(
        self: &Arc<Self>,
        request: RequestV3,
        internal: bool,
    ) -> Result<PendingOutcome, WorkerError> {
        let bytes = serde_json::to_vec(&request)
            .map_err(|e| WorkerError::new("ProtocolError", e.to_string()))?;
        let mut request = parse_request(&bytes)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| WorkerError::new("Scheduler", "state poisoned"))?;
        self.sync_lanes(&mut state)?;
        let ref_domain = request.context.as_ref().and_then(|c| c.domain.clone());
        let current = if let Some(domain) = &ref_domain {
            self.registry.context(domain)?
        } else {
            global(&self.registry)
        };
        if !internal && state.replies.len() >= self.limits.max_reply_slots {
            return Err(WorkerError::new(
                "ReplyCapacity",
                "reply delivery capacity exhausted",
            ));
        }
        state.sequence = state
            .sequence
            .checked_add(1)
            .ok_or_else(|| WorkerError::new("Capacity", "request sequence exhausted"))?;
        let promise = Arc::new(Promise {
            value: Mutex::new((None, vec![])),
            ready: Condvar::new(),
            core: Arc::downgrade(self),
            ticket: state.sequence,
        });
        let pending = PendingOutcome {
            promise: promise.clone(),
        };
        if !internal {
            let ticket = state.sequence;
            state.replies.insert(ticket, promise.clone());
        }
        let key = ref_domain.as_ref().map(key);
        let domain_method = matches!(
            request.method.as_str(),
            "action" | "connect" | "disconnect" | "resume"
        );
        let driver = key
            .as_ref()
            .and_then(|k| state.lanes.get(k))
            .map(|l| l.driver.clone())
            .unwrap_or_default();
        let intent = safety::intent(&driver, &request);
        let normal = domain_method && intent.is_none() && request.method != "resume";
        let duplicate = state.ids.contains(&request.id) || state.recent.contains(&request.id);
        let tracks_id = !duplicate;
        if tracks_id {
            state.ids.insert(request.id.clone());
        }
        let mut work = Work {
            request: request.clone(),
            promise,
            sequence: state.sequence,
            normal,
            counted: Arc::new(AtomicBool::new(false)),
            tracks_id,
        };
        let safe_old = intent.is_some()
            && request.context.as_ref().is_some_and(|c| {
                c.session_id == current.session_id
                    && c.connection_id == current.connection_id
                    && c.epoch <= current.epoch
            });
        let contextual = request.context.as_ref() == Some(&current)
            || (matches!(request.method.as_str(), "ping" | "status") && request.context.is_none())
            || safe_old;
        let mut immediate = None;
        let mut canceled = Vec::new();
        let mut shutdown = false;
        let mut stop_notification = None;
        if duplicate {
            immediate = Some(failure(
                Some(current.clone()),
                Phase::RejectedBeforeCall,
                "DuplicateRequest",
                "request ID already used",
            ));
        } else if !contextual {
            immediate = Some(failure(
                Some(current.clone()),
                Phase::RejectedBeforeCall,
                "StaleContext",
                "request context changed",
            ));
        } else if matches!(request.method.as_str(), "ping" | "status") {
            immediate = Some(OutcomeV3 {
                phase: Phase::Completed,
                context: Some(global(&self.registry)),
                result: Some(self.cached_status(&state)),
                error: None,
            });
        } else if state.terminated
            || (state.closing && intent.is_none() && request.method != "shutdown")
        {
            immediate = Some(failure(
                Some(current.clone()),
                Phase::RejectedBeforeCall,
                "Closing",
                "worker is closing",
            ));
        } else if state.outstanding + state.observing >= self.limits.max_responsibility_slots {
            immediate = Some(failure(
                Some(current.clone()),
                Phase::RejectedBeforeCall,
                "Capacity",
                "responsibility capacity exhausted",
            ));
        } else if request.method == "resume" {
            let lane = key.as_ref().and_then(|k| state.lanes.get_mut(k)).unwrap();
            if lane.state == "STOP_HELD"
                && !lane.active
                && !lane.observing
                && lane.safety.is_none()
                && lane.healthy.as_ref() == Some(&current)
            {
                lane.state = "READY";
                immediate = Some(OutcomeV3 {
                    phase: Phase::Completed,
                    context: Some(current.clone()),
                    result: Some(json!({"resumed":true})),
                    error: None,
                });
            } else {
                immediate = Some(failure(
                    Some(current.clone()),
                    Phase::RejectedBeforeCall,
                    "ResumeRestricted",
                    "fresh healthy readback and quiescent held state required",
                ));
            }
        } else if let Some(intent) = intent {
            let key = key.as_ref().unwrap();
            let lane = state.lanes.get_mut(key).unwrap();
            if current.connection_id.is_none() {
                immediate = Some(failure(
                    Some(current.clone()),
                    Phase::RejectedBeforeCall,
                    "Disconnected",
                    "no owned connection",
                ));
            } else if lane
                .safety
                .as_ref()
                .is_some_and(|g| !stronger(intent, g.intent))
            {
                immediate = Some(failure(
                    Some(current.clone()),
                    Phase::RejectedBeforeCall,
                    "AlreadyRunning",
                    "existing safety responsibility covers this intent",
                ));
            } else {
                let fenced = self.registry.fence(ref_domain.as_ref().unwrap())?;
                request.context = Some(fenced.clone());
                stop_notification = Some((fenced, intent));
                lane.pending_notifications += 1;
                if let Some(old) = lane.queue.take() {
                    canceled.push((old, Phase::SupersededBeforeCall));
                }
                if let Some((old, _)) = lane.readback.take() {
                    canceled.push((old, Phase::CompletedReadbackFailed));
                }
                lane.healthy = None;
                lane.refresh = false;
                lane.state = if intent == SafetyIntent::Disconnect {
                    "CLOSING"
                } else {
                    "STOPPING"
                };
                if let Some(group) = &mut lane.safety {
                    group.intent = intent;
                    group.request = request;
                    group.waiters.push(work.clone());
                } else {
                    lane.safety = Some(Group {
                        intent,
                        request,
                        waiters: vec![work.clone()],
                        running: false,
                        attempt_id: new_id()?,
                    });
                }
                if state.management.as_ref().is_some_and(|w| {
                    w.request.context.as_ref().and_then(|c| c.domain.as_ref())
                        == ref_domain.as_ref()
                }) {
                    canceled.push((
                        state.management.take().unwrap(),
                        Phase::SupersededBeforeCall,
                    ));
                }
                self.admit(&mut state, &work)?;
            }
        } else if normal {
            let key = key.as_ref().unwrap();
            let lane = state.lanes.get(key).unwrap();
            if lane.active
                || lane.queue.is_some()
                || lane.readback.is_some()
                || lane.safety.is_some()
                || state.normal >= self.limits.max_pending_normal
            {
                immediate = Some(failure(
                    Some(current.clone()),
                    Phase::RejectedBeforeCall,
                    "QueueFull",
                    "ordinary capacity/domain busy",
                ));
            } else if (request.method == "connect" && current.connection_id.is_some())
                || (request.method != "connect"
                    && lane.state != "READY"
                    && !(driver == "osa"
                        && request.params["name"] == "retry_staging"
                        && matches!(lane.state, "FAULT" | "RETAINED" | "DISCONNECTED")))
            {
                immediate = Some(failure(
                    Some(current.clone()),
                    Phase::RejectedBeforeCall,
                    "NotReady",
                    "domain is not ready or remains owned",
                ));
            } else {
                if request.method == "connect" {
                    request.context = Some(
                        self.registry
                            .bind(ref_domain.as_ref().unwrap(), &new_id()?)?,
                    );
                    work.request = request;
                    state.lanes.get_mut(key).unwrap().state = "CONNECTING";
                }
                self.admit(&mut state, &work)?;
                state.lanes.get_mut(key).unwrap().queue = Some(work.clone());
            }
        } else {
            if state.management.is_some() || state.management_active {
                immediate = Some(failure(
                    Some(current.clone()),
                    Phase::RejectedBeforeCall,
                    "ManagementBusy",
                    "management capacity exhausted",
                ));
            } else {
                self.admit(&mut state, &work)?;
                shutdown = work.request.method == "shutdown";
                state.management = Some(work.clone());
            }
        }
        // Publish quiescence atomically with removal from executable queues.
        // Promise delivery (which can call consumers) stays outside the lock.
        for (old, _) in &canceled {
            self.obligation_done(&mut state, old);
        }
        drop(state);
        // Call bounded metadata hooks without the status lock. The lane remains
        // gated until every admitted notification (including upgrades) settles.
        if let Some((context, intent)) = stop_notification {
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.backend.request_safety(&context, intent)
            }))
            .is_err();
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .lanes
                .get_mut(&crate::scheduler::key(context.domain.as_ref().unwrap()))
                .unwrap()
                .pending_notifications -= 1;
            if panicked {
                state.callback_errors += 1;
            }
        }
        self.wake.notify_all();
        for (old, phase) in canceled {
            self.complete(
                &old,
                failure(
                    old.request.context.clone(),
                    phase,
                    "StaleContext",
                    "work superseded by safety/context change",
                ),
            );
        }
        if let Some(outcome) = immediate {
            self.complete(&work, outcome);
        }
        if shutdown {
            self.begin_shutdown();
        }
        Ok(pending)
    }
    fn admit(&self, state: &mut State, work: &Work) -> Result<(), WorkerError> {
        if let Some(domain) = work
            .request
            .context
            .as_ref()
            .and_then(|c| c.domain.as_ref())
        {
            self.registry.pending_add(domain)?;
        }
        work.counted.store(true, Ordering::Release);
        state.outstanding += 1;
        if work.normal {
            state.normal += 1;
        }
        Ok(())
    }
    fn begin_shutdown(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closing {
            return;
        }
        state.closing = true;
        self.backend.begin_shutdown();
        let mut canceled = Vec::new();
        for lane in state.lanes.values_mut() {
            if let Some(work) = lane.queue.take() {
                canceled.push((work, Phase::SupersededBeforeCall));
            }
            if let Some((work, _)) = lane.readback.take() {
                canceled.push((work, Phase::CompletedReadbackFailed));
            }
            lane.refresh = false;
            lane.healthy = None;
        }
        if state
            .management
            .as_ref()
            .is_some_and(|w| w.request.method != "shutdown")
        {
            canceled.push((
                state.management.take().unwrap(),
                Phase::SupersededBeforeCall,
            ));
        }
        for (work, _) in &canceled {
            self.obligation_done(&mut state, work);
        }
        let mut contexts = Vec::new();
        let mut notifications = Vec::new();
        for snapshot in self.registry.snapshot() {
            if snapshot.context.connection_id.is_some() {
                contexts.push(snapshot.context);
            } else if let Ok(context) = self
                .registry
                .fence(snapshot.context.domain.as_ref().unwrap())
            {
                notifications.push(context);
            }
        }
        drop(state);
        for context in notifications {
            self.notify_stop(&context);
        }
        for (work, phase) in canceled {
            self.complete(
                &work,
                failure(
                    work.request.context.clone(),
                    phase,
                    "Closing",
                    "shutdown fences queued/readback work",
                ),
            );
        }
        for context in contexts {
            if let Ok(id) = new_id() {
                let _ = self.submit(
                    RequestV3 {
                        v: 3,
                        id,
                        method: "disconnect".into(),
                        params: json!({}),
                        context: Some(context),
                    },
                    true,
                );
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.maybe_terminate(&mut state);
        self.wake.notify_all();
    }
    fn notify_stop(&self, context: &ContextV3) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.backend.request_stop(context)
        }))
        .is_err()
        {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .callback_errors += 1;
        }
    }
    fn invoke(&self, request: &RequestV3) -> OutcomeV3 {
        let mut outcome = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.backend.execute(request)
        })) {
            Ok(outcome) => outcome,
            Err(_) => failure(
                request.context.clone(),
                Phase::FailedAfterCallStarted,
                "BackendPanic",
                "native backend call panicked; responsibility retained",
            ),
        };
        outcome.context = request.context.clone();
        if let Err(error) = encode_outcome(&request.id, &outcome) {
            return failure(
                request.context.clone(),
                Phase::CompletedReadbackFailed,
                "InvalidOutcome",
                &error.to_string(),
            );
        }
        outcome
    }
    fn run(self: Arc<Self>, pool: usize) {
        loop {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.terminated {
                self.wake.notify_all();
                return;
            }
            if pool == 0 {
                let selected = if state.closing {
                    None
                } else {
                    state
                        .lanes
                        .iter()
                        .filter(|(_, l)| {
                            !l.active
                                && !l.observing
                                && l.safety.is_none()
                                && l.readback.is_none()
                                && l.queue.is_some()
                        })
                        .min_by_key(|(_, l)| l.queue.as_ref().unwrap().sequence)
                        .map(|(k, _)| k.clone())
                };
                if let Some(key) = selected {
                    let lane = state.lanes.get_mut(&key).unwrap();
                    let work = lane.queue.take().unwrap();
                    lane.active = true;
                    lane.active_request_id = Some(work.request.id.clone());
                    state.active += 1;
                    drop(state);
                    let mut outcome = if work
                        .request
                        .context
                        .as_ref()
                        .is_some_and(|c| self.registry.matches(c))
                    {
                        self.invoke(&work.request)
                    } else {
                        failure(
                            work.request.context.clone(),
                            Phase::SupersededBeforeCall,
                            "StaleContext",
                            "context changed before call",
                        )
                    };
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.active -= 1;
                    let lane = state.lanes.get_mut(&key).unwrap();
                    lane.active = false;
                    lane.active_request_id = None;
                    let current = work
                        .request
                        .context
                        .as_ref()
                        .is_some_and(|c| self.registry.matches(c))
                        && !state.closing;
                    let lane = state.lanes.get_mut(&key).unwrap();
                    let deferred = current
                        && lane.safety.is_none()
                        && outcome.phase == Phase::Completed
                        && outcome
                            .result
                            .as_ref()
                            .is_some_and(|r| r["post_readback"] == true);
                    if deferred {
                        lane.readback = Some((work.clone(), outcome.clone()));
                        lane.refresh = true;
                    } else if current && lane.safety.is_none() {
                        if outcome.phase == Phase::Completed {
                            if work.request.params["name"] == "retry_staging" {
                                lane.state = if outcome
                                    .result
                                    .as_ref()
                                    .is_some_and(|v| v["connected"] == true)
                                {
                                    "READY"
                                } else if work
                                    .request
                                    .context
                                    .as_ref()
                                    .is_some_and(|c| c.connection_id.is_none())
                                {
                                    "DISCONNECTED"
                                } else {
                                    "RETAINED"
                                };
                            }
                            if work.request.method == "connect" {
                                lane.state = if outcome
                                    .result
                                    .as_ref()
                                    .is_some_and(|r| r["connected"] == true)
                                {
                                    "READY"
                                } else {
                                    "FAULT"
                                };
                            }
                            if let Some(result) = &outcome.result {
                                merge(&mut lane.status, &result["status"]);
                                if work.request.method=="connect" && lane.state=="READY" {
                                    lane.status.as_object_mut().unwrap().remove("observation_error");
                                }
                            }
                            lane.refresh = lane.state == "READY";
                            if let Some(context) = &work
                                .request
                                .context
                                .clone()
                                .filter(|_| work.request.params["name"] != "retry_staging")
                            {
                                self.registry.publish(
                                    context,
                                    if lane.state == "READY" {
                                        DriverState::Ready
                                    } else {
                                        DriverState::Fault
                                    },
                                );
                            }
                        } else if !matches!(
                            outcome.phase,
                            Phase::RejectedBeforeCall | Phase::SupersededBeforeCall
                        ) {
                            lane.state = "FAULT";
                            lane.refresh = true;
                        }
                    }
                    if !deferred {
                        self.obligation_done(&mut state, &work);
                        // Laser connect performs preserving cleanup on failure.
                        // Advance authority only after this request is quiescent;
                        // uncertain cleanup retains the original fault context.
                        if current
                            && work.request.method == "connect"
                            && state.lanes.get(&key).is_some_and(|l| l.driver == "laser" && l.safety.is_none())
                            && outcome.phase != Phase::Completed
                        {
                            let context = work.request.context.as_ref().unwrap();
                            let evidence = self.backend.take_failed_connect_cleanup(context);
                            if evidence.as_ref().is_some_and(|r| self.registry.release(context, r).is_ok()) {
                                outcome.context = self.registry.context(context.domain.as_ref().unwrap()).ok();
                                let lane = state.lanes.get_mut(&key).unwrap();
                                lane.state = "DISCONNECTED";
                                lane.status = json!({"connected":false,"state":"DISCONNECTED"});
                                lane.refresh = false;
                                lane.healthy = None;
                            }
                        }
                    }
                    drop(state);
                    if !deferred {
                        self.complete(&work, outcome);
                    }
                    self.wake.notify_all();
                    continue;
                }
            } else if pool == 1
                && state.outstanding + state.observing < self.limits.max_responsibility_slots
            {
                let now = self.clock.now();
                let selected = state
                    .lanes
                    .iter()
                    .find(|(_, l)| {
                        !l.observing
                            && !l.active
                            && l.safety.is_none()
                            && (l.readback.is_some()
                                || (!state.closing
                                    && l.queue.is_none()
                                    && matches!(
                                        l.state,
                                        "READY" | "STOP_HELD" | "FAULT" | "RETAINED"
                                    )
                                    && (l.refresh || now >= l.next_refresh)))
                    })
                    .map(|(k, _)| k.clone());
                if let Some(key) = selected {
                    let domain = self
                        .registry
                        .snapshot()
                        .into_iter()
                        .find(|d| key == crate::scheduler::key(d.context.domain.as_ref().unwrap()))
                        .unwrap()
                        .context
                        .domain
                        .unwrap();
                    let context = self.registry.context(&domain).unwrap();
                    let lane = state.lanes.get_mut(&key).unwrap();
                    lane.observing = true;
                    lane.refresh = false;
                    lane.next_refresh = now + Duration::from_millis(2500);
                    state.observing += 1;
                    drop(state);
                    let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        self.backend.observe(&context)
                    }))
                    .map_err(|_| {
                        WorkerError::new("ObservationPanic", "observation callback panicked")
                    })
                    .and_then(|o| {
                        o.validate(self.clock.now())?;
                        Ok(o)
                    });
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.observing -= 1;
                    let closing = state.closing;
                    let lane = state.lanes.get_mut(&key).unwrap();
                    lane.observing = false;
                    let mut completed = None;
                    if self.registry.matches(&context) && !closing && lane.safety.is_none() {
                        let (status, more, error) = match observed {
                            Ok(o) => {
                                let error = if o.more {
                                    None
                                } else {
                                    o.status.get("observation_error").cloned()
                                };
                                (o.status, o.more, error)
                            }
                            Err(e) => (
                                json!({}),
                                false,
                                Some(json!({"type":e.code,"message":e.message})),
                            ),
                        };
                        merge(&mut lane.status, &status);
                        if lane.driver=="laser" && status["state"]=="FAULT" {
                            lane.state="FAULT";
                            self.registry.publish(&context,DriverState::Fault);
                        }
                        lane.refresh = more;
                        lane.healthy = if !more
                            && error.is_none()
                            && matches!(status["state"].as_str(), Some("READY" | "ACTIVE"))
                        {
                            Some(context.clone())
                        } else {
                            None
                        };
                        if let Some(error) = &error {
                            lane.status["observation_error"] = error.clone();
                        } else if !more && matches!(status["state"].as_str(),Some("READY"|"ACTIVE")) {
                            lane.status.as_object_mut().unwrap().remove("observation_error");
                        }
                        if !more {
                            if let Some((work, mut outcome)) = lane.readback.take() {
                                if let Some(error) = error {
                                    outcome = failure(
                                        work.request.context.clone(),
                                        Phase::CompletedReadbackFailed,
                                        error["type"].as_str().unwrap(),
                                        error["message"].as_str().unwrap(),
                                    );
                                    lane.state = "FAULT";
                                } else if let Some(result) =
                                    outcome.result.as_mut().and_then(Value::as_object_mut)
                                {
                                    result.remove("post_readback");
                                    result.insert("status".into(), lane.status.clone());
                                }
                                completed = Some((work, outcome));
                            }
                        }
                    }
                    if let Some((work, _)) = &completed {
                        self.obligation_done(&mut state, work);
                    }
                    self.maybe_terminate(&mut state);
                    drop(state);
                    if let Some((work, outcome)) = completed {
                        self.complete(&work, outcome);
                    }
                    self.wake.notify_all();
                    continue;
                }
            } else if pool == 2 {
                let selected = state
                    .lanes
                    .iter()
                    .find(|(_, l)| {
                        !l.active
                            && l.pending_notifications == 0
                            && !l.observing
                            && l.readback.is_none()
                            && l.safety.as_ref().is_some_and(|g| !g.running)
                    })
                    .map(|(k, _)| k.clone());
                if let Some(key) = selected {
                    let group = state.lanes.get_mut(&key).unwrap().safety.as_mut().unwrap();
                    group.running = true;
                    let request = group.request.clone();
                    let intent = group.intent;
                    drop(state);
                    let mut outcome = self.invoke(&request);
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let lane = state.lanes.get_mut(&key).unwrap();
                    if lane.safety.as_ref().unwrap().intent != intent {
                        lane.safety.as_mut().unwrap().running = false;
                        drop(state);
                        self.wake.notify_all();
                        continue;
                    }
                    let group = lane.safety.take().unwrap();
                    for work in &group.waiters {
                        self.obligation_done(&mut state, work);
                    }
                    if intent == SafetyIntent::Disconnect && outcome.phase == Phase::Completed {
                        let evidence = outcome
                            .result
                            .as_ref()
                            .filter(|r| r["connected"] == false && r["data_retained"] != true)
                            .and_then(|r| {
                                serde_json::from_value::<CleanupReport>(r["cleanup"].clone()).ok()
                            });
                        let released = evidence.as_ref().is_some_and(|r| {
                            self.registry
                                .release(request.context.as_ref().unwrap(), r)
                                .is_ok()
                        });
                        if released {
                            outcome.context = Some(
                                self.registry
                                    .context(
                                        request.context.as_ref().unwrap().domain.as_ref().unwrap(),
                                    )
                                    .unwrap(),
                            );
                            state.lanes.get_mut(&key).unwrap().state = "DISCONNECTED";
                        } else {
                            outcome = failure(
                                request.context.clone(),
                                Phase::FailedAfterCallStarted,
                                "ReleaseUnconfirmed",
                                "disconnect did not confirm quiescent resource release",
                            );
                            state.lanes.get_mut(&key).unwrap().state = "RETAINED";
                        }
                    } else {
                        let lane = state.lanes.get_mut(&key).unwrap();
                        lane.state = if outcome.phase == Phase::Completed {
                            "STOP_HELD"
                        } else {
                            "RETAINED"
                        };
                        lane.refresh = outcome.phase == Phase::Completed;
                    }
                    if let Some(result) = outcome.result.as_mut().and_then(Value::as_object_mut) {
                        result.insert("attempt_id".into(), json!(group.attempt_id));
                        result.insert(
                            "effective_intent".into(),
                            json!(match intent {
                                SafetyIntent::Zero => "zero",
                                SafetyIntent::CurrentOff => "current_off",
                                SafetyIntent::TecOff => "tec_off",
                                SafetyIntent::Disconnect => "disconnect",
                            }),
                        );
                    }
                    if let Some(error) = &mut outcome.error {
                        error.attempt_id = Some(group.attempt_id.clone());
                    }
                    let lane = state.lanes.get_mut(&key).unwrap();
                    lane.last_safety = Some(
                        json!({"state":lane.state,"attempt_id":group.attempt_id,
                        "request_id":group.request.id,"context":outcome.context,"phase":outcome.phase,
                        "result":outcome.result,"error":outcome.error}),
                    );
                    drop(state);
                    for work in group.waiters {
                        self.complete(&work, outcome.clone());
                    }
                    self.wake.notify_all();
                    continue;
                }
            } else if pool == 3 && !state.management_active {
                let ready = state.management.as_ref().is_some_and(|w| {
                    if w.request.method == "shutdown" {
                        state.active == 0
                            && state.observing == 0
                            && state.lanes.values().all(|l| l.safety.is_none())
                    } else {
                        w.request
                            .context
                            .as_ref()
                            .and_then(|c| c.domain.as_ref())
                            .and_then(|d| state.lanes.get(&key(d)))
                            .is_none_or(|l| !l.active && !l.observing && l.safety.is_none())
                    }
                });
                if ready {
                    let work = state.management.take().unwrap();
                    state.management_active = true;
                    state.management_method = Some(work.request.method.clone());
                    let domain_key = work
                        .request
                        .context
                        .as_ref()
                        .and_then(|c| c.domain.as_ref())
                        .map(key);
                    if let Some(key) = &domain_key {
                        state.lanes.get_mut(key).unwrap().active = true;
                        state.lanes.get_mut(key).unwrap().active_request_id =
                            Some(work.request.id.clone());
                    }
                    drop(state);
                    let current = work
                        .request
                        .context
                        .as_ref()
                        .is_none_or(|c| c.domain.is_none() || self.registry.matches(c));
                    let outcome = if current {
                        self.invoke(&work.request)
                    } else {
                        failure(
                            work.request.context.clone(),
                            Phase::SupersededBeforeCall,
                            "StaleContext",
                            "management context changed before call",
                        )
                    };
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.management_active = false;
                    state.management_method = None;
                    if let Some(key) = &domain_key {
                        state.lanes.get_mut(key).unwrap().active = false;
                        state.lanes.get_mut(key).unwrap().active_request_id = None;
                    }
                    let _ = self.sync_lanes(&mut state);
                    self.obligation_done(&mut state, &work);
                    drop(state);
                    self.complete(&work, outcome);
                    self.wake.notify_all();
                    continue;
                }
            }
            let _ = self
                .wake
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}
struct RetainedThreads {
    _core: Arc<Core>,
    _handles: Vec<JoinHandle<()>>,
}
fn retain_threads(core: Arc<Core>, handles: Vec<JoinHandle<()>>) {
    static RETAINED: OnceLock<Mutex<Vec<RetainedThreads>>> = OnceLock::new();
    RETAINED
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(RetainedThreads {
            _core: core,
            _handles: handles,
        });
}
impl Drop for Scheduler {
    fn drop(&mut self) {
        self.begin_shutdown();
        let handles = std::mem::take(self.threads.get_mut().unwrap_or_else(|e| e.into_inner()));
        if !handles.is_empty() {
            retain_threads(self.core.clone(), handles);
        }
    }
}
