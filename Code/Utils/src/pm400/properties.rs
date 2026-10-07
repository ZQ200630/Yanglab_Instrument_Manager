use super::{finite, integer, Pm400, SensorCapabilities};
use crate::{DriverError, DriverResult};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitSelector {
    Minimum,
    Maximum,
    Default,
}
impl LimitSelector {
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::Minimum => "MINimum",
            Self::Maximum => "MAXimum",
            Self::Default => "DEFault",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumericValue {
    Value(f64),
    Limit(LimitSelector),
}
impl From<f64> for NumericValue {
    fn from(v: f64) -> Self {
        Self::Value(v)
    }
}
impl From<LimitSelector> for NumericValue {
    fn from(v: LimitSelector) -> Self {
        Self::Limit(v)
    }
}
#[derive(Clone, Copy)]
pub(crate) enum Capability {
    None,
    Power,
    Energy,
    Wavelength,
    Tau,
    ResponsePower,
    ResponseEnergy,
    Optical,
}
impl Pm400 {
    pub(crate) fn capability(&self, cap: Capability) -> DriverResult<()> {
        let c: &SensorCapabilities = &self.sensor_info().ok_or(DriverError::Closed)?.capabilities;
        let legal = match cap {
            Capability::None => true,
            Capability::Power => c.power,
            Capability::Energy => c.energy,
            Capability::Wavelength => c.wavelength_settable,
            Capability::Tau => c.power && c.tau_settable,
            Capability::ResponsePower => c.power && c.response_settable,
            Capability::ResponseEnergy => c.energy && c.response_settable,
            Capability::Optical => c.power || c.energy,
        };
        if legal {
            Ok(())
        } else {
            Err(DriverError::Invalid(
                "connected sensor capability unavailable".into(),
            ))
        }
    }
    pub(crate) fn confirm(confirm: bool) -> DriverResult<()> {
        if confirm {
            Ok(())
        } else {
            Err(DriverError::Invalid(
                "explicit operator confirmation required".into(),
            ))
        }
    }
    pub(crate) fn set_confirm<T>(
        &mut self,
        header: &str,
        token: &str,
        expected: T,
        parse: impl FnOnce(&str) -> DriverResult<T>,
        matches: impl FnOnce(&T, &T) -> bool,
    ) -> DriverResult<T> {
        let d = self.deadline();
        self.write(&format!("{header} {token}"), d)?;
        let observed = parse(&self.query(&format!("{header}?"), d)?)?;
        if !matches(&observed, &expected) {
            return Err(DriverError::Protocol(
                "PM setter readback mismatch; effect unknown".into(),
            ));
        }
        Ok(observed)
    }
    pub(crate) fn get_numeric(
        &mut self,
        header: &str,
        selector: Option<LimitSelector>,
        default: bool,
    ) -> DriverResult<f64> {
        if selector == Some(LimitSelector::Default) && !default {
            return Err(DriverError::Invalid("default selector unsupported".into()));
        }
        let command = match selector {
            Some(s) => format!("{header}? {}", s.token()),
            None => format!("{header}?"),
        };
        finite(&self.query(&command, self.deadline())?)
    }
    pub(crate) fn set_numeric(
        &mut self,
        header: &str,
        value: NumericValue,
        default: bool,
    ) -> DriverResult<f64> {
        let (token, expected) = match value {
            NumericValue::Limit(s) => {
                if s == LimitSelector::Default && !default {
                    return Err(DriverError::Invalid("default selector unsupported".into()));
                }
                (
                    s.token().into(),
                    self.get_numeric(header, Some(s), default)?,
                )
            }
            NumericValue::Value(v) => {
                if !v.is_finite() {
                    return Err(DriverError::Invalid(
                        "numeric setting must be finite".into(),
                    ));
                }
                let lo = self.get_numeric(header, Some(LimitSelector::Minimum), default)?;
                let hi = self.get_numeric(header, Some(LimitSelector::Maximum), default)?;
                if lo > hi {
                    return Err(DriverError::Protocol("inverted device limits".into()));
                }
                if !(lo..=hi).contains(&v) {
                    return Err(DriverError::Invalid(
                        "setting outside connected device limits".into(),
                    ));
                }
                (v.to_string(), v)
            }
        };
        self.set_confirm(header, &token, expected, finite, |a, b| close(*a, *b, 1e-9))
    }
    pub(crate) fn set_boolean(&mut self, header: &str, value: bool) -> DriverResult<bool> {
        self.set_confirm(
            header,
            if value { "1" } else { "0" },
            value,
            boolean,
            |a, b| a == b,
        )
    }
    pub(crate) fn get_boolean(&mut self, header: &str) -> DriverResult<bool> {
        boolean(&self.query(&format!("{header}?"), self.deadline())?)
    }
    pub(crate) fn set_integer(
        &mut self,
        header: &str,
        value: u32,
        min: u32,
        max: u32,
    ) -> DriverResult<u32> {
        if !(min..=max).contains(&value) {
            return Err(DriverError::Invalid(
                "integer setting outside bounds".into(),
            ));
        }
        self.set_confirm(
            header,
            &value.to_string(),
            value,
            |t| Ok(integer(t, min.into(), max.into())? as u32),
            |a, b| a == b,
        )
    }
    pub(crate) fn get_integer(&mut self, header: &str, min: u32, max: u32) -> DriverResult<u32> {
        let t = self.query(&format!("{header}?"), self.deadline())?;
        Ok(integer(&t, min.into(), max.into())? as u32)
    }
}
pub(crate) fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= 1e-12f64.max(rel * a.abs().max(b.abs()))
}
pub(crate) fn boolean(t: &str) -> DriverResult<bool> {
    match t {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(DriverError::Protocol(
            "boolean reply must be literal 0 or 1".into(),
        )),
    }
}
