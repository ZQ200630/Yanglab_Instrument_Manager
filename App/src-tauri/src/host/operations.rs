//! Durable admission, never an instruction to replay after a restart.
use super::{
    contracts::{valid_id, HostError, MAX_SEQUENCE},
    leases::{DomainRef, Session},
    registry::{new_id, write_atomic},
    verification::canonical_digest,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteParams {
    pub domain: DomainRef,
    pub lease_token: String,
    pub control_epoch: u64,
    pub config_rev: u64,
    pub context: Value,
    pub method: String,
    pub params: Value,
    pub sequence: u64,
    pub confirmation: Option<String>,
}
impl ExecuteParams {
    pub fn validate(&self) -> Result<(), HostError> {
        self.domain.validate()?;
        if !valid_id(&self.lease_token)
            || self.control_epoch > MAX_SEQUENCE
            || !(1..=MAX_SEQUENCE).contains(&self.config_rev)
            || !(1..=MAX_SEQUENCE).contains(&self.sequence)
            || !self.params.is_object()
            || !matches!(
                self.method.as_str(),
                "connect" | "resume" | "action" | "probe"
            )
            || !crate::reply_broker::valid_context_v3(&self.context)
            || self.context["domain"] != serde_json::to_value(&self.domain).unwrap()
        {
            return Err(HostError::new(
                "OperationInvalid",
                "Invalid typed operation binding",
            ));
        }
        Ok(())
    }
    fn digest(&self) -> Result<String, HostError> {
        let mut value = serde_json::to_value(self).unwrap();
        value.as_object_mut().unwrap().remove("confirmation");
        canonical_digest(&value)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Confirmation {
    pub token: String,
    pub digest: String,
    pub expires_in_ms: u64,
}
#[derive(Clone)]
struct BoundConfirmation {
    public: Confirmation,
    session: String,
    issued: Instant,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRecord {
    pub operation_id: String,
    pub worker_id: String,
    pub session_id: String,
    pub request_id: String,
    pub domain: DomainRef,
    pub digest: String,
    pub sequence: u64,
    pub status: String,
    pub phase: String,
    pub result: Value,
    pub terminal_at: Option<u64>,
    #[serde(default)]
    pub command: Value,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    records: Vec<OperationRecord>,
    watermarks: BTreeMap<String, u64>,
}
#[derive(Clone)]
pub struct OperationBook {
    path: PathBuf,
    boot: String,
    journal: Journal,
    confirmations: BTreeMap<String, BoundConfirmation>,
}
fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
impl OperationBook {
    pub fn open(path: &Path, boot: String) -> Result<Self, HostError> {
        if !valid_id(&boot) {
            return Err(HostError::new("OperationInvalid", "Invalid boot identity"));
        }
        let mut journal = if path.exists() {
            if std::fs::metadata(path).map_err(storage)?.len() > 8 * 1024 * 1024 {
                return Err(HostError::new(
                    "OperationStorage",
                    "Journal exceeds bounded size",
                ));
            }
            serde_json::from_slice::<Journal>(&std::fs::read(path).map_err(storage)?)
                .map_err(storage)?
        } else {
            Journal {
                version: 1,
                records: Vec::new(),
                watermarks: BTreeMap::new(),
            }
        };
        if journal.version != 1 || journal.records.len() > 4321 || journal.watermarks.len() > 4096 {
            return Err(HostError::new(
                "OperationStorage",
                "Unsupported or oversized journal",
            ));
        }
        for op in &mut journal.records {
            if op.terminal_at.is_none() {
                op.status = "Outcome Unknown".into();
                op.phase = "timed_out_unknown".into();
            }
        }
        let book = Self {
            path: path.into(),
            boot,
            journal,
            confirmations: BTreeMap::new(),
        };
        book.persist(&book.journal)?;
        Ok(book)
    }
    fn persist(&self, next: &Journal) -> Result<(), HostError> {
        let bytes = serde_json::to_vec(next).map_err(storage)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(HostError::new("OperationStorage", "Journal exceeds 8 MiB"));
        }
        write_atomic(&self.path, &bytes).map_err(storage)
    }
    pub fn prepare(
        &mut self,
        s: &Session,
        intent: ExecuteParams,
        now: Instant,
    ) -> Result<Confirmation, HostError> {
        intent.validate()?;
        if s.boot_id() != self.boot {
            return Err(HostError::new("ControlDenied", "Boot identity changed"));
        }
        self.confirmations.retain(|_, proof| {
            now.checked_duration_since(proof.issued)
                .is_some_and(|age| age < Duration::from_secs(60))
        });
        if self.confirmations.len() >= 256 {
            return Err(HostError::new(
                "ConfirmationCapacity",
                "Too many outstanding confirmations",
            ));
        }
        let proof = Confirmation {
            token: new_id()?,
            digest: intent.digest()?,
            expires_in_ms: 60000,
        };
        self.confirmations.insert(
            proof.token.clone(),
            BoundConfirmation {
                public: proof.clone(),
                session: s.id().into(),
                issued: now,
            },
        );
        Ok(proof)
    }
    pub fn lookup(
        &self,
        s: &Session,
        id: &str,
        intent: &ExecuteParams,
    ) -> Result<Option<OperationRecord>, HostError> {
        if let Some(old) = self
            .journal
            .records
            .iter()
            .find(|old| old.session_id == s.id() && old.request_id == id)
        {
            if old.digest != intent.digest()? {
                return Err(HostError::new(
                    "RequestConflict",
                    "Request ID was used with different parameters",
                ));
            }
            return Ok(Some(old.clone()));
        }
        if self
            .journal
            .watermarks
            .get(s.id())
            .is_some_and(|high| intent.sequence <= *high)
        {
            return Err(HostError::new(
                "RecordExpired",
                "Sequence was already accepted; do not replay",
            ));
        }
        Ok(None)
    }
    pub fn admit(
        &mut self,
        s: &Session,
        id: &str,
        intent: ExecuteParams,
        now: Instant,
    ) -> Result<OperationRecord, HostError> {
        intent.validate()?;
        if s.boot_id() != self.boot {
            return Err(HostError::new("ControlDenied", "Boot identity changed"));
        }
        if id.is_empty() || id.len() > 64 || id.chars().any(char::is_control) {
            return Err(HostError::new("OperationInvalid", "Invalid request ID"));
        }
        if let Some(old) = self.lookup(s, id, &intent)? {
            return Ok(old);
        }
        let token = intent.confirmation.as_ref().ok_or_else(|| {
            HostError::new(
                "ConfirmationRequired",
                "Prepare and explicitly confirm this operation",
            )
        })?;
        let digest = intent.digest()?;
        let proof = self
            .confirmations
            .get(token)
            .filter(|proof| {
                proof.session == s.id()
                    && proof.public.digest == digest
                    && now
                        .checked_duration_since(proof.issued)
                        .is_some_and(|age| age < Duration::from_secs(60))
            })
            .ok_or_else(|| {
                HostError::new(
                    "ConfirmationRequired",
                    "Confirmation expired or target changed",
                )
            })?;
        let _ = proof;
        if self.pending_count() >= 225 {
            return Err(HostError::new(
                "OperationCapacity",
                "Unresolved operations retain capacity",
            ));
        }
        let mut next = self.journal.clone();
        if !next.watermarks.contains_key(s.id()) && next.watermarks.len() >= 4096 {
            return Err(HostError::new(
                "OperationCapacity",
                "Session watermark capacity reached",
            ));
        }
        let now_wall = stamp();
        next.records.retain(|op| {
            op.terminal_at
                .map_or(true, |when| now_wall.saturating_sub(when) < 86400)
        });
        while next
            .records
            .iter()
            .filter(|op| op.terminal_at.is_some())
            .count()
            >= 4096
        {
            let oldest = next
                .records
                .iter()
                .enumerate()
                .filter_map(|(i, op)| op.terminal_at.map(|t| (i, t)))
                .min_by_key(|(_, t)| *t)
                .unwrap()
                .0;
            next.records.remove(oldest);
        }
        let operation_id = new_id()?;
        let op = OperationRecord {
            command: json!({"method":intent.method,"params":intent.params}),
            worker_id: canonical_digest(&json!({"boot":self.boot,"operation":operation_id}))?,
            operation_id,
            session_id: s.id().into(),
            request_id: id.into(),
            domain: intent.domain,
            digest,
            sequence: intent.sequence,
            status: "Accepted".into(),
            phase: "accepted".into(),
            result: Value::Null,
            terminal_at: None,
        };
        next.watermarks.insert(s.id().into(), intent.sequence);
        next.records.push(op.clone());
        self.persist(&next)?;
        self.journal = next;
        self.confirmations.remove(token);
        Ok(op)
    }
    pub fn finish(
        &mut self,
        id: &str,
        phase: &str,
        result: Value,
    ) -> Result<OperationRecord, HostError> {
        if !matches!(
            phase,
            "rejected_before_call"
                | "superseded_before_call"
                | "completed"
                | "failed_after_call_started"
                | "completed_readback_failed"
                | "timed_out_unknown"
        ) {
            return Err(HostError::new("OperationInvalid", "Invalid outcome phase"));
        }
        let mut next = self.journal.clone();
        let op = next
            .records
            .iter_mut()
            .find(|op| op.operation_id == id)
            .ok_or_else(|| HostError::new("OperationUnknown", "Operation not found"))?;
        if op.terminal_at.is_some() {
            return Ok(op.clone());
        }
        op.phase = phase.into();
        op.status = if phase == "timed_out_unknown" {
            "Outcome Unknown"
        } else {
            "Terminal"
        }
        .into();
        op.result = result;
        if phase != "timed_out_unknown" {
            op.terminal_at = Some(stamp())
        };
        let out = op.clone();
        if let Err(error) = self.persist(&next) {
            let retained = self
                .journal
                .records
                .iter_mut()
                .find(|op| op.operation_id == id)
                .unwrap();
            retained.status = "Outcome Unknown".into();
            retained.phase = "timed_out_unknown".into();
            retained.result = json!({"terminal_persistence_confirmed":false,"observed_reply":out.result,"audit_error":error});
            return Err(error);
        }
        self.journal = next;
        Ok(out)
    }
    pub fn find(&self, s: &Session, id: &str) -> Result<OperationRecord, HostError> {
        self.journal
            .records
            .iter()
            .find(|op| op.session_id == s.id() && (op.request_id == id || op.operation_id == id))
            .cloned()
            .ok_or_else(|| {
                HostError::new(
                    "OperationUnknown",
                    "No operation in this authenticated session",
                )
            })
    }
    pub fn record(&self, id: &str) -> Option<OperationRecord> {
        self.journal
            .records
            .iter()
            .find(|op| op.operation_id == id)
            .cloned()
    }
    pub fn pending_count(&self) -> usize {
        self.journal
            .records
            .iter()
            .filter(|op| op.terminal_at.is_none())
            .count()
    }
    pub fn safety_audit(&self, s: &Session, id: &str, result: Value) -> Result<(), HostError> {
        write_atomic(
            &self.path.with_file_name("safety-audit.json"),
            &serde_json::to_vec(&json!({"session":s.id(),"request":id,"result":result}))
                .map_err(storage)?,
        )
        .map_err(storage)
    }
}
fn storage(error: impl std::fmt::Display) -> HostError {
    HostError::new("OperationStorage", error.to_string())
}
type JournalJob = Box<dyn FnOnce(&mut OperationBook) -> Result<Value, HostError> + Send>;
enum Reply {
    Async(tokio::sync::oneshot::Sender<Result<Value, HostError>>),
    Sync(std::sync::mpsc::Sender<Result<Value, HostError>>),
}
pub(crate) struct OperationActor {
    sender: std::sync::mpsc::SyncSender<(JournalJob, Reply)>,
    cache: std::sync::Arc<std::sync::Mutex<OperationBook>>,
}
impl OperationActor {
    pub fn new(mut book: OperationBook) -> Result<Self, HostError> {
        let cache = std::sync::Arc::new(std::sync::Mutex::new(book.clone()));
        let published = cache.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel::<(JournalJob, Reply)>(256);
        std::thread::Builder::new()
            .name("host-operation-journal".into())
            .spawn(move || {
                while let Ok((job, reply)) = rx.recv() {
                    let result = job(&mut book);
                    let snapshot = book.clone();
                    *published.lock().unwrap() = snapshot;
                    match reply {
                        Reply::Async(tx) => {
                            let _ = tx.send(result);
                        }
                        Reply::Sync(tx) => {
                            let _ = tx.send(result);
                        }
                    }
                }
            })
            .map_err(storage)?;
        Ok(Self { sender: tx, cache })
    }
    pub fn read<T>(
        &self,
        f: impl FnOnce(&OperationBook) -> Result<T, HostError>,
    ) -> Result<T, HostError> {
        f(&self.cache.lock().unwrap())
    }
    pub async fn call<T: serde::de::DeserializeOwned>(
        &self,
        job: JournalJob,
    ) -> Result<T, HostError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender.try_send((job, Reply::Async(tx))).map_err(|_| {
            HostError::new(
                "OperationCapacity",
                "Journal admission queue unavailable or full",
            )
        })?;
        serde_json::from_value(rx.await.map_err(|_| {
            HostError::new(
                "OutcomeUnknown",
                "Journal reply lost; query original request ID",
            )
        })??)
        .map_err(storage)
    }
    pub fn call_sync<T: serde::de::DeserializeOwned>(
        &self,
        job: JournalJob,
    ) -> Result<T, HostError> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.sender.try_send((job, Reply::Sync(tx))).map_err(|_| {
            HostError::new("OperationCapacity", "Journal queue unavailable or full")
        })?;
        serde_json::from_value(rx.recv().map_err(storage)??).map_err(storage)
    }
}
pub(crate) struct SafetyAudit {
    sender:
        std::sync::mpsc::SyncSender<(Value, tokio::sync::oneshot::Sender<Result<(), HostError>>)>,
}
impl SafetyAudit {
    pub fn new(path: PathBuf) -> Result<Self, HostError> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<(
            Value,
            tokio::sync::oneshot::Sender<Result<(), HostError>>,
        )>(64);
        std::thread::Builder::new()
            .name("host-safety-audit".into())
            .spawn(move || {
                let mut attempts = std::collections::VecDeque::new();
                while let Ok((value, reply)) = rx.recv() {
                    attempts.push_back(value);
                    if attempts.len() > 256 {
                        attempts.pop_front();
                    }
                    let result = serde_json::to_vec(&attempts)
                        .map_err(storage)
                        .and_then(|bytes| write_atomic(&path, &bytes).map_err(storage));
                    let _ = reply.send(result);
                }
            })
            .map_err(storage)?;
        Ok(Self { sender: tx })
    }
    pub async fn record(&self, value: Value) -> Option<HostError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.sender.try_send((value, tx)).is_err() {
            return Some(HostError::new(
                "SafetyAuditCapacity",
                "Cleanup admitted; audit queue unavailable or full",
            ));
        }
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(result)) => result.err(),
            _ => Some(HostError::new(
                "SafetyAuditUnknown",
                "Cleanup admitted; durable audit not yet confirmed",
            )),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn failed_terminal_persistence_reports_unknown_and_keeps_liability() {
        let (mut book, dir, s) = fixture();
        let now = Instant::now();
        let mut args = intent();
        args.confirmation = Some(book.prepare(&s, args.clone(), now).unwrap().token);
        let op = book.admit(&s, "one", args, now).unwrap();
        std::fs::remove_file(dir.join("operations.json")).unwrap();
        std::fs::create_dir(dir.join("operations.json")).unwrap();
        assert!(book
            .finish(&op.operation_id, "completed", json!({"ok":true}))
            .is_err());
        assert_eq!(book.find(&s, "one").unwrap().status, "Outcome Unknown");
        assert_eq!(book.pending_count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    fn intent() -> ExecuteParams {
        ExecuteParams {
            domain: super::super::leases::DomainRef {
                kind: "device".into(),
                id: "1".repeat(32),
            },
            lease_token: "2".repeat(32),
            control_epoch: 1,
            config_rev: 1,
            context: json!({"session_id":"a".repeat(32),"domain":{"kind":"device","id":"1".repeat(32)},"connection_id":null,"epoch":0}),
            method: "connect".into(),
            params: json!({"acknowledge_lifecycle":true}),
            sequence: 1,
            confirmation: None,
        }
    }
    #[test]
    fn public_record_preserves_typed_result_command_without_control_secrets() {
        let (mut book, dir, s) = fixture();
        let now = Instant::now();
        let mut args = intent();
        args.method = "action".into();
        args.params = json!({"name":"acquire","args":{"trace":"A"}});
        args.confirmation = Some(book.prepare(&s, args.clone(), now).unwrap().token);
        let op = book.admit(&s, "shared", args, now).unwrap();
        let value = serde_json::to_value(op).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(
            value["command"],
            json!({"method":"action","params":{"name":"acquire","args":{"trace":"A"}}})
        );
        assert!(value["command"].get("lease_token").is_none());
    }
    fn fixture() -> (
        OperationBook,
        std::path::PathBuf,
        super::super::leases::Session,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "yang-operations-{}",
            super::super::registry::new_id().unwrap()
        ));
        std::fs::create_dir(&dir).unwrap();
        let session = super::super::leases::Session::local("b".repeat(32), "c".repeat(32)).unwrap();
        (
            OperationBook::open(&dir.join("operations.json"), "c".repeat(32)).unwrap(),
            dir,
            session,
        )
    }
    #[test]
    fn duplicate_does_not_consume_confirmation_twice() {
        let (mut book, dir, s) = fixture();
        let now = std::time::Instant::now();
        let mut args = intent();
        let proof = book.prepare(&s, args.clone(), now).unwrap();
        args.confirmation = Some(proof.token);
        let first = book.admit(&s, "same", args.clone(), now).unwrap();
        assert_eq!(
            first.operation_id,
            book.admit(&s, "same", args.clone(), now)
                .unwrap()
                .operation_id
        );
        args.params = json!({"acknowledge_lifecycle":false});
        assert_eq!(
            book.admit(&s, "same", args, now).unwrap_err().code,
            "RequestConflict"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn disk_full_rejects_normal_but_not_safe_stop() {
        let (mut book, dir, s) = fixture();
        let now = std::time::Instant::now();
        let mut args = intent();
        args.confirmation = Some(book.prepare(&s, args.clone(), now).unwrap().token);
        // Replace the exact test-owned journal path with a directory to inject storage failure.
        std::fs::remove_file(dir.join("operations.json")).unwrap();
        std::fs::create_dir(dir.join("operations.json")).unwrap();
        std::fs::create_dir(dir.join("safety-audit.json")).unwrap();
        assert_eq!(
            book.admit(&s, "one", args, now).unwrap_err().code,
            "OperationStorage"
        );
        assert!(book
            .safety_audit(&s, "safe", json!({"domain":"test"}))
            .is_err());
        assert_eq!(book.pending_count(), 0); // safety runs outside this journal gate
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn restart_preserves_unknown_without_replay() {
        let (mut book, dir, s) = fixture();
        let now = std::time::Instant::now();
        let mut args = intent();
        args.confirmation = Some(book.prepare(&s, args.clone(), now).unwrap().token);
        let op = book.admit(&s, "lost", args, now).unwrap();
        drop(book);
        let next = OperationBook::open(&dir.join("operations.json"), "d".repeat(32)).unwrap();
        assert_eq!(
            next.record(&op.operation_id).unwrap().status,
            "Outcome Unknown"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn exact_expiry_and_cross_session_reject() {
        let (mut book, dir, s) = fixture();
        let now = std::time::Instant::now();
        let mut args = intent();
        args.confirmation = Some(book.prepare(&s, args.clone(), now).unwrap().token);
        let other = super::super::leases::Session::local("e".repeat(32), "c".repeat(32)).unwrap();
        assert_eq!(
            book.admit(&other, "cross-session", args.clone(), now)
                .unwrap_err()
                .code,
            "ConfirmationRequired"
        );
        let mut changed = args.clone();
        changed.domain.id = "f".repeat(32);
        changed.context["domain"]["id"] = json!("f".repeat(32));
        assert_eq!(
            book.admit(&s, "cross-domain", changed, now)
                .unwrap_err()
                .code,
            "ConfirmationRequired"
        );
        let mut changed = args.clone();
        changed.control_epoch += 1;
        assert_eq!(
            book.admit(&s, "cross-epoch", changed, now)
                .unwrap_err()
                .code,
            "ConfirmationRequired"
        );
        assert_eq!(
            book.admit(
                &s,
                "expired",
                args,
                now + std::time::Duration::from_secs(60)
            )
            .unwrap_err()
            .code,
            "ConfirmationRequired"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn terminal_eviction_keeps_sequence_watermark_and_unknown_capacity() {
        let (mut book, dir, s) = fixture();
        let now = Instant::now();
        let mut args = intent();
        args.confirmation = Some(book.prepare(&s, args.clone(), now).unwrap().token);
        let op = book.admit(&s, "first", args.clone(), now).unwrap();
        book.finish(&op.operation_id, "completed", json!({}))
            .unwrap();
        book.journal.records[0].terminal_at = Some(0);
        let mut next = args.clone();
        next.sequence = 2;
        next.confirmation = Some(book.prepare(&s, next.clone(), now).unwrap().token);
        book.admit(&s, "second", next, now).unwrap();
        assert_eq!(
            book.admit(&s, "first", args, now).unwrap_err().code,
            "RecordExpired"
        );
        assert_eq!(book.pending_count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn journal_actor_blocked_write_does_not_block_cached_queries() {
        let (book, dir, _) = fixture();
        let actor = OperationActor::new(book).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (tx, rx) = tokio::sync::oneshot::channel();
        actor
            .sender
            .try_send((
                Box::new(move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(json!({}))
                }),
                Reply::Async(tx),
            ))
            .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(actor.read(|book| Ok(book.pending_count())).unwrap(), 0);
        release_tx.send(()).unwrap();
        let _ = rx.blocking_recv();
        drop(actor);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
