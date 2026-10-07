use crate::{DriverError, DriverResult};
use serde::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TraceId {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
}
impl TraceId {
    pub fn letter(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
        }
    }
    pub fn instrument_name(self) -> String {
        format!("TR{}", self.letter())
    }
}
impl std::str::FromStr for TraceId {
    type Err = DriverError;
    fn from_str(value: &str) -> DriverResult<Self> {
        let upper = value.trim().to_ascii_uppercase();
        match upper.strip_prefix("TR").unwrap_or(&upper) {
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            "C" => Ok(Self::C),
            "D" => Ok(Self::D),
            "E" => Ok(Self::E),
            "F" => Ok(Self::F),
            "G" => Ok(Self::G),
            _ => Err(DriverError::Invalid("OSA trace must be A through G".into())),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TransferFormat {
    #[serde(rename = "ASCII")]
    Ascii,
    #[serde(rename = "REAL,32")]
    Real32,
    #[serde(rename = "REAL,64")]
    Real64,
}
impl TransferFormat {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Ascii => "ASCII",
            Self::Real32 => "REAL,32",
            Self::Real64 => "REAL,64",
        }
    }
}
impl std::str::FromStr for TransferFormat {
    type Err = DriverError;
    fn from_str(value: &str) -> DriverResult<Self> {
        let parts: Vec<_> = value
            .trim()
            .split(',')
            .map(|p| p.trim().to_ascii_uppercase())
            .collect();
        match parts.join(",").as_str() {
            "ASCII" => Ok(Self::Ascii),
            "REAL,32" => Ok(Self::Real32),
            "REAL,64" => Ok(Self::Real64),
            _ => Err(DriverError::Protocol(
                "Unsupported OSA transfer format".into(),
            )),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum NativeUnit {
    #[serde(rename = "dBm")]
    Dbm,
    #[serde(rename = "W")]
    Watt,
}
impl NativeUnit {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Dbm => "dBm",
            Self::Watt => "W",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TraceContextParams {
    #[serde(serialize_with = "format_name")]
    pub transfer_format: TransferFormat,
    pub sample_count: usize,
    pub spacing: u8,
    pub level_unit: u8,
    pub x_unit: u8,
    pub trace_attribute: u8,
    #[serde(serialize_with = "trace_name")]
    pub active_trace: TraceId,
    pub center_m: f64,
    pub span_m: f64,
    pub resolution_m: f64,
    pub sweep_mode: u8,
}
fn format_name<S: serde::Serializer>(
    format: &TransferFormat,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(format.wire_name())
}
fn trace_name<S: serde::Serializer>(trace: &TraceId, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&trace.instrument_name())
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct TraceContext(TraceContextParams);
impl TraceContext {
    pub fn new(params: TraceContextParams) -> DriverResult<Self> {
        if !(1..=super::MAX_TRACE_POINTS).contains(&params.sample_count)
            || params.spacing > 1
            || params.level_unit > 3
            || params.x_unit != 0
            || params.trace_attribute > 5
            || !(1..=3).contains(&params.sweep_mode)
            || !params.center_m.is_finite()
            || params.center_m <= 0.
            || !params.span_m.is_finite()
            || params.span_m < 0.
            || !params.resolution_m.is_finite()
            || params.resolution_m <= 0.
        {
            return Err(DriverError::Protocol(
                "Unsupported or invalid OSA panel context; wavelength mode is required".into(),
            ));
        }
        Ok(Self(params))
    }
    pub fn params(&self) -> &TraceContextParams {
        &self.0
    }
    pub fn native_unit(&self) -> DriverResult<NativeUnit> {
        if self.0.trace_attribute == 5 {
            return Err(DriverError::Protocol(
                "OSA CALC interpretation is not validated".into(),
            ));
        }
        match (self.0.spacing, self.0.level_unit) {
            (0, 0) => Ok(NativeUnit::Dbm),
            (1, 1) => Ok(NativeUnit::Watt),
            _ => Err(DriverError::Protocol(
                "Density mode or conflicting OSA power scales are not validated".into(),
            )),
        }
    }
}
