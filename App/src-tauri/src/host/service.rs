use super::leases::{power_generation, PowerObserver};
use super::leases::{CleanupTicket, DomainRef, LeaseBook, ReleaseEvidence, RevokeCause, Session};
use super::operations::{
    ExecuteParams, OperationActor, OperationBook, OperationRecord, SafetyAudit,
};
use super::{
    contracts::HostError,
    instance::{InstanceGuard, StartupRecord},
    ipc::{
        parse_request, read_frame, write_frame, write_frame_limit, HostReply, HostRequest,
        PipeSecurity, MAX_CHANNELS,
    },
    registry::{new_id, Registry},
};
use crate::runtime::{RuntimeConfig, WorkerRequest, WorkerRuntime};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    net::windows::named_pipe::NamedPipeServer,
    sync::{Notify, Semaphore},
};

pub struct HostConfig {
    pub record_dir: PathBuf,
    pub python: PathBuf,
    pub root: PathBuf,
    pub mode: String,
}
pub struct HostService {
    _tray: super::tray::HostTray,
    _power: PowerObserver,
    _guard: InstanceGuard,
    executor: tokio::runtime::Runtime,
    security: PipeSecurity,
    endpoint: String,
    first: Option<NamedPipeServer>,
    core: Arc<HostCore>,
}
pub(crate) struct HostCore {
    tray_status: Arc<Mutex<super::tray::HostStatus>>,
    tray_stop: Arc<AtomicBool>,
    clients: Mutex<super::sessions::ClientSessions>,
    events: super::events::EventHub,
    results: super::results::ResultActor,
    archive: Option<super::archive::ArchiveActor>,
    archive_error: Option<HostError>,
    archive_root: PathBuf,
    status_cache: Mutex<Option<(Instant, Value)>>,
    checks: Mutex<super::checks::Checks>,
    status_failed: AtomicBool,
    power_seen: std::sync::atomic::AtomicU64,
    pub(crate) boot_id: String,
    pub(crate) leases: Arc<Mutex<LeaseBook>>,
    pub(crate) bindings: Arc<Mutex<BTreeMap<DomainRef, Value>>>,
    cleanup_attempted: Mutex<BTreeSet<String>>,
    cleanup_results: Mutex<BTreeMap<String, Value>>,
    query_gate: Arc<tokio::sync::Mutex<()>>,
    operations: Arc<OperationActor>,
    safety_audit: SafetyAudit,
    ordinary_admission: tokio::sync::Mutex<()>,
    ordinary_capacity: Arc<Semaphore>,
    cleanup_running: Mutex<BTreeSet<String>>,
    pub(crate) registry: Arc<Mutex<super::contracts::RegistrySnapshot>>,
    configuration: super::configuration::Configuration,
    verification_control: Arc<Mutex<BTreeMap<String, super::configuration::VerificationControl>>>,
    verification_port: super::configuration::WorkerVerificationPort,
    pub(crate) worker: Arc<WorkerRuntime>,
    record: Arc<Mutex<StartupRecord>>,
    pub(crate) mode: String,
    startup_error: Option<String>,
    stopped: AtomicBool,
    stopping: AtomicBool,
    event_streams: AtomicUsize,
    wake: Notify,
}
struct EventStreamGuard(Arc<HostCore>);
impl Drop for EventStreamGuard {
    fn drop(&mut self) {
        self.0.event_streams.fetch_sub(1, Ordering::AcqRel);
    }
}
impl HostService {
    pub fn start(config: HostConfig) -> Result<Self, HostError> {
        if config.mode != "real" {
            return Err(HostError::new(
                "HostMode",
                "Only real hardware is supported",
            ));
        }
        let guard = InstanceGuard::acquire(&config.record_dir)?;
        // Reject known pre-spawn failures before persisting an ownership intent.
        // An unresolved record from an earlier attempt still wins above.
        let (python, root) = crate::runtime::resolve_startup_paths(&config.python, &config.root)
            .map_err(|error| HostError::new("WorkerStartup", error))?;
        let registry = Registry::open(&config.record_dir.join("devices.json"))?;
        registry.snapshot()?;
        let boot_id = new_id()?;
        let operations =
            OperationBook::open(&config.record_dir.join("operations.json"), boot_id.clone())?;
        let operations = Arc::new(OperationActor::new(operations)?);
        let safety_audit = SafetyAudit::new(config.record_dir.join("safety-audit.json"))?;
        let security = PipeSecurity::current()?;
        let endpoint = security.endpoint();
        let executor = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(4)
            .enable_all()
            .build()
            .map_err(|error| HostError::new("HostRuntime", error.to_string()))?;
        // Pipe ownership must be established before any child spawn/activation.
        let first = {
            let _entered = executor.enter();
            security.create_server(&endpoint, true)?
        };
        let tray = super::tray::HostTray::install(
            &registry.snapshot()?.host_id,
            super::tray::HostStatus {
                mode: config.mode.clone(),
                state: "STARTING".into(),
            },
            super::tray::ReopenTarget {
                executable: std::env::current_exe()
                    .map_err(|e| HostError::new("HostTray", e.to_string()))?
                    .parent()
                    .ok_or_else(|| HostError::new("HostTray", "Executable directory missing"))?
                    .join(if cfg!(test) {
                        "../sil-instrument-console.exe"
                    } else {
                        "sil-instrument-console.exe"
                    }),
            },
        )?;
        let power = PowerObserver::register()?;
        let record = Arc::new(Mutex::new(StartupRecord::intent(&config.record_dir)?));
        let nonce = record.lock().unwrap().nonce().to_string();
        let callback = record.clone();
        let runtime = WorkerRuntime::spawn(RuntimeConfig {
            python,
            root,
            mode: config.mode.clone(),
            protocol: 3,
            ownership_nonce: Some(nonce),
            record_child: Some(Arc::new(move |identity| {
                callback
                    .lock()
                    .unwrap()
                    .identified(identity)
                    .map_err(|error| error.to_string())
            })),
        });
        let (worker, mut startup_error) = match runtime {
            Ok(worker) => (worker, None),
            Err(error) => match error.retained_runtime {
                Some(worker) => (worker, Some(error.message)),
                None => return Err(HostError::new("WorkerStartup", error.message)),
            },
        };
        let mut leases = LeaseBook::new(&boot_id)?;
        let mut bindings = BTreeMap::new();
        if startup_error.is_none() {
            let initial = registry.snapshot()?;
            let mut configs = initial
                .devices
                .iter()
                .filter(|d| {
                    super::contracts::verification_mode_matches(&d.verified_mode, &config.mode)
                        .is_ok()
                        && !initial
                            .setups
                            .iter()
                            .any(|s| s.members.contains(&d.device_id))
                })
                .map(super::configuration::device_wire)
                .collect::<Result<Vec<_>, _>>()?;
            for setup in &initial.setups {
                if setup.members.iter().all(|id| {
                    initial.devices.iter().any(|d| {
                        &d.device_id == id
                            && super::contracts::verification_mode_matches(
                                &d.verified_mode,
                                &config.mode,
                            )
                            .is_ok()
                    })
                }) {
                    configs.push(super::configuration::setup_wire(setup, &initial)?);
                }
            }
            for config in configs {
                let domain = DomainRef {
                    kind: config["domain"]["kind"].as_str().unwrap().into(),
                    id: config["domain"]["id"].as_str().unwrap().into(),
                };
                let request = json!({"v":3,"id":new_id()?,"method":"configure_domain","params":{"config":config},
                    "context":worker.global_context().map_err(|error|HostError::new("WorkerStartup",error))?});
                match worker
                    .submit(WorkerRequest::V3(request))
                    .and_then(|pending| pending.wait(Duration::from_secs(30)))
                {
                    Ok(reply) if reply["ok"] == true => {
                        leases.register(&domain)?;
                        bindings.insert(domain, reply["result"]["context"].clone());
                    }
                    Ok(reply) => {
                        startup_error = Some(format!("Domain configuration rejected: {reply}"));
                        break;
                    }
                    Err(error) => {
                        startup_error = Some(error.message);
                        break;
                    }
                }
            }
        }
        let leases = Arc::new(Mutex::new(leases));
        let bindings = Arc::new(Mutex::new(bindings));
        let verification_control = Arc::new(Mutex::new(BTreeMap::new()));
        let query_gate = Arc::new(tokio::sync::Mutex::new(()));
        let verification_port = super::configuration::WorkerVerificationPort {
            query_gate: query_gate.clone(),
            worker: worker.clone(),
            leases: leases.clone(),
            bindings: bindings.clone(),
            control: verification_control.clone(),
            operations: operations.clone(),
            proofs: Arc::new(Mutex::new(BTreeMap::new())),
            retired: Arc::new(Mutex::new(BTreeMap::new())),
        };
        let configuration = super::configuration::Configuration::new(
            registry,
            Box::new(verification_port.clone()),
        )?;
        let registry = configuration.cache.clone();
        let host_id = registry.lock().unwrap().host_id.clone();
        let events = super::events::EventHub::new(host_id.clone(), boot_id.clone(), json!({}))?;
        let results = super::results::ResultActor::new(super::results::ResultStore::open(
            &config.root.join("Result/console").join(&host_id),
            boot_id.clone(),
        )?)?;
        let archive_root = registry
            .lock()
            .unwrap()
            .settings
            .data_root
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| config.root.join("Result"));
        // Storage failure is visible but cannot prevent status or safe shutdown.
        let (archive, archive_error) =
            match super::archive::ArchiveStore::open(&archive_root, &host_id)
                .and_then(super::archive::ArchiveActor::new)
            {
                Ok(actor) => (Some(actor), None),
                Err(error) => (None, Some(error)),
            };
        let core = Arc::new(HostCore {
            tray_status: tray.status.clone(),
            tray_stop: tray.stop.clone(),
            clients: Mutex::new(super::sessions::ClientSessions::new(boot_id.clone())),
            events,
            results,
            archive,
            archive_error,
            archive_root,
            status_cache: Mutex::new(None),
            checks: Mutex::new(super::checks::Checks::default()),
            status_failed: AtomicBool::new(false),
            power_seen: std::sync::atomic::AtomicU64::new(power_generation()),
            boot_id,
            leases,
            bindings,
            cleanup_attempted: Mutex::new(BTreeSet::new()),
            cleanup_results: Mutex::new(BTreeMap::new()),
            query_gate,
            operations,
            safety_audit,
            ordinary_admission: tokio::sync::Mutex::new(()),
            ordinary_capacity: Arc::new(Semaphore::new(31)),
            cleanup_running: Mutex::new(BTreeSet::new()),
            registry,
            configuration,
            verification_control,
            verification_port,
            worker,
            record,
            mode: config.mode,
            startup_error,
            stopped: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            event_streams: AtomicUsize::new(0),
            wake: Notify::new(),
        });
        core.events.update(core.snapshot()?)?;
        Ok(Self {
            _tray: tray,
            _power: power,
            _guard: guard,
            executor,
            security,
            endpoint,
            first: Some(first),
            core,
        })
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn serve(mut self) -> Result<(), HostError> {
        let mut server = self.first.take().unwrap();
        let security = self.security.clone();
        let endpoint = self.endpoint.clone();
        let core = self.core.clone();
        self.executor.block_on(async move {
            let capacity = Arc::new(Semaphore::new(MAX_CHANNELS));
            let poll_core = core.clone();
            tokio::spawn(async move {
                while !poll_core.stopped.load(Ordering::Acquire) {
                    poll_core
                        .status_failed
                        .store(poll_core.worker_cache().await.is_err(), Ordering::Release);
                    if let Ok(snapshot) = poll_core.snapshot() {
                        let _ = poll_core.events.update(snapshot);
                    }
                    for session in poll_core.events.take_closed_sessions() {
                        let _ = poll_core.leases.lock().unwrap().close_session(&session);
                    }
                    poll_core.schedule_cleanups();
                    tokio::time::sleep(Duration::from_millis(2500)).await;
                }
            });
            let check_core = core.clone();
            tokio::spawn(async move {
                while !check_core.stopped.load(Ordering::Acquire) {
                    check_core.check_next().await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            });
            let timer_core = core.clone();
            tokio::spawn(async move {
                let mut last = Instant::now();
                while !timer_core.stopped.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    timer_core.sync_power_fence();
                    if timer_core.tray_stop.swap(false, Ordering::AcqRel) {
                        let request = HostRequest {
                            v: 1,
                            id: new_id().unwrap_or_default(),
                            method: "stop".into(),
                            params: json!({"confirm":true}),
                        };
                        let core = timer_core.clone();
                        tokio::spawn(async move {
                            match core.dispatch(&request, &core.boot_id).await {
                                Ok(_) => core.finalize_stop(),
                                Err(error) => {
                                    core.tray_status.lock().unwrap().state =
                                        format!("RETAINED: {}", error.code)
                                }
                            }
                        });
                    }
                    if timer_core
                        .tray_status
                        .lock()
                        .unwrap()
                        .state
                        .starts_with("STARTING")
                    {
                        timer_core.tray_status.lock().unwrap().state =
                            if timer_core.startup_error.is_some() {
                                "RETAINED"
                            } else {
                                "RUNNING"
                            }
                            .into();
                    }
                    let now;
                    {
                        let mut book = timer_core.leases.lock().unwrap();
                        now = Instant::now();
                        if now
                            .checked_duration_since(last)
                            .map_or(true, |gap| gap >= Duration::from_secs(10))
                        {
                            let _ = book.system_resume();
                        }
                        let _ = book.tick(now);
                    }
                    last = now;
                    timer_core.schedule_cleanups();
                }
            });
            while !core.stopped.load(Ordering::Acquire) {
                // timeout is a cancellation-aware async wait, not PIPE_NOWAIT polling.
                match tokio::time::timeout(Duration::from_millis(250), server.connect()).await {
                    Err(_) => continue,
                    Ok(Err(error)) => return Err(HostError::new("LocalIpc", error.to_string())),
                    Ok(Ok(())) => {}
                }
                let next = security.create_server(&endpoint, false)?;
                let mut connected = std::mem::replace(&mut server, next);
                if security.verify_client(&connected).is_err() {
                    drop(connected);
                    continue;
                }
                let permit = match capacity.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        let reply = HostReply::from_result(
                            String::new(),
                            Err(HostError::new(
                                "ClientCapacity",
                                "Host allows at most 16 clients",
                            )),
                        );
                        let _ = tokio::time::timeout(
                            Duration::from_secs(1),
                            write_frame(&mut connected, &reply),
                        )
                        .await;
                        // Let the rejected client consume its terminal frame before
                        // closing, but never let it monopolize listener progress.
                        let _ = tokio::time::timeout(
                            Duration::from_secs(1),
                            read_frame(&mut connected),
                        )
                        .await;
                        continue;
                    }
                };
                let client_core = core.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = serve_client(connected, client_core.clone()).await;
                });
            }
            // All resources and the worker are already released. Keep only the
            // authenticated event streams alive briefly to deliver that evidence.
            let deadline = Instant::now() + Duration::from_secs(5);
            while core.event_streams.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(())
        })
    }
}
impl HostCore {
    async fn check_next(self: &Arc<Self>) {
        if self.startup_error.is_some()
            || self.stopping.load(Ordering::Acquire)
            || self.worker.transport_failure().is_some()
        {
            return;
        }
        let intent = {
            let _ordered = self.ordinary_admission.lock().await;
            let devices = self.registry.lock().unwrap().devices.clone();
            let contexts = self
                .bindings
                .lock()
                .unwrap()
                .iter()
                .map(|(d, c)| {
                    (
                        d.clone(),
                        self.worker
                            .domain_context(&d.key())
                            .unwrap_or_else(|| c.clone()),
                    )
                })
                .collect();
            let control = self.leases.lock().unwrap().snapshot();
            self.checks
                .lock()
                .unwrap()
                .begin(
                    &devices,
                    &self.mode,
                    &contexts,
                    &control,
                    self.events.monotonic_ms(),
                    |key| self.worker.domain_pending(key) != 0,
                )
                .ok()
                .flatten()
        };
        let Some(i) = intent else { return };
        let domain = DomainRef {
            kind: "device".into(),
            id: i.device.device_id.clone(),
        };
        let result:Result<Value,HostError>=async {
            let config=super::configuration::device_wire(&i.device)?;
            let (method,params,context)=if i.stage==super::availability::CheckStage::Enumeration {
                ("inventory",json!({}),self.worker.global_context().map_err(|e|HostError::new("CheckUnknown",e))?)
            } else {
                let digest=super::verification::sha256_bytes(&serde_json::to_vec(&config).map_err(|e|HostError::new("CheckInvalid",e.to_string()))?)?;
                ("check_online",json!({"authorization":{"accepted":true,"stage":"readonly","supervised":false,"retain_session":false,
                    "binding":{"mode":self.mode,"domain":domain,"config_rev":i.device.config_rev,"model_id":i.device.model_id,"profile_id":i.device.profile_id,"config_digest":digest}}}),i.context.clone())
            };
            let request=json!({"v":3,"id":new_id()?,"method":method,"params":params,"context":context});
            let _query=if i.stage==super::availability::CheckStage::Enumeration {Some(self.query_gate.lock().await)}else{None};
            self.worker.submit(WorkerRequest::V3(request)).map_err(|e|HostError::new("CheckUnknown",e.message))?.wait_async(Duration::from_secs(35)).await.map_err(|e|HostError::new("CheckUnknown",e.message))
        }.await;
        let (success, detected, retained) = match &result {
            Ok(r) if r["ok"] == true && i.stage == super::availability::CheckStage::Readonly => (
                r["result"]["release_confirmed"] == true,
                None,
                r["result"]["release_confirmed"] != true,
            ),
            Ok(r) if r["ok"] == true => {
                let address = i.device.params["resource"]
                    .as_str()
                    .or_else(|| i.device.params["port"].as_str())
                    .unwrap_or("");
                let detected = r["result"]["visa"]
                    .as_array()
                    .is_some_and(|v| v.iter().any(|p| p.as_str() == Some(address)))
                    || r["result"]["serial"]
                        .as_array()
                        .is_some_and(|v| v.iter().any(|p| p["resource"] == address));
                // PnP metadata cannot associate a physical USB path with the
                // Newport controller serial. Do not invent per-device detection.
                (true, if i.device.model_id == "tlb6700" { None } else { Some(detected) }, false)
            }
            _ => (false, None, false),
        };
        let records = self.registry.lock().unwrap().devices.clone();
        let current = self.worker.domain_context(&domain.key());
        self.checks.lock().unwrap().finish(
            &i,
            records.iter().find(|d| d.device_id == domain.id),
            current.as_ref(),
            self.events.monotonic_ms(),
            success,
            detected,
            retained,
        );
        if let Ok(snapshot) = self.snapshot() {
            let _ = self.events.update(snapshot);
        }
    }
    fn sync_power_fence(&self) {
        let current = power_generation();
        if self.power_seen.swap(current, Ordering::AcqRel) != current {
            let _ = self.leases.lock().unwrap().system_resume();
        }
    }
    fn schedule_cleanups(self: &Arc<Self>) {
        if self.stopped.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire) {
            return;
        }
        let tickets = self.leases.lock().unwrap().pending_cleanups();
        for ticket in tickets {
            if self
                .cleanup_running
                .lock()
                .unwrap()
                .contains(&ticket.ticket_id)
            {
                continue;
            }
            if !self
                .cleanup_attempted
                .lock()
                .unwrap()
                .insert(ticket.ticket_id.clone())
            {
                continue;
            }
            let core = self.clone();
            self.cleanup_running
                .lock()
                .unwrap()
                .insert(ticket.ticket_id.clone());
            tokio::spawn(async move {
                let outcome = core.cleanup_domain(&ticket).await;
                let attempt = json!({"attempt_id":new_id().ok(),"ticket":ticket,"resource_release_confirmed":outcome.is_ok(),
                    "physical_zero_verified":false,"result":outcome.as_ref().ok(),"error":outcome.as_ref().err()});
                {
                    let mut reports = core.cleanup_results.lock().unwrap();
                    // Store snapshots from separate attempts, never rewrite an old report.
                    let history = reports
                        .entry(ticket.ticket_id.clone())
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .unwrap();
                    history.push(attempt);
                    if history.len() > 16 {
                        history.remove(0);
                    }
                    let retained = core
                        .leases
                        .lock()
                        .unwrap()
                        .pending_cleanups()
                        .into_iter()
                        .map(|ticket| ticket.ticket_id)
                        .collect::<BTreeSet<_>>();
                    while reports.len() > 256 {
                        let old = reports.keys().find(|key| !retained.contains(*key)).cloned();
                        if let Some(old) = old {
                            reports.remove(&old);
                        } else {
                            break;
                        }
                    }
                }
                if outcome.is_ok() {
                    let _ = core
                        .leases
                        .lock()
                        .unwrap()
                        .complete_cleanup(ReleaseEvidence::confirmed(&ticket));
                    core.cleanup_attempted
                        .lock()
                        .unwrap()
                        .remove(&ticket.ticket_id);
                }
                core.cleanup_running
                    .lock()
                    .unwrap()
                    .remove(&ticket.ticket_id);
            });
        }
    }
    pub(crate) async fn worker_cache(&self) -> Result<Value, HostError> {
        let _query = self.query_gate.lock().await;
        if let Some((at, value)) = &*self.status_cache.lock().unwrap() {
            if at.elapsed() < Duration::from_millis(2500) {
                return Ok(value.clone());
            }
        }
        let sampled_at = self.events.monotonic_ms();
        let request = json!({"v":3,"id":new_id()?,"method":"status","params":{},
            "context":self.worker.global_context().map_err(|error|HostError::new("WorkerUnknown",error))?});
        let reply = self
            .worker
            .submit(WorkerRequest::V3(request))
            .map_err(|error| HostError::new("WorkerUnknown", error.message))?
            .wait_async(Duration::from_secs(5))
            .await
            .map_err(|error| HostError::new("WorkerUnknown", error.message))?;
        if reply["ok"] != true {
            return Err(HostError::new("WorkerUnknown", reply.to_string()));
        }
        let mut value = reply["result"].clone();
        value["cached_at_monotonic_ms"] = json!(sampled_at);
        *self.status_cache.lock().unwrap() = Some((Instant::now(), value.clone()));
        Ok(value)
    }
    async fn cleanup_domain(&self, ticket: &CleanupTicket) -> Result<Value, HostError> {
        let key = ticket.domain.key();
        self.worker
            .fence_domain(&key)
            .map_err(|error| HostError::new("CleanupRetained", error))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(error) = self.worker.transport_failure() {
                return Err(HostError::new("CleanupRetained", error));
            }
            let mut context = self
                .worker
                .domain_context(&key)
                .or_else(|| self.bindings.lock().unwrap().get(&ticket.domain).cloned())
                .ok_or_else(|| HostError::new("CleanupRetained", "Domain context unavailable"))?;
            if context["connection_id"].is_null() && self.worker.domain_pending(&key) > 0 {
                let cache = self.worker_cache().await?;
                context = cache["domains"][&key]["context"].clone();
                if !crate::reply_broker::valid_context_v3(&context) || context.is_null() {
                    return Err(HostError::new(
                        "CleanupRetained",
                        "Current worker domain context unavailable",
                    ));
                }
            }
            if context["connection_id"].is_null() {
                if self.worker.domain_pending(&key) == 0 {
                    self.status_cache.lock().unwrap().take();
                    let status = self.worker_cache().await?;
                    if status["domains"][&key]["probe_responsibility"] == true {
                        return Err(HostError::new("CleanupRetained","A failed identity probe still owns its transport. Stop Host to retry its confirmed cleanup; do not discard this record."));
                    }
                    return Ok(
                        json!({"context":context,"pending_calls":0,"resource_responsibility":"none","evidence":"current disarmed/released domain context","physical_zero_verified":false}),
                    );
                }
                if Instant::now() >= deadline {
                    return Err(HostError::new(
                        "CleanupRetained",
                        "Earlier domain admission remains pending",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            }
            let request =
                json!({"v":3,"id":new_id()?,"method":"disconnect","params":{},"context":context});
            let reply = self
                .worker
                .submit(WorkerRequest::V3(request))
                .map_err(|error| HostError::new("CleanupRetained", error.message))?
                .wait_async(Duration::from_secs(90))
                .await
                .map_err(|error| HostError::new("CleanupRetained", error.message))?;
            if reply["ok"] == true
                && reply["phase"] == "completed"
                && reply["context"]["domain"] == serde_json::to_value(&ticket.domain).unwrap()
                && reply["context"]["connection_id"].is_null()
                && reply["result"]["connected"] == false
                && reply["result"]["cleanup"]["unreleased"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
                && self.worker.domain_pending(&key) == 0
            {
                return Ok(reply);
            }
            return Err(HostError::new(
                "CleanupRetained",
                format!("Final release not confirmed: {reply}"),
            ));
        }
    }
    pub(crate) fn snapshot(&self) -> Result<Value, HostError> {
        let mut registry = self.registry.lock().unwrap().clone();
        registry.drafts.retain(|d| d.status != "Cancelled");
        registry.tombstones.clear();
        let cache = self
            .status_cache
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| json!({}));
        let mut domains = cache["domains"].clone();
        if !domains.is_object() {
            domains = json!({});
        }
        let configured = self
            .bindings
            .lock()
            .unwrap()
            .keys()
            .map(DomainRef::key)
            .collect::<BTreeSet<_>>();
        domains
            .as_object_mut()
            .unwrap()
            .retain(|key, _| configured.contains(key));
        if let Some(values) = domains.as_object_mut() {
            for (key, status) in values {
                status["device"] = cache["devices"][key].clone();
                status["host_sample_ms"] = cache["cached_at_monotonic_ms"].clone();
                if self.status_failed.load(Ordering::Acquire) {
                    status["communication"] = json!("UNKNOWN");
                    status["freshness"] = json!("Freshness Unknown");
                    status["host_sample_ms"] = Value::Null;
                }
            }
        }
        for (domain, ctx) in &*self.bindings.lock().unwrap() {
            let current = self
                .worker
                .domain_context(&domain.key())
                .unwrap_or_else(|| ctx.clone());
            if domains.get(domain.key()).is_none() || domains[domain.key()]["context"] != current {
                domains[domain.key()] = json!({"context":current,"connected":false,"communication":"UNKNOWN","state":"UNKNOWN"});
            }
        }
        for device in &registry.devices {
            let key = format!("device:{}", device.device_id);
            if let Some(status) = domains.get_mut(&key) {
                status["availability"] = self.checks.lock().unwrap().state(
                    device,
                    &status["context"],
                    self.events.monotonic_ms(),
                );
            }
        }
        Ok(
            json!({"host_id":registry.host_id,"host_name":registry.settings.host_name,"mode":self.mode,
            "registry":registry,"domains":domains,"sample_cached_at_monotonic_ms":cache["cached_at_monotonic_ms"],"startup_error":self.startup_error,"host_status":if self.startup_error.is_some(){"RETAINED"}else{"ONLINE"},
            "worker_cleanup":self.worker.shutdown_evidence(),"boot_id":self.boot_id,
            "archive":{"active_root":self.archive_root,"error":self.archive_error,"available":self.archive.is_some()},
            "control":self.leases.lock().unwrap().snapshot(),"cleanup_attempts":self.cleanup_results.lock().unwrap().values().collect::<Vec<_>>()}),
        )
    }
    fn validate_intent(&self, intent: &ExecuteParams) -> Result<(), HostError> {
        intent.validate()?;
        if self.startup_error.is_some()
            || self.stopping.load(Ordering::Acquire)
            || self.worker.transport_failure().is_some()
        {
            return Err(HostError::new(
                "HostRetained",
                "Host startup or stop is retained",
            ));
        }
        let snapshot = self.registry.lock().unwrap().clone();
        if intent.domain.kind == "device"
            && snapshot
                .setups
                .iter()
                .any(|s| s.members.contains(&intent.domain.id))
        {
            return Err(HostError::new(
                "SetupOwnsDevice",
                "Operate this controller through its Fiber setup",
            ));
        }
        let model = if intent.domain.kind == "device" {
            let device = snapshot
                .devices
                .iter()
                .find(|device| device.device_id == intent.domain.id);
            let (model_id, config_rev) = if let Some(device) = device {
                super::contracts::verification_mode_matches(&device.verified_mode, &self.mode)?;
                (device.model_id.as_str(), device.config_rev)
            } else {
                let draft = snapshot
                    .drafts
                    .iter()
                    .find(|draft| {
                        draft.device_id == intent.domain.id
                            && draft.status != "Cancelled"
                            && draft.mode == self.mode
                    })
                    .ok_or_else(|| {
                        HostError::new(
                            "DeviceUnknown",
                            "Registered device or active draft not found",
                        )
                    })?;
                let model_id = draft.model_id.as_deref().unwrap_or("");
                if intent.method != "connect" || !matches!(model_id, "gain" | "voltage") {
                    return Err(HostError::new("ManualVerificationRequired","Only separately confirmed supervised connection is allowed before registration"));
                }
                (model_id, draft.revision)
            };
            if config_rev != intent.config_rev {
                return Err(HostError::new(
                    "ConfigConflict",
                    "Device configuration changed",
                ));
            }
            Some(
                super::catalog::Catalog::load(super::catalog::DOCUMENT)?
                    .model(model_id)?
                    .clone(),
            )
        } else {
            let setup = snapshot
                .setups
                .iter()
                .find(|setup| setup.setup_id == intent.domain.id)
                .ok_or_else(|| HostError::new("DeviceUnknown", "Setup not found"))?;
            if setup.config_rev != intent.config_rev {
                return Err(HostError::new("ConfigConflict", "Setup changed"));
            };
            for id in &setup.members {
                super::contracts::verification_mode_matches(
                    &snapshot
                        .devices
                        .iter()
                        .find(|d| &d.device_id == id)
                        .ok_or_else(|| HostError::new("DeviceUnknown", "Setup member missing"))?
                        .verified_mode,
                    &self.mode,
                )?;
            }
            None
        };
        let current = self
            .worker
            .domain_context(&intent.domain.key())
            .or_else(|| self.bindings.lock().unwrap().get(&intent.domain).cloned())
            .ok_or_else(|| HostError::new("DeviceUnknown", "Worker domain not configured"))?;
        if current != intent.context {
            return Err(HostError::new(
                "StaleContext",
                "Connection or safety generation changed",
            ));
        }
        match intent.method.as_str() {
            "connect" => {
                if intent.params != json!({})
                    && intent.params != json!({"acknowledge_lifecycle":true})
                {
                    return Err(HostError::new(
                        "OperationInvalid",
                        "Invalid connect parameters",
                    ));
                }
                if model
                    .as_ref()
                    .is_some_and(|m| matches!(m.driver_kind.as_str(), "osa" | "gain" | "voltage"))
                    && intent.params["acknowledge_lifecycle"] != true
                {
                    return Err(HostError::new(
                        "ConfirmationRequired",
                        "Explicit lifecycle effects acknowledgement required",
                    ));
                }
            }
            "resume" => {
                if intent.params != json!({"confirm":true}) {
                    return Err(HostError::new(
                        "ConfirmationRequired",
                        "Explicit resume required",
                    ));
                }
            }
            "action" => {
                fields(&intent.params, &["name", "args"])?;
                let name = intent.params["name"]
                    .as_str()
                    .ok_or_else(|| HostError::new("OperationInvalid", "Action name required"))?;
                let allowed = model
                    .as_ref()
                    .map_or(matches!(name, "move" | "adopt_baseline"), |m| {
                        m.operations.iter().any(|item| item == name)
                    });
                if !allowed {
                    return Err(HostError::new(
                        "OperationInvalid",
                        "Action is not in the trusted model catalog",
                    ));
                }
                validate_action(
                    model.as_ref().map_or("fiber", |m| m.driver_kind.as_str()),
                    name,
                    &intent.params["args"],
                )?;
            }
            _ => {
                return Err(HostError::new(
                    "OperationInvalid",
                    "Verification uses its dedicated coordinator",
                ))
            }
        }
        Ok(())
    }
    async fn execute(
        self: &Arc<Self>,
        session: Session,
        id: String,
        intent: ExecuteParams,
    ) -> Result<Value, HostError> {
        // The journal actor may wait on disk; lease/cleanup locks never do.
        let permit = self
            .ordinary_capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| HostError::new("OperationCapacity", "Ordinary admission queue is full"))?;
        let _ordered = self.ordinary_admission.lock().await;
        if let Some(old) = self
            .operations
            .read(|book| book.lookup(&session, &id, &intent))?
        {
            return Ok(serde_json::to_value(old).unwrap());
        }
        self.validate_intent(&intent)?;
        let capture_origin = if intent.method == "action"
            && matches!(
                intent.params["name"].as_str(),
                Some("acquire" | "read_trace")
            ) {
            let snapshot = self.registry.lock().unwrap();
            snapshot
                .devices
                .iter()
                .find(|d| {
                    intent.domain.kind == "device"
                        && d.device_id == intent.domain.id
                        && d.model_id == "aq6370"
                })
                .map(|d| super::archive::CaptureOrigin {
                    host_id: snapshot.host_id.clone(),
                    domain: intent.domain.clone(),
                    device_identity: d.expected_identity.clone(),
                    config_rev: intent.config_rev,
                    operation_id: String::new(),
                })
        } else {
            None
        };
        let capture_name = if capture_origin.is_some() {
            recording_name(&intent.params)?.to_owned()
        } else {
            String::new()
        };
        self.leases.lock().unwrap().admit(
            &intent.lease_token,
            &session,
            &intent.domain,
            intent.control_epoch,
            Instant::now(),
        )?;
        let args = intent.clone();
        let caller = session.clone();
        let record: OperationRecord = self
            .operations
            .call(Box::new(move |book| {
                Ok(serde_json::to_value(book.admit(&caller, &id, args, Instant::now())?).unwrap())
            }))
            .await?;
        let _ = self.events.publish(super::events::HostEvent {
            kind: "operation".into(),
            host_id: self.registry.lock().unwrap().host_id.clone(),
            boot_id: self.boot_id.clone(),
            seq: 0,
            first_seq: 0,
            sent_monotonic_ms: 0,
            domain: Some(record.domain.clone()),
            data: serde_json::to_value(&record).unwrap(),
        });
        let capture_origin = capture_origin.map(|mut origin| {
            origin.operation_id = record.operation_id.clone();
            origin
        });
        // Bind a durable, sample-free attempt before worker delivery. Failure
        // to create it is separately reported, never invented as stored data.
        let attempt_storage_error = if let Some(origin) = capture_origin.clone() {
            let name = capture_name.clone();
            match &self.archive {
                Some(actor) => actor
                    .call::<Value>(Box::new(move |store| {
                        store.begin_attempt(&name, &origin)?;
                        Ok(Value::Null)
                    }))
                    .await
                    .err(),
                None => Some(self.archive_error.clone().unwrap_or_else(|| {
                    HostError::new("ArchiveUnavailable", "Archive unavailable")
                })),
            }
        } else {
            None
        };
        // Nonblocking bounded enqueue is ordered with revocation. No OS write,
        // wait, driver call or disk operation happens under the lease mutex.
        let submission = {
            let mut leases = self.leases.lock().unwrap();
            leases
                .admit(
                    &intent.lease_token,
                    &session,
                    &intent.domain,
                    intent.control_epoch,
                    Instant::now(),
                )
                .and_then(|_| {
                    self.worker
                        .submit(WorkerRequest::V3(
                            json!({"v":3,"id":record.worker_id,"method":intent.method,
                    "params":instrument_params(&intent.method,&intent.params),"context":intent.context}),
                        ))
                        .map_err(|error| HostError::new("WorkerAdmission", error.message))
                })
        };
        let core = self.clone();
        let op_id = record.operation_id.clone();
        tokio::spawn(async move {
            let _capacity = permit;
            let (mut phase, mut result) = match submission {
                Ok(pending) => match pending.wait_async(Duration::from_secs(180)).await {
                    Ok(reply) => (
                        reply["phase"]
                            .as_str()
                            .unwrap_or("timed_out_unknown")
                            .to_string(),
                        reply,
                    ),
                    Err(error) => ("timed_out_unknown".into(), json!({"error":error.message})),
                },
                Err(error) => ("rejected_before_call".into(), json!({"error":error})),
            };
            if let Some(origin) = capture_origin.clone().filter(|_| phase == "completed") {
                let descriptor = result["result"]["result"].clone();
                let import_core = core.clone();
                let name = capture_name.clone();
                let stored=match &core.archive {
                    Some(actor)=>actor.call::<Value>(Box::new(move|store| {
                        let reference=store.import(&name,&origin,&descriptor,
                            |offset,length|import_core.worker.read_capture_chunk(&descriptor,offset,length)
                                .map_err(|e|HostError::new("ArchiveUnavailable",e)),
                            ||import_core.stopping.load(Ordering::Acquire)||import_core.stopped.load(Ordering::Acquire))?;
                        // Worker bytes remain its responsibility until durable import.
                        // Failed acknowledgement does not undo a committed archive.
                        let acknowledgement=import_core.worker.ack_capture(&descriptor);
                        Ok(json!({"archive_ref":reference,"staging_release_confirmed":acknowledgement.is_ok(),
                            "staging_release_error":acknowledgement.err()}))
                    })).await,
                    None=>Err(core.archive_error.clone().unwrap_or_else(||HostError::new("ArchiveUnavailable","Archive unavailable"))),
                };
                match stored {
                    Ok(value) => result["result"] = value,
                    Err(error) => {
                        phase = "completed_readback_failed".into();
                        result["result"] = json!({"error":error,"storage_state":"incomplete","hardware_read_completed":true,"retry_hardware":false});
                    }
                }
            } else if capture_origin.is_none() {
                if let Ok(bytes) = serde_json::to_vec(&result["result"]) {
                    if bytes.len() > 28000
                        || matches!(
                            intent.params["name"].as_str(),
                            Some("acquire" | "measure_kind" | "read_setting" | "run_maintenance")
                        )
                    {
                        let owner = session.clone();
                        let domain = intent.domain.clone();
                        let stored = core
                            .results
                            .call::<super::results::ResultMeta>(Box::new(move |store| {
                                Ok(serde_json::to_value(store.put(&owner, &domain, &bytes)?)
                                    .unwrap())
                            }))
                            .await;
                        match stored {
                            Ok(meta) => result["result"] = json!({"result_ref":meta}),
                            Err(error) => {
                                phase = "completed_readback_failed".into();
                                result["result"] = json!({"error":error,"output_state":"UNKNOWN"});
                            }
                        }
                    }
                }
            }
            if let Some(origin) = capture_origin.filter(|_| phase != "completed") {
                let primary = result["result"]["error"]["message"]
                    .as_str()
                    .or_else(|| result["error"]["message"].as_str())
                    .or_else(|| result["error"].as_str())
                    .unwrap_or("Capture did not complete; hardware completion unconfirmed")
                    .to_owned();
                let failure_phase = phase.clone();
                let confirmed = phase == "completed_readback_failed";
                let saved = match &core.archive {
                    Some(actor) => {
                        actor
                            .call::<Value>(Box::new(move |store| {
                                store.record_failure(
                                    &capture_name,
                                    &origin,
                                    &failure_phase,
                                    &primary,
                                    confirmed,
                                )?;
                                Ok(Value::Null)
                            }))
                            .await
                    }
                    None => Err(core.archive_error.clone().unwrap_or_else(|| {
                        HostError::new("ArchiveUnavailable", "Archive unavailable")
                    })),
                };
                result["partial_history_confirmed"] = json!(saved.is_ok());
                result["partial_history_error"] = json!(saved.err());
                result["attempt_storage_error"] = json!(attempt_storage_error);
            }
            let finished: Result<OperationRecord, _> = core
                .operations
                .call(Box::new(move |book| {
                    Ok(serde_json::to_value(book.finish(&op_id, &phase, result)?).unwrap())
                }))
                .await;
            if let Ok(done) = finished {
                let _ = core.events.publish(super::events::HostEvent {
                    kind: "operation".into(),
                    host_id: core.registry.lock().unwrap().host_id.clone(),
                    boot_id: core.boot_id.clone(),
                    seq: 0,
                    first_seq: 0,
                    sent_monotonic_ms: 0,
                    domain: Some(done.domain.clone()),
                    data: serde_json::to_value(done).unwrap(),
                });
            }
            if let Ok(snapshot) = core.snapshot() {
                let _ = core.events.update(snapshot);
            }
        });
        Ok(serde_json::to_value(record).unwrap())
    }
    fn archive_actor(&self) -> Result<&super::archive::ArchiveActor, HostError> {
        self.archive.as_ref().ok_or_else(|| {
            self.archive_error
                .clone()
                .unwrap_or_else(|| HostError::new("ArchiveUnavailable", "Archive unavailable"))
        })
    }
    async fn dispatch(
        self: &Arc<Self>,
        request: &HostRequest,
        client_session: &str,
    ) -> Result<Value, HostError> {
        self.sync_power_fence();
        self.schedule_cleanups();
        match request.method.as_str() {
            "close_client" => {
                empty(&request.params)?;
                let session = Session::local(client_session.into(), self.boot_id.clone())?;
                let tickets = {
                    let mut clients = self.clients.lock().unwrap();
                    clients.begin_close(client_session)?;
                    self.leases.lock().unwrap().close_session(&session)?
                };
                self.schedule_cleanups();
                let deadline = Instant::now() + Duration::from_secs(12);
                loop {
                    let pending = self.leases.lock().unwrap().pending_cleanups();
                    if !tickets
                        .iter()
                        .any(|t| pending.iter().any(|p| p.ticket_id == t.ticket_id))
                    {
                        break;
                    }
                    let failed = tickets.iter().any(|t| {
                        self.cleanup_results
                            .lock()
                            .unwrap()
                            .get(&t.ticket_id)
                            .and_then(|v| v.as_array())
                            .and_then(|v| v.last())
                            .is_some_and(|v| v["resource_release_confirmed"] != true)
                    });
                    if failed || Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                let pending = self.leases.lock().unwrap().pending_cleanups();
                let attempts=tickets.iter().map(|t|json!({"domain":t.domain,"ticket_id":t.ticket_id,"confirmed":!pending.iter().any(|p|p.ticket_id==t.ticket_id),"attempts":self.cleanup_results.lock().unwrap().get(&t.ticket_id).cloned()})).collect::<Vec<_>>();
                Ok(
                    json!({"released":attempts.iter().all(|a|a["confirmed"]==true),"cleanup_attempts":attempts,"host_stopped":false,"physical_zero_verified":false}),
                )
            }
            "save_check_policy" => {
                fields(
                    &request.params,
                    &[
                        "device_id",
                        "config_rev",
                        "expected_rev",
                        "interval_s",
                        "enumeration",
                        "readonly",
                    ],
                )?;
                let params = request.params.clone();
                let mode = self.mode.clone();
                self.configuration
                    .call(Box::new(move |v| {
                        let id = text_param(&params, "device_id")?.to_string();
                        let config_rev = uint_param(&params, "config_rev")?;
                        let before = v.registry.snapshot()?;
                        let device = before
                            .devices
                            .iter()
                            .find(|d| d.device_id == id)
                            .ok_or_else(|| {
                                HostError::new("DeviceUnknown", "Register the device first")
                            })?;
                        let cat = super::catalog::Catalog::load(super::catalog::DOCUMENT)?;
                        let profile = cat.profile(&device.model_id, &device.profile_id)?;
                        let binding = super::contracts::CheckBinding {
                            mode,
                            identity: device.expected_identity.clone(),
                            config_rev,
                            profile_id: device.profile_id.clone(),
                            probe_version: profile.probe_version,
                        };
                        let interval_s = u32::try_from(uint_param(&params, "interval_s")?)
                            .map_err(|_| HostError::new("CheckInvalid", "Interval too large"))?;
                        let mut policy = super::contracts::CheckPolicy {
                            interval_s,
                            enumeration: None,
                            readonly: None,
                        };
                        for (name, stage) in [
                            ("enumeration", super::availability::CheckStage::Enumeration),
                            ("readonly", super::availability::CheckStage::Readonly),
                        ] {
                            let enabled = params[name].as_bool().ok_or_else(|| {
                                HostError::new("CheckInvalid", "Explicit boolean consent required")
                            })?;
                            if enabled {
                                policy = policy.authorize(stage, binding.clone());
                                if !policy.permits(stage, device, &binding.mode)? {
                                    return Err(HostError::new(
                                        "CheckForbidden",
                                        "Profile forbids this automatic check",
                                    ));
                                }
                            }
                        }
                        Ok(serde_json::to_value(v.registry.commit(
                            uint_param(&params, "expected_rev")?,
                            super::registry::RegistryChange::SaveCheckPolicy {
                                device_id: id,
                                config_rev,
                                policy,
                            },
                        )?)
                        .unwrap())
                    }))
                    .await
            }
            "save_settings" => {
                fields(&request.params, &["settings", "expected_rev"])?;
                let settings: super::contracts::HostSettings =
                    serde_json::from_value(request.params["settings"].clone())
                        .map_err(|e| HostError::new("ConfigInvalid", e.to_string()))?;
                let rev = uint_param(&request.params, "expected_rev")?;
                self.configuration
                    .call(Box::new(move |v| {
                        let changed_root =
                            v.registry.snapshot()?.settings.data_root != settings.data_root;
                        let mut result = serde_json::to_value(v.registry.commit(
                            rev,
                            super::registry::RegistryChange::SaveSettings(settings),
                        )?)
                        .unwrap();
                        result["restart_required"] = json!(changed_root);
                        Ok(result)
                    }))
                    .await
            }
            "list_archives" => {
                fields(&request.params, &["domain", "offset", "limit"])?;
                let domain = if request.params["domain"].is_null() {
                    None
                } else {
                    Some(parse_domain(&request.params["domain"])?)
                };
                let offset = usize::try_from(uint_param(&request.params, "offset")?)
                    .map_err(|_| HostError::new("ArchiveScope", "Invalid page offset"))?;
                let limit = uint_param(&request.params, "limit")?;
                if !(1..=4).contains(&limit) || offset > 4096 {
                    return Err(HostError::new(
                        "ArchiveScope",
                        "Archive pages contain at most four entries",
                    ));
                }
                self.archive_actor()?
                    .call::<Value>(Box::new(move |store| {
                        store.list_page(domain.as_ref(), offset, limit as usize)
                    }))
                    .await
            }
            "archive_manifest" | "archive_manifest_bytes" | "read_archive" => {
                let read = request.method == "read_archive";
                let manifest_bytes = request.method == "archive_manifest_bytes";
                fields(
                    &request.params,
                    if read {
                        &["domain", "name", "id", "offset", "length"]
                    } else {
                        &["domain", "name", "id"]
                    },
                )?;
                let domain = parse_domain(&request.params["domain"])?;
                let name = text_param(&request.params, "name")?.to_owned();
                let id = text_param(&request.params, "id")?.to_owned();
                let offset = if read {
                    uint_param(&request.params, "offset")?
                } else {
                    0
                };
                let length = if read {
                    uint_param(&request.params, "length")?
                } else {
                    0
                };
                self.archive_actor()?.call::<Value>(Box::new(move|store| {
                    let manifest=store.manifest(&name,&id)?;
                    if manifest["origin"]["domain"]!=serde_json::to_value(&domain).unwrap() {return Err(HostError::new("ArchiveScope","Archive belongs to another domain"));}
                    if manifest_bytes {
                        let bytes=store.manifest_bytes(&name,&id)?;
                        return Ok(json!({"id":id,"name":name,"byte_count":bytes.len(),"sha256":super::verification::sha256_bytes(&bytes)?,
                            "data_hex":bytes.iter().map(|b|format!("{b:02x}")).collect::<String>()}));
                    }
                    if !read {return Ok(manifest);}
                    let bytes=store.read(&name,&id,offset,length)?;
                    Ok(json!({"id":id,"name":name,"offset":offset,"length":bytes.len(),"sha256":manifest["descriptor"]["sha256"],
                        "data_hex":bytes.iter().map(|b|format!("{b:02x}")).collect::<String>()}))
                })).await
            }
            "rename_device" | "retire_device" => {
                let rename = request.method == "rename_device";
                fields(
                    &request.params,
                    if rename {
                        &["device_id", "config_rev", "expected_rev", "name"]
                    } else {
                        &["device_id", "config_rev", "expected_rev"]
                    },
                )?;
                let params = request.params.clone();
                let mut port = self.verification_port.clone();
                self.configuration
                    .call(Box::new(move |v| {
                        let id = text_param(&params, "device_id")?.to_string();
                        let rev = uint_param(&params, "expected_rev")?;
                        let snapshot = v.registry.snapshot()?;
                        if snapshot.registry_rev != rev {
                            return Err(HostError::new(
                                "ConfigConflict",
                                "Registry revision changed",
                            ));
                        }
                        let record = snapshot
                            .devices
                            .iter()
                            .find(|d| d.device_id == id)
                            .ok_or_else(|| HostError::new("DeviceUnknown", "Device missing"))?
                            .clone();
                        if record.config_rev != uint_param(&params, "config_rev")? {
                            return Err(HostError::new(
                                "ConfigConflict",
                                "Device revision changed",
                            ));
                        }
                        if snapshot.setups.iter().any(|s| s.members.contains(&id)) {
                            return Err(HostError::new(
                                "SetupRetained",
                                "Member belongs to a Fiber setup; do not detach its responsibility",
                            ));
                        }
                        let domain = DomainRef {
                            kind: "device".into(),
                            id: id.clone(),
                        };
                        port.quiescent(&domain)?;
                        if rename {
                            let next = v.registry.commit(
                                rev,
                                super::registry::RegistryChange::EditName {
                                    device_id: id.clone(),
                                    config_rev: record.config_rev,
                                    name: text_param(&params, "name")?.into(),
                                },
                            )?;
                            port.configure_wire(super::configuration::device_wire(
                                next.devices.iter().find(|d| d.device_id == id).unwrap(),
                            )?)?;
                            Ok(serde_json::to_value(next).unwrap())
                        } else {
                            use super::verification::VerificationPort;
                            let draft = super::contracts::DraftRecord {
                                device_id: id.clone(),
                                name: record.name.clone(),
                                model_id: Some(record.model_id.clone()),
                                profile_id: Some(record.profile_id.clone()),
                                params: record.params.clone(),
                                config_digest: String::new(),
                                revision: record.config_rev,
                                mode: String::new(),
                                status: String::new(),
                            };
                            let attempt = port.cancel(&draft)?;
                            let permit = super::registry::ReleasePermit::confirmed(
                                id.clone(),
                                record.config_rev,
                                attempt,
                            );
                            Ok(serde_json::to_value(v.registry.commit(
                                rev,
                                super::registry::RegistryChange::RetireDevice {
                                    device_id: id,
                                    config_rev: record.config_rev,
                                    permit,
                                },
                            )?)
                            .unwrap())
                        }
                    }))
                    .await
            }
            "save_setup" => {
                fields(&request.params, &["name", "members", "expected_rev"])?;
                let params = request.params.clone();
                let mut port = self.verification_port.clone();
                let mode = self.mode.clone();
                self.configuration
                    .call(Box::new(move |v| {
                        use super::verification::VerificationPort;
                        let before = v.registry.snapshot()?;
                        let rev = uint_param(&params, "expected_rev")?;
                        if rev != before.registry_rev {
                            return Err(HostError::new(
                                "ConfigConflict",
                                "Registry revision changed",
                            ));
                        }
                        let members: Vec<String> =
                            serde_json::from_value(params["members"].clone())
                                .map_err(|e| HostError::new("SetupInvalid", e.to_string()))?;
                        for id in &members {
                            super::contracts::verification_mode_matches(
                                &before
                                    .devices
                                    .iter()
                                    .find(|d| &d.device_id == id)
                                    .ok_or_else(|| {
                                        HostError::new("DeviceUnknown", "Setup member missing")
                                    })?
                                    .verified_mode,
                                &mode,
                            )?;
                            port.quiescent(&DomainRef {
                                kind: "device".into(),
                                id: id.clone(),
                            })?;
                        }
                        let setup = super::contracts::SetupRecord {
                            setup_id: new_id()?,
                            name: text_param(&params, "name")?.into(),
                            kind: "fiber".into(),
                            members,
                            config_rev: 1,
                        };
                        let next = v.registry.commit(
                            rev,
                            super::registry::RegistryChange::SaveSetup(setup.clone()),
                        )?;
                        for id in &setup.members {
                            let d = next.devices.iter().find(|d| d.device_id == *id).unwrap();
                            port.cancel(&super::contracts::DraftRecord {
                                device_id: id.clone(),
                                name: d.name.clone(),
                                model_id: Some(d.model_id.clone()),
                                profile_id: Some(d.profile_id.clone()),
                                params: d.params.clone(),
                                config_digest: String::new(),
                                revision: d.config_rev,
                                mode: String::new(),
                                status: String::new(),
                            })?;
                        }
                        port.configure_wire(super::configuration::setup_wire(&setup, &next)?)?;
                        Ok(serde_json::to_value(setup).unwrap())
                    }))
                    .await
            }
            "retire_setup" => {
                fields(&request.params, &["setup_id", "config_rev", "expected_rev"])?;
                let params = request.params.clone();
                let mut port = self.verification_port.clone();
                self.configuration
                    .call(Box::new(move |v| {
                        let before = v.registry.snapshot()?;
                        let rev = uint_param(&params, "expected_rev")?;
                        if before.registry_rev != rev {
                            return Err(HostError::new(
                                "ConfigConflict",
                                "Registry revision changed",
                            ));
                        }
                        let setup = before
                            .setups
                            .iter()
                            .find(|s| Some(s.setup_id.as_str()) == params["setup_id"].as_str())
                            .ok_or_else(|| HostError::new("SetupUnknown", "Setup missing"))?
                            .clone();
                        if setup.config_rev != uint_param(&params, "config_rev")? {
                            return Err(HostError::new("ConfigConflict", "Setup revision changed"));
                        }
                        let domain = DomainRef {
                            kind: "setup".into(),
                            id: setup.setup_id.clone(),
                        };
                        let attempt = port.retire(&domain, setup.config_rev)?;
                        let next = v.registry.commit(
                            rev,
                            super::registry::RegistryChange::RetireSetup {
                                setup_id: setup.setup_id.clone(),
                                config_rev: setup.config_rev,
                                permit: super::registry::ReleasePermit::confirmed(
                                    setup.setup_id,
                                    setup.config_rev,
                                    attempt,
                                ),
                            },
                        )?;
                        let _ = next;
                        Ok(json!({"retired":true,"restart_required":true,"physical_zero_verified":false}))
                    }))
                    .await
            }
            "create_draft" => {
                fields(
                    &request.params,
                    &["model_id", "profile_id", "params", "name", "expected_rev"],
                )?;
                let params = request.params.clone();
                let mode = self.mode.clone();
                let port = self.verification_port.clone();
                self.configuration
                    .call(Box::new(move |verifier| {
                        let draft = verifier.create_draft(
                            text_param(&params, "model_id")?,
                            text_param(&params, "profile_id")?,
                            params["params"].clone(),
                            text_param(&params, "name")?,
                            &mode,
                            uint_param(&params, "expected_rev")?,
                        )?;
                        port.configure(&draft)?;
                        Ok(serde_json::to_value(draft).unwrap())
                    }))
                    .await
            }
            "test_connection" => {
                fields(
                    &request.params,
                    &[
                        "draft_id",
                        "expected_rev",
                        "consent",
                        "lease_token",
                        "control_epoch",
                        "request_id",
                        "sequence",
                    ],
                )?;
                let params = request.params.clone();
                let control = self.verification_control.clone();
                let session = Session::local(client_session.into(), self.boot_id.clone())?;
                let domain = DomainRef {
                    kind: "device".into(),
                    id: text_param(&params, "draft_id")?.into(),
                };
                self.leases.lock().unwrap().admit(
                    text_param(&params, "lease_token")?,
                    &session,
                    &domain,
                    uint_param(&params, "control_epoch")?,
                    Instant::now(),
                )?;
                self.configuration
                    .call(Box::new(move |verifier| {
                        let consent: super::verification::Consent =
                            serde_json::from_value(params["consent"].clone())
                                .map_err(|e| HostError::new("ConsentRequired", e.to_string()))?;
                        control.lock().unwrap().insert(
                            text_param(&params, "draft_id")?.into(),
                            super::configuration::VerificationControl {
                                session,
                                token: text_param(&params, "lease_token")?.into(),
                                epoch: uint_param(&params, "control_epoch")?,
                                request_id: text_param(&params, "request_id")?.into(),
                                sequence: uint_param(&params, "sequence")?,
                            },
                        );
                        Ok(serde_json::to_value(verifier.begin(
                            text_param(&params, "draft_id")?,
                            uint_param(&params, "expected_rev")?,
                            consent,
                        )?)
                        .unwrap())
                    }))
                    .await
            }
            "save_device" => {
                fields(&request.params, &["draft_id", "proof_id", "expected_rev"])?;
                let params = request.params.clone();
                self.configuration
                    .call(Box::new(move |verifier| {
                        Ok(serde_json::to_value(verifier.save(
                            text_param(&params, "draft_id")?,
                            text_param(&params, "proof_id")?,
                            uint_param(&params, "expected_rev")?,
                        )?)
                        .unwrap())
                    }))
                    .await
            }
            "cancel_draft" => {
                fields(&request.params, &["draft_id", "expected_rev"])?;
                let params = request.params.clone();
                self.configuration
                    .call(Box::new(move |verifier| {
                        verifier.cancel(
                            text_param(&params, "draft_id")?,
                            uint_param(&params, "expected_rev")?,
                        )?;
                        Ok(json!({"cancelled":true}))
                    }))
                    .await
            }
            "worker_status" => {
                empty(&request.params)?;
                Ok(self.snapshot()?)
            }
            "prepare" => {
                fields(&request.params, &["intent"])?;
                let intent: ExecuteParams =
                    serde_json::from_value(request.params["intent"].clone())
                        .map_err(|error| HostError::new("OperationInvalid", error.to_string()))?;
                self.validate_intent(&intent)?;
                let session = Session::local(client_session.into(), self.boot_id.clone())?;
                self.leases.lock().unwrap().admit(
                    &intent.lease_token,
                    &session,
                    &intent.domain,
                    intent.control_epoch,
                    Instant::now(),
                )?;
                self.operations
                    .call::<Value>(Box::new(move |book| {
                        Ok(
                            serde_json::to_value(book.prepare(&session, intent, Instant::now())?)
                                .unwrap(),
                        )
                    }))
                    .await
            }
            "execute" => {
                fields(&request.params, &["request_id", "intent"])?;
                let id = request.params["request_id"]
                    .as_str()
                    .ok_or_else(|| HostError::new("OperationInvalid", "Request ID required"))?
                    .to_string();
                let intent: ExecuteParams =
                    serde_json::from_value(request.params["intent"].clone())
                        .map_err(|error| HostError::new("OperationInvalid", error.to_string()))?;
                let core = self.clone();
                let session = Session::local(client_session.into(), self.boot_id.clone())?;
                let pending = tokio::spawn(async move { core.execute(session, id, intent).await });
                tokio::time::timeout(Duration::from_secs(5), pending)
                    .await
                    .map_err(|_| {
                        HostError::new(
                            "AdmissionPending",
                            "Receipt not confirmed; query original request ID, never resend",
                        )
                    })?
                    .map_err(|e| HostError::new("OutcomeUnknown", e.to_string()))?
            }
            "operation" => {
                fields(&request.params, &["request_id"])?;
                let id = request.params["request_id"]
                    .as_str()
                    .ok_or_else(|| HostError::new("OperationInvalid", "Request ID required"))?;
                let session = Session::local(client_session.into(), self.boot_id.clone())?;
                Ok(
                    serde_json::to_value(self.operations.read(|book| book.find(&session, id))?)
                        .unwrap(),
                )
            }
            "ping" => {
                empty(&request.params)?;
                Ok(
                    json!({"protocol_version":1,"client_session_id":client_session,"boot_id":self.boot_id,"mode":self.mode,"worker_protocol":self.worker.protocol(),"monotonic_ms":self.events.monotonic_ms(),"tray":{"visible":true,"has_reopen_management":true,"mode":self.mode,"status":self.tray_status.lock().unwrap().state}}),
                )
            }
            "snapshot" | "subscribe" => {
                empty(&request.params)?;
                self.events.update(self.snapshot()?)?;
                Ok(self.events.current())
            }
            "request_snapshot" => {
                empty(&request.params)?;
                self.events.update(self.snapshot()?)?;
                let seq = self.events.resnapshot(&Session::local(
                    client_session.into(),
                    self.boot_id.clone(),
                )?)?;
                Ok(json!({"seq":seq}))
            }
            "catalog" => {
                empty(&request.params)?;
                serde_json::from_slice(super::catalog::DOCUMENT)
                    .map_err(|error| HostError::new("Catalog", error.to_string()))
            }
            "driver_status" => {
                empty(&request.params)?;
                // Inventory and the status sampler share one reserved worker query slot.
                let _query = self.query_gate.lock().await;
                let query = json!({"v":3,"id":new_id()?,"method":"inventory","params":{},
                    "context":self.worker.global_context().map_err(|e|HostError::new("DriverStatus",e))?});
                let reply = self.worker.submit(WorkerRequest::V3(query))
                    .map_err(|e|HostError::new("DriverStatus",e.message))?
                    .wait_async(Duration::from_secs(10)).await
                    .map_err(|e|HostError::new("DriverStatus",e.message))?;
                if reply["ok"] != true { return Err(HostError::new("DriverStatus",reply.to_string())); }
                Ok(reply["result"].clone())
            }
            "acquire_control" => {
                let _ordered = self.ordinary_admission.lock().await;
                fields(&request.params, &["domain"])?;
                if !self.clients.lock().unwrap().control_allowed(client_session) {
                    return Err(HostError::new(
                        "ClientClosing",
                        "Reconnect as a new authenticated client before requesting control",
                    ));
                }
                if self.startup_error.is_some() || self.stopping.load(Ordering::Acquire) {
                    return Err(HostError::new(
                        "HostRetained",
                        "Host startup/stop is retained",
                    ));
                }
                let domain = parse_domain(&request.params["domain"])?;
                {
                    let snapshot = self.registry.lock().unwrap();
                    if domain.kind == "device" {
                        if let Some(d) = snapshot.devices.iter().find(|d| d.device_id == domain.id)
                        {
                            super::contracts::verification_mode_matches(
                                &d.verified_mode,
                                &self.mode,
                            )?;
                        }
                    } else if let Some(s) = snapshot.setups.iter().find(|s| s.setup_id == domain.id)
                    {
                        for id in &s.members {
                            super::contracts::verification_mode_matches(
                                &snapshot
                                    .devices
                                    .iter()
                                    .find(|d| &d.device_id == id)
                                    .ok_or_else(|| {
                                        HostError::new("DeviceUnknown", "Setup member missing")
                                    })?
                                    .verified_mode,
                                &self.mode,
                            )?;
                        }
                    }
                }
                if self.checks.lock().unwrap().checking(&domain) {
                    return Err(HostError::new(
                        "CheckBusy",
                        "An identity check is still responsible for this instance",
                    ));
                }
                if domain.kind == "device"
                    && self
                        .registry
                        .lock()
                        .unwrap()
                        .setups
                        .iter()
                        .any(|s| s.members.contains(&domain.id))
                {
                    return Err(HostError::new("SetupOwned", "Use the owning Fiber setup"));
                }
                let session = Session::local(client_session.into(), self.boot_id.clone())?;
                let result = {
                    // Pair this final eligibility check with close_client's
                    // clients -> leases lock order. Neither lock spans disk.
                    let clients = self.clients.lock().unwrap();
                    if !clients.control_allowed(client_session) {
                        return Err(HostError::new(
                            "ClientClosing",
                            "Reconnect before taking control",
                        ));
                    }
                    self.leases
                        .lock()
                        .unwrap()
                        .acquire(&session, &domain, Instant::now())
                };
                self.schedule_cleanups();
                serde_json::to_value(result?)
                    .map_err(|error| HostError::new("HostProtocol", error.to_string()))
            }
            "renew_control" => {
                fields(&request.params, &["token"])?;
                let session = Session::local(client_session.into(), self.boot_id.clone())?;
                let token = request.params["token"]
                    .as_str()
                    .filter(|token| super::contracts::valid_id(token))
                    .ok_or_else(|| HostError::new("ControlDenied", "Invalid control token"))?;
                let result = self
                    .leases
                    .lock()
                    .unwrap()
                    .renew(token, &session, Instant::now());
                self.schedule_cleanups();
                serde_json::to_value(result?)
                    .map_err(|error| HostError::new("HostProtocol", error.to_string()))
            }
            "release_control" | "safe_stop" => {
                if request.method == "safe_stop" {
                    fields(&request.params, &["domain"])?;
                } else {
                    fields(&request.params, &["domain", "token", "control_epoch"])?;
                }
                let domain = parse_domain(&request.params["domain"])?;
                let ticket = {
                    let mut book = self.leases.lock().unwrap();
                    if request.method == "release_control" {
                        let session = Session::local(client_session.into(), self.boot_id.clone())?;
                        let token = request.params["token"]
                            .as_str()
                            .ok_or_else(|| HostError::new("ControlDenied", "Invalid token"))?;
                        let epoch = request.params["control_epoch"].as_u64().ok_or_else(|| {
                            HostError::new("ControlDenied", "Invalid control epoch")
                        })?;
                        book.admit(token, &session, &domain, epoch, Instant::now())?;
                    }
                    book.revoke(
                        &domain,
                        if request.method == "safe_stop" {
                            RevokeCause::SafeStop
                        } else {
                            RevokeCause::Released
                        },
                    )?
                };
                if request.method == "safe_stop" {
                    self.cleanup_attempted
                        .lock()
                        .unwrap()
                        .remove(&ticket.ticket_id);
                }
                self.schedule_cleanups();
                let caller = Session::local(client_session.into(), self.boot_id.clone())?;
                let audit_ticket = serde_json::to_value(&ticket).unwrap();
                let id = request.id.clone();
                let audit_error = self
                    .safety_audit
                    .record(json!({"session":caller.id(),"request":id,"ticket":audit_ticket}))
                    .await;
                Ok(
                    json!({"accepted":true,"ticket":ticket,"physical_stop_confirmed":false,"audit_error":audit_error}),
                )
            }
            "stop" => {
                if request.params != json!({"confirm":true}) {
                    return Err(HostError::new(
                        "ConfirmationRequired",
                        "Explicit Host stop confirmation required",
                    ));
                }
                if self.stopping.swap(true, Ordering::AcqRel) {
                    return Err(HostError::new(
                        "HostStopping",
                        "A stop attempt is already in progress",
                    ));
                }
                self.leases
                    .lock()
                    .unwrap()
                    .revoke_all(RevokeCause::HostStop)?;
                let core = self.clone();
                let outcome=tokio::task::spawn_blocking(move||{
                    let report=core.worker.stop().map_err(|error|HostError::new("WorkerRetained",error.message))?;
                    if !report.resource_released||report.process_exit.as_ref().map_or(true,|exit|exit["confirmed"]!=true||exit["success"]!=true){
                        return Err(HostError::new("WorkerRetained","Release and successful process exit are separate required evidence"));
                    }
                    core.record.lock().unwrap().released()?;
                    Ok(json!({"cleanup":report.cleanup,"process_exit":report.process_exit,"resource_released":true,"physical_zero_verified":false}))
                }).await.map_err(|error|HostError::new("HostRuntime",error.to_string()))?;
                if let Ok(report) = &outcome {
                    let host_id = self.registry.lock().unwrap().host_id.clone();
                    let _=self.events.publish(super::events::HostEvent {
                        kind:"host_stopped".into(),host_id,boot_id:self.boot_id.clone(),
                        seq:0,first_seq:0,sent_monotonic_ms:0,domain:None,
                        data:json!({"resource_released":true,"process_exit":report["process_exit"],"physical_zero_verified":false}),
                    });
                } else {
                    self.stopping.store(false, Ordering::Release);
                }
                outcome
            }
            _ => Err(HostError::new(
                "HostMethod",
                "This Host method is not implemented; raw worker RPC is never accepted",
            )),
        }
    }
    fn finalize_stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_waiters();
    }
}
fn fields(params: &Value, expected: &[&str]) -> Result<(), HostError> {
    if params.as_object().is_some_and(|object| {
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
    }) {
        Ok(())
    } else {
        Err(HostError::new(
            "HostProtocol",
            "Unexpected or missing parameters",
        ))
    }
}
fn text_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, HostError> {
    params[key]
        .as_str()
        .ok_or_else(|| HostError::new("HostProtocol", format!("{key} must be text")))
}
fn uint_param(params: &Value, key: &str) -> Result<u64, HostError> {
    params[key]
        .as_u64()
        .filter(|n| *n <= super::contracts::MAX_SEQUENCE)
        .ok_or_else(|| HostError::new("HostProtocol", format!("{key} must be a JS-safe integer")))
}
fn recording_name(params: &Value) -> Result<&str, HostError> {
    let name = match params["args"].get("archive_name") {
        None => "osa",
        Some(value) => value
            .as_str()
            .ok_or_else(|| HostError::new("OperationInvalid", "Recording name must be text"))?,
    };
    if !super::archive::valid_name(name) {
        return Err(HostError::new("OperationInvalid","Recording name must be 1-40 ASCII letters, digits, hyphen or underscore, beginning with a letter or digit"));
    }
    Ok(name)
}
fn instrument_params(method: &str, params: &Value) -> Value {
    let mut result = params.clone();
    if method == "action" && matches!(params["name"].as_str(), Some("acquire" | "read_trace")) {
        if let Some(args) = result["args"].as_object_mut() {
            args.remove("archive_name");
        }
    }
    result
}
fn validate_action(kind: &str, name: &str, args: &Value) -> Result<(), HostError> {
    let schema = match (kind, name) {
        ("osa", "acquire" | "read_trace") => (vec![], vec!["trace", "archive_name"]),
        ("voltage", "set_channel") => (vec!["channel", "voltage"], vec![]),
        ("voltage", "set_all") => (vec!["values"], vec![]),
        ("gain", "set_temperature") => (vec!["temperature_c"], vec![]),
        ("gain", "set_current") => (vec!["current_ma"], vec![]),
        ("gain", "wait_stable") => (vec![], vec!["timeout_s"]),
        ("pm400", "measure_kind") => (vec!["kind"], vec![]),
        ("pm400", "read_setting") => (vec!["setting"], vec!["group", "selector"]),
        ("pm400", "write_setting") => (
            vec!["setting"],
            vec!["value", "group", "selector", "confirm"],
        ),
        ("pm400", "run_maintenance") => (vec!["command"], vec!["confirm"]),
        ("fiber", "move") => (vec!["side"], vec!["dx", "dy", "dz"]),
        ("fiber", "adopt_baseline") => (vec!["side", "confirm"], vec!["allow_nominal"]),
        ("laser", "set_remote") => (vec!["remote", "confirm"], vec![]),
        ("laser", "set_output" | "set_tracking") => (vec!["enabled", "confirm"], vec![]),
        ("laser", "set_wavelength") => (vec!["wavelength_nm", "confirm"], vec![]),
        ("laser", "set_piezo") => (vec!["percent", "confirm"], vec![]),
        ("voltage", "zero")
        | ("gain", "enable_tec" | "disable_tec" | "enable_current" | "disable_current")
        | ("pm400", "measure_power")
        | ("mdt", "read_status") | ("laser", "read_status") => (vec![], vec![]),
        _ => {
            return Err(HostError::new(
                "OperationInvalid",
                "Unreviewed typed action",
            ))
        }
    };
    let object = args
        .as_object()
        .ok_or_else(|| HostError::new("OperationInvalid", "Typed arguments must be an object"))?;
    if !schema.0.iter().all(|key| object.contains_key(*key))
        || object
            .keys()
            .any(|key| !schema.0.contains(&key.as_str()) && !schema.1.contains(&key.as_str()))
    {
        return Err(HostError::new(
            "OperationInvalid",
            "Unexpected or missing typed arguments",
        ));
    }
    let bounded = |value: &Value, min: f64, max: f64| {
        value
            .as_f64()
            .is_some_and(|n| n.is_finite() && n >= min && n <= max)
    };
    let valid = match (kind, name) {
        ("osa", "acquire" | "read_trace") => {
            recording_name(&json!({"args":args})).is_ok()
                && (!object.contains_key("trace")
                    || args["trace"]
                        .as_str()
                        .is_some_and(|t| matches!(t, "A" | "B" | "C" | "D" | "E" | "F" | "G")))
        }
        ("pm400", "measure_kind" | "read_setting" | "write_setting" | "run_maintenance") => {
            ["kind", "setting", "command", "group", "selector"]
                .iter()
                .all(|k| {
                    !object.contains_key(*k)
                        || args[*k]
                            .as_str()
                            .is_some_and(|v| !v.is_empty() && v.len() <= 128)
                })
                && (!object.contains_key("confirm") || args["confirm"].is_boolean())
                && (!object.contains_key("value")
                    || args["value"].is_boolean()
                    || args["value"].is_string()
                    || args["value"].as_f64().is_some_and(f64::is_finite))
        }
        ("voltage", "set_channel") => {
            args["channel"]
                .as_u64()
                .is_some_and(|n| (1..=8).contains(&n))
                && bounded(&args["voltage"], 0.0, 14.0)
        }
        ("voltage", "set_all") => args["values"]
            .as_array()
            .is_some_and(|v| v.len() == 8 && v.iter().all(|n| bounded(n, 0.0, 14.0))),
        ("gain", "set_current") => bounded(&args["current_ma"], 0.0, 200.0),
        ("laser", "set_remote") => args["confirm"] == true && args["remote"].is_boolean(),
        ("laser", "set_output" | "set_tracking") => args["confirm"] == true && args["enabled"].is_boolean(),
        ("laser", "set_wavelength") => args["confirm"] == true && bounded(&args["wavelength_nm"], 1.0, 5000.0),
        ("laser", "set_piezo") => args["confirm"] == true && bounded(&args["percent"], 0.0, 100.0),
        ("gain", "set_temperature") => bounded(&args["temperature_c"], 15.0, 40.0),
        ("gain", "wait_stable") => {
            !object.contains_key("timeout_s") || bounded(&args["timeout_s"], 0.05, 180.0)
        }
        ("fiber", _) => {
            args["side"]
                .as_str()
                .is_some_and(|side| matches!(side, "left" | "right"))
                && if name == "adopt_baseline" {
                    args["confirm"] == true
                        && (!object.contains_key("allow_nominal")
                            || args["allow_nominal"].is_boolean())
                } else {
                    ["dx", "dy", "dz"]
                        .iter()
                        .all(|key| !object.contains_key(*key) || bounded(&args[*key], -1.0, 1.0))
                }
        }
        _ => true,
    };
    if !valid {
        return Err(HostError::new(
            "OperationInvalid",
            "Invalid value or safety limit",
        ));
    };
    Ok(())
}
fn parse_domain(value: &Value) -> Result<DomainRef, HostError> {
    let domain: DomainRef = serde_json::from_value(value.clone())
        .map_err(|error| HostError::new("DomainIdentity", error.to_string()))?;
    domain.validate()?;
    Ok(domain)
}
fn empty(params: &Value) -> Result<(), HostError> {
    if params == &json!({}) {
        Ok(())
    } else {
        Err(HostError::new("HostProtocol", "Unexpected parameters"))
    }
}
async fn serve_client(mut pipe: NamedPipeServer, core: Arc<HostCore>) -> Result<(), HostError> {
    let first = tokio::time::timeout(Duration::from_secs(15), read_frame(&mut pipe))
        .await
        .map_err(|_| HostError::new("ClientTimeout", "Handshake timeout"))??
        .ok_or_else(|| HostError::new("ClientOffline", "Client closed before handshake"))?;
    let first = parse_request(&first)?;
    let empty_join = json!({});
    let join = core
        .clients
        .lock()
        .unwrap()
        .join(if first.method == "ping" {
            &first.params
        } else {
            &empty_join
        });
    let channel = match join {
        Ok(c) => c,
        Err(e) => {
            write_frame(&mut pipe, &HostReply::from_result(first.id.clone(), Err(e))).await?;
            let _ = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut pipe)).await;
            return Ok(());
        }
    };
    let result = serve_channel(&mut pipe, core.clone(), &channel, first).await;
    let revoke = core.clients.lock().unwrap().leave(&channel);
    if let Some(session) = revoke {
        if !core.stopped.load(Ordering::Acquire) {
            let _ = core.leases.lock().unwrap().close_session(&session);
            core.schedule_cleanups();
        }
    }
    result
}
async fn serve_channel(
    pipe: &mut NamedPipeServer,
    core: Arc<HostCore>,
    channel: &super::sessions::ClientChannel,
    first: HostRequest,
) -> Result<(), HostError> {
    let mut initial = Some(first);
    while !core.stopped.load(Ordering::Acquire) {
        let is_initial = initial.is_some();
        let request = match initial.take() {
            Some(request) => request,
            None => {
                let Some(bytes) = read_frame(pipe).await? else {
                    return Ok(());
                };
                parse_request(&bytes)?
            }
        };
        let active = core.clients.lock().unwrap().active(channel.session.id());
        let allowed = match channel.channel.as_str() {
            "heartbeat" => matches!(request.method.as_str(), "ping" | "renew_control"),
            "events" => matches!(request.method.as_str(), "ping" | "subscribe"),
            "results" => matches!(
                request.method.as_str(),
                "ping"
                    | "read_result"
                    | "operation"
                    | "request_snapshot"
                    | "list_archives"
                    | "read_archive"
                    | "archive_manifest"
                    | "archive_manifest_bytes"
            ),
            "safety" => matches!(
                request.method.as_str(),
                "ping" | "safe_stop" | "release_control" | "close_client" | "stop"
            ),
            _ => true,
        };
        if !allowed
            || (!active
                && !matches!(
                    request.method.as_str(),
                    "ping" | "safe_stop" | "stop" | "close_client" | "operation" | "read_result"
                ))
        {
            write_frame(
                pipe,
                &HostReply::from_result(
                    request.id,
                    Err(HostError::new(
                        "SessionRevoked",
                        "Channel or session is not authorized",
                    )),
                ),
            )
            .await?;
            continue;
        }
        if request.method == "subscribe" {
            empty(&request.params)?;
            let stream = core.events.subscribe(&channel.session)?;
            core.event_streams.fetch_add(1, Ordering::AcqRel);
            let _stream_guard = EventStreamGuard(core.clone());
            write_frame(
                pipe,
                &HostReply::from_result(request.id, Ok(json!({"stream":true}))),
            )
            .await?;
            super::events::write_event(pipe, &stream.snapshot).await?;
            loop {
                if !core.clients.lock().unwrap().active(channel.session.id()) {
                    return Ok(());
                }
                if let Some(event) = stream.try_next()? {
                    tokio::time::timeout(
                        Duration::from_secs(5),
                        super::events::write_event(pipe, &event),
                    )
                    .await
                    .map_err(|_| {
                        HostError::new("SlowConsumer", "Event write deadline expired")
                    })??;
                } else {
                    if core.stopped.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
        let outcome = if request.method == "read_result" {
            fields(&request.params, &["domain", "id", "offset", "length"])?;
            let domain = parse_domain(&request.params["domain"])?;
            if !core.bindings.lock().unwrap().contains_key(&domain) {
                return Err(HostError::new("ResultScope", "Unknown domain"));
            }
            let id = text_param(&request.params, "id")?.to_string();
            let offset = uint_param(&request.params, "offset")?;
            let length = u32::try_from(uint_param(&request.params, "length")?)
                .map_err(|_| HostError::new("ResultScope", "Invalid chunk length"))?;
            let session = channel.session.clone();
            core.results
                .call::<Value>(Box::new(move |store| {
                    Ok(
                        serde_json::to_value(store.read(&session, &domain, &id, offset, length)?)
                            .unwrap(),
                    )
                }))
                .await
        } else {
            let mut cleaned = request.clone();
            if is_initial && request.method == "ping" {
                cleaned.params = json!({});
            }
            core.dispatch(&cleaned, channel.session.id()).await
        };
        let mut response = HostReply::from_result(request.id.clone(), outcome);
        if is_initial && request.method == "ping" && response.ok {
            response.result["attach_token"] = json!(channel.attach_token);
        }
        let stop = request.method == "stop" && response.ok;
        let limit = if request.method == "read_result" {
            600 * 1024
        } else {
            super::ipc::MAX_FRAME
        };
        let write = tokio::time::timeout(
            Duration::from_secs(5),
            write_frame_limit(pipe, &response, limit),
        )
        .await;
        if stop {
            core.finalize_stop();
        }
        write.map_err(|_| HostError::new("ClientTimeout", "Client reply timed out"))??;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recording_names_are_bound_to_the_host_operation_not_sent_to_the_instrument() {
        for name in ["osa", "Run_7", "a-1", &"a".repeat(40)] {
            let args = json!({"trace":"A","archive_name":name});
            assert!(validate_action("osa", "read_trace", &args).is_ok());
            let params = json!({"name":"read_trace","args":args});
            assert_eq!(recording_name(&params).unwrap(), name);
            assert_eq!(
                instrument_params("action", &params),
                json!({"name":"read_trace","args":{"trace":"A"}})
            );
            assert_eq!(params["args"]["archive_name"], name);
        }
        for name in [
            "",
            "../outside",
            "a/b",
            "a.b",
            "_a",
            "光谱",
            &"a".repeat(41),
        ] {
            assert!(validate_action("osa", "read_trace", &json!({"archive_name":name})).is_err());
        }
        assert!(validate_action("osa", "acquire", &json!({"archive_name":7})).is_err());
        assert_eq!(
            recording_name(&json!({"name":"acquire","args":{}})).unwrap(),
            "osa"
        );
        assert_eq!(
            instrument_params("connect", &json!({"acknowledge_lifecycle":true})),
            json!({"acknowledge_lifecycle":true})
        );
    }
    #[test]
    fn osa_read_action_accepts_only_typed_trace_not_private_staging() {
        assert!(validate_action("osa", "read_trace", &json!({"trace":"G"})).is_ok());
        assert!(validate_action("osa", "read_trace", &json!({"trace":"H"})).is_err());
        assert!(validate_action("osa", "read_trace", &json!({"path":"outside"})).is_err());
        assert!(validate_action("osa", "read_capture_chunk", &json!({})).is_err());
        assert!(validate_action("osa", "ack_capture", &json!({})).is_err());
    }
    #[test]
    fn real_only_host_rejects_obsolete_mode_before_any_startup_effect() {
        let result = HostService::start(HostConfig {
            root: PathBuf::from("must-not-open"),
            record_dir: PathBuf::from("must-not-create"),
            python: PathBuf::from("must-not-launch"),
            mode: "simulate".into(),
        });
        assert!(matches!(result, Err(ref error) if error.code == "HostMode"));
    }
}
