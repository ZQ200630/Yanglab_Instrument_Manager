//! Typed PM400 API. Connect/normal close preserve front-panel settings.
mod measurement;
mod sensor;
mod session;
mod status;
use crate::{DriverError, DriverResult};
pub use measurement::{Measurement, MeasurementKind, PowerUnit};
pub use sensor::{InstrumentInfo, SensorCapabilities, SensorInfo};
pub use session::{retry_retained, Pm400, PmOptions, StopHandle};
pub use status::{Status, StatusGroup, SystemError};
pub(crate) fn csv(text: &str, count: usize) -> DriverResult<Vec<String>> {
    if text.contains(['\n', '\r']) || text.len() > 8192 {
        return Err(DriverError::Protocol("invalid CSV response".into()));
    }
    let mut result = vec![];
    let mut chars = text.chars().peekable();
    loop {
        let mut field = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') => {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            field.push('"');
                        } else {
                            break;
                        }
                    }
                    Some(c) => field.push(c),
                    None => return Err(DriverError::Protocol("unterminated quoted CSV".into())),
                }
            }
            if chars.peek().is_some_and(|c| *c != ',') {
                return Err(DriverError::Protocol("trailing quoted CSV data".into()));
            }
        } else {
            while chars.peek().is_some_and(|c| *c != ',') {
                let c = chars.next().unwrap();
                if c == '"' {
                    return Err(DriverError::Protocol("quote in unquoted CSV".into()));
                }
                field.push(c);
            }
        }
        if field.trim().is_empty() {
            return Err(DriverError::Protocol("empty required CSV field".into()));
        }
        result.push(field);
        if chars.next().is_none() {
            break;
        }
    }
    if result.len() != count {
        return Err(DriverError::Protocol("unexpected CSV field count".into()));
    }
    Ok(result)
}
pub(crate) fn finite(text: &str) -> DriverResult<f64> {
    let v = text
        .trim()
        .parse::<f64>()
        .map_err(|_| DriverError::Protocol("expected numeric response".into()))?;
    if !v.is_finite() {
        return Err(DriverError::Protocol("nonfinite response".into()));
    }
    Ok(v)
}
pub(crate) fn integer(text: &str, min: i64, max: i64) -> DriverResult<i64> {
    let v = text
        .parse::<i64>()
        .map_err(|_| DriverError::Protocol("expected integer response".into()))?;
    if !(min..=max).contains(&v) {
        return Err(DriverError::Protocol("register outside bounds".into()));
    }
    Ok(v)
}
