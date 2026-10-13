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
const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
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
    pin: Option<File>,
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
    reserved: HashMap<String,u64>,
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
    /// Reserve exact requested raw bytes before instrument I/O. Unknown memory
    /// depths reserve the caller's whole admitted byte budget, never guessed points.
    pub fn reserve_read(&mut self,id:&str,bytes:u64)->Result<(),WorkerError>{
        if !valid_id(id)||bytes==0||bytes>yang_protocol::MAX_WAVEFORM_BYTES||self.reserved.contains_key(id)||self.entries.contains_key(id)||self.acked.iter().any(|(old,_)|old==id){return Err(error("Invalid capture byte reservation"));}
        let _pins=self.pins()?;let(groups,size)=self.budget()?;
        let pending=self.reserved.values().try_fold(0_u64,|sum,n|sum.checked_add(*n+MAX_METADATA as u64+1024)).ok_or_else(||error("Capture reservation overflow"))?;
        if groups.max(self.entries.len()).saturating_add(self.reserved.len())>=MAX_ENTRIES||size.checked_add(pending).and_then(|n|n.checked_add(bytes+MAX_METADATA as u64+1024)).is_none_or(|n|n>MAX_TOTAL_BYTES){return Err(WorkerError::new("CaptureCapacity","Requested bytes and pending captures exceed staging budget; no instrument read started"));}
        self.reserved.insert(id.into(),bytes);Ok(())
    }
    pub fn release_read(&mut self,id:&str){self.reserved.remove(id);}
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
            reserved: HashMap::new(),
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
            groups.insert(key.clone());
            let length=file.metadata().map_err(error)?.len();
            if !self.entries.contains_key(&key)&&!self.reserved.contains_key(&key)&&length>MAX_BYTES{return Err(error("Unowned orphan exceeds legacy staging budget"));}
            size = size
                .checked_add(length)
                .ok_or_else(|| error("Staging size overflow"))?;
            if groups.len() > MAX_ENTRIES || size > MAX_TOTAL_BYTES {
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
                pin: None,
            },
        );
        self.partial.retain(|(key, _), _| key != &id);
        self.signatures.remove(&id);
        self.reserved.remove(&id);
        Ok(descriptor)
    }
    pub fn stage_waveform_owned(&mut self,id:&str,frame:&yang_data::WaveformFrame,source:&serde_json::Value)->Result<CaptureDescriptor,WorkerError>{
        if !valid_id(id)||!valid_id(&frame.id)||self.acked.iter().any(|(old,_)|old==id)||self.entries.contains_key(id)||frame.channels.is_empty()||frame.channels.len()>4||frame.nominal_offset_s.is_some_and(|n|!n.is_finite()||n<0.){return Err(error("Invalid/duplicate waveform staging identity or frame"));}
        let mut names=BTreeSet::new();let mut channels=vec![];let mut bytes=0_u64;let mut points=0_u64;
        for channel in &frame.channels {
            if !names.insert(&channel.channel){return Err(error("Duplicate waveform channel"));}
            channel.validate().map_err(error)?;
            let length=channel.samples.byte_len() as u64;
            let dtype=match channel.samples{yang_data::Samples::U8(_)=>"u8",yang_data::Samples::I8(_)=>"i8",yang_data::Samples::U16(_)=>"u16_le",yang_data::Samples::I16(_)=>"i16_le",yang_data::Samples::F64(_)=>"f64_le"};
            channels.push(serde_json::json!({"channel":channel.channel,"dtype":dtype,"point_count":channel.samples.len(),"byte_offset":bytes,"byte_count":length,"calibration":channel.calibration,"metadata":channel.metadata}));
            bytes=bytes.checked_add(length).ok_or_else(||error("Waveform byte overflow"))?;points=points.checked_add(channel.samples.len() as u64).ok_or_else(||error("Waveform point overflow"))?;
        }
        if bytes>yang_protocol::MAX_WAVEFORM_BYTES||self.reserved.get(id).is_none_or(|n|bytes>*n){return Err(error("Waveform bytes exceed pre-I/O reservation"));}
        let metadata=serde_json::json!({"schema":1,"frame":{"id":frame.id,"captured_at_unix_ms":frame.captured_at_unix_ms,"nominal_offset_s":frame.nominal_offset_s,"channels":channels},"source":source});
        if serde_json::to_vec(&metadata).map_err(error)?.len()>MAX_METADATA{return Err(error("Waveform metadata exceeds bounded descriptor"));}
        let _pins=self.pins()?;
        let (groups,physical)=self.budget()?;
        let mut own_partial=0_u64;
        for suffix in ["bin","json"] {if let Some(expected)=self.partial.get(&(id.to_owned(),suffix.into())) {let file=files::open(&self.root.join(format!("{id}.{suffix}")),false,false,false)?;if files::identity(&file,false)?!=*expected{return Err(error("Partial capture identity changed"));}own_partial=own_partial.checked_add(file.metadata().map_err(error)?.len()).ok_or_else(||error("Partial size overflow"))?;}}
        let other_reserved=self.reserved.iter().filter(|(key,_)|key.as_str()!=id).try_fold(0_u64,|sum,(_,n)|sum.checked_add(*n+MAX_METADATA as u64+1024)).ok_or_else(||error("Reservation overflow"))?;
        if groups.saturating_add(usize::from(own_partial==0))>MAX_ENTRIES||physical.checked_sub(own_partial).and_then(|n|n.checked_add(other_reserved)).and_then(|n|n.checked_add(bytes+MAX_METADATA as u64+1024)).is_none_or(|n|n>MAX_TOTAL_BYTES){return Err(error("Staging capacity changed after read; original waveform retained for storage-only retry"));}
        let mut digest=ring::digest::Context::new(&ring::digest::SHA256);
        for channel in &frame.channels{sample_chunks(&channel.samples,|chunk|{digest.update(chunk);Ok(())})?;}
        let hash=digest.finish().as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
        let descriptor=CaptureDescriptor{schema:1,kind:"waveform".into(),capture_id:id.into(),point_count:points,byte_count:bytes,sha256:hash,metadata};
        let encoded=serde_json::to_vec(&descriptor).map_err(error)?;let signature=sha256(&encoded);
        if self.signatures.get(id).is_some_and(|old|old!=&signature){return Err(error("Storage retry cannot replace original waveform"));}
        self.signatures.insert(id.into(),signature);
        let key=(id.to_owned(),"bin".to_owned());let path=self.root.join(format!("{id}.bin"));
        let mut file=match self.partial.get(&key){Some(expected)=>files::repair(&path,*expected)?,None=>files::open(&path,false,true,false)?};
        let native=files::identity(&file,false)?;self.partial.insert(key,native);
        use std::io::{Seek,SeekFrom,Read};
        file.seek(SeekFrom::Start(0)).map_err(error)?;file.set_len(0).map_err(error)?;
        for channel in &frame.channels{sample_chunks(&channel.samples,|chunk|file.write_all(chunk).map_err(error))?;}
        file.flush().map_err(error)?;self.durability.sync(&file).map_err(error)?;drop(file);
        let manifest=self.write_new(id,"json",&encoded)?;
        // Hash once while obtaining an immutable no-write/delete handle. Every
        // later chunk reads just its range, not a full-array clone or full rehash.
        let mut pin=files::open(&path,false,false,false)?;
        if files::identity(&pin,false)?!=native||pin.metadata().map_err(error)?.len()!=bytes{return Err(error("Waveform changed before pin"));}
        let mut verified=ring::digest::Context::new(&ring::digest::SHA256);let mut buffer=[0;16384];
        loop{let n=pin.read(&mut buffer).map_err(error)?;if n==0{break;}verified.update(&buffer[..n]);}
        let verified=verified.finish().as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
        if verified!=descriptor.sha256{return Err(error("Waveform changed before immutable pin"));}
        self.entries.insert(id.into(),Entry{descriptor:descriptor.clone(),native,manifest,pin:Some(pin)});
        self.partial.retain(|(key,_),_|key!=id);self.signatures.remove(id);self.reserved.remove(id);Ok(descriptor)
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
        if let Some(pin)=&entry.pin{return files::read_range(pin,entry.native,entry.descriptor.byte_count,offset,length);}
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
            .get_mut(id)
            .ok_or_else(|| error("Unknown capture ACK"))?;
        if entry.descriptor.sha256 != sha256 {
            return Err(error("ACK hash mismatch"));
        }
        let native=entry.native;let manifest=entry.manifest;drop(entry.pin.take());
        let _pins = self.pins()?;
        files::delete(&self.root.join(format!("{id}.bin")), native)?;
        files::delete(&self.root.join(format!("{id}.json")), manifest)?;
        self.entries.remove(id);
        self.acked.push_back((id.into(), sha256.into()));
        if self.acked.len() > ACK_HISTORY {
            self.acked.pop_front();
        }
        Ok(())
    }
}

fn sample_chunks(samples:&yang_data::Samples,mut visit:impl FnMut(&[u8])->Result<(),WorkerError>)->Result<(),WorkerError>{
    use yang_data::Samples;
    let mut buffer=Vec::with_capacity(16384);
    macro_rules! encode {($values:expr,$width:expr)=>{for chunk in $values.chunks(16384/$width){buffer.clear();for value in chunk{buffer.extend_from_slice(&value.to_le_bytes());}visit(&buffer)?;}}}
    match samples{
        Samples::U8(v)=>{for chunk in v.chunks(16384){visit(chunk)?;}},
        Samples::I8(v)=>{for chunk in v.chunks(16384){buffer.clear();buffer.extend(chunk.iter().map(|n|*n as u8));visit(&buffer)?;}},
        Samples::U16(v)=>encode!(v,2),Samples::I16(v)=>encode!(v,2),Samples::F64(v)=>encode!(v,8)
    }Ok(())
}
