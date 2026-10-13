use crate::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportLimits {
    pub max_archive_bytes: u64,
    pub max_uncompressed_bytes: u64,
    pub max_member_bytes: u64,
    pub max_metadata_bytes: u64,
    pub max_members: usize,
    pub max_samples_per_channel: usize,
    pub max_records: usize,
    pub max_frames: usize,
}
impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 536_870_912,
            max_uncompressed_bytes: 536_870_912,
            max_member_bytes: 536_870_912,
            max_metadata_bytes: 16_777_216,
            max_members: 4096,
            max_samples_per_channel: 250_000_000,
            max_records: 256,
            max_frames: 9999,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportReport {
    pub records: Vec<WaveformRecord>,
    pub warnings: Vec<String>,
}
pub fn import_path(path: &Path, limits: &ImportLimits) -> Result<ImportReport, DataError> {
    import_path_with_cancel(path, limits, &|| false)
}
pub fn import_path_with_cancel(
    path: &Path,
    limits: &ImportLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportReport, DataError> {
    crate::npy::check(cancelled)?;
    if path.is_dir() || path.file_name().is_some_and(|n| n == "database.json") {
        let root = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(Path::new("."))
        };
        let mut source = Source::directory(root, limits)?;
        return read_database(&mut source, path, limits, cancelled);
    }
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "db" => {
            let mut source = Source::archive(path, limits)?;
            read_database(&mut source, path, limits, cancelled)
        }
        "npz" => crate::npz::read(path, limits, cancelled),
        "csv" => crate::csv::read(path, limits, cancelled),
        _ => Err(DataError::Unsupported(
            "import supports legacy .db/database.json, .npz and calibrated .csv".into(),
        )),
    }
}
pub(crate) fn member_name(name: &str) -> Result<(), DataError> {
    if name.is_empty()
        || name.len() > 4096
        || name.starts_with('/')
        || name.contains(['\\', ':', '\0'])
        || name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        Err(DataError::Invalid(
            "archive member is not a confined relative path".into(),
        ))
    } else {
        Ok(())
    }
}
enum SourceKind {
    Zip(zip::ZipArchive<std::fs::File>),
    Directory(PathBuf),
}
pub(crate) struct Source {
    kind: SourceKind,
    names: HashSet<String>,
    used: u64,
}
impl Source {
    pub(crate) fn archive(path: &Path, limits: &ImportLimits) -> Result<Self, DataError> {
        let mut f = std::fs::File::open(path)?;
        if f.metadata()?.len() > limits.max_archive_bytes {
            return Err(DataError::Limit(
                "compressed archive exceeds byte budget".into(),
            ));
        }
        // Inspect the central directory before zip allocates its index. ZIP64 is unnecessary
        // inside our sub-4GiB input budget and is rejected explicitly.
        let names = scan_directory(&mut f, limits)?;
        f.seek(SeekFrom::Start(0))?;
        let mut archive = zip::ZipArchive::new(f)?;
        if archive.len() != names.len() {
            return Err(DataError::Invalid("ambiguous duplicate ZIP members".into()));
        }
        for i in 0..archive.len() {
            let member = archive.by_index(i)?;
            if member.is_symlink() {
                return Err(DataError::Invalid(
                    "archive symlinks are unsupported".into(),
                ));
            }
        }
        Ok(Self {
            kind: SourceKind::Zip(archive),
            names,
            used: 0,
        })
    }
    fn directory(path: &Path, limits: &ImportLimits) -> Result<Self, DataError> {
        let root = path.canonicalize()?;
        let mut names = HashSet::new();
        let mut total = 0;
        fn walk(
            root: &Path,
            dir: &Path,
            names: &mut HashSet<String>,
            total: &mut u64,
            limits: &ImportLimits,
        ) -> Result<(), DataError> {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let meta = std::fs::symlink_metadata(entry.path())?;
                if meta.file_type().is_symlink() {
                    return Err(DataError::Invalid(
                        "unpacked import contains a symlink".into(),
                    ));
                }
                if meta.is_dir() {
                    walk(root, &entry.path(), names, total, limits)?;
                } else if meta.is_file() {
                    let name = entry
                        .path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    member_name(&name)?;
                    if !names.insert(name) || names.len() > limits.max_members {
                        return Err(DataError::Limit(
                            "unpacked import member count exceeds budget".into(),
                        ));
                    }
                    *total = total
                        .checked_add(meta.len())
                        .ok_or_else(|| DataError::Limit("unpacked size overflow".into()))?;
                    if meta.len() > limits.max_member_bytes
                        || *total > limits.max_uncompressed_bytes
                    {
                        return Err(DataError::Limit(
                            "unpacked import exceeds byte budget".into(),
                        ));
                    }
                }
            }
            Ok(())
        }
        walk(&root, &root, &mut names, &mut total, limits)?;
        Ok(Self {
            kind: SourceKind::Directory(root),
            names,
            used: 0,
        })
    }
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }
    fn with_member<T>(
        &mut self,
        name: &str,
        limits: &ImportLimits,
        action: impl FnOnce(&mut dyn Read) -> Result<T, DataError>,
    ) -> Result<T, DataError> {
        member_name(name)?;
        if !self.names.contains(name) {
            return Err(DataError::NotFound(format!(
                "archive member {name} is absent"
            )));
        }
        match &mut self.kind {
            SourceKind::Zip(z) => {
                let mut f = z.by_name(name)?;
                self.used = self
                    .used
                    .checked_add(f.size())
                    .ok_or_else(|| DataError::Limit("import size overflow".into()))?;
                if self.used > limits.max_uncompressed_bytes {
                    return Err(DataError::Limit(
                        "repeated import reads exceed total budget".into(),
                    ));
                }
                action(&mut f)
            }
            SourceKind::Directory(root) => {
                let path = root.join(name).canonicalize()?;
                if !path.starts_with(&*root) {
                    return Err(DataError::Invalid("unpacked member escapes root".into()));
                }
                let mut f = std::fs::File::open(path)?;
                self.used = self
                    .used
                    .checked_add(f.metadata()?.len())
                    .ok_or_else(|| DataError::Limit("import size overflow".into()))?;
                if self.used > limits.max_uncompressed_bytes {
                    return Err(DataError::Limit("import exceeds total budget".into()));
                }
                action(&mut f)
            }
        }
    }
    pub(crate) fn bytes(
        &mut self,
        name: &str,
        limits: &ImportLimits,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, DataError> {
        self.with_member(name, limits, |r| {
            let mut out = vec![];
            let mut chunk = [0; 16384];
            loop {
                crate::npy::check(cancelled)?;
                let n = r.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                if out.len() as u64 + n as u64 > limits.max_metadata_bytes {
                    return Err(DataError::Limit("metadata exceeds byte budget".into()));
                }
                out.extend_from_slice(&chunk[..n]);
            }
            Ok(out)
        })
    }
    pub(crate) fn numeric(
        &mut self,
        name: &str,
        limits: &ImportLimits,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Samples, String), DataError> {
        self.with_member(name, limits, |mut r| {
            crate::npy::read_numeric(&mut r, limits, cancelled)
        })
    }
    pub(crate) fn strings(
        &mut self,
        name: &str,
        limits: &ImportLimits,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<String>, DataError> {
        self.with_member(name, limits, |mut r| {
            crate::npy::read_strings(&mut r, limits, cancelled)
        })
    }
    pub(crate) fn evidence(
        &mut self,
        name: &str,
        limits: &ImportLimits,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Samples, String, String, u64), DataError> {
        self.with_member(name, limits, |mut r| {
            crate::npy::read_evidence(&mut r, limits, cancelled)
        })
    }
}
fn scan_directory(
    f: &mut std::fs::File,
    limits: &ImportLimits,
) -> Result<HashSet<String>, DataError> {
    let length = f.metadata()?.len();
    let tail_len = length.min(65557) as usize;
    let mut tail = vec![0; tail_len];
    f.seek(SeekFrom::End(-(tail_len as i64)))?;
    f.read_exact(&mut tail)?;
    let pos = (0..tail_len.saturating_sub(21))
        .rev()
        .find(|&i| {
            tail[i..].starts_with(b"PK\x05\x06")
                && i + 22 + u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize == tail_len
        })
        .ok_or_else(|| DataError::Invalid("ZIP end directory is absent".into()))?;
    let e = &tail[pos..];
    let u16at = |i| u16::from_le_bytes([e[i], e[i + 1]]);
    let u32at = |i| u32::from_le_bytes(e[i..i + 4].try_into().unwrap());
    let count = u16at(10) as usize;
    let size = u32at(12) as u64;
    let offset = u32at(16) as u64;
    if u16at(4) != 0 || u16at(6) != 0 || u16at(8) != u16at(10) {
        return Err(DataError::Unsupported(
            "split ZIP archives are unsupported".into(),
        ));
    }
    if count == 65535 || size == u32::MAX as u64 || offset == u32::MAX as u64 {
        return Err(DataError::Unsupported(
            "ZIP64 archives are unsupported within the bounded import format".into(),
        ));
    }
    if count > limits.max_members
        || size > limits.max_metadata_bytes
        || offset
            .checked_add(size)
            .is_none_or(|v| v != length - tail_len as u64 + pos as u64)
    {
        return Err(DataError::Limit(
            "ZIP central directory exceeds bounds or has an ambiguous prefix".into(),
        ));
    }
    f.seek(SeekFrom::Start(offset))?;
    let mut names = HashSet::new();
    let mut folded = HashSet::new();
    let mut total = 0u64;
    let mut consumed = 0u64;
    for _ in 0..count {
        let mut h = [0; 46];
        f.read_exact(&mut h)?;
        if &h[..4] != b"PK\x01\x02" {
            return Err(DataError::Invalid("invalid ZIP central entry".into()));
        }
        let u16h = |i| u16::from_le_bytes([h[i], h[i + 1]]);
        let u32h = |i| u32::from_le_bytes(h[i..i + 4].try_into().unwrap());
        let flags = u16h(8);
        let compression = u16h(10);
        let bytes = u32h(24) as u64;
        let name_len = u16h(28) as usize;
        let extra = u16h(30) as usize;
        let comment = u16h(32) as usize;
        if flags & 1 != 0 || !matches!(compression, 0 | 8) {
            return Err(DataError::Unsupported(
                "encrypted or unsupported ZIP compression".into(),
            ));
        }
        if name_len == 0 || name_len > 4096 {
            return Err(DataError::Limit("ZIP member name exceeds bound".into()));
        }
        let mut raw = vec![0; name_len];
        f.read_exact(&mut raw)?;
        let name = std::str::from_utf8(&raw)
            .map_err(|_| DataError::Invalid("ZIP member names must be UTF-8".into()))?
            .to_string();
        let is_dir = name.ends_with('/');
        member_name(name.trim_end_matches('/'))?;
        if !folded.insert(name.to_lowercase()) {
            return Err(DataError::Invalid("duplicate ZIP member name".into()));
        }
        if !is_dir {
            names.insert(name);
        } else {
            return Err(DataError::Unsupported(
                "ZIP directory entries are unnecessary; use files with relative paths".into(),
            ));
        }
        total = total
            .checked_add(bytes)
            .ok_or_else(|| DataError::Limit("ZIP size overflow".into()))?;
        if bytes > limits.max_member_bytes || total > limits.max_uncompressed_bytes {
            return Err(DataError::Limit(
                "ZIP decompressed bytes exceed budget".into(),
            ));
        }
        let step = 46 + name_len + extra + comment;
        consumed += step as u64;
        if consumed > size {
            return Err(DataError::Invalid("ZIP directory overrun".into()));
        }
        f.seek(SeekFrom::Current((extra + comment) as i64))?;
    }
    if consumed != size {
        return Err(DataError::Invalid("ZIP directory size mismatch".into()));
    }
    Ok(names)
}
fn read_database(
    source: &mut Source,
    path: &Path,
    limits: &ImportLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportReport, DataError> {
    let bytes = source.bytes("database.json", limits, cancelled)?;
    let root: serde_json::Value = serde_json::from_slice(&bytes)?;
    if root.get("format").is_some() {
        return crate::store::read_database(source, root, path, limits, cancelled);
    }
    let descriptions = root
        .get("records")
        .and_then(|v| v.as_array())
        .ok_or_else(|| DataError::Invalid("database.json needs records array".into()))?;
    if descriptions.len() > limits.max_records {
        return Err(DataError::Limit("too many legacy records".into()));
    }
    let mut records = vec![];
    let mut warnings = vec![];
    let mut decoded = 0u64;
    for desc in descriptions {
        crate::npy::check(cancelled)?;
        let metadata = desc
            .get("metadata")
            .and_then(|v| v.as_object())
            .ok_or_else(|| DataError::Invalid("legacy record metadata missing".into()))?;
        let descriptions = desc
            .get("frames")
            .and_then(|v| v.as_array())
            .ok_or_else(|| DataError::Invalid("legacy frames missing".into()))?;
        if descriptions.len() > limits.max_frames {
            return Err(DataError::Limit("too many legacy frames".into()));
        }
        let mut frames = vec![];
        for frame in descriptions {
            let refs = frame
                .get("channels")
                .and_then(|v| v.as_object())
                .ok_or_else(|| DataError::Invalid("legacy channel references missing".into()))?;
            if refs.len() > 64 {
                return Err(DataError::Limit("too many legacy channels".into()));
            }
            let mut channels = vec![];
            for (name, file) in refs {
                crate::npy::check(cancelled)?;
                let file = file.as_str().ok_or_else(|| {
                    DataError::Invalid("legacy NPY reference must be a string".into())
                })?;
                let meta = metadata
                    .get(name)
                    .ok_or_else(|| DataError::Invalid(format!("metadata missing for {name}")))?;
                let (samples, dtype) = source.numeric(file, limits, cancelled)?;
                decoded = decoded
                    .checked_add(samples.byte_len() as u64)
                    .ok_or_else(|| DataError::Limit("decoded sample budget overflow".into()))?;
                if decoded > limits.max_uncompressed_bytes {
                    return Err(DataError::Limit(
                        "decoded samples exceed configured byte budget".into(),
                    ));
                }
                let number = |key: &str| {
                    meta.get(key).and_then(|v| v.as_f64()).ok_or_else(|| {
                        DataError::Invalid(format!("legacy calibration {key} missing"))
                    })
                };
                let expected = meta.get("dtype").and_then(|v| v.as_str()).unwrap_or("");
                let actual = match &samples {
                    Samples::U8(_) => "uint8",
                    Samples::I8(_) => "int8",
                    Samples::U16(_) => "uint16",
                    Samples::I16(_) => "int16",
                    Samples::F64(_) => {
                        if dtype.ends_with("f4") {
                            "float32"
                        } else {
                            "float64"
                        }
                    }
                };
                if expected != actual {
                    warnings.push(format!("{name}: metadata dtype {expected:?} conflicts with authoritative NPY {dtype}; retained {actual}"));
                }
                let yi = number("yincrement")?;
                let calibration = Calibration {
                    x_increment_s: number("xincrement")?,
                    x_origin_s: number("xorigin")?,
                    x_reference: number("xreference")?,
                    y_increment_v: yi,
                    y_origin: -number("yorigin")? * yi,
                    y_reference: number("yreference")?,
                };
                let channel = ChannelWaveform {
                    channel: name.clone(),
                    samples,
                    calibration,
                    metadata: serde_json::json!({"legacy_metadata":meta,"actual_npy_dtype":dtype,"legacy_y_convention":"(raw-yreference-yorigin)*yincrement","hardware_authority":false}),
                };
                channel.validate()?;
                channels.push(channel);
            }
            frames.push(WaveformFrame {
                id: crate::new_id()?,
                captured_at_unix_ms: 0,
                nominal_offset_s: frame.get("time_offset").and_then(|v| v.as_f64()),
                channels,
            });
        }
        let mut record = imported_record(path, frames)?;
        record.name = desc
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Imported legacy record")
            .into();
        record.comment = desc
            .get("comment")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into();
        record.source["legacy_record"] = desc.clone();
        record.source["legacy_created_time"] = desc
            .get("created_time")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        record.validate()?;
        records.push(record);
    }
    Ok(ImportReport { records, warnings })
}
pub(crate) fn imported_record(
    path: &Path,
    frames: Vec<WaveformFrame>,
) -> Result<WaveformRecord, DataError> {
    Ok(WaveformRecord {
        schema: 1,
        id: crate::new_id()?,
        name: path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Imported waveform")
            .into(),
        comment: String::new(),
        created_at_unix_ms: crate::unix_ms(),
        source: serde_json::json!({"kind":"imported_history","path":path.to_string_lossy(),"imported_at_unix_ms":crate::unix_ms(),"hardware_authority":false}),
        frames,
        analyses: vec![],
    })
}
pub(crate) fn uniform_calibration(time_us: &Samples) -> Result<Calibration, DataError> {
    if time_us.len() < 2 {
        return Err(DataError::Invalid(
            "calibrated import needs two or more time samples".into(),
        ));
    }
    let origin = time_us.value(0).unwrap() * 1e-6;
    let step = (time_us.value(1).unwrap() - time_us.value(0).unwrap()) * 1e-6;
    if !origin.is_finite() || !step.is_finite() || step <= 0.0 {
        return Err(DataError::Invalid(
            "time axis must be finite and strictly increasing".into(),
        ));
    }
    for i in 0..time_us.len() {
        let actual = time_us.value(i).unwrap() * 1e-6;
        let want = origin + i as f64 * step;
        if !actual.is_finite() || (actual - want).abs() > step.abs() * 1e-6 + want.abs() * 1e-10 {
            return Err(DataError::Unsupported(
                "nonuniform time axis cannot be represented by linear waveform calibration".into(),
            ));
        }
    }
    Ok(Calibration {
        x_increment_s: step,
        x_origin_s: origin,
        x_reference: 0.0,
        y_increment_v: 1.0,
        y_origin: 0.0,
        y_reference: 0.0,
    })
}
