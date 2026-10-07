use super::TransferFormat;
use crate::{DriverError, DriverResult};
pub const MAX_TRACE_POINTS: usize = 200001;
pub const TRACE_CHUNK_POINTS: usize = 1024;
pub const MAX_TRACE_REPLY_BYTES: usize = 65536;
fn invalid() -> DriverError {
    DriverError::Protocol("Malformed, nonfinite, truncated or oversized OSA trace range".into())
}
pub(crate) fn ascii_number(field: &[u8]) -> DriverResult<f64> {
    let start = field
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t'))
        .ok_or_else(invalid)?;
    let end = field
        .iter()
        .rposition(|b| !matches!(b, b' ' | b'\t'))
        .unwrap()
        + 1;
    let field = &field[start..end];
    // Only horizontal whitespace is admitted in numeric fields. Rust's float
    // parser also accepts inf/NaN; validate the reviewed decimal grammar first.
    if field.is_empty()
        || field
            .iter()
            .any(|b| !b.is_ascii() || b.is_ascii_whitespace())
    {
        return Err(invalid());
    }
    let mut index = usize::from(matches!(field[0], b'+' | b'-'));
    let before = index;
    while field.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let mut digits = index - before;
    if field.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while field.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        digits += index - start;
    }
    if digits == 0 {
        return Err(invalid());
    }
    if matches!(field.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(field.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let start = index;
        while field.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == start {
            return Err(invalid());
        }
    }
    if index != field.len() {
        return Err(invalid());
    }
    let value = std::str::from_utf8(field)
        .map_err(|_| invalid())?
        .parse::<f64>()
        .map_err(|_| invalid())?;
    if !value.is_finite() {
        return Err(invalid());
    }
    Ok(value)
}
pub fn decode_trace_reply(
    reply: &[u8],
    format: TransferFormat,
    expected: usize,
) -> DriverResult<Vec<f64>> {
    if !(1..=TRACE_CHUNK_POINTS).contains(&expected) || reply.len() > MAX_TRACE_REPLY_BYTES {
        return Err(invalid());
    }
    let values = match format {
        TransferFormat::Ascii => {
            let body = reply
                .strip_suffix(b"\r\n")
                .or_else(|| reply.strip_suffix(b"\n"))
                .unwrap_or(reply);
            let fields: Vec<_> = body.split(|b| *b == b',').collect();
            if fields.len() != expected {
                return Err(invalid());
            }
            fields
                .into_iter()
                .map(ascii_number)
                .collect::<DriverResult<Vec<_>>>()?
        }
        TransferFormat::Real32 | TransferFormat::Real64 => {
            if reply.first() != Some(&b'#') || !matches!(reply.get(1), Some(b'1'..=b'9')) {
                return Err(invalid());
            }
            let header_end = 2 + usize::from(reply[1] - b'0');
            let length = reply
                .get(2..header_end)
                .filter(|bytes| bytes.iter().all(u8::is_ascii_digit))
                .ok_or_else(invalid)?;
            let byte_count = std::str::from_utf8(length)
                .map_err(|_| invalid())?
                .parse::<usize>()
                .map_err(|_| invalid())?;
            let width = if format == TransferFormat::Real32 {
                4
            } else {
                8
            };
            if byte_count != expected * width {
                return Err(invalid());
            }
            let data_end = header_end + byte_count;
            let data = reply.get(header_end..data_end).ok_or_else(invalid)?;
            if !matches!(reply.get(data_end..), Some(b"" | b"\n" | b"\r\n")) {
                return Err(invalid());
            }
            data.chunks_exact(width)
                .map(|bytes| {
                    if width == 4 {
                        f64::from(f32::from_le_bytes(bytes.try_into().unwrap()))
                    } else {
                        f64::from_le_bytes(bytes.try_into().unwrap())
                    }
                })
                .collect()
        }
    };
    if values.iter().any(|v| !v.is_finite()) {
        return Err(invalid());
    }
    Ok(values)
}
