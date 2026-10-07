use super::{Spectrum, TraceId};
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
pub struct OsaOptions {
    pub timeout: Duration,
    pub close_timeout: Duration,
}
impl Default for OsaOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            close_timeout: Duration::from_secs(2),
        }
    }
}
#[derive(Clone)]
pub struct StopHandle(Arc<AtomicBool>);
impl StopHandle {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
    pub fn request_stop(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub(crate) fn check(&self) -> DriverResult<()> {
        if self.0.load(Ordering::Acquire) {
            Err(DriverError::Responsibility(
                "OSA operation canceled; explicit close required".into(),
            ))
        } else {
            Ok(())
        }
    }
}
struct CleanupResult {
    session: Option<VisaSession>,
    owned: bool,
    steps: Vec<CleanupStep>,
    error: Option<DriverError>,
}
struct CleanupJob {
    result: Arc<(Mutex<Option<CleanupResult>>, Condvar)>,
    handle: JoinHandle<()>,
}
pub struct Osa {
    pub(crate) manager: VisaManager,
    resource: String,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) io_elapsed: Duration,
    pub(crate) options: OsaOptions,
    pub(crate) session: Option<VisaSession>,
    pub(crate) state: DriverState,
    pub(crate) identity: String,
    pub(crate) sweep_owned: bool,
    pub(crate) stop: StopHandle,
    job: Option<CleanupJob>,
    pub(crate) cleanup_error: Option<DriverError>,
    last_cleanup: Option<CleanupReport>,
    retain_on_drop: bool,
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
fn step(action: &str, error: Option<&DriverError>) -> CleanupStep {
    CleanupStep {
        role: "osa".into(),
        action: action.into(),
        error: error.map(ToString::to_string),
    }
}
impl Osa {
    pub fn new(
        manager: VisaManager,
        resource: String,
        clock: Arc<dyn Clock>,
    ) -> DriverResult<Self> {
        Self::with_options(manager, resource, clock, OsaOptions::default())
    }
    pub fn with_options(
        manager: VisaManager,
        resource: String,
        clock: Arc<dyn Clock>,
        options: OsaOptions,
    ) -> DriverResult<Self> {
        if resource.is_empty()
            || resource.len() >= 256
            || !resource.is_ascii()
            || resource.bytes().any(|b| b < 32 || b == 127)
            || options.timeout.is_zero()
            || options.close_timeout.is_zero()
            || options.timeout > Duration::from_secs(180)
            || options.close_timeout > Duration::from_secs(180)
        {
            return Err(DriverError::Invalid(
                "Invalid OSA resource or finite timeout budget".into(),
            ));
        }
        Ok(Self {
            manager,
            resource,
            clock,
            io_elapsed: Duration::ZERO,
            options,
            session: None,
            state: DriverState::Disconnected,
            identity: String::new(),
            sweep_owned: false,
            stop: StopHandle::new(),
            job: None,
            cleanup_error: None,
            last_cleanup: None,
            retain_on_drop: true,
        })
    }
    pub fn connect(&mut self) -> DriverResult<ProbeReport> {
        if self.state == DriverState::Ready {
            self.stop.check()?;
            return Ok(ProbeReport::new(
                self.identity_fields()?,
                serde_json::json!({}),
                false,
            ));
        }
        if self.has_resource_responsibility() || self.state != DriverState::Disconnected {
            return Err(DriverError::Responsibility(
                "Close the old OSA attempt before reconnecting".into(),
            ));
        }
        self.stop = StopHandle::new();
        self.state = DriverState::Connecting;
        let result = (|| {
            let resource = self.manager.canonicalize(&self.resource)?;
            let deadline = Deadline::after(self.options.timeout);
            match self.manager.open_owned(resource, deadline) {
                Ok(session) => self.session = Some(session),
                Err(failure) => {
                    self.session = failure.session;
                    return Err(failure.error);
                }
            }
            self.stop.check()?;
            self.session.as_mut().unwrap().disable_termination()?;
            let identity = self.query_text("*IDN?", deadline, 1024)?;
            self.identity = identity;
            let fields = self.identity_fields()?;
            self.stop.check()?;
            self.state = DriverState::Ready;
            Ok(ProbeReport::new(fields, serde_json::json!({}), false))
        })();
        if let Err(error) = &result {
            self.state = DriverState::Fault;
            self.cleanup_error = Some(error.clone());
        }
        result
    }
    pub fn probe_identity(&mut self) -> DriverResult<ProbeReport> {
        if self.has_resource_responsibility() || self.state != DriverState::Disconnected {
            return Err(DriverError::Responsibility(
                "Identity probe requires an unowned OSA".into(),
            ));
        }
        let report = self.connect();
        let cleanup = self.close()?;
        let report = report?;
        if !cleanup.resources_released() {
            return Err(DriverError::Responsibility(
                "Identity probe cleanup is retained".into(),
            ));
        }
        Ok(ProbeReport::new(
            report.identity().clone(),
            report.observations().clone(),
            true,
        ))
    }
    pub(crate) fn identity_fields(&self) -> DriverResult<serde_json::Value> {
        let parts: Vec<_> = self.identity.split(',').map(str::trim).collect();
        if parts.len() != 4
            || parts.iter().any(|p| p.is_empty())
            || !parts[1].to_ascii_uppercase().starts_with("AQ6370")
        {
            return Err(DriverError::Protocol(
                "OSA identity must have four fields and an AQ6370 model".into(),
            ));
        }
        Ok(
            serde_json::json!({"manufacturer":parts[0],"model":parts[1],"serial":parts[2],"firmware":parts[3]}),
        )
    }
    pub(crate) fn query(
        &mut self,
        command: &str,
        deadline: Deadline,
        maximum: usize,
    ) -> DriverResult<Vec<u8>> {
        let started = std::time::Instant::now();
        let result = self.query_inner(command, deadline, maximum);
        self.io_elapsed += started.elapsed();
        result
    }
    fn query_inner(
        &mut self,
        command: &str,
        deadline: Deadline,
        maximum: usize,
    ) -> DriverResult<Vec<u8>> {
        self.stop.check()?;
        deadline.remaining_millis()?;
        let session = self.session.as_mut().ok_or(DriverError::Closed)?;
        session.write_all(format!("{command}\n").as_bytes(), deadline)?;
        self.stop.check()?;
        let result = session.read_reply(maximum, deadline)?;
        self.stop.check()?;
        deadline.remaining_millis()?;
        Ok(result)
    }
    pub(crate) fn query_text(
        &mut self,
        command: &str,
        deadline: Deadline,
        maximum: usize,
    ) -> DriverResult<String> {
        let reply = self.query(command, deadline, maximum)?;
        if !reply.is_ascii() {
            return Err(DriverError::Protocol("OSA metadata is not ASCII".into()));
        }
        let text = std::str::from_utf8(&reply).unwrap().trim();
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(DriverError::Protocol("Malformed OSA metadata text".into()));
        }
        Ok(text.into())
    }
    pub(crate) fn begin_operation(&mut self, deadline: Deadline) -> DriverResult<()> {
        deadline.remaining_millis()?;
        self.stop.check()?;
        if self.state != DriverState::Ready {
            return Err(DriverError::Responsibility(
                "OSA must be READY for this transaction".into(),
            ));
        }
        self.state = DriverState::Active;
        Ok(())
    }
    pub(crate) fn finish_operation<T>(&mut self, result: DriverResult<T>) -> DriverResult<T> {
        self.state = if result.is_ok() {
            DriverState::Ready
        } else {
            DriverState::Fault
        };
        if let Err(error) = &result {
            self.cleanup_error = Some(error.clone());
        }
        result
    }
    pub fn acquire(&mut self, trace: TraceId, deadline: Deadline) -> DriverResult<Spectrum> {
        self.acquire_trace(trace, deadline)?.spectrum()
    }
    fn start_cleanup(&mut self, deadline: Deadline) -> DriverResult<()> {
        let Some(session) = self.session.take() else {
            return Ok(());
        };
        let source = Arc::new(Mutex::new(Some(session)));
        let slot = Arc::new((Mutex::new(None), Condvar::new()));
        let (input, output) = (source.clone(), slot.clone());
        let owned = self.sweep_owned;
        let handle = std::thread::Builder::new()
            .name("osa-cleanup".into())
            .spawn(move || {
                let mut session = input.lock().unwrap().take().unwrap();
                let mut owned = owned;
                let mut steps = vec![];
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> DriverResult<()> {
                        if owned {
                            let result = session.abort_owned_sweep(deadline);
                            steps.push(step("abort_owned_sweep", result.as_ref().err()));
                            result?;
                            owned = false;
                        }
                        let result = session.close();
                        match result {
                            Ok(report) if report.released => {
                                steps.push(step("release_transport", None));
                                Ok(())
                            }
                            Ok(report) => {
                                let error = DriverError::Native {
                                    operation: "viClose".into(),
                                    status: report.status.unwrap_or(-1),
                                    transferred: 0,
                                };
                                steps.push(step("release_transport", Some(&error)));
                                Err(error)
                            }
                            Err(error) => {
                                steps.push(step("release_transport", Some(&error)));
                                Err(error)
                            }
                        }
                    },
                ))
                .unwrap_or_else(|_| {
                    Err(DriverError::Responsibility(
                        "OSA native cleanup panicked; retained".into(),
                    ))
                });
                let error = result.err();
                if steps.is_empty() {
                    steps.push(step("cleanup", error.as_ref()));
                }
                let session = session.has_responsibility().then_some(session);
                *output.0.lock().unwrap() = Some(CleanupResult {
                    session,
                    owned,
                    steps,
                    error,
                });
                output.1.notify_all();
            });
        match handle {
            Ok(handle) => {
                self.job = Some(CleanupJob {
                    result: slot,
                    handle,
                });
                Ok(())
            }
            Err(error) => {
                self.session = source.lock().unwrap().take();
                Err(DriverError::Responsibility(format!(
                    "OSA cleanup thread launch failed: {error}"
                )))
            }
        }
    }
    pub fn close(&mut self) -> DriverResult<CleanupReport> {
        self.stop.request_stop();
        self.state = DriverState::Closing;
        let deadline = Deadline::after(self.options.close_timeout);
        let mut steps = Vec::new();
        if self.job.is_none() {
            if let Err(error) = self.start_cleanup(deadline) {
                self.cleanup_error = Some(error.clone());
                steps.push(step("launch_cleanup", Some(&error)));
            }
        }
        if let Some(job) = &self.job {
            let mut result = job.result.0.lock().unwrap_or_else(|e| e.into_inner());
            while result.is_none() || !job.handle.is_finished() {
                let Ok(millis) = deadline.remaining_millis() else {
                    break;
                };
                let wait = Duration::from_millis(u64::from(if result.is_some() {
                    millis.min(1)
                } else {
                    millis
                }));
                result = job
                    .result
                    .1
                    .wait_timeout(result, wait)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            if result.is_some() && job.handle.is_finished() {
                let completed = result.take().unwrap();
                drop(result);
                let job = self.job.take().unwrap();
                if job.handle.join().is_err() {
                    self.cleanup_error = Some(DriverError::Responsibility(
                        "OSA cleanup thread panicked".into(),
                    ));
                } else {
                    self.cleanup_error = completed.error;
                }
                self.session = completed.session;
                self.sweep_owned = completed.owned;
                steps = completed.steps;
            } else {
                self.cleanup_error = Some(DriverError::Timeout {
                    operation: "OSA cleanup; native call still retained".into(),
                    transferred: 0,
                });
                steps.push(step("await_cleanup", self.cleanup_error.as_ref()));
            }
        }
        if steps.is_empty() {
            steps.push(step("already_released", None));
        }
        let pending = self.has_resource_responsibility();
        let report = CleanupReport::new(
            attempt_id(),
            steps,
            None,
            if pending { vec!["osa".into()] } else { vec![] },
        )?;
        self.state = if pending {
            DriverState::Fault
        } else {
            DriverState::Disconnected
        };
        if !pending {
            self.cleanup_error = None;
        }
        self.last_cleanup = Some(report.clone());
        Ok(report)
    }
    pub fn state(&self) -> DriverState {
        self.state
    }
    pub fn has_resource_responsibility(&self) -> bool {
        self.job.is_some()
            || self.sweep_owned
            || self
                .session
                .as_ref()
                .is_some_and(VisaSession::has_responsibility)
    }
    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn resource_name(&self) -> &str {
        &self.resource
    }
    pub fn options(&self) -> &OsaOptions {
        &self.options
    }
    pub fn cleanup_error(&self) -> Option<&DriverError> {
        self.cleanup_error.as_ref()
    }
    pub fn cleanup_report(&self) -> Option<&CleanupReport> {
        self.last_cleanup.as_ref()
    }
}
impl DriverLifecycle for Osa {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        Osa::close(self)
    }
    fn has_responsibility(&self) -> bool {
        self.has_resource_responsibility()
    }
}
type RetainedSlot = Arc<Mutex<Option<Osa>>>;
fn retained() -> &'static Mutex<Vec<RetainedSlot>> {
    static RETAINED: OnceLock<Mutex<Vec<RetainedSlot>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(vec![]))
}
pub fn retry_retained() -> Vec<DriverResult<CleanupReport>> {
    let slots = retained().lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut reports = vec![];
    for slot in slots {
        let mut driver = slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(osa) = driver.as_mut() {
            let report = osa.close();
            if report.as_ref().is_ok_and(CleanupReport::resources_released) {
                driver.take();
            }
            reports.push(report);
        }
    }
    retained()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|s| s.lock().unwrap_or_else(|e| e.into_inner()).is_some());
    reports
}
impl Drop for Osa {
    fn drop(&mut self) {
        if self.retain_on_drop && self.has_resource_responsibility() {
            let preserved = Self {
                manager: self.manager.clone(),
                resource: self.resource.clone(),
                clock: self.clock.clone(),
                io_elapsed: self.io_elapsed,
                options: self.options.clone(),
                session: self.session.take(),
                state: DriverState::Fault,
                identity: self.identity.clone(),
                sweep_owned: self.sweep_owned,
                stop: self.stop.clone(),
                job: self.job.take(),
                cleanup_error: self.cleanup_error.clone(),
                last_cleanup: self.last_cleanup.clone(),
                retain_on_drop: false,
            };
            retained()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(Arc::new(Mutex::new(Some(preserved))));
        }
    }
}
