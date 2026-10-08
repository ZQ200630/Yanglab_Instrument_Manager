//! Bounded worker-owned delivery staging, never a durable archive.
use crate::{
    capture_files::{self as files, Identity},
    WorkerError,
};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};
use yang_drivers::osa::TraceCapture;
use yang_protocol::{valid_id, CaptureDescriptor};

const MAX_ENTRIES: usize = 32;
const MAX_BYTES: u64 = 128 * 1024 * 1024;
const MAX_METADATA: usize = 8192;
const MAX_NATIVE: u64 = 200001 * 16;
const MAX_CHUNK: usize = 16384;
const ACK_HISTORY: usize = 256;

pub trait Durability: Send + Sync {
    fn sync(&self, file: &File) -> std::io::Result<()>;
}
struct DiskDurability;
impl Durability for DiskDurability {
    fn sync(&self, file: &File) -> std::io::Result<()> {
        file.sync_all()
    }
}
struct Entry {
    descriptor: CaptureDescriptor,
    native: Identity,
    manifest: Identity,
}
pub struct CaptureSpool {
    root: PathBuf,
    nonce: String,
    root_id: Identity,
    entries: HashMap<String, Entry>,
    partial: HashMap<(String, String), Identity>,
    signatures: HashMap<String, String>,
    acked: VecDeque<(String, String)>,
    durability: Arc<dyn Durability>,
}
fn error(e: impl std::fmt::Display) -> WorkerError {
    files::error(e)
}
fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
// Gregorian civil date, UTC, microsecond precision: matches the existing schema
// without a locale, timezone database or new runtime dependency.
fn stamp(time: SystemTime) -> Result<String, WorkerError> {
    let elapsed = time.duration_since(SystemTime::UNIX_EPOCH).map_err(error)?;
    if elapsed.as_secs() > 253402300799 {
        return Err(error("Capture timestamp exceeds year 9999"));
    }
    let days = (elapsed.as_secs() / 86400) as i64 + 719468;
    let era = days / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let seconds = elapsed.as_secs() % 86400;
    let base = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    Ok(if elapsed.subsec_micros() == 0 {
        format!("{base}+00:00")
    } else {
        format!("{base}.{:06}+00:00", elapsed.subsec_micros())
    })
}
impl CaptureSpool {
    pub(crate) fn ensure_read_capacity(&self, pending: usize) -> Result<(), WorkerError> {
        let _pins = self.pins()?;
        let (groups, size) = self.budget()?;
        if groups.saturating_add(pending) >= MAX_ENTRIES
            || size
                .checked_add((pending as u64 + 1) * (MAX_NATIVE + MAX_METADATA as u64 + 1024))
                .is_none_or(|n| n > MAX_BYTES)
        {
            return Err(WorkerError::new(
                "CaptureCapacity",
                "Staging capacity is unavailable; no instrument read started",
            ));
        }
        Ok(())
    }
    pub(crate) fn ownership_nonce(&self) -> &str {
        &self.nonce
    }
    pub(crate) fn descriptor(&self, id: &str) -> Result<&CaptureDescriptor, WorkerError> {
        self.entries
            .get(id)
            .map(|e| &e.descriptor)
            .ok_or_else(|| error("Unknown capture"))
    }
    pub fn open(root: PathBuf, nonce: String) -> Result<Self, WorkerError> {
        Self::with_durability(root, nonce, Arc::new(DiskDurability))
    }
    /// Inject only the final durability operation; containment and all actual
    /// file opens/writes remain guarded. Production uses File::sync_all.
    pub fn with_durability(
        root: PathBuf,
        nonce: String,
        durability: Arc<dyn Durability>,
    ) -> Result<Self, WorkerError> {
        if !valid_id(&nonce) || root.file_name().and_then(|n| n.to_str()) != Some(nonce.as_str()) {
            return Err(error("Spool must be named by its ownership nonce"));
        }
        let pins = files::pin(&root)?;
        let root_id = files::identity(pins.last().unwrap(), true)?;
        let result = Self {
            root,
            nonce,
            root_id,
            entries: HashMap::new(),
            partial: HashMap::new(),
            signatures: HashMap::new(),
            acked: VecDeque::new(),
            durability,
        };
        result.budget()?;
        Ok(result)
    }
    fn pins(&self) -> Result<Vec<File>, WorkerError> {
        let pins = files::pin(&self.root)?;
        if files::identity(pins.last().unwrap(), true)? != self.root_id {
            return Err(error("Staging directory identity changed"));
        }
        Ok(pins)
    }
    fn budget(&self) -> Result<(usize, u64), WorkerError> {
        let mut groups = BTreeSet::new();
        let mut size = 0_u64;
        let mut count = 0;
        for entry in fs::read_dir(&self.root).map_err(error)? {
            count += 1;
            if count > MAX_ENTRIES * 2 {
                return Err(error("Staging file count exceeds capacity"));
            }
            let entry = entry.map_err(error)?;
            let file = files::open(&entry.path(), false, false, false)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| error("Invalid orphan file name"))?;
            let key = match name.rsplit_once('.') {
                Some((id, "bin" | "json")) if valid_id(id) => id.to_owned(),
                _ => name,
            };
            groups.insert(key);
            size = size
                .checked_add(file.metadata().map_err(error)?.len())
                .ok_or_else(|| error("Staging size overflow"))?;
            if groups.len() > MAX_ENTRIES || size > MAX_BYTES {
                return Err(error("Staging physical capacity exceeded"));
            }
        }
        Ok((groups.len(), size))
    }
    fn write_new(&mut self, id: &str, suffix: &str, bytes: &[u8]) -> Result<Identity, WorkerError> {
        use std::io::{Seek, SeekFrom};
        let key = (id.to_owned(), suffix.to_owned());
        let path = self.root.join(format!("{id}.{suffix}"));
        let mut file = match self.partial.get(&key) {
            Some(expected) => files::repair(&path, *expected)?,
            None => files::open(&path, false, true, false)?,
        };
        let identity = files::identity(&file, false)?;
        self.partial.insert(key, identity);
        // Explicit storage retry may repair our own partial file, never an
        // archive, unknown orphan, linked file, or a new hardware measurement.
        file.seek(SeekFrom::Start(0)).map_err(error)?;
        file.set_len(0).map_err(error)?;
        for chunk in bytes.chunks(65536) {
            file.write_all(chunk).map_err(error)?;
        }
        file.flush().map_err(error)?;
        self.durability.sync(&file).map_err(error)?;
        Ok(identity)
    }
    pub fn stage(&mut self, capture: &TraceCapture) -> Result<CaptureDescriptor, WorkerError> {
        self.stage_owned(&crate::new_id()?, capture)
    }
    pub fn stage_owned(
        &mut self,
        id: &str,
        capture: &TraceCapture,
    ) -> Result<CaptureDescriptor, WorkerError> {
        if !valid_id(id)
            || self.acked.iter().any(|(old, _)| old == id)
            || self.entries.contains_key(id)
        {
            return Err(error(
                "Capture staging identity already completed or invalid",
            ));
        }
        let count = capture.native_values().len();
        let byte_count = (count as u64)
            .checked_mul(16)
            .filter(|n| *n <= MAX_NATIVE)
            .ok_or_else(|| error("Capture sample count exceeds limit"))?;
        let metadata = serde_json::json!({"native_unit":capture.native_unit(),"trace":capture.trace(),"identity":capture.identity(),"read_started_at":stamp(capture.timing().started_utc)?,"read_finished_at":stamp(capture.timing().finished_utc)?,"elapsed_s":capture.timing().elapsed.as_secs_f64(),"context_before":capture.context_before(),"context_after":capture.context_after(),"consistency":capture.consistency()});
        if serde_json::to_vec(&metadata).map_err(error)?.len() > MAX_METADATA {
            return Err(error("Capture metadata exceeds envelope"));
        }
        let _pins = self.pins()?;
        let (groups, size) = self.budget()?;
        // Conservative descriptor overhead is small and bounded; budget and
        // count are checked before allocating the native payload.
        if self.entries.len() >= MAX_ENTRIES
            || (self.signatures.len() >= MAX_ENTRIES && !self.signatures.contains_key(id))
            || (groups >= MAX_ENTRIES && !self.partial.keys().any(|(key, _)| key == id))
            || size
                .checked_add(byte_count + MAX_METADATA as u64 + 1024)
                .is_none_or(|n| n > MAX_BYTES)
        {
            return Err(error("Capture staging capacity exhausted"));
        }
        let id = id.to_owned();
        let mut payload = Vec::with_capacity(byte_count as usize);
        for (&x, &y) in capture.wavelength_nm().iter().zip(capture.native_values()) {
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
        }
        let descriptor = CaptureDescriptor {
            schema: 1,
            kind: "osa_trace".into(),
            capture_id: id.clone(),
            point_count: count as u64,
            byte_count,
            sha256: sha256(&payload),
            metadata,
        };
        let encoded = serde_json::to_vec(&descriptor).map_err(error)?;
        let signature = sha256(&encoded);
        if self
            .signatures
            .get(&id)
            .is_some_and(|old| old != &signature)
        {
            return Err(error("Storage retry cannot replace the original capture"));
        }
        self.signatures.insert(id.clone(), signature);
        let native = self.write_new(&id, "bin", &payload)?;
        let manifest = self.write_new(&id, "json", &encoded)?;
        self.entries.insert(
            id.clone(),
            Entry {
                descriptor: descriptor.clone(),
                native,
                manifest,
            },
        );
        self.partial.retain(|(key, _), _| key != &id);
        self.signatures.remove(&id);
        Ok(descriptor)
    }
    fn authorize(&self, nonce: &str, id: &str) -> Result<(), WorkerError> {
        if nonce != self.nonce || !valid_id(id) {
            return Err(error("Capture nonce or identity mismatch"));
        }
        Ok(())
    }
    pub fn read_chunk(
        &self,
        nonce: &str,
        id: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, WorkerError> {
        self.authorize(nonce, id)?;
        if !(1..=MAX_CHUNK).contains(&length) {
            return Err(error("Capture chunk exceeds limit"));
        }
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| error("Unknown capture"))?;
        let end = offset
            .checked_add(length as u64)
            .filter(|n| *n <= entry.descriptor.byte_count)
            .ok_or_else(|| error("Capture chunk outside native bytes"))?;
        let _pins = self.pins()?;
        let payload = files::read(
            &self.root.join(format!("{id}.bin")),
            entry.native,
            MAX_NATIVE,
        )?;
        if payload.len() as u64 != entry.descriptor.byte_count
            || sha256(&payload) != entry.descriptor.sha256
        {
            return Err(error("Staged length/hash changed"));
        }
        Ok(payload[offset as usize..end as usize].to_vec())
    }
    pub fn ack(&mut self, nonce: &str, id: &str, sha256: &str) -> Result<(), WorkerError> {
        self.authorize(nonce, id)?;
        if let Some((_, hash)) = self.acked.iter().find(|(old, _)| old == id) {
            return if hash == sha256 {
                Ok(())
            } else {
                Err(error("ACK hash mismatch"))
            };
        }
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| error("Unknown capture ACK"))?;
        if entry.descriptor.sha256 != sha256 {
            return Err(error("ACK hash mismatch"));
        }
        let _pins = self.pins()?;
        files::delete(&self.root.join(format!("{id}.bin")), entry.native)?;
        files::delete(&self.root.join(format!("{id}.json")), entry.manifest)?;
        self.entries.remove(id);
        self.acked.push_back((id.into(), sha256.into()));
        if self.acked.len() > ACK_HISTORY {
            self.acked.pop_front();
        }
        Ok(())
    }
}
