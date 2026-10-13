//! Native Rigol session adapters. Observations never issue instrument I/O.
use crate::{
    actions::{Action, GeneratorAction, ScopeAction},
    observations::Observation,
    session::{CachedObserver, DeviceSession, StopSignal},
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
    generator::{Generator, GeneratorModel, GeneratorOptions},
    lifecycle::{CleanupReport, DriverLifecycle, DriverState, ProbeReport},
    scope::{Scope, ScopeModel, ScopeOptions, StopHandle, CAPTURE_TIMEOUT, DS7054_WINDOW_POINTS},
    transport::{Deadline, VisaManager},
    DriverError, DriverResult,
};
use yang_protocol::{ContextV3, DomainConfig, OutcomeV3, Phase};

enum Driver {
    Scope(Scope),
    Generator(Generator),
}
struct Stop {
    requested: AtomicBool,
    handle: Mutex<StopHandle>,
}
impl StopSignal for Stop {
    fn request_stop(&self) {
        self.requested.store(true, Ordering::Release);
        self.handle.lock().unwrap().request_stop();
    }
    fn cancel_operation(&self) {
        self.handle.lock().unwrap().cancel_operation();
    }
}
struct Cache {
    value: Value,
    sampled_at: Option<Duration>,
}
struct Observer(Arc<Mutex<Cache>>);
impl CachedObserver for Observer {
    fn observe(&self, _: &ContextV3) -> Observation {
        let c = self.0.lock().unwrap();
        Observation {
            status: c.value.clone(),
            sampled_at: c.sampled_at,
            more: false,
        }
    }
}
fn status_identity(mut value: Value, verified: &Value) -> DriverResult<Value> {
    let raw = value["identity"]
        .as_str()
        .ok_or_else(|| DriverError::Protocol("status IDN is absent".into()))?;
    let parts: Vec<_> = raw.split(',').map(str::trim).collect();
    if parts.len() != 4 || parts.iter().any(|part| part.is_empty()) {
        return Err(DriverError::Protocol("status IDN is malformed".into()));
    }
    let parsed =
        json!({"manufacturer":parts[0],"model":parts[1],"serial":parts[2],"firmware":parts[3]});
    if &parsed != verified {
        return Err(DriverError::Protocol(
            "status IDN differs from the verified connection identity".into(),
        ));
    }
    // Use the parsed status value, not a replacement cached identity. A mismatch
    // fails closed above; the literal connect-time IDN remains explicit evidence.
    value["identity_text"] = value["identity"].clone();
    value["identity"] = parsed;
    Ok(value)
}
pub struct RigolSession {
    driver: Driver,
    clock: Arc<dyn Clock>,
    stop: Arc<Stop>,
    cache: Arc<Mutex<Cache>>,
    waveform: Option<yang_data::WaveformFrame>,
    identity: Value,
}
impl RigolSession {
    pub fn new(
        config: &DomainConfig,
        manager: VisaManager,
        clock: Arc<dyn Clock>,
    ) -> DriverResult<Self> {
        let resource = config.params["resource"]
            .as_str()
            .ok_or_else(|| DriverError::Invalid("VISA resource required".into()))?;
        let timeout = Duration::from_secs_f64(config.params["timeout_s"].as_f64().unwrap_or(10.));
        match config.model_id.as_str() {
            "ds7054" | "mho5054" => Self::from_scope(
                Scope::new(
                    if config.model_id == "ds7054" {
                        ScopeModel::Ds7054
                    } else {
                        ScopeModel::Mho5054
                    },
                    manager,
                    resource,
                    clock.clone(),
                    ScopeOptions {
                        timeout,
                        // The instrument request window is independent from
                        // bounded native reads and Worker/Host payload chunks.
                        chunk_points: if config.model_id == "ds7054" {
                            DS7054_WINDOW_POINTS
                        } else {
                            ScopeOptions::default().chunk_points
                        },
                        ..Default::default()
                    },
                )?,
                clock,
            ),
            "dg4162" | "dg4202" | "dg922pro" => Self::from_generator(
                Generator::new(
                    match config.model_id.as_str() {
                        "dg4162" => GeneratorModel::Dg4162,
                        "dg4202" => GeneratorModel::Dg4202,
                        _ => GeneratorModel::Dg922Pro,
                    },
                    manager,
                    resource,
                    clock.clone(),
                    GeneratorOptions {
                        timeout,
                        ..Default::default()
                    },
                )?,
                clock,
            ),
            _ => Err(DriverError::Invalid("unreviewed Rigol model".into())),
        }
    }
    /// Explicit injection for hardware-free protocol/lifecycle tests, never a catalog backend.
    pub fn from_scope(driver: Scope, clock: Arc<dyn Clock>) -> DriverResult<Self> {
        Ok(Self::wrap(Driver::Scope(driver), clock))
    }
    pub fn from_generator(driver: Generator, clock: Arc<dyn Clock>) -> DriverResult<Self> {
        Ok(Self::wrap(Driver::Generator(driver), clock))
    }
    fn wrap(driver: Driver, clock: Arc<dyn Clock>) -> Self {
        let handle = match &driver {
            Driver::Scope(d) => d.stop_handle(),
            Driver::Generator(d) => d.stop_handle(),
        };
        Self {
            driver,
            clock,
            stop: Arc::new(Stop {
                requested: AtomicBool::new(false),
                handle: Mutex::new(handle),
            }),
            cache: Arc::new(Mutex::new(Cache {
                value: json!({"state":"DISCONNECTED","connected":false,"cached":true}),
                sampled_at: None,
            })),
            waveform: None,
            identity: json!({}),
        }
    }
    fn update(&self, status: Option<Value>, historical: bool) {
        let mut c = self.cache.lock().unwrap();
        if let Some(value) = status {
            c.value = value;
            c.sampled_at = Some(self.clock.now());
        }
        c.value["state"] = json!(self.state());
        c.value["connected"] = json!(
            self.state() == DriverState::Ready && !self.stop.requested.load(Ordering::Acquire)
        );
        c.value["cached"] = json!(true);
        c.value["previous"] = json!(historical);
        c.value["quality"] = json!(if historical {
            "historical"
        } else {
            "confirmed"
        });
    }
    fn deadline(&self) -> Deadline {
        Deadline::after(match &self.driver {
            Driver::Scope(d) => d.options().timeout,
            Driver::Generator(d) => d.options().timeout,
        })
    }
    fn run(&mut self, action: Action) -> DriverResult<(Option<Value>, bool)> {
        let deadline = if matches!(&action, Action::Scope(ScopeAction::Capture { .. })) {
            // A capture contains setup plus many bounded I/O calls. This is a
            // whole-operation ceiling, not an ETA or a longer stall timeout.
            Deadline::after(CAPTURE_TIMEOUT)
        } else {
            self.deadline()
        };
        match (&mut self.driver, action) {
            (Driver::Scope(d), Action::Scope(a)) => match a {
                ScopeAction::Status => Ok((
                    Some(status_identity(
                        serde_json::to_value(d.read_status(deadline)?).unwrap(),
                        &self.identity,
                    )?),
                    false,
                )),
                ScopeAction::Display { channel, enabled } => {
                    d.set_channel_display(channel, enabled, deadline)?;
                    Ok((None, false))
                }
                ScopeAction::Depth(points) => {
                    // Depth confirmation may span several individually bounded
                    // I/O calls; use the driver's whole-transition budget.
                    let depth_deadline = Deadline::after(d.options().depth_timeout);
                    d.set_memory_depth(points, depth_deadline)?;
                    // Depth application and its owned STOP restoration can
                    // exhaust their budget before the complete status read.
                    let status_deadline = Deadline::after(d.options().timeout);
                    Ok((
                        Some(status_identity(
                            serde_json::to_value(d.read_status(status_deadline)?).unwrap(),
                            &self.identity,
                        )?),
                        false,
                    ))
                }
                ScopeAction::Run => {
                    d.run(deadline)?;
                    Ok((None, false))
                }
                ScopeAction::Stop => {
                    d.stop(deadline)?;
                    d.stop_handle().clear_operation_cancel();
                    Ok((None, false))
                }
                ScopeAction::Capture { request, fresh, stop_before } => {
                    self.waveform = Some(if fresh {
                        d.capture_fresh(&request, deadline)?
                    } else if stop_before {
                        d.capture_snapshot(&request, deadline)?
                    } else {
                        d.capture(&request, deadline)?
                    });
                    Ok((None, true))
                }
            },
            (Driver::Generator(d), Action::Generator(a)) => match a {
                GeneratorAction::Status => Ok((
                    Some(status_identity(
                        serde_json::to_value(d.read_status(deadline)?).unwrap(),
                        &self.identity,
                    )?),
                    false,
                )),
                GeneratorAction::Configure { channel, signal } => {
                    d.configure_signal(channel, &signal, deadline)?;
                    Ok((None, false))
                }
                GeneratorAction::Output { channel, enabled } => {
                    d.set_output(channel, enabled, deadline)?;
                    if !enabled {
                        d.stop_handle().clear_operation_cancel();
                    }
                    Ok((None, false))
                }
                GeneratorAction::Phase { channel, degrees } => {
                    d.set_phase(channel, degrees, deadline)?;
                    Ok((None, false))
                }
                GeneratorAction::Load { channel, load } => {
                    d.set_load(channel, load, deadline)?;
                    Ok((None, false))
                }
                GeneratorAction::Polarity { channel, polarity } => {
                    d.set_polarity(channel, polarity, deadline)?;
                    Ok((None, false))
                }
            },
            _ => Err(DriverError::Invalid("action/session kind mismatch".into())),
        }
    }
}
#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn status_identity_is_parsed_and_mismatch_is_not_hidden() {
        let verified =
            json!({"manufacturer":"RIGOL","model":"DS7054","serial":"SCOPE1","firmware":"01"});
        let value =
            status_identity(json!({"identity":"RIGOL,DS7054,SCOPE1,01"}), &verified).unwrap();
        assert_eq!(value["identity"], verified);
        assert_eq!(value["identity_text"], "RIGOL,DS7054,SCOPE1,01");
        for idn in [
            "RIGOL,DS7054,OTHER,01",
            "RIGOL,DS7054,SCOPE1,02",
            "RIGOL,DS7054,SCOPE1",
            "RIGOL,DS7054,,01",
        ] {
            assert!(status_identity(json!({"identity":idn}), &verified).is_err());
        }
    }
}

