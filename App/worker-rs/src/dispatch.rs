use crate::{
    backend::{completed, failed},
    captures::CaptureSpool,
    scheduler::{Backend, PendingOutcome, Scheduler},
    WorkerError,
};
use std::{
    collections::VecDeque,
    io::{BufReader, Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    thread::JoinHandle,
    time::Duration,
};
use yang_drivers::lifecycle::CleanupReport;
use yang_drivers::transport::Deadline;
use yang_protocol::{
    encode_outcome, parse_request, read_frame, ContextV3, NativeIdentity, OutcomeV3, Phase,
    RequestV3, MAX_REQUEST_BYTES,
};
#[derive(Clone, Debug, serde::Serialize)]
pub struct ShutdownReceipt {
    pub reports: Vec<CleanupReport>,
    pub all_resources_released: bool,
    pub process_lifecycle: String,
    pub pending_replies: usize,
}
struct Reply {
    bytes: Vec<u8>,
    pending: Option<PendingOutcome>,
}
#[derive(Default)]
struct Delivery {
    queue: VecDeque<Reply>,
    pending: usize,
    closing: bool,
    failed: bool,
}
pub struct Worker {
    backend: Arc<dyn Backend>,
    scheduler: Arc<Scheduler>,
    _spool: Arc<Mutex<CaptureSpool>>,
    nonce: String,
    identity: NativeIdentity,
    activated: AtomicBool,
    started: AtomicBool,
    delivery: Arc<(Mutex<Delivery>, Condvar)>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    seen: Mutex<VecDeque<String>>,
    shutdown_request: Mutex<Option<(String, Option<ContextV3>)>>,
}
struct Retained {
    _backend: Arc<dyn Backend>,
    _scheduler: Arc<Scheduler>,
    _delivery: Arc<(Mutex<Delivery>, Condvar)>,
    _threads: Vec<JoinHandle<()>>,
}
fn retained() -> &'static Mutex<Vec<Retained>> {
    static RETAINED: OnceLock<Mutex<Vec<Retained>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(Vec::new()))
}
impl Worker {
    /// Continue only the already-requested cleanup; never reconnect or replay
    /// a measurement/action. A blocked delivery still owns its reply slot.
    pub fn retry_shutdown(&self) -> ShutdownReceipt {
        self.scheduler.continue_shutdown();
        let joined = self
            .scheduler
            .join_when_released(Deadline::after(Duration::from_secs(2)))
            .is_ok();
        let released = joined
            && self.backend.finish_shutdown().is_ok()
            && !self.backend.auxiliary_responsibility();
        let pending = self.delivery.0.lock().unwrap().pending;
        ShutdownReceipt {
            reports: self.backend.cleanup_reports(),
            all_resources_released: released && pending == 0,
            process_lifecycle: if released && pending == 0 {
                "native_work_settled"
            } else {
                "responsibility_retained"
            }
            .into(),
            pending_replies: pending,
        }
    }
    pub fn new(backend: Arc<dyn Backend>, scheduler: Scheduler, spool: CaptureSpool) -> Self {
        let nonce = spool.ownership_nonce().to_owned();
        let spool = Arc::new(Mutex::new(spool));
        backend.install_spool(spool.clone());
        let identity = NativeIdentity {
            startup_revision: 1,
            worker_kind: "rust".into(),
            executable: std::env::current_exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "unavailable-native-image".into()),
            package_revision: option_env!("YANG_PACKAGE_REVISION")
                .unwrap_or(env!("CARGO_PKG_VERSION"))
                .into(),
            protocol_version: 3,
            mode: "real".into(),
            session_id: backend.registry().session_id().into(),
            activated: false,
            connected: false,
            domains: vec![],
        };
        Self {
            backend,
            scheduler: Arc::new(scheduler),
            _spool: spool,
            nonce,
            identity,
            activated: AtomicBool::new(false),
            started: AtomicBool::new(false),
            delivery: Arc::new((Mutex::new(Delivery::default()), Condvar::new())),
            threads: Mutex::new(Vec::new()),
            seen: Mutex::new(VecDeque::new()),
            shutdown_request: Mutex::new(None),
        }
    }
    fn context(&self) -> ContextV3 {
        ContextV3 {
            session_id: self.identity.session_id.clone(),
            domain: None,
            connection_id: None,
            epoch: 0,
        }
    }
    fn identity_result(&self, mut status: serde_json::Value) -> serde_json::Value {
        let mut identity = serde_json::to_value(&self.identity).unwrap();
        identity["activated"] = self.activated.load(Ordering::Acquire).into();
        if self.activated.load(Ordering::Acquire) {
            identity["connected"] = self
                .backend
                .registry()
                .snapshot()
                .iter()
                .any(|d| d.responsibility)
                .into();
            if let Some(fields) = status.as_object_mut() {
                for (key, value) in fields.iter() {
                    identity[key] = value.clone();
                }
            }
        }
        identity
    }
    fn publish(&self, id: &str, outcome: OutcomeV3, pending: Option<PendingOutcome>) {
        queue_reply(&self.delivery, id, outcome, pending);
    }
    fn accept(&self, request: RequestV3) -> Result<bool, WorkerError> {
        {
            let mut delivery = self.delivery.0.lock().unwrap();
            if delivery.pending >= 226 || delivery.failed {
                return Err(WorkerError::new(
                    "ReplyCapacity",
                    "Unsent outcomes retain accepted responsibility",
                ));
            }
            delivery.pending += 1;
        }
        let context = Some(self.context());
        let mut seen = self.seen.lock().unwrap();
        if seen.contains(&request.id) {
            drop(seen);
            self.publish(
                &request.id,
                failed(
                    context,
                    Phase::RejectedBeforeCall,
                    WorkerError::new("DuplicateRequest", "Request identity already consumed"),
                ),
                None,
            );
            return Ok(false);
        }
        seen.push_back(request.id.clone());
        if seen.len() > 256 {
            seen.pop_front();
        }
        drop(seen);
        if request.method == "shutdown" {
            if request.context.as_ref() != Some(&self.context()) {
                self.publish(
                    &request.id,
                    failed(
                        context,
                        Phase::RejectedBeforeCall,
                        WorkerError::new(
                            "ContextMismatch",
                            "Shutdown belongs to another session/context",
                        ),
                    ),
                    None,
                );
                return Ok(false);
            }
            *self.shutdown_request.lock().unwrap() = Some((request.id, request.context));
            return Ok(true);
        }
        if request.method == "activate" {
            let valid = request.context.as_ref() == Some(&self.context())
                && request.params["ownership_nonce"].as_str() == Some(self.nonce.as_str());
            let outcome = if valid
                && self
                    .activated
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                completed(context, serde_json::json!({"activated":true}))
            } else {
                failed(
                    context,
                    Phase::RejectedBeforeCall,
                    WorkerError::new("OwnershipRejected", "Wrong, stale or consumed activation"),
                )
            };
            self.publish(&request.id, outcome, None);
            return Ok(false);
        }
        if !self.activated.load(Ordering::Acquire) {
            let matching =
                request.context.is_none() || request.context.as_ref() == Some(&self.context());
            let outcome = if matching && matches!(request.method.as_str(), "ping" | "status") {
                completed(context, self.identity_result(serde_json::json!({})))
            } else if matching && request.method == "shutdown" {
                completed(context, serde_json::json!({"shutdown_requested":true}))
            } else {
                failed(
                    context,
                    Phase::RejectedBeforeCall,
                    WorkerError::new(
                        "OwnershipRequired",
                        "Activation and current startup context required",
                    ),
                )
            };
            let shutdown = request.method == "shutdown" && matching;
            self.publish(&request.id, outcome, None);
            return Ok(shutdown);
        }
        let id = request.id.clone();
        let status = matches!(request.method.as_str(), "ping" | "status");
        let shutdown = request.method == "shutdown";
        match self.scheduler.submit(request) {
            Ok(pending) => {
                let delivery = self.delivery.clone();
                let copy = pending.clone();
                let identity = self.identity.clone();
                let registry = self.backend.registry();
                pending.on_complete(move |mut outcome| {
                    if status && outcome.phase == Phase::Completed {
                        let mut merged = serde_json::to_value(identity).unwrap();
                        merged["activated"] = true.into();
                        merged["connected"] =
                            registry.snapshot().iter().any(|d| d.responsibility).into();
                        if let Some(value) = outcome.result.take() {
                            if let Some(fields) = value.as_object() {
                                for (key, value) in fields {
                                    merged[key] = value.clone();
                                }
                            }
                        }
                        outcome.result = Some(merged);
                    }
                    queue_reply(&delivery, &id, outcome, Some(copy));
                });
            }
            Err(error) => {
                self.publish(&id, failed(context, Phase::RejectedBeforeCall, error), None)
            }
        }
        Ok(shutdown)
    }
    pub fn run_io(
        &self,
        reader: impl Read + Send + 'static,
        mut writer: impl Write + Send + 'static,
    ) -> Result<ShutdownReceipt, WorkerError> {
        if self.started.swap(true, Ordering::AcqRel) {
            return Err(WorkerError::new(
                "WorkerStarted",
                "stdio can be owned only once",
            ));
        }
        let delivery = self.delivery.clone();
        let output = std::thread::Builder::new()
            .name("yang-protocol-output".into())
            .spawn(move || loop {
                let mut state = delivery.0.lock().unwrap();
                while state.queue.is_empty() && !(state.closing && state.pending == 0) {
                    state = delivery.1.wait(state).unwrap();
                }
                if state.closing && state.pending == 0 {
                    return;
                }
                let reply = state.queue.pop_front().unwrap();
                drop(state);
                if writer
                    .write_all(&reply.bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    let mut state = delivery.0.lock().unwrap();
                    state.failed = true;
                    state.queue.push_front(reply);
                    delivery.1.notify_all();
                    return;
                }
                if let Some(pending) = reply.pending {
                    pending.acknowledge();
                }
                let mut state = delivery.0.lock().unwrap();
                state.pending -= 1;
                delivery.1.notify_all();
            })
            .map_err(|e| WorkerError::new("ProtocolOutput", e.to_string()))?;
        self.threads.lock().unwrap().push(output);
        let (tx, rx) = std::sync::mpsc::sync_channel(32);
        let input = std::thread::Builder::new()
            .name("yang-protocol-input".into())
            .spawn(move || {
                let mut reader = BufReader::with_capacity(4096, reader);
                loop {
                    let frame =
                        read_frame(&mut reader, MAX_REQUEST_BYTES).map_err(WorkerError::from);
                    let done = !matches!(frame, Ok(Some(_)));
                    if tx.send(frame).is_err() || done {
                        return;
                    }
                }
            })
            .map_err(|e| WorkerError::new("ProtocolInput", e.to_string()))?;
        self.threads.lock().unwrap().push(input);
        loop {
            if self.delivery.0.lock().unwrap().failed {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(Some(bytes))) => match parse_request(&bytes) {
                    Ok(request) => match self.accept(request) {
                        Ok(true) => break,
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("{error}; admission stopped");
                            break;
                        }
                    },
                    Err(error) => {
                        eprintln!("Protocol input rejected: {error}");
                        break;
                    }
                },
                Ok(Ok(None)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(Err(error)) => {
                    eprintln!("{error}");
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        self.scheduler.begin_shutdown();
        let joined = self
            .scheduler
            .join_when_released(Deadline::after(Duration::from_secs(2)))
            .is_ok();
        let auxiliary_released = joined
            && self.backend.finish_shutdown().is_ok()
            && !self.backend.auxiliary_responsibility();
        if let Some((id, context)) = self.shutdown_request.lock().unwrap().take() {
            let outcome = if auxiliary_released {
                completed(
                    context,
                    serde_json::json!({"all_resources_released":true,"cleanup_reports":self.backend.cleanup_reports(),"process_lifecycle":"native_work_settled","physical_zero_verified":false}),
                )
            } else {
                failed(
                    context,
                    Phase::FailedAfterCallStarted,
                    WorkerError::new(
                        "ReleaseUnconfirmed",
                        "Native cleanup remains owned; process must be retained",
                    ),
                )
            };
            self.publish(&id, outcome, None);
        }
        let mut delivery = self.delivery.0.lock().unwrap();
        delivery.closing = true;
        self.delivery.1.notify_all();
        let deadline = std::time::Instant::now() + Duration::from_millis(250);
        while delivery.pending != 0 && !delivery.failed && std::time::Instant::now() < deadline {
            delivery = self
                .delivery
                .1
                .wait_timeout(delivery, Duration::from_millis(5))
                .unwrap()
                .0;
        }
        let pending = delivery.pending;
        drop(delivery);
        Ok(ShutdownReceipt {
            reports: self.backend.cleanup_reports(),
            all_resources_released: auxiliary_released && pending == 0,
            process_lifecycle: if auxiliary_released && pending == 0 {
                "native_work_settled"
            } else {
                "responsibility_retained"
            }
            .into(),
            pending_replies: pending,
        })
    }
}
fn queue_reply(
    delivery: &Arc<(Mutex<Delivery>, Condvar)>,
    id: &str,
    outcome: OutcomeV3,
    pending: Option<PendingOutcome>,
) {
    let encoded = encode_outcome(id, &outcome).or_else(|error| {
        encode_outcome(
            id,
            &failed(
                outcome.context.clone(),
                Phase::CompletedReadbackFailed,
                WorkerError::from(error),
            ),
        )
    });
    let mut state = delivery.0.lock().unwrap();
    match encoded {
        Ok(bytes) => state.queue.push_back(Reply { bytes, pending }),
        Err(_) => state.failed = true,
    }
    delivery.1.notify_all();
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.scheduler.begin_shutdown();
        if self.delivery.0.lock().unwrap().pending != 0
            || self.backend.auxiliary_responsibility()
            || self
                .backend
                .registry()
                .snapshot()
                .iter()
                .any(|d| d.responsibility)
        {
            retained().lock().unwrap().push(Retained {
                _backend: self.backend.clone(),
                _scheduler: self.scheduler.clone(),
                _delivery: self.delivery.clone(),
                _threads: std::mem::take(self.threads.get_mut().unwrap()),
            });
        }
    }
}
