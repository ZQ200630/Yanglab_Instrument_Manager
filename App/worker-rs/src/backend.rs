use crate::{
    captures::CaptureSpool,
    catalog::{admit, ClaimToken, Claims},
    discovery::{Discovery, InventoryPort},
    observations::Observation,
    scheduler::Backend,
    session::{CachedObserver, DriverFactory, InstrumentSession, StopSignal},
    verification::{ProbeAuthorization, ProbePort, Verifier},
    DomainRegistry, WorkerError,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
};
use yang_drivers::{
    clock::Clock,
    lifecycle::{CleanupReport, CleanupStep, DriverLifecycle, DriverState, ProbeReport},
    osa::TraceCapture,
};
use yang_protocol::{ContextV3, DomainConfig, DomainRef, OutcomeV3, Phase, RequestV3, WireError};
struct Slot {
    session: InstrumentSession,
    claim: ClaimToken,
    pending_capture: Option<PendingCapture>,
    controller: Option<String>,
}
fn same_connection(owner:&ContextV3,current:&ContextV3)->bool {
    owner.session_id==current.session_id&&owner.domain==current.domain&&owner.connection_id.is_some()&&owner.connection_id==current.connection_id&&owner.epoch<=current.epoch
}
struct PendingCapture {
    data: TraceCapture,
    key: String,
    context: ContextV3,
    request_id: String,
    result: Value,
    config_rev: u64,
}
impl PendingCapture {
    fn ticket(&self) -> Value {
        json!({"capture_id":self.key,"source_context":self.context,"original_request_id":self.request_id})
    }
}
struct ProbeSlot {
    session: InstrumentSession,
}
pub struct NativeBackend {
    registry: DomainRegistry,
    factory: Arc<dyn DriverFactory>,
    clock: Arc<dyn Clock>,
    discovery: Discovery,
    verifier: Verifier,
    claims: Claims,
    slots: Mutex<HashMap<DomainRef, Arc<Mutex<Slot>>>>,
    stops: Mutex<HashMap<DomainRef, Arc<dyn StopSignal>>>,
    cached_observers: Mutex<HashMap<DomainRef, (ContextV3, Arc<dyn CachedObserver>)>>,
    probes: Mutex<HashMap<DomainRef, Arc<Mutex<ProbeSlot>>>>,
    spool: Mutex<Option<Arc<Mutex<CaptureSpool>>>>,
    reports: Mutex<HashMap<DomainRef, CleanupReport>>,
    failed_laser_connects: Mutex<HashMap<DomainRef, (ContextV3, CleanupReport)>>,
    capture_obligations: Mutex<HashMap<String, Option<DomainRef>>>,
    recovery: Mutex<HashMap<String, (DomainRef, u64, Value)>>,
    closing: AtomicBool,
}
struct Port {
    registry: DomainRegistry,
    backend: Weak<NativeBackend>,
}
impl ProbePort for Port {
    fn registry(&self) -> DomainRegistry {
        self.registry.clone()
    }
    fn probe_readonly(
        &self,
        config: &DomainConfig,
        _: &ProbeAuthorization,
    ) -> Result<ProbeReport, WorkerError> {
        self.backend
            .upgrade()
            .ok_or_else(|| WorkerError::new("Closed", "backend released"))?
            .probe(config)
    }
    fn supervised_snapshot(
        &self,
        config: &DomainConfig,
    ) -> Result<Option<crate::verification::SupervisedSnapshot>, WorkerError> {
        let backend = self.backend.upgrade().ok_or_else(|| WorkerError::new("Closed", "backend released"))?;
        let slot = backend.slots.lock()
            .map_err(|_| WorkerError::new("Unhealthy", "controlled session cache lock poisoned"))?
            .get(&config.domain).cloned();
        let Some(slot) = slot else { return Ok(None); };
        let slot = slot.try_lock().map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => WorkerError::new("ResourceBusy", "controlled session is busy; cached proof is unavailable"),
            std::sync::TryLockError::Poisoned(_) => WorkerError::new("Unhealthy", "controlled session lock poisoned"),
        })?;
        slot.session.check_health()?;
        let Some(controller) = slot.controller.clone() else { return Ok(None); };
        Ok(Some(crate::verification::SupervisedSnapshot {
            context: backend.registry.context(&config.domain)?,
            controller,
            report: ProbeReport::new(
                slot.session.driver.identity(),
                json!({"state":slot.session.driver.state(),"cached":true}),
                false,
            ),
        }))
    }
}
pub(crate) fn failed(context: Option<ContextV3>, phase: Phase, error: WorkerError) -> OutcomeV3 {
    OutcomeV3 {
        phase,
        context,
        result: None,
        error: Some(WireError {
            kind: error.code,
            message: error.message,
            attempt_id: None,
        }),
    }
}
pub(crate) fn completed(context: Option<ContextV3>, result: Value) -> OutcomeV3 {
    OutcomeV3 {
        phase: Phase::Completed,
        context,
        result: Some(result),
        error: None,
    }
}
fn report(role: &str, error: Option<String>, released: bool) -> CleanupReport {
    CleanupReport::new(
        crate::new_id().expect("secure cleanup identity"),
        vec![CleanupStep {
            role: role.into(),
            action: "close".into(),
            error,
        }],
        None,
        if released { vec![] } else { vec![role.into()] },
    )
    .expect("bounded cleanup report")
}
impl NativeBackend {
    pub fn system() -> Result<Arc<Self>, WorkerError> {
        let clock: Arc<dyn Clock> = Arc::new(yang_drivers::clock::SystemClock::default());
        Ok(Self::with_ports(
            DomainRegistry::new(&crate::new_id()?)?,
            clock.clone(),
            Arc::new(crate::session::SystemFactory::new(clock)),
            Arc::new(crate::discovery::SystemInventory),
        ))
    }
    pub fn with_ports(
        registry: DomainRegistry,
        clock: Arc<dyn Clock>,
        factory: Arc<dyn DriverFactory>,
        inventory: Arc<dyn InventoryPort>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| {
            let verifier = Verifier::new(
                Arc::new(Port {
                    registry: registry.clone(),
                    backend: weak.clone(),
                }),
                clock.clone(),
            );
            let claims = verifier.claims();
            Self {
                registry,
                factory,
                clock,
                discovery: Discovery::new(inventory),
                verifier,
                claims,
                slots: Mutex::new(HashMap::new()),
                stops: Mutex::new(HashMap::new()),
                cached_observers: Mutex::new(HashMap::new()),
                probes: Mutex::new(HashMap::new()),
                spool: Mutex::new(None),
                reports: Mutex::new(HashMap::new()),
                failed_laser_connects: Mutex::new(HashMap::new()),
                capture_obligations: Mutex::new(HashMap::new()),
                recovery: Mutex::new(HashMap::new()),
                closing: AtomicBool::new(false),
            }
        })
    }
    pub fn global_context(&self) -> ContextV3 {
        ContextV3 {
            session_id: self.registry.session_id().into(),
            domain: None,
            connection_id: None,
            epoch: 0,
        }
    }
    fn probe(&self, config: &DomainConfig) -> Result<ProbeReport, WorkerError> {
        let driver = match self.factory.create(config) {
            Ok(driver) => driver,
            Err(error) => {
                let cleanup = report(&config.driver_kind, Some(error.to_string()), true);
                self.verifier
                    .confirm_probe_release(&config.domain, &cleanup)?;
                self.reports
                    .lock()
                    .unwrap()
                    .insert(config.domain.clone(), cleanup);
                return Err(error);
            }
        };
        let slot = Arc::new(Mutex::new(ProbeSlot {
            session: InstrumentSession::new(driver),
        }));
        self.probes
            .lock()
            .unwrap()
            .insert(config.domain.clone(), slot.clone());
        let mut state = slot.lock().unwrap();
        let result = state.session.probe_readonly();
        let connect_failed = result.is_err();
        let cleanup = state.session.close();
        let released = cleanup
            .as_ref()
            .is_ok_and(CleanupReport::resources_released)
            && !state.session.has_responsibility();
        if let Ok(report) = &cleanup {
            self.reports
                .lock()
                .unwrap()
                .insert(config.domain.clone(), report.clone());
        }
        let output = result.map_err(WorkerError::from).and_then(|r| {
            if released {
                Ok(ProbeReport::new(
                    r.identity().clone(),
                    r.observations().clone(),
                    true,
                ))
            } else {
                Err(WorkerError::new(
                    "ReleaseUnconfirmed",
                    "Probe retains native resources",
                ))
            }
        });
        drop(state);
        if released {
            if connect_failed {
                self.verifier
                    .confirm_probe_release(&config.domain, cleanup.as_ref().unwrap())?;
            }
            self.probes.lock().unwrap().remove(&config.domain);
        }
        output
    }
    fn current(&self, request: &RequestV3) -> Result<Option<DomainConfig>, WorkerError> {
        let context = request
            .context
            .as_ref()
            .ok_or_else(|| WorkerError::new("ContextMismatch", "Context required"))?;
        if context.domain.is_some() {
            if !self.registry.matches(context) {
                return Err(WorkerError::new(
                    "StaleContext",
                    "Session/connection/epoch changed",
                ));
            }
            Ok(Some(
                self.registry.config(context.domain.as_ref().unwrap())?,
            ))
        } else if *context != self.global_context() {
            Err(WorkerError::new(
                "ContextMismatch",
                "Worker session changed",
            ))
        } else {
            Ok(None)
        }
    }
    fn handle(&self, request: &RequestV3, fence:Option<u64>) -> Result<Value, WorkerError> {
        let config = self.current(request)?;
        let params = &request.params;
        match request.method.as_str() {
            "configure_domain" => {
                let config: DomainConfig = serde_json::from_value(params["config"].clone())
                    .map_err(|e| WorkerError::new("ProtocolError", e.to_string()))?;
                let approved = admit(&config)?;
                let context = self.registry.configure(approved)?;
                Ok(json!({"context":context,"configured":true,"software_supported":true}))
            }
            "retire_domain" => {
                let domain: DomainRef = serde_json::from_value(params["domain"].clone())
                    .map_err(|e| WorkerError::new("ProtocolError", e.to_string()))?;
                self.registry
                    .retire(&domain, params["config_rev"].as_u64().unwrap())?;
                Ok(json!({"retired":true}))
            }
            "register_verified" => {
                let domain: DomainRef = serde_json::from_value(params["domain"].clone())
                    .map_err(|e| WorkerError::new("ProtocolError", e.to_string()))?;
                self.verifier.register_verified_for(
                    &domain,
                    params["proof_id"].as_str().unwrap(),
                    params["config_digest"].as_str().unwrap(),
                    params["config_rev"].as_u64().unwrap(),
                )?;
                Ok(json!({"identity_bound":true}))
            }
            "probe" | "check_online" => {
                let c = config.as_ref().unwrap();
                let authorization: ProbeAuthorization =
                    serde_json::from_value(params["authorization"].clone())
                        .map_err(|e| WorkerError::new("Authorization", e.to_string()))?;
                let proof = self.verifier.probe(c, &authorization)?;
                let proof = serde_json::to_value(proof)
                    .map_err(|e| WorkerError::new("InvalidEvidence", e.to_string()))?;
                Ok(
                    json!({"release_confirmed":proof["release_confirmed"],"proof":proof,"online":true}),
                )
            }
            "inventory" => serde_json::to_value(self.discovery.inventory())
                .map_err(|e| WorkerError::new("Inventory", e.to_string())),
            "scan_lasers" => self.factory.scan_lasers(),
            "read_capture_chunk" | "ack_capture" => {
                let spool =
                    self.spool.lock().unwrap().clone().ok_or_else(|| {
                        WorkerError::new("CaptureUnavailable", "Spool not installed")
                    })?;
                let mut spool = spool.lock().unwrap();
                let nonce = params["ownership_nonce"].as_str().unwrap();
                let id = params["capture_id"].as_str().unwrap();
                if request.method == "ack_capture" {
                    spool.ack(nonce, id, params["sha256"].as_str().unwrap())?;
                    self.capture_obligations.lock().unwrap().remove(id);
                    self.recovery.lock().unwrap().remove(id);
                    Ok(json!({"acknowledged":true}))
                } else {
                    let offset = params["offset"].as_u64().unwrap();
                    let length = params["length"].as_u64().unwrap() as usize;
                    let bytes = spool.read_chunk(nonce, id, offset, length)?;
                    Ok(
                        json!({"capture_id":id,"offset":offset,"byte_count":length,"sha256":spool.descriptor(id)?.sha256,"data_hex":bytes.iter().map(|b|format!("{b:02x}")).collect::<String>()}),
                    )
                }
            }
            "connect" => {
                let c = config.as_ref().unwrap();
                let context = request.context.as_ref().unwrap();
                let bound = self.registry.snapshot().iter().any(|snapshot| {
                    snapshot.context == *context
                        && snapshot.context.connection_id.is_some()
                        && snapshot.state == DriverState::Connecting
                        && snapshot.responsibility
                });
                if !bound {
                    return Err(WorkerError::new(
                        "StaleContext",
                        "A reserved connection is required before native opening",
                    ));
                }
                let (approved, controller) = if let Some(auth) = params.get("authorization") {
                    let auth: ProbeAuthorization = serde_json::from_value(auth.clone())
                        .map_err(|e| WorkerError::new("Authorization", e.to_string()))?;
                    let approved = self.verifier.authorize_supervised_connection(c, &auth)?;
                    (
                        approved,
                        auth.binding["controller"].as_str().map(str::to_owned),
                    )
                } else {
                    (self.verifier.authorize_connection(c)?, None)
                };
                let claim = self.claims.reserve(&approved)?;
                let driver = match self.factory.create(&approved) {
                    Ok(driver) => driver,
                    Err(e) => {
                        let evidence = report("factory", Some(e.to_string()), true);
                        self.claims.confirm_release(&claim, &evidence)?;
                        if c.driver_kind == "laser" {
                            self.failed_laser_connects.lock().unwrap().insert(
                                c.domain.clone(), (context.clone(), evidence.clone()),
                            );
                        }
                        self.reports
                            .lock()
                            .unwrap()
                            .insert(c.domain.clone(), evidence);
                        return Err(e);
                    }
                };
                let stop = driver.stop_signal();
                let slot = Arc::new(Mutex::new(Slot {
                    session: InstrumentSession::new(driver),
                    claim,
                    pending_capture: None,
                    controller,
                }));
                self.stops.lock().unwrap().insert(c.domain.clone(), stop);
                self.slots
                    .lock()
                    .unwrap()
                    .insert(c.domain.clone(), slot.clone());
                let mut slot = slot.lock().unwrap();
                let proof = match slot.session.connect() {
                    Ok(proof) => proof,
                    Err(error) => {
                        if c.driver_kind == "laser" {
                            // A failed full read may still own the shared Bus.
                            // Preserve every output while collecting the exact
                            // pending exchange and attempting normal release.
                            let cleanup = slot.session.close().unwrap_or_else(|error| {
                                report(
                                    "laser", Some(error.to_string()),
                                    !slot.session.has_responsibility(),
                                )
                            });
                            let released = cleanup.resources_released() && !slot.session.has_responsibility();
                            self.reports.lock().unwrap().insert(c.domain.clone(), cleanup.clone());
                            self.failed_laser_connects.lock().unwrap().insert(
                                c.domain.clone(), (context.clone(), cleanup.clone()),
                            );
                            if released {
                                self.claims.confirm_release(&slot.claim, &cleanup)?;
                                drop(slot);
                                self.slots.lock().unwrap().remove(&c.domain);
                                self.stops.lock().unwrap().remove(&c.domain);
                            }
                        }
                        return Err(error.into());
                    }
                };
                if !identity_matches(&c.expected_identity, proof.identity()) {
                    slot.session.request_stop();
                    return Err(WorkerError::new(
                        "IdentityMismatch",
                        "Current instrument differs from verified identity",
                    ));
                }
                if !self.registry.matches(request.context.as_ref().unwrap()) {
                    slot.session.request_stop();
                    return Err(WorkerError::new(
                        "StaleContext",
                        "Connection superseded during native call",
                    ));
                }
                self.registry
                    .publish(request.context.as_ref().unwrap(), DriverState::Ready);
                if let Some(observer)=slot.session.driver.cached_observer() {
                    self.cached_observers.lock().unwrap().insert(c.domain.clone(),(request.context.as_ref().unwrap().clone(),observer));
                }
                Ok(json!({
                    "connected":true,
                    "status":if c.driver_kind == "laser" {
                        proof.observations().clone()
                    } else {
                        json!({"state":"READY","identity":proof.identity()})
                    }
                }))
            }
            "disconnect" => {
                let c = config.as_ref().unwrap();
                let slot = self.slots.lock().unwrap().get(&c.domain).cloned();
                let Some(slot) = slot else {
                    return Ok(
                        json!({"connected":false,"cleanup":report(&c.driver_kind,None,true)}),
                    );
                };
                let mut slot = slot.lock().unwrap();
                let cleanup = match slot.session.close() {
                    Ok(report) => report,
                    Err(e) => report(
                        &c.driver_kind,
                        Some(e.to_string()),
                        !slot.session.has_responsibility(),
                    ),
                };
                self.reports
                    .lock()
                    .unwrap()
                    .insert(c.domain.clone(), cleanup.clone());
                let released = cleanup.resources_released() && !slot.session.has_responsibility();
                // EOF cannot accept another user command. Retry only the owned
                // storage transaction after native handles have been released.
                // Staged files remain unacknowledged on disk, never deleted here.
                let data_recovery = if released
                    && self.closing.load(Ordering::Acquire)
                    && slot.pending_capture.is_some()
                {
                    self.stage_pending(&mut slot).ok()
                } else {
                    None
                };
                let data_retained = slot.pending_capture.is_some();
                if released && !data_retained {
                    self.claims.confirm_release(&slot.claim, &cleanup)?;
                    self.capture_obligations
                        .lock()
                        .unwrap()
                        .retain(|_, domain| domain.as_ref() != Some(&c.domain));
                    drop(slot);
                    self.slots.lock().unwrap().remove(&c.domain);
                    self.stops.lock().unwrap().remove(&c.domain);
                    self.cached_observers.lock().unwrap().remove(&c.domain);
                }
                Ok(
                    json!({"connected":!released,"cleanup":cleanup,"data_retained":data_retained,"data_recovery":data_recovery}),
                )
            }
            "action" => {
                let c = config.as_ref().unwrap();
                let name = params["name"].as_str().unwrap();
                validate_action(&c.driver_kind, name, &params["args"])?;
                if name == "retry_staging" {
                    let id = params["args"]["capture_id"].as_str().unwrap();
                    if let Some((domain, rev, mut value)) =
                        self.recovery.lock().unwrap().get(id).cloned()
                    {
                        if domain != c.domain || rev != c.config_rev {
                            return Err(WorkerError::new(
                                "StaleCapture",
                                "Capture belongs to another configuration",
                            ));
                        }
                        let spool = self.spool.lock().unwrap().clone().unwrap();
                        let spool = spool.lock().unwrap();
                        spool.read_chunk(spool.ownership_nonce(), id, 0, 1)?;
                        value["connected"] = json!(self
                            .slots
                            .lock()
                            .unwrap()
                            .get(&domain)
                            .is_some_and(|s| s.lock().unwrap().session.check_health().is_ok()));
                        return Ok(value);
                    }
                }
                let slot = self
                    .slots
                    .lock()
                    .unwrap()
                    .get(&c.domain)
                    .cloned()
                    .ok_or_else(|| {
                        WorkerError::new("Disconnected", "No native instrument session")
                    })?;
                let mut slot = slot.lock().unwrap();
                if name == "retry_staging" {
                    let pending = slot.pending_capture.as_ref().ok_or_else(|| {
                        WorkerError::new("CaptureUnavailable", "No unstaged read remains")
                    })?;
                    if params["args"]["capture_id"] != pending.key {
                        return Err(WorkerError::new(
                            "StaleCapture",
                            "Recovery ticket differs from original read",
                        ));
                    }
                    return self.stage_pending(&mut slot);
                }
                slot.session.check_health()?;
                if c.driver_kind != "osa" {
                    let outcome = slot.session.driver.action_with_fence(
                        name,
                        &params["args"],
                        request.context.as_ref().unwrap(),
                        fence,
                    );
                    if outcome.phase != Phase::Completed {
                        return Err(WorkerError::new(
                            outcome
                                .error
                                .as_ref()
                                .map_or("DriverError", |e| e.kind.as_str()),
                            outcome
                                .error
                                .as_ref()
                                .map_or("action failed", |e| e.message.as_str()),
                        ));
                    }
                    if !self.registry.matches(request.context.as_ref().unwrap()) {
                        return Err(WorkerError::new(
                            "StaleContext",
                            "action finished after authority changed; effects are not replayed",
                        ));
                    }
                    self.registry.publish(
                        request.context.as_ref().unwrap(),
                        slot.session.driver.state(),
                    );
                    return Ok(outcome.result.unwrap_or(Value::Null));
                }
                if slot.pending_capture.is_some() {
                    return Err(WorkerError::new(
                        "CaptureUnstaged",
                        "A completed read remains unstaged; do not repeat it",
                    ));
                }
                let spool = self.spool.lock().unwrap().clone().ok_or_else(|| {
                    WorkerError::new(
                        "CaptureUnavailable",
                        "Staging must be installed before reading",
                    )
                })?;
                let capture_key = crate::new_id()?;
                {
                    let spool = spool.lock().unwrap();
                    let mut obligations = self.capture_obligations.lock().unwrap();
                    if obligations.len() >= 32 {
                        return Err(WorkerError::new("CaptureCapacity","Unacknowledged/unstaged captures fill the budget; no instrument read started"));
                    }
                    spool.ensure_read_capacity(
                        obligations
                            .values()
                            .filter(|domain| domain.is_some())
                            .count(),
                    )?;
                    obligations.insert(capture_key.clone(), Some(c.domain.clone()));
                }
                let mut result = slot.session.driver.action(
                    name,
                    &params["args"],
                    request.context.as_ref().unwrap(),
                );
                if result.phase != Phase::Completed {
                    self.capture_obligations
                        .lock()
                        .unwrap()
                        .remove(&capture_key);
                    return Err(WorkerError::new(
                        result
                            .error
                            .as_ref()
                            .map_or("DriverError", |e| e.kind.as_str()),
                        result
                            .error
                            .as_ref()
                            .map_or("Native action failed", |e| e.message.as_str()),
                    ));
                }
                let capture = match slot.session.driver.take_capture() {
                    Some(capture) => capture,
                    None => {
                        self.capture_obligations
                            .lock()
                            .unwrap()
                            .remove(&capture_key);
                        return Err(WorkerError::new(
                            "CaptureUnavailable",
                            "OSA action returned no owned capture",
                        ));
                    }
                };
                slot.pending_capture = Some(PendingCapture {
                    data: capture,
                    key: capture_key,
                    context: request.context.clone().unwrap(),
                    request_id: request.id.clone(),
                    config_rev: c.config_rev,
                    result: result.result.take().unwrap_or_else(|| json!({})),
                });
                self.stage_pending(&mut slot)
            }
            "shutdown" => Ok(json!({"steps":[],"unreleased":[],"voltage_zero":null})),
            _ => Err(WorkerError::new(
                "UnsupportedMethod",
                "No raw/interpreter/fixture endpoint",
            )),
        }
    }
    /// Storage-only: never enters a driver or replays an instrument acquisition.
    fn stage_pending(&self, slot: &mut Slot) -> Result<Value, WorkerError> {
        let pending = slot
            .pending_capture
            .as_ref()
            .ok_or_else(|| WorkerError::new("CaptureUnavailable", "No pending capture"))?;
        let spool = self
            .spool
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| WorkerError::new("CaptureUnavailable", "Staging unavailable"))?;
        let start = std::time::Instant::now();
        let staged = spool
            .lock()
            .unwrap()
            .stage_owned(&pending.key, &pending.data)
            .map_err(|e| {
                WorkerError::new(
                    "CaptureStagingFailed",
                    format!("Read completed; storage failed: {e}; retry storage only"),
                )
            })?;
        let mut value = pending.result.as_object().cloned().unwrap_or_default();
        value.insert("original_request_id".into(), json!(pending.request_id));
        value.insert("source_context".into(), json!(pending.context));
        value.insert("capture".into(), serde_json::to_value(&staged).unwrap());
        value.insert(
            "connected".into(),
            json!(slot.session.check_health().is_ok()),
        );
        let mut timing = value
            .remove("timing")
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        timing.insert("stage_s".into(), json!(start.elapsed().as_secs_f64()));
        value.insert("timing".into(), Value::Object(timing));
        let mut obligations = self.capture_obligations.lock().unwrap();
        obligations.remove(&pending.key);
        obligations.insert(staged.capture_id, None);
        self.recovery.lock().unwrap().insert(
            pending.key.clone(),
            (
                pending.context.domain.clone().unwrap(),
                pending.config_rev,
                Value::Object(value.clone()),
            ),
        );
        slot.pending_capture = None;
        Ok(Value::Object(value))
    }
}
fn identity_matches(expected: &Value, actual: &Value) -> bool {
    expected
        .as_object()
        .is_some_and(|fields| fields.iter().all(|(k, v)| actual.get(k) == Some(v)))
}
pub(crate) fn validate_action(kind: &str, name: &str, args: &Value) -> Result<(), WorkerError> {
    crate::actions::parse(kind, name, args).map(|_| ())
}
impl Backend for NativeBackend {
    fn take_failed_connect_cleanup(&self, context: &ContextV3) -> Option<CleanupReport> {
        context.domain.as_ref()
            .and_then(|d| self.failed_laser_connects.lock().unwrap().remove(d))
            .filter(|(attempt_context, _)| attempt_context == context)
            .map(|(_, report)| report)
    }
    fn begin_shutdown(&self) {
        self.closing.store(true, Ordering::Release);
    }
    fn registry(&self) -> DomainRegistry {
        self.registry.clone()
    }
    fn install_spool(&self, spool: Arc<Mutex<CaptureSpool>>) {
        let mut current = self.spool.lock().unwrap();
        assert!(current.is_none(), "one spool per backend");
        *current = Some(spool);
    }
    fn execute(&self, request: &RequestV3) -> OutcomeV3 {
        let fence=self.capture_action_fence(request);
        self.execute_fenced(request,fence)
    }
    fn execute_fenced(&self, request: &RequestV3, fence:Option<u64>) -> OutcomeV3 {
        if let Err(e) = serde_json::to_vec(request)
            .map_err(|e| WorkerError::new("ProtocolError", e.to_string()))
            .and_then(|bytes| yang_protocol::parse_request(&bytes).map_err(WorkerError::from))
        {
            return failed(request.context.clone(), Phase::RejectedBeforeCall, e);
        }
        let validation = self.current(request).and_then(|config| {
            if request.method == "action" {
                validate_action(
                    &config.unwrap().driver_kind,
                    request.params["name"].as_str().unwrap(),
                    &request.params["args"],
                )
            } else {
                Ok(())
            }
        });
        if let Err(e) = validation {
            return failed(request.context.clone(), Phase::RejectedBeforeCall, e);
        }
        match self.handle(request,fence) {
            Ok(value) => completed(request.context.clone(), value),
            Err(e) => {
                let phase = if ["connect", "disconnect", "action", "probe", "check_online", "scan_lasers"]
                    .contains(&request.method.as_str())
                    && !matches!(
                        e.code.as_str(),
                        "UnsupportedDriver"
                            | "UnsupportedAction"
                            | "InvalidArguments"
                            | "Disconnected"
                            | "CaptureCapacity"
                            | "CaptureUnavailable"
                            | "CaptureUnstaged"
                            | "VerificationRequired"
                            | "Authorization"
                            | "ResourceBusy"
                            | "Unhealthy"
                            | "StaleContext"
                    ) {
                    Phase::FailedAfterCallStarted
                } else {
                    Phase::RejectedBeforeCall
                };
                let mut outcome = failed(request.context.clone(), phase, e.clone());
                if e.code == "CaptureStagingFailed" {
                    outcome.phase = Phase::CompletedReadbackFailed;
                    if let Some(slot) = request
                        .context
                        .as_ref()
                        .and_then(|c| c.domain.as_ref())
                        .and_then(|d| self.slots.lock().unwrap().get(d).cloned())
                    {
                        if let Some(p) = slot.lock().unwrap().pending_capture.as_ref() {
                            outcome.result = Some(
                                json!({"unstaged_capture":p.ticket(),"hardware_read_completed":true,"retry_hardware":false,
                                "timing":p.result["timing"],"error":{"code":e.code,"message":e.message}}),
                            );
                        }
                    }
                }
                outcome
            }
        }
    }
    fn observe(&self, context: &ContextV3) -> Observation {
        if !self.registry.matches(context) {
            return Observation {status:json!({"state":"DISCONNECTED","connected":false,"quality":"unknown"}),more:false,sampled_at:Some(self.clock.now())};
        }
        if self.registry.matches(context) {
            let cached=context.domain.as_ref().and_then(|domain|self.cached_observers.lock().unwrap().get(domain).filter(|(owner,_)|same_connection(owner,context)).map(|(_,observer)|observer.clone()));
            if let Some(observer)=cached {
                let observed=observer.observe(context);
                if self.registry.matches(context) { return observed; }
                return Observation {status:json!({"state":"DISCONNECTED","connected":false,"quality":"unknown"}),more:false,sampled_at:Some(self.clock.now())};
            }
        }
        let slot = context
            .domain
            .as_ref()
            .and_then(|d| self.slots.lock().unwrap().get(d).cloned());
        if let Some(slot) = slot {
            let mut slot = slot.lock().unwrap();
            if self.registry.matches(context) {
                let mut o = slot.session.driver.observe(context);
                o.status["unstaged_capture"] = slot
                    .pending_capture
                    .as_ref()
                    .map(PendingCapture::ticket)
                    .unwrap_or(Value::Null);
                if o.sampled_at.is_none() {
                    o.sampled_at = Some(self.clock.now());
                }
                return o;
            }
        }
        Observation {
            status: json!({"state":"DISCONNECTED"}),
            more: false,
            sampled_at: Some(self.clock.now()),
        }
    }
    fn request_stop(&self, context: &ContextV3) {
        if let Some(stop) = context
            .domain
            .as_ref()
            .and_then(|d| self.stops.lock().unwrap().get(d).cloned())
        {
            stop.request_stop();
        }
    }
    fn has_cached_observer(&self,context:&ContextV3)->bool {
        self.registry.matches(context)&&context.domain.as_ref().is_some_and(|domain|self.cached_observers.lock().unwrap().get(domain).is_some_and(|(owner,_)|same_connection(owner,context)))
    }
    fn capture_action_fence(&self,request:&RequestV3)->Option<u64> {
        if request.method!="action" || matches!(request.params["name"].as_str(),Some("disable_current"|"disable_tec")) {return None;}
        let context=request.context.as_ref()?;
        if !self.registry.matches(context) {return None;}
        context.domain.as_ref().and_then(|domain|self.cached_observers.lock().unwrap().get(domain).filter(|(owner,_)|same_connection(owner,context)).and_then(|(_,observer)|observer.generation()))
    }
    fn request_safety(&self, context: &ContextV3, intent: crate::safety::SafetyIntent) {
        if intent == crate::safety::SafetyIntent::Disconnect {
            return self.request_stop(context);
        }
        if let Some(stop) = context
            .domain
            .as_ref()
            .and_then(|d| self.stops.lock().unwrap().get(d).cloned())
        {
            stop.cancel_operation();
        }
    }
    fn cleanup_reports(&self) -> Vec<CleanupReport> {
        self.reports.lock().unwrap().values().cloned().collect()
    }
    fn auxiliary_responsibility(&self) -> bool {
        !self.probes.lock().unwrap().is_empty() || self.factory.auxiliary_responsibility()
    }
    fn newport_resources_released(&self)->bool {self.factory.newport_resources_released()}
    fn finish_shutdown(&self) -> Result<(), WorkerError> {
        let probes: Vec<_> = self
            .probes
            .lock()
            .unwrap()
            .iter()
            .map(|(d, s)| (d.clone(), s.clone()))
            .collect();
        for (domain, slot) in probes {
            let mut slot = slot.lock().unwrap();
            let cleanup = slot.session.close()?;
            let released = cleanup.resources_released() && !slot.session.has_responsibility();
            self.reports
                .lock()
                .unwrap()
                .insert(domain.clone(), cleanup.clone());
            drop(slot);
            if released {
                self.verifier.confirm_probe_release(&domain, &cleanup)?;
                self.probes.lock().unwrap().remove(&domain);
            } else {
                return Err(WorkerError::new(
                    "ReleaseUnconfirmed",
                    "Probe cleanup remains retained",
                ));
            }
        }
        let pool = self.factory.finish_shutdown()?;
        let released = pool.resources_released();
        let domain = DomainRef {
            kind: "device".into(),
            id: "0".repeat(32),
        };
        self.reports.lock().unwrap().insert(domain, pool);
        if released {
            Ok(())
        } else {
            Err(WorkerError::new(
                "ReleaseUnconfirmed",
                "Native manager pool remains retained",
            ))
        }
    }
}