#[cfg(test)]
mod capture_budget_tests {
    use super::*;
    use std::collections::VecDeque;
    use yang_data::Samples;
    use yang_drivers::{
        clock::ManualClock,
        scope::{CaptureRequest, CaptureSource},
        transport::{ByteTransport, CloseReport},
    };

    // Each actual I/O finishes within its stall timeout; their total exceeds it.
    // Keeping the real session/driver above this bounded wire catches confusing
    // the per-call timeout with the aggregate acquisition budget.
    struct SlowTranscript {
        exchanges: VecDeque<(&'static str, Vec<u8>)>,
        pending: VecDeque<u8>,
        live: bool,
    }
    impl SlowTranscript {
        fn delay(deadline: Deadline) -> DriverResult<()> {
            deadline.remaining_millis()?;
            std::thread::sleep(Duration::from_millis(5));
            deadline.remaining_millis()?;
            Ok(())
        }
    }
    impl ByteTransport for SlowTranscript {
        fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<()> {
            Self::delay(deadline)?;
            assert!(self.pending.is_empty());
            let (command, reply) = self.exchanges.pop_front().expect("unexpected command");
            assert_eq!(bytes, format!("{command}\n").as_bytes());
            self.pending.extend(reply);
            Ok(())
        }
        fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
            Self::delay(deadline)?;
            assert!(maximum <= 16_384 && !self.pending.is_empty());
            Ok(self.pending.drain(..maximum.min(self.pending.len())).collect())
        }
        fn close(&mut self) -> DriverResult<CloseReport> {
            self.live = false;
            Ok(CloseReport { released: true, status: Some(0) })
        }
        fn has_responsibility(&self) -> bool { self.live }
    }

