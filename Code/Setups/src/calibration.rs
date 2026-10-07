use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};
use yang_drivers::{mdt::Axis, transport::serial::canonical_com, DriverError, DriverResult};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StageSide {
    Left,
    Right,
}
impl StageSide {
    pub fn serial(self) -> &'static str {
        match self {
            Self::Left => "2110148249-10",
            Self::Right => "160721175410",
        }
    }
    pub fn toward_chip_sign(self) -> i8 {
        match self {
            Self::Left => 1,
            Self::Right => -1,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogicalAxis {
    X,
    Y,
    Z,
}
impl LogicalAxis {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
    pub fn controller_axis(self, side: StageSide) -> Axis {
        match (side, self) {
            (StageSide::Left, Self::X) => Axis::Y,
            (StageSide::Left, Self::Y) => Axis::X,
            (_, Self::X) => Axis::X,
            (_, Self::Y) => Axis::Y,
            (_, Self::Z) => Axis::Z,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Vector3Um {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
impl Vector3Um {
    pub fn new(x: f64, y: f64, z: f64) -> DriverResult<Self> {
        if [x, y, z].iter().any(|v| !v.is_finite()) {
            return Err(invalid("motion vector must be finite"));
        }
        Ok(Self { x, y, z })
    }
    pub fn for_axis(self, a: LogicalAxis) -> f64 {
        [self.x, self.y, self.z][a.index()]
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationCoefficient {
    pub um_per_v: f64,
    pub source: String,
    pub date: Option<String>,
    pub note: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxisCalibration {
    pub positive: CalibrationCoefficient,
    pub negative: CalibrationCoefficient,
}
#[derive(Clone, Debug)]
pub struct StageDefinition {
    pub side: StageSide,
    pub serial_number: String,
    pub toward_chip_sign: i8,
    pub axis_map: BTreeMap<LogicalAxis, Axis>,
    pub polarity: BTreeMap<LogicalAxis, i8>,
    pub calibration: BTreeMap<LogicalAxis, AxisCalibration>,
}
#[derive(Clone, Debug)]
pub struct MemberBinding {
    pub serial: String,
    pub port: String,
}
#[derive(Clone, Debug)]
pub struct FiberConfig {
    pub(crate) stages: BTreeMap<StageSide, StageDefinition>,
    pub(crate) bindings: Vec<MemberBinding>,
    pub(crate) toward: f64,
    pub(crate) other: f64,
    pub(crate) ceilings: [f64; 3],
    pub(crate) timeout: Duration,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    model: String,
    operator_limits_um: RawLimits,
    stages: BTreeMap<StageSide, RawStage>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLimits {
    toward_chip: f64,
    other: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStage {
    serial_number: String,
    toward_chip_sign: i8,
    axis_map: BTreeMap<LogicalAxis, String>,
    polarity: BTreeMap<LogicalAxis, i8>,
    calibration: BTreeMap<LogicalAxis, AxisCalibration>,
}
pub(crate) fn invalid(s: &str) -> DriverError {
    DriverError::Invalid(format!("Fiber: {s}"))
}
impl Default for FiberConfig {
    fn default() -> Self {
        Self::from_json(include_bytes!("../../../config/fiber_coupling.json"))
            .expect("reviewed nominal setup configuration")
    }
}
impl FiberConfig {
    pub fn from_json(bytes: &[u8]) -> DriverResult<Self> {
        let value =
            yang_protocol::strict_json(bytes, 65536).map_err(|e| invalid(&e.to_string()))?;
        let raw: RawConfig = serde_json::from_value(value).map_err(|e| invalid(&e.to_string()))?;
        if raw.model != "MAX312D"
            || !raw.operator_limits_um.toward_chip.is_finite()
            || !(0.0..=0.2).contains(&raw.operator_limits_um.toward_chip)
            || !(0.0..=1.0).contains(&raw.operator_limits_um.other)
            || raw.stages.len() != 2
        {
            return Err(invalid("invalid model, stages, or displacement limits"));
        }
        let mut stages = BTreeMap::new();
        for (side, stage) in raw.stages {
            if stage.serial_number != side.serial()
                || stage.toward_chip_sign != side.toward_chip_sign()
                || stage.axis_map.len() != 3
                || stage.polarity.len() != 3
                || stage.calibration.len() != 3
            {
                return Err(invalid("registered side binding cannot change"));
            }
            let mut axis_map = BTreeMap::new();
            for a in [LogicalAxis::X, LogicalAxis::Y, LogicalAxis::Z] {
                let token = stage
                    .axis_map
                    .get(&a)
                    .ok_or_else(|| invalid("incomplete axis map"))?;
                let axis = match token.as_str() {
                    "X" => Axis::X,
                    "Y" => Axis::Y,
                    "Z" => Axis::Z,
                    _ => return Err(invalid("unknown physical axis")),
                };
                if axis != a.controller_axis(side)
                    || !stage.polarity.get(&a).is_some_and(|v| [-1, 1].contains(v))
                {
                    return Err(invalid("invalid setup axis mapping or polarity"));
                }
                axis_map.insert(a, axis);
                let c = stage
                    .calibration
                    .get(&a)
                    .ok_or_else(|| invalid("missing calibration"))?;
                for coeff in [&c.positive, &c.negative] {
                    if !coeff.um_per_v.is_finite()
                        || coeff.um_per_v <= 0.
                        || !matches!(coeff.source.as_str(), "nominal_MAX312D" | "measured")
                        || coeff.date.as_ref().is_some_and(|d| d.trim().is_empty())
                    {
                        return Err(invalid("invalid directional calibration"));
                    }
                }
            }
            stages.insert(
                side,
                StageDefinition {
                    side,
                    serial_number: stage.serial_number,
                    toward_chip_sign: stage.toward_chip_sign,
                    axis_map,
                    polarity: stage.polarity,
                    calibration: stage.calibration,
                },
            );
        }
        Ok(Self {
            stages,
            bindings: vec![],
            toward: raw.operator_limits_um.toward_chip,
            other: raw.operator_limits_um.other,
            ceilings: [75.; 3],
            timeout: Duration::from_millis(500),
        })
    }
    pub fn from_path(path: &Path) -> DriverResult<Self> {
        use std::io::Read;
        let file = std::fs::File::open(path).map_err(|e| invalid(&e.to_string()))?;
        let mut bytes = vec![];
        file.take(65537)
            .read_to_end(&mut bytes)
            .map_err(|e| invalid(&e.to_string()))?;
        Self::from_json(&bytes)
    }
    pub fn stage_definition(&self, side: StageSide) -> &StageDefinition {
        &self.stages[&side]
    }
    pub fn toward_chip_limit_um(&self) -> f64 {
        self.toward
    }
    pub fn other_limit_um(&self) -> f64 {
        self.other
    }
    pub fn with_limits(
        mut self,
        toward: f64,
        other: f64,
        voltage_ceiling: f64,
    ) -> DriverResult<Self> {
        if !(0.0..=self.toward).contains(&toward)
            || !(0.0..=self.other).contains(&other)
            || !voltage_ceiling.is_finite()
            || voltage_ceiling <= 0.
            || voltage_ceiling > 75.
        {
            return Err(invalid("configuration may lower but not raise limits"));
        }
        self.toward = toward;
        self.other = other;
        self.ceilings = [voltage_ceiling; 3];
        Ok(self)
    }
    pub fn with_timeout(mut self, timeout: Duration) -> DriverResult<Self> {
        if timeout < Duration::from_millis(50) || timeout > Duration::from_secs(5) {
            return Err(invalid("serial timeout outside 0.05..5 seconds"));
        }
        self.timeout = timeout;
        Ok(self)
    }
    pub fn with_bindings(mut self, bindings: Vec<MemberBinding>) -> DriverResult<Self> {
        if bindings.is_empty() || bindings.len() > 2 {
            return Err(invalid("one or two bindings required"));
        }
        let mut serials = BTreeSet::new();
        let mut ports = BTreeSet::new();
        for b in &bindings {
            if ![StageSide::Left.serial(), StageSide::Right.serial()].contains(&b.serial.as_str())
                || !serials.insert(&b.serial)
                || !ports.insert(canonical_com(&b.port)?.as_str().to_string())
            {
                return Err(invalid("duplicate/unregistered serial or port"));
            }
        }
        self.bindings = bindings;
        Ok(self)
    }
}
