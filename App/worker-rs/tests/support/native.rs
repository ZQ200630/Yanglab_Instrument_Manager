#![allow(dead_code)]
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};
use yang_drivers::{
    clock::SystemClock, lifecycle::*, osa::*, transport::serial_discovery::SerialDeviceInfo,
    DriverResult,
};
use yang_protocol::*;
use yang_worker::{
    backend::NativeBackend,
    captures::CaptureSpool,
    discovery::InventoryPort,
    dispatch::Worker,
    observations::Observation,
    scheduler::{Backend, Scheduler},
    session::*,
    DomainRegistry, WorkerError,
};
pub struct Fixture {
    pub parent: PathBuf,
    pub root: PathBuf,
    pub nonce: String,
}
impl Fixture {
    pub fn new() -> Self {
        let nonce = yang_worker::new_id().unwrap();
        let parent =
            std::env::temp_dir().join(format!("native-worker-{}", yang_worker::new_id().unwrap()));
        let root = parent.join(&nonce);
        fs::create_dir_all(&root).unwrap();
        Self {
            parent,
            root,
            nonce,
        }
    }
    pub fn spool(&self) -> CaptureSpool {
        CaptureSpool::open(self.root.clone(), self.nonce.clone()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.parent);
    }
}
#[derive(Default)]
pub struct Factory {
    pub creates: AtomicUsize,
    pub reads: AtomicUsize,
    pub failed_closes: AtomicUsize,
    pub open: AtomicUsize,
    pub failed_connects: AtomicUsize,
}
struct Signal(AtomicBool);
impl StopSignal for Signal {
    fn request_stop(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct Device {
    factory: Arc<Factory>,
    open: bool,
    state: DriverState,
    signal: Arc<Signal>,
    capture: Option<TraceCapture>,
}
pub struct FactoryPort(pub Arc<Factory>);
impl DriverFactory for FactoryPort {
    fn create(&self, config: &DomainConfig) -> Result<Box<dyn DeviceSession>, WorkerError> {
        if config.driver_kind != "osa" {
            return Err(WorkerError::new("UnsupportedDriver", "not ported"));
        }
        self.0.creates.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Device {
            factory: self.0.clone(),
            open: false,
            state: DriverState::Disconnected,
            signal: Arc::new(Signal(AtomicBool::new(false))),
            capture: None,
        }))
    }
}
pub fn report(released: bool) -> CleanupReport {
    CleanupReport::new(
        yang_worker::new_id().unwrap(),
        vec![CleanupStep {
            role: "osa".into(),
            action: "close".into(),
            error: (!released).then(|| "retained close".into()),
        }],
        None,
        if released { vec![] } else { vec!["osa".into()] },
    )
    .unwrap()
}
impl DriverLifecycle for Device {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        let fail = self
            .factory
            .failed_closes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if !fail && self.open {
            self.open = false;
            self.state = DriverState::Disconnected;
            self.factory.open.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(report(!fail))
    }
    fn has_responsibility(&self) -> bool {
        self.open
    }
}
fn capture() -> TraceCapture {
    let context = TraceContext::new(TraceContextParams {
        transfer_format: TransferFormat::Ascii,
        sample_count: 2,
        spacing: 0,
        level_unit: 0,
        x_unit: 0,
        trace_attribute: 0,
        active_trace: TraceId::A,
        center_m: 1.55e-6,
        span_m: 2e-9,
        resolution_m: 2e-11,
        sweep_mode: 1,
    })
    .unwrap();
    TraceCapture::new(
        vec![1549., 1550.],
        vec![-40., -30.],
        NativeUnit::Dbm,
        TraceId::A,
        "YOKOGAWA,AQ6370E,OFFLINE-WIRE,FW".into(),
        ReadTiming {
            started_utc: SystemTime::UNIX_EPOCH + Duration::from_secs(1791280800),
            finished_utc: SystemTime::UNIX_EPOCH + Duration::from_secs(1791280801),
            elapsed: Duration::from_secs(1),
            decode: Duration::from_millis(1),
            io: Duration::from_millis(900),
        },
        context.clone(),
        context,
    )
    .unwrap()
}
impl DeviceSession for Device {
    fn probe_readonly(&mut self) -> DriverResult<ProbeReport> {
        self.connect()
    }
    fn connect(&mut self) -> DriverResult<ProbeReport> {
        self.open = true;
        self.state = DriverState::Ready;
        self.factory.open.fetch_add(1, Ordering::SeqCst);
        if self
            .factory
            .failed_connects
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            self.state = DriverState::Fault;
            return Err(yang_drivers::DriverError::Protocol(
                "identity query failed".into(),
            ));
        }
        Ok(ProbeReport::new(self.identity(), json!({}), false))
    }
    fn state(&self) -> DriverState {
        self.state
    }
    fn identity(&self) -> Value {
        json!({"manufacturer":"YOKOGAWA","model":"AQ6370E","serial":"OFFLINE-WIRE"})
    }
    fn stop_signal(&self) -> Arc<dyn StopSignal> {
        self.signal.clone()
    }
    fn action(&mut self, name: &str, _args: &Value, context: &ContextV3) -> OutcomeV3 {
        assert!(matches!(name, "read_trace" | "acquire"));
        self.factory.reads.fetch_add(1, Ordering::SeqCst);
        self.capture = Some(capture());
        OutcomeV3 {
            phase: Phase::Completed,
            context: Some(context.clone()),
            result: Some(
                json!({"hardware_read_completed":true,"timing":{"io_s":0.9,"decode_s":0.001,"read_elapsed_s":1.}}),
            ),
            error: None,
        }
    }
    fn observe(&mut self, _: &ContextV3) -> Observation {
        Observation {
            status: json!({"state":"READY","identity":self.identity()}),
            more: false,
            sampled_at: None,
        }
    }
    fn take_capture(&mut self) -> Option<TraceCapture> {
        self.capture.take()
    }
}
struct Inventory;
impl InventoryPort for Inventory {
    fn serial(&self) -> Result<Vec<SerialDeviceInfo>, WorkerError> {
        panic!("unexpected enumeration")
    }
    fn visa(&self) -> Result<Vec<String>, WorkerError> {
        panic!("unexpected enumeration")
    }
}
pub fn backend() -> (Arc<NativeBackend>, Arc<Factory>) {
    let factory = Arc::new(Factory::default());
    let backend = NativeBackend::with_ports(
        DomainRegistry::new(&yang_worker::new_id().unwrap()).unwrap(),
        Arc::new(SystemClock::default()),
        Arc::new(FactoryPort(factory.clone())),
        Arc::new(Inventory),
    );
    (backend, factory)
}
pub fn worker(backend: Arc<NativeBackend>, fixture: &Fixture) -> Worker {
    let scheduler = Scheduler::new(
        backend.clone(),
        Arc::new(SystemClock::default()),
        Limits::default(),
    )
    .unwrap();
    Worker::new(backend, scheduler, fixture.spool())
}
pub fn config() -> DomainConfig {
    DomainConfig {
        domain: DomainRef {
            kind: "device".into(),
            id: "b".repeat(32),
        },
        config_rev: 1,
        driver_kind: "osa".into(),
        model_id: "aq6370".into(),
        profile_id: Some("gpib-visa".into()),
        params: json!({"resource":"GPIB0::4::INSTR"}),
        expected_identity: json!({}),
        members: vec![],
    }
}
pub fn request(id: &str, method: &str, params: Value, context: Option<ContextV3>) -> RequestV3 {
    RequestV3 {
        v: 3,
        id: id.into(),
        method: method.into(),
        params,
        context,
    }
}
pub fn configured(backend: &NativeBackend) -> DomainConfig {
    let c = config();
    let result = backend.execute(&request(
        "cfg",
        "configure_domain",
        json!({"config":c}),
        Some(backend.global_context()),
    ));
    assert_eq!(result.phase, Phase::Completed);
    c
}
pub fn registered(backend: &NativeBackend) -> DomainConfig {
    let c = configured(backend);
    let context = backend.registry().context(&c.domain).unwrap();
    let proof=backend.execute(&request("probe","probe",json!({"authorization":{"stage":"readonly","accepted":true,"supervised":false,"retain_session":false,"binding":{"mode":"real","domain":c.domain,"config_rev":1,"model_id":"aq6370","profile_id":"gpib-visa","config_digest":"a".repeat(64)}}}),Some(context)));
    assert_eq!(proof.phase, Phase::Completed, "{:?}", proof.error);
    let id = proof.result.unwrap()["proof"]["proof_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let registered = backend.execute(&request(
        "reg",
        "register_verified",
        json!({"domain":c.domain,"proof_id":id,"config_digest":"a".repeat(64),"config_rev":1}),
        Some(backend.global_context()),
    ));
    assert_eq!(registered.phase, Phase::Completed);
    backend.registry().config(&c.domain).unwrap()
}
pub fn connected(backend: &NativeBackend) -> ContextV3 {
    let c = registered(backend);
    let context = backend
        .registry()
        .bind(&c.domain, &yang_worker::new_id().unwrap())
        .unwrap();
    assert_eq!(
        backend
            .execute(&request(
                "connect",
                "connect",
                json!({}),
                Some(context.clone())
            ))
            .phase,
        Phase::Completed
    );
    context
}
#[derive(Clone, Default)]
pub struct Output(pub Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Output {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub fn frames(requests: &[RequestV3]) -> std::io::Cursor<Vec<u8>> {
    let mut bytes = Vec::new();
    for r in requests {
        bytes.extend(serde_json::to_vec(r).unwrap());
        bytes.push(b'\n');
    }
    std::io::Cursor::new(bytes)
}
pub fn replies(output: &Output) -> Vec<Value> {
    output
        .0
        .lock()
        .unwrap()
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect()
}
