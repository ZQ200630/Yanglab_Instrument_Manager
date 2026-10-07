mod codec;
mod monitor;
mod ramp;
use crate::{
    clock::Clock,
    lifecycle::{CleanupReport, DriverState, ProbeReport},
    transport::{serial_abi::SerialBackend, Deadline, ResourceBook},
    DriverError, DriverResult,
};
pub use codec::{decode_telemetry, encode_voltages, BOARD_SCALE_V};
pub use monitor::TelemetryDecoder;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
#[derive(Clone)]
pub struct VoltageConfig {
    pub port: String,
    pub limit: f64,
    pub ramp_step: f64,
    pub ramp_interval: Duration,
    pub io_timeout: Duration,
    pub startup_timeout: Duration,
    pub telemetry_timeout: Duration,
    pub communication_recovery_timeout: Duration,
    pub communication_retry_interval: Duration,
    pub close_timeout: Duration,
}
impl Default for VoltageConfig {
    fn default() -> Self {
        Self {
            port: "COM1".into(),
            limit: 14.,
            ramp_step: 0.1,
            ramp_interval: Duration::from_millis(50),
            io_timeout: Duration::from_millis(500),
            startup_timeout: Duration::from_secs(3),
            telemetry_timeout: Duration::from_secs(1),
            communication_recovery_timeout: Duration::from_secs(3),
            communication_retry_interval: Duration::from_millis(100),
            close_timeout: Duration::from_secs(2),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoltageStatus {
    pub voltage_v: [f64; 8],
    pub current_ma: [f64; 8],
    pub received_at: Duration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroState {
    Unknown,
    CommandSent,
    MeasuredZero,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZeroEvidence {
    state: ZeroState,
    sent_at: Option<Duration>,
    observed_at: Option<Duration>,
    voltage_v: Option<[f64; 8]>,
}
impl ZeroEvidence {
    pub fn state(&self) -> ZeroState {
        self.state
    }
    pub fn sent_at(&self) -> Option<Duration> {
        self.sent_at
    }
    pub fn observed_at(&self) -> Option<Duration> {
        self.observed_at
    }
    pub fn voltage_v(&self) -> Option<&[f64; 8]> {
        self.voltage_v.as_ref()
    }
}
use crate::{
    lifecycle::{CleanupStep, DriverLifecycle},
    transport::{
        serial::canonical_com, serial_abi::NativeSerial, ByteTransport, SerialConfig, SerialSession,
    },
};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Condvar, Mutex, OnceLock,
    },
    thread::JoinHandle,
    time::SystemTime,
};
impl ZeroEvidence {
    fn unknown() -> Self {
        Self {
            state: ZeroState::Unknown,
            sent_at: None,
            observed_at: None,
            voltage_v: None,
        }
    }
}
impl VoltageConfig {
    fn validate(&self) -> DriverResult<()> {
        canonical_com(&self.port)?;
        codec::codes([0.; 8], self.limit)?;
        ramp::plan([0; 8], [0; 8], self.ramp_step)?;
        if self.ramp_interval < Duration::from_millis(50)
            || [
                self.ramp_interval,
                self.io_timeout,
                self.startup_timeout,
                self.telemetry_timeout,
                self.communication_recovery_timeout,
                self.communication_retry_interval,
                self.close_timeout,
            ]
            .iter()
            .any(|d| d.is_zero() || *d > Duration::from_secs(180))
        {
            return Err(DriverError::Invalid(
                "finite positive voltage timing budgets required; ramps >=50 ms".into(),
            ));
        }
        Ok(())
    }
}
struct MonitorState {
    state: DriverState,
    latest: Option<VoltageStatus>,
    sequence: u64,
    commanded: [u16; 8],
    command_at: Option<Duration>,
    command_sequence: u64,
    zero: ZeroEvidence,
    zero_minimum_sequence: u64,
    command_generation: u64,
    latest_frame_generation: u64,
    failure_started: Option<Duration>,
    cleanup_error: Option<DriverError>,
    has_io: bool,
    reader_quiescent: bool,
    receipt: Option<(u64, CleanupReport)>,
    readonly: bool,
}
impl Default for MonitorState {
    fn default() -> Self {
        Self {
            state: DriverState::Disconnected,
            latest: None,
            sequence: 0,
            commanded: [0; 8],
            command_at: None,
            command_sequence: 0,
            zero: ZeroEvidence::unknown(),
            zero_minimum_sequence: 0,
            command_generation: 0,
            latest_frame_generation: 0,
            failure_started: None,
            cleanup_error: None,
            has_io: false,
            reader_quiescent: true,
            receipt: None,
            readonly: false,
        }
    }
}
struct Shared {
    config: VoltageConfig,
    clock: Arc<dyn Clock>,
    state: Mutex<MonitorState>,
    changed: Condvar,
    generation: AtomicU64,
    connection: AtomicU64,
    stop: AtomicBool,
    close_request: AtomicU64,
}
impl Shared {
    fn notify(&self) {
        self.changed.notify_all();
    }
    fn current_zero(&self) -> ZeroEvidence {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.zero.observed_at.is_some_and(|at| {
            self.clock
                .now()
                .checked_sub(at)
                .is_none_or(|age| age > self.config.telemetry_timeout)
        }) {
            state.zero = ZeroEvidence::unknown();
        }
        state.zero.clone()
    }
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
            self.shared.notify();
        }
    }
}
struct WriteCommand {
    raw: [u16; 8],
    generation: u64,
    startup: bool,
    emergency: bool,
    reply: SyncSender<DriverResult<()>>,
}
pub struct VoltageSource {
    config: VoltageConfig,
    book: ResourceBook,
    clock: Arc<dyn Clock>,
    backend: Arc<dyn SerialBackend>,
    shared: Arc<Shared>,
    normal: Option<SyncSender<WriteCommand>>,
    safety: Option<SyncSender<WriteCommand>>,
    reader: Option<JoinHandle<()>>,
    close_attempt: Option<u64>,
    last_cleanup: Option<CleanupReport>,
    stranded: Arc<Mutex<Option<SerialSession>>>,
    retain_on_drop: bool,
}
fn attempt_id() -> String {
    static N: AtomicU64 = AtomicU64::new(1);
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    format!("{t:016x}{:016x}", N.fetch_add(1, Ordering::Relaxed))
}
fn step(action: &str, error: Option<&DriverError>) -> CleanupStep {
    CleanupStep {
        role: "voltage".into(),
        action: action.into(),
        error: error.map(ToString::to_string),
    }
}
fn timeout(operation: &str) -> DriverError {
    DriverError::Timeout {
        operation: operation.into(),
        transferred: 0,
    }
}
fn report(
    steps: Vec<CleanupStep>,
    zero: Option<ZeroEvidence>,
    unreleased: Vec<String>,
) -> CleanupReport {
    CleanupReport::new(
        attempt_id(),
        steps,
        zero.map(|z| serde_json::to_value(z).expect("finite zero evidence")),
        unreleased,
    )
    .expect("bounded voltage cleanup evidence")
}
fn retained() -> &'static Mutex<Vec<VoltageSource>> {
    static OWNERS: OnceLock<Mutex<Vec<VoltageSource>>> = OnceLock::new();
    OWNERS.get_or_init(|| Mutex::new(vec![]))
}
impl VoltageSource {
    pub fn new(
        config: VoltageConfig,
        book: ResourceBook,
        clock: Arc<dyn Clock>,
    ) -> DriverResult<Self> {
        Self::with_backend(config, book, clock, Arc::new(NativeSerial))
    }
    pub fn with_backend(
        config: VoltageConfig,
        book: ResourceBook,
        clock: Arc<dyn Clock>,
        backend: Arc<dyn SerialBackend>,
    ) -> DriverResult<Self> {
        config.validate()?;
        let shared = Arc::new(Shared {
            config: config.clone(),
            clock: clock.clone(),
            state: Mutex::new(MonitorState::default()),
            changed: Condvar::new(),
            generation: AtomicU64::new(0),
            connection: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            close_request: AtomicU64::new(0),
        });
        Ok(Self {
            config,
            book,
            clock,
            backend,
            shared,
            normal: None,
            safety: None,
            reader: None,
            close_attempt: None,
            last_cleanup: None,
            stranded: Arc::new(Mutex::new(None)),
            retain_on_drop: true,
        })
    }
    pub fn config(&self) -> &VoltageConfig {
        &self.config
    }
    pub fn state(&self) -> DriverState {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .state
    }
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle {
            shared: self.shared.clone(),
            connection: self.shared.connection.load(Ordering::Acquire),
        }
    }
    pub fn cleanup_error(&self) -> Option<DriverError> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cleanup_error
            .clone()
    }
    pub fn last_cleanup(&self) -> Option<&CleanupReport> {
        self.last_cleanup.as_ref()
    }
    pub fn zero_evidence(&self) -> ZeroEvidence {
        self.shared.current_zero()
    }
    pub fn commanded_voltages(&self) -> [f64; 8] {
        codec::volts(
            self.shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .commanded,
        )
    }
    pub fn has_resource_responsibility(&self) -> bool {
        let s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        s.has_io || !s.reader_quiescent
    }
    pub fn resources_released(&self) -> bool {
        !self.has_resource_responsibility()
    }
    fn start(&mut self, readonly: bool) -> DriverResult<()> {
        if self.state() != DriverState::Disconnected || self.has_resource_responsibility() {
            return Err(DriverError::Responsibility(
                "close voltage session before opening".into(),
            ));
        }
        if let Some(reader) = self.reader.take() {
            if reader.is_finished() {
                let _ = reader.join();
            } else {
                self.reader = Some(reader);
                return Err(DriverError::Responsibility(
                    "prior reader has not exited".into(),
                ));
            }
        }
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.connection.fetch_add(1, Ordering::AcqRel);
        self.shared.stop.store(false, Ordering::Release);
        self.shared.close_request.store(0, Ordering::Release);
        {
            let mut s = self.shared.state.lock().unwrap();
            *s = MonitorState {
                state: DriverState::Connecting,
                readonly,
                ..MonitorState::default()
            };
        }
        self.close_attempt = None;
        self.last_cleanup = None;
        let opened = SerialSession::from_backend_owned(
            SerialConfig::instrument(&self.config.port),
            &self.book,
            self.backend.clone(),
        );
        let (session, configuration_error) = match opened {
            Ok(io) => (io, None),
            Err(mut failure) => {
                if let Some(io) = failure.session.take() {
                    (io, Some(failure.error))
                } else {
                    let mut s = self.shared.state.lock().unwrap();
                    s.state = DriverState::Disconnected;
                    s.cleanup_error = Some(failure.error.clone());
                    return Err(failure.error);
                }
            }
        };
        {
            let mut s = self.shared.state.lock().unwrap();
            s.has_io = true;
            s.reader_quiescent = false;
            if let Some(error) = &configuration_error {
                s.state = DriverState::Fault;
                s.cleanup_error = Some(error.clone());
                s.readonly = true;
            }
        }
        self.spawn_reader(session)?;
        if let Some(error) = configuration_error {
            return Err(error);
        }
        Ok(())
    }
    fn spawn_reader(&mut self, session: SerialSession) -> DriverResult<()> {
        let (normal_tx, normal_rx) = mpsc::sync_channel(8);
        let (safety_tx, safety_rx) = mpsc::sync_channel(1);
        // Until the thread actually starts, this owner holds the native handle.
        // A failed spawn or unwound reader returns it here for cleanup-only retry.
        *self.stranded.lock().unwrap() = Some(session);
        let holder = self.stranded.clone();
        self.shared.state.lock().unwrap().reader_quiescent = false;
        let shared = self.shared.clone();
        let spawn = std::thread::Builder::new()
            .name("voltage-monitor".into())
            .spawn(move || {
                let mut session = holder
                    .lock()
                    .unwrap()
                    .take()
                    .expect("one voltage reader owner");
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    reader_loop(&shared, &mut session, normal_rx, safety_rx)
                }));
                if outcome.is_err() {
                    *holder.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
                    let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                    s.state = DriverState::Fault;
                    s.cleanup_error = Some(DriverError::Responsibility(
                        "voltage reader panicked; native owner retained".into(),
                    ));
                    s.reader_quiescent = true;
                    shared.notify();
                }
            });
        match spawn {
            Ok(reader) => {
                self.reader = Some(reader);
                self.normal = Some(normal_tx);
                self.safety = Some(safety_tx);
            }
            Err(error) => {
                let mut s = self.shared.state.lock().unwrap();
                s.state = DriverState::Fault;
                s.reader_quiescent = true;
                s.cleanup_error = Some(DriverError::Responsibility(format!(
                    "reader spawn failed; native session retained: {error}"
                )));
                return Err(s.cleanup_error.clone().unwrap());
            }
        }
        Ok(())
    }
    fn identity(&self) -> serde_json::Value {
        json!({"model":"8-channel voltage source","resource":self.config.port,"identity_strength":"weak","protocol":"continuous_8ch_telemetry"})
    }
    pub fn connect(&mut self) -> DriverResult<ProbeReport> {
        if matches!(self.state(), DriverState::Ready | DriverState::Active) {
            return Ok(ProbeReport::new(
                self.identity(),
                json!({"zero_evidence":self.zero_evidence()}),
                false,
            ));
        }
        self.start(false)?;
        let result = (|| {
            self.wait_matching(Deadline::after(self.config.startup_timeout), true, |s| {
                s.latest.clone()
            })?;
            self.write([0; 8], true, true)?;
            self.wait_matching(Deadline::after(self.config.startup_timeout), true, |s| {
                if s.zero.state == ZeroState::MeasuredZero {
                    s.latest.clone()
                } else {
                    None
                }
            })?;
            let mut s = self.shared.state.lock().unwrap();
            if self.shared.stop.load(Ordering::Acquire) || s.state != DriverState::Connecting {
                return Err(DriverError::Responsibility(
                    "voltage startup canceled".into(),
                ));
            }
            s.state = DriverState::Ready;
            Ok(ProbeReport::new(
                self.identity(),
                json!({"zero_evidence":s.zero}),
                false,
            ))
        })();
        if result.is_err() {
            let _ = self.close();
        }
        result
    }
    pub fn probe_identity(&mut self) -> DriverResult<ProbeReport> {
        self.start(true)?;
        let observed =
            self.wait_matching(Deadline::after(self.config.startup_timeout), true, |s| {
                s.latest.clone()
            });
        let cleanup = self.close()?;
        match observed {
            Ok(status) => Ok(ProbeReport::new(
                self.identity(),
                json!({"telemetry":status,"cleanup":cleanup}),
                cleanup.resources_released(),
            )),
            Err(error) => Err(error),
        }
    }
    pub fn read_status(&self, max_age: Duration) -> DriverResult<VoltageStatus> {
        let s = self.shared.state.lock().unwrap();
        if !matches!(s.state, DriverState::Ready | DriverState::Active) {
            return Err(DriverError::Responsibility(
                "voltage source not ready".into(),
            ));
        }
        s.latest
            .as_ref()
            .filter(|status| {
                self.clock
                    .now()
                    .checked_sub(status.received_at)
                    .is_some_and(|age| age <= max_age)
            })
            .cloned()
            .ok_or_else(|| timeout("stale voltage telemetry"))
    }
    fn wait_matching(
        &self,
        deadline: Deadline,
        startup: bool,
        predicate: impl Fn(&MonitorState) -> Option<VoltageStatus>,
    ) -> DriverResult<VoltageStatus> {
        let mut s = self.shared.state.lock().unwrap();
        loop {
            if self.shared.stop.load(Ordering::Acquire)
                || !matches!(s.state, DriverState::Ready | DriverState::Active)
                    && !(startup && s.state == DriverState::Connecting)
            {
                return Err(DriverError::Responsibility(
                    "voltage lifecycle canceled/faulted".into(),
                ));
            }
            if let Some(status) = predicate(&s).filter(|v| {
                self.clock
                    .now()
                    .checked_sub(v.received_at)
                    .is_some_and(|age| age <= self.config.telemetry_timeout)
            }) {
                return Ok(status);
            }
            let millis = deadline.remaining_millis()?;
            let (next, _) = self
                .shared
                .changed
                .wait_timeout(s, Duration::from_millis(millis.min(20) as u64))
                .unwrap();
            s = next;
        }
    }
    pub fn wait_for_status(&self, deadline: Deadline) -> DriverResult<VoltageStatus> {
        let sequence = self.shared.state.lock().unwrap().sequence;
        self.wait_matching(deadline, false, |s| {
            if s.sequence > sequence {
                s.latest.clone()
            } else {
                None
            }
        })
    }
    pub fn wait_for_channel(
        &self,
        channel: u8,
        target: f64,
        tolerance: f64,
        deadline: Deadline,
    ) -> DriverResult<VoltageStatus> {
        if !(1..=8).contains(&channel) || !tolerance.is_finite() || tolerance <= 0. {
            return Err(DriverError::Invalid(
                "one-based channel and positive finite tolerance required".into(),
            ));
        }
        codec::codes([target; 8], self.config.limit)?;
        if self.shared.state.lock().unwrap().command_at.is_none() {
            return Err(DriverError::Invalid("no voltage command boundary".into()));
        }
        self.wait_matching(deadline, false, |s| {
            s.latest
                .as_ref()
                .filter(|v| {
                    s.sequence > s.command_sequence
                        && s.latest_frame_generation == s.command_generation
                        && s.command_at.is_some_and(|at| v.received_at >= at)
                        && (v.voltage_v[channel as usize - 1] - target).abs() < tolerance
                })
                .cloned()
        })
    }
    fn write(&mut self, raw: [u16; 8], startup: bool, emergency: bool) -> DriverResult<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        let generation = self.shared.generation.load(Ordering::Acquire);
        let cmd = WriteCommand {
            raw,
            generation,
            startup,
            emergency,
            reply: tx,
        };
        let lane = if emergency {
            &self.safety
        } else {
            &self.normal
        };
        lane.as_ref()
            .ok_or(DriverError::Closed)?
            .try_send(cmd)
            .map_err(|_| DriverError::Busy("voltage command lane full/unavailable".into()))?;
        match rx.recv_timeout(self.config.io_timeout + Duration::from_millis(50)) {
            Ok(reply) => reply,
            Err(_) => {
                self.stop_handle().request_stop();
                Err(timeout("voltage write outcome unknown; never replay"))
            }
        }
    }
    fn normal_ready(&self, generation: u64) -> DriverResult<()> {
        if self.shared.stop.load(Ordering::Acquire)
            || self.shared.generation.load(Ordering::Acquire) != generation
            || !matches!(self.state(), DriverState::Ready | DriverState::Active)
        {
            return Err(DriverError::Responsibility(
                "voltage operation canceled/not ready".into(),
            ));
        }
        if self.shared.state.lock().unwrap().failure_started.is_some() {
            return Err(DriverError::Responsibility(
                "voltage telemetry is recovering; no normal write permitted".into(),
            ));
        }
        self.read_status(self.config.telemetry_timeout)?;
        Ok(())
    }
    pub fn set_channel(&mut self, channel: u8, voltage: f64) -> DriverResult<()> {
        if !(1..=8).contains(&channel) {
            return Err(DriverError::Invalid("channel must be 1–8".into()));
        }
        codec::codes([voltage; 8], self.config.limit)?;
        let mut target = self.commanded_voltages();
        target[channel as usize - 1] = voltage;
        self.ramp_to(target)
    }
    pub fn set_all(&mut self, values: [f64; 8]) -> DriverResult<()> {
        self.ramp_to(values)
    }
    pub fn ramp_to(&mut self, values: [f64; 8]) -> DriverResult<()> {
        let target = codec::codes(values, self.config.limit)?;
        let generation = self.shared.generation.load(Ordering::Acquire);
        self.normal_ready(generation)?;
        let begin = self.shared.state.lock().unwrap().commanded;
        let frames = ramp::plan(begin, target, self.config.ramp_step)?;
        for raw in frames {
            self.normal_ready(generation)?;
            let last = self.shared.state.lock().unwrap().command_at;
            if let Some(last) = last {
                let age = self.clock.now().checked_sub(last).ok_or_else(|| {
                    DriverError::Invalid("monotonic voltage clock reversed".into())
                })?;
                if age < self.config.ramp_interval {
                    self.clock.wait(self.config.ramp_interval - age);
                }
            }
            self.normal_ready(generation)?;
            self.write(raw, false, false)?;
            self.normal_ready(generation)?;
        }
        Ok(())
    }
    pub fn zero(&mut self, emergency: bool) -> DriverResult<()> {
        if !emergency {
            return self.ramp_to([0.; 8]);
        }
        self.normal_ready(self.shared.generation.load(Ordering::Acquire))?;
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.write([0; 8], false, true)
    }
    fn begin_close(&mut self) -> u64 {
        if let Some(request) = self.close_attempt {
            return request;
        }
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        let mut s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        s.state = DriverState::Closing;
        s.zero = ZeroEvidence::unknown();
        drop(s);
        let request = self.shared.close_request.fetch_add(1, Ordering::AcqRel) + 1;
        self.close_attempt = Some(request);
        self.shared.notify();
        request
    }
    pub fn close(&mut self) -> DriverResult<CleanupReport> {
        if self.resources_released() {
            let completed = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .receipt
                .as_ref()
                .map(|(_, report)| report.clone());
            let result = completed
                .or_else(|| self.last_cleanup.clone())
                .unwrap_or_else(|| report(vec![step("no_session", None)], None, vec![]));
            self.close_attempt = None;
            self.last_cleanup = Some(result.clone());
            self.shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state = DriverState::Disconnected;
            return Ok(result);
        }
        if self
            .reader
            .as_ref()
            .is_none_or(|reader| reader.is_finished())
        {
            let owned = self
                .stranded
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(session) = owned {
                let _ = self.spawn_reader(session);
            }
        }
        let request = self.begin_close();
        let deadline = Deadline::after(self.config.close_timeout);
        let mut s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some((sequence, result)) = &s.receipt {
                if *sequence == request {
                    let result = result.clone();
                    self.close_attempt = None;
                    self.last_cleanup = Some(result.clone());
                    return Ok(result);
                }
            }
            let millis = match deadline.remaining_millis() {
                Ok(ms) => ms,
                Err(_) => break,
            };
            let (next, _) = self
                .shared
                .changed
                .wait_timeout(s, Duration::from_millis(millis.min(20) as u64))
                .unwrap_or_else(|e| e.into_inner());
            s = next;
        }
        let zero = s.zero.clone();
        let pending = report(
            vec![step(
                "reader_or_cleanup_pending",
                Some(&timeout("voltage cleanup pending")),
            )],
            Some(zero),
            vec![self.config.port.clone()],
        );
        self.last_cleanup = Some(pending.clone());
        Ok(pending)
    }
}
impl DriverLifecycle for VoltageSource {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        VoltageSource::close(self)
    }
    fn has_responsibility(&self) -> bool {
        self.has_resource_responsibility()
    }
}
impl Drop for VoltageSource {
    fn drop(&mut self) {
        if self.retain_on_drop && self.has_resource_responsibility() {
            self.begin_close();
            let owner = Self {
                config: self.config.clone(),
                book: self.book.clone(),
                clock: self.clock.clone(),
                backend: self.backend.clone(),
                shared: self.shared.clone(),
                normal: self.normal.take(),
                safety: self.safety.take(),
                reader: self.reader.take(),
                close_attempt: self.close_attempt.take(),
                last_cleanup: self.last_cleanup.take(),
                stranded: self.stranded.clone(),
                retain_on_drop: false,
            };
            retained()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(owner);
        }
    }
}
pub fn retry_retained() -> usize {
    let owners = std::mem::take(&mut *retained().lock().unwrap_or_else(|e| e.into_inner()));
    for mut owner in owners {
        if !owner
            .close()
            .is_ok_and(|report| report.resources_released())
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

fn write_codes(
    shared: &Shared,
    io: &mut SerialSession,
    _decoder: &mut TelemetryDecoder,
    raw: [u16; 8],
) -> DriverResult<()> {
    {
        let mut s = shared.state.lock().unwrap();
        s.zero = ZeroEvidence::unknown();
    }
    io.write_all(
        &codec::encode_codes(raw),
        Deadline::after(shared.config.io_timeout),
    )?;
    let mut s = shared.state.lock().unwrap();
    s.command_generation = s.command_generation.saturating_add(1);
    s.commanded = raw;
    s.command_at = Some(shared.clock.now());
    s.command_sequence = s.sequence;
    if raw == [0; 8] {
        s.zero_minimum_sequence = s.sequence;
        s.zero = ZeroEvidence {
            state: ZeroState::CommandSent,
            sent_at: s.command_at,
            observed_at: None,
            voltage_v: None,
        };
    }
    if matches!(s.state, DriverState::Ready | DriverState::Active) {
        s.state = if raw == [0; 8] {
            DriverState::Ready
        } else {
            DriverState::Active
        };
    }
    shared.notify();
    Ok(())
}
fn publish(shared: &Shared, status: VoltageStatus, frame_generation: u64) {
    let mut s = shared.state.lock().unwrap();
    s.sequence = s.sequence.saturating_add(1);
    s.failure_started = None;
    s.latest_frame_generation = frame_generation;
    if status.voltage_v.iter().any(|v| v.abs() >= 0.05) {
        let sent_at = s.zero.sent_at;
        s.zero = ZeroEvidence {
            state: ZeroState::Unknown,
            sent_at,
            observed_at: None,
            voltage_v: None,
        };
    } else if frame_generation == s.command_generation
        && s.zero.sent_at.is_some_and(|at| status.received_at >= at)
        && s.sequence > s.zero_minimum_sequence
    {
        s.zero = ZeroEvidence {
            state: ZeroState::MeasuredZero,
            sent_at: s.zero.sent_at,
            observed_at: Some(status.received_at),
            voltage_v: Some(status.voltage_v),
        };
    }
    s.latest = Some(status);
    shared.notify();
}
fn poll(
    shared: &Shared,
    io: &mut SerialSession,
    decoder: &mut TelemetryDecoder,
) -> DriverResult<bool> {
    let generation = shared.state.lock().unwrap().command_generation;
    match io.read_bounded(
        68,
        Deadline::after(shared.config.io_timeout.min(Duration::from_millis(10))),
    ) {
        Ok(bytes) => {
            let statuses = decoder.feed_scoped(&bytes, shared.clock.now(), generation);
            let found = !statuses.is_empty();
            for (status, frame_generation) in statuses {
                publish(shared, status, frame_generation);
            }
            Ok(found)
        }
        Err(DriverError::Timeout { transferred: 0, .. }) => Ok(false),
        Err(error) => Err(error),
    }
}
fn enter_fault(
    shared: &Shared,
    io: &mut SerialSession,
    decoder: &mut TelemetryDecoder,
    error: DriverError,
) {
    shared.generation.fetch_add(1, Ordering::AcqRel);
    shared.stop.store(true, Ordering::Release);
    let readonly = {
        let mut s = shared.state.lock().unwrap();
        s.state = DriverState::Fault;
        s.cleanup_error = Some(error);
        s.zero = ZeroEvidence::unknown();
        s.readonly
    };
    shared.notify();
    if !readonly {
        if let Err(error) = write_codes(shared, io, decoder, [0; 8]) {
            shared.state.lock().unwrap().cleanup_error = Some(error);
        }
    }
}
fn cleanup(
    shared: &Shared,
    io: &mut SerialSession,
    decoder: &mut TelemetryDecoder,
    retry: bool,
) -> CleanupReport {
    let readonly = shared.state.lock().unwrap().readonly;
    let mut steps = vec![];
    if !retry && !readonly {
        let zero = write_codes(shared, io, decoder, [0; 8]);
        steps.push(step("zero_command", zero.as_ref().err()));
        if zero.is_ok() {
            let budget = (shared.config.close_timeout / 2).min(Duration::from_secs(1));
            let deadline = Deadline::after(budget);
            while shared.current_zero().state != ZeroState::MeasuredZero
                && deadline.remaining_millis().is_ok()
            {
                match poll(shared, io, decoder) {
                    Ok(_) => {}
                    Err(error) => {
                        steps.push(step("zero_observation", Some(&error)));
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    } else {
        steps.push(step(
            if readonly {
                "read_only_close"
            } else {
                "retry_transport_close"
            },
            None,
        ));
    }
    let zero = (!readonly).then(|| shared.current_zero());
    if zero
        .as_ref()
        .is_some_and(|e| e.state != ZeroState::MeasuredZero)
    {
        steps.push(step(
            "zero_unconfirmed",
            Some(&DriverError::Responsibility(
                "host-observed zero is unconfirmed; no physical measurement is claimed".into(),
            )),
        ));
    }
    let closed = io.close();
    let released = closed.as_ref().is_ok_and(|r| r.released) && !io.has_responsibility();
    let error = closed.as_ref().err().cloned().or_else(|| {
        (!released).then(|| DriverError::Responsibility("serial release unconfirmed".into()))
    });
    let output_error = zero
        .as_ref()
        .filter(|e| e.state != ZeroState::MeasuredZero)
        .map(|_| DriverError::Responsibility("voltage shutdown zero is unconfirmed".into()));
    steps.push(step("transport_close", error.as_ref()));
    let result = report(
        steps,
        zero,
        if released {
            vec![]
        } else {
            vec![shared.config.port.clone()]
        },
    );
    {
        let mut s = shared.state.lock().unwrap();
        s.has_io = !released;
        s.state = if released {
            DriverState::Disconnected
        } else {
            DriverState::Fault
        };
        s.cleanup_error = error.or(output_error);
    }
    result
}
fn reader_loop(
    shared: &Shared,
    io: &mut SerialSession,
    normal: Receiver<WriteCommand>,
    safety: Receiver<WriteCommand>,
) {
    let mut decoder = TelemetryDecoder::default();
    let mut closed_attempt = 0;
    let mut close_started = false;
    let mut recovery_poll_at = None;
    loop {
        let requested = shared.close_request.load(Ordering::Acquire);
        if requested > closed_attempt {
            let receipt = cleanup(shared, io, &mut decoder, close_started);
            close_started = true;
            closed_attempt = requested;
            let released = receipt.resources_released();
            let mut s = shared.state.lock().unwrap();
            s.reader_quiescent = released;
            s.receipt = Some((requested, receipt));
            shared.notify();
            drop(s);
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
            enter_fault(
                shared,
                io,
                &mut decoder,
                DriverError::Responsibility("voltage cancellation requires zero and close".into()),
            );
            continue;
        }
        let cmd = match safety.try_recv() {
            Ok(cmd) => Some(cmd),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => normal.try_recv().ok(),
        };
        if let Some(cmd) = cmd {
            let allowed = shared.generation.load(Ordering::Acquire) == cmd.generation
                && !shared.stop.load(Ordering::Acquire)
                && (matches!(state, DriverState::Ready | DriverState::Active)
                    || cmd.startup && state == DriverState::Connecting)
                && !shared.state.lock().unwrap().readonly;
            let result = if allowed {
                write_codes(shared, io, &mut decoder, cmd.raw)
            } else {
                Err(DriverError::Responsibility(
                    "voltage command canceled before write".into(),
                ))
            };
            if result.is_err() && allowed {
                enter_fault(
                    shared,
                    io,
                    &mut decoder,
                    result.as_ref().err().unwrap().clone(),
                );
            }
            if cmd.emergency && !cmd.startup {
                shared.generation.fetch_add(1, Ordering::AcqRel);
            }
            let delivered = result.is_ok();
            let _ = cmd.reply.send(result);
            // A stream reader cannot be starved by a sequence of ordinary writes.
            // Safety/close metadata still preempts this finite poll.
            if delivered
                && !shared.stop.load(Ordering::Acquire)
                && shared.close_request.load(Ordering::Acquire) == closed_attempt
            {
                if let Err(error) = poll(shared, io, &mut decoder) {
                    enter_fault(shared, io, &mut decoder, error);
                }
            }
            continue;
        }
        if matches!(
            state,
            DriverState::Connecting | DriverState::Ready | DriverState::Active
        ) {
            let recovering = shared.state.lock().unwrap().failure_started;
            if recovering.is_some_and(|began| {
                shared.clock.now().saturating_sub(began)
                    >= shared.config.communication_recovery_timeout
            }) {
                enter_fault(
                    shared,
                    io,
                    &mut decoder,
                    timeout("voltage telemetry recovery exhausted"),
                );
                continue;
            }
            if recovering.is_some()
                && recovery_poll_at.is_some_and(|at| std::time::Instant::now() < at)
            {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            match poll(shared, io, &mut decoder) {
                Ok(true) => {
                    recovery_poll_at = None;
                }
                Ok(false) => {
                    let mut s = shared.state.lock().unwrap();
                    let now = shared.clock.now();
                    let since = s.latest.as_ref().map_or(Duration::ZERO, |v| v.received_at);
                    let silence = now.checked_sub(since).unwrap_or(Duration::MAX);
                    let exhausted = if silence >= shared.config.io_timeout {
                        let began = *s
                            .failure_started
                            .get_or_insert(since + shared.config.io_timeout);
                        s.zero = ZeroEvidence::unknown();
                        recovery_poll_at = Some(
                            std::time::Instant::now() + shared.config.communication_retry_interval,
                        );
                        now.saturating_sub(began) >= shared.config.communication_recovery_timeout
                    } else {
                        false
                    };
                    drop(s);
                    if exhausted {
                        enter_fault(
                            shared,
                            io,
                            &mut decoder,
                            timeout("voltage telemetry recovery exhausted"),
                        );
                    }
                }
                Err(error) => enter_fault(shared, io, &mut decoder, error),
            };
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
