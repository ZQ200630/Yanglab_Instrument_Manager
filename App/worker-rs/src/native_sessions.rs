//! Lazy production sessions: construction is metadata-only; admitted connect/probe owns I/O.
use crate::{
    actions::{Action, GainAction, VoltageAction},
    observations::Observation,
    session::{DeviceSession, DriverFactory, OsaSession, StopSignal},
    WorkerError,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use yang_drivers::{
    clock::Clock,
    gain::{GainConfig, GainDriver},
    lifecycle::{CleanupReport, CleanupStep, DriverLifecycle, DriverState, ProbeReport},
    mdt::{Mdt693b, MdtConfig},
    pm400::{Pm400, PmOptions},
    transport::{
        serial_abi::{NativeSerial, SerialBackend},
        visa_abi::VisaApi,
        Deadline, ResourceBook, VisaManager,
    },
    voltage::{VoltageConfig, VoltageSource},
    DriverError, DriverResult,
};
use yang_protocol::{ContextV3, DomainConfig, OutcomeV3, Phase};
use yang_setups::{FiberConfig, FiberCouplingSetup, MemberBinding, StageSide};
struct Resources {
    newport: Arc<crate::newport::Newport>,
    manager: Mutex<Option<VisaManager>>,
    book: ResourceBook,
    serial: Arc<dyn SerialBackend>,
    visa: Option<Arc<dyn VisaApi>>,
}
impl Resources {
    fn manager(&self) -> DriverResult<VisaManager> {
        let mut m = self.manager.lock().unwrap();
        if m.is_none() {
            *m = Some(match &self.visa {
                Some(api) => VisaManager::from_api(api.clone(), self.book.clone())?,
                None => VisaManager::load_system(self.book.clone())?,
            });
        }
        Ok(m.as_ref().unwrap().clone())
    }
}
pub struct SystemFactory {
    clock: Arc<dyn Clock>,
    resources: Arc<Resources>,
}
impl SystemFactory {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock: clock.clone(),
            resources: Arc::new(Resources {
                newport: crate::newport::Newport::system(clock),
                manager: Mutex::new(None),
                book: ResourceBook::default(),
                serial: Arc::new(NativeSerial),
                visa: None,
            }),
        }
    }
    /// Explicit finite transport injection for offline tests; not a selectable backend.
    pub fn with_backends(
        clock: Arc<dyn Clock>,
        serial: Arc<dyn SerialBackend>,
        visa: Arc<dyn VisaApi>,
    ) -> Self {
        Self {
            clock: clock.clone(),
            resources: Arc::new(Resources {
                newport: crate::newport::Newport::system(clock),
                manager: Mutex::new(None),
                book: ResourceBook::isolated(),
                serial,
                visa: Some(visa),
            }),
        }
    }
    /// Finite owner-thread transport injection only; never selected by app config.
    pub fn with_newport(
        clock: Arc<dyn Clock>,
        factory: Arc<dyn crate::newport::NewportFactory>,
        timeout: Duration,
    ) -> Self {
        Self {
            clock: clock.clone(),
            resources: Arc::new(Resources {
                newport: crate::newport::Newport::new(clock, factory, timeout),
                manager: Mutex::new(None),
                book: ResourceBook::isolated(),
                serial: Arc::new(NativeSerial),
                visa: None,
            }),
        }
    }
}
impl DriverFactory for SystemFactory {
    fn create(&self, config: &DomainConfig) -> Result<Box<dyn DeviceSession>, WorkerError> {
        let config = crate::catalog::admit(config)?;
        Ok(Box::new(LazySession {
            config,
            resources: self.resources.clone(),
            clock: self.clock.clone(),
            inner: None,
            stop: Arc::new(BoundStop::default()),
            identity: json!({}),
            closed: false,
        }))
    }
    fn auxiliary_responsibility(&self) -> bool {
        !self.resources.newport.released() || self.resources.manager.lock().unwrap().is_some()
            || yang_drivers::transport::visa::retained_count() != 0
    }
    fn newport_resources_released(&self) -> bool {
        self.resources.newport.released()
    }
    fn scan_lasers(&self) -> Result<Value, WorkerError> {
        match self.resources.newport.call(crate::newport::Command::Discover)
            .map_err(|e| WorkerError::new("ScanFailed", e.message))? {
            crate::newport::Reply::Controllers(ids) => Ok(json!({
                "controllers": ids.into_iter().map(|id| json!({
                    "device_key":format!("6700 SN{}", id.serial),
                    "serial":id.serial,
                    "head_model":id.head_model,
                    "head_serial":id.head_serial,
                })).collect::<Vec<_>>()
            })),
            _ => Err(WorkerError::new("ScanFailed", "Unexpected native discovery result")),
        }
    }
    fn finish_shutdown(&self) -> DriverResult<CleanupReport> {
        // Each retry is finite, does not restore outputs, and leaves pending owners retained.
        let mut steps = vec![];
        let mut remaining = vec![];
        if !self.resources.newport.released() {
            let release = self.resources.newport.call(crate::newport::Command::CloseAll);
            let released = self.resources.newport.released();
            if !released {
                remaining.push("newport:native".into());
            }
            steps.push(CleanupStep {
                role: "newport:native".into(),
                action: "preserving_close".into(),
                error: release.err().map(|e| e.message),
            });
        }
        for (role, count) in [
            ("voltage", yang_drivers::voltage::retry_retained()),
            ("gain", yang_drivers::gain::retry_retained()),
            ("serial", yang_drivers::transport::serial::retry_retained()),
        ] {
            if count != 0 {
                remaining.push(role.into());
            }
            steps.push(CleanupStep {
                role: role.into(),
                action: "retry_retained_release".into(),
                error: if count == 0 {
                    None
                } else {
                    Some("native owner remains retained".into())
                },
            });
        }
        for (role, reports) in [
            ("osa", yang_drivers::osa::retry_retained()),
            ("pm400", yang_drivers::pm400::retry_retained()),
            ("mdt", yang_drivers::mdt::retry_retained()),
        ] {
            for r in reports {
                let retained = r.as_ref().map_or(true, |r| !r.resources_released());
                if retained && !remaining.iter().any(|s| s == role) {
                    remaining.push(role.into());
                }
                steps.push(CleanupStep {
                    role: role.into(),
                    action: "retry_retained_release".into(),
                    error: r.err().map(|e| e.to_string()),
                });
            }
        }
        if yang_drivers::transport::visa::retry_retained() != 0 {
            remaining.push("visa_auxiliary".into());
        }
        let mut manager = self.resources.manager.lock().unwrap();
        let mut error = None;
        if let Some(pool) = manager.take() {
            if let Err(failure) = pool.release() {
                error = Some(failure.error.to_string());
                *manager = Some(failure.manager);
                remaining.push("visa_manager".into());
            }
        }
        steps.push(CleanupStep {
            role: "visa_manager".into(),
            action: "release_pool".into(),
            error,
        });
        CleanupReport::new(
            crate::new_id().map_err(|e| DriverError::Responsibility(e.to_string()))?,
            steps,
            None,
            remaining,
        )
    }
}
#[derive(Default)]
struct BoundStop {
    requested: AtomicBool,
    target: Mutex<Option<Arc<dyn StopSignal>>>,
}
impl StopSignal for BoundStop {
    fn cancel_operation(&self) {
        let target = self.target.lock().unwrap();
        if let Some(s) = target.as_ref() {
            s.cancel_operation();
        } else {
            self.requested.store(true, Ordering::Release);
        }
    }
    fn request_stop(&self) {
        self.requested.store(true, Ordering::Release);
        if let Some(s) = self.target.lock().unwrap().as_ref() {
            s.request_stop();
        }
    }
}
impl BoundStop {
    fn bind(&self, signal: Arc<dyn StopSignal>) {
        let mut target = self.target.lock().unwrap();
        if self.requested.load(Ordering::Acquire) {
            signal.request_stop();
        }
        *target = Some(signal);
    }
}
macro_rules! signal {($($t:path),*)=>{$(impl StopSignal for $t{fn request_stop(&self){<$t>::request_stop(self)}})*}}
signal!(
    yang_drivers::pm400::StopHandle,
    yang_drivers::mdt::StopHandle,
    yang_setups::StopHandle
);
impl StopSignal for yang_drivers::gain::StopHandle {
    fn request_stop(&self) {
        self.request_stop();
    }
    fn cancel_operation(&self) {
        self.cancel_operation();
    }
}
impl StopSignal for yang_drivers::voltage::StopHandle {
    fn request_stop(&self) {
        self.request_stop();
    }
    fn cancel_operation(&self) {
        self.cancel_operation();
    }
}
struct LazySession {
    config: DomainConfig,
    resources: Arc<Resources>,
    clock: Arc<dyn Clock>,
    inner: Option<Box<dyn DeviceSession>>,
    stop: Arc<BoundStop>,
    identity: Value,
    closed: bool,
}
impl LazySession {
    fn initialize(&mut self) -> DriverResult<()> {
        if self.closed || self.stop.requested.load(Ordering::Acquire) {
            return Err(DriverError::Responsibility(
                "stopped session cannot initialize".into(),
            ));
        }
        if self.inner.is_some() {
            return Ok(());
        }
        let p = &self.config.params;
        let port = p["port"].as_str().unwrap_or("").to_string();
        let timeout = Duration::from_secs_f64(p["io_timeout_s"].as_f64().unwrap_or(0.5));
        let r = &self.resources;
        let driver = match self.config.driver_kind.as_str() {
            "laser"=>{
                let adapter=crate::laser_session::LaserSession::new(self.config.clone(),r.newport.clone(),self.clock.clone());
                self.stop.bind(adapter.stop_signal());self.inner=Some(Box::new(adapter));return Ok(());
            }
            "osa" => {
                let d = yang_drivers::osa::Osa::with_options(
                    r.manager()?,
                    p["resource"].as_str().unwrap().into(),
                    self.clock.clone(),
                    yang_drivers::osa::OsaOptions {
                        timeout: Duration::from_secs_f64(p["timeout_s"].as_f64().unwrap_or(30.)),
                        close_timeout: Duration::from_secs(2),
                    },
                )?;
                self.inner = Some(Box::new(OsaSession::new(d)));
                self.stop.bind(self.inner.as_ref().unwrap().stop_signal());
                return Ok(());
            }
            "voltage" => Driver::Voltage(VoltageSource::with_backend(
                VoltageConfig {
                    port,
                    io_timeout: timeout,
                    ..VoltageConfig::default()
                },
                r.book.clone(),
                self.clock.clone(),
                r.serial.clone(),
            )?),
            "gain" => Driver::Gain(GainDriver::with_backend(
                GainConfig {
                    port: Some(port),
                    usb_serial: None,
                    io_timeout: Duration::from_secs_f64(p["io_timeout_s"].as_f64().unwrap_or(1.)),
                    ..GainConfig::default()
                },
                r.book.clone(),
                self.clock.clone(),
                r.serial.clone(),
            )?),
            "pm400" => Driver::Pm(Pm400::with_options(
                r.manager()?,
                p["resource"].as_str().unwrap().into(),
                self.clock.clone(),
                PmOptions {
                    timeout: Duration::from_secs_f64(p["timeout_s"].as_f64().unwrap_or(2.)),
                    close_timeout: Duration::from_secs(2),
                },
            )?),
            "mdt" => Driver::Mdt(Mdt693b::with_backend(
                MdtConfig {
                    port,
                    io_timeout: timeout,
                    ..MdtConfig::default()
                },
                r.book.clone(),
                self.clock.clone(),
                r.serial.clone(),
            )?),
            "fiber" => {
                let bindings: Vec<_> = self
                    .config
                    .members
                    .iter()
                    .map(|m| MemberBinding {
                        serial: m
                            .expected_identity
                            .get("serial")
                            .or_else(|| m.expected_identity.get("transport_serial"))
                            .and_then(Value::as_str)
                            .unwrap()
                            .into(),
                        port: m.params["port"].as_str().unwrap().into(),
                    })
                    .collect();
                let serials = bindings
                    .iter()
                    .map(|b| b.serial.clone())
                    .collect::<Vec<_>>();
                let config = FiberConfig::default()
                    .with_limits(
                        p["toward_chip_limit_um"].as_f64().unwrap_or(0.2),
                        p["other_limit_um"].as_f64().unwrap_or(1.),
                        p["voltage_limit_v"].as_f64().unwrap_or(75.),
                    )?
                    .with_timeout(Duration::from_secs_f64(
                        p["serial_timeout_s"].as_f64().unwrap_or(0.5),
                    ))?
                    .with_bindings(bindings)?;
                Driver::Fiber(FiberCouplingSetup::with_backend(
                    &serials,
                    config,
                    r.book.clone(),
                    self.clock.clone(),
                    r.serial.clone(),
                )?)
            }
            _ => {
                return Err(DriverError::Invalid(
                    "catalog driver missing native adapter".into(),
                ))
            }
        };
        let adapter = TypedSession {
            driver,
            clock: self.clock.clone(),
            identity: json!({}),
            last: None,
            fields: None,
            last_sample: None,
            revision: 0,
            gain_cache: None,
            voltage_cache: None,
        };
        self.stop.bind(adapter.stop_signal());
        self.inner = Some(Box::new(adapter));
        Ok(())
    }
    fn open(&mut self, readonly: bool) -> DriverResult<ProbeReport> {
        self.initialize()?;
        let d = self.inner.as_mut().unwrap();
        let result = if readonly {
            d.probe_readonly()
        } else {
            d.connect()
        };
        self.stop.bind(d.stop_signal());
        if let Ok(r) = &result {
            self.identity = r.identity().clone();
        }
        if self.stop.requested.load(Ordering::Acquire) {
            return Err(DriverError::Responsibility(
                "connection finished after stop intent; cleanup required".into(),
            ));
        }
        result
    }
}
impl DriverLifecycle for LazySession {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        self.closed = true;
        self.stop.request_stop();
        if let Some(inner) = &mut self.inner {
            inner.close()
        } else {
            CleanupReport::new(
                crate::new_id().map_err(|e| DriverError::Responsibility(e.to_string()))?,
                vec![CleanupStep {
                    role: self.config.driver_kind.clone(),
                    action: "no_transport_opened".into(),
                    error: None,
                }],
                None,
                vec![],
            )
        }
    }
    fn has_responsibility(&self) -> bool {
        self.inner.as_ref().is_some_and(|d| d.has_responsibility())
    }
}
impl DeviceSession for LazySession {
    fn cached_observer(&self) -> Option<Arc<dyn crate::session::CachedObserver>> {
        self.inner.as_ref().and_then(|d| d.cached_observer()).map(|inner| Arc::new(LazyCachedObserver { inner, stop:self.stop.clone() }) as Arc<dyn crate::session::CachedObserver>)
    }
    fn connect(&mut self) -> DriverResult<ProbeReport> {
        self.open(false)
    }
    fn probe_readonly(&mut self) -> DriverResult<ProbeReport> {
        self.open(true)
    }
    fn state(&self) -> DriverState {
        self.inner
            .as_ref()
            .map_or(DriverState::Disconnected, |d| d.state())
    }
    fn health_error(&self) -> Option<DriverError> {
        self.inner.as_ref().and_then(|d| d.health_error())
    }
    fn identity(&self) -> Value {
        self.identity.clone()
    }
    fn stop_signal(&self) -> Arc<dyn StopSignal> {
        self.stop.clone()
    }
    fn action(&mut self, n: &str, a: &Value, c: &ContextV3) -> OutcomeV3 {
        self.action_with_fence(n,a,c,None)
    }
    fn action_with_fence(&mut self, n: &str, a: &Value, c: &ContextV3, fence:Option<u64>) -> OutcomeV3 {
        if self.stop.requested.load(Ordering::Acquire) || self.closed {
            return crate::backend::failed(
                Some(c.clone()),
                Phase::RejectedBeforeCall,
                WorkerError::new("Unhealthy", "stopped session"),
            );
        }
        match self.inner.as_mut() {
            Some(d) => d.action_with_fence(n, a, c, fence),
            None => crate::backend::failed(
                Some(c.clone()),
                Phase::RejectedBeforeCall,
                WorkerError::new("Disconnected", "session not connected"),
            ),
        }
    }
    fn observe(&mut self, c: &ContextV3) -> Observation {
        let mut o = self.inner.as_mut().map_or(
            Observation {
                status: json!({"state":"DISCONNECTED"}),
                more: false,
                sampled_at: Some(self.clock.now()),
            },
            |d| d.observe(c),
        );
        if self.stop.requested.load(Ordering::Acquire) && self.has_responsibility() {
            o.status["state"] = json!("CLOSING");
            o.status["quality"] = json!("unknown");
            o.status["connected"] = json!(false);
            if let Some(fields) = o.status["fields"].as_object_mut() {
                for f in fields.values_mut() {
                    f["quality"] = json!("unknown");
                    f["reason"] = json!("stop intent; current state not confirmed");
                }
            }
        }
        o
    }
    fn take_capture(&mut self) -> Option<yang_drivers::osa::TraceCapture> {
        self.inner.as_mut().and_then(|d| d.take_capture())
    }
}
enum Driver {
    Voltage(VoltageSource),
    Gain(GainDriver),
    Pm(Pm400),
    Mdt(Mdt693b),
    Fiber(FiberCouplingSetup),
}
struct LazyCachedObserver {
    inner: Arc<dyn crate::session::CachedObserver>,
    stop: Arc<BoundStop>,
}
impl crate::session::CachedObserver for LazyCachedObserver {
    fn generation(&self)->Option<u64> { self.inner.generation() }
    fn observe(&self, context: &ContextV3) -> Observation {
        let mut observed = self.inner.observe(context);
        if self.stop.requested.load(Ordering::Acquire) {
            observed.status["state"] = json!("CLOSING");
            observed.status["quality"] = json!("unknown");
            observed.status["connected"] = json!(false);
            if let Some(fields) = observed.status["fields"].as_object_mut() {
                for field in fields.values_mut() { field["quality"] = json!("unknown"); }
            }
            if observed.status["pid"].is_object() { observed.status["pid"]["quality"] = json!("unknown"); }
        }
        observed
    }
}
struct GainEvidence {
    context: Option<ContextV3>,
    store: Option<crate::observations::EvidenceStore>,
    sample: Option<Duration>,
    revision: u64,
    invalidated: bool,
}
struct GainCachedObserver {
    handle: yang_drivers::gain::GainCacheHandle,
    clock: Arc<dyn Clock>,
    identity: Value,
    evidence: Mutex<GainEvidence>,
}
struct VoltageCachedObserver {
    handle: yang_drivers::voltage::VoltageCacheHandle,
    clock: Arc<dyn Clock>,
    identity: Value,
}
impl crate::session::CachedObserver for VoltageCachedObserver {
    // Do not expose Voltage's command/cancel generation: metadata access must
    // preserve the existing action fence and safety behavior.
    fn observe(&self, _context: &ContextV3) -> Observation {
        let Some(cache) = self.handle.snapshot() else {
            return Observation {status:json!({"state":"DISCONNECTED","connected":false,"quality":"unknown"}),more:false,sampled_at:None};
        };
        let now = self.clock.now();
        let at = cache.status.as_ref().map(|sample| sample.received_at);
        let age = at.and_then(|at| now.checked_sub(at));
        let healthy = matches!(cache.state,DriverState::Ready|DriverState::Active);
        let quality = if !healthy || age.is_none() {"unknown"}
            else if age.is_some_and(|age| age <= cache.telemetry_timeout) {"fresh"} else {"stale"};
        let fault = cache.fault.as_ref().map(ToString::to_string);
        let error = fault.clone().or_else(|| (quality!="fresh").then(|| "voltage telemetry is stale or unavailable".to_string()));
        Observation {status:json!({"state":cache.state,"connected":healthy,"identity":self.identity,"cached":true,
            "voltage_v":cache.status.as_ref().map(|sample|sample.voltage_v),
            "current_ma":cache.status.as_ref().map(|sample|sample.current_ma),
            "received_at":at.map(|at|at.as_secs_f64()),"observed_age_s":age.map(|age|age.as_secs_f64()),
            "quality":quality,"voltage_unit":"V","current_unit":"mA",
            "requested_voltage_v":cache.commanded_voltage_v,"zero_evidence":cache.zero_evidence,
            "fault":fault,"status_error":error}),more:false,sampled_at:at}
    }
}
impl GainCachedObserver {
    fn new(handle:yang_drivers::gain::GainCacheHandle,clock:Arc<dyn Clock>,identity:Value)->Self {
        Self {handle,clock,identity,evidence:Mutex::new(GainEvidence {context:None,store:None,sample:None,revision:0,invalidated:false})}
    }
    fn invalidate(&self) {
        let mut evidence=self.evidence.lock().unwrap();
        evidence.invalidated=true;
        evidence.revision+=1;
        if let Some(store)=&mut evidence.store {
            let _=store.invalidate(&["temperature_c","target_c","current_ma","tec_enabled","current_enabled"],"command started; await a later observed snapshot");
        }
    }
}
impl crate::session::CachedObserver for GainCachedObserver {
    fn generation(&self)->Option<u64> { self.handle.generation() }
    fn observe(&self, context:&ContextV3)->Observation {
        let Some(cache)=self.handle.snapshot() else {
            return Observation {status:json!({"state":"DISCONNECTED","connected":false,"quality":"unknown"}),more:false,sampled_at:None};
        };
        let now=self.clock.now();let at=cache.status.as_ref().map(|s|s.received_at);
        let age=at.and_then(|at|now.checked_sub(at)).map(|age|age.as_secs_f64());
        let quality=if !matches!(cache.state,DriverState::Ready|DriverState::Active) {"unknown"} else if age.is_some_and(|age|age<=1.5) {"fresh"} else {"stale"};
        let mut evidence=self.evidence.lock().unwrap();
        if evidence.context.as_ref().is_some_and(|old|old!=context && (old.session_id!=context.session_id || old.domain!=context.domain || old.connection_id!=context.connection_id || old.epoch>=context.epoch)) {
            return Observation {status:json!({"state":"DISCONNECTED","connected":false,"quality":"unknown"}),more:false,sampled_at:None};
        }
        if evidence.context.as_ref()!=Some(context) {
            evidence.context=Some(context.clone());
            evidence.store=crate::observations::EvidenceStore::new(context.clone(),self.clock.clone()).ok();
            evidence.sample=None;
        }
        if let Some(sample)=&cache.status {
            if evidence.sample!=Some(sample.received_at)&&sample.received_at<=now {
                let skip=evidence.invalidated&&evidence.sample.is_none();
                if !skip {
                    evidence.revision+=1;let revision=evidence.revision;
                    if let Some(store)=&mut evidence.store {
                        for (name,value) in [("temperature_c",json!(sample.temperature_c)),("target_c",json!(sample.target_c)),("current_ma",json!(sample.current_ma)),("tec_enabled",json!(sample.tec_enabled)),("current_enabled",json!(sample.current_enabled))] {
                            let _=store.record(name,value,sample.received_at,revision);
                        }
                    }
                    evidence.invalidated=false;
                }
                evidence.sample=Some(sample.received_at);
            }
        }
        let mut fields=evidence.store.as_ref().map(|store|store.snapshot()).unwrap_or_default();
        for field in fields.values_mut() { if quality!="fresh" {field["quality"]=json!(quality);} }
        let pid=cache.pid.map(|pid|json!({"values":pid.values,"connection_id":context.connection_id,"revision":pid.revision,"quality":if matches!(cache.state,DriverState::Ready|DriverState::Active){"fresh"}else{"unknown"},"observed_age_s":now.saturating_sub(pid.received_at).as_secs_f64(),"error":cache.fault.as_ref().map(ToString::to_string)}));
        let operation=cache.current_operation.map(|op|json!({"kind":op.kind,"phase":op.phase,"active":op.active,"target_ma":op.target_ma,"current_ma":op.current_ma,"steps_completed":op.steps_completed,"steps_total":op.steps_total,"elapsed_s":op.finished_at.unwrap_or(now).saturating_sub(op.started_at).as_secs_f64(),"error":op.error}));
        Observation {status:json!({"state":cache.state,"connected":matches!(cache.state,DriverState::Ready|DriverState::Active),"identity":self.identity,"cached":true,"last_status":cache.status,"fields":fields,"quality":quality,"observed_age_s":age,"temperature_unit":"degC","current_unit":"mA","fault":cache.fault.map(|e|e.to_string()),"pid":pid,"current_operation":operation}),more:false,sampled_at:at}
    }
}
struct TypedSession {
    driver: Driver,
    clock: Arc<dyn Clock>,
    identity: Value,
    last: Option<Value>,
    fields: Option<crate::observations::EvidenceStore>,
    last_sample: Option<Duration>,
    revision: u64,
    gain_cache: Option<Arc<GainCachedObserver>>,
    voltage_cache: Option<Arc<VoltageCachedObserver>>,
}
impl DriverLifecycle for TypedSession {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        match &mut self.driver {
            Driver::Voltage(d) => d.close(),
            Driver::Gain(d) => d.close(),
            Driver::Pm(d) => d.close(),
            Driver::Mdt(d) => d.close(),
            Driver::Fiber(d) => d.close(),
        }
    }
    fn has_responsibility(&self) -> bool {
        match &self.driver {
            Driver::Voltage(d) => d.has_resource_responsibility(),
            Driver::Gain(d) => d.has_resource_responsibility(),
            Driver::Pm(d) => d.has_resource_responsibility(),
            Driver::Mdt(d) => d.has_resource_responsibility(),
            Driver::Fiber(d) => d.has_resource_responsibility(),
        }
    }
}
impl TypedSession {
    fn open(&mut self, readonly: bool) -> DriverResult<ProbeReport> {
        let report = match &mut self.driver {
            Driver::Voltage(d) => {
                if readonly {
                    d.probe_identity()
                } else {
                    d.connect()
                }
            }
            Driver::Gain(d) => {
                if readonly {
                    d.probe_identity()
                } else {
                    d.connect()
                }
            }
            Driver::Pm(d) => {
                if readonly {
                    d.probe_identity()
                } else {
                    d.connect()
                }
            }
            Driver::Mdt(d) => {
                if readonly {
                    d.probe_identity()
                } else {
                    d.connect()
                }
            }
            Driver::Fiber(d) => d.connect(),
        }?;
        self.identity = report.identity().clone();
        if let Driver::Gain(d) = &self.driver {
            self.gain_cache = Some(Arc::new(GainCachedObserver::new(d.cache_handle(),self.clock.clone(),self.identity.clone())));
        }
        if let Driver::Voltage(d) = &self.driver {
            self.voltage_cache = Some(Arc::new(VoltageCachedObserver {handle:d.cache_handle(),clock:self.clock.clone(),identity:self.identity.clone()}));
        }
        Ok(report)
    }
    fn kind(&self) -> &str {
        match &self.driver {
            Driver::Voltage(_) => "voltage",
            Driver::Gain(_) => "gain",
            Driver::Pm(_) => "pm400",
            Driver::Mdt(_) => "mdt",
            Driver::Fiber(_) => "fiber",
        }
    }
    fn run(&mut self, action: Action, fence:Option<u64>) -> DriverResult<Value> {
        Ok(match (&mut self.driver, action) {
            (Driver::Voltage(d), Action::Voltage(a)) => {
                match a {
                    VoltageAction::Channel(c, v) => d.set_channel(c, v)?,
                    VoltageAction::All(v) => d.set_all(v)?,
                    VoltageAction::Zero => d.zero(true)?,
                }
                json!({"commanded_voltage_v":d.commanded_voltages(),"zero_evidence":d.zero_evidence()})
            }
            (Driver::Gain(d), Action::Gain(a)) => {
                let work = |d:&mut GainDriver|->DriverResult<Value> {
                match a {
                    GainAction::ReadPid => { d.read_pid()?; }
                    GainAction::SetPid(v) => { d.set_pid(v[0],v[1],v[2])?; }
                    GainAction::Ramp {current,step,interval} => { d.ramp_current(current,step,interval)?; }
                    GainAction::Start {current,soft_start,step,interval,timeout} => { d.start_current(current,soft_start,step,interval,timeout)?; }
                    GainAction::Temperature(v) => {
                        d.set_temperature(v)?;
                    }
                    GainAction::Current(v) => {
                        d.set_current(v)?;
                    }
                    GainAction::EnableTec => {
                        d.enable_tec()?;
                    }
                    GainAction::DisableTec => {
                        d.disable_tec()?;
                    }
                    GainAction::DisableCurrent => {
                        d.disable_current()?;
                    }
                    GainAction::EnableCurrent => {
                        d.enable_current()?;
                    }
                    GainAction::Stable(t) => d.wait_stable(Deadline::after(t))?,
                }
                Ok(json!({"status":d.status()}))
                };
                if let Some(generation)=fence { d.with_operation_fence(generation,work)? } else { work(d)? }
            }
            (Driver::Pm(d), Action::Pm(a)) => {
                crate::pm_ops::execute(d, a, Deadline::after(Duration::from_secs(180)))?
            }
            (Driver::Mdt(d), Action::MdtStatus) => {
                d.get_all_voltages()?;
                json!({"status":d.status()})
            }
            (
                Driver::Fiber(d),
                Action::FiberAdopt {
                    side,
                    allow_nominal,
                },
            ) => {
                let a = d.stage(side).baseline_attestation(true, allow_nominal)?;
                json!({"stage":d.adopt_baseline(side,a)?})
            }
            (Driver::Fiber(d), Action::FiberMove { side, delta }) => {
                json!({"move":d.move_by_um(side,delta)?})
            }
            _ => return Err(DriverError::Invalid("typed action/driver mismatch".into())),
        })
    }
}
impl DeviceSession for TypedSession {
    fn cached_observer(&self) -> Option<Arc<dyn crate::session::CachedObserver>> {
        self.gain_cache.clone().map(|cache|cache as Arc<dyn crate::session::CachedObserver>)
            .or_else(||self.voltage_cache.clone().map(|cache|cache as Arc<dyn crate::session::CachedObserver>))
    }
    fn connect(&mut self) -> DriverResult<ProbeReport> {
        self.open(false)
    }
    fn probe_readonly(&mut self) -> DriverResult<ProbeReport> {
        self.open(true)
    }
    fn state(&self) -> DriverState {
        match &self.driver {
            Driver::Voltage(d) => d.state(),
            Driver::Gain(d) => d.state(),
            Driver::Pm(d) => d.state(),
            Driver::Mdt(d) => d.state(),
            Driver::Fiber(d) => d.state(),
        }
    }
    fn health_error(&self) -> Option<DriverError> {
        match &self.driver {
            Driver::Gain(d) => d.fault_error(),
            _ => None,
        }
    }
    fn identity(&self) -> Value {
        self.identity.clone()
    }
    fn stop_signal(&self) -> Arc<dyn StopSignal> {
        match &self.driver {
            Driver::Voltage(d) => Arc::new(d.stop_handle()),
            Driver::Gain(d) => Arc::new(d.stop_handle()),
            Driver::Pm(d) => Arc::new(d.stop_handle()),
            Driver::Mdt(d) => Arc::new(d.stop_handle()),
            Driver::Fiber(d) => Arc::new(d.stop_handle()),
        }
    }
    fn action(&mut self, n: &str, a: &Value, c: &ContextV3) -> OutcomeV3 {
        self.action_with_fence(n,a,c,None)
    }
    fn action_with_fence(&mut self, n: &str, a: &Value, c: &ContextV3, fence:Option<u64>) -> OutcomeV3 {
        let action = match crate::actions::parse(self.kind(), n, a) {
            Ok(a) => a,
            Err(e) => return crate::backend::failed(Some(c.clone()), Phase::RejectedBeforeCall, e),
        };
        if matches!(action, Action::Gain(_)) && !matches!(action, Action::Gain(GainAction::ReadPid | GainAction::SetPid(_))) {
            if let Some(cache) = &self.gain_cache { cache.invalidate(); }
            if let Some(store) = &mut self.fields {
                let _ = store.invalidate(
                    &[
                        "temperature_c",
                        "target_c",
                        "current_ma",
                        "tec_enabled",
                        "current_enabled",
                    ],
                    "command started; await a later observed snapshot",
                );
                self.revision += 1;
            }
        }
        match self.run(action,fence) {
            Ok(v) => {
                self.last = Some(v.clone());
                crate::backend::completed(
                    Some(c.clone()),
                    json!({"result":v,"status":self.observe(c).status}),
                )
            }
            Err(e) => {
                let error=if matches!(e,DriverError::Canceled) {WorkerError::new("Canceled",e.to_string())} else {e.into()};
                crate::backend::failed(Some(c.clone()), Phase::FailedAfterCallStarted, error)
            }
        }
    }
    fn observe(&mut self, context: &ContextV3) -> Observation {
        if let Some(cache) = &self.voltage_cache {
            return crate::session::CachedObserver::observe(cache.as_ref(),context);
        }
        if let Some(cache) = &self.gain_cache {
            return crate::session::CachedObserver::observe(cache.as_ref(),context);
        }
        let state = self.state();
        let now = self.clock.now();
        let mut at = None;
        let data = match &mut self.driver {
            Driver::Voltage(d) => match d.read_status(Duration::from_secs(1)) {
                Ok(s) => {
                    at = Some(s.received_at);
                    json!({"voltage_v":s.voltage_v,"current_ma":s.current_ma,"received_at":s.received_at.as_secs_f64(),"quality":"fresh","observed_age_s":now.saturating_sub(s.received_at).as_secs_f64(),"voltage_unit":"V","current_unit":"mA","requested_voltage_v":d.commanded_voltages(),"zero_evidence":d.zero_evidence()})
                }
                Err(e) => {
                    json!({"quality":"unknown","voltage_v":null,"current_ma":null,"requested_voltage_v":null,"status_error":e.to_string(),"zero_evidence":d.zero_evidence()})
                }
            },
            Driver::Gain(d) => {
                let s = d.status();
                at = s.as_ref().map(|s| s.received_at);
                let age = at.and_then(|t| now.checked_sub(t)).map(|v| v.as_secs_f64());
                let quality = if !matches!(state, DriverState::Ready | DriverState::Active) {
                    "unknown"
                } else if age.is_some_and(|v| v <= 1.5) {
                    "fresh"
                } else {
                    "stale"
                };
                if self.fields.is_none() {
                    self.fields = crate::observations::EvidenceStore::new(
                        context.clone(),
                        self.clock.clone(),
                    )
                    .ok();
                }
                if let (Some(s), Some(store)) = (&s, &mut self.fields) {
                    if self.last_sample != Some(s.received_at) && s.received_at <= now {
                        self.revision += 1;
                        for (name, value) in [
                            ("temperature_c", json!(s.temperature_c)),
                            ("target_c", json!(s.target_c)),
                            ("current_ma", json!(s.current_ma)),
                            ("tec_enabled", json!(s.tec_enabled)),
                            ("current_enabled", json!(s.current_enabled)),
                        ] {
                            let _ = store.record(name, value, s.received_at, self.revision);
                        }
                        self.last_sample = Some(s.received_at);
                    }
                }
                let mut fields = self
                    .fields
                    .as_ref()
                    .map(|s| s.snapshot())
                    .unwrap_or_default();
                for f in fields.values_mut() {
                    if quality != "fresh" {
                        f["quality"] = json!(quality);
                    }
                }
                json!({"last_status":s,"fields":fields,"quality":quality,"observed_age_s":age,"temperature_unit":"degC","current_unit":"mA","fault":d.fault_error().map(|e|e.to_string())})
            }
            Driver::Pm(d) => {
                json!({"sensor":d.sensor_info(),"last_result":self.last,"catalog":crate::pm_ops::catalog(d)})
            }
            Driver::Mdt(d) => {
                let s = d.status();
                at = s.as_ref().and_then(|s| {
                    if s.observed_at.is_finite() && s.observed_at >= 0. {
                        Some(Duration::from_secs_f64(s.observed_at))
                    } else {
                        None
                    }
                });
                json!({"status":s,"quality":if matches!(state,DriverState::Ready|DriverState::Active){"cached"}else{"unknown"},"voltage_unit":"V"})
            }
            Driver::Fiber(d) => {
                json!({"left":d.stage(StageSide::Left).status(),"right":d.stage(StageSide::Right).status(),"position_kind":"session_open_loop_estimate","position_unit":"um","voltage_unit":"V"})
            }
        };
        let mut status = data.as_object().cloned().unwrap_or_default();
        status.insert("state".into(), json!(state));
        status.insert(
            "connected".into(),
            json!(matches!(state, DriverState::Ready | DriverState::Active)),
        );
        status.insert("identity".into(), self.identity.clone());
        status.insert("cached".into(), json!(true));
        Observation {
            status: Value::Object(status),
            more: false,
            sampled_at: at,
        }
    }
}
