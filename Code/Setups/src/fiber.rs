use super::calibration::*;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use yang_drivers::{
    clock::Clock,
    lifecycle::{CleanupReport, CleanupStep, DriverLifecycle, DriverState, ProbeReport},
    mdt::{Axis, BaselineAttestation, Mdt693b, MdtConfig, MdtStatus},
    transport::{
        serial_abi::{NativeSerial, SerialBackend},
        serial_discovery::SerialDeviceInfo,
        ResourceBook,
    },
    DriverError, DriverResult,
};
const SIDES: [StageSide; 2] = [StageSide::Left, StageSide::Right];
const AXES: [LogicalAxis; 3] = [LogicalAxis::X, LogicalAxis::Y, LogicalAxis::Z];
struct StopState {
    requested: AtomicBool,
    generation: AtomicU64,
    handles: Mutex<Vec<yang_drivers::mdt::StopHandle>>,
}
#[derive(Clone)]
pub struct StopHandle(Arc<StopState>);
impl StopHandle {
    pub fn request_stop(&self) {
        self.0.requested.store(true, Ordering::Release);
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        for handle in self.0.handles.lock().unwrap().iter() {
            handle.request_stop();
        }
    }
}
#[derive(Debug)]
pub struct StageBaselineAttestation {
    side: StageSide,
    generation: u64,
    nominal: bool,
    driver: BaselineAttestation,
}
#[derive(Clone, Debug, Serialize)]
pub struct MotionEvidence {
    pub requested_um: Vector3Um,
    pub completed_axes: Vec<LogicalAxis>,
    pub last_observed_voltage_v: Option<BTreeMap<LogicalAxis, f64>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct StageStatus {
    pub side: StageSide,
    pub available: bool,
    pub serial_number: String,
    pub resource: Option<String>,
    pub toward_chip_sign: i8,
    pub toward_chip_limit_um: f64,
    pub other_limit_um: f64,
    pub baseline_known: bool,
    pub estimated_position_um: Option<Vector3Um>,
    pub observed_voltage_v: Option<BTreeMap<LogicalAxis, f64>>,
    pub calibration: BTreeMap<LogicalAxis, AxisCalibration>,
    pub nominal_authorized: bool,
    pub restricted: bool,
    pub fault: Option<String>,
    pub motion_evidence: Option<MotionEvidence>,
}
#[derive(Clone, Debug, Serialize)]
pub struct MoveResult {
    pub side: StageSide,
    pub requested_um: Vector3Um,
    pub voltage_delta_v: BTreeMap<LogicalAxis, f64>,
    pub calibration_used: BTreeMap<LogicalAxis, CalibrationCoefficient>,
    pub estimated_before_um: Vector3Um,
    pub estimated_after_um: Vector3Um,
    pub observed_voltage_v: BTreeMap<LogicalAxis, f64>,
    pub confirmed: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct FiberDiscovery {
    pub registered: BTreeMap<StageSide, SerialDeviceInfo>,
    pub missing_sides: Vec<StageSide>,
    pub unknown_devices: Vec<SerialDeviceInfo>,
}
pub struct FiberStage {
    definition: StageDefinition,
    config: FiberConfig,
    driver: Option<Mdt693b>,
    resource: Option<String>,
    baseline: Option<BTreeMap<LogicalAxis, f64>>,
    estimate: Option<Vector3Um>,
    nominal_authorized: bool,
    evidence: Option<MotionEvidence>,
    stop: Arc<StopState>,
}
impl FiberStage {
    fn invalidate(&mut self) {
        self.baseline = None;
        self.estimate = None;
        self.nominal_authorized = false;
    }
    fn observed(&self, st: &MdtStatus) -> BTreeMap<LogicalAxis, f64> {
        AXES.into_iter()
            .map(|a| (a, st.axes[&self.definition.axis_map[&a]].actual_v))
            .collect()
    }
    fn validate(&self, st: &MdtStatus) -> DriverResult<BTreeMap<LogicalAxis, f64>> {
        if self.stop.requested.load(Ordering::Acquire)
            || !st.axis_command_known
            || st.restricted
            || st.fault_evidence.is_some()
            || st.serial_number != self.definition.serial_number
            || st.master_scan_enabled
            || st.master_scan_voltage_v.abs() > 1e-6
        {
            return Err(invalid(
                "stage lost current baseline/control authority; stop and hold",
            ));
        }
        let observed = self.observed(st);
        if observed.values().any(|v| !v.is_finite()) {
            return Err(invalid("nonfinite stage observation"));
        }
        Ok(observed)
    }
    fn coherent(
        &self,
        st: &MdtStatus,
        expected: &BTreeMap<LogicalAxis, f64>,
    ) -> DriverResult<BTreeMap<LogicalAxis, f64>> {
        let values = self.validate(st)?;
        if AXES.iter().any(|a| (values[a] - expected[a]).abs() > 1e-6) {
            return Err(invalid(
                "external/manual move or inconsistent command observation",
            ));
        }
        Ok(values)
    }
    fn needs_nominal(&self) -> bool {
        self.definition.calibration.values().any(|c| {
            [&c.positive, &c.negative]
                .iter()
                .any(|c| c.source == "nominal_MAX312D")
        })
    }
    pub fn status(&mut self) -> StageStatus {
        let st = self.driver.as_ref().and_then(Mdt693b::status);
        let available = self.driver.as_ref().is_some_and(|d| {
            matches!(
                d.state(),
                DriverState::Ready | DriverState::Active | DriverState::Fault
            )
        });
        if self.stop.requested.load(Ordering::Acquire)
            || !available
            || st.as_ref().is_none_or(|st| {
                self.baseline
                    .as_ref()
                    .is_none_or(|b| self.coherent(st, b).is_err())
            })
        {
            self.invalidate();
        }
        StageStatus {
            side: self.definition.side,
            available,
            serial_number: self.definition.serial_number.clone(),
            resource: self.resource.clone(),
            toward_chip_sign: self.definition.toward_chip_sign,
            toward_chip_limit_um: self.config.toward,
            other_limit_um: self.config.other,
            baseline_known: self.baseline.is_some(),
            estimated_position_um: self.estimate,
            observed_voltage_v: st.as_ref().map(|st| self.observed(st)),
            calibration: self.definition.calibration.clone(),
            nominal_authorized: self.nominal_authorized,
            restricted: st.as_ref().is_some_and(|st| st.restricted),
            fault: st.as_ref().and_then(|st| st.fault_evidence.clone()),
            motion_evidence: self.evidence.clone(),
        }
    }
    pub fn baseline_attestation(
        &self,
        confirm: bool,
        allow_nominal: bool,
    ) -> DriverResult<StageBaselineAttestation> {
        if !confirm || self.stop.requested.load(Ordering::Acquire) {
            return Err(invalid(
                "baseline adoption requires explicit confirmation and live setup",
            ));
        }
        if self.needs_nominal() && !allow_nominal {
            return Err(invalid(
                "nominal MAX312D conversion requires explicit authorization",
            ));
        }
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| invalid("stage unavailable"))?;
        Ok(StageBaselineAttestation {
            side: self.definition.side,
            generation: self.stop.generation.load(Ordering::Acquire),
            nominal: allow_nominal && self.needs_nominal(),
            driver: driver.baseline_attestation(confirm)?,
        })
    }
    pub fn adopt_baseline(&mut self, a: StageBaselineAttestation) -> DriverResult<StageStatus> {
        if a.side != self.definition.side
            || a.generation != self.stop.generation.load(Ordering::Acquire)
            || self.stop.requested.load(Ordering::Acquire)
        {
            return Err(invalid("stale/wrong-side baseline attestation"));
        }
        self.invalidate();
        let st = self
            .driver
            .as_ref()
            .ok_or_else(|| invalid("stage unavailable"))?
            .adopt_current_axis_baseline(a.driver)?;
        let observed = self.validate(&st)?;
        let current = self
            .driver
            .as_ref()
            .unwrap()
            .status()
            .ok_or_else(|| invalid("missing adoption status"))?;
        self.coherent(&current, &observed)?;
        if a.generation != self.stop.generation.load(Ordering::Acquire)
            || self.stop.requested.load(Ordering::Acquire)
        {
            return Err(invalid("setup stopped during adoption"));
        }
        self.baseline = Some(observed);
        self.estimate = Some(Vector3Um::new(0., 0., 0.)?);
        self.nominal_authorized = a.nominal;
        self.evidence = None;
        Ok(self.status())
    }
    pub fn move_by_um(&mut self, requested: Vector3Um) -> DriverResult<MoveResult> {
        Vector3Um::new(requested.x, requested.y, requested.z)?;
        for a in AXES {
            let delta = requested.for_axis(a);
            let limit = if a == LogicalAxis::X
                && delta * f64::from(self.definition.toward_chip_sign) > 0.
            {
                self.config.toward
            } else {
                self.config.other
            };
            if delta.abs() > limit {
                return Err(invalid(
                    "requested displacement exceeds exact per-axis limit",
                ));
            }
        }
        let before = self
            .estimate
            .ok_or_else(|| invalid("session baseline unknown"))?;
        let baseline = self
            .baseline
            .clone()
            .ok_or_else(|| invalid("command baseline unknown"))?;
        if self.needs_nominal() && !self.nominal_authorized {
            return Err(invalid("nominal conversion is not authorized"));
        }
        let generation = self.stop.generation.load(Ordering::Acquire);
        let fresh = (|| {
            let d = self
                .driver
                .as_ref()
                .ok_or_else(|| invalid("stage unavailable"))?;
            d.get_all_voltages()?;
            let st = d.status().ok_or_else(|| invalid("no fresh status"))?;
            self.coherent(&st, &baseline)?;
            Ok(st)
        })();
        let st = match fresh {
            Ok(st) => st,
            Err(e) => {
                self.invalidate();
                return Err(e);
            }
        };
        let mut targets = BTreeMap::new();
        let mut deltas = BTreeMap::new();
        let mut calibration = BTreeMap::new();
        for a in AXES {
            let d = requested.for_axis(a);
            if d == 0. {
                continue;
            }
            let c = if d > 0. {
                &self.definition.calibration[&a].positive
            } else {
                &self.definition.calibration[&a].negative
            };
            let delta = d / c.um_per_v / f64::from(self.definition.polarity[&a]);
            let target = baseline[&a] + delta;
            let physical = self.definition.axis_map[&a];
            let bounds = st.axes[&physical];
            let ceiling = bounds.maximum_v.min(st.hardware_limit.volts()).min(
                self.config.ceilings[match physical {
                    Axis::X => 0,
                    Axis::Y => 1,
                    Axis::Z => 2,
                }],
            );
            if !delta.is_finite()
                || !target.is_finite()
                || target < 0.
                || target < bounds.minimum_v
                || target > ceiling
            {
                return Err(invalid(
                    "planned target outside effective electrical bounds",
                ));
            }
            targets.insert(a, target);
            deltas.insert(a, delta);
            calibration.insert(a, c.clone());
        }
        let order = if requested.x * f64::from(self.definition.toward_chip_sign) > 0. {
            [LogicalAxis::Y, LogicalAxis::Z, LogicalAxis::X]
        } else {
            [LogicalAxis::X, LogicalAxis::Y, LogicalAxis::Z]
        };
        let mut working = baseline;
        let mut completed = vec![];
        let mut last = Some(working.clone());
        let result = (|| {
            for a in order {
                let Some(target) = targets.get(&a) else {
                    continue;
                };
                if self.stop.requested.load(Ordering::Acquire)
                    || generation != self.stop.generation.load(Ordering::Acquire)
                {
                    return Err(invalid("setup stopped during move"));
                }
                let physical = self.definition.axis_map[&a];
                let returned = self
                    .driver
                    .as_ref()
                    .unwrap()
                    .set_axis_voltage(physical, *target)?;
                working.insert(a, *target);
                last = Some(self.coherent(&returned, &working)?);
                let current = self
                    .driver
                    .as_ref()
                    .unwrap()
                    .status()
                    .ok_or_else(|| invalid("lost current move status"))?;
                last = Some(self.coherent(&current, &working)?);
                completed.push(a);
            }
            self.driver.as_ref().unwrap().get_all_voltages()?;
            let current = self
                .driver
                .as_ref()
                .unwrap()
                .status()
                .ok_or_else(|| invalid("lost final move status"))?;
            let final_observed = self.coherent(&current, &working)?;
            last = Some(final_observed.clone());
            let after = Vector3Um::new(
                before.x + requested.x,
                before.y + requested.y,
                before.z + requested.z,
            )?;
            if generation != self.stop.generation.load(Ordering::Acquire)
                || self.stop.requested.load(Ordering::Acquire)
            {
                return Err(invalid("setup stopped before position publication"));
            }
            self.baseline = Some(final_observed.clone());
            self.estimate = Some(after);
            self.evidence = None;
            Ok(MoveResult {
                side: self.definition.side,
                requested_um: requested,
                voltage_delta_v: deltas,
                calibration_used: calibration,
                estimated_before_um: before,
                estimated_after_um: after,
                observed_voltage_v: final_observed,
                confirmed: true,
            })
        })();
        if result.is_err() {
            self.evidence = Some(MotionEvidence {
                requested_um: requested,
                completed_axes: completed,
                last_observed_voltage_v: last,
            });
            self.invalidate();
        }
        result
    }
}
pub struct FiberCouplingSetup {
    config: FiberConfig,
    serials: BTreeSet<String>,
    stages: BTreeMap<StageSide, FiberStage>,
    discovery: FiberDiscovery,
    book: ResourceBook,
    clock: Arc<dyn Clock>,
    backend: Arc<dyn SerialBackend>,
    stop: Arc<StopState>,
    report: Option<CleanupReport>,
}
impl FiberCouplingSetup {
    pub fn for_members(serials: &[String], config: FiberConfig) -> DriverResult<Self> {
        Self::with_backend(
            serials,
            config,
            ResourceBook::default(),
            Arc::new(yang_drivers::clock::SystemClock::default()),
            Arc::new(NativeSerial),
        )
    }
    pub fn with_backend(
        serials: &[String],
        config: FiberConfig,
        book: ResourceBook,
        clock: Arc<dyn Clock>,
        backend: Arc<dyn SerialBackend>,
    ) -> DriverResult<Self> {
        let selected: BTreeSet<_> = serials.iter().cloned().collect();
        if selected.is_empty()
            || selected.len() != serials.len()
            || selected.len() > 2
            || selected
                .iter()
                .any(|s| !SIDES.iter().any(|side| side.serial() == s))
        {
            return Err(invalid("select one or both registered serial numbers"));
        }
        let stop = Arc::new(StopState {
            requested: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            handles: Mutex::new(vec![]),
        });
        let stages = SIDES
            .into_iter()
            .map(|side| {
                (
                    side,
                    FiberStage {
                        definition: config.stages[&side].clone(),
                        config: config.clone(),
                        driver: None,
                        resource: None,
                        baseline: None,
                        estimate: None,
                        nominal_authorized: false,
                        evidence: None,
                        stop: stop.clone(),
                    },
                )
            })
            .collect();
        Ok(Self {
            config,
            serials: selected,
            stages,
            discovery: FiberDiscovery {
                registered: BTreeMap::new(),
                missing_sides: SIDES.to_vec(),
                unknown_devices: vec![],
            },
            book,
            clock,
            backend,
            stop,
            report: None,
        })
    }
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle(self.stop.clone())
    }
    pub fn enumerate(&self) -> DriverResult<FiberDiscovery> {
        let ports = yang_drivers::transport::serial::enumerate_with(self.backend.as_ref())?;
        Self::classify(ports)
    }
    pub fn classify(ports: Vec<SerialDeviceInfo>) -> DriverResult<FiberDiscovery> {
        let mut registered = BTreeMap::new();
        let mut canonical = BTreeSet::new();
        let mut unknown = vec![];
        for port in ports {
            if let Some(side) = SIDES.into_iter().find(|s| s.serial() == port.serial) {
                if registered.contains_key(&side)
                    || !canonical.insert(
                        yang_drivers::transport::serial::canonical_com(&port.resource)?
                            .as_str()
                            .to_string(),
                    )
                {
                    return Err(invalid("duplicate serial metadata or shared stage port"));
                }
                registered.insert(side, port);
            } else if port
                .description
                .to_ascii_lowercase()
                .split_whitespace()
                .any(|p| p == "mdt693b")
                && port
                    .description
                    .to_ascii_lowercase()
                    .split_whitespace()
                    .any(|p| p == "thorlabs")
                && !port.serial.is_empty()
            {
                unknown.push(port);
            }
        }
        unknown.sort_by(|a, b| (&a.serial, &a.resource).cmp(&(&b.serial, &b.resource)));
        let missing_sides = SIDES
            .into_iter()
            .filter(|s| !registered.contains_key(s))
            .collect();
        Ok(FiberDiscovery {
            registered,
            missing_sides,
            unknown_devices: unknown,
        })
    }
    pub fn connect(&mut self) -> DriverResult<ProbeReport> {
        if self.stop.requested.load(Ordering::Acquire) {
            return Err(invalid("closed/stopped setup cannot reconnect"));
        }
        if self.has_resource_responsibility() {
            return Err(DriverError::Busy(
                "fiber setup already owns resources".into(),
            ));
        }
        let discovery = if self.config.bindings.is_empty() {
            self.enumerate()?
        } else {
            Self::classify(
                self.config
                    .bindings
                    .iter()
                    .map(|b| SerialDeviceInfo {
                        resource: b.port.clone(),
                        instance_id: String::new(),
                        serial: b.serial.clone(),
                        description: "explicit registered binding (not identity proof)".into(),
                        vid: None,
                        pid: None,
                    })
                    .collect(),
            )?
        };
        for serial in &self.serials {
            if !discovery.registered.values().any(|d| &d.serial == serial) {
                return Err(invalid("selected stage absent"));
            }
        }
        self.discovery = discovery;
        let selected: Vec<_> = SIDES
            .into_iter()
            .filter(|s| self.serials.contains(s.serial()))
            .collect();
        for side in selected {
            let info = &self.discovery.registered[&side];
            let driver = Mdt693b::with_backend(
                MdtConfig {
                    port: info.resource.clone(),
                    application_limits_v: self.config.ceilings,
                    io_timeout: self.config.timeout,
                    ..MdtConfig::default()
                },
                self.book.clone(),
                self.clock.clone(),
                self.backend.clone(),
            )?;
            let stage = self.stages.get_mut(&side).unwrap();
            stage.resource = Some(info.resource.clone());
            stage.driver = Some(driver);
            let result = stage.driver.as_mut().unwrap().connect();
            let handle = stage.driver.as_ref().unwrap().stop_handle();
            self.stop.handles.lock().unwrap().push(handle.clone());
            if self.stop.requested.load(Ordering::Acquire) {
                handle.request_stop();
            }
            let success = result.and_then(|r| {
                let serial = r.identity()["serial"].as_str().unwrap_or("");
                if serial != side.serial() || self.stop.requested.load(Ordering::Acquire) {
                    Err(invalid("physical stage identity mismatch or setup stop"))
                } else {
                    Ok(())
                }
            });
            if let Err(e) = success {
                let _ = self.close();
                return Err(e);
            }
        }
        let identities: BTreeMap<_, _> = self
            .available_sides()
            .into_iter()
            .map(|side| {
                (
                    side,
                    self.stages[&side]
                        .driver
                        .as_ref()
                        .unwrap()
                        .status()
                        .unwrap()
                        .serial_number,
                )
            })
            .collect();
        Ok(ProbeReport::new(
            serde_json::json!({"model":"MAX312D","members":identities}),
            serde_json::json!({"baseline_known":false,"estimated_position_um":null}),
            false,
        ))
    }
    pub fn available_sides(&self) -> Vec<StageSide> {
        SIDES
            .into_iter()
            .filter(|s| {
                self.stages[s]
                    .driver
                    .as_ref()
                    .is_some_and(|d| d.has_resource_responsibility())
            })
            .collect()
    }
    pub fn missing_sides(&self) -> Vec<StageSide> {
        SIDES
            .into_iter()
            .filter(|s| !self.available_sides().contains(s))
            .collect()
    }
    pub fn unknown_devices(&self) -> &[SerialDeviceInfo] {
        &self.discovery.unknown_devices
    }
    pub fn stage(&mut self, side: StageSide) -> &mut FiberStage {
        self.stages.get_mut(&side).unwrap()
    }
    pub fn adopt_baseline(
        &mut self,
        side: StageSide,
        a: StageBaselineAttestation,
    ) -> DriverResult<StageStatus> {
        self.stage(side).adopt_baseline(a)
    }
    pub fn move_by_um(&mut self, side: StageSide, delta: Vector3Um) -> DriverResult<MoveResult> {
        self.stage(side).move_by_um(delta)
    }
    pub fn state(&self) -> DriverState {
        if !self.has_resource_responsibility() {
            return DriverState::Disconnected;
        }
        if self.stop.requested.load(Ordering::Acquire) {
            return DriverState::Closing;
        }
        let states: Vec<_> = self
            .stages
            .values()
            .filter_map(|s| s.driver.as_ref().map(Mdt693b::state))
            .collect();
        if states
            .iter()
            .any(|s| matches!(s, DriverState::Fault | DriverState::Disconnected))
        {
            DriverState::Fault
        } else if states.contains(&DriverState::Active) {
            DriverState::Active
        } else if states.contains(&DriverState::Connecting) {
            DriverState::Connecting
        } else {
            DriverState::Ready
        }
    }
    pub fn has_resource_responsibility(&self) -> bool {
        self.stages.values().any(|s| {
            s.driver
                .as_ref()
                .is_some_and(Mdt693b::has_resource_responsibility)
        })
    }
    pub fn close(&mut self) -> DriverResult<CleanupReport> {
        self.stop_handle().request_stop();
        if !self.has_resource_responsibility() {
            if let Some(r) = &self.report {
                return Ok(r.clone());
            }
        }
        let mut steps = vec![];
        let mut remaining = vec![];
        for (side, stage) in &mut self.stages {
            stage.invalidate();
            if let Some(d) = stage.driver.as_mut() {
                let result = d.close();
                let error = result.as_ref().err().map(ToString::to_string).or_else(|| {
                    result
                        .as_ref()
                        .ok()
                        .and_then(|r| r.steps().iter().find_map(|s| s.error.clone()))
                });
                steps.push(CleanupStep {
                    role: format!("fiber_{side:?}"),
                    action: "hold_and_release".into(),
                    error,
                });
                if d.has_resource_responsibility() {
                    remaining.push(format!("fiber_{side:?}"));
                } else {
                    stage.driver = None;
                    stage.resource = None;
                }
            }
        }
        if steps.is_empty() {
            steps.push(CleanupStep {
                role: "fiber".into(),
                action: "no_transport_opened".into(),
                error: None,
            });
        }
        static SEQ: AtomicU64 = AtomicU64::new(1);
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let report = CleanupReport::new(
            format!("{at:016x}{:016x}", SEQ.fetch_add(1, Ordering::Relaxed)),
            steps,
            None,
            remaining,
        )?;
        self.report = Some(report.clone());
        Ok(report)
    }
}
impl DriverLifecycle for FiberCouplingSetup {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        FiberCouplingSetup::close(self)
    }
    fn has_responsibility(&self) -> bool {
        self.has_resource_responsibility()
    }
}
impl Drop for FiberCouplingSetup {
    fn drop(&mut self) {
        self.stop_handle().request_stop(); /* Child driver Drops retain unfinished native owners; never automatic zero. */
    }
}
