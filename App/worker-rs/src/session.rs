use crate::{observations::Observation, WorkerError};
use serde_json::Value;
use std::sync::Arc;
use yang_drivers::{
    lifecycle::{CleanupReport, DriverLifecycle, DriverState, ProbeReport},
    osa::TraceCapture,
    DriverResult,
};
use yang_protocol::{ContextV3, DomainConfig, OutcomeV3};
pub trait StopSignal: Send + Sync {
    fn request_stop(&self);
}
pub trait DeviceSession: DriverLifecycle + Send {
    fn connect(&mut self) -> DriverResult<ProbeReport>;
    fn state(&self) -> DriverState;
    fn identity(&self) -> Value;
    fn stop_signal(&self) -> Arc<dyn StopSignal>;
    fn action(&mut self, name: &str, args: &Value, context: &ContextV3) -> OutcomeV3;
    fn observe(&mut self, context: &ContextV3) -> Observation;
    fn take_capture(&mut self) -> Option<TraceCapture> {
        None
    }
}
pub trait DriverFactory: Send + Sync {
    fn create(&self, config: &DomainConfig) -> Result<Box<dyn DeviceSession>, WorkerError>;
    fn auxiliary_responsibility(&self) -> bool {
        false
    }
    fn finish_shutdown(&self) -> DriverResult<CleanupReport> {
        CleanupReport::new(
            crate::new_id()
                .map_err(|e| yang_drivers::DriverError::Responsibility(e.to_string()))?,
            vec![yang_drivers::lifecycle::CleanupStep {
                role: "factory".into(),
                action: "no_auxiliary_handles".into(),
                error: None,
            }],
            None,
            vec![],
        )
    }
}
pub struct InstrumentSession {
    pub(crate) driver: Box<dyn DeviceSession>,
    report: Option<CleanupReport>,
    stopped: bool,
}
impl InstrumentSession {
    pub fn new(driver: Box<dyn DeviceSession>) -> Self {
        Self {
            driver,
            report: None,
            stopped: false,
        }
    }
    pub fn request_stop(&self) {
        self.driver.stop_signal().request_stop();
    }
    pub fn check_health(&self) -> Result<DriverState, WorkerError> {
        let state = self.driver.state();
        if self.stopped || !matches!(state, DriverState::Ready | DriverState::Active) {
            Err(WorkerError::new(
                "Unhealthy",
                "session is stopped, faulted or not ready",
            ))
        } else {
            Ok(state)
        }
    }
    pub fn connect(&mut self) -> DriverResult<ProbeReport> {
        if self.stopped {
            return Err(yang_drivers::DriverError::Responsibility(
                "Stopped session cannot reconnect".into(),
            ));
        }
        self.driver.connect()
    }
    pub fn cleanup_report(&self) -> Option<&CleanupReport> {
        self.report.as_ref()
    }
}
impl DriverLifecycle for InstrumentSession {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        self.stopped = true;
        self.request_stop();
        if !self.driver.has_responsibility() {
            if let Some(report) = &self.report {
                return Ok(report.clone());
            }
        }
        let report = self.driver.close()?;
        self.report = Some(report.clone());
        Ok(report)
    }
    fn has_responsibility(&self) -> bool {
        self.driver.has_responsibility()
    }
}