    #[test]
    fn capture_budget_does_not_expire_after_one_communication_timeout() {
        let wire = SlowTranscript {
            exchanges: VecDeque::from([
                ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
                (":TRIGger:STATus?", b"STOP\n".to_vec()),
                (":ACQuire:MDEPth?", b"3\n".to_vec()),
                (":WAVeform:SOURce CHANnel1", vec![]),
                (":WAVeform:MODE RAW", vec![]),
                (":WAVeform:FORMat BYTE", vec![]),
                (":WAVeform:STARt 1", vec![]),
                (":WAVeform:POINts 3", vec![]),
                (":WAVeform:STOP 3", vec![]),
                (":WAVeform:PREamble?", b"0,2,3,1,1e-6,0,0,0.1,0,128\n".to_vec()),
                (":WAVeform:STARt 1", vec![]),
                (":WAVeform:STOP 3", vec![]),
                (":WAVeform:DATA?", b"#13\x00\x80\xff\n".to_vec()),
            ]),
            pending: VecDeque::new(), live: true,
        };
        let clock = Arc::new(ManualClock::default());
        let scope = Scope::with_transport(ScopeModel::Ds7054, Box::new(wire), clock.clone(), ScopeOptions {
            timeout: Duration::from_millis(50), ..Default::default()
        }).unwrap();
        let mut session = RigolSession::from_scope(scope, clock).unwrap();
        session.connect().unwrap();
        let start = std::time::Instant::now();
        session.run(Action::Scope(ScopeAction::Capture {
            request: CaptureRequest { channels: vec![1], source: CaptureSource::Memory, points: Some(3), byte_budget: 3 },
            fresh: false, stop_before: false,
        })).unwrap();
        assert!(start.elapsed() > Duration::from_millis(50));
        let frame = session.take_waveform().unwrap();
        assert!(matches!(&frame.channels[0].samples, Samples::U8(samples) if samples == &[0, 128, 255]));
        assert!(session.close().unwrap().resources_released());
    }
}
impl DriverLifecycle for RigolSession {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        self.stop.request_stop();
        let report = match &mut self.driver {
            Driver::Scope(d) => d.close(),
            Driver::Generator(d) => d.close(),
        };
        self.update(None, true);
        report
    }
    fn has_responsibility(&self) -> bool {
        match &self.driver {
            Driver::Scope(d) => d.has_responsibility(),
            Driver::Generator(d) => d.has_responsibility(),
        }
    }
}
impl DeviceSession for RigolSession {
    fn connect(&mut self) -> DriverResult<ProbeReport> {
        if self.stop.requested.load(Ordering::Acquire) {
            return Err(DriverError::Responsibility(
                "stopped session cannot connect".into(),
            ));
        }
        let report = match &mut self.driver {
            Driver::Scope(d) => d.connect(),
            Driver::Generator(d) => d.connect(),
        };
        if let Ok(report) = &report {
            self.identity = report.identity().clone();
        }
        let handle = match &self.driver {
            Driver::Scope(d) => d.stop_handle(),
            Driver::Generator(d) => d.stop_handle(),
        };
        let mut current = self.stop.handle.lock().unwrap();
        *current = handle;
        if self.stop.requested.load(Ordering::Acquire) {
            current.request_stop();
            return Err(DriverError::Responsibility(
                "connect completed after stop intent".into(),
            ));
        }
        drop(current);
        self.update(Some(json!({"identity":self.identity()})), true);
        report
    }
    fn probe_readonly(&mut self) -> DriverResult<ProbeReport> {
        let report = self.connect()?;
        let deadline = self.deadline();
        let transfer_state = match &mut self.driver {
            Driver::Scope(d) if d.model() == ScopeModel::Ds7054 => {
                d.inspect_transfer_state_readonly(deadline)?
            }
            _ => return Ok(report),
        };
        let mut observations = report.observations().clone();
        observations["transfer_state"] = transfer_state;
        Ok(ProbeReport::new(report.identity().clone(), observations, report.release_confirmed()))
    }
    fn state(&self) -> DriverState {
        match &self.driver {
            Driver::Scope(d) => d.state(),
            Driver::Generator(d) => d.state(),
        }
    }
    fn identity(&self) -> Value {
        self.identity.clone()
    }
    fn stop_signal(&self) -> Arc<dyn StopSignal> {
        self.stop.clone()
    }
    fn cached_observer(&self) -> Option<Arc<dyn CachedObserver>> {
        Some(Arc::new(Observer(self.cache.clone())))
    }
    fn action(&mut self, name: &str, args: &Value, context: &ContextV3) -> OutcomeV3 {
        let kind = match self.driver {
            Driver::Scope(_) => "scope",
            Driver::Generator(_) => "generator",
        };
        let action = match crate::actions::parse(kind, name, args) {
            Ok(a) => a,
            Err(e) => {
                return crate::backend::failed(Some(context.clone()), Phase::RejectedBeforeCall, e)
            }
        };
        let start = std::time::Instant::now();
        match self.run(action) {
            Ok((status, capture)) => {
                let historical = status.is_none();
                self.update(status, historical);
                OutcomeV3 {
                    phase: Phase::Completed,
                    context: Some(context.clone()),
                    result: Some(
                        json!({"status":self.observe(context).status,"hardware_read_completed":capture,"timing":{"io_s":start.elapsed().as_secs_f64()}}),
                    ),
                    error: None,
                }
            }
            Err(error) => {
                self.update(None, true);
                let error = if matches!(error, DriverError::Canceled) {
                    WorkerError::new("Canceled", error.to_string())
                } else {
                    WorkerError::from(error)
                };
                crate::backend::failed(Some(context.clone()), Phase::FailedAfterCallStarted, error)
            }
        }
    }
    fn observe(&mut self, context: &ContextV3) -> Observation {
        Observer(self.cache.clone()).observe(context)
    }
    fn take_waveform(&mut self) -> Option<yang_data::WaveformFrame> {
        self.waveform.take()
    }
}
