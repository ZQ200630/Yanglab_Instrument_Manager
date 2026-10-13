//! Session-owned RAM previews. Raw samples are moved once and shared immutably.
use super::*;
use yang_data::{ChannelMetadata, FrameMetadata, RecordMetadata, WaveformRecord};

const MAX_BYTES: u64 = 536_870_912;
const MAX_PREVIEWS: usize = 32;
struct Entry {
    owner: String,
    bytes: u64,
    record: Arc<WaveformRecord>,
    saving: AtomicBool,
    saved: AtomicBool,
    retired: AtomicBool,
}
#[derive(Default)]
struct Book {
    entries: BTreeMap<String, Arc<Entry>>,
    reservations: BTreeMap<String, (String, u64, bool)>,
}
#[derive(Clone, Default)]
pub(crate) struct Previews(Arc<Mutex<Book>>);
pub(crate) struct Reservation {
    store: Previews,
    id: String,
    bytes: u64,
}
fn raw_bytes(record: &WaveformRecord) -> Result<u64, HostError> {
    record.frames.iter().flat_map(|f| &f.channels).try_fold(0u64, |sum, c| {
        sum.checked_add(c.samples.byte_len() as u64 + 256)
            .ok_or_else(|| error("Preview byte accounting overflow"))
    })
}
impl Reservation {
    pub(crate) fn validate(&self, record: &WaveformRecord) -> Result<(), HostError> {
        if raw_bytes(record)? > self.bytes {
            return Err(HostError::new("PreviewBudget", "Capture exceeds reserved preview budget"));
        }
        if self.store.0.lock().unwrap().reservations.get(&self.id).is_none_or(|(_, _, retired)| *retired) {
            return Err(HostError::new("ControlDenied", "Preview owner departed"));
        }
        Ok(())
    }
    pub(crate) fn publish(self, record: WaveformRecord) -> Result<String, HostError> {
        self.validate(&record)?;
        record.validate().map_err(error)?;
        let bytes = raw_bytes(&record)?;
        let id = record.id.clone();
        let mut book = self.store.0.lock().unwrap();
        if book.entries.contains_key(&id) {
            return Err(error("Preview identity already exists"));
        }
        let (owner, _, retired) = book.reservations.remove(&self.id)
            .ok_or_else(|| HostError::new("ControlDenied", "Preview owner departed"))?;
        if retired { return Err(HostError::new("ControlDenied", "Preview owner departed")); }
        book.entries.insert(id.clone(), Arc::new(Entry { owner, bytes,
            record: Arc::new(record), saving: AtomicBool::new(false), saved: AtomicBool::new(false), retired: AtomicBool::new(false) }));
        Ok(id)
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.store.0.lock().unwrap().reservations.remove(&self.id);
    }
}
impl Previews {
    pub(crate) fn reserve(&self, owner: &str, bytes: u64) -> Result<Reservation, HostError> {
        self.reap();
        let mut b = self.0.lock().unwrap();
        let used = b.entries.values().map(|e| e.bytes).chain(b.reservations.values().map(|(_, n, _)| *n)).sum::<u64>();
        if bytes == 0 || bytes > MAX_BYTES || used.checked_add(bytes).is_none_or(|n| n > MAX_BYTES)
            || b.entries.len() + b.reservations.len() >= MAX_PREVIEWS {
            return Err(HostError::new("PreviewBudget", "Preview memory is full; discard an existing preview before acquiring"));
        }
        let id = yang_data::new_id().map_err(error)?;
        b.reservations.insert(id.clone(), (owner.into(), bytes, false));
        Ok(Reservation { store: self.clone(), id, bytes })
    }
    fn entry(&self, owner: &str, id: &str) -> Result<Option<Arc<Entry>>, HostError> {
        let b = self.0.lock().unwrap();
        match b.entries.get(id) {
            Some(e) if e.retired.load(Ordering::Acquire) && !e.saved.load(Ordering::Acquire) => Err(HostError::new("PreviewOwner", "Preview owner departed")),
            Some(e) if e.retired.load(Ordering::Acquire) => Ok(None),
            Some(e) if e.owner != owner && e.saved.load(Ordering::Acquire) => Ok(None),
            Some(e) if e.owner != owner => Err(HostError::new("PreviewOwner", "Preview absent for this session")),
            entry => Ok(entry.cloned()),
        }
    }
    pub(super) fn record(&self, owner: &str, id: &str) -> Result<Option<Arc<WaveformRecord>>, HostError> {
        Ok(self.entry(owner, id)?.map(|e| e.record.clone()))
    }
    pub(super) fn saved(&self, owner: &str, id: &str) -> Result<Option<bool>, HostError> {
        Ok(self.entry(owner, id)?.map(|e| e.saved.load(Ordering::Acquire)))
    }
    pub(super) fn contains(&self, owner: &str, id: &str) -> Result<bool, HostError> {
        Ok(self.entry(owner, id)?.is_some())
    }
    pub(super) fn save(&self, store: &DataStore, owner: &str, id: &str, name: &str, comment: &str) -> Result<Value, HostError> {
        if name.trim().is_empty() || name.len() > 1024 || comment.len() > 65536 {
            return Err(error("Record name/comment exceeds limits or name is blank"));
        }
        let entry = {
            let b = self.0.lock().unwrap();
            let entry = b.entries.get(id).filter(|e| e.owner == owner && !e.retired.load(Ordering::Acquire)).cloned()
                .ok_or_else(|| error("Preview absent for this session"))?;
            if entry.saving.swap(true, Ordering::AcqRel) {
                return Err(HostError::new("Busy", "Preview save is already pending"));
            }
            entry
        };
        struct Saving<'a>(&'a AtomicBool);
        impl Drop for Saving<'_> { fn drop(&mut self) { self.0.store(false, Ordering::Release); } }
        let _saving = Saving(&entry.saving);
        // No preview book lock is held during validation, hashing or disk I/O.
        if entry.saved.load(Ordering::Acquire) {
            store.rename(id, name, comment).map_err(error)?;
        } else {
            store.save_with_labels(&entry.record, name, comment).map_err(error)?;
            entry.saved.store(true, Ordering::Release);
        }
        Ok(json!({"record_id":id,"saved":true}))
    }
    pub(super) fn discard(&self, owner: &str, id: &str) -> Result<Value, HostError> {
        let mut b = self.0.lock().unwrap();
        let entry = b.entries.get(id).filter(|e| e.owner == owner)
            .ok_or_else(|| error("Preview absent for this session"))?;
        if entry.saving.load(Ordering::Acquire) || Arc::strong_count(&entry.record) > 1 || Arc::strong_count(entry) > 1 {
            return Err(HostError::new("Busy", "Preview is being saved, displayed or exported"));
        }
        let removed = b.entries.remove(id);
        drop(b);
        drop(removed);
        Ok(json!({"record_id":id,"discarded":true}))
    }
    pub(super) fn cancel_owner(&self, owner: &str) {
        let mut b = self.0.lock().unwrap();
        for (o, _, retired) in b.reservations.values_mut() {
            if o == owner { *retired = true; }
        }
        for entry in b.entries.values().filter(|e| e.owner == owner) {
            entry.retired.store(true, Ordering::Release);
        }
        drop(b);
        self.reap();
    }
    fn reap(&self) {
        let mut b = self.0.lock().unwrap();
        let ids = b.entries.iter().filter(|(_, e)| e.retired.load(Ordering::Acquire)
            && !e.saving.load(Ordering::Acquire) && Arc::strong_count(&e.record) == 1 && Arc::strong_count(e) == 1)
            .map(|(id, _)| id.clone()).collect::<Vec<_>>();
        let removed = ids.into_iter().filter_map(|id| b.entries.remove(&id)).collect::<Vec<_>>();
        drop(b);
        drop(removed);
    }
}
pub(super) fn metadata(record: &WaveformRecord) -> RecordMetadata {
    RecordMetadata { schema: record.schema, id: record.id.clone(), name: record.name.clone(), comment: record.comment.clone(),
        created_at_unix_ms: record.created_at_unix_ms, source: record.source.clone(), analyses: record.analyses.clone(),
        frames: record.frames.iter().map(|f| FrameMetadata { id: f.id.clone(), captured_at_unix_ms: f.captured_at_unix_ms,
            nominal_offset_s: f.nominal_offset_s, channels: f.channels.iter().map(|c| ChannelMetadata {
                channel: c.channel.clone(), sample_count: c.samples.len(), calibration: c.calibration.clone(), metadata: c.metadata.clone(),
                dtype: match c.samples { yang_data::Samples::U8(_) => "|u1", yang_data::Samples::I8(_) => "|i1", yang_data::Samples::U16(_) => "<u2", yang_data::Samples::I16(_) => "<i2", yang_data::Samples::F64(_) => "<f8" }.into()
            }).collect() }).collect() }
}
