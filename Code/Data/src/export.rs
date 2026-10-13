use crate::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Csv,
    Npz,
    Native,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub directory: PathBuf,
    pub format: ExportFormat,
    pub frame_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportFileReport {
    pub record_id: String,
    pub frame_id: Option<String>,
    pub path: Option<PathBuf>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportReport {
    pub files: Vec<ExportFileReport>,
}
pub fn export_records(records: &[WaveformRecord], request: &ExportRequest) -> ExportReport {
    export_records_with_cancel(records, request, &|| false)
}
pub fn export_records_with_cancel(
    records: &[WaveformRecord],
    request: &ExportRequest,
    cancelled: &dyn Fn() -> bool,
) -> ExportReport {
    export_record_refs_with_cancel(&records.iter().collect::<Vec<_>>(), request, cancelled)
}
/// Export borrowed records so volatile deep-memory payloads are never cloned.
pub fn export_record_refs_with_cancel(
    records: &[&WaveformRecord],
    request: &ExportRequest,
    cancelled: &dyn Fn() -> bool,
) -> ExportReport {
    let mut files = vec![];
    let destination = std::fs::create_dir_all(&request.directory)
        .and_then(|_| request.directory.canonicalize())
        .map_err(DataError::from);
    for record in records {
        let valid = record.validate_with_cancel(cancelled);
        if matches!(request.format, ExportFormat::Native) {
            let result = (|| {
                crate::npy::check(cancelled)?;
                valid.clone()?;
                let target = destination.as_ref().map_err(Clone::clone)?.join(format!(
                    "{}-{}-{}.db",
                    stem(&record.name),
                    &record.id[..8],
                    crate::new_id()?
                ));
                let part = target.with_extension("db.part");
                let mut pending = Output::new(part.clone())?;
                {
                    let file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&part)?;
                    crate::store::write_database(&file, record, &request.frame_ids, cancelled)?;
                    file.sync_all()?;
                }
                crate::npy::check(cancelled)?;
                std::fs::rename(&part, &target)?;
                pending.paths.clear();
                Ok(target)
            })();
            files.push(report(record, None, result));
            continue;
        }
        for frame in &record.frames {
            if !request.frame_ids.is_empty() && !request.frame_ids.contains(&frame.id) {
                continue;
            }
            let result = (|| {
                crate::npy::check(cancelled)?;
                valid.clone()?;
                let directory = destination.as_ref().map_err(Clone::clone)?;
                let extension = match request.format {
                    ExportFormat::Csv => "csv",
                    ExportFormat::Npz => "npz",
                    ExportFormat::Native => unreachable!(),
                };
                let target = directory.join(format!(
                    "{}-{}-{}-{}.{}",
                    stem(&record.name),
                    &record.id[..8],
                    &frame.id[..8],
                    crate::new_id()?,
                    extension
                ));
                let part = target.with_extension(format!("{extension}.part"));
                let mut pending = Output::new(part.clone())?;
                {
                    let file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&part)?;
                    match request.format {
                        ExportFormat::Csv => {
                            let mut writer = std::io::BufWriter::new(file);
                            crate::csv::write(&mut writer, frame, cancelled)?;
                            std::io::Write::flush(&mut writer)?;
                            writer.get_ref().sync_all()?;
                        }
                        ExportFormat::Npz => {
                            crate::npz::write(&file, frame, cancelled)?;
                            file.sync_all()?;
                        }
                        ExportFormat::Native => unreachable!(),
                    }
                }
                let metadata = serde_json::json!({"schema":1,"record_id":record.id,"frame_id":frame.id,"name":record.name,"comment":record.comment,"source":record.source,"created_at_unix_ms":record.created_at_unix_ms,"captured_at_unix_ms":frame.captured_at_unix_ms,"nominal_offset_s":frame.nominal_offset_s,"raw_preserved":false,"values":"calibrated_volts","axes":"microseconds; per-channel NPZ axes when unequal","channels":frame.channels.iter().map(|c|serde_json::json!({"channel":c.channel,"sample_count":c.samples.len(),"calibration":c.calibration,"metadata":c.metadata})).collect::<Vec<_>>(),"analyses":record.analyses.iter().filter(|a|a.frame_id==frame.id).collect::<Vec<_>>()});
                let bytes = serde_json::to_vec_pretty(&metadata)?;
                if bytes.len() > 16_777_216 {
                    return Err(DataError::Limit(
                        "export metadata exceeds 16MiB budget".into(),
                    ));
                }
                let sidecar = target.with_extension(format!("{extension}.metadata.json"));
                let sidepart = sidecar.with_extension("json.part");
                pending.paths.push(sidepart.clone());
                let mut writer = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&sidepart)?;
                std::io::Write::write_all(&mut writer, &bytes)?;
                writer.sync_all()?;
                drop(writer);
                crate::npy::check(cancelled)?;
                std::fs::rename(&sidepart, &sidecar)?;
                pending.paths.push(sidecar);
                std::fs::rename(&part, &target)?;
                pending.paths.clear();
                Ok(target)
            })();
            files.push(report(record, Some(frame.id.clone()), result));
        }
    }
    for id in &request.frame_ids {
        if !records.iter().flat_map(|r| &r.frames).any(|f| &f.id == id) {
            files.push(ExportFileReport {
                record_id: String::new(),
                frame_id: Some(id.clone()),
                path: None,
                error: Some("selected frame does not exist in supplied records".into()),
            });
        }
    }
    ExportReport { files }
}
fn report(
    record: &WaveformRecord,
    frame_id: Option<String>,
    result: Result<PathBuf, DataError>,
) -> ExportFileReport {
    match result {
        Ok(path) => ExportFileReport {
            record_id: record.id.clone(),
            frame_id,
            path: Some(path),
            error: None,
        },
        Err(e) => ExportFileReport {
            record_id: record.id.clone(),
            frame_id,
            path: None,
            error: Some(e.to_string()),
        },
    }
}
fn stem(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    if s.trim_matches('_').is_empty() {
        "waveform".into()
    } else {
        format!("waveform_{s}")
    }
}
struct Output {
    paths: Vec<PathBuf>,
}
impl Output {
    fn new(path: PathBuf) -> Result<Self, DataError> {
        if path.exists() {
            return Err(DataError::Conflict(
                "export temporary path already exists".into(),
            ));
        }
        Ok(Self { paths: vec![path] })
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}
