//! Laser lifecycle and evidence over the shared typed owner proxy.
use crate::{
    actions::{Action, LaserAction},
    newport::{Command, Newport, Reply, Ticket},
    observations::Observation,
    session::{DeviceSession, StopSignal},
    WorkerError
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration
};
use yang_drivers::{
    clock::Clock,
    lifecycle::{CleanupReport, CleanupStep, DriverLifecycle, DriverState, ProbeReport},
    DriverError, DriverResult,
};
use yang_protocol::{ContextV3, DomainConfig, OutcomeV3, Phase};
use yang_lab_tlb::{self as tlb, Control};
struct Stop(Arc<AtomicBool>);
struct OwnedMove {
    kind: &'static str,
    phase: &'static str,
    target: f64,
    started: Duration,
    ended: Option<Duration>,
    message: String,
    settled_since: Option<Duration>,
    hold_started: Option<Duration>,
    target_changed: bool,
}
impl OwnedMove {
    fn active(&self) -> bool {
        matches!(self.phase,"moving"|"holding"|"hold_timed_out"|"stopping")
    }
}
impl StopSignal for Stop {
    fn request_stop(&self) {
        self.0.store(true, Ordering::Release);
    }
    fn cancel_operation(&self) {}
}
pub(crate) struct LaserSession {
    config: DomainConfig,
    owner: Arc<Newport>,
    clock: Arc<dyn Clock>,
    stop: Arc<Stop>,
    state: DriverState,
    identity: Value,
    responsibility: bool,
    pending: Option<Ticket>,
    full: Option<Value>,
    motion: Option<Value>,
    full_at: Option<Duration>,
    motion_at: Option<Duration>,
    following: Option<bool>,
    motion_pending: bool,
    read_full: bool,
    movement: Option<OwnedMove>,
}
fn worker_error(error: tlb::Error) -> WorkerError {
    WorkerError::new(match error.kind {
        "safety" | "canceled" | "capacity" => "InvalidArguments",
        _ => "NativeLaser"
    }, error.message)
}
impl LaserSession {
    pub(crate) fn new(config: DomainConfig, owner: Arc<Newport>, clock: Arc<dyn Clock>) -> Self {
        Self {
            config,
            owner,
            clock,
            stop: Arc::new(Stop(Arc::new(AtomicBool::new(false)))),
            state: DriverState::Disconnected,
            identity: json!({}),
            responsibility: false,
            pending: None,
            full: None,
            motion: None,
            full_at: None,
            motion_at: None,
            following: None,
            motion_pending: false,
            read_full: true,
            movement: None,
        }
    }
    fn key(&self) -> String {
        self.config.params["device_key"].as_str().unwrap().into()
    }
    fn invoke(&mut self, command: Command) -> Result<Reply, WorkerError> {
        if self.pending.is_some() {
            return Err(WorkerError::new("NativeLaser", "Exact prior native exchange remains pending"));
        }
        let fence = if matches!(&command, Command::Disconnect(_)) {
            None
        } else {
            Some(self.stop.0.clone())
        };
        let ticket = self.owner.submit_guarded(command, fence).map_err(worker_error)?;
        self.pending = Some(ticket.clone());
        match ticket.wait(self.owner.timeout()) {
            Ok(completed) => {
                self.pending = None;
                self.responsibility = completed.responsibility;
                completed.result.map_err(worker_error)
            }
            Err(error) => {
                self.responsibility = true;
                Err(worker_error(error))
            }
        }
    }
    fn open(&mut self, readonly: bool) -> DriverResult<ProbeReport> {
        if self.state != DriverState::Disconnected || self.stop.0.load(Ordering::Acquire) {
            return Err(DriverError::Closed);
        }
        self.state = DriverState::Connecting;
        let limits = crate::catalog::laser_limits(
            &self.config.params,
            self.config.expected_identity["head_model"].as_str(),
        ).map_err(|e| DriverError::Invalid(e.to_string()))?;
        let result = self.invoke(Command::Connect {
            key: self.key(),
            limits,
            full: !readonly
        });
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => {
                self.state = DriverState::Fault;
                return Err(DriverError::Responsibility(error.to_string()));
            }
        };
        if let Reply::Connected { identity, sample } = reply {
            self.identity = serde_json::to_value(identity).unwrap();
            if !self.config.expected_identity.as_object().unwrap().iter().all(|(k, v)| self.identity.get(k) == Some(v)) {
                self.state = DriverState::Fault;
                return Err(DriverError::Invalid("Connected controller/head differs from registered identity".into()));
            }
            self.state = DriverState::Ready;
            if let Some((sample, at)) = sample {
                self.record(sample, at, true);
            }
            Ok(ProbeReport::new(self.identity.clone(), self.snapshot(), false))
        } else {
            self.state = DriverState::Fault;
            Err(DriverError::Invalid("Unexpected native connect result".into()))
        }
    }
    fn record(&mut self, mut value: Value, at: Duration, full: bool) {
        value["received_at"] = json!(at.as_secs_f64());
        if full {
            // Full refreshes are ownership evidence too. Latch a changed
            // setpoint before any subsequent conditional hold can write.
            if let Some(m)=self.movement.as_mut().filter(|m|m.kind=="goto"&&m.active()) {
                if value["wavelength_setpoint_nm"].as_f64().is_none_or(|v|(v-m.target).abs()>0.000001) {
                    m.target_changed=true;
                    if m.phase=="moving" {
                        m.phase="interrupted";
                        m.ended=Some(self.clock.now());
                        m.message="The target changed during the move. No automatic command was sent; use Stop Scan before retrying.".into();
                    }
                }
            }
            if self.following.is_none() {
                self.following = value["tracking"].as_bool();
            }
            self.full = Some(value);
            self.full_at = Some(at);
            self.motion = None;
            self.motion_at = None;
        } else {
            self.motion = Some(value);
            self.motion_at = Some(at);
        }
        self.motion_pending = false;
        self.read_full = false;
    }
    fn snapshot(&self) -> Value {
        let spec = self.identity["head_model"].as_str().and_then(tlb::model_spec);
        let p = &self.config.params;
        let now = self.clock.now();
        json!({
            "state": self.state,
            "connected": self.state == DriverState::Ready,
            "identity": self.identity,
            "resource": self.key(),
            "laser": self.full,
            "motion": self.motion,
            "motion_pending": self.motion_pending,
            "move": self.movement.as_ref().map(|m| json!({
                "kind": m.kind, "phase": m.phase, "target_nm": m.target,
                "elapsed_s": m.ended.unwrap_or(now).saturating_sub(m.started).as_secs_f64(),
                "message": m.message
            })),
            "single_scan_supported": self.full.as_ref().and_then(|s|s["single_scan_supported"].as_bool()).unwrap_or(false),
            "single_scan_rates_nm_s": self.full.as_ref().and_then(|s|s.get("single_scan_rates_nm_s")).cloned().unwrap_or_else(||json!([])),
            "target_following_enabled": self.following,
            "wavelength_range_nm": spec.map(|(a, b, _)| [a, b]),
            "max_scan_speed_nm_s": spec.map(|(_, _, v)| v),
            "operating_range_nm": spec.map(|(a, b, _)| [p["operating_min_nm"].as_f64().unwrap_or(a), p["operating_max_nm"].as_f64().unwrap_or(b)]),
            "operating_max_speed_nm_s": spec.map(|(_, _, v)| p["scan_speed_limit_nm_s"].as_f64().unwrap_or(v)),
            "sample_age_s": self.full_at.map(|at| now.saturating_sub(at).as_secs_f64()),
            "motion_age_s": self.motion_at.map(|at| now.saturating_sub(at).as_secs_f64())
        })
    }
    fn end_move(&mut self, phase: &'static str, message: impl Into<String>) {
        if let Some(m) = &mut self.movement {
            m.phase = phase;
            m.ended = Some(self.clock.now());
            m.message = message.into();
        }
    }
    fn observe_move(&mut self) -> Result<(), WorkerError> {
        let Some(m) = &self.movement else { return Ok(()); };
        let (phase, target, goto) = (m.phase, m.target, m.kind == "goto");
        if !m.active() { return Ok(()); }
        let settled = goto && m.settled_since.is_some_and(|at|self.clock.now().saturating_sub(at)>=Duration::from_secs(1));
        if phase == "moving" && goto && self.clock.now().saturating_sub(m.started) >= Duration::from_secs(120) {
            self.end_move("timed_out", "Arrival was not confirmed within 120 s. Use Stop Scan to hold, then retry.");
            // No automatic stop or replay on timeout. Keep fresh truthful motion.
            if let Reply::Sample(sample,at) = self.invoke(Command::Motion(self.key()))? { self.record(sample,at,false); }
            return Ok(());
        }
        if phase == "holding" && m.hold_started.is_some_and(|at|self.clock.now().saturating_sub(at)>=Duration::from_secs(120)) {
            let m=self.movement.as_mut().unwrap();
            m.phase="hold_timed_out";
            m.message="Tracking-off hold verification exceeded 120 s. Use Stop Scan; new movement remains blocked until the hold is verified.".into();
        }
        let command = if phase == "stopping" { Command::Motion(self.key()) }
            else { Command::FinishMove(self.key(), target, goto, settled) };
        let (sample, at, progress) = match self.invoke(command)? {
            Reply::MoveSample(s, at, progress) => (s, at, progress),
            Reply::Sample(s, at) => (s, at, tlb::MoveProgress::Moving),
            _ => return Err(WorkerError::new("NativeLaser", "Unexpected owned-move sample")),
        };
        if goto && sample["wavelength_setpoint_nm"].as_f64().is_none_or(|v|(v-target).abs()>0.000001) {
            self.movement.as_mut().unwrap().target_changed=true;
        }
        let target_changed=self.movement.as_ref().unwrap().target_changed;
        if phase == "stopping" {
            if sample["operation_complete"] == true && sample["tracking"] == false {
                self.end_move("stopped", "Stopped; motor tracking is off and the current position is held.");
            }
        } else if target_changed && progress!=tlb::MoveProgress::VerifyingHold {
            self.end_move("interrupted",if progress==tlb::MoveProgress::Held {
                "The target changed during the move. Motor hold is verified; no further move was sent."
            } else {
                "The tracking or target changed. No automatic command was sent; use Stop Scan before retrying."
            });
        } else {
            match progress {
                tlb::MoveProgress::VerifyingHold => {
                    let m=self.movement.as_mut().unwrap();
                    m.hold_started.get_or_insert(self.clock.now());
                    if m.phase!="hold_timed_out" {
                        m.phase="holding";
                        m.message=if m.target_changed {
                            "The target changed during hold verification. Waiting for Tracking Off and controller readiness; no new move will be sent."
                        } else {
                            "Tracking Off accepted; waiting for Tracking Off and controller readiness readbacks."
                        }.into();
                    }
                }
                tlb::MoveProgress::Held => {
                    self.following=Some(false);
                    let tolerance=if goto {tlb::GOTO_ARRIVAL_TOLERANCE_NM} else {tlb::SCAN_ARRIVAL_TOLERANCE_NM};
                    let near=sample["wavelength_nm"].as_f64().is_some_and(|v|(v-target).abs()<=tolerance);
                    if near {self.end_move("arrived","Target reached; motor tracking is off.");}
                    else {self.end_move("held_off_target",format!("Motor hold verified at {} nm; target was {target} nm. No further move was sent.",sample["wavelength_nm"]));}
                }
                tlb::MoveProgress::Moving => {
                    if sample["tracking"]==false && (goto||sample["operation_complete"]==true) {
                        self.end_move("interrupted", "The tracking or target changed. No automatic command was sent; use Stop Scan before retrying.");
                    } else if goto {
                        let near=sample["wavelength_nm"].as_f64().is_some_and(|v|(v-target).abs()<=tlb::GOTO_ARRIVAL_TOLERANCE_NM);
                        let m=self.movement.as_mut().unwrap();
                        // Only completed observations contribute to the dwell.
                        if near {m.settled_since.get_or_insert(self.clock.now());} else {m.settled_since=None;}
                    }
                }
            }
        }
        self.record(sample, at, false);
        Ok(())
    }
}
impl DriverLifecycle for LaserSession {
    fn has_responsibility(&self) -> bool {
        self.responsibility || self.pending.is_some()
    }
    fn close(&mut self) -> DriverResult<CleanupReport> {
        self.stop.request_stop();
        if self.movement.as_ref().is_some_and(OwnedMove::active) {
            self.end_move("interrupted", "Disconnected; settings and outputs were preserved.");
        }
        self.state = DriverState::Closing;
        let result = (|| -> Result<(), WorkerError> {
            if let Some(ticket) = self.pending.clone() {
                let completed = ticket.wait(self.owner.timeout()).map_err(worker_error)?;
                self.pending = None;
                self.responsibility = completed.responsibility;
                // Collect the exact prior result, including an uncertain setter,
                // without replaying it. Only preserving disconnect follows.
            }
            if self.responsibility {
                self.invoke(Command::Disconnect(self.key()))?;
            }
            Ok(())
        })();
        let released = result.is_ok() && !self.has_responsibility();
        self.state = if released {
            DriverState::Disconnected
        } else {
            DriverState::Fault
        };
        if released {
            self.following = None;
        }
        CleanupReport::new(crate::new_id().map_err(|e| DriverError::Responsibility(e.to_string()))?,
        vec![CleanupStep {
            role: "laser".into(),
            action: "preserving_close".into(),
            error: result.err().map(|e| e.to_string())
        }], None,
        if released {
            vec![]
        } else {
            vec!["laser".into()]
        })
    }
}
impl DeviceSession for LaserSession {
    fn connect(&mut self) -> DriverResult<ProbeReport> {
        self.open(false)
    }
    fn probe_readonly(&mut self) -> DriverResult<ProbeReport> {
        self.open(true)
    }
    fn state(&self) -> DriverState {
        self.state
    }
    fn identity(&self) -> Value {
        self.identity.clone()
    }
    fn stop_signal(&self) -> Arc<dyn StopSignal> {
        self.stop.clone()
    }
    fn action(&mut self, name: &str, args: &Value, context: &ContextV3) -> OutcomeV3 {
        let action = match crate::actions::parse("laser", name, args) {
            Ok(Action::Laser(a)) => a,
            Err(e) => return crate::backend::failed(Some(context.clone()), Phase::RejectedBeforeCall, e),
            _ => unreachable!()
        };
        if self.state != DriverState::Ready || self.stop.0.load(Ordering::Acquire) {
            return crate::backend::failed(
                Some(context.clone()),
                Phase::RejectedBeforeCall,
                WorkerError::new("Disconnected", "Laser session is stopped or faulted"),
            );
        }
        let following = match &action {
            LaserAction::Legacy(tlb::Action::Tracking(v)) | LaserAction::Control(Control::Tracking(v)) => Some(*v),
            _ => None
        };
        let passive = matches!(&action, LaserAction::Status | LaserAction::Motion |
            LaserAction::Control(Control::Output(_)|Control::ScanStop|Control::Tracking(false)) |
            LaserAction::Legacy(tlb::Action::Output(_)|tlb::Action::Tracking(false)));
        if !passive && self.movement.as_ref().is_some_and(OwnedMove::active) {
            return crate::backend::failed(Some(context.clone()), Phase::RejectedBeforeCall,
                WorkerError::new("InvalidArguments", "A move is active. Use Stop Scan before starting another move."));
        }
        let endpoint = match &action {
            LaserAction::Goto(v) => Some(("goto", *v)),
            LaserAction::Control(Control::ScanStart(p)) => Some(("full_scan", p.start_nm)),
            LaserAction::Control(Control::ScanTo(p)) => Some(("single_scan", p.target_nm)),
            _ => None,
        };
        let command = match action {
            LaserAction::Status => {
                self.read_full = true;
                None
            },
            LaserAction::Motion => {
                self.read_full = false;
                None
            },
            LaserAction::Legacy(a) => Some(Command::Legacy(self.key(), a)),
            LaserAction::Control(a @ (Control::ScanStart(_)|Control::ScanTo(_))) => Some(Command::BeginMove(self.key(), a)),
            LaserAction::Control(a) => Some(Command::Control(self.key(), a)),
            LaserAction::Goto(v) => Some(Command::BeginMove(self.key(), Control::Wavelength(v))),
            LaserAction::Target(v) => Some(Command::Control(self.key(), if self.following == Some(true) {
                Control::Wavelength(v)
            } else {
                Control::Target(v)
            })),
        };
        if let Some(command) = command {
            if let Err(error) = self.invoke(command) {
                let phase = if error.code == "InvalidArguments" {
                    Phase::RejectedBeforeCall
                } else {
                    self.state = DriverState::Fault;
                    Phase::FailedAfterCallStarted
                };
                return crate::backend::failed(Some(context.clone()), phase, error);
            }
            if let Some(value) = following {
                self.following = Some(value);
                if !value { self.end_move("interrupted", "Tracking was turned off."); }
            }
            if let Some((kind,target)) = endpoint {
                self.movement = Some(OwnedMove {kind, phase:"moving", target,
                    started:self.clock.now(), ended:None, settled_since:None, hold_started:None, target_changed:false,
                    message:"Moving; waiting for controller arrival and tracking-off verification.".into()});
            }
            if name == "stop_scan" {
                let target = self.motion.as_ref().or(self.full.as_ref()).and_then(|s|s["wavelength_nm"].as_f64()).unwrap_or(0.);
                self.movement = Some(OwnedMove {kind:"stop", phase:"stopping", target,
                    started:self.clock.now(), ended:None, settled_since:None, hold_started:None, target_changed:false,
                    message:"Stop accepted; verifying the motor hold.".into()});
            }
            if matches!(name, "goto_wavelength" | "set_target_wavelength" | "control_piezo" | "start_scan" | "scan_forward" | "scan_backward" | "stop_scan") {
                self.motion_pending = true;
                self.read_full = false;
                return crate::backend::completed(Some(context.clone()), json!({
                    "result": null,
                    "acknowledged": true,
                    "status": {
                        "motion_pending": true,
                        "target_following_enabled": self.following,
                        "move": self.snapshot()["move"]
                    }
                }));
            }
            self.read_full = true;
        }
        crate::backend::completed(Some(context.clone()), json!({
            "result": null,
            "post_readback": true
        }))
    }
    fn observe(&mut self, _: &ContextV3) -> Observation {
        let mut failure = None;
        if self.state == DriverState::Ready && !self.stop.0.load(Ordering::Acquire) {
            let full = self.read_full;
            let moving = self.movement.as_ref().is_some_and(OwnedMove::active);
            // Scheduler observations continue while the GUI is on another page.
            // An ordinary full read still updates emission/current/power, then
            // the explicitly owned lifecycle performs its conditional hold.
            let command = if full {
                Command::Status(self.key())
            } else {
                Command::Motion(self.key())
            };
            let result = if moving && !full { self.observe_move() } else { match self.invoke(command) {
                Ok(Reply::Sample(sample, at)) => self.record(sample, at, full),
                Ok(_) => {
                    self.state = DriverState::Fault;
                    failure = Some("Unexpected native sample".into());
                }
                Err(error) => {
                    self.state = DriverState::Fault;
                    failure = Some(error.to_string());
                }
            }; Ok(()) };
            if moving && full && failure.is_none() {
                if let Err(error) = self.observe_move() { failure = Some(error.to_string()); }
            }
            if let Err(error) = result { failure = Some(error.to_string()); }
            if failure.is_some() {
                self.state = DriverState::Fault;
                self.end_move("failed", "Motion evidence or tracking-off verification failed. Disconnect and review the controller.");
            }
        }
        let mut status = self.snapshot();
        if let Some(error) = failure {
            status["observation_error"] = json!({
                "type": "NativeLaser",
                "message": error
            });
        }
        Observation {
            status,
            more: false,
            sampled_at: self.motion_at.or(self.full_at).or(Some(self.clock.now()))
        }
    }
}
