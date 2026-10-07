//! Reviewed PM400 operation matrix, with compile-time typed driver calls only.
use crate::{
    actions::{fields, flag, invalid},
    WorkerError,
};
use serde_json::{json, Value};
use yang_drivers::{pm400::*, transport::Deadline, DriverResult};
#[derive(Clone, Copy, Debug)]
pub enum Setting {
    RootIdentity,
    RootStandardEventEnable,
    RootStandardEventStatus,
    RootServiceRequestEnable,
    RootStatusByte,
    RootSelfTest,
    SystemBeeper,
    SystemDate,
    SystemTime,
    SystemLineFrequency,
    SystemScpiVersion,
    SystemSensor,
    SystemNextError,
    StatusEvent,
    StatusCondition,
    StatusPositive,
    StatusNegative,
    StatusEnable,
    DisplayBrightness,
    DisplayContrast,
    CalibrationString,
    SenseAverage,
    SenseZeroState,
    SenseZeroMagnitude,
    SenseFrequencyUpper,
    SenseFrequencyLower,
    SensePowerUnit,
    InputAdapter,
    MeasurementConfig,
    SenseLossDb,
    SenseBeamDiameterMm,
    SenseWavelengthNm,
    SensePhotodiodeResponseAPerW,
    SenseThermopileResponseVPerW,
    SensePyroResponseVPerJ,
    SenseCurrentRangeA,
    SenseCurrentReferenceA,
    SenseEnergyRangeJ,
    SenseEnergyReferenceJ,
    SensePowerRangeW,
    SensePowerReferenceW,
    SenseVoltageRangeV,
    SenseVoltageReferenceV,
    SensePeakThresholdPercent,
    SenseCurrentAutoRange,
    SenseCurrentDeltaEnabled,
    SenseEnergyDeltaEnabled,
    SensePowerAutoRange,
    SensePowerDeltaEnabled,
    SenseVoltageAutoRange,
    SenseVoltageDeltaEnabled,
    InputPhotodiodeLowpassEnabled,
    InputThermopileAcceleratorEnabled,
    InputThermopileAcceleratorAuto,
    InputTau,
}
pub struct SettingSpec {
    pub setting: Setting,
    pub key: &'static str,
    pub kind: &'static str,
    pub writable: bool,
    pub selectors: &'static [&'static str],
    pub sensitive: bool,
}
pub const SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        setting: Setting::RootIdentity,
        key: "root.identity",
        kind: "text",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::RootStandardEventEnable,
        key: "root.standard_event_enable",
        kind: "register8",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::RootStandardEventStatus,
        key: "root.standard_event_status",
        kind: "register8",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::RootServiceRequestEnable,
        key: "root.service_request_enable",
        kind: "register8",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::RootStatusByte,
        key: "root.status_byte",
        kind: "register8",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::RootSelfTest,
        key: "root.self_test",
        kind: "int",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SystemBeeper,
        key: "system.beeper_enabled",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SystemDate,
        key: "system.date",
        kind: "date",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SystemTime,
        key: "system.time",
        kind: "time",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SystemLineFrequency,
        key: "system.line_frequency_hz",
        kind: "line_frequency",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SystemScpiVersion,
        key: "system.scpi_version",
        kind: "text",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SystemSensor,
        key: "system.sensor_info",
        kind: "object",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SystemNextError,
        key: "system.next_error",
        kind: "object",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::StatusEvent,
        key: "status.event",
        kind: "register16",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::StatusCondition,
        key: "status.condition",
        kind: "register16",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::StatusPositive,
        key: "status.positive_transition",
        kind: "register16",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::StatusNegative,
        key: "status.negative_transition",
        kind: "register16",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::StatusEnable,
        key: "status.enable",
        kind: "register16",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::DisplayBrightness,
        key: "display.brightness",
        kind: "float",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::DisplayContrast,
        key: "display.contrast",
        kind: "float",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::CalibrationString,
        key: "calibration.string",
        kind: "text",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseAverage,
        key: "sense.average_count",
        kind: "int",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseZeroState,
        key: "sense.zero_state",
        kind: "bool",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseZeroMagnitude,
        key: "sense.zero_magnitude",
        kind: "float",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseFrequencyUpper,
        key: "sense.frequency_upper_hz",
        kind: "float",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseFrequencyLower,
        key: "sense.frequency_lower_hz",
        kind: "float",
        writable: false,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SensePowerUnit,
        key: "sense.power_unit",
        kind: "power_unit",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::InputAdapter,
        key: "input.adapter_type",
        kind: "adapter_type",
        writable: true,
        selectors: &[],
        sensitive: true,
    },
    SettingSpec {
        setting: Setting::MeasurementConfig,
        key: "measurement.configuration",
        kind: "measurement_kind",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseLossDb,
        key: "sense.loss_db",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseBeamDiameterMm,
        key: "sense.beam_diameter_mm",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseWavelengthNm,
        key: "sense.wavelength_nm",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SensePhotodiodeResponseAPerW,
        key: "sense.photodiode_response_a_per_w",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: true,
    },
    SettingSpec {
        setting: Setting::SenseThermopileResponseVPerW,
        key: "sense.thermopile_response_v_per_w",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: true,
    },
    SettingSpec {
        setting: Setting::SensePyroResponseVPerJ,
        key: "sense.pyro_response_v_per_j",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: true,
    },
    SettingSpec {
        setting: Setting::SenseCurrentRangeA,
        key: "sense.current_range_a",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseCurrentReferenceA,
        key: "sense.current_reference_a",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseEnergyRangeJ,
        key: "sense.energy_range_j",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseEnergyReferenceJ,
        key: "sense.energy_reference_j",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SensePowerRangeW,
        key: "sense.power_range_w",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SensePowerReferenceW,
        key: "sense.power_reference_w",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseVoltageRangeV,
        key: "sense.voltage_range_v",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseVoltageReferenceV,
        key: "sense.voltage_reference_v",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SensePeakThresholdPercent,
        key: "sense.peak_threshold_percent",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseCurrentAutoRange,
        key: "sense.current_auto_range",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseCurrentDeltaEnabled,
        key: "sense.current_delta_enabled",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseEnergyDeltaEnabled,
        key: "sense.energy_delta_enabled",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SensePowerAutoRange,
        key: "sense.power_auto_range",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SensePowerDeltaEnabled,
        key: "sense.power_delta_enabled",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseVoltageAutoRange,
        key: "sense.voltage_auto_range",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::SenseVoltageDeltaEnabled,
        key: "sense.voltage_delta_enabled",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::InputPhotodiodeLowpassEnabled,
        key: "input.photodiode_lowpass_enabled",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::InputThermopileAcceleratorEnabled,
        key: "input.thermopile_accelerator_enabled",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::InputThermopileAcceleratorAuto,
        key: "input.thermopile_accelerator_auto",
        kind: "bool",
        writable: true,
        selectors: &[],
        sensitive: false,
    },
    SettingSpec {
        setting: Setting::InputTau,
        key: "input.thermopile_tau_s",
        kind: "float",
        writable: true,
        selectors: &["minimum", "maximum", "default"],
        sensitive: false,
    },
];
#[derive(Clone, Copy, Debug)]
pub enum Maintenance {
    Clear,
    Mark,
    WaitComplete,
    Reset,
    WaitContinue,
    Cancel,
    Beep,
    Errors,
    Preset,
    Zero,
    AbortZero,
    Initiate,
    Abort,
    Fetch,
    Read,
}
pub const COMMANDS: &[(&str, Maintenance, bool)] = &[
    ("root.clear_status", Maintenance::Clear, true),
    ("root.mark_operation_complete", Maintenance::Mark, false),
    (
        "root.wait_operation_complete",
        Maintenance::WaitComplete,
        false,
    ),
    ("root.reset", Maintenance::Reset, true),
    ("root.wait_to_continue", Maintenance::WaitContinue, false),
    ("root.cancel_measurement", Maintenance::Cancel, false),
    ("system.beep", Maintenance::Beep, false),
    ("system.drain_errors", Maintenance::Errors, false),
    ("status.preset", Maintenance::Preset, true),
    ("sense.start_zero_collection", Maintenance::Zero, true),
    ("sense.abort_zero_collection", Maintenance::AbortZero, false),
    ("measurement.initiate", Maintenance::Initiate, false),
    ("measurement.abort", Maintenance::Abort, false),
    ("measurement.fetch", Maintenance::Fetch, false),
    ("measurement.read", Maintenance::Read, false),
];
#[derive(Clone, Copy, Debug)]
pub enum SettingValue {
    Empty,
    Float(f64),
    Integer(u32),
    Bool(bool),
    Date(Date),
    Time(Time),
    Power(PowerUnit),
    Adapter(AdapterType),
    Measurement(MeasurementKind),
}
impl SettingValue {
    fn integer(self) -> u32 {
        if let Self::Integer(v) = self {
            v
        } else {
            unreachable!("typed integer")
        }
    }
    fn float(self) -> f64 {
        if let Self::Float(v) = self {
            v
        } else {
            unreachable!("typed float")
        }
    }
    fn boolean(self) -> bool {
        if let Self::Bool(v) = self {
            v
        } else {
            unreachable!("typed bool")
        }
    }
    fn date(self) -> Date {
        if let Self::Date(v) = self {
            v
        } else {
            unreachable!("typed date")
        }
    }
    fn time(self) -> Time {
        if let Self::Time(v) = self {
            v
        } else {
            unreachable!("typed time")
        }
    }
    fn power(self) -> PowerUnit {
        if let Self::Power(v) = self {
            v
        } else {
            unreachable!("typed unit")
        }
    }
    fn adapter(self) -> AdapterType {
        if let Self::Adapter(v) = self {
            v
        } else {
            unreachable!("typed adapter")
        }
    }
    fn measurement(self) -> MeasurementKind {
        if let Self::Measurement(v) = self {
            v
        } else {
            unreachable!("typed kind")
        }
    }
    fn numeric(self, s: Option<LimitSelector>) -> NumericValue {
        match s {
            Some(s) => s.into(),
            None => self.float().into(),
        }
    }
}
#[derive(Clone, Debug)]
pub enum PmAction {
    Measure(MeasurementKind),
    Setting {
        setting: Setting,
        write: bool,
        value: SettingValue,
        selector: Option<LimitSelector>,
        group: Option<StatusGroup>,
        confirm: bool,
    },
    Maintenance(Maintenance, bool),
}
pub fn measurement_kind(s: &str) -> Result<MeasurementKind, WorkerError> {
    Ok(match s {
        "power" => MeasurementKind::Power,
        "current" => MeasurementKind::Current,
        "voltage" => MeasurementKind::Voltage,
        "energy" => MeasurementKind::Energy,
        "frequency" => MeasurementKind::Frequency,
        "power_density" => MeasurementKind::PowerDensity,
        "energy_density" => MeasurementKind::EnergyDensity,
        "resistance" => MeasurementKind::Resistance,
        "temperature" => MeasurementKind::Temperature,
        _ => return Err(invalid("unknown PM400 measurement kind")),
    })
}
pub fn kind_key(k: MeasurementKind) -> &'static str {
    match k {
        MeasurementKind::Power => "power",
        MeasurementKind::Current => "current",
        MeasurementKind::Voltage => "voltage",
        MeasurementKind::Energy => "energy",
        MeasurementKind::Frequency => "frequency",
        MeasurementKind::PowerDensity => "power_density",
        MeasurementKind::EnergyDensity => "energy_density",
        MeasurementKind::Resistance => "resistance",
        MeasurementKind::Temperature => "temperature",
    }
}
pub fn parse(name: &str, a: &Value) -> Result<PmAction, WorkerError> {
    match name {
        "measure_power" => {
            fields(a, &[], &[])?;
            Ok(PmAction::Measure(MeasurementKind::Power))
        }
        "measure_kind" => {
            fields(a, &["kind"], &["kind"])?;
            Ok(PmAction::Measure(measurement_kind(
                a["kind"]
                    .as_str()
                    .ok_or_else(|| invalid("measurement kind required"))?,
            )?))
        }
        "run_maintenance" => {
            fields(a, &["command", "confirm"], &["command"])?;
            let key = a["command"]
                .as_str()
                .ok_or_else(|| invalid("maintenance command required"))?;
            let (_, c, sensitive) = COMMANDS
                .iter()
                .find(|(k, _, _)| *k == key)
                .ok_or_else(|| invalid("unknown maintenance command"))?;
            let confirm = if a.get("confirm").is_some() {
                flag(a, "confirm")?
            } else {
                false
            };
            if *sensitive && !confirm {
                return Err(invalid("maintenance requires explicit confirm=true"));
            }
            Ok(PmAction::Maintenance(*c, confirm))
        }
        "read_setting" | "write_setting" => {
            let write = name == "write_setting";
            fields(
                a,
                if write {
                    &["setting", "value", "selector", "group", "confirm"]
                } else {
                    &["setting", "selector", "group"]
                },
                &["setting"],
            )?;
            let spec = SETTINGS
                .iter()
                .find(|s| Some(s.key) == a["setting"].as_str())
                .ok_or_else(|| invalid("unknown PM400 setting"))?;
            if write && !spec.writable {
                return Err(invalid("setting is read-only"));
            }
            let confirm = if a.get("confirm").is_some() {
                flag(a, "confirm")?
            } else {
                false
            };
            if write && spec.sensitive && !confirm {
                return Err(invalid("setting requires explicit confirm=true"));
            }
            let group = if spec.key.starts_with("status.") {
                Some(match a["group"].as_str() {
                    Some("measurement" | "MEASUREMENT") => StatusGroup::Measurement,
                    Some("auxiliary" | "AUXILIARY") => StatusGroup::Auxiliary,
                    Some("operation" | "OPERATION") => StatusGroup::Operation,
                    Some("questionable" | "QUESTIONABLE") => StatusGroup::Questionable,
                    _ => return Err(invalid("status group required")),
                })
            } else {
                if a.get("group").is_some() {
                    return Err(invalid("unexpected status group"));
                }
                None
            };
            let selector = if let Some(s) = a.get("selector") {
                let s = s
                    .as_str()
                    .filter(|s| spec.selectors.contains(s))
                    .ok_or_else(|| invalid("selector not supported for setting"))?;
                Some(match s {
                    "minimum" => LimitSelector::Minimum,
                    "maximum" => LimitSelector::Maximum,
                    _ => LimitSelector::Default,
                })
            } else {
                None
            };
            if write && selector.is_some() && a.get("value").is_some() {
                return Err(invalid("choose value or selector, not both"));
            }
            let value = if !write || selector.is_some() {
                SettingValue::Empty
            } else {
                typed_value(spec, &a["value"])?
            };
            Ok(PmAction::Setting {
                setting: spec.setting,
                write,
                value,
                selector,
                group,
                confirm,
            })
        }
        _ => Err(WorkerError::new(
            "UnsupportedAction",
            "PM400 action is not exposed",
        )),
    }
}
fn typed_value(s: &SettingSpec, v: &Value) -> Result<SettingValue, WorkerError> {
    Ok(match s.kind {
        "bool" => SettingValue::Bool(
            v.as_bool()
                .ok_or_else(|| invalid("boolean setting required"))?,
        ),
        "float" => SettingValue::Float(
            v.as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| invalid("finite setting required"))?,
        ),
        "int" | "register8" | "register16" | "line_frequency" => {
            let n = v
                .as_u64()
                .filter(|n| *n <= u32::MAX as u64)
                .ok_or_else(|| invalid("unsigned integer setting required"))?
                as u32;
            if (s.kind == "register8" && n > 255)
                || (s.kind == "register16" && n > 65535)
                || (s.kind == "int" && n == 0)
                || (s.kind == "line_frequency" && !matches!(n, 50 | 60))
            {
                return Err(invalid("integer outside setting limits"));
            }
            SettingValue::Integer(n)
        }
        "date" => {
            let t = v.as_str().ok_or_else(|| invalid("ISO date required"))?;
            let p: Vec<_> = t.split('-').collect();
            if p.len() != 3
                || p[0].len() != 4
                || p[1].len() != 2
                || p[2].len() != 2
                || p.iter().any(|s| !s.bytes().all(|c| c.is_ascii_digit()))
            {
                return Err(invalid("ISO date required"));
            }
            SettingValue::Date(Date::new(
                p[0].parse().map_err(|_| invalid("date year"))?,
                p[1].parse().map_err(|_| invalid("date month"))?,
                p[2].parse().map_err(|_| invalid("date day"))?,
            )?)
        }
        "time" => {
            let t = v
                .as_str()
                .ok_or_else(|| invalid("local ISO time required"))?;
            let p: Vec<_> = t.split(':').collect();
            if p.len() != 3 || p[0].len() != 2 || p[1].len() != 2 {
                return Err(invalid("HH:MM:SS time required"));
            }
            let (sec, f) = p[2]
                .split_once('.')
                .map_or((p[2], None), |(s, f)| (s, Some(f)));
            if sec.len() != 2
                || [p[0], p[1], sec]
                    .iter()
                    .any(|s| !s.bytes().all(|c| c.is_ascii_digit()))
                || f.is_some_and(|f| {
                    f.is_empty() || f.len() > 6 || !f.bytes().all(|c| c.is_ascii_digit())
                })
            {
                return Err(invalid("local ISO time required"));
            }
            let micros = if let Some(f) = f {
                f.parse::<u32>().unwrap() * 10u32.pow(6 - f.len() as u32)
            } else {
                0
            };
            SettingValue::Time(Time::new(
                p[0].parse().map_err(|_| invalid("hour"))?,
                p[1].parse().map_err(|_| invalid("minute"))?,
                sec.parse().map_err(|_| invalid("second"))?,
                micros,
            )?)
        }
        "power_unit" => SettingValue::Power(match v.as_str() {
            Some("W") => PowerUnit::Watts,
            Some("DBM") => PowerUnit::Dbm,
            _ => return Err(invalid("power unit must be W or DBM")),
        }),
        "adapter_type" => SettingValue::Adapter(match v.as_str() {
            Some("photodiode") => AdapterType::Photodiode,
            Some("thermal") => AdapterType::Thermal,
            Some("pyro") => AdapterType::Pyro,
            _ => return Err(invalid("unknown adapter type")),
        }),
        "measurement_kind" => SettingValue::Measurement(measurement_kind(
            v.as_str()
                .ok_or_else(|| invalid("measurement kind required"))?,
        )?),
        _ => return Err(invalid("setting is not writable")),
    })
}
pub fn execute(d: &mut Pm400, action: PmAction, deadline: Deadline) -> DriverResult<Value> {
    match action {
        PmAction::Measure(k) => {
            let m = d.measure(k, deadline)?;
            let mut v = json!(m);
            v["kind"] = json!(kind_key(m.kind));
            Ok(v)
        }
        PmAction::Maintenance(c, confirm) => {
            match c {
                Maintenance::Clear => d.clear_status()?,
                Maintenance::Mark => d.mark_operation_complete()?,
                Maintenance::WaitComplete => d.wait_operation_complete(deadline)?,
                Maintenance::Reset => d.reset(confirm)?,
                Maintenance::WaitContinue => d.wait_to_continue()?,
                Maintenance::Cancel => d.cancel_measurement(),
                Maintenance::Beep => d.system().beep()?,
                Maintenance::Errors => return Ok(json!(d.system().drain_errors(64)?)),
                Maintenance::Preset => d.status().preset(confirm)?,
                Maintenance::Zero => d.sense().start_zero_collection(confirm)?,
                Maintenance::AbortZero => d.sense().abort_zero_collection()?,
                Maintenance::Initiate => d.initiate()?,
                Maintenance::Abort => d.abort()?,
                Maintenance::Fetch => return Ok(json!(d.fetch_configured()?)),
                Maintenance::Read => return Ok(json!(d.read_configured(deadline)?)),
            }
            Ok(Value::Null)
        }
        PmAction::Setting {
            setting,
            write,
            value: v,
            selector: s,
            group: g,
            confirm,
        } => match setting {
            Setting::RootIdentity => Ok(json!((d.identify())?)),
            Setting::RootStandardEventEnable => Ok(json!(
                (if write {
                    d.set_standard_event_enable(v.integer())
                } else {
                    d.get_standard_event_enable()
                })?
            )),
            Setting::RootStandardEventStatus => Ok(json!((d.read_standard_event_status())?)),
            Setting::RootServiceRequestEnable => Ok(json!(
                (if write {
                    d.set_service_request_enable(v.integer())
                } else {
                    d.get_service_request_enable()
                })?
            )),
            Setting::RootStatusByte => Ok(json!((d.read_status_byte())?)),
            Setting::RootSelfTest => Ok(json!((d.self_test())?)),
            Setting::SystemBeeper => Ok(json!(
                (if write {
                    d.system().set_beeper_enabled(v.boolean())
                } else {
                    d.system().get_beeper_enabled()
                })?
            )),
            Setting::SystemDate => {
                let x = (if write {
                    d.system().set_date(v.date())
                } else {
                    d.system().get_date()
                })?;
                Ok(json!(format!(
                    "{:04}-{:02}-{:02}",
                    x.year(),
                    x.month(),
                    x.day()
                )))
            }
            Setting::SystemTime => {
                let x = (if write {
                    d.system().set_time(v.time())
                } else {
                    d.system().get_time()
                })?;
                Ok(json!(format!(
                    "{:02}:{:02}:{:02}.{:06}",
                    x.hour(),
                    x.minute(),
                    x.second(),
                    x.microsecond()
                )))
            }
            Setting::SystemLineFrequency => Ok(json!(
                (if write {
                    d.system().set_line_frequency_hz(v.integer())
                } else {
                    d.system().get_line_frequency_hz()
                })?
            )),
            Setting::SystemScpiVersion => Ok(json!((d.system().get_scpi_version())?)),
            Setting::SystemSensor => Ok(json!((d.system().get_sensor_info())?)),
            Setting::SystemNextError => Ok(json!((d.system().next_error())?)),
            Setting::StatusEvent => Ok(json!((d.status().read_event(g.unwrap()))?)),
            Setting::StatusCondition => Ok(json!((d.status().read_condition(g.unwrap()))?)),
            Setting::StatusPositive => Ok(json!(
                (if write {
                    d.status().set_positive_transition(g.unwrap(), v.integer())
                } else {
                    d.status().get_positive_transition(g.unwrap())
                })?
            )),
            Setting::StatusNegative => Ok(json!(
                (if write {
                    d.status().set_negative_transition(g.unwrap(), v.integer())
                } else {
                    d.status().get_negative_transition(g.unwrap())
                })?
            )),
            Setting::StatusEnable => Ok(json!(
                (if write {
                    d.status().set_enable(g.unwrap(), v.integer())
                } else {
                    d.status().get_enable(g.unwrap())
                })?
            )),
            Setting::DisplayBrightness => Ok(json!(
                (if write {
                    d.display().set_brightness(v.float())
                } else {
                    d.display().get_brightness()
                })?
            )),
            Setting::DisplayContrast => Ok(json!(
                (if write {
                    d.display().set_contrast(v.float())
                } else {
                    d.display().get_contrast()
                })?
            )),
            Setting::CalibrationString => Ok(json!((d.calibration().get_string())?)),
            Setting::SenseAverage => Ok(json!(
                (if write {
                    d.sense().set_average_count(v.integer())
                } else {
                    d.sense().get_average_count()
                })?
            )),
            Setting::SenseZeroState => Ok(json!((d.sense().get_zero_state())?)),
            Setting::SenseZeroMagnitude => Ok(json!((d.sense().get_zero_magnitude())?)),
            Setting::SenseFrequencyUpper => Ok(json!((d.sense().get_frequency_upper_hz())?)),
            Setting::SenseFrequencyLower => Ok(json!((d.sense().get_frequency_lower_hz())?)),
            Setting::SensePowerUnit => Ok(json!((if write {
                d.sense().set_power_unit(v.power())
            } else {
                d.sense().get_power_unit()
            })?
            .as_str())),
            Setting::InputAdapter => {
                let x = (if write {
                    d.input().set_adapter_type(v.adapter(), confirm)
                } else {
                    d.input().get_adapter_type()
                })?;
                Ok(json!(match x {
                    AdapterType::Photodiode => "photodiode",
                    AdapterType::Thermal => "thermal",
                    AdapterType::Pyro => "pyro",
                }))
            }
            Setting::MeasurementConfig => Ok(json!(kind_key(
                (if write {
                    d.configure(v.measurement())
                } else {
                    d.get_configuration()
                })?
            ))),
            Setting::SenseLossDb => Ok(json!(
                (if write {
                    d.sense().set_loss_db(v.numeric(s))
                } else {
                    d.sense().get_loss_db(s)
                })?
            )),
            Setting::SenseBeamDiameterMm => Ok(json!(
                (if write {
                    d.sense().set_beam_diameter_mm(v.numeric(s))
                } else {
                    d.sense().get_beam_diameter_mm(s)
                })?
            )),
            Setting::SenseWavelengthNm => Ok(json!(
                (if write {
                    d.sense().set_wavelength_nm(v.numeric(s))
                } else {
                    d.sense().get_wavelength_nm(s)
                })?
            )),
            Setting::SensePhotodiodeResponseAPerW => Ok(json!(
                (if write {
                    d.sense()
                        .set_photodiode_response_a_per_w(v.numeric(s), confirm)
                } else {
                    d.sense().get_photodiode_response_a_per_w(s)
                })?
            )),
            Setting::SenseThermopileResponseVPerW => Ok(json!(
                (if write {
                    d.sense()
                        .set_thermopile_response_v_per_w(v.numeric(s), confirm)
                } else {
                    d.sense().get_thermopile_response_v_per_w(s)
                })?
            )),
            Setting::SensePyroResponseVPerJ => Ok(json!(
                (if write {
                    d.sense().set_pyro_response_v_per_j(v.numeric(s), confirm)
                } else {
                    d.sense().get_pyro_response_v_per_j(s)
                })?
            )),
            Setting::SenseCurrentRangeA => Ok(json!(
                (if write {
                    d.sense().set_current_range_a(v.numeric(s))
                } else {
                    d.sense().get_current_range_a(s)
                })?
            )),
            Setting::SenseCurrentReferenceA => Ok(json!(
                (if write {
                    d.sense().set_current_reference_a(v.numeric(s))
                } else {
                    d.sense().get_current_reference_a(s)
                })?
            )),
            Setting::SenseEnergyRangeJ => Ok(json!(
                (if write {
                    d.sense().set_energy_range_j(v.numeric(s))
                } else {
                    d.sense().get_energy_range_j(s)
                })?
            )),
            Setting::SenseEnergyReferenceJ => Ok(json!(
                (if write {
                    d.sense().set_energy_reference_j(v.numeric(s))
                } else {
                    d.sense().get_energy_reference_j(s)
                })?
            )),
            Setting::SensePowerRangeW => Ok(json!(
                (if write {
                    d.sense().set_power_range_w(v.numeric(s))
                } else {
                    d.sense().get_power_range_w(s)
                })?
            )),
            Setting::SensePowerReferenceW => Ok(json!(
                (if write {
                    d.sense().set_power_reference_w(v.numeric(s))
                } else {
                    d.sense().get_power_reference_w(s)
                })?
            )),
            Setting::SenseVoltageRangeV => Ok(json!(
                (if write {
                    d.sense().set_voltage_range_v(v.numeric(s))
                } else {
                    d.sense().get_voltage_range_v(s)
                })?
            )),
            Setting::SenseVoltageReferenceV => Ok(json!(
                (if write {
                    d.sense().set_voltage_reference_v(v.numeric(s))
                } else {
                    d.sense().get_voltage_reference_v(s)
                })?
            )),
            Setting::SensePeakThresholdPercent => Ok(json!(
                (if write {
                    d.sense().set_peak_threshold_percent(v.numeric(s))
                } else {
                    d.sense().get_peak_threshold_percent(s)
                })?
            )),
            Setting::SenseCurrentAutoRange => Ok(json!(
                (if write {
                    d.sense().set_current_auto_range(v.boolean())
                } else {
                    d.sense().get_current_auto_range()
                })?
            )),
            Setting::SenseCurrentDeltaEnabled => Ok(json!(
                (if write {
                    d.sense().set_current_delta_enabled(v.boolean())
                } else {
                    d.sense().get_current_delta_enabled()
                })?
            )),
            Setting::SenseEnergyDeltaEnabled => Ok(json!(
                (if write {
                    d.sense().set_energy_delta_enabled(v.boolean())
                } else {
                    d.sense().get_energy_delta_enabled()
                })?
            )),
            Setting::SensePowerAutoRange => Ok(json!(
                (if write {
                    d.sense().set_power_auto_range(v.boolean())
                } else {
                    d.sense().get_power_auto_range()
                })?
            )),
            Setting::SensePowerDeltaEnabled => Ok(json!(
                (if write {
                    d.sense().set_power_delta_enabled(v.boolean())
                } else {
                    d.sense().get_power_delta_enabled()
                })?
            )),
            Setting::SenseVoltageAutoRange => Ok(json!(
                (if write {
                    d.sense().set_voltage_auto_range(v.boolean())
                } else {
                    d.sense().get_voltage_auto_range()
                })?
            )),
            Setting::SenseVoltageDeltaEnabled => Ok(json!(
                (if write {
                    d.sense().set_voltage_delta_enabled(v.boolean())
                } else {
                    d.sense().get_voltage_delta_enabled()
                })?
            )),
            Setting::InputPhotodiodeLowpassEnabled => Ok(json!(
                (if write {
                    d.input().set_photodiode_lowpass_enabled(v.boolean())
                } else {
                    d.input().get_photodiode_lowpass_enabled()
                })?
            )),
            Setting::InputThermopileAcceleratorEnabled => Ok(json!(
                (if write {
                    d.input().set_thermopile_accelerator_enabled(v.boolean())
                } else {
                    d.input().get_thermopile_accelerator_enabled()
                })?
            )),
            Setting::InputThermopileAcceleratorAuto => Ok(json!(
                (if write {
                    d.input().set_thermopile_accelerator_auto(v.boolean())
                } else {
                    d.input().get_thermopile_accelerator_auto()
                })?
            )),
            Setting::InputTau => Ok(json!(
                (if write {
                    d.input().set_thermopile_tau_s(v.numeric(s))
                } else {
                    d.input().get_thermopile_tau_s(s)
                })?
            )),
        },
    }
}
fn requirements(key: &str, write: bool) -> Vec<&'static str> {
    if key == "sense.wavelength_nm" {
        return if write {
            vec!["wavelength_settable"]
        } else {
            vec![]
        };
    }
    if key == "input.thermopile_tau_s" {
        return if write {
            vec!["power", "tau_settable"]
        } else {
            vec!["power"]
        };
    }
    let energy = key.contains("energy_")
        || key == "sense.peak_threshold_percent"
        || key == "sense.pyro_response_v_per_j";
    let power = key.starts_with("sense.current_")
        || key.starts_with("sense.power_")
        || key.starts_with("sense.voltage_")
        || key == "sense.photodiode_response_a_per_w"
        || key == "sense.thermopile_response_v_per_w"
        || key.starts_with("input.photodiode_")
        || key.starts_with("input.thermopile_accelerator_");
    let mut r = if energy {
        vec!["energy"]
    } else if power {
        vec!["power"]
    } else {
        vec![]
    };
    if write && key.contains("response_") {
        r.push("response_settable");
    }
    r
}
pub fn catalog(d: &Pm400) -> Value {
    let supported = |r: &[&str]| {
        r.iter().all(|n| {
            d.sensor_info().is_some_and(|s| match *n {
                "power" => s.capabilities.power,
                "energy" => s.capabilities.energy,
                "response_settable" => s.capabilities.response_settable,
                "wavelength_settable" => s.capabilities.wavelength_settable,
                "tau_settable" => s.capabilities.tau_settable,
                "temperature_sensor" => s.capabilities.temperature_sensor,
                _ => false,
            })
        })
    };
    let measurements:Vec<_>=["power","current","voltage","energy","frequency","power_density","energy_density","resistance","temperature"].iter().map(|key|{let k=measurement_kind(key).unwrap();let r=match *key{"power"|"power_density"=>vec!["power"],"energy"|"energy_density"=>vec!["energy"],"temperature"=>vec!["temperature_sensor"],_=>vec![]};json!({"key":key,"label":key.replace('_'," "),"unit":k.default_unit(),"supported":supported(&r),"requirements":r})}).collect();
    let settings:Vec<_>=SETTINGS.iter().map(|s|{let read=requirements(s.key,false);let write=requirements(s.key,true);json!({"key":s.key,"section":s.key.split('.').next().unwrap(),"label":s.key.split('.').nth(1).unwrap().replace('_'," "),"kind":s.kind,"reader":true,"writer":s.writable,"readable":true,"writable":s.writable,"read_supported":supported(&read),"write_supported":s.writable&&supported(&write),"supported":supported(if s.writable{&write}else{&read}),"read_requirements":read,"write_requirements":write,"selectors":s.selectors,"sensitive":s.sensitive,"grouped":s.key.starts_with("status."),"choices":if s.kind=="measurement_kind"{measurements.clone()}else{vec![]}})}).collect();
    let commands:Vec<_>=COMMANDS.iter().map(|(key,_,confirm)|json!({"key":key,"section":key.split('.').next().unwrap(),"label":key.split('.').nth(1).unwrap().replace('_'," "),"confirm":confirm,"supported":if *key=="sense.start_zero_collection"{supported(&["power"])||supported(&["energy"])}else{true},"requirements":[]})).collect();
    json!({"measurements":measurements,"settings":settings,"commands":commands})
}
