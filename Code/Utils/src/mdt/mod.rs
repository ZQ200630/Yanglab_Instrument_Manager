//! MDT693B prompt protocol and stop-and-hold lifecycle. No startup output writes.
mod monitor;
mod protocol;
mod session;
pub use protocol::{parse_reply, ParsedReply};
use serde::Serialize;
pub use session::{retry_retained, Mdt693b, MdtConfig, StopHandle};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Axis {
    X,
    Y,
    Z,
}
impl Axis {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum VoltageLimit {
    V75,
    V100,
    V150,
}
impl VoltageLimit {
    pub fn code(self) -> u8 {
        match self {
            Self::V75 => 0,
            Self::V100 => 1,
            Self::V150 => 2,
        }
    }
    pub fn volts(self) -> f64 {
        match self {
            Self::V75 => 75.,
            Self::V100 => 100.,
            Self::V150 => 150.,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RotaryMode {
    Default,
    TenTurn,
    Fine,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct AxisState {
    pub actual_v: f64,
    pub minimum_v: f64,
    pub maximum_v: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MdtStatus {
    pub product: String,
    pub firmware: String,
    pub serial_number: String,
    pub friendly_name: String,
    pub echo_enabled: bool,
    pub hardware_limit: VoltageLimit,
    pub display_intensity: u8,
    pub master_scan_enabled: bool,
    pub master_scan_voltage_v: f64,
    pub axes: BTreeMap<Axis, AxisState>,
    pub dac_step: u16,
    pub compatibility_enabled: bool,
    pub rotary_mode: RotaryMode,
    pub push_to_adjust_disabled: bool,
    pub supported_commands: BTreeSet<String>,
    pub restricted: bool,
    pub fault_evidence: Option<String>,
    pub observed_at: f64,
    pub axis_command_known: bool,
}
