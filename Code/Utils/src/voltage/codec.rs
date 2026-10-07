use super::VoltageStatus;
use crate::{DriverError, DriverResult};
use std::time::Duration;
pub const BOARD_SCALE_V: f64 = 28.0;
pub(super) fn codes(values: [f64; 8], limit: f64) -> DriverResult<[u16; 8]> {
    if !limit.is_finite() || !(0.0..=14.0).contains(&limit) {
        return Err(DriverError::Invalid(
            "voltage ceiling must be within 0–14 V".into(),
        ));
    }
    let mut raw = [0; 8];
    for (i, value) in values.iter().enumerate() {
        if !value.is_finite() || !(0.0..=limit).contains(value) {
            return Err(DriverError::Invalid(format!(
                "channel {} outside 0–{limit} V",
                i + 1
            )));
        }
        raw[i] = (*value * 65535. / BOARD_SCALE_V) as u16;
    }
    Ok(raw)
}
pub(super) fn encode_codes(raw: [u16; 8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(18);
    for value in raw {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(b"\r\n");
    bytes
}
pub(super) fn volts(raw: [u16; 8]) -> [f64; 8] {
    raw.map(|v| v as f64 * BOARD_SCALE_V / 65535.)
}
pub fn encode_voltages(values: [f64; 8], limit: f64) -> DriverResult<Vec<u8>> {
    Ok(encode_codes(codes(values, limit)?))
}
pub fn decode_telemetry(frame: &[u8], received_at: Duration) -> DriverResult<VoltageStatus> {
    if frame.len() != 34 || &frame[32..] != b"\r\n" {
        return Err(DriverError::Protocol(
            "expected 34-byte voltage telemetry ending in CRLF".into(),
        ));
    }
    let mut voltage_v = [0.; 8];
    let mut current_ma = [0.; 8];
    for i in 0..8 {
        let p = i * 4;
        voltage_v[i] = u16::from_be_bytes([frame[p], frame[p + 1]]) as f64 * 0.0016;
        current_ma[i] = i16::from_be_bytes([frame[p + 2], frame[p + 3]]) as f64 * 0.0025;
    }
    Ok(VoltageStatus {
        voltage_v,
        current_ma,
        received_at,
    })
}
