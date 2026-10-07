use super::{InstrumentInfo, SensorInfo, SystemError};
use crate::{
    clock::Clock,
    lifecycle::{CleanupReport, CleanupStep, DriverLifecycle, DriverState, ProbeReport},
    transport::{Deadline, VisaManager, VisaSession},
    DriverError, DriverResult,
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    thread::JoinHandle,
    time::{Duration, SystemTime},
};
#[derive(Clone)]
pub struct PmOptions {
    pub timeout: Duration,
    pub close_timeout: Duration,
}
impl Default for PmOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            close_timeout: Duration::from_secs(2),
        }
    }
}
#[derive(Clone)]
pub struct StopHandle {
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}
impl StopHandle {
    fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
    /// Metadata only: no native I/O or session mutex is touched.
    pub fn cancel_measurement(&self) {
        if self.active.load(Ordering::Acquire) {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.cancel_measurement();
    }
}
struct CloseResult {
    session: Option<VisaSession>,
    step: CleanupStep,
    error: Option<DriverError>,
}
struct CloseJob {
    slot: Arc<(Mutex<Option<CloseResult>>, Condvar)>,
    thread: JoinHandle<()>,
}
pub struct Pm400 {
    manager: VisaManager,
    resource: String,
    pub(crate) clock: Arc<dyn Clock>,
    options: PmOptions,
    session: Option<VisaSession>,
    state: DriverState,
    info: Option<InstrumentInfo>,
    sensor: Option<SensorInfo>,
    stop: StopHandle,
    measurement_generation: Option<u64>,
    job: Option<CloseJob>,
    cleanup_error: Option<DriverError>,
    last_cleanup: Option<CleanupReport>,
    pub(crate) preexisting: Vec<SystemError>,
    retain_on_drop: bool,
}
impl Pm400 {
    pub fn new(
        manager: VisaManager,
        resource: String,
        clock: Arc<dyn Clock>,
    ) -> DriverResult<Self> {
        Self::with_options(manager, resource, clock, PmOptions::default())
    }
    pub fn with_options(
        manager: VisaManager,
        resource: String,
        clock: Arc<dyn Clock>,
        options: PmOptions,
    ) -> DriverResult<Self> {
        if resource.trim().is_empty()
            || resource.len() >= 256
            || !resource.is_ascii()
            || resource.bytes().any(|b| b < 32 || b == 127)
            || options.timeout.is_zero()
            || options.close_timeout.is_zero()
            || options.timeout > Duration::from_secs(180)
            || options.close_timeout > Duration::from_secs(180)
        {
            return Err(DriverError::Invalid(
                "invalid PM resource or finite timeouts".into(),
            ));
        }
        Ok(Self {
            manager,
            resource,
            clock,
            options,
            session: None,
            state: DriverState::Disconnected,
            info: None,
            sensor: None,
            stop: StopHandle::new(),
            measurement_generation: None,
            job: None,
            cleanup_error: None,
            last_cleanup: None,
            preexisting: vec![],
            retain_on_drop: true,
        })
    }
    pub fn connect(&mut self) -> DriverResult<ProbeReport> {
        if self.state == DriverState::Ready {
            self.guard()?;
            return self.report(false);
        }
        if self.has_resource_responsibility() || self.state != DriverState::Disconnected {
            return Err(DriverError::Responsibility(
                "close PM session before reconnect".into(),
            ));
        }
        self.state = DriverState::Connecting;
        self.stop = StopHandle::new();
        self.info = None;
        self.sensor = None;
        self.cleanup_error = None;
        let result = (|| {
            let canonical = self.manager.canonicalize(&self.resource)?;
            self.session = Some(match self.manager.open_owned(canonical, self.deadline()) {
                Ok(s) => s,
                Err(f) => {
                    self.session = f.session;
                    return Err(f.error);
                }
            });
            self.session.as_mut().unwrap().disable_termination()?;
            let info = self.identify()?;
            info.validate()?;
            self.info = Some(info);
            self.sensor = Some(SensorInfo::parse(
                &self.query("SYSTem:SENSor:IDN?", self.deadline())?,
            )?);
            self.state = DriverState::Ready;
            self.report(false)
        })();
        if result.is_err() {
            self.state = DriverState::Fault;
            let _ = self.close();
        }
        result
    }
    fn report(&self, released: bool) -> DriverResult<ProbeReport> {
        let i = self.info.as_ref().ok_or(DriverError::Closed)?;
        Ok(ProbeReport::new(
            serde_json::json!({"manufacturer":i.manufacturer,"model":i.model,"serial":i.serial_number,"firmware":i.firmware,"resource":self.resource,"quality":"strong_identity"}),
            serde_json::json!({"sensor":self.sensor}),
            released,
        ))
    }
    pub fn probe_identity(&mut self) -> DriverResult<ProbeReport> {
        if self.has_resource_responsibility() || self.state != DriverState::Disconnected {
            return Err(DriverError::Busy(
                "PM probe needs disconnected session".into(),
            ));
        }
        let report = self.connect()?;
        let released = self.close()?.resources_released();
        Ok(ProbeReport::new(
            report.identity().clone(),
            report.observations().clone(),
            released,
        ))
    }
    pub fn identify(&mut self) -> DriverResult<InstrumentInfo> {
        InstrumentInfo::parse(&self.query("*IDN?", self.deadline())?)
    }
    pub fn instrument_info(&self) -> Option<&InstrumentInfo> {
        self.info.as_ref()
    }
    pub fn sensor_info(&self) -> Option<&SensorInfo> {
        self.sensor.as_ref()
    }
    pub fn state(&self) -> DriverState {
        self.state
    }
    pub fn resource_name(&self) -> &str {
        &self.resource
    }
    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }
    pub fn cancel_measurement(&self) {
        self.stop.cancel_measurement();
    }
    pub fn has_resource_responsibility(&self) -> bool {
        self.job.is_some()
            || self
                .session
                .as_ref()
                .is_some_and(VisaSession::has_responsibility)
    }
    pub fn cleanup_error(&self) -> Option<&DriverError> {
        self.cleanup_error.as_ref()
    }
    pub fn cleanup_report(&self) -> Option<&CleanupReport> {
        self.last_cleanup.as_ref()
    }
    pub fn last_preexisting_errors(&self) -> &[SystemError] {
        &self.preexisting
    }
    pub(crate) fn deadline(&self) -> Deadline {
        Deadline::after(self.options.timeout)
    }
    pub(crate) fn guard(&self) -> DriverResult<()> {
        if self.stop.stop.load(Ordering::Acquire)
            || self
                .measurement_generation
                .is_some_and(|g| g != self.stop.generation.load(Ordering::Acquire))
        {
            return Err(DriverError::Responsibility(
                "PM request canceled; close required".into(),
            ));
        }
        if !matches!(
            self.state,
            DriverState::Connecting | DriverState::Ready | DriverState::Active
        ) {
            return Err(DriverError::Closed);
        }
        Ok(())
    }
    pub(crate) fn query(&mut self, command: &str, deadline: Deadline) -> DriverResult<String> {
        self.guard()?;
        let deadline = deadline.earlier(self.deadline());
        let result = (|| {
            let s = self.session.as_mut().ok_or(DriverError::Closed)?;
            s.write_all(format!("{command}\n").as_bytes(), deadline)?;
            let bytes = s.read_reply(8192, deadline)?;
            deadline.remaining_millis()?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| DriverError::Protocol("PM response is not ASCII text".into()))?;
            if !text.is_ascii() {
                return Err(DriverError::Protocol("non-ASCII PM reply".into()));
            }
            let text = text.trim_end_matches(['\n', '\r']);
            if text.trim().is_empty() || text.contains(['\n', '\r']) {
                return Err(DriverError::Protocol(
                    "PM response must contain exactly one nonempty line".into(),
                ));
            }
            s.reject_pending_response()?;
            deadline.remaining_millis()?;
            Ok(text.to_string())
        })();
        if result.is_err() {
            self.state = DriverState::Fault;
        }
        result.and_then(|value| {
            self.guard()?;
            Ok(value)
        })
    }
    pub(crate) fn write(&mut self, command: &str, deadline: Deadline) -> DriverResult<()> {
        self.guard()?;
        let deadline = deadline.earlier(self.deadline());
        let r = self
            .session
            .as_mut()
            .ok_or(DriverError::Closed)?
            .write_all(format!("{command}\n").as_bytes(), deadline);
        if r.is_err() {
            self.state = DriverState::Fault;
        }
        r.and_then(|()| self.guard())
    }
    pub(crate) fn action(&mut self, command: &str, deadline: Deadline) -> DriverResult<()> {
        self.preexisting = self.drain_errors(deadline)?;
        self.write(command, deadline)?;
        let errors = self.drain_errors(deadline)?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(DriverError::Protocol(format!(
                "PM action reported errors: {errors:?}"
            )))
        }
    }
    pub(crate) fn drain_errors(&mut self, deadline: Deadline) -> DriverResult<Vec<SystemError>> {
        let mut errors = vec![];
        for _ in 0..128 {
            let e = SystemError::parse(&self.query("SYSTem:ERRor?", deadline)?)?;
            if e.code == 0 {
                return Ok(errors);
            }
            errors.push(e);
        }
        Err(DriverError::Protocol(
            "PM error queue did not terminate".into(),
        ))
    }
    pub(crate) fn supported(&self, kind: super::MeasurementKind) -> DriverResult<()> {
        let c = &self
            .sensor
            .as_ref()
            .ok_or(DriverError::Closed)?
            .capabilities;
        let legal = match kind {
            super::MeasurementKind::Power | super::MeasurementKind::PowerDensity => c.power,
            super::MeasurementKind::Energy | super::MeasurementKind::EnergyDensity => c.energy,
            super::MeasurementKind::Temperature => c.temperature_sensor,
            _ => true,
        };
        if legal {
            Ok(())
        } else {
            Err(DriverError::Invalid("sensor capability unavailable".into()))
        }
    }
    pub(crate) fn begin_measurement(&mut self) -> DriverResult<()> {
        self.guard()?;
        if self.state != DriverState::Ready {
            return Err(DriverError::Busy("PM measurement already active".into()));
        }
        self.measurement_generation = Some(self.stop.generation.load(Ordering::Acquire));
        self.state = DriverState::Active;
        self.stop.active.store(true, Ordering::Release);
        Ok(())
    }
    pub(crate) fn finish_measurement<T>(&mut self, result: DriverResult<T>) -> DriverResult<T> {
        let result = result.and_then(|value| {
            self.guard()?;
            Ok(value)
        });
        self.stop.active.store(false, Ordering::Release);
        self.measurement_generation = None;
        self.state = if result.is_ok() {
            DriverState::Ready
        } else {
            DriverState::Fault
        };
        result
    }
    pub fn close(&mut self) -> DriverResult<CleanupReport> {
        self.stop.request_stop();
        self.state = DriverState::Closing;
        let deadline = Deadline::after(self.options.close_timeout);
        let mut steps = vec![];
        if self.job.is_none() {
            if let Some(session) = self.session.take() {
                let input = Arc::new(Mutex::new(Some(session)));
                let slot = Arc::new((Mutex::new(None), Condvar::new()));
                let (source, result) = (input.clone(), slot.clone());
                let thread = std::thread::Builder::new()
                    .name("pm400-close".into())
                    .spawn(move || {
                        let mut session = source
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .take()
                            .unwrap();
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                session.close()
                            }))
                            .unwrap_or_else(|_| {
                                Err(DriverError::Responsibility(
                                    "PM native close panicked".into(),
                                ))
                            });
                        let error = match outcome {
                            Ok(r) if r.released => None,
                            Ok(r) => Some(DriverError::Native {
                                operation: "PM viClose".into(),
                                status: r.status.unwrap_or(-1),
                                transferred: 0,
                            }),
                            Err(e) => Some(e),
                        };
                        let step = CleanupStep {
                            role: "pm400".into(),
                            action: "release_transport".into(),
                            error: error.as_ref().map(ToString::to_string),
                        };
                        let session = session.has_responsibility().then_some(session);
                        *result.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(CloseResult {
                            session,
                            step,
                            error,
                        });
                        result.1.notify_all();
                    });
                match thread {
                    Ok(thread) => self.job = Some(CloseJob { slot, thread }),
                    Err(e) => {
                        self.session = input.lock().unwrap_or_else(|e| e.into_inner()).take();
                        self.cleanup_error = Some(DriverError::Responsibility(format!(
                            "PM cleanup launch failed: {e}"
                        )));
                    }
                }
            }
        }
        if let Some(job) = &self.job {
            let mut slot = job.slot.0.lock().unwrap_or_else(|e| e.into_inner());
            while slot.is_none() || !job.thread.is_finished() {
                let Ok(ms) = deadline.remaining_millis() else {
                    break;
                };
                let wait =
                    Duration::from_millis(u64::from(if slot.is_some() { ms.min(1) } else { ms }));
                slot = job
                    .slot
                    .1
                    .wait_timeout(slot, wait)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            if slot.is_some() && job.thread.is_finished() {
                let result = slot.take().unwrap();
                drop(slot);
                let job = self.job.take().unwrap();
                let _ = job.thread.join();
                self.session = result.session;
                self.cleanup_error = result.error;
                steps.push(result.step);
            } else {
                self.cleanup_error = Some(DriverError::Timeout {
                    operation: "PM cleanup still retained".into(),
                    transferred: 0,
                });
            }
        }
        if steps.is_empty() {
            steps.push(CleanupStep {
                role: "pm400".into(),
                action: if self.has_resource_responsibility() {
                    "await_cleanup"
                } else {
                    "already_released"
                }
                .into(),
                error: self.cleanup_error.as_ref().map(ToString::to_string),
            });
        }
        let pending = self.has_resource_responsibility();
        self.state = if pending {
            DriverState::Fault
        } else {
            DriverState::Disconnected
        };
        let report = CleanupReport::new(
            attempt_id(),
            steps,
            None,
            if pending {
                vec![self.resource.clone()]
            } else {
                vec![]
            },
        )?;
        self.last_cleanup = Some(report.clone());
        Ok(report)
    }
}
fn attempt_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    format!(
        "{time:016x}{:016x}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
impl DriverLifecycle for Pm400 {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        Pm400::close(self)
    }
    fn has_responsibility(&self) -> bool {
        self.has_resource_responsibility()
    }
}
fn retained() -> &'static Mutex<Vec<Pm400>> {
    static STORE: OnceLock<Mutex<Vec<Pm400>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(vec![]))
}
pub fn retry_retained() -> Vec<DriverResult<CleanupReport>> {
    let mut pending = std::mem::take(&mut *retained().lock().unwrap_or_else(|e| e.into_inner()));
    let mut results = vec![];
    for mut driver in pending.drain(..) {
        results.push(driver.close());
        if driver.has_resource_responsibility() {
            retained()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(driver);
        }
    }
    results
}
impl Drop for Pm400 {
    fn drop(&mut self) {
        if self.retain_on_drop && self.has_resource_responsibility() {
            self.stop.request_stop();
            retained()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Self {
                    manager: self.manager.clone(),
                    resource: self.resource.clone(),
                    clock: self.clock.clone(),
                    options: self.options.clone(),
                    session: self.session.take(),
                    state: DriverState::Closing,
                    info: self.info.take(),
                    sensor: self.sensor.take(),
                    stop: self.stop.clone(),
                    measurement_generation: self.measurement_generation,
                    job: self.job.take(),
                    cleanup_error: self.cleanup_error.take(),
                    last_cleanup: self.last_cleanup.take(),
                    preexisting: std::mem::take(&mut self.preexisting),
                    retain_on_drop: false,
                });
        }
    }
}
