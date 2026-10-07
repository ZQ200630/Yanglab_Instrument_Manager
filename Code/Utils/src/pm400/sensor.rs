use super::csv;
use crate::{DriverError, DriverResult};
use serde::Serialize;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InstrumentInfo {
    pub manufacturer: String,
    pub model: String,
    pub serial_number: String,
    pub firmware: String,
    pub raw: String,
}
impl InstrumentInfo {
    pub fn parse(text: &str) -> DriverResult<Self> {
        let f = csv(text, 4)?;
        Ok(Self {
            manufacturer: f[0].clone(),
            model: f[1].clone(),
            serial_number: f[2].clone(),
            firmware: f[3].clone(),
            raw: text.into(),
        })
    }
    pub(crate) fn validate(&self) -> DriverResult<()> {
        if !self.manufacturer.trim().eq_ignore_ascii_case("Thorlabs")
            || !self.model.trim().eq_ignore_ascii_case("PM400")
        {
            Err(DriverError::Protocol("unexpected PM400 identity".into()))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SensorCapabilities {
    pub power: bool,
    pub energy: bool,
    pub response_settable: bool,
    pub wavelength_settable: bool,
    pub tau_settable: bool,
    pub temperature_sensor: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SensorInfo {
    pub name: String,
    pub serial_number: String,
    pub calibration_message: String,
    pub sensor_type: i32,
    pub subtype: i32,
    pub raw_flags: u32,
    pub capabilities: SensorCapabilities,
    pub raw: String,
}
impl SensorInfo {
    pub fn parse(text: &str) -> DriverResult<Self> {
        let f = csv(text, 6)?;
        let number = |s: &str| {
            s.trim()
                .parse::<i32>()
                .map_err(|_| DriverError::Protocol("sensor field not integer".into()))
        };
        let flags = f[5]
            .trim()
            .parse::<u32>()
            .map_err(|_| DriverError::Protocol("sensor flags must be nonnegative".into()))?;
        Ok(Self {
            name: f[0].clone(),
            serial_number: f[1].clone(),
            calibration_message: f[2].clone(),
            sensor_type: number(&f[3])?,
            subtype: number(&f[4])?,
            raw_flags: flags,
            capabilities: SensorCapabilities {
                power: flags & 1 != 0,
                energy: flags & 2 != 0,
                response_settable: flags & 16 != 0,
                wavelength_settable: flags & 32 != 0,
                tau_settable: flags & 64 != 0,
                temperature_sensor: flags & 256 != 0,
            },
            raw: text.into(),
        })
    }
}
