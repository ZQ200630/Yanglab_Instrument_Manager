use super::{
    authority::{actual, Authority, TOLERANCE},
    protocol,
    session::{guard, publish, Io, Mdt693b, Op, Shared},
    settings::{confirmation, voltage},
    Axis, MdtStatus,
};
use crate::{lifecycle::DriverState, transport::Deadline, DriverError, DriverResult};
use std::time::Duration;
const AXES: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];
#[derive(Clone, Copy)]
pub(super) enum Motion {
    Axis(Axis, f64),
    All(f64),
    Master(f64),
    MasterEnabled(bool),
    Zero,
    Select(&'static str),
    Arrow(&'static str, i8),
}
impl Mdt693b {
    fn motion(&self, motion: Motion) -> DriverResult<MdtStatus> {
        let steps = (75. / self.config().ramp_step_v).ceil();
        if steps > 100_000. {
            return Err(DriverError::Invalid(
                "MDT configured ramp exceeds bounded step budget".into(),
            ));
        }
        let budget = (self.config().io_timeout.saturating_mul(18) + self.config().ramp_interval)
            .saturating_mul(steps as u32)
            .saturating_mul(4)
            + self.config().io_timeout.saturating_mul(60);
        self.request_budget(
            Op::Motion(motion),
            budget.min(Duration::from_secs(86400)),
            false,
        )
    }
    pub fn set_axis_voltage(&self, a: Axis, v: f64) -> DriverResult<MdtStatus> {
        voltage(v)?;
        self.motion(Motion::Axis(a, v))
    }
    pub fn set_all_voltages(&self, v: f64, confirm: bool) -> DriverResult<MdtStatus> {
        confirmation(confirm)?;
        voltage(v)?;
        self.motion(Motion::All(v))
    }
    pub fn set_master_scan_voltage(&self, v: f64, confirm: bool) -> DriverResult<MdtStatus> {
        confirmation(confirm)?;
        voltage(v)?;
        self.motion(Motion::Master(v))
    }
    pub fn set_master_scan_enabled(&self, v: bool, confirm: bool) -> DriverResult<MdtStatus> {
        confirmation(confirm)?;
        self.motion(Motion::MasterEnabled(v))
    }
    pub fn ramp_to_zero(&self) -> DriverResult<MdtStatus> {
        self.motion(Motion::Zero)
    }
    pub fn emergency_zero(&self, confirm: bool) -> DriverResult<MdtStatus> {
        confirmation(confirm)?;
        self.require_capabilities(&[
            "msvoltage=",
            "allvoltage=",
            "msenable=",
            "msvoltage?",
            "msenable?",
            "xvoltage?",
            "yvoltage?",
            "zvoltage?",
        ])?;
        self.request_budget(
            Op::EmergencyZero,
            self.config().io_timeout.saturating_mul(60),
            true,
        )
    }
    pub fn select_previous_channel(&self) -> DriverResult<Axis> {
        self.motion(Motion::Select("left"))?
            .selected_channel
            .ok_or_else(|| DriverError::Protocol("MDT no confirmed selection".into()))
    }
    pub fn select_next_channel(&self) -> DriverResult<Axis> {
        self.motion(Motion::Select("right"))?
            .selected_channel
            .ok_or_else(|| DriverError::Protocol("MDT no confirmed selection".into()))
    }
    pub fn increment_selected(&self) -> DriverResult<MdtStatus> {
        self.motion(Motion::Arrow("up", 1))
    }
    pub fn decrement_selected(&self) -> DriverResult<MdtStatus> {
        self.motion(Motion::Arrow("down", -1))
    }
}
fn caps(io: &Io, m: Motion) -> DriverResult<()> {
    io.capabilities(&[
        "msenable?",
        "msvoltage?",
        "xvoltage?",
        "yvoltage?",
        "zvoltage?",
    ])?;
    match m {
        Motion::Axis(a, _) => io.capabilities(&[&format!("{}voltage=", a.token())]),
        Motion::All(_) => io.capabilities(&["allvoltage="]),
        Motion::Master(_) => io.capabilities(&["msvoltage="]),
        Motion::MasterEnabled(_) => io.capabilities(&["msenable="]),
        Motion::Zero => io.capabilities(&[
            "msvoltage=",
            "msenable=",
            "allvoltage=",
            "xvoltage=",
            "yvoltage=",
            "zvoltage=",
        ]),
        Motion::Select(name) | Motion::Arrow(name, _) => {
            if io.unsupported_arrows.contains(name) {
                return Err(DriverError::Invalid("MDT arrow latched unsupported".into()));
            }
            io.capabilities(&[name, "dacstep?", "vlimit?"])
        }
    }
}
fn bound(s: &Shared, st: &MdtStatus, a: Axis) -> (f64, f64) {
    (
        st.axes[&a].minimum_v,
        st.axes[&a]
            .maximum_v
            .min(st.hardware_limit.volts())
            .min(s.config.application_limits_v[a.index()])
            .min(75.),
    )
}
fn violation(v: f64, b: (f64, f64)) -> f64 {
    (b.0 - v).max(v - b.1).max(0.)
}
fn validate(
    s: &Shared,
    st: &MdtStatus,
    from: [f64; 3],
    to: [f64; 3],
    affected: &[Axis],
) -> DriverResult<()> {
    for &a in affected {
        let i = a.index();
        let b = bound(s, st, a);
        let old = violation(from[i], b);
        let new = violation(to[i], b);
        if (!st.restricted && new > 0.)
            || (st.restricted
                && (to[i] > from[i] + TOLERANCE
                    || (old > 0. && new >= old)
                    || (old <= TOLERANCE && new > TOLERANCE)))
        {
            return Err(DriverError::Invalid("MDT target exceeds effective bounds or is not a provable restricted-state reduction".into()));
        }
    }
    Ok(())
}
fn transition(
    s: &Shared,
    st: &MdtStatus,
    before: [f64; 3],
    after: [f64; 3],
    wanted: [f64; 3],
) -> DriverResult<()> {
    for a in AXES {
        let i = a.index();
        if (after[i] - before[i]).abs() > 0.1 + TOLERANCE
            || (after[i] - wanted[i]).abs() > TOLERANCE
            || violation(after[i], bound(s, st, a))
                > violation(before[i], bound(s, st, a)) + TOLERANCE
            || (st.restricted && after[i] > before[i] + TOLERANCE)
        {
            return Err(DriverError::Protocol(
                "MDT unconfirmed/unsafe actual transition; hold".into(),
            ));
        }
    }
    Ok(())
}
impl Io {
    fn live(&mut self, s: &Shared, g: u64, end: Deadline) -> DriverResult<MdtStatus> {
        let mut st = self.status.clone().ok_or(DriverError::Closed)?;
        st.master_scan_enabled = protocol::boolean(&self.query(s, "msenable?", g, end)?)?;
        st.master_scan_voltage_v = protocol::number(&self.query(s, "msvoltage?", g, end)?)?;
        for a in AXES {
            let n = protocol::number(&self.query(s, &format!("{}voltage?", a.token()), g, end)?)?;
            st.axes.get_mut(&a).unwrap().actual_v = n;
        }
        st.observed_at = s.clock.now().as_secs_f64();
        // New actual readings are not yet confirmed command effects.
        st.axis_command_known = false;
        st.baseline_evidence = None;
        if AXES
            .iter()
            .any(|a| violation(st.axes[a].actual_v, bound(s, &st, *a)) > TOLERANCE)
        {
            st.restricted = true;
            st.fault_evidence = Some("MDT motion actual outside effective interval; hold".into());
        }
        Ok(st)
    }
    fn fresh(&mut self, s: &Shared, g: u64, end: Deadline) -> DriverResult<MdtStatus> {
        let mut st = self.live(s, g, end)?;
        st.hardware_limit = match protocol::integer(&self.query(s, "vlimit?", g, end)?, 0, 150)? {
            0 | 75 => super::VoltageLimit::V75,
            1 | 100 => super::VoltageLimit::V100,
            2 | 150 => super::VoltageLimit::V150,
            _ => return Err(DriverError::Protocol("unknown MDT hardware limit".into())),
        };
        st.dac_step = protocol::integer(&self.query(s, "dacstep?", g, end)?, 1, 1000)? as u16;
        for axis in AXES {
            let minimum =
                protocol::number(&self.query(s, &format!("{}min?", axis.token()), g, end)?)?;
            let maximum =
                protocol::number(&self.query(s, &format!("{}max?", axis.token()), g, end)?)?;
            if minimum > maximum {
                return Err(DriverError::Protocol("MDT fresh limits inverted".into()));
            }
            st.axes.get_mut(&axis).unwrap().minimum_v = minimum;
            st.axes.get_mut(&axis).unwrap().maximum_v = maximum;
        }
        if AXES
            .iter()
            .any(|a| violation(st.axes[a].actual_v, bound(s, &st, *a)) > TOLERANCE)
        {
            st.restricted = true;
            st.fault_evidence = Some("MDT fresh motion limits exclude actual voltage".into());
        }
        let st = self.observe(s, st, false);
        publish(s, self, st.clone());
        Ok(st)
    }
    fn authority(&self, s: &Shared, g: u64) -> DriverResult<Authority> {
        guard(s, g)?;
        self.authority
            .clone()
            .filter(|a| a.generation == g)
            .ok_or_else(|| {
                DriverError::Invalid(
                    "MDT absolute motion disarmed; adopt baseline explicitly".into(),
                )
            })
    }
    fn spacing(&mut self, s: &Shared, g: u64, end: Deadline) -> DriverResult<()> {
        if let Some(last) = self.last_motion {
            let due = last + s.config.ramp_interval;
            while s.clock.now() < due {
                guard(s, g)?;
                end.remaining_millis()?;
                s.clock
                    .wait((due - s.clock.now()).min(Duration::from_millis(50)));
            }
        }
        guard(s, g)?;
        Ok(())
    }
    fn remember(
        &mut self,
        s: &Shared,
        g: u64,
        st: MdtStatus,
        commands: [f64; 3],
    ) -> DriverResult<MdtStatus> {
        guard(s, g)?;
        let old = self.authority(s, g)?;
        self.authority = Some(Authority {
            commands,
            actual: actual(&st),
            master: st.master_scan_voltage_v,
            enabled: st.master_scan_enabled,
            generation: g,
            evidence: old.evidence,
        });
        let mut st = st;
        st.axis_command_known = true;
        st.baseline_evidence = self.authority.as_ref().map(|a| a.evidence.clone());
        publish(s, self, st.clone());
        Ok(st)
    }
}
pub(super) fn run(
    s: &Shared,
    io: &mut Io,
    g: u64,
    end: Deadline,
    m: Motion,
) -> DriverResult<MdtStatus> {
    caps(io, m)?;
    if let Motion::Select(name) = m {
        return select(s, io, g, end, name);
    }
    io.authority(s, g)?;
    let snapshot = io.snapshot(s, g, end)?;
    let snapshot = io.observe(s, snapshot, false);
    publish(s, io, snapshot);
    io.authority(s, g)?;
    caps(io, m)?;
    if let Motion::Zero = m {
        if AXES
            .iter()
            .any(|a| bound(s, io.status.as_ref().unwrap(), *a).0 > 0.)
        {
            return Err(DriverError::Invalid(
                "MDT configured minima exclude zero".into(),
            ));
        }
        if io.status.as_ref().unwrap().master_scan_enabled
            && io.status.as_ref().unwrap().master_scan_voltage_v != 0.
        {
            ramp(s, io, g, end, Motion::Master(0.))?;
        }
        ramp(s, io, g, end, Motion::All(0.))?;
        if io.status.as_ref().unwrap().master_scan_enabled {
            enable(s, io, g, end, false)?;
        }
        let st = io.fresh(s, g, end)?;
        if actual(&st).iter().any(|v| v.abs() > TOLERANCE) {
            return Err(DriverError::Protocol(
                "MDT zero ramp has residual; hold and disarm".into(),
            ));
        }
        return Ok(st);
    }
    match m {
        Motion::MasterEnabled(enabled) => enable(s, io, g, end, enabled),
        Motion::Arrow(name, direction) => arrow(s, io, g, end, name, direction),
        _ => ramp(s, io, g, end, m),
    }
}
fn enable(
    s: &Shared,
    io: &mut Io,
    g: u64,
    end: Deadline,
    enabled: bool,
) -> DriverResult<MdtStatus> {
    let before = io.fresh(s, g, end)?;
    let a = io.authority(s, g)?;
    if before.master_scan_voltage_v.abs() > 1e-9 {
        return Err(DriverError::Invalid(
            "Master Scan enable changes require fresh zero contribution".into(),
        ));
    }
    validate(s, &before, actual(&before), actual(&before), &AXES)?;
    io.spacing(s, g, end)?;
    let before = io.fresh(s, g, end)?;
    io.authority(s, g)?;
    io.last_motion = Some(s.clock.now());
    io.write_confirmed(s, g, end, &format!("msenable={}", enabled as u8), false)?;
    let after = io.live(s, g, end)?;
    publish(s, io, after.clone());
    transition(s, &before, actual(&before), actual(&after), actual(&before))?;
    if after.master_scan_enabled != enabled || after.master_scan_voltage_v.abs() > TOLERANCE {
        return Err(DriverError::Protocol(
            "MDT Master Scan enable readback mismatch".into(),
        ));
    }
    io.remember(s, g, after, a.commands)
}
fn ramp(s: &Shared, io: &mut Io, g: u64, end: Deadline, m: Motion) -> DriverResult<MdtStatus> {
    let start = io.fresh(s, g, end)?;
    let a = io.authority(s, g)?;
    let mut targets = a.commands;
    let mut target_master = a.master;
    let affected: Vec<Axis> = match m {
        Motion::Axis(axis, v) => {
            targets[axis.index()] = v;
            vec![axis]
        }
        Motion::All(v) => {
            targets = [v; 3];
            AXES.to_vec()
        }
        Motion::Master(v) => {
            if !start.master_scan_enabled {
                return Err(DriverError::Invalid(
                    "Master Scan ramp requires fresh enabled state".into(),
                ));
            }
            target_master = v;
            AXES.to_vec()
        }
        _ => return Err(DriverError::Invalid("invalid typed ramp".into())),
    };
    let contribution = if a.enabled { a.master } else { 0. };
    let residual = std::array::from_fn::<_, 3, _>(|i| a.actual[i] - a.commands[i] - contribution);
    let expected_target = std::array::from_fn(|i| {
        targets[i] + if a.enabled { target_master } else { 0. } + residual[i]
    });
    validate(s, &start, a.actual, expected_target, &affected)?;
    let max_delta = targets
        .iter()
        .zip(a.commands)
        .map(|(x, y)| (*x - y).abs())
        .fold((target_master - a.master).abs(), f64::max);
    if max_delta <= 1e-12 {
        return Ok(start);
    }
    let count = (max_delta / s.config.ramp_step_v).ceil().max(1.) as usize;
    if count > 100_000 {
        return Err(DriverError::Invalid("MDT ramp step budget exceeded".into()));
    }
    let unequal = a
        .commands
        .iter()
        .any(|v| (*v - a.commands[0]).abs() > TOLERANCE);
    if matches!(m, Motion::All(_)) && unequal {
        io.capabilities(&["xvoltage=", "yvoltage=", "zvoltage="])?;
    }
    for index in 1..=count {
        let next = std::array::from_fn::<_, 3, _>(|i| {
            if index == count {
                targets[i]
            } else {
                a.commands[i] + (targets[i] - a.commands[i]) * (index as f64 / count as f64)
            }
        });
        let master = if index == count {
            target_master
        } else {
            a.master + (target_master - a.master) * (index as f64 / count as f64)
        };
        let command_axes: Vec<Axis> = match m {
            Motion::Axis(axis, _) => vec![axis],
            Motion::All(_) if unequal && index < count => AXES.to_vec(),
            _ => vec![],
        };
        if !command_axes.is_empty() {
            for axis in command_axes {
                io.spacing(s, g, end)?;
                let before = io.fresh(s, g, end)?;
                let authority = io.authority(s, g)?;
                let mut commands = authority.commands;
                commands[axis.index()] = next[axis.index()];
                let expected = std::array::from_fn(|i| {
                    commands[i]
                        + if authority.enabled {
                            authority.master
                        } else {
                            0.
                        }
                        + residual[i]
                });
                validate(s, &before, actual(&before), expected, &[axis])?;
                io.last_motion = Some(s.clock.now());
                io.write_confirmed(
                    s,
                    g,
                    end,
                    &format!("{}voltage={}", axis.token(), next[axis.index()]),
                    false,
                )?;
                let after = io.live(s, g, end)?;
                publish(s, io, after.clone());
                transition(s, &before, actual(&before), actual(&after), expected)?;
                if after.master_scan_enabled != authority.enabled
                    || (after.master_scan_voltage_v - authority.master).abs() > TOLERANCE
                {
                    return Err(DriverError::Protocol(
                        "MDT Master Scan changed during axis write".into(),
                    ));
                }
                io.remember(s, g, after, commands)?;
            }
        } else {
            io.spacing(s, g, end)?;
            let before = io.fresh(s, g, end)?;
            let authority = io.authority(s, g)?;
            let (command, commands, expected_master) = match m {
                Motion::Master(_) => (format!("msvoltage={master}"), authority.commands, master),
                Motion::All(_) => (format!("allvoltage={}", next[0]), next, authority.master),
                _ => unreachable!(),
            };
            let expected = std::array::from_fn(|i| {
                commands[i]
                    + if authority.enabled {
                        expected_master
                    } else {
                        0.
                    }
                    + residual[i]
            });
            validate(s, &before, actual(&before), expected, &affected)?;
            io.last_motion = Some(s.clock.now());
            io.write_confirmed(s, g, end, &command, false)?;
            let after = io.live(s, g, end)?;
            publish(s, io, after.clone());
            transition(s, &before, actual(&before), actual(&after), expected)?;
            if after.master_scan_enabled != authority.enabled
                || (after.master_scan_voltage_v - expected_master).abs() > TOLERANCE
            {
                return Err(DriverError::Protocol(
                    "MDT Master Scan readback mismatch during ramp".into(),
                ));
            }
            io.remember(s, g, after, commands)?;
        }
    }
    Ok(io.status.clone().unwrap())
}
fn selected(lines: &[String]) -> DriverResult<Axis> {
    match protocol::single(lines)?
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "X" => Ok(Axis::X),
        "Y" => Ok(Axis::Y),
        "Z" => Ok(Axis::Z),
        _ => Err(DriverError::Protocol(
            "MDT arrow selection unrecognized".into(),
        )),
    }
}
fn exchange_arrow(
    s: &Shared,
    io: &mut Io,
    g: u64,
    end: Deadline,
    name: &str,
) -> DriverResult<Vec<String>> {
    guard(s, g)?;
    io.wrote = true;
    let r = io
        .protocol
        .arrow(name, end.earlier(Deadline::after(s.config.io_timeout)));
    guard(s, g)?;
    if matches!(&r,Err(DriverError::Protocol(text))if text=="MDT command rejected") {
        io.unsupported_arrows.insert(name.into());
        io.wrote = false;
        return Err(DriverError::Invalid(
            "MDT firmware rejected fixed arrow candidate".into(),
        ));
    }
    r
}
fn select(s: &Shared, io: &mut Io, g: u64, end: Deadline, name: &str) -> DriverResult<MdtStatus> {
    let before = io.fresh(s, g, end)?;
    let lines = exchange_arrow(s, io, g, end, name)?;
    let selection = selected(&lines);
    let mut after = io.live(s, g, end)?;
    publish(s, io, after.clone());
    if actual(&before)
        .iter()
        .zip(actual(&after))
        .any(|(x, y)| (*x - y).abs() > TOLERANCE)
    {
        return Err(DriverError::Protocol(
            "MDT channel selection moved output; hold".into(),
        ));
    }
    after.selected_channel = Some(selection?);
    let after = io.observe(s, after, false);
    publish(s, io, after.clone());
    Ok(after)
}
fn arrow(
    s: &Shared,
    io: &mut Io,
    g: u64,
    end: Deadline,
    name: &str,
    direction: i8,
) -> DriverResult<MdtStatus> {
    io.spacing(s, g, end)?;
    let before = io.fresh(s, g, end)?;
    let authority = io.authority(s, g)?;
    let step = before.dac_step as f64 * before.hardware_limit.volts() / 65535.;
    if step > 0.1 {
        return Err(DriverError::Invalid(
            "MDT DAC-derived arrow step exceeds 0.1 V".into(),
        ));
    }
    let candidate = std::array::from_fn(|i| actual(&before)[i] + f64::from(direction) * step);
    validate(s, &before, actual(&before), candidate, &AXES)?;
    io.last_motion = Some(s.clock.now());
    let lines = exchange_arrow(s, io, g, end, name)?;
    let after = io.live(s, g, end)?;
    publish(s, io, after.clone());
    let changed: Vec<Axis> = AXES
        .into_iter()
        .filter(|a| (after.axes[a].actual_v - before.axes[a].actual_v).abs() > TOLERANCE)
        .collect();
    let axis = if lines.is_empty() {
        if changed.len() != 1 {
            return Err(DriverError::Protocol(
                "MDT prompt-only arrow did not identify one changed axis".into(),
            ));
        }
        changed[0]
    } else {
        selected(&lines)?
    };
    let mut expected = actual(&before);
    expected[axis.index()] += f64::from(direction) * step;
    transition(s, &before, actual(&before), actual(&after), expected)?;
    if after.master_scan_enabled != authority.enabled
        || (after.master_scan_voltage_v - authority.master).abs() > TOLERANCE
    {
        return Err(DriverError::Protocol(
            "MDT Master Scan changed during arrow".into(),
        ));
    }
    let mut commands = authority.commands;
    commands[axis.index()] += after.axes[&axis].actual_v - before.axes[&axis].actual_v;
    let mut after = after;
    after.selected_channel = Some(axis);
    io.remember(s, g, after, commands)
}
pub(super) fn emergency(s: &Shared, io: &mut Io, g: u64, end: Deadline) -> DriverResult<MdtStatus> {
    io.capabilities(&[
        "msvoltage=",
        "allvoltage=",
        "msenable=",
        "msvoltage?",
        "msenable?",
        "xvoltage?",
        "yvoltage?",
        "zvoltage?",
    ])?;
    io.invalidate("MDT explicit emergency zero in progress");
    for c in ["msvoltage=0", "allvoltage=0", "msenable=0"] {
        io.write_confirmed(s, g, end, c, false)?;
    }
    let mut after = io.snapshot(s, g, end)?;
    publish(s, io, after.clone());
    if after.master_scan_enabled
        || after.master_scan_voltage_v.abs() > TOLERANCE
        || actual(&after).iter().any(|v| v.abs() > TOLERANCE)
        || after.restricted
    {
        return Err(DriverError::Protocol(
            "MDT emergency zero not fully confirmed; hold disarmed".into(),
        ));
    }
    io.authority=Some(Authority{commands:[0.;3],actual:actual(&after),master:0.,enabled:false,generation:g,evidence:"Explicit emergency zero: all commands acknowledged and Master/XYZ read back zero (host evidence, not physical measurement)".into()});
    after.axis_command_known = true;
    after.baseline_evidence = io.authority.as_ref().map(|a| a.evidence.clone());
    after.fault_evidence = None;
    after.restricted = false;
    io.last_motion = Some(s.clock.now());
    publish(s, io, after.clone());
    s.state.lock().unwrap().state = DriverState::Ready;
    Ok(after)
}
