use crate::{
    captures::CaptureSpool,
    catalog::{admit, ClaimToken, Claims},
    discovery::{Discovery, InventoryPort},
    observations::Observation,
    scheduler::Backend,
    session::{DriverFactory, InstrumentSession, StopSignal},
    verification::{ProbeAuthorization, ProbePort, Verifier},
    DomainRegistry, WorkerError,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
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
    pending_capture: Option<TraceCapture>,
    controller: Option<String>,
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
    probes: Mutex<HashMap<DomainRef, Arc<Mutex<ProbeSlot>>>>,
    spool: Mutex<Option<Arc<Mutex<CaptureSpool>>>>,
    reports: Mutex<HashMap<DomainRef, CleanupReport>>,
    capture_obligations: Mutex<HashMap<String, Option<DomainRef>>>,
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
    ) -> Option<crate::verification::SupervisedSnapshot> {
        let backend = self.backend.upgrade()?;
        let slot = backend.slots.lock().ok()?.get(&config.domain)?.clone();
        let slot = slot.try_lock().ok()?;
        slot.session.check_health().ok()?;
        Some(crate::verification::SupervisedSnapshot {
            context: backend.registry.context(&config.domain).ok()?,
            controller: slot.controller.clone()?,
            report: ProbeReport::new(
                slot.session.driver.identity(),
                json!({"state":slot.session.driver.state(),"cached":true}),
                false,
            ),
        })
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
                probes: Mutex::new(HashMap::new()),
                spool: Mutex::new(None),
                reports: Mutex::new(HashMap::new()),
                capture_obligations: Mutex::new(HashMap::new()),
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
    fn handle(&self, request: &RequestV3) -> Result<Value, WorkerError> {
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
                let proof = slot.session.connect().map_err(WorkerError::from)?;
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
                Ok(json!({"connected":true,"status":{"state":"READY","identity":proof.identity()}}))
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
                if released {
                    self.claims.confirm_release(&slot.claim, &cleanup)?;
                    self.capture_obligations
                        .lock()
                        .unwrap()
                        .retain(|_, domain| domain.as_ref() != Some(&c.domain));
                    drop(slot);
                    self.slots.lock().unwrap().remove(&c.domain);
                    self.stops.lock().unwrap().remove(&c.domain);
                }
                Ok(json!({"connected":!released,"cleanup":cleanup}))
            }
            "action" => {
                let c = config.as_ref().unwrap();
                let name = params["name"].as_str().unwrap();
                validate_action(&c.driver_kind, name, &params["args"])?;
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
                slot.session.check_health()?;
                if c.driver_kind != "osa" {
                    let outcome = slot.session.driver.action(
                        name,
                        &params["args"],
                        request.context.as_ref().unwrap(),
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
                slot.pending_capture = Some(capture);
                let stage_started = std::time::Instant::now();
                let staged = spool
                    .lock()
                    .unwrap()
                    .stage(slot.pending_capture.as_ref().unwrap())
                    .map_err(|e| {
                        WorkerError::new(
                            "CaptureStagingFailed",
                            format!("Read completed, staging failed: {e}; do not repeat"),
                        )
                    })?;
                slot.pending_capture = None;
                {
                    let mut obligations = self.capture_obligations.lock().unwrap();
                    obligations.remove(&capture_key);
                    obligations.insert(staged.capture_id.clone(), None);
                }
                let value = result.result.take().unwrap_or_else(|| json!({}));
                let mut value = value.as_object().cloned().unwrap_or_default();
                value.insert("capture".into(), serde_json::to_value(staged).unwrap());
                let mut timing = value
                    .remove("timing")
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                timing.insert(
                    "stage_s".into(),
                    json!(stage_started.elapsed().as_secs_f64()),
                );
                value.insert("timing".into(), Value::Object(timing));
                Ok(Value::Object(value))
            }
            "shutdown" => Ok(json!({"steps":[],"unreleased":[],"voltage_zero":null})),
            _ => Err(WorkerError::new(
                "UnsupportedMethod",
                "No raw/interpreter/fixture endpoint",
            )),
        }
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
    fn registry(&self) -> DomainRegistry {
        self.registry.clone()
    }
    fn install_spool(&self, spool: Arc<Mutex<CaptureSpool>>) {
        let mut current = self.spool.lock().unwrap();
        assert!(current.is_none(), "one spool per backend");
        *current = Some(spool);
    }
    fn execute(&self, request: &RequestV3) -> OutcomeV3 {
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
        match self.handle(request) {
            Ok(value) => completed(request.context.clone(), value),
            Err(e) => {
                let phase = if ["connect", "disconnect", "action", "probe", "check_online"]
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
                            | "StaleContext"
                    ) {
                    Phase::FailedAfterCallStarted
                } else {
                    Phase::RejectedBeforeCall
                };
                failed(request.context.clone(), phase, e)
            }
        }
    }
    fn observe(&self, context: &ContextV3) -> Observation {
        let slot = context
            .domain
            .as_ref()
            .and_then(|d| self.slots.lock().unwrap().get(d).cloned());
        if let Some(slot) = slot {
            let mut slot = slot.lock().unwrap();
            if self.registry.matches(context) {
                let mut o = slot.session.driver.observe(context);
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
    fn cleanup_reports(&self) -> Vec<CleanupReport> {
        self.reports.lock().unwrap().values().cloned().collect()
    }
    fn auxiliary_responsibility(&self) -> bool {
        !self.probes.lock().unwrap().is_empty() || self.factory.auxiliary_responsibility()
    }
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