/// The production factory never selects an environment or diagnostic transport.
/// VISA is loaded lazily by an explicitly admitted OSA connection/probe only.
pub struct SystemFactory {
    clock: Arc<dyn yang_drivers::clock::Clock>,
    manager: std::sync::Mutex<Option<yang_drivers::transport::VisaManager>>,
}
impl SystemFactory {
    pub fn new(clock: Arc<dyn yang_drivers::clock::Clock>) -> Self {
        Self {
            clock,
            manager: std::sync::Mutex::new(None),
        }
    }
}
impl DriverFactory for SystemFactory {
    fn auxiliary_responsibility(&self) -> bool {
        self.manager.lock().unwrap().is_some()
            || yang_drivers::transport::visa::retained_count() != 0
    }
    fn create(&self, config: &DomainConfig) -> Result<Box<dyn DeviceSession>, WorkerError> {
        let config = crate::catalog::admit(config)?;
        if config.driver_kind != "osa" {
            return Err(WorkerError::new(
                "UnsupportedDriver",
                "Native driver is not ported yet",
            ));
        }
        let mut manager = self.manager.lock().unwrap();
        if manager.is_none() {
            *manager = Some(yang_drivers::transport::VisaManager::load_system(
                yang_drivers::transport::ResourceBook::default(),
            )?);
        }
        let resource = config.params["resource"]
            .as_str()
            .ok_or_else(|| WorkerError::new("InvalidResource", "OSA VISA resource required"))?;
        let timeout = config.params["timeout_s"].as_f64().unwrap_or(30.);
        let osa = yang_drivers::osa::Osa::with_options(
            manager.as_ref().unwrap().clone(),
            resource.into(),
            self.clock.clone(),
            yang_drivers::osa::OsaOptions {
                timeout: std::time::Duration::from_secs_f64(timeout),
                close_timeout: std::time::Duration::from_secs(2),
            },
        )?;
        Ok(Box::new(OsaSession::new(osa)))
    }
    fn finish_shutdown(&self) -> DriverResult<CleanupReport> {
        let mut manager = self.manager.lock().unwrap();
        let mut error = None;
        let mut unreleased = Vec::new();
        let retained = yang_drivers::transport::visa::retry_retained();
        if retained != 0 {
            error = Some("Native VISA auxiliary handles remain retained".into());
            unreleased.push("visa_auxiliary".into());
        }
        if let Some(pool) = manager.take() {
            if let Err(failure) = pool.release() {
                error = Some(failure.error.to_string());
                unreleased.push("visa_manager".into());
                *manager = Some(failure.manager);
            }
        }
        CleanupReport::new(
            crate::new_id()
                .map_err(|e| yang_drivers::DriverError::Responsibility(e.to_string()))?,
            vec![yang_drivers::lifecycle::CleanupStep {
                role: "visa_manager".into(),
                action: "release_pool".into(),
                error,
            }],
            None,
            unreleased,
        )
    }
}
struct OsaStop {
    requested: std::sync::atomic::AtomicBool,
    handle: std::sync::Mutex<yang_drivers::osa::StopHandle>,
}
impl StopSignal for OsaStop {
    fn request_stop(&self) {
        self.requested
            .store(true, std::sync::atomic::Ordering::Release);
        self.handle.lock().unwrap().request_stop();
    }
}
pub struct OsaSession {
    osa: yang_drivers::osa::Osa,
    stop: Arc<OsaStop>,
    capture: Option<TraceCapture>,
}
impl OsaSession {
    pub fn new(osa: yang_drivers::osa::Osa) -> Self {
        let stop = Arc::new(OsaStop {
            requested: std::sync::atomic::AtomicBool::new(false),
            handle: std::sync::Mutex::new(osa.stop_handle()),
        });
        Self {
            osa,
            stop,
            capture: None,
        }
    }
}
impl DriverLifecycle for OsaSession {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        self.stop.request_stop();
        self.osa.close()
    }
    fn has_responsibility(&self) -> bool {
        self.osa.has_resource_responsibility()
    }
}
impl DeviceSession for OsaSession {
    fn connect(&mut self) -> DriverResult<ProbeReport> {
        if self
            .stop
            .requested
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(yang_drivers::DriverError::Responsibility(
                "OSA connection canceled before call".into(),
            ));
        }
        let result = self.osa.connect();
        // A held identity query may finish after cancellation; bind its new
        // driver-generation stop handle before allowing any later transaction.
        let handle = self.osa.stop_handle();
        if self
            .stop
            .requested
            .load(std::sync::atomic::Ordering::Acquire)
        {
            handle.request_stop();
        }
        *self.stop.handle.lock().unwrap() = handle;
        if self
            .stop
            .requested
            .load(std::sync::atomic::Ordering::Acquire)
        {
            Err(yang_drivers::DriverError::Responsibility(
                "OSA connection completed after stop intent; close required".into(),
            ))
        } else {
            result
        }
    }
    fn state(&self) -> DriverState {
        self.osa.state()
    }
    fn identity(&self) -> Value {
        let fields: Vec<_> = self.osa.identity().split(',').map(str::trim).collect();
        if fields.len() == 4 {
            serde_json::json!({"manufacturer":fields[0],"model":fields[1],"serial":fields[2],"firmware":fields[3]})
        } else {
            serde_json::json!({})
        }
    }
    fn stop_signal(&self) -> Arc<dyn StopSignal> {
        self.stop.clone()
    }
    fn action(&mut self, name: &str, args: &Value, context: &ContextV3) -> OutcomeV3 {
        if let Err(error) = crate::backend::validate_action("osa", name, args) {
            return crate::backend::failed(
                Some(context.clone()),
                yang_protocol::Phase::RejectedBeforeCall,
                error,
            );
        }
        let trace = args["trace"]
            .as_str()
            .unwrap_or("A")
            .parse()
            .expect("validated trace");
        let deadline = yang_drivers::transport::Deadline::after(self.osa.options().timeout);
        let capture = if name == "acquire" {
            self.osa.acquire_trace(trace, deadline)
        } else {
            self.osa.read_trace(trace, deadline)
        };
        match capture {
            Ok(capture) => {
                let timing = serde_json::json!({"io_s":capture.timing().io.as_secs_f64(),"decode_s":capture.timing().decode.as_secs_f64(),"read_elapsed_s":capture.timing().elapsed.as_secs_f64()});
                self.capture = Some(capture);
                crate::backend::completed(
                    Some(context.clone()),
                    serde_json::json!({"hardware_read_completed":true,"timing":timing,"status":{"state":"READY","identity":self.identity()}}),
                )
            }
            Err(error) => crate::backend::failed(
                Some(context.clone()),
                yang_protocol::Phase::FailedAfterCallStarted,
                error.into(),
            ),
        }
    }
    fn observe(&mut self, _: &ContextV3) -> Observation {
        Observation {
            status: serde_json::json!({"state":self.osa.state(),
                "connected":matches!(self.osa.state(),DriverState::Ready|DriverState::Active),
                "resource":self.osa.resource_name(),"identity":self.osa.identity(),"cached":true}),
            more: false,
            sampled_at: None,
        }
    }
    fn take_capture(&mut self) -> Option<TraceCapture> {
        self.capture.take()
    }
}
