use crate::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::{
    collections::HashMap,
    io::{BufReader, Read, Seek, Write},
    sync::{Arc, Mutex, OnceLock, Weak},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordSummary {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub created_at_unix_ms: u64,
    pub frame_count: usize,
    pub channels: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelMetadata {
    pub channel: String,
    pub sample_count: usize,
    pub dtype: String,
    pub calibration: Calibration,
    pub metadata: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameMetadata {
    pub id: String,
    pub captured_at_unix_ms: u64,
    pub nominal_offset_s: Option<f64>,
    pub channels: Vec<ChannelMetadata>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordMetadata {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub comment: String,
    pub created_at_unix_ms: u64,
    pub source: serde_json::Value,
    pub frames: Vec<FrameMetadata>,
    pub analyses: Vec<AnalysisResult>,
}
#[derive(Clone, Debug)]
pub struct DataStore {
    root: PathBuf,
    limits: ImportLimits,
    lock: Arc<Mutex<()>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SampleFile {
    path: String,
    sha256: String,
    byte_length: u64,
    count: usize,
    dtype: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredChannel {
    channel: String,
    samples: SampleFile,
    calibration: Calibration,
    metadata: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredFrame {
    id: String,
    captured_at_unix_ms: u64,
    nominal_offset_s: Option<f64>,
    channels: Vec<StoredChannel>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    id: String,
    name: String,
    comment: String,
    created_at_unix_ms: u64,
    source: serde_json::Value,
    frames: Vec<StoredFrame>,
    analyses: Vec<AnalysisResult>,
}
impl DataStore {
    /// Finalize only an experiment's run status against the latest manifest.
    /// Immutable provenance must match; labels, analyses and raw refs are retained.
    pub fn finalize_record_source(
        &self,
        id: &str,
        source: &serde_json::Value,
    ) -> Result<(), DataError> {
        let status = source["run_status"]
            .as_str()
            .filter(|s| matches!(*s, "completed" | "cancelled" | "failed" | "uncertain"))
            .ok_or_else(|| DataError::Invalid("terminal experiment run status required".into()))?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DataError::Io("store lock poisoned".into()))?;
        let dir = self.dir(id)?;
        let path = latest(&dir)?.ok_or_else(|| DataError::NotFound("record absent".into()))?;
        let mut manifest = read_manifest(&path, &self.limits)?;
        let mut incoming = source.clone();
        let mut original = manifest.source.clone();
        if manifest.id != id
            || manifest.schema != 1
            || original["kind"] != "experiment"
            || !original["run_id"].as_str().is_some_and(valid_id)
        {
            return Err(DataError::Invalid(
                "stored experiment provenance invalid".into(),
            ));
        }
        incoming
            .as_object_mut()
            .ok_or_else(|| DataError::Invalid("experiment source must be an object".into()))?
            .remove("run_status");
        original
            .as_object_mut()
            .ok_or_else(|| DataError::Invalid("stored experiment source must be an object".into()))?
            .remove("run_status");
        if incoming != original {
            return Err(DataError::Conflict(
                "immutable experiment provenance changed".into(),
            ));
        }
        manifest.source["run_status"] = serde_json::json!(status);
        self.commit(&dir, &manifest, &|| false)
    }
    /// Durably append one newly completed immutable frame. Only this frame's raw
    /// samples are validated/written; prior frames retain their committed refs.
    /// Existing labels, source and analyses win over the supplied initial header.
    /// Duplicate frame identities are rejected, never replaced.
    pub fn append_frame(
        &self,
        header: &WaveformRecord,
        frame: &WaveformFrame,
    ) -> Result<(), DataError> {
        if header.schema != 1
            || !valid_id(&header.id)
            || header.name.len() > 1024
            || header.comment.len() > 65536
            || !valid_id(&frame.id)
            || frame.channels.is_empty()
            || frame.channels.len() > 64
            || frame.nominal_offset_s.is_some_and(|v| !v.is_finite())
        {
            return Err(DataError::Invalid(
                "invalid append record/frame header".into(),
            ));
        }
        let mut names = std::collections::HashSet::new();
        let mut added = 0u64;
        for channel in &frame.channels {
            if !names.insert(&channel.channel) {
                return Err(DataError::Invalid("duplicate frame channel".into()));
            }
            channel.validate()?;
            let bytes = channel.samples.byte_len() as u64 + 256;
            if bytes > self.limits.max_member_bytes
                || channel.samples.len() > self.limits.max_samples_per_channel
            {
                return Err(DataError::Limit(
                    "new frame exceeds channel payload budget".into(),
                ));
            }
            added = added
                .checked_add(bytes)
                .ok_or_else(|| DataError::Limit("frame size overflow".into()))?;
        }
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DataError::Io("store lock poisoned".into()))?;
        let dir = self.dir(&header.id)?;
        std::fs::create_dir_all(&dir)?;
        let mut manifest = if let Some(path) = latest(&dir)? {
            read_manifest(&path, &self.limits)?
        } else {
            if !header.analyses.is_empty() {
                return Err(DataError::Invalid(
                    "initial appended frame cannot reference unsaved analyses".into(),
                ));
            }
            Manifest {
                schema: 1,
                id: header.id.clone(),
                name: header.name.clone(),
                comment: header.comment.clone(),
                created_at_unix_ms: header.created_at_unix_ms,
                source: header.source.clone(),
                frames: vec![],
                analyses: vec![],
            }
        };
        if manifest.id != header.id || manifest.schema != 1 {
            return Err(DataError::Invalid(
                "append manifest identity/schema mismatch".into(),
            ));
        }
        if manifest.frames.iter().any(|f| f.id == frame.id) {
            return Err(DataError::Conflict(
                "frame identity already committed".into(),
            ));
        }
        if manifest.frames.len() >= self.limits.max_frames.min(9999) {
            return Err(DataError::Limit("record frame budget exhausted".into()));
        }
        let total = manifest
            .frames
            .iter()
            .flat_map(|f| &f.channels)
            .try_fold(added, |n, c| n.checked_add(c.samples.byte_length))
            .ok_or_else(|| DataError::Limit("record size overflow".into()))?;
        if total > self.limits.max_uncompressed_bytes {
            return Err(DataError::Limit(
                "appended record exceeds raw byte budget".into(),
            ));
        }
        let mut channels = vec![];
        for c in &frame.channels {
            let part = dir.join(format!(".{}.part", crate::new_id()?));
            let mut pending = PendingFile::new(part.clone())?;
            crate::npy::write_numeric(&mut pending.file, &c.samples, &|| false)?;
            pending.file.sync_all()?;
            let hash = digest(&part)?;
            let filename = format!("raw-{hash}.npy");
            let target = dir.join(&filename);
            let bytes = pending.file.metadata()?.len();
            if target.exists() {
                let meta = std::fs::symlink_metadata(&target)?;
                if meta.file_type().is_symlink()
                    || !meta.is_file()
                    || meta.len() != bytes
                    || digest(&target)? != hash
                {
                    return Err(DataError::Conflict(
                        "existing same-hash payload was altered".into(),
                    ));
                }
            } else {
                std::fs::rename(&part, &target)?;
                pending.published = true;
            }
            let dtype = match c.samples {
                Samples::U8(_) => "|u1",
                Samples::I8(_) => "|i1",
                Samples::U16(_) => "<u2",
                Samples::I16(_) => "<i2",
                Samples::F64(_) => "<f8",
            }
            .into();
            channels.push(StoredChannel {
                channel: c.channel.clone(),
                samples: SampleFile {
                    path: filename,
                    sha256: hash,
                    byte_length: bytes,
                    count: c.samples.len(),
                    dtype,
                },
                calibration: c.calibration.clone(),
                metadata: c.metadata.clone(),
            });
        }
        manifest.frames.push(StoredFrame {
            id: frame.id.clone(),
            captured_at_unix_ms: frame.captured_at_unix_ms,
            nominal_offset_s: frame.nominal_offset_s,
            channels,
        });
        self.commit(&dir, &manifest, &|| false)
    }
    /// Atomically append an analysis to the latest metadata, preserving concurrent
    /// labels and analyses without re-reading or rewriting immutable raw samples.
    pub fn append_analysis(&self, result: AnalysisResult) -> Result<(), DataError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DataError::Io("store lock poisoned".into()))?;
        let dir = self.dir(&result.record_id)?;
        let path = latest(&dir)?.ok_or_else(|| DataError::NotFound("record absent".into()))?;
        let mut manifest = read_manifest(&path, &self.limits)?;
        if manifest.schema != 1
            || manifest.id != result.record_id
            || result.schema != 1
            || !valid_id(&result.id)
        {
            return Err(DataError::Invalid(
                "analysis/record identity or schema invalid".into(),
            ));
        }
        let channel = manifest
            .frames
            .iter()
            .find(|f| f.id == result.frame_id)
            .and_then(|f| f.channels.iter().find(|c| c.channel == result.channel))
            .ok_or_else(|| DataError::Invalid("analysis frame/channel absent".into()))?;
        result.roi.validate(channel.samples.count)?;
        if result.curve.len() > 8192
            || result.markers.len() > 8192
            || result
                .curve
                .iter()
                .chain(&result.markers)
                .any(|p| !p.x.is_finite() || !p.y.is_finite())
        {
            return Err(DataError::Invalid(
                "analysis overlay invalid or over budget".into(),
            ));
        }
        if let Some(old) = manifest.analyses.iter().find(|a| a.id == result.id) {
            return if old == &result {
                Ok(())
            } else {
                Err(DataError::Conflict("analysis identity is immutable".into()))
            };
        }
        manifest.analyses.push(result);
        self.commit(&dir, &manifest, &|| false)
    }
    pub fn metadata(&self, id: &str) -> Result<RecordMetadata, DataError> {
        let dir = self.dir(id)?;
        let path = latest(&dir)?.ok_or_else(|| DataError::NotFound("record absent".into()))?;
        let m = read_manifest(&path, &self.limits)?;
        if m.id != id || m.schema != 1 || m.frames.len() > self.limits.max_frames {
            return Err(DataError::Invalid(
                "stored metadata identity/schema/frame count invalid".into(),
            ));
        }
        Ok(RecordMetadata {
            schema: m.schema,
            id: m.id,
            name: m.name,
            comment: m.comment,
            created_at_unix_ms: m.created_at_unix_ms,
            source: m.source,
            frames: m
                .frames
                .into_iter()
                .map(|f| FrameMetadata {
                    id: f.id,
                    captured_at_unix_ms: f.captured_at_unix_ms,
                    nominal_offset_s: f.nominal_offset_s,
                    channels: f
                        .channels
                        .into_iter()
                        .map(|c| ChannelMetadata {
                            channel: c.channel,
                            sample_count: c.samples.count,
                            dtype: c.samples.dtype,
                            calibration: c.calibration,
                            metadata: c.metadata,
                        })
                        .collect(),
                })
                .collect(),
            analyses: m.analyses,
        })
    }
    pub fn viewport(
        &self,
        id: &str,
        frame_id: &str,
        channel: &str,
        roi: SampleRange,
        max_points: usize,
    ) -> Result<Vec<PlotPoint>, DataError> {
        self.viewport_with_cancel(id, frame_id, channel, roi, max_points, &|| false)
    }
    pub fn viewport_with_cancel(
        &self,
        id: &str,
        frame_id: &str,
        channel: &str,
        roi: SampleRange,
        max_points: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<PlotPoint>, DataError> {
        crate::npy::check(cancelled)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DataError::Io("store lock poisoned".into()))?;
        let dir = self.dir(id)?;
        let path = latest(&dir)?.ok_or_else(|| DataError::NotFound("record absent".into()))?;
        let m = read_manifest(&path, &self.limits)?;
        if m.id != id || m.schema != 1 {
            return Err(DataError::Invalid(
                "stored viewport identity/schema invalid".into(),
            ));
        }
        let frame = m
            .frames
            .iter()
            .find(|f| f.id == frame_id)
            .ok_or_else(|| DataError::NotFound("viewport frame absent".into()))?;
        let c = frame
            .channels
            .iter()
            .find(|c| c.channel == channel)
            .ok_or_else(|| DataError::NotFound("viewport channel absent".into()))?;
        let sample = &c.samples;
        roi.validate(sample.count)?;
        if sample.path != format!("raw-{}.npy", sample.sha256)
            || sample.sha256.len() != 64
            || !sample
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(DataError::Invalid(
                "invalid viewport payload reference".into(),
            ));
        }
        let path = dir.join(&sample.path);
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != sample.byte_length {
            return Err(DataError::Invalid(
                "viewport raw file length/kind mismatch".into(),
            ));
        }
        if meta.len() > self.limits.max_member_bytes
            || meta.len() > self.limits.max_uncompressed_bytes
        {
            return Err(DataError::Limit(
                "viewport payload exceeds byte budget".into(),
            ));
        }
        let mut reader = BufReader::with_capacity(16384, std::fs::File::open(path)?);
        drop(_guard);
        let (points, dtype, sha, bytes, count) = crate::npy::read_viewport(
            &mut reader,
            &self.limits,
            &c.calibration,
            roi,
            max_points,
            cancelled,
        )?;
        if dtype != sample.dtype
            || sha != sample.sha256
            || bytes != sample.byte_length
            || count != sample.count
        {
            return Err(DataError::Invalid(
                "viewport checksum, dtype or shape mismatch".into(),
            ));
        }
        Ok(points)
    }
    pub fn new(path: impl AsRef<Path>) -> Result<Self, DataError> {
        Self::with_limits(path, ImportLimits::default())
    }
    pub fn with_limits(path: impl AsRef<Path>, limits: ImportLimits) -> Result<Self, DataError> {
        std::fs::create_dir_all(path.as_ref())?;
        let root = path.as_ref().canonicalize()?;
        static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
        let mut locks = LOCKS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| DataError::Io("store lock poisoned".into()))?;
        let lock = locks.get(&root).and_then(Weak::upgrade).unwrap_or_else(|| {
            let lock = Arc::new(Mutex::new(()));
            locks.insert(root.clone(), Arc::downgrade(&lock));
            lock
        });
        Ok(Self { root, limits, lock })
    }
    fn dir(&self, id: &str) -> Result<PathBuf, DataError> {
        if !valid_id(id) {
            return Err(DataError::Invalid(
                "record ID must be 32 lowercase hexadecimal digits".into(),
            ));
        }
        let dir = self.root.join(id);
        if let Ok(meta) = std::fs::symlink_metadata(&dir) {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(DataError::Invalid(
                    "store record path is not a regular directory".into(),
                ));
            }
        }
        Ok(dir)
    }
    pub fn list(&self) -> Result<Vec<RecordSummary>, DataError> {
        let mut result = vec![];
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) {
                continue;
            }
            let dir = self.dir(&id)?;
            let Some(path) = latest(&dir)? else {
                continue;
            };
            let m = read_manifest(&path, &self.limits)?;
            if m.id != id || m.schema != 1 {
                return Err(DataError::Invalid(
                    "stored record identity/schema mismatch".into(),
                ));
            }
            let mut channels = vec![];
            for f in &m.frames {
                for c in &f.channels {
                    if !channels.contains(&c.channel) {
                        channels.push(c.channel.clone());
                    }
                }
            }
            result.push(RecordSummary {
                id: m.id,
                name: m.name,
                comment: m.comment,
                created_at_unix_ms: m.created_at_unix_ms,
                frame_count: m.frames.len(),
                channels,
            });
        }
        result.sort_by(|a, b| {
            b.created_at_unix_ms
                .cmp(&a.created_at_unix_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(result)
    }
    pub fn load(&self, id: &str) -> Result<WaveformRecord, DataError> {
        self.load_with_cancel(id, &|| false)
    }
    pub fn load_with_cancel(
        &self,
        id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WaveformRecord, DataError> {
        crate::npy::check(cancelled)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DataError::Io("store lock poisoned".into()))?;
        self.load_inner(id, cancelled)
    }
    fn load_inner(
        &self,
        id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WaveformRecord, DataError> {
        let dir = self.dir(id)?;
        let path =
            latest(&dir)?.ok_or_else(|| DataError::NotFound(format!("record {id} is absent")))?;
        let m = read_manifest(&path, &self.limits)?;
        if m.schema != 1 || m.id != id || m.frames.len() > self.limits.max_frames {
            return Err(DataError::Invalid(
                "stored record schema, identity or frame count invalid".into(),
            ));
        }
        let mut frames = vec![];
        let mut total = 0u64;
        for f in m.frames {
            crate::npy::check(cancelled)?;
            let mut channels = vec![];
            for c in f.channels {
                let sample = &c.samples;
                if sample.sha256.len() != 64
                    || !sample
                        .sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || sample.path != format!("raw-{}.npy", sample.sha256)
                {
                    return Err(DataError::Invalid(
                        "invalid stored payload reference".into(),
                    ));
                }
                let path = dir.join(&sample.path);
                let meta = std::fs::symlink_metadata(&path)?;
                if !meta.is_file()
                    || meta.file_type().is_symlink()
                    || meta.len() != sample.byte_length
                {
                    return Err(DataError::Invalid(
                        "stored payload length or file kind mismatch".into(),
                    ));
                }
                total = total
                    .checked_add(meta.len())
                    .ok_or_else(|| DataError::Limit("store size overflow".into()))?;
                if meta.len() > self.limits.max_member_bytes
                    || total > self.limits.max_uncompressed_bytes
                {
                    return Err(DataError::Limit(
                        "stored raw payload exceeds configured byte budget".into(),
                    ));
                }
                let mut reader = BufReader::new(std::fs::File::open(path)?);
                let (samples, dtype, hash, bytes) =
                    crate::npy::read_evidence(&mut reader, &self.limits, cancelled)?;
                if samples.len() != sample.count
                    || dtype != sample.dtype
                    || hash != sample.sha256
                    || bytes != sample.byte_length
                {
                    return Err(DataError::Invalid(
                        "stored payload dtype/shape mismatch".into(),
                    ));
                }
                channels.push(ChannelWaveform {
                    channel: c.channel,
                    samples,
                    calibration: c.calibration,
                    metadata: c.metadata,
                });
            }
            frames.push(WaveformFrame {
                id: f.id,
                captured_at_unix_ms: f.captured_at_unix_ms,
                nominal_offset_s: f.nominal_offset_s,
                channels,
            });
        }
        let record = WaveformRecord {
            schema: m.schema,
            id: m.id,
            name: m.name,
            comment: m.comment,
            created_at_unix_ms: m.created_at_unix_ms,
            source: m.source,
            frames,
            analyses: m.analyses,
        };
        record.validate_with_cancel(cancelled)?;
        Ok(record)
    }
    pub fn save(&self, record: &WaveformRecord) -> Result<(), DataError> {
        self.save_with_cancel(record, &|| false)
    }
    pub fn save_with_cancel(
        &self,
        record: &WaveformRecord,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), DataError> {
        self.save_labels_with_cancel(record, &record.name, &record.comment, cancelled)
    }
    /// Commit user labels atomically with borrowed immutable raw samples.
    pub fn save_with_labels(&self, record: &WaveformRecord, name: &str, comment: &str) -> Result<(), DataError> {
        self.save_labels_with_cancel(record, name, comment, &|| false)
    }
    fn save_labels_with_cancel(&self, record: &WaveformRecord, name: &str, comment: &str, cancelled: &dyn Fn() -> bool) -> Result<(), DataError> {
        if name.len() > 1024 || comment.len() > 65536 {
            return Err(DataError::Limit("record labels exceed limits".into()));
        }
        crate::npy::check(cancelled)?;
        record.validate_with_cancel(cancelled)?;
        let mut total = 0u64;
        for f in &record.frames {
            for c in &f.channels {
                total = total
                    .checked_add(c.samples.byte_len() as u64 + 256)
                    .ok_or_else(|| DataError::Limit("store byte count overflow".into()))?;
                if c.samples.byte_len() as u64 + 256 > self.limits.max_member_bytes
                    || c.samples.len() > self.limits.max_samples_per_channel
                    || total > self.limits.max_uncompressed_bytes
                {
                    return Err(DataError::Limit(
                        "record raw payload exceeds configured byte budget".into(),
                    ));
                }
            }
        }
        if record.frames.len() > self.limits.max_frames {
            return Err(DataError::Limit(
                "record frames exceed configured budget".into(),
            ));
        }
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DataError::Io("store lock poisoned".into()))?;
        let dir = self.dir(&record.id)?;
        std::fs::create_dir_all(&dir)?;
        let old = latest(&dir)?
            .map(|p| read_manifest(&p, &self.limits))
            .transpose()?;
        let mut frames = vec![];
        for f in &record.frames {
            let mut channels = vec![];
            for c in &f.channels {
                crate::npy::check(cancelled)?;
                let part = dir.join(format!(".{}.part", crate::new_id()?));
                let mut pending = PendingFile::new(part.clone())?;
                crate::npy::write_numeric(&mut pending.file, &c.samples, cancelled)?;
                pending.file.sync_all()?;
                let hash = digest(&part)?;
                let filename = format!("raw-{hash}.npy");
                let target = dir.join(&filename);
                let bytes = pending.file.metadata()?.len();
                drop(pending.file.try_clone()?);
                if target.exists() {
                    if std::fs::symlink_metadata(&target)?.file_type().is_symlink()
                        || digest(&target)? != hash
                    {
                        return Err(DataError::Conflict(
                            "existing raw payload was altered".into(),
                        ));
                    }
                } else {
                    std::fs::rename(&part, &target)?;
                    pending.published = true;
                }
                let dtype = match c.samples {
                    Samples::U8(_) => "|u1",
                    Samples::I8(_) => "|i1",
                    Samples::U16(_) => "<u2",
                    Samples::I16(_) => "<i2",
                    Samples::F64(_) => "<f8",
                }
                .to_string();
                if let Some(previous) = old
                    .as_ref()
                    .and_then(|m| m.frames.iter().find(|g| g.id == f.id))
                    .and_then(|g| g.channels.iter().find(|ch| ch.channel == c.channel))
                {
                    if previous.samples.sha256 != hash || previous.calibration != c.calibration {
                        return Err(DataError::Conflict("existing frame samples and calibration are immutable; save a new record/frame identity".into()));
                    }
                }
                channels.push(StoredChannel {
                    channel: c.channel.clone(),
                    samples: SampleFile {
                        path: filename,
                        sha256: hash,
                        byte_length: bytes,
                        count: c.samples.len(),
                        dtype,
                    },
                    calibration: c.calibration.clone(),
                    metadata: c.metadata.clone(),
                });
            }
            frames.push(StoredFrame {
                id: f.id.clone(),
                captured_at_unix_ms: f.captured_at_unix_ms,
                nominal_offset_s: f.nominal_offset_s,
                channels,
            });
        }
        let manifest = Manifest {
            schema: record.schema,
            id: record.id.clone(),
            name: name.into(),
            comment: comment.into(),
            created_at_unix_ms: record.created_at_unix_ms,
            source: record.source.clone(),
            frames,
            analyses: record.analyses.clone(),
        };
        self.commit(&dir, &manifest, cancelled)
    }
    fn commit(
        &self,
        dir: &Path,
        manifest: &Manifest,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), DataError> {
        let bytes = serde_json::to_vec_pretty(manifest)?;
        if bytes.len() as u64 > self.limits.max_metadata_bytes {
            return Err(DataError::Limit(
                "record manifest exceeds metadata byte budget".into(),
            ));
        }
        crate::npy::check(cancelled)?;
        let seq = latest(&dir)?
            .and_then(|p| sequence(&p))
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| DataError::Limit("store commit sequence exhausted".into()))?;
        let target = dir.join(format!("commit-{seq:020}-{}.json", crate::new_id()?));
        let part = dir.join(format!(".{}.part", crate::new_id()?));
        let mut pending = PendingFile::new(part.clone())?;
        pending.file.write_all(&bytes)?;
        pending.file.sync_all()?;
        crate::npy::check(cancelled)?;
        std::fs::rename(part, target)?;
        pending.published = true;
        Ok(())
    }
    pub fn rename(&self, id: &str, name: &str, comment: &str) -> Result<(), DataError> {
        self.rename_labels(id, name, Some(comment))
    }
    /// Change the name while preserving the latest comment, analyses and raw refs.
    pub fn rename_name(&self, id: &str, name: &str) -> Result<(), DataError> {
        self.rename_labels(id, name, None)
    }
    fn rename_labels(&self, id: &str, name: &str, comment: Option<&str>) -> Result<(), DataError> {
        if name.len() > 1024 || comment.is_some_and(|text| text.len() > 65536) {
            return Err(DataError::Limit("record labels exceed limits".into()));
        }
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DataError::Io("store lock poisoned".into()))?;
        let dir = self.dir(id)?;
        let path = latest(&dir)?.ok_or_else(|| DataError::NotFound("record absent".into()))?;
        let mut manifest = read_manifest(&path, &self.limits)?;
        if manifest.id != id || manifest.schema != 1 {
            return Err(DataError::Invalid("record identity/schema mismatch".into()));
        }
        manifest.name = name.into();
        if let Some(comment) = comment { manifest.comment = comment.into(); }
        self.commit(&dir, &manifest, &|| false)
    }
    /// Recoverable deletion: atomically moves this record into the store's .trash directory.
    pub fn remove(&self, id: &str) -> Result<(), DataError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DataError::Io("store lock poisoned".into()))?;
        let dir = self.dir(id)?;
        if latest(&dir)?.is_none() {
            return Err(DataError::NotFound("record absent".into()));
        }
        let trash = self.root.join(".trash");
        if trash.exists() && std::fs::symlink_metadata(&trash)?.file_type().is_symlink() {
            return Err(DataError::Invalid(
                "store trash must not be a symlink".into(),
            ));
        }
        std::fs::create_dir_all(&trash)?;
        std::fs::rename(dir, trash.join(format!("{id}-{}", crate::new_id()?)))?;
        Ok(())
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
}
fn sequence(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    if !name.starts_with("commit-") || !name.ends_with(".json") {
        return None;
    }
    name.get(7..27)?.parse().ok()
}
fn latest(dir: &Path) -> Result<Option<PathBuf>, DataError> {
    if !dir.exists() {
        return Ok(None);
    }
    let mut latest = None;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if sequence(&path).is_some()
            && latest
                .as_ref()
                .is_none_or(|p: &PathBuf| path.file_name() > p.file_name())
        {
            latest = Some(path);
        }
    }
    Ok(latest)
}
fn read_manifest(path: &Path, limits: &ImportLimits) -> Result<Manifest, DataError> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > limits.max_metadata_bytes {
        return Err(DataError::Limit(
            "manifest file kind or byte budget invalid".into(),
        ));
    }
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}
pub(crate) fn digest(path: &Path) -> Result<String, DataError> {
    let mut r = BufReader::new(std::fs::File::open(path)?);
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut chunk = [0; 16384];
    loop {
        let n = r.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        context.update(&chunk[..n]);
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
struct PendingFile {
    file: std::fs::File,
    path: PathBuf,
    published: bool,
}
impl PendingFile {
    fn new(path: PathBuf) -> Result<Self, DataError> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            file,
            path,
            published: false,
        })
    }
}
impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeDatabase {
    schema: u32,
    format: String,
    records: Vec<Manifest>,
}
/// Self-contained raw-preserving ZIP/NPY archive; manifests never contain sample arrays.
pub(crate) fn write_database(
    w: impl Write + Seek,
    record: &WaveformRecord,
    frame_ids: &[String],
    cancelled: &dyn Fn() -> bool,
) -> Result<(), DataError> {
    let mut zip = zip::ZipWriter::new(w);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut frames = vec![];
    for (fi, f) in record.frames.iter().enumerate() {
        if !frame_ids.is_empty() && !frame_ids.contains(&f.id) {
            continue;
        }
        let mut channels = vec![];
        for (ci, c) in f.channels.iter().enumerate() {
            crate::npy::check(cancelled)?;
            let path = format!("frame{fi:04}_channel{ci:02}.npy");
            zip.start_file(&path, options)?;
            let (sha256, byte_length) =
                crate::npy::write_evidence(&mut zip, &c.samples, cancelled)?;
            let dtype = match c.samples {
                Samples::U8(_) => "|u1",
                Samples::I8(_) => "|i1",
                Samples::U16(_) => "<u2",
                Samples::I16(_) => "<i2",
                Samples::F64(_) => "<f8",
            }
            .into();
            channels.push(StoredChannel {
                channel: c.channel.clone(),
                samples: SampleFile {
                    path,
                    sha256,
                    byte_length,
                    count: c.samples.len(),
                    dtype,
                },
                calibration: c.calibration.clone(),
                metadata: c.metadata.clone(),
            });
        }
        frames.push(StoredFrame {
            id: f.id.clone(),
            captured_at_unix_ms: f.captured_at_unix_ms,
            nominal_offset_s: f.nominal_offset_s,
            channels,
        });
    }
    if frames.is_empty() {
        return Err(DataError::Invalid(
            "native export has no selected frames".into(),
        ));
    }
    let selected_analyses = record
        .analyses
        .iter()
        .filter(|a| frames.iter().any(|f| f.id == a.frame_id))
        .cloned()
        .collect();
    let manifest = Manifest {
        schema: record.schema,
        id: record.id.clone(),
        name: record.name.clone(),
        comment: record.comment.clone(),
        created_at_unix_ms: record.created_at_unix_ms,
        source: record.source.clone(),
        frames,
        analyses: selected_analyses,
    };
    let db = NativeDatabase {
        schema: 1,
        format: "yang_waveform_db".into(),
        records: vec![manifest],
    };
    let bytes = serde_json::to_vec_pretty(&db)?;
    if bytes.len() > 16_777_216 {
        return Err(DataError::Limit(
            "native DB metadata exceeds 16MiB budget".into(),
        ));
    }
    crate::npy::check(cancelled)?;
    zip.start_file("database.json", options)?;
    zip.write_all(&bytes)?;
    zip.finish()?;
    Ok(())
}
pub(crate) fn read_database(
    source: &mut crate::legacy::Source,
    value: serde_json::Value,
    path: &Path,
    limits: &ImportLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportReport, DataError> {
    let db: NativeDatabase = serde_json::from_value(value)?;
    if db.schema != 1 || db.format != "yang_waveform_db" {
        return Err(DataError::Unsupported(
            "unknown native database format/schema".into(),
        ));
    }
    if db.records.len() > limits.max_records {
        return Err(DataError::Limit(
            "native DB record count exceeds budget".into(),
        ));
    }
    let mut records = vec![];
    let mut total = 0u64;
    let mut ids = std::collections::HashSet::new();
    for m in db.records {
        crate::npy::check(cancelled)?;
        if m.frames.len() > limits.max_frames || m.schema != 1 || !ids.insert(m.id.clone()) {
            return Err(DataError::Invalid(
                "invalid native record/frame schema or duplicate identity".into(),
            ));
        }
        let mut frames = vec![];
        for f in m.frames {
            let mut channels = vec![];
            if f.channels.len() > 64 {
                return Err(DataError::Limit(
                    "native DB channel count exceeds budget".into(),
                ));
            }
            for c in f.channels {
                crate::npy::check(cancelled)?;
                let reference = c.samples;
                if reference.count > limits.max_samples_per_channel
                    || reference.byte_length > limits.max_member_bytes
                {
                    return Err(DataError::Limit(
                        "native DB declared raw payload exceeds budget".into(),
                    ));
                }
                let (samples, dtype, sha, bytes) =
                    source.evidence(&reference.path, limits, cancelled)?;
                total = total
                    .checked_add(samples.byte_len() as u64)
                    .ok_or_else(|| DataError::Limit("native DB size overflow".into()))?;
                if total > limits.max_uncompressed_bytes {
                    return Err(DataError::Limit(
                        "native DB decoded samples exceed budget".into(),
                    ));
                }
                if samples.len() != reference.count
                    || dtype != reference.dtype
                    || sha != reference.sha256
                    || bytes != reference.byte_length
                {
                    return Err(DataError::Invalid(
                        "native DB raw payload checksum, shape or dtype mismatch".into(),
                    ));
                }
                channels.push(ChannelWaveform {
                    channel: c.channel,
                    samples,
                    calibration: c.calibration,
                    metadata: c.metadata,
                });
            }
            frames.push(WaveformFrame {
                id: f.id,
                captured_at_unix_ms: f.captured_at_unix_ms,
                nominal_offset_s: f.nominal_offset_s,
                channels,
            });
        }
        let mut provenance = serde_json::json!({"kind":"imported_history","original_source":m.source,"path":path.to_string_lossy(),"hardware_authority":false,"imported_at_unix_ms":crate::unix_ms()});
        provenance["native_record_id"] = serde_json::json!(m.id);
        let record = WaveformRecord {
            schema: m.schema,
            id: m.id,
            name: m.name,
            comment: m.comment,
            created_at_unix_ms: m.created_at_unix_ms,
            source: provenance,
            frames,
            analyses: m.analyses,
        };
        record.validate()?;
        records.push(record);
    }
    Ok(ImportReport {
        records,
        warnings: vec![],
    })
}
