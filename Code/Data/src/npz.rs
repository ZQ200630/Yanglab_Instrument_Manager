use crate::*;
use std::{
    io::{Seek, Write},
    path::Path,
};
pub(crate) fn read(
    path: &Path,
    limits: &ImportLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportReport, DataError> {
    let mut source = crate::legacy::Source::archive(path, limits)?;
    let names = source.strings("channel_names.npy", limits, cancelled)?;
    let mut unique = std::collections::HashSet::new();
    for name in &names {
        crate::legacy::member_name(name)?;
        if !unique.insert(name) {
            return Err(DataError::Invalid(
                "NPZ channel_names contains duplicates".into(),
            ));
        }
    }
    let common = if source.contains("time_us.npy") {
        let (time, _) = source.numeric("time_us.npy", limits, cancelled)?;
        Some((time.len(), crate::legacy::uniform_calibration(&time)?))
    } else {
        None
    };
    let mut channels = vec![];
    let mut decoded = 0u64;
    for name in names {
        crate::npy::check(cancelled)?;
        let time_name = format!("{name}__time_us.npy");
        let (count, calibration) = if source.contains(&time_name) {
            let (time, _) = source.numeric(&time_name, limits, cancelled)?;
            (time.len(), crate::legacy::uniform_calibration(&time)?)
        } else {
            common
                .clone()
                .ok_or_else(|| DataError::Invalid(format!("NPZ needs time_us or {time_name}")))?
        };
        let (voltage, dtype) = source.numeric(&format!("{name}.npy"), limits, cancelled)?;
        if count != voltage.len() {
            return Err(DataError::Invalid(format!(
                "NPZ time and voltage lengths differ for {name}"
            )));
        }
        decoded = decoded
            .checked_add(voltage.len() as u64 * 8)
            .ok_or_else(|| DataError::Limit("NPZ decoded size overflow".into()))?;
        if decoded > limits.max_uncompressed_bytes {
            return Err(DataError::Limit(
                "NPZ decoded arrays exceed byte budget".into(),
            ));
        }
        // Calibrated numeric arrays are volts, independent of their numeric storage dtype.
        let samples = match voltage {
            Samples::F64(v) => Samples::F64(v),
            v => Samples::F64((0..v.len()).map(|i| v.value(i).unwrap()).collect()),
        };
        channels.push(ChannelWaveform{channel:name,samples,calibration,metadata:serde_json::json!({"input_dtype":dtype,"values":"calibrated_volts","hardware_authority":false})});
    }
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
pub(crate) fn equal_axes(frame: &WaveformFrame) -> bool {
    let Some(first) = frame.channels.first() else {
        return false;
    };
    frame.channels.iter().all(|c| {
        c.samples.len() == first.samples.len()
            && (c.calibration.x_increment_s - first.calibration.x_increment_s).abs()
                <= first.calibration.x_increment_s.abs() * 1e-12
            && (c.calibration.time_s(0) - first.calibration.time_s(0)).abs()
                <= first.calibration.x_increment_s.abs() * 1e-9
    })
}
pub(crate) fn write(
    w: impl Write + Seek,
    frame: &WaveformFrame,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), DataError> {
    let mut zip = zip::ZipWriter::new(w);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let names: Vec<String> = frame.channels.iter().map(|c| c.channel.clone()).collect();
    let mut entries = std::collections::HashSet::new();
    entries.insert("channel_names".to_string());
    entries.insert("time_us".to_string());
    for name in &names {
        crate::legacy::member_name(name)?;
        if !entries.insert(name.clone()) || !entries.insert(format!("{name}__time_us")) {
            return Err(DataError::Invalid(
                "NPZ channel name collides with reserved axis/name entries".into(),
            ));
        }
    }
    zip.start_file("channel_names.npy", options)?;
    crate::npy::write_strings(&mut zip, &names)?;
    let common = equal_axes(frame);
    if common {
        let first = &frame.channels[0];
        zip.start_file("time_us.npy", options)?;
        crate::npy::write_values(
            &mut zip,
            first.samples.len(),
            (0..first.samples.len()).map(|i| first.calibration.time_s(i) * 1e6),
            cancelled,
        )?;
    }
    for c in &frame.channels {
        crate::npy::check(cancelled)?;
        if !common {
            zip.start_file(format!("{}__time_us.npy", c.channel), options)?;
            crate::npy::write_values(
                &mut zip,
                c.samples.len(),
                (0..c.samples.len()).map(|i| c.calibration.time_s(i) * 1e6),
                cancelled,
            )?;
        }
        zip.start_file(format!("{}.npy", c.channel), options)?;
        crate::npy::write_values(
            &mut zip,
            c.samples.len(),
            (0..c.samples.len()).map(|i| c.calibration.voltage_v(c.samples.value(i).unwrap())),
            cancelled,
        )?;
    }
    zip.finish()?;
    Ok(())
}
