use crate::{DriverError, DriverResult};
fn invalid(message: &str) -> DriverError {
    DriverError::Protocol(message.into())
}
fn name(name: &str) -> DriverResult<()> {
    if !(3..=4).contains(&name.len())
        || !name.as_bytes()[0].is_ascii_uppercase()
        || !name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(invalid(
            "Gain command must be 3–4 uppercase ASCII characters",
        ));
    }
    Ok(())
}
pub fn format_fixed(value: f64, minimum: f64, maximum: f64) -> DriverResult<String> {
    if !value.is_finite()
        || !minimum.is_finite()
        || !maximum.is_finite()
        || minimum > maximum
        || value < minimum
        || value > maximum
    {
        return Err(DriverError::Invalid(
            "Gain numeric value is outside finite limits".into(),
        ));
    }
    let scaled = (value * 1000.).round_ties_even();
    if !(0. ..=999999.).contains(&scaled) {
        return Err(DriverError::Invalid(
            "Gain value cannot fit six digits".into(),
        ));
    }
    Ok(format!("{:06}", scaled as u32))
}
pub fn build_command(
    name_value: &str,
    value: Option<f64>,
    minimum: f64,
    maximum: f64,
) -> DriverResult<Vec<u8>> {
    name(name_value)?;
    let number = value
        .map(|v| format_fixed(v, minimum, maximum))
        .transpose()?
        .unwrap_or_default();
    Ok(format!("{name_value}{number}\r\n").into_bytes())
}
pub fn build_bool(name_value: &str, value: bool) -> DriverResult<Vec<u8>> {
    name(name_value)?;
    Ok(format!("{name_value}{:06}\r\n", u8::from(value)).into_bytes())
}
fn framed(bytes: &[u8]) -> DriverResult<&str> {
    if bytes.len() > 512 || !bytes.is_ascii() {
        return Err(invalid("Gain reply is non-ASCII or oversized"));
    }
    std::str::from_utf8(bytes)
        .map_err(|_| invalid("Gain reply is not ASCII"))?
        .strip_suffix("\r\n")
        .ok_or_else(|| invalid("Gain reply lacks exact CRLF"))
}
pub fn parse_reply(bytes: &[u8], field: u8) -> DriverResult<&str> {
    if !field.is_ascii_uppercase() {
        return Err(invalid("Gain field must be uppercase ASCII"));
    }
    let text = framed(bytes)?;
    let tail = text
        .strip_prefix("READY;")
        .ok_or_else(|| invalid("Gain reply lacks READY field"))?;
    let tail_bytes = tail.as_bytes();
    if tail_bytes.len() < 3
        || tail_bytes[0] != field
        || tail_bytes[1] != b'='
        || tail_bytes[2..]
            .iter()
            .any(|b| matches!(b, b';' | b'\r' | b'\n'))
    {
        return Err(invalid("Gain reply field/framing mismatch"));
    }
    Ok(&tail[2..])
}
pub fn parse_ack(bytes: &[u8]) -> DriverResult<&str> {
    let text = framed(bytes)?;
    if text == "READY" {
        return Ok(text);
    }
    let field = *bytes
        .get(6)
        .ok_or_else(|| invalid("Gain malformed acknowledgement"))?;
    parse_reply(bytes, field)?;
    Ok(text)
}
pub(super) fn number(value: &str, field: u8) -> DriverResult<f64> {
    if value.is_empty()
        || value.trim() != value
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'+' | b'-' | b'e' | b'E'))
    {
        return Err(invalid("Gain invalid numeric reply"));
    }
    let numeric: f64 = value
        .parse()
        .map_err(|_| invalid("Gain invalid numeric reply"))?;
    let limits = match field {
        b'E' => (15., 40.),
        b'C' => (0., 200.),
        b'P' | b'I' | b'D' => (0., 999.999),
        b'T' => (f64::NEG_INFINITY, f64::INFINITY),
        _ => return Err(invalid("Gain unknown numeric field")),
    };
    if !numeric.is_finite() || numeric < limits.0 || numeric > limits.1 {
        return Err(invalid("Gain reply outside field limits"));
    }
    Ok(numeric)
}
pub(super) fn flag(value: &str) -> DriverResult<bool> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(invalid("Gain boolean reply must be literal 0 or 1")),
    }
}
