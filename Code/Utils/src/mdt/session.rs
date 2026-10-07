use super::{
    protocol::{self, Protocol},
    Axis, AxisState, MdtStatus, RotaryMode, VoltageLimit,
};
use crate::{
    clock::Clock,
    lifecycle::{CleanupReport, CleanupStep, DriverLifecycle, DriverState, ProbeReport},
    transport::{
        serial::{canonical_com, SerialConfig, SerialSession},
        serial_abi::{NativeSerial, SerialBackend},
        ByteTransport, Deadline, ResourceBook,
    },
    DriverError, DriverResult,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Condvar, Mutex, OnceLock,
    },
    thread::JoinHandle,
    time::{Duration, SystemTime},
};
pub(crate) const QUERIES: &[&str] = &[
    "?",
    "id?",
    "serial?",
    "friendly?",
    "echo?",
    "vlimit?",
    "intensity?",
    "msenable?",
    "msvoltage?",
    "xvoltage?",
    "yvoltage?",
    "zvoltage?",
    "xmin?",
    "ymin?",
    "zmin?",
    "xmax?",
    "ymax?",
    "zmax?",
    "dacstep?",
    "cm?",
    "rotarymode?",
    "pushdisable?",
];
#[derive(Clone)]
pub struct MdtConfig {
    pub port: String,
    pub application_limits_v: [f64; 3],
    pub ramp_step_v: f64,
    pub ramp_interval: Duration,
    pub io_timeout: Duration,
    pub monitor_interval: Duration,
    pub monitor_failure_limit: u32,
    pub close_timeout: Duration,
    pub start_monitor: bool,
}
impl Default for MdtConfig {
    fn default() -> Self {
        Self {
            port: String::new(),
            application_limits_v: [75.; 3],
            ramp_step_v: 0.1,
            ramp_interval: Duration::from_millis(50),
            io_timeout: Duration::from_millis(500),
            monitor_interval: Duration::from_millis(500),
            monitor_failure_limit: 3,
            close_timeout: Duration::from_secs(2),
            start_monitor: true,
        }
    }
}
pub(crate) struct State {
    pub state: DriverState,
    pub status: Option<MdtStatus>,
    pub has_io: bool,
    pub quiescent: bool,
    pub cleanup: Option<CleanupReport>,
    pub error: Option<DriverError>,
}
pub(crate) struct Shared {
    pub state: Mutex<State>,
    pub changed: Condvar,
    pub stop: AtomicBool,
    pub closing: AtomicBool,
    pub generation: AtomicU64,
    pub connection: AtomicU64,
    pub config: MdtConfig,
    pub clock: Arc<dyn Clock>,
}
#[derive(Clone)]
pub struct StopHandle {
    shared: Arc<Shared>,
    connection: u64,
}
impl StopHandle {
    pub fn request_stop(&self) {
        if self.connection == self.shared.connection.load(Ordering::Acquire) {
            self.shared.stop.store(true, Ordering::Release);
            self.shared.generation.fetch_add(1, Ordering::AcqRel);
            let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(status) = st.status.as_mut() {
                status.axis_command_known = false;
                status.baseline_evidence = None;
                status.fault_evidence = Some("MDT metadata stop; hold".into());
            }
        }
    }
}
pub(crate) struct Io {
    pub protocol: Protocol,
    pub status: Option<MdtStatus>,
    pub authority: Option<super::authority::Authority>,
    pub wrote: bool,
    pub last_motion: Option<Duration>,
    pub unsupported_arrows: BTreeSet<String>,
}
pub(super) enum Op {
    Snapshot { recover: bool },
    Adopt(super::BaselineAttestation),
    Setting(super::settings::Setting),
    Motion(super::motion::Motion),
    EmergencyZero,
    Restore,
}
struct Job {
    op: Op,
    generation: u64,
    deadline: Deadline,
    reply: mpsc::Sender<DriverResult<MdtStatus>>,
}
pub struct Mdt693b {
    pub(crate) shared: Arc<Shared>,
    book: ResourceBook,
    backend: Arc<dyn SerialBackend>,
    normal: Option<SyncSender<Job>>,
    priority: Option<SyncSender<Job>>,
    actor: Option<JoinHandle<()>>,
    stranded: Arc<Mutex<Option<Io>>>,
    last_cleanup: Option<CleanupReport>,
    retain_on_drop: bool,
}
impl Mdt693b {
    pub fn new(config: MdtConfig, book: ResourceBook, clock: Arc<dyn Clock>) -> DriverResult<Self> {
        Self::with_backend(config, book, clock, Arc::new(NativeSerial))
    }
    pub fn with_backend(
        config: MdtConfig,
        book: ResourceBook,
        clock: Arc<dyn Clock>,
        backend: Arc<dyn SerialBackend>,
    ) -> DriverResult<Self> {
        canonical_com(&config.port)?;
        if config
            .application_limits_v
            .iter()
            .any(|v| !v.is_finite() || *v <= 0. || *v > 75.)
            || !config.ramp_step_v.is_finite()
            || config.ramp_step_v <= 0.
            || config.ramp_step_v > 0.1
            || config.ramp_interval < Duration::from_millis(50)
            || [
                config.io_timeout,
                config.monitor_interval,
                config.close_timeout,
                config.ramp_interval,
            ]
            .iter()
            .any(|d| d.is_zero() || *d > Duration::from_secs(180))
            || config.monitor_failure_limit == 0
            || config.monitor_failure_limit > 128
        {
            return Err(DriverError::Invalid(
                "invalid MDT safety configuration".into(),
            ));
        }
        Ok(Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    state: DriverState::Disconnected,
                    status: None,
                    has_io: false,
                    quiescent: true,
                    cleanup: None,
                    error: None,
                }),
                changed: Condvar::new(),
                stop: AtomicBool::new(false),
                closing: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                connection: AtomicU64::new(0),
                config,
                clock,
            }),
            book,
            backend,
            normal: None,
            priority: None,
            actor: None,
            stranded: Arc::new(Mutex::new(None)),
            last_cleanup: None,
            retain_on_drop: true,
        })
    }
    pub fn config(&self) -> &MdtConfig {
        &self.shared.config
    }
    pub fn state(&self) -> DriverState {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .state
    }
    pub fn status(&self) -> Option<MdtStatus> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .clone()
    }
    pub fn has_resource_responsibility(&self) -> bool {
        let s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        s.has_io || !s.quiescent
    }
    pub fn cleanup_error(&self) -> Option<DriverError> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .error
            .clone()
    }
    pub fn cleanup_report(&self) -> Option<&CleanupReport> {
        self.last_cleanup.as_ref()
    }
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle {
            shared: self.shared.clone(),
            connection: self.shared.connection.load(Ordering::Acquire),
        }
    }
    fn join_finished(&mut self) {
        if self.actor.as_ref().is_some_and(JoinHandle::is_finished) {
            let _ = self.actor.take().unwrap().join();
        }
    }
    fn spawn(&mut self) -> DriverResult<()> {
        self.join_finished();
        if self.actor.is_some() {
            return Err(DriverError::Responsibility(
                "MDT previous actor retained".into(),
            ));
        }
        let (tx, rx) = mpsc::sync_channel(8);
        let (urgent, urgent_rx) = mpsc::sync_channel(1);
        let shared = self.shared.clone();
        let slot = self.stranded.clone();
        // Publish non-quiescence before the new thread can publish completion.
        self.shared.state.lock().unwrap().quiescent = false;
        let thread = std::thread::Builder::new()
            .name("mdt-io".into())
            .spawn(move || {
                let mut io = slot
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                    .unwrap();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    actor(&shared, &mut io, rx, urgent_rx)
                }));
                let live = io.protocol.session.has_responsibility();
                if live {
                    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(io);
                }
                let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                s.has_io = live;
                s.quiescent = true;
                if r.is_err() {
                    s.state = DriverState::Fault;
                    s.error = Some(DriverError::Responsibility(
                        "MDT actor panicked; retained".into(),
                    ));
                }
                shared.changed.notify_all();
            })
            .map_err(|e| {
                self.shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .quiescent = true;
                DriverError::Responsibility(format!("MDT actor launch failed: {e}"))
            })?;
        self.normal = Some(tx);
        self.priority = Some(urgent);
        self.actor = Some(thread);
        Ok(())
    }
    pub fn connect(&mut self) -> DriverResult<ProbeReport> {
        if self.state() == DriverState::Ready {
            return self.report(false);
        }
        if self.has_resource_responsibility() || self.state() != DriverState::Disconnected {
            return Err(DriverError::Responsibility(
                "close MDT before connection".into(),
            ));
        }
        self.join_finished();
        self.shared.stop.store(false, Ordering::Release);
        self.shared.closing.store(false, Ordering::Release);
        self.shared.connection.fetch_add(1, Ordering::AcqRel);
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        {
            let mut s = self.shared.state.lock().unwrap();
            s.state = DriverState::Connecting;
            s.status = None;
            s.error = None;
            s.cleanup = None;
        }
        let opened = SerialSession::from_backend_owned(
            SerialConfig::instrument(&self.shared.config.port),
            &self.book,
            self.backend.clone(),
        );
        let session = match opened {
            Ok(s) => s,
            Err(e) => {
                if let Some(s) = e.session {
                    *self.stranded.lock().unwrap() = Some(Io {
                        protocol: Protocol::new(s),
                        status: None,
                        authority: None,
                        wrote: false,
                        last_motion: None,
                        unsupported_arrows: BTreeSet::new(),
                    });
                    self.shared.state.lock().unwrap().has_io = true;
                    let _ = self.close();
                } else {
                    self.shared.state.lock().unwrap().state = DriverState::Disconnected;
                }
                return Err(e.error);
            }
        };
        *self.stranded.lock().unwrap() = Some(Io {
            protocol: Protocol::new(session),
            status: None,
            authority: None,
            wrote: false,
            last_motion: None,
            unsupported_arrows: BTreeSet::new(),
        });
        self.shared.state.lock().unwrap().has_io = true;
        if let Err(e) = self.spawn() {
            self.shared.state.lock().unwrap().state = DriverState::Fault;
            return Err(e);
        }
        match self.request(Op::Snapshot { recover: true }) {
            Ok(_) => self.report(false),
            Err(e) => {
                let _ = self.close();
                Err(e)
            }
        }
    }
    fn report(&self, released: bool) -> DriverResult<ProbeReport> {
        let s = self.status().ok_or(DriverError::Closed)?;
        Ok(ProbeReport::new(
            serde_json::json!({"model":s.product,"serial":s.serial_number,"firmware":s.firmware,"resource":self.shared.config.port,"quality":"strong_identity"}),
            serde_json::json!({"voltage_v":s.axes,"axis_command_known":s.axis_command_known,"restricted":s.restricted}),
            released,
        ))
    }
    pub fn probe_identity(&mut self) -> DriverResult<ProbeReport> {
        if self.has_resource_responsibility() || self.state() != DriverState::Disconnected {
            return Err(DriverError::Busy("MDT probe requires disconnection".into()));
        }
        let r = self.connect()?;
        let released = self.close()?.resources_released();
        Ok(ProbeReport::new(
            r.identity().clone(),
            r.observations().clone(),
            released,
        ))
    }
    pub(super) fn request(&self, op: Op) -> DriverResult<MdtStatus> {
        let packets = if matches!(op, Op::Setting(_)) { 60 } else { 30 };
        self.request_budget(
            op,
            self.shared.config.io_timeout.saturating_mul(packets) + Duration::from_millis(50),
            false,
        )
    }
    pub(super) fn request_budget(
        &self,
        op: Op,
        budget: Duration,
        priority: bool,
    ) -> DriverResult<MdtStatus> {
        if self.shared.closing.load(Ordering::Acquire) {
            return Err(DriverError::Closed);
        }
        let tx = if priority {
            self.priority.as_ref()
        } else {
            self.normal.as_ref()
        }
        .ok_or(DriverError::Closed)?;
        let generation = if priority {
            let g = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
            self.shared.stop.store(false, Ordering::Release);
            let mut state = self.shared.state.lock().unwrap();
            if let Some(st) = state.status.as_mut() {
                st.axis_command_known = false;
                st.baseline_evidence = None;
            }
            g
        } else {
            self.shared.generation.load(Ordering::Acquire)
        };
        let deadline = Deadline::after(budget);
        let (reply, rx) = mpsc::channel();
        tx.try_send(Job {
            op,
            generation,
            deadline,
            reply,
        })
        .map_err(|_| DriverError::Busy("MDT queue full or unavailable".into()))?;
        match rx.recv_timeout(budget) {
            Ok(r) => r,
            Err(_) => {
                self.stop_handle().request_stop();
                Err(DriverError::Timeout {
                    operation: "MDT job retained after caller deadline".into(),
                    transferred: 0,
                })
            }
        }
    }
    pub fn recover(&self) -> DriverResult<MdtStatus> {
        if !matches!(self.state(), DriverState::Ready | DriverState::Fault) {
            return Err(DriverError::Closed);
        }
        self.request_budget(
            Op::Snapshot { recover: true },
            self.shared.config.io_timeout.saturating_mul(30) + Duration::from_millis(50),
            true,
        )
    }
    pub(super) fn refresh(&self) -> DriverResult<MdtStatus> {
        self.request(Op::Snapshot { recover: false })
    }
    pub fn get_supported_commands(&self) -> DriverResult<BTreeSet<String>> {
        Ok(self.refresh()?.supported_commands)
    }
    pub fn get_product_information(&self) -> DriverResult<(String, String)> {
        let s = self.refresh()?;
        Ok((s.product, s.firmware))
    }
    pub fn get_serial_number(&self) -> DriverResult<String> {
        Ok(self.refresh()?.serial_number)
    }
    pub fn get_axis_voltage(&self, a: Axis) -> DriverResult<f64> {
        Ok(self.refresh()?.axes[&a].actual_v)
    }
    pub fn get_all_voltages(&self) -> DriverResult<[f64; 3]> {
        let s = self.refresh()?;
        Ok([
            s.axes[&Axis::X].actual_v,
            s.axes[&Axis::Y].actual_v,
            s.axes[&Axis::Z].actual_v,
        ])
    }
    pub fn get_axis_state(&self, a: Axis) -> DriverResult<AxisState> {
        Ok(self.refresh()?.axes[&a])
    }
    pub fn close(&mut self) -> DriverResult<CleanupReport> {
        self.stop_handle().request_stop();
        self.shared.closing.store(true, Ordering::Release);
        {
            let mut s = self.shared.state.lock().unwrap();
            s.state = DriverState::Closing;
        }
        self.join_finished();
        if self.actor.is_none() && self.stranded.lock().unwrap().is_some() {
            self.shared.state.lock().unwrap().cleanup = None;
            if let Err(e) = self.spawn() {
                self.shared.state.lock().unwrap().error = Some(e);
            }
        }
        let d = Deadline::after(self.shared.config.close_timeout);
        let mut s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        while !s.quiescent {
            let Ok(ms) = d.remaining_millis() else { break };
            s = self
                .shared
                .changed
                .wait_timeout(s, Duration::from_millis(u64::from(ms)))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let report = if s.quiescent {
            if let Some(r) = &s.cleanup {
                r.clone()
            } else {
                cleanup_report(!s.has_io, None)?
            }
        } else {
            cleanup_report(
                false,
                Some(DriverError::Timeout {
                    operation: "MDT cleanup still retained".into(),
                    transferred: 0,
                }),
            )?
        };
        s.state = if report.resources_released() {
            DriverState::Disconnected
        } else {
            DriverState::Fault
        };
        drop(s);
        self.last_cleanup = Some(report.clone());
        Ok(report)
    }
}
pub(crate) fn guard(s: &Shared, g: u64) -> DriverResult<()> {
    if s.stop.load(Ordering::Acquire)
        || s.closing.load(Ordering::Acquire)
        || g != s.generation.load(Ordering::Acquire)
    {
        Err(DriverError::Responsibility(
            "MDT canceled; stop and hold".into(),
        ))
    } else {
        Ok(())
    }
}
impl Io {
    pub(super) fn query(
        &mut self,
        s: &Shared,
        c: &str,
        g: u64,
        end: Deadline,
    ) -> DriverResult<Vec<String>> {
        guard(s, g)?;
        let r = self
            .protocol
            .exchange(c, end.earlier(Deadline::after(s.config.io_timeout)))?;
        guard(s, g)?;
        Ok(r)
    }
    pub(super) fn snapshot(
        &mut self,
        s: &Shared,
        g: u64,
        end: Deadline,
    ) -> DriverResult<MdtStatus> {
        self.protocol.echo = None;
        let help = self.query(s, "?", g, end)?;
        let mut supported: BTreeSet<String> = help
            .iter()
            .flat_map(|l| l.split(|c: char| c.is_whitespace() || c == ','))
            .filter(|t| !t.is_empty())
            .map(|t| t.to_ascii_lowercase())
            .collect();
        supported.insert("?".into());
        self.protocol.echo = None;
        for &q in QUERIES {
            if !supported.contains(q) {
                return Err(DriverError::Invalid(format!("MDT firmware lacks {q}")));
            }
        }
        let id = self.query(s, "id?", g, end)?;
        let fields: Vec<String> = if id.len() == 2 {
            id.clone()
        } else {
            let v = protocol::single(&id)?;
            if v.contains(',') {
                v.splitn(2, ',').map(str::to_string).collect()
            } else {
                let parts: Vec<&str> = v.split_whitespace().collect();
                if parts.len() == 2 {
                    parts.into_iter().map(str::to_string).collect()
                } else if parts.len() == 3 && parts[1].eq_ignore_ascii_case("firmware") {
                    vec![parts[0].into(), parts[2].into()]
                } else {
                    return Err(DriverError::Protocol("MDT missing identity fields".into()));
                }
            }
        };
        let product = fields[0].rsplit(':').next().unwrap().trim().to_string();
        let firmware = fields[1].rsplit(':').next().unwrap().trim().to_string();
        if !product.eq_ignore_ascii_case("MDT693B") || firmware.is_empty() {
            return Err(DriverError::Protocol("unexpected MDT identity".into()));
        }
        let serial_number: String = protocol::single(&self.query(s, "serial?", g, end)?)?
            .trim()
            .into();
        if serial_number.is_empty() {
            return Err(DriverError::Protocol(
                "MDT device serial must not be empty".into(),
            ));
        }
        let friendly_name = protocol::single(&self.query(s, "friendly?", g, end)?)?;
        let echo_enabled = protocol::boolean(&self.query(s, "echo?", g, end)?)?;
        if self.protocol.echo != Some(echo_enabled) {
            return Err(DriverError::Protocol(
                "MDT echo query contradicts wire".into(),
            ));
        }
        let hardware_limit = match protocol::integer(&self.query(s, "vlimit?", g, end)?, 0, 150)? {
            0 | 75 => VoltageLimit::V75,
            1 | 100 => VoltageLimit::V100,
            2 | 150 => VoltageLimit::V150,
            _ => {
                return Err(DriverError::Protocol(
                    "unknown hardware voltage limit".into(),
                ))
            }
        };
        let display_intensity =
            protocol::integer(&self.query(s, "intensity?", g, end)?, 0, 15)? as u8;
        let master_scan_enabled = protocol::boolean(&self.query(s, "msenable?", g, end)?)?;
        let master_scan_voltage_v = protocol::number(&self.query(s, "msvoltage?", g, end)?)?;
        let mut actual = [0.; 3];
        let mut minimum = [0.; 3];
        let mut maximum = [0.; 3];
        for a in [Axis::X, Axis::Y, Axis::Z] {
            actual[a.index()] =
                protocol::number(&self.query(s, &format!("{}voltage?", a.token()), g, end)?)?;
        }
        for a in [Axis::X, Axis::Y, Axis::Z] {
            minimum[a.index()] =
                protocol::number(&self.query(s, &format!("{}min?", a.token()), g, end)?)?;
        }
        for a in [Axis::X, Axis::Y, Axis::Z] {
            maximum[a.index()] =
                protocol::number(&self.query(s, &format!("{}max?", a.token()), g, end)?)?;
        }
        let mut axes = BTreeMap::new();
        for a in [Axis::X, Axis::Y, Axis::Z] {
            let i = a.index();
            if minimum[i] > maximum[i] {
                return Err(DriverError::Protocol("inverted MDT axis limits".into()));
            }
            axes.insert(
                a,
                AxisState {
                    actual_v: actual[i],
                    minimum_v: minimum[i],
                    maximum_v: maximum[i],
                },
            );
        }
        let dac_step = protocol::integer(&self.query(s, "dacstep?", g, end)?, 1, 1000)? as u16;
        let compatibility_enabled = protocol::boolean(&self.query(s, "cm?", g, end)?)?;
        let rotary_mode = match protocol::integer(&self.query(s, "rotarymode?", g, end)?, 0, 2)? {
            0 => RotaryMode::Default,
            1 => RotaryMode::TenTurn,
            _ => RotaryMode::Fine,
        };
        let push_to_adjust_disabled = protocol::boolean(&self.query(s, "pushdisable?", g, end)?)?;
        let restricted = actual.iter().enumerate().any(|(i, v)| {
            *v < minimum[i]
                || *v
                    > maximum[i]
                        .min(hardware_limit.volts())
                        .min(s.config.application_limits_v[i])
        });
        Ok(MdtStatus {
            product,
            firmware,
            serial_number,
            friendly_name,
            echo_enabled,
            hardware_limit,
            display_intensity,
            master_scan_enabled,
            master_scan_voltage_v,
            axes,
            dac_step,
            compatibility_enabled,
            rotary_mode,
            push_to_adjust_disabled,
            supported_commands: supported,
            restricted,
            fault_evidence: restricted
                .then(|| format!("MDT actual over project ceiling: {actual:?}")),
            observed_at: s.clock.now().as_secs_f64(),
            axis_command_known: false,
            baseline_evidence: None,
            selected_channel: None,
        })
    }
}
pub(crate) fn publish(s: &Shared, io: &mut Io, mut status: MdtStatus) {
    let mut state = s.state.lock().unwrap();
    let live = !s.stop.load(Ordering::Acquire)
        && !s.closing.load(Ordering::Acquire)
        && io
            .authority
            .as_ref()
            .is_some_and(|a| a.generation == s.generation.load(Ordering::Acquire));
    status.axis_command_known &= live;
    if !status.axis_command_known {
        status.baseline_evidence = None;
    }
    io.status = Some(status.clone());
    state.status = Some(status);
    if state.state != DriverState::Active
        && !s.closing.load(Ordering::Acquire)
        && !s.stop.load(Ordering::Acquire)
    {
        state.state = DriverState::Ready;
    }
    state.error = None;
    s.changed.notify_all();
}
fn actor(s: &Shared, io: &mut Io, rx: Receiver<Job>, urgent: Receiver<Job>) {
    let mut due = s.clock.now() + s.config.monitor_interval;
    let mut failures = 0;
    loop {
        if s.closing.load(Ordering::Acquire) {
            let r = io.protocol.session.close();
            let released =
                r.as_ref().is_ok_and(|r| r.released) && !io.protocol.session.has_responsibility();
            let error = match r {
                Err(e) => Some(e),
                Ok(r) if !released => Some(DriverError::Native {
                    operation: "MDT serial close".into(),
                    status: r.status.unwrap_or(-1),
                    transferred: 0,
                }),
                _ => None,
            };
            let report = cleanup_report(released, error.clone()).unwrap();
            let mut state = s.state.lock().unwrap();
            state.has_io = !released;
            state.cleanup = Some(report);
            state.error = error;
            return;
        }
        if s.config.start_monitor
            && !s.stop.load(Ordering::Acquire)
            && s.state.lock().unwrap().state == DriverState::Ready
            && s.clock.now() >= due
        {
            let g = s.generation.load(Ordering::Acquire);
            let result = super::monitor::poll(s, io, g);
            due = s.clock.now() + s.config.monitor_interval;
            match result {
                Ok(status) => {
                    failures = 0;
                    publish(s, io, status);
                }
                Err(e) => {
                    io.invalidate("MDT monitor uncertainty; hold");
                    failures += 1;
                    let terminal = io.protocol.broken || failures >= s.config.monitor_failure_limit;
                    let mut state = s.state.lock().unwrap();
                    if let Some(status) = state.status.as_mut() {
                        status.axis_command_known = false;
                        status.fault_evidence =
                            Some(format!("MDT monitor failure {failures}: {e}"));
                    }
                    state.error = Some(e);
                    if terminal {
                        state.state = DriverState::Fault;
                    }
                    s.changed.notify_all();
                }
            }
            continue;
        }
        let next = urgent
            .try_recv()
            .map_err(|_| mpsc::RecvTimeoutError::Timeout)
            .or_else(|_| rx.recv_timeout(Duration::from_millis(1)));
        match next {
            Ok(job) => {
                // A queued job that never began owns no I/O or newer authority.
                if job.generation != s.generation.load(Ordering::Acquire) {
                    let _ = job.reply.send(Err(DriverError::Responsibility(
                        "MDT queued generation revoked; hold".into(),
                    )));
                    continue;
                }
                io.wrote = false;
                let result = (|| {
                    guard(s, job.generation)?;
                    let recovery = matches!(
                        job.op,
                        Op::Snapshot { recover: true }
                            | Op::EmergencyZero
                            | Op::Restore
                            | Op::Adopt(_)
                    );
                    if !recovery && s.state.lock().unwrap().state != DriverState::Ready {
                        return Err(DriverError::Closed);
                    }
                    if io.protocol.broken {
                        return Err(DriverError::Responsibility(
                            "MDT ambiguous serial stream; explicit close required".into(),
                        ));
                    }
                    io.wrote = false;
                    let status = match job.op {
                        Op::Snapshot { recover } => {
                            let st = io.snapshot(s, job.generation, job.deadline)?;
                            io.observe(s, st, recover)
                        }
                        Op::Adopt(a) => io.adopt(s, job.generation, job.deadline, a)?,
                        Op::Setting(setting) => {
                            super::settings::run(s, io, job.generation, job.deadline, setting)?
                        }
                        Op::Motion(motion) => {
                            super::motion::run(s, io, job.generation, job.deadline, motion)?
                        }
                        Op::EmergencyZero => {
                            super::motion::emergency(s, io, job.generation, job.deadline)?
                        }
                        Op::Restore => {
                            super::settings::restore(s, io, job.generation, job.deadline)?
                        }
                    };
                    guard(s, job.generation)?;
                    let mut state = s.state.lock().unwrap();
                    guard(s, job.generation)?;
                    state.state = if matches!(status.fault_evidence.as_deref(),Some(e) if e.starts_with("MDT restore limit"))
                    {
                        DriverState::Fault
                    } else {
                        DriverState::Ready
                    };
                    drop(state);
                    publish(s, io, status.clone());
                    Ok(status)
                })();
                if let Err(e) = &result {
                    let invalid_admission =
                        matches!(e, DriverError::Invalid(_) | DriverError::Busy(_)) && !io.wrote;
                    if invalid_admission {
                        let _ = job.reply.send(result);
                        continue;
                    }
                    io.invalidate(&e.to_string());
                    let mut state = s.state.lock().unwrap();
                    if job.generation == s.generation.load(Ordering::Acquire)
                        && !s.closing.load(Ordering::Acquire)
                    {
                        state.state = DriverState::Fault;
                    }
                    state.error = Some(e.clone());
                    if let Some(status) = state.status.as_mut() {
                        status.axis_command_known = false;
                        status.baseline_evidence = None;
                        status.fault_evidence = Some(e.to_string());
                    }
                }
                let _ = job.reply.send(result);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                s.closing.store(true, Ordering::Release);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}
fn cleanup_report(released: bool, error: Option<DriverError>) -> DriverResult<CleanupReport> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    CleanupReport::new(
        format!("{t:016x}{:016x}", SEQUENCE.fetch_add(1, Ordering::Relaxed)),
        vec![CleanupStep {
            role: "mdt693b".into(),
            action: "stop_and_hold_release".into(),
            error: error.map(|e| e.to_string()),
        }],
        None,
        if released {
            vec![]
        } else {
            vec!["mdt693b".into()]
        },
    )
}
impl DriverLifecycle for Mdt693b {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        Mdt693b::close(self)
    }
    fn has_responsibility(&self) -> bool {
        self.has_resource_responsibility()
    }
}
fn retained() -> &'static Mutex<Vec<Mdt693b>> {
    static STORE: OnceLock<Mutex<Vec<Mdt693b>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(vec![]))
}
pub fn retry_retained() -> Vec<DriverResult<CleanupReport>> {
    let pending = std::mem::take(&mut *retained().lock().unwrap_or_else(|e| e.into_inner()));
    let mut reports = vec![];
    for mut d in pending {
        reports.push(d.close());
        if d.has_resource_responsibility() {
            retained().lock().unwrap_or_else(|e| e.into_inner()).push(d);
        }
    }
    reports
}
impl Drop for Mdt693b {
    fn drop(&mut self) {
        if self.retain_on_drop && self.has_resource_responsibility() {
            self.stop_handle().request_stop();
            self.shared.closing.store(true, Ordering::Release);
            retained()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Self {
                    shared: self.shared.clone(),
                    book: self.book.clone(),
                    backend: self.backend.clone(),
                    normal: self.normal.take(),
                    priority: self.priority.take(),
                    actor: self.actor.take(),
                    stranded: self.stranded.clone(),
                    last_cleanup: self.last_cleanup.take(),
                    retain_on_drop: false,
                });
        }
    }
}
#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::{
        clock::ManualClock,
        transport::{serial_abi::SerialIo, serial_discovery::DeviceRecord, CloseReport},
    };
    struct Endpoint;
    impl SerialIo for Endpoint {
        fn configure(&mut self, _: &SerialConfig) -> DriverResult<()> {
            Ok(())
        }
        fn read(&mut self, _: usize, _: Deadline) -> DriverResult<Vec<u8>> {
            panic!("metadata publication must not read")
        }
        fn write(&mut self, _: &[u8], _: Deadline) -> DriverResult<usize> {
            panic!("metadata publication must not write")
        }
        fn close(&mut self) -> DriverResult<CloseReport> {
            Ok(CloseReport {
                released: true,
                status: Some(0),
            })
        }
        fn has_pending(&self) -> bool {
            false
        }
    }
    struct Backend;
    impl SerialBackend for Backend {
        fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
            panic!("no enumeration")
        }
        fn open(&self, _: &str) -> DriverResult<Box<dyn SerialIo>> {
            Ok(Box::new(Endpoint))
        }
    }
    #[test]
    fn late_publication_cannot_rearm_after_metadata_stop_or_replace_closing_state() {
        let backend: Arc<dyn SerialBackend> = Arc::new(Backend);
        let book = ResourceBook::isolated();
        let driver = Mdt693b::with_backend(
            MdtConfig {
                port: "COM16".into(),
                ..MdtConfig::default()
            },
            book.clone(),
            Arc::new(ManualClock::default()),
            backend.clone(),
        )
        .unwrap();
        let session =
            SerialSession::from_backend(SerialConfig::instrument("COM16"), &book, backend).unwrap();
        let stale = MdtStatus {
            product: "MDT693B".into(),
            firmware: "1.23".into(),
            serial_number: "test".into(),
            friendly_name: "".into(),
            echo_enabled: false,
            hardware_limit: VoltageLimit::V75,
            display_intensity: 7,
            master_scan_enabled: false,
            master_scan_voltage_v: 0.,
            axes: BTreeMap::new(),
            dac_step: 1,
            compatibility_enabled: false,
            rotary_mode: RotaryMode::Fine,
            push_to_adjust_disabled: true,
            supported_commands: BTreeSet::new(),
            restricted: false,
            fault_evidence: None,
            observed_at: 0.,
            axis_command_known: true,
            baseline_evidence: Some("operator".into()),
            selected_channel: None,
        };
        let mut io = Io {
            protocol: Protocol::new(session),
            status: None,
            authority: Some(super::super::authority::Authority {
                commands: [0.; 3],
                actual: [0.; 3],
                master: 0.,
                enabled: false,
                generation: 0,
                evidence: "operator".into(),
            }),
            wrote: false,
            last_motion: None,
            unsupported_arrows: BTreeSet::new(),
        };
        driver.stop_handle().request_stop();
        driver.shared.closing.store(true, Ordering::Release);
        driver.shared.state.lock().unwrap().state = DriverState::Closing;
        publish(&driver.shared, &mut io, stale);
        assert!(!driver.status().unwrap().axis_command_known);
        assert_eq!(driver.state(), DriverState::Closing);
        assert!(driver.status().unwrap().baseline_evidence.is_none());
        io.protocol.session.close().unwrap();
    }
}
