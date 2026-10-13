use crate::*;
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
};
fn line(r: &mut impl BufRead) -> Result<Option<String>, DataError> {
    let mut bytes = vec![];
    loop {
        let available = r.fill_buf()?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        let count = available
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(available.len());
        if bytes.len() + count > 1_048_576 {
            return Err(DataError::Limit("CSV line exceeds 1MiB bound".into()));
        }
        bytes.extend_from_slice(&available[..count]);
        r.consume(count);
        if bytes.last() == Some(&b'\n') {
            break;
        }
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| DataError::Invalid("CSV must be UTF-8".into()))
}
fn fields(text: &str) -> Result<Vec<String>, DataError> {
    let mut out = vec![];
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = text.trim_end_matches(['\r', '\n']).chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if quoted && chars.peek() == Some(&'"') {
                    chars.next();
                    current.push('"');
                } else {
                    quoted = !quoted;
                }
            }
            ',' if !quoted => {
                out.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if quoted {
        return Err(DataError::Invalid("unterminated CSV quote".into()));
    }
    out.push(current.trim().to_string());
    Ok(out)
}
pub(crate) fn read(
    path: &Path,
    limits: &ImportLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportReport, DataError> {
    let f = std::fs::File::open(path)?;
    let bytes = f.metadata()?.len();
    if bytes > limits.max_archive_bytes || bytes > limits.max_uncompressed_bytes {
        return Err(DataError::Limit("CSV exceeds byte budget".into()));
    }
    let mut reader = BufReader::new(f);
    let mut header = None;
    let mut columns: Vec<Vec<f64>> = vec![];
    let mut rows = 0;
    while let Some(text) = line(&mut reader)? {
        crate::npy::check(cancelled)?;
        if text.trim().is_empty() || text.trim_start().starts_with('#') {
            continue;
        }
        let values = fields(&text)?;
        if header.is_none() {
            if values.first().map(String::as_str) != Some("Time (us)")
                || values.len() < 2
                || values.len() > 65
            {
                return Err(DataError::Invalid(
                    "CSV header must be Time (us),CH1 (V),...".into(),
                ));
            }
            let mut names = std::collections::HashSet::new();
            for name in &values[1..] {
                let name = name.strip_suffix(" (V)").ok_or_else(|| {
                    DataError::Invalid("CSV channel header must end in (V)".into())
                })?;
                if name.is_empty() || !names.insert(name) {
                    return Err(DataError::Invalid(
                        "CSV channel names must be nonempty and unique".into(),
                    ));
                }
            }
            columns = vec![vec![]; values.len()];
            header = Some(values);
            continue;
        }
        if values.len() != columns.len() {
            return Err(DataError::Invalid(format!(
                "CSV row {} column count differs",
                rows + 1
            )));
        }
        rows += 1;
        if rows > limits.max_samples_per_channel
            || (rows as u64)
                .checked_mul(columns.len() as u64 * 8)
                .is_none_or(|n| n > limits.max_uncompressed_bytes)
        {
            return Err(DataError::Limit(
                "decoded CSV exceeds sample or byte budget".into(),
            ));
        }
        for (col, text) in columns.iter_mut().zip(values) {
            let value = text
                .parse::<f64>()
                .map_err(|_| DataError::Invalid(format!("CSV row {rows} has nonnumeric value")))?;
            if !value.is_finite() {
                return Err(DataError::Invalid(format!(
                    "CSV row {rows} has nonfinite value"
                )));
            }
            col.push(value);
        }
    }
    let header = header.ok_or_else(|| DataError::Invalid("CSV is empty".into()))?;
    let time = Samples::F64(std::mem::take(&mut columns[0]));
    let calibration = crate::legacy::uniform_calibration(&time)?;
    let channels = columns
        .into_iter()
        .skip(1)
        .zip(header.into_iter().skip(1))
        .map(|(values, name)| ChannelWaveform {
            channel: name.strip_suffix(" (V)").unwrap().into(),
            samples: Samples::F64(values),
            calibration: calibration.clone(),
            metadata: serde_json::json!({"values":"calibrated_volts","hardware_authority":false}),
        })
        .collect();
    let frame = WaveformFrame {
        id: crate::new_id()?,
        captured_at_unix_ms: 0,
        nominal_offset_s: None,
        channels,
    };
    let record = crate::legacy::imported_record(path, vec![frame])?;
    record.validate()?;
    Ok(ImportReport {
        records: vec![record],
        warnings: vec![],
    })
}
fn escaped(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.into()
    }
}
pub(crate) fn write(
    w: &mut impl Write,
    frame: &WaveformFrame,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), DataError> {
    if !crate::npz::equal_axes(frame) {
        return Err(DataError::Unsupported(
            "CSV requires identical channel lengths and time axes; export NPZ for independent axes"
                .into(),
        ));
    }
    w.write_all(b"# Exported by Yang Lab; calibrated volts\nTime (us)")?;
    for c in &frame.channels {
        write!(w, ",{}", escaped(&format!("{} (V)", c.channel)))?;
    }
    w.write_all(b"\n")?;
    let first = &frame.channels[0];
    for i in 0..first.samples.len() {
        if i % 2048 == 0 {
            crate::npy::check(cancelled)?;
        }
        write!(w, "{:.17e}", first.calibration.time_s(i) * 1e6)?;
        for c in &frame.channels {
            write!(
                w,
                ",{:.17e}",
                c.calibration.voltage_v(c.samples.value(i).unwrap())
            )?;
        }
        w.write_all(b"\n")?;
    }
    Ok(())
}
