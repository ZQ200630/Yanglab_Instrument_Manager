pub mod codec;
pub mod interlock;
mod monitor;
mod pid;
use crate::{
    clock::Clock,
    lifecycle::{CleanupReport, CleanupStep, DriverLifecycle, DriverState, ProbeReport},
    transport::{
        serial::{canonical_com, enumerate_with},
        serial_abi::{NativeSerial, SerialBackend},
        ByteTransport, Deadline, ResourceBook, SerialConfig, SerialSession,
    },
    DriverError, DriverResult,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc, Condvar, Mutex, OnceLock,
    },
    thread::JoinHandle,
    time::Duration,
};
pub const DEFAULT_USB_SERIAL: &str = "E42E432326A3ED118B3D99412981D5C7";
#[derive(Clone)]
pub struct GainConfig {
    pub port: Option<String>,
    pub usb_serial: Option<String>,
    pub io_timeout: Duration,
    pub poll_interval: Duration,
    pub start_watchdog: bool,
    pub close_timeout: Duration,
}
impl Default for GainConfig {
    fn default() -> Self {
        Self {
            port: None,
            usb_serial: Some(DEFAULT_USB_SERIAL.into()),
            io_timeout: Duration::from_secs(1),
            poll_interval: Duration::from_secs(1),
            start_watchdog: true,
            close_timeout: Duration::from_secs(3),
        }
    }
}
impl GainConfig {
    fn validate(&self) -> DriverResult<()> {
        if let Some(port) = &self.port {
            canonical_com(port)?;
        }
        if self.poll_interval != Duration::from_secs(1)
            || [self.io_timeout, self.close_timeout]
                .iter()
                .any(|v| v.is_zero() || *v > Duration::from_secs(180))
            || self.usb_serial.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 256
                    || !s.is_ascii()
                    || s.bytes().any(|b| b.is_ascii_control())
            })
        {
            return Err(DriverError::Invalid(
                "Gain timing/serial configuration invalid; monitor interval is exactly one second"
                    .into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GainStatus {
    pub temperature_c: f64,
    pub target_c: f64,
    pub tec_enabled: bool,
    pub current_ma: f64,
    pub current_enabled: bool,
    pub received_at: Duration,
}
struct State {
    state: DriverState,
    status: Option<GainStatus>,
    thermal: interlock::ThermalInterlock,
    fault_error: Option<DriverError>,
    cleanup_error: Option<DriverError>,
    has_io: bool,
    quiescent: bool,
    readonly: bool,
    receipt: Option<(u64, CleanupReport)>,
    current_off: bool,
    tec_off: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            state: DriverState::Disconnected,
            status: None,
            thermal: Default::default(),
            fault_error: None,
            cleanup_error: None,
            has_io: false,
            quiescent: true,
            readonly: false,
            receipt: None,
            current_off: false,
            tec_off: false,
        }
    }
}
struct Shared {
    config: GainConfig,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    changed: Condvar,
    stop: AtomicBool,
    generation: AtomicU64,
    connection: AtomicU64,
    close_request: AtomicU64,
}
#[derive(Clone)]
pub struct StopHandle {
    shared: Arc<Shared>,
    connection: u64,
}
impl StopHandle {
    pub fn request_stop(&self) {
        if self.shared.connection.load(Ordering::Acquire) == self.connection {
            self.shared.generation.fetch_add(1, Ordering::AcqRel);
            self.shared.stop.store(true, Ordering::Release);
            self.shared.changed.notify_all();
        }
    }
}
#[derive(Clone, Copy)]
enum Op {
    Snapshot,
    Read(u8),
    Set(u8, f64),
    Flag(u8, bool),
    EnableCurrent,
    ReadPid,
    SetPid([f64; 3]),
    ResetPid,
    ClearIntegral,
}
enum Reply {
    Number(f64),
    Flag(bool),
    Pid([f64; 3]),
    Unit,
}
struct Job {
    op: Op,
    generation: u64,
    deadline: Deadline,
    reply: SyncSender<DriverResult<Reply>>,
}
pub struct GainDriver {
    config: GainConfig,
    book: ResourceBook,
    backend: Arc<dyn SerialBackend>,
    shared: Arc<Shared>,
    normal: Option<SyncSender<Job>>,
    safety: Option<SyncSender<Job>>,
    reader: Option<JoinHandle<()>>,
    stranded: Arc<Mutex<Option<SerialSession>>>,
    port: Option<String>,
    close_attempt: Option<u64>,
    last_cleanup: Option<CleanupReport>,
    retain_on_drop: bool,
}
fn retained() -> &'static Mutex<Vec<GainDriver>> {
    static OWNERS: OnceLock<Mutex<Vec<GainDriver>>> = OnceLock::new();
    OWNERS.get_or_init(|| Mutex::new(vec![]))
}
fn timeout(action: &str) -> DriverError {
    DriverError::Timeout {
        operation: action.into(),
        transferred: 0,
    }
}
fn blocked(action: &str) -> DriverError {
    DriverError::Responsibility(action.into())
}
fn step(action: &str, error: Option<DriverError>) -> CleanupStep {
    CleanupStep {
        role: "gain".into(),
        action: action.into(),
        error: error.map(|e| e.to_string()),
    }
}
fn report(steps: Vec<CleanupStep>, unreleased: Vec<String>) -> CleanupReport {
    static ATTEMPT: AtomicU64 = AtomicU64::new(1);
    CleanupReport::new(
        format!("{:032x}", ATTEMPT.fetch_add(1, Ordering::Relaxed)),
        steps,
        None,
        unreleased,
    )
    .expect("bounded Gain cleanup fields")
}
impl GainDriver {
    pub fn new(
        config: GainConfig,
        book: ResourceBook,
        clock: Arc<dyn Clock>,
    ) -> DriverResult<Self> {
        Self::with_backend(config, book, clock, Arc::new(NativeSerial))
    }
    pub fn with_backend(
        config: GainConfig,
        book: ResourceBook,
        clock: Arc<dyn Clock>,
        backend: Arc<dyn SerialBackend>,
    ) -> DriverResult<Self> {
        config.validate()?;
        let shared = Arc::new(Shared {
            config: config.clone(),
            clock,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            connection: AtomicU64::new(0),
            close_request: AtomicU64::new(0),
        });
        Ok(Self {
            config,
            book,
            backend,
            shared,
            normal: None,
            safety: None,
            reader: None,
            stranded: Arc::new(Mutex::new(None)),
            port: None,
            close_attempt: None,
            last_cleanup: None,
            retain_on_drop: true,
        })
    }
    pub fn config(&self) -> &GainConfig {
        &self.config
    }
    pub fn port(&self) -> Option<&str> {
        self.port.as_deref()
    }
    pub fn state(&self) -> DriverState {
        self.shared.state.lock().unwrap().state
    }
    pub fn status(&self) -> Option<GainStatus> {
        self.shared.state.lock().unwrap().status.clone()
    }
    pub fn fault_error(&self) -> Option<DriverError> {
        self.shared.state.lock().unwrap().fault_error.clone()
    }
    pub fn cleanup_error(&self) -> Option<DriverError> {
        self.shared.state.lock().unwrap().cleanup_error.clone()
    }
    pub fn last_cleanup(&self) -> Option<&CleanupReport> {
        self.last_cleanup.as_ref()
    }
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle {
            shared: self.shared.clone(),
            connection: self.shared.connection.load(Ordering::Acquire),
        }
    }
    pub fn has_resource_responsibility(&self) -> bool {
        let s = self.shared.state.lock().unwrap();
        s.has_io || !s.quiescent
    }
    pub fn resources_released(&self) -> bool {
        !self.has_resource_responsibility()
    }
    fn resolve(&self) -> DriverResult<String> {
        if let Some(port) = &self.config.port {
            return Ok(canonical_com(port)?
                .as_str()
                .strip_prefix("serial://")
                .unwrap()
                .into());
        }
        let devices = enumerate_with(self.backend.as_ref())?
            .into_iter()
            .filter(|d| d.vid == Some(0x10C4) && d.pid == Some(0xEA60))
            .collect::<Vec<_>>();
        if let Some(serial) = &self.config.usb_serial {
            let selected = devices
                .iter()
                .filter(|d| &d.serial == serial)
                .collect::<Vec<_>>();
            if selected.len() == 1 {
                return Ok(selected[0].resource.clone());
            }
            if selected.len() > 1 {
                return Err(blocked("duplicate preferred Gain USB serial"));
            }
        }
        if devices.len() == 1 {
            Ok(devices[0].resource.clone())
        } else {
            Err(blocked("Gain CP210x selection is absent or ambiguous"))
        }
    }
    fn spawn_reader(&mut self) -> DriverResult<()> {
        let (tx, normal) = mpsc::sync_channel(8);
        let (stx, safety) = mpsc::sync_channel(1);
        let shared = self.shared.clone();
        let stranded = self.stranded.clone();
        let reader = std::thread::Builder::new()
            .name("gain-io".into())
            .spawn(move || {
                let mut io = stranded
                    .lock()
                    .unwrap()
                    .take()
                    .expect("Gain actor native owner");
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    actor(&shared, &mut io, normal, safety)
                }));
                if result.is_err() {
                    *stranded.lock().unwrap_or_else(|e| e.into_inner()) = Some(io);
                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    s.state = DriverState::Fault;
                    s.fault_error = Some(blocked("Gain actor panicked; native owner retained"));
                    s.quiescent = true;
                    shared.changed.notify_all();
                }
            })
            .map_err(|e| blocked(&format!("Gain reader spawn failed: {e}")))?;
        self.normal = Some(tx);
        self.safety = Some(stx);
        self.reader = Some(reader);
        Ok(())
    }
    fn start(&mut self, readonly: bool) -> DriverResult<()> {
        if self.state() != DriverState::Disconnected || self.has_resource_responsibility() {
            return Err(blocked(
                "Gain must be explicitly closed before new connection",
            ));
        }
        if let Some(reader) = self.reader.take() {
            if !reader.is_finished() {
                self.reader = Some(reader);
                return Err(blocked("Gain old reader retained"));
            }
            let _ = reader.join();
        }
        let port = self.resolve()?;
        self.port = Some(port.clone());
        self.close_attempt = None;
        self.last_cleanup = None;
        self.shared.stop.store(false, Ordering::Release);
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.connection.fetch_add(1, Ordering::AcqRel);
        self.shared.close_request.store(0, Ordering::Release);
        *self.shared.state.lock().unwrap() = State {
            state: DriverState::Connecting,
            readonly,
            ..Default::default()
        };
        let opened = SerialSession::from_backend_owned(
            SerialConfig::instrument(port),
            &self.book,
            self.backend.clone(),
        );
        let (session, configuration_error) = match opened {
            Ok(s) => (s, None),
            Err(f) => match f.session {
                Some(s) => (s, Some(f.error)),
                None => {
                    self.shared.state.lock().unwrap().state = DriverState::Disconnected;
                    return Err(f.error);
                }
            },
        };
        *self.stranded.lock().unwrap() = Some(session);
        {
            let mut s = self.shared.state.lock().unwrap();
            s.has_io = true;
            s.quiescent = false;
            if let Some(error) = &configuration_error {
                s.state = DriverState::Fault;
                s.fault_error = Some(error.clone());
                s.readonly = true;
            }
        }
        if let Err(error) = self.spawn_reader() {
            let mut s = self.shared.state.lock().unwrap();
            s.state = DriverState::Fault;
            s.fault_error = Some(error.clone());
            s.quiescent = true;
            return Err(error);
        }
        if let Some(error) = configuration_error {
            return Err(error);
        }
        Ok(())
    }
    fn call(&self, op: Op, safety: bool) -> DriverResult<Reply> {
        let state = self.state();
        if !matches!(state, DriverState::Ready | DriverState::Active)
            && !(state == DriverState::Connecting && matches!(op, Op::Snapshot))
        {
            return Err(blocked("Gain operation is not ready"));
        }
        if safety {
            self.shared.generation.fetch_add(1, Ordering::AcqRel);
        }
        if self.shared.stop.load(Ordering::Acquire) {
            return Err(blocked("Gain stop pending"));
        }
        let generation = self.shared.generation.load(Ordering::Acquire);
        let budget = self.config.io_timeout * 12 + Duration::from_millis(50);
        let deadline = Deadline::after(budget);
        let (tx, rx) = mpsc::sync_channel(1);
        let queue = if safety { &self.safety } else { &self.normal };
        queue
            .as_ref()
            .ok_or(DriverError::Closed)?
            .try_send(Job {
                op,
                generation,
                deadline,
                reply: tx,
            })
            .map_err(|_| DriverError::Busy("Gain bounded queue".into()))?;
        match rx.recv_timeout(budget) {
            Ok(result) => result,
            Err(_) => {
                self.shared.generation.fetch_add(1, Ordering::AcqRel);
                self.shared.stop.store(true, Ordering::Release);
                self.shared.changed.notify_all();
                Err(timeout(
                    "Gain pending native operation retained; never replayed",
                ))
            }
        }
    }
    pub fn connect(&mut self) -> DriverResult<ProbeReport> {
        if matches!(self.state(), DriverState::Ready | DriverState::Active) {
            return Ok(self.probe_report(false));
        }
        self.start(false)?;
        if let Err(error) = self.call(Op::Snapshot, false) {
            let _ = self.close();
            return Err(error);
        }
        Ok(self.probe_report(false))
    }
    fn probe_report(&self, released: bool) -> ProbeReport {
        ProbeReport::new(
            json!({"model":"Gain Chip Driver","resource":self.port,"identity_quality":"weak_protocol","protocol":"READY fields"}),
            json!({"status":self.status(),"measurement_consistency":"unproven"}),
            released,
        )
    }
    pub fn probe_identity(&mut self) -> DriverResult<ProbeReport> {
        self.start(true)?;
        let observed = self.call(Op::Snapshot, false);
        let evidence = self.probe_report(false);
        let cleanup = self.close()?;
        observed?;
        if !cleanup.resources_released() {
            return Err(blocked("Gain read-only probe release unconfirmed"));
        }
        Ok(ProbeReport::new(
            evidence.identity().clone(),
            evidence.observations().clone(),
            true,
        ))
    }
    pub fn read_status(&self) -> DriverResult<GainStatus> {
        if !matches!(self.state(), DriverState::Ready | DriverState::Active)
            || self.shared.stop.load(Ordering::Acquire)
        {
            return Err(blocked("Gain cached status is not operational"));
        }
        let status = self.status().ok_or(DriverError::Closed)?;
        if self.shared.clock.now() < status.received_at
            || self.shared.clock.now() - status.received_at > Duration::from_millis(1500)
        {
            return Err(blocked("Gain cached temperature status is stale"));
        }
        Ok(status)
    }
    fn read_number(&self, field: u8) -> DriverResult<f64> {
        match self.call(Op::Read(field), false)? {
            Reply::Number(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    fn read_flag(&self, field: u8) -> DriverResult<bool> {
        match self.call(Op::Read(field), false)? {
            Reply::Flag(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn read_temperature(&self) -> DriverResult<f64> {
        self.read_number(b'T')
    }
    pub fn read_target(&self) -> DriverResult<f64> {
        self.read_number(b'E')
    }
    pub fn read_current(&self) -> DriverResult<f64> {
        self.read_number(b'C')
    }
    pub fn read_tec_enabled(&self) -> DriverResult<bool> {
        self.read_flag(b'R')
    }
    pub fn read_current_enabled(&self) -> DriverResult<bool> {
        self.read_flag(b'Q')
    }
    fn set_number(&self, field: u8, value: f64, min: f64, max: f64) -> DriverResult<f64> {
        codec::format_fixed(value, min, max)?;
        match self.call(Op::Set(field, value), false)? {
            Reply::Number(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn set_temperature(&mut self, value: f64) -> DriverResult<f64> {
        self.set_number(b'E', value, 15., 40.)
    }
    pub fn set_current(&mut self, value: f64) -> DriverResult<f64> {
        self.set_number(b'C', value, 0., 200.)
    }
    fn set_flag(&self, field: u8, value: bool, safety: bool) -> DriverResult<bool> {
        match self.call(Op::Flag(field, value), safety)? {
            Reply::Flag(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn enable_tec(&mut self) -> DriverResult<bool> {
        self.set_flag(b'D', true, false)
    }
    pub fn disable_tec(&mut self) -> DriverResult<bool> {
        self.set_flag(b'D', false, true)
    }
    pub fn disable_current(&mut self) -> DriverResult<bool> {
        self.set_flag(b'Q', false, true)
    }
    pub fn wait_stable(&self, deadline: Deadline) -> DriverResult<()> {
        let mut s = self.shared.state.lock().unwrap();
        loop {
            if !matches!(s.state, DriverState::Ready | DriverState::Active)
                || self.shared.stop.load(Ordering::Acquire)
            {
                return Err(blocked("Gain stability wait canceled/faulted"));
            }
            if s.thermal.ready(self.shared.clock.now()) {
                return Ok(());
            }
            let remaining = deadline
                .remaining_millis()
                .map_err(|_| timeout("Gain did not stabilize"))?;
            s = self
                .shared
                .changed
                .wait_timeout(s, Duration::from_millis(remaining.min(50) as u64))
                .unwrap()
                .0;
        }
    }
    pub fn enable_current(&mut self) -> DriverResult<bool> {
        match self.call(Op::EnableCurrent, false)? {
            Reply::Flag(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn ramp_current(
        &mut self,
        target: f64,
        step: f64,
        interval: Duration,
    ) -> DriverResult<f64> {
        codec::format_fixed(target, 0., 200.)?;
        if !step.is_finite()
            || step < 0.001
            || step > 1.
            || interval < Duration::from_millis(50)
            || interval > Duration::from_secs(180)
        {
            return Err(DriverError::Invalid(
                "Gain ramp requires 0.001–1 mA steps and 50 ms–180 s intervals".into(),
            ));
        }
        let initial = self.read_status()?;
        if !initial.current_enabled || !initial.tec_enabled || self.state() != DriverState::Active {
            return Err(blocked(
                "Gain ramp requires confirmed enabled current and TEC",
            ));
        }
        let generation = self.shared.generation.load(Ordering::Acquire);
        let start = (initial.current_ma * 1000.).round_ties_even() as i64;
        let end = (target * 1000.).round_ties_even() as i64;
        let step = (step * 1000.).floor() as i64;
        let mut current = start;
        let result = (|| {
            while current != end {
                let mut remaining = interval;
                while !remaining.is_zero() {
                    if self.shared.stop.load(Ordering::Acquire)
                        || self.shared.generation.load(Ordering::Acquire) != generation
                    {
                        return Err(blocked("Gain ramp canceled"));
                    }
                    let quantum = remaining.min(Duration::from_millis(50));
                    self.shared.clock.wait(quantum);
                    remaining -= quantum;
                }
                if self.shared.stop.load(Ordering::Acquire)
                    || self.shared.generation.load(Ordering::Acquire) != generation
                {
                    return Err(blocked("Gain ramp canceled"));
                }
                let status = self.read_status()?;
                if !status.current_enabled
                    || !status.tec_enabled
                    || self.state() != DriverState::Active
                {
                    return Err(blocked("Gain output changed during ramp"));
                }
                current = if current < end {
                    (current + step).min(end)
                } else {
                    (current - step).max(end)
                };
                let requested = current as f64 / 1000.;
                let applied = self.set_current(requested)?;
                if (applied - requested).abs() > 0.0005 {
                    return Err(DriverError::Protocol(
                        "Gain ramp acknowledgement differs from requested step".into(),
                    ));
                }
            }
            let actual = self.read_current()?;
            let enabled = self.read_current_enabled()?;
            let tec = self.read_tec_enabled()?;
            let temperature = self.read_temperature()?;
            let target_c = self.read_target()?;
            if (actual - end as f64 / 1000.).abs() > 0.0005
                || !enabled
                || !tec
                || (temperature - target_c).abs() > 0.2
            {
                return Err(blocked("Gain final ramp readback/safety mismatch"));
            }
            Ok(actual)
        })();
        if result.is_err() {
            self.stop_handle().request_stop();
        }
        result
    }
    pub fn read_pid(&self) -> DriverResult<[f64; 3]> {
        match self.call(Op::ReadPid, false)? {
            Reply::Pid(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn set_pid(&mut self, p: f64, i: f64, d: f64) -> DriverResult<[f64; 3]> {
        pid::validate([p, i, d])?;
        match self.call(Op::SetPid([p, i, d]), false)? {
            Reply::Pid(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn reset_pid(&mut self) -> DriverResult<[f64; 3]> {
        match self.call(Op::ResetPid, false)? {
            Reply::Pid(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn clear_integral(&mut self) -> DriverResult<()> {
        self.call(Op::ClearIntegral, false)?;
        Ok(())
    }
    fn begin_close(&mut self) -> u64 {
        if let Some(attempt) = self.close_attempt {
            return attempt;
        }
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.stop.store(true, Ordering::Release);
        {
            let mut s = self.shared.state.lock().unwrap();
            s.state = DriverState::Closing;
            s.thermal.invalidate();
        }
        let attempt = self.shared.close_request.fetch_add(1, Ordering::AcqRel) + 1;
        self.close_attempt = Some(attempt);
        self.shared.changed.notify_all();
        attempt
    }
    pub fn close(&mut self) -> DriverResult<CleanupReport> {
        if self.resources_released() {
            let actual = self
                .shared
                .state
                .lock()
                .unwrap()
                .receipt
                .as_ref()
                .map(|(_, r)| r.clone());
            let result = actual
                .or_else(|| self.last_cleanup.clone())
                .unwrap_or_else(|| report(vec![step("no_resource", None)], vec![]));
            self.last_cleanup = Some(result.clone());
            return Ok(result);
        }
        if self.reader.as_ref().is_none_or(|r| r.is_finished())
            && self.stranded.lock().unwrap().is_some()
        {
            self.spawn_reader()?;
        }
        let attempt = self.begin_close();
        let deadline = Deadline::after(self.config.close_timeout);
        let mut s = self.shared.state.lock().unwrap();
        loop {
            if let Some((id, result)) = &s.receipt {
                if *id >= attempt {
                    let result = result.clone();
                    self.last_cleanup = Some(result.clone());
                    self.close_attempt = None;
                    return Ok(result);
                }
            }
            let remaining = match deadline.remaining_millis() {
                Ok(v) => v,
                Err(_) => {
                    let result = report(
                        vec![step(
                            "native_cleanup_pending",
                            Some(timeout("Gain actor still owns native work")),
                        )],
                        vec![self.port.clone().unwrap_or_else(|| "gain".into())],
                    );
                    self.last_cleanup = Some(result.clone());
                    return Ok(result);
                }
            };
            s = self
                .shared
                .changed
                .wait_timeout(s, Duration::from_millis(remaining.min(20) as u64))
                .unwrap()
                .0;
        }
    }
}
impl DriverLifecycle for GainDriver {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        GainDriver::close(self)
    }
    fn has_responsibility(&self) -> bool {
        self.has_resource_responsibility()
    }
}
impl Drop for GainDriver {
    fn drop(&mut self) {
        if self.retain_on_drop && self.has_resource_responsibility() {
            self.begin_close();
            retained()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Self {
                    config: self.config.clone(),
                    book: self.book.clone(),
                    backend: self.backend.clone(),
                    shared: self.shared.clone(),
                    normal: self.normal.take(),
                    safety: self.safety.take(),
                    reader: self.reader.take(),
                    stranded: self.stranded.clone(),
                    port: self.port.take(),
                    close_attempt: self.close_attempt.take(),
                    last_cleanup: self.last_cleanup.take(),
                    retain_on_drop: false,
                });
        }
    }
}
pub fn retry_retained() -> usize {
    let owners = std::mem::take(&mut *retained().lock().unwrap_or_else(|e| e.into_inner()));
    for mut owner in owners {
        if !owner.close().is_ok_and(|r| r.resources_released())
            || owner.has_resource_responsibility()
        {
            retained()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(owner);
        }
    }
    retained().lock().unwrap_or_else(|e| e.into_inner()).len()
}
fn guard(shared: &Shared, generation: u64) -> DriverResult<()> {
    if shared.stop.load(Ordering::Acquire)
        || shared.generation.load(Ordering::Acquire) != generation
        || matches!(
            shared.state.lock().unwrap().state,
            DriverState::Closing | DriverState::Disconnected | DriverState::Fault
        )
    {
        return Err(blocked("Gain operation fenced before command"));
    }
    Ok(())
}
fn request(
    shared: &Shared,
    io: &mut SerialSession,
    command: &[u8],
    field: Option<u8>,
    generation: Option<u64>,
    deadline: Deadline,
) -> DriverResult<String> {
    let deadline = deadline.earlier(Deadline::after(shared.config.io_timeout));
    if let Some(g) = generation {
        guard(shared, g)?;
    }
    deadline.remaining_millis()?;
    io.write_all(command, deadline)?;
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n") {
        if let Err(error) = deadline.remaining_millis() {
            let _ = io.fence_protocol();
            return Err(error);
        }
        if bytes.len() >= 512 {
            let _ = io.fence_protocol();
            return Err(DriverError::Protocol(
                "Gain response exceeds 512 bytes".into(),
            ));
        }
        let chunk = match io.read_bounded(512 - bytes.len(), deadline) {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = io.fence_protocol();
                return Err(error);
            }
        };
        if chunk.is_empty() {
            let _ = io.fence_protocol();
            return Err(timeout("Gain empty response"));
        }
        bytes.extend(chunk);
    }
    if let Some(g) = generation {
        guard(shared, g)?;
    }
    Ok(match field {
        Some(f) => codec::parse_reply(&bytes, f)?,
        None => codec::parse_ack(&bytes)?,
    }
    .into())
}
fn command(field: u8) -> DriverResult<&'static str> {
    Ok(match field {
        b'T' => "RDTA",
        b'E' => "RDEA",
        b'R' => "RDRA",
        b'C' => "RDCA",
        b'Q' => "RDQA",
        b'P' => "RDPA",
        b'I' => "RDIA",
        b'D' => "RDDA",
        _ => return Err(DriverError::Invalid("Gain unknown typed field".into())),
    })
}
fn read_number(
    shared: &Shared,
    io: &mut SerialSession,
    field: u8,
    g: u64,
    end: Deadline,
) -> DriverResult<f64> {
    let value = request(
        shared,
        io,
        &codec::build_command(command(field)?, None, 0., 0.)?,
        Some(field),
        Some(g),
        end,
    )?;
    codec::number(&value, field)
}
fn read_flag(
    shared: &Shared,
    io: &mut SerialSession,
    field: u8,
    g: u64,
    end: Deadline,
) -> DriverResult<bool> {
    let value = request(
        shared,
        io,
        &codec::build_command(command(field)?, None, 0., 0.)?,
        Some(field),
        Some(g),
        end,
    )?;
    codec::flag(&value)
}
fn set_flag(
    shared: &Shared,
    io: &mut SerialSession,
    field: u8,
    value: bool,
    g: Option<u64>,
    end: Deadline,
) -> DriverResult<bool> {
    // A new attempt cannot inherit an older shutdown acknowledgement.
    {
        let mut s = shared.state.lock().unwrap();
        if field == b'D' {
            s.tec_off = false;
        } else {
            s.current_off = false;
        }
    }
    let cmd = if field == b'D' { "STRA" } else { "STQA" };
    let applied = codec::flag(&request(
        shared,
        io,
        &codec::build_bool(cmd, value)?,
        Some(field),
        g,
        end,
    )?)?;
    if applied != value {
        return Err(DriverError::Protocol(
            "Gain boolean acknowledgement did not confirm requested state".into(),
        ));
    }
    let mut s = shared.state.lock().unwrap();
    if field == b'D' {
        s.tec_off = !applied;
        let changed = s.status.as_ref().is_some_and(|v| v.tec_enabled != applied);
        if changed {
            s.thermal.invalidate();
        }
        if let Some(status) = &mut s.status {
            status.tec_enabled = applied;
        }
    } else {
        s.current_off = !applied;
        if let Some(status) = &mut s.status {
            status.current_enabled = applied;
        }
        if matches!(s.state, DriverState::Ready | DriverState::Active) {
            s.state = if applied {
                DriverState::Active
            } else {
                DriverState::Ready
            };
        }
    }
    shared.changed.notify_all();
    Ok(applied)
}
fn snapshot(
    shared: &Shared,
    io: &mut SerialSession,
    g: u64,
    end: Deadline,
) -> DriverResult<GainStatus> {
    let temperature_c = read_number(shared, io, b'T', g, end)?;
    let received_at = shared.clock.now();
    Ok(GainStatus {
        temperature_c,
        target_c: read_number(shared, io, b'E', g, end)?,
        tec_enabled: read_flag(shared, io, b'R', g, end)?,
        current_ma: read_number(shared, io, b'C', g, end)?,
        current_enabled: read_flag(shared, io, b'Q', g, end)?,
        received_at,
    })
}
fn publish(shared: &Shared, status: GainStatus, sample: bool) -> DriverResult<()> {
    let unsafe_output = status.current_enabled && !status.tec_enabled;
    let mut s = shared.state.lock().unwrap();
    if s.status
        .as_ref()
        .is_some_and(|v| v.target_c != status.target_c || v.tec_enabled != status.tec_enabled)
    {
        s.thermal.invalidate();
    }
    if sample {
        s.thermal.observe(
            status.temperature_c,
            status.target_c,
            status.tec_enabled,
            status.received_at,
        );
    }
    if !s.readonly && !matches!(s.state, DriverState::Closing | DriverState::Fault) {
        s.state = if status.current_enabled {
            DriverState::Active
        } else {
            DriverState::Ready
        };
    }
    s.status = Some(status);
    shared.changed.notify_all();
    if unsafe_output && !s.readonly {
        return Err(blocked("Gain current is on while TEC is off"));
    }
    Ok(())
}
fn shutdown_outputs(shared: &Shared, io: &mut SerialSession) -> Vec<CleanupStep> {
    let mut steps = vec![];
    for (field, action) in [(b'Q', "current_off"), (b'D', "tec_off")] {
        let result = set_flag(
            shared,
            io,
            field,
            false,
            None,
            Deadline::after(shared.config.io_timeout),
        );
        steps.push(step(action, result.err()));
    }
    steps
}
fn trip(shared: &Shared, io: &mut SerialSession, cause: DriverError) {
    shared.generation.fetch_add(1, Ordering::AcqRel);
    shared.stop.store(true, Ordering::Release);
    let readonly = {
        let mut s = shared.state.lock().unwrap();
        s.state = DriverState::Fault;
        s.thermal.invalidate();
        s.fault_error.get_or_insert(cause);
        s.readonly
    };
    shared.changed.notify_all();
    if !readonly {
        let steps = shutdown_outputs(shared, io);
        let failure = steps.iter().find_map(|s| s.error.as_ref());
        if let Some(error) = failure {
            shared.state.lock().unwrap().cleanup_error = Some(blocked(error));
        }
    }
}
fn execute(shared: &Shared, io: &mut SerialSession, job: &Job) -> DriverResult<Reply> {
    guard(shared, job.generation)?;
    let g = job.generation;
    let end = job.deadline;
    match job.op {
        Op::Snapshot => {
            let status = snapshot(shared, io, g, end)?;
            publish(shared, status.clone(), false)?;
            Ok(Reply::Unit)
        }
        Op::Read(field) => {
            let mut status = shared
                .state
                .lock()
                .unwrap()
                .status
                .clone()
                .ok_or(DriverError::Closed)?;
            if matches!(field, b'R' | b'Q') {
                let value = read_flag(shared, io, field, g, end)?;
                if field == b'R' {
                    status.tec_enabled = value;
                } else {
                    status.current_enabled = value;
                }
                publish(shared, status, false)?;
                Ok(Reply::Flag(value))
            } else {
                let value = read_number(shared, io, field, g, end)?;
                match field {
                    b'T' => {
                        status.temperature_c = value;
                        status.received_at = shared.clock.now();
                        if (value - status.target_c).abs() > 0.2 {
                            shared.state.lock().unwrap().thermal.invalidate();
                        }
                    }
                    b'E' => status.target_c = value,
                    b'C' => status.current_ma = value,
                    _ => {}
                }
                publish(shared, status, false)?;
                Ok(Reply::Number(value))
            }
        }
        Op::Set(field, value) => {
            let (name, min, max) = if field == b'E' {
                ("STEA", 15., 40.)
            } else {
                ("STCA", 0., 200.)
            };
            let applied = codec::number(
                &request(
                    shared,
                    io,
                    &codec::build_command(name, Some(value), min, max)?,
                    Some(field),
                    Some(g),
                    end,
                )?,
                field,
            )?;
            let mut s = shared.state.lock().unwrap();
            if field == b'E' {
                s.thermal.invalidate();
            }
            if let Some(status) = &mut s.status {
                if field == b'E' {
                    status.target_c = applied;
                } else {
                    status.current_ma = applied;
                }
            }
            shared.changed.notify_all();
            Ok(Reply::Number(applied))
        }
        Op::Flag(field, value) => {
            if field == b'D' && !value {
                set_flag(shared, io, b'Q', false, Some(g), end)?;
            }
            Ok(Reply::Flag(set_flag(
                shared,
                io,
                field,
                value,
                Some(g),
                end,
            )?))
        }
        Op::EnableCurrent => {
            let already_enabled = shared
                .state
                .lock()
                .unwrap()
                .status
                .as_ref()
                .is_some_and(|s| s.current_enabled);
            if already_enabled {
                let current = snapshot(shared, io, g, end)?;
                publish(shared, current.clone(), false)?;
                if current.current_enabled {
                    // Reissuing Q=1 resets this controller's setpoint to 3 mA.
                    // Observing a running output confers no new enable authority.
                    return Ok(Reply::Flag(true));
                }
            }
            let epoch = {
                let s = shared.state.lock().unwrap();
                if !s.thermal.ready(shared.clock.now()) {
                    return Err(DriverError::Invalid(
                        "Gain temperature stability is incomplete/stale".into(),
                    ));
                }
                s.thermal.epoch()
            };
            let status = snapshot(shared, io, g, end)?;
            publish(shared, status.clone(), false)?;
            {
                let s = shared.state.lock().unwrap();
                if epoch != s.thermal.epoch()
                    || !s.thermal.ready(shared.clock.now())
                    || !status.tec_enabled
                    || (status.temperature_c - status.target_c).abs() > 0.2
                {
                    return Err(DriverError::Invalid(
                        "Gain fresh TEC/target/temperature preflight failed".into(),
                    ));
                }
            }
            guard(shared, g)?;
            let enabled = set_flag(shared, io, b'Q', true, Some(g), end)?;
            let current = read_number(shared, io, b'C', g, end)?;
            shared
                .state
                .lock()
                .unwrap()
                .status
                .as_mut()
                .unwrap()
                .current_ma = current;
            Ok(Reply::Flag(enabled))
        }
        Op::ReadPid | Op::ResetPid => {
            if matches!(job.op, Op::ResetPid) {
                request(shared, io, b"RST\r\n", None, Some(g), end)?;
            }
            Ok(Reply::Pid([
                read_number(shared, io, b'P', g, end)?,
                read_number(shared, io, b'I', g, end)?,
                read_number(shared, io, b'D', g, end)?,
            ]))
        }
        Op::SetPid(values) => {
            let mut applied = [0.; 3];
            for (i, (name, field)) in [("STPA", b'P'), ("STIA", b'I'), ("STDA", b'D')]
                .into_iter()
                .enumerate()
            {
                applied[i] = codec::number(
                    &request(
                        shared,
                        io,
                        &codec::build_command(name, Some(values[i]), 0., 999.999)?,
                        Some(field),
                        Some(g),
                        end,
                    )?,
                    field,
                )?;
            }
            Ok(Reply::Pid(applied))
        }
        Op::ClearIntegral => {
            request(shared, io, b"CLR\r\n", None, Some(g), end)?;
            Ok(Reply::Unit)
        }
    }
}
fn cleanup(shared: &Shared, io: &mut SerialSession, port: &str, retry: bool) -> CleanupReport {
    let readonly = shared.state.lock().unwrap().readonly;
    let mut steps = if readonly {
        vec![step("read_only_close", None)]
    } else if retry {
        vec![step("retry_release_only", None)]
    } else {
        shutdown_outputs(shared, io)
    };
    if !readonly {
        let s = shared.state.lock().unwrap();
        if !s.current_off {
            steps.push(step(
                "current_off_unconfirmed",
                Some(blocked("current-off acknowledgement missing")),
            ));
        }
        if !s.tec_off {
            steps.push(step(
                "tec_off_unconfirmed",
                Some(blocked("TEC-off acknowledgement missing")),
            ));
        }
    }
    let close = io.close();
    let released = close.as_ref().is_ok_and(|r| r.released) && !io.has_responsibility();
    steps.push(step(
        "transport_close",
        close
            .err()
            .or_else(|| (!released).then(|| blocked("Gain native release unconfirmed"))),
    ));
    let failure = steps
        .iter()
        .find_map(|s| s.error.as_ref())
        .map(|e| blocked(e));
    {
        let mut s = shared.state.lock().unwrap();
        s.has_io = !released;
        s.state = if released {
            DriverState::Disconnected
        } else {
            DriverState::Fault
        };
        s.cleanup_error = failure;
    }
    report(steps, if released { vec![] } else { vec![port.into()] })
}
fn actor(shared: &Shared, io: &mut SerialSession, normal: Receiver<Job>, safety: Receiver<Job>) {
    let port = shared.config.port.clone().unwrap_or_else(|| "gain".into());
    let mut close_attempt = 0;
    let mut closing = false;
    let mut next = shared.clock.now() + Duration::from_secs(1);
    let mut deviation = monitor::Deviation::default();
    loop {
        let requested = shared.close_request.load(Ordering::Acquire);
        if requested > close_attempt {
            let result = cleanup(shared, io, &port, closing);
            closing = true;
            close_attempt = requested;
            let released = result.resources_released();
            {
                let mut s = shared.state.lock().unwrap();
                s.quiescent = released;
                s.receipt = Some((requested, result));
                shared.changed.notify_all();
            }
            if released {
                return;
            }
            continue;
        }
        let state = shared.state.lock().unwrap().state;
        if shared.stop.load(Ordering::Acquire)
            && matches!(
                state,
                DriverState::Connecting | DriverState::Ready | DriverState::Active
            )
        {
            trip(shared, io, blocked("Gain stop requested"));
            continue;
        }
        if shared.config.start_watchdog
            && matches!(state, DriverState::Ready | DriverState::Active)
            && shared.clock.now() >= next
        {
            let g = shared.generation.load(Ordering::Acquire);
            let snapshot = snapshot(shared, io, g, Deadline::after(shared.config.io_timeout * 5));
            let result = snapshot.and_then(|status| {
                let diff = (status.temperature_c - status.target_c).abs();
                let at = status.received_at;
                publish(shared, status, true)?;
                if diff > 3. {
                    return Err(blocked("Gain temperature deviation exceeds 3 degC"));
                }
                if deviation.observe(diff, at) {
                    set_flag(
                        shared,
                        io,
                        b'Q',
                        false,
                        Some(g),
                        Deadline::after(shared.config.io_timeout),
                    )?;
                }
                Ok(())
            });
            if let Err(error) = result {
                trip(shared, io, error);
            }
            next += Duration::from_secs(1);
            if next <= shared.clock.now() {
                next = shared.clock.now() + Duration::from_secs(1);
            }
            continue;
        }
        let job = match safety.try_recv() {
            Ok(job) => Some(job),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => normal.try_recv().ok(),
        };
        if let Some(job) = job {
            let result = execute(shared, io, &job);
            // Refused preconditions have sent no output command. Protocol/transport
            // failures and interrupted compound operations retain fault liability.
            if let Err(error) = &result {
                if !matches!(error, DriverError::Invalid(_)) {
                    trip(shared, io, error.clone());
                }
            }
            let _ = job.reply.send(result);
            continue;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
