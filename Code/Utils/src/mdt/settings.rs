use super::{
    protocol,
    session::{guard, publish, Io, Mdt693b, Op, Shared},
    Axis, AxisState, MdtStatus, RotaryMode, VoltageLimit,
};
use crate::{transport::Deadline, DriverError, DriverResult};
#[derive(Clone)]
pub(super) enum Setting {
    Friendly(String),
    Echo(bool),
    Intensity(u8),
    Dac(u16),
    Compatibility(bool),
    Rotary(RotaryMode),
    PushDisabled(bool),
    Minimum(Axis, f64),
    Maximum(Axis, f64),
}
pub(super) fn confirmation(confirm: bool) -> DriverResult<()> {
    if confirm {
        Ok(())
    } else {
        Err(DriverError::Invalid(
            "explicit operator confirmation required".into(),
        ))
    }
}
pub(super) fn voltage(v: f64) -> DriverResult<()> {
    if v.is_finite() && (0.0..=75.).contains(&v) {
        Ok(())
    } else {
        Err(DriverError::Invalid(
            "MDT command voltage outside 0..75 V".into(),
        ))
    }
}
impl Mdt693b {
    pub fn get_friendly_name(&self) -> DriverResult<String> {
        Ok(self.refresh()?.friendly_name)
    }
    pub fn get_echo_enabled(&self) -> DriverResult<bool> {
        Ok(self.refresh()?.echo_enabled)
    }
    pub fn get_hardware_voltage_limit(&self) -> DriverResult<VoltageLimit> {
        Ok(self.refresh()?.hardware_limit)
    }
    pub fn get_display_intensity(&self) -> DriverResult<u8> {
        Ok(self.refresh()?.display_intensity)
    }
    pub fn get_dac_step(&self) -> DriverResult<u16> {
        Ok(self.refresh()?.dac_step)
    }
    pub fn get_compatibility_enabled(&self) -> DriverResult<bool> {
        Ok(self.refresh()?.compatibility_enabled)
    }
    pub fn get_rotary_mode(&self) -> DriverResult<RotaryMode> {
        Ok(self.refresh()?.rotary_mode)
    }
    pub fn get_push_to_adjust_disabled(&self) -> DriverResult<bool> {
        Ok(self.refresh()?.push_to_adjust_disabled)
    }
    pub fn get_master_scan_enabled(&self) -> DriverResult<bool> {
        Ok(self.refresh()?.master_scan_enabled)
    }
    pub fn get_master_scan_voltage(&self) -> DriverResult<f64> {
        Ok(self.refresh()?.master_scan_voltage_v)
    }
    pub fn set_friendly_name(&self, v: &str) -> DriverResult<String> {
        if v.is_empty() || v.len() > 119 || !v.is_ascii() || v.contains(['\n', '\r', '\0']) {
            return Err(DriverError::Invalid(
                "MDT friendly name must be bounded one-line ASCII".into(),
            ));
        }
        Ok(self
            .request(Op::Setting(Setting::Friendly(v.into())))?
            .friendly_name)
    }
    pub fn set_echo_enabled(&self, v: bool) -> DriverResult<bool> {
        Ok(self.request(Op::Setting(Setting::Echo(v)))?.echo_enabled)
    }
    pub fn set_display_intensity(&self, v: u8) -> DriverResult<u8> {
        if v > 15 {
            return Err(DriverError::Invalid("intensity outside 0..15".into()));
        }
        Ok(self
            .request(Op::Setting(Setting::Intensity(v)))?
            .display_intensity)
    }
    pub fn set_dac_step(&self, v: u16) -> DriverResult<u16> {
        if !(1..=1000).contains(&v) {
            return Err(DriverError::Invalid("DAC step outside 1..1000".into()));
        }
        Ok(self.request(Op::Setting(Setting::Dac(v)))?.dac_step)
    }
    pub fn set_compatibility_enabled(&self, v: bool) -> DriverResult<bool> {
        Ok(self
            .request(Op::Setting(Setting::Compatibility(v)))?
            .compatibility_enabled)
    }
    pub fn set_rotary_mode(&self, v: RotaryMode) -> DriverResult<RotaryMode> {
        Ok(self.request(Op::Setting(Setting::Rotary(v)))?.rotary_mode)
    }
    pub fn set_push_to_adjust_disabled(&self, v: bool) -> DriverResult<bool> {
        Ok(self
            .request(Op::Setting(Setting::PushDisabled(v)))?
            .push_to_adjust_disabled)
    }
    pub fn set_axis_minimum(&self, a: Axis, v: f64) -> DriverResult<AxisState> {
        voltage(v)?;
        Ok(self.request(Op::Setting(Setting::Minimum(a, v)))?.axes[&a])
    }
    pub fn set_axis_maximum(&self, a: Axis, v: f64) -> DriverResult<AxisState> {
        voltage(v)?;
        Ok(self.request(Op::Setting(Setting::Maximum(a, v)))?.axes[&a])
    }
    pub fn restore_factory_defaults(&self, confirm: bool) -> DriverResult<MdtStatus> {
        confirmation(confirm)?;
        self.require_capabilities(&["restore"])?;
        self.request_budget(
            Op::Restore,
            self.config().io_timeout.saturating_mul(60),
            true,
        )
    }
    pub(super) fn require_capabilities(&self, names: &[&str]) -> DriverResult<()> {
        let status = self.status().ok_or(DriverError::Closed)?;
        for name in names {
            if !status.supported_commands.contains(*name) {
                return Err(DriverError::Invalid(format!("MDT firmware lacks {name}")));
            }
        }
        Ok(())
    }
}
impl Io {
    pub(super) fn capabilities(&self, names: &[&str]) -> DriverResult<()> {
        let st = self.status.as_ref().ok_or(DriverError::Closed)?;
        for name in names {
            if !st.supported_commands.contains(*name) {
                return Err(DriverError::Invalid(format!("MDT firmware lacks {name}")));
            }
        }
        Ok(())
    }
    pub(super) fn write_confirmed(
        &mut self,
        s: &Shared,
        g: u64,
        end: Deadline,
        command: &str,
        transition: bool,
    ) -> DriverResult<()> {
        guard(s, g)?;
        self.wrote = true;
        let d = end.earlier(Deadline::after(s.config.io_timeout));
        let lines = if transition {
            self.protocol.transition(command, d)?
        } else {
            self.protocol.exchange(command, d)?
        };
        guard(s, g)?;
        if !lines.is_empty() {
            return Err(DriverError::Protocol(
                "MDT setter returned unexpected result".into(),
            ));
        }
        Ok(())
    }
}
pub(super) fn run(
    s: &Shared,
    io: &mut Io,
    g: u64,
    end: Deadline,
    setting: Setting,
) -> DriverResult<MdtStatus> {
    let (name, value) = match &setting {
        Setting::Friendly(v) => ("friendly".into(), v.clone()),
        Setting::Echo(v) => ("echo".into(), (*v as u8).to_string()),
        Setting::Intensity(v) => ("intensity".into(), v.to_string()),
        Setting::Dac(v) => ("dacstep".into(), v.to_string()),
        Setting::Compatibility(v) => ("cm".into(), (*v as u8).to_string()),
        Setting::Rotary(v) => (
            "rotarymode".into(),
            match v {
                RotaryMode::Default => "0",
                RotaryMode::TenTurn => "1",
                RotaryMode::Fine => "2",
            }
            .into(),
        ),
        Setting::PushDisabled(v) => ("pushdisable".into(), (*v as u8).to_string()),
        Setting::Minimum(a, v) => (format!("{}min", a.token()), v.to_string()),
        Setting::Maximum(a, v) => (format!("{}max", a.token()), v.to_string()),
    };
    let setter = format!("{name}=");
    let query = format!("{name}?");
    io.capabilities(&[&setter, &query])?;
    let before = io.snapshot(s, g, end)?;
    let before = io.observe(s, before, false);
    publish(s, io, before.clone());
    io.capabilities(&[&setter, &query])?;
    if let Setting::Minimum(a, v) | Setting::Maximum(a, v) = &setting {
        let axis = before.axes[a];
        let ceiling = s.config.application_limits_v[a.index()].min(before.hardware_limit.volts());
        if *v > ceiling
            || matches!(setting, Setting::Minimum(_, _))
                && (*v > axis.actual_v || *v > axis.maximum_v)
            || matches!(setting, Setting::Maximum(_, _))
                && (*v < axis.actual_v || *v < axis.minimum_v)
        {
            return Err(DriverError::Invalid(
                "MDT axis limit cannot exclude actual output or relax project ceiling".into(),
            ));
        }
    }
    io.write_confirmed(
        s,
        g,
        end,
        &format!("{name}={value}"),
        matches!(setting, Setting::Echo(_)),
    )?;
    let lines = io.query(s, &query, g, end)?;
    let matched = match setting {
        Setting::Friendly(v) => protocol::single(&lines)? == v,
        Setting::Echo(v) | Setting::Compatibility(v) | Setting::PushDisabled(v) => {
            protocol::boolean(&lines)? == v
        }
        Setting::Intensity(v) => protocol::integer(&lines, 0, 15)? == i64::from(v),
        Setting::Dac(v) => protocol::integer(&lines, 1, 1000)? == i64::from(v),
        Setting::Rotary(v) => {
            protocol::integer(&lines, 0, 2)?
                == match v {
                    RotaryMode::Default => 0,
                    RotaryMode::TenTurn => 1,
                    RotaryMode::Fine => 2,
                }
        }
        Setting::Minimum(_, v) | Setting::Maximum(_, v) => {
            (protocol::number(&lines)? - v).abs() <= 1e-9_f64.max(v.abs() * 1e-9)
        }
    };
    if !matched {
        return Err(DriverError::Protocol(
            "MDT setter readback mismatch; hold".into(),
        ));
    }
    let after = io.snapshot(s, g, end)?;
    let after = io.observe(s, after, false);
    publish(s, io, after.clone());
    if super::authority::actual(&before)
        .iter()
        .zip(super::authority::actual(&after))
        .any(|(x, y)| (*x - y).abs() > super::authority::TOLERANCE)
    {
        return Err(DriverError::Protocol(
            "MDT non-motion setting changed actual output; hold".into(),
        ));
    }
    Ok(after)
}
pub(super) fn restore(s: &Shared, io: &mut Io, g: u64, end: Deadline) -> DriverResult<MdtStatus> {
    io.capabilities(&["restore"])?;
    io.invalidate("MDT factory restore disarms motion");
    io.write_confirmed(s, g, end, "restore", true)?;
    let mut st = io.snapshot(s, g, end)?;
    st.axis_command_known = false;
    if st.restricted {
        st.fault_evidence = Some("MDT restore limit violation; hold".into());
    }
    publish(s, io, st.clone());
    Ok(st)
}
