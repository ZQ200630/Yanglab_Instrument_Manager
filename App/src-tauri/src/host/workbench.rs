//! Host-owned offline waveform workbench. No instrument transport access.
use super::{contracts::HostError, registry::write_atomic};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
};
use yang_data::{AnalysisRequest, DataStore, SampleRange};
pub(super) mod previews;

fn error(e: impl std::fmt::Display) -> HostError {
    HostError::new("Workbench", e.to_string())
}
fn data_error(e: yang_data::DataError) -> HostError {
    HostError::new(
        if e == yang_data::DataError::Cancelled {
            "Cancelled"
        } else {
            "Workbench"
        },
        e.to_string(),
    )
}
/// Bounded named channels; remote sharing uses a separate explicit method allowlist.
pub fn method_channel(method: &str) -> Option<&'static str> {
    match method {
        "analysis_status" | "experiment_status" | "experiment_templates" => Some("status"),
        "job_cancel" | "experiment_stop" | "data_import_abort" | "data_export_release" | "preview_discard" => {
            Some("safety")
        }
        "data_list" | "data_get" | "data_viewport" | "analysis_result" | "data_import_begin"
        | "data_import_chunk" | "data_import_commit" | "data_export" | "data_export_chunk" => {
            Some("results")
        }
        "data_update" | "data_remove" | "analysis_start" | "experiment_save"
        | "experiment_list" | "preview_save" => Some("background"),
        "experiment_start" | "capture_start" => Some("control"),
        _ => None,
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Job {
    id: String,
    owner: String,
    kind: String,
    status: String,
    stage: String,
    completed_units: u64,
    total_units: u64,
    started_at_unix_ms: u64,
    finished_at_unix_ms: Option<u64>,
    result: Value,
    error: Option<HostError>,
}
const MAX_JOB_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DEFINITION_BYTES: u64 = 2 * 1024 * 1024;

fn parse_saved_jobs(bytes: &[u8]) -> Result<Vec<Job>, HostError> {
    if bytes.len() as u64 > MAX_JOB_JOURNAL_BYTES {
        return Err(error("Job journal oversized"));
    }
    let saved: Vec<Job> = serde_json::from_value(
        crate::runtime::strict_json(bytes).map_err(error)?).map_err(error)?;
    if saved.len() > 128 {
        return Err(error("Too many saved jobs"));
    }
    if saved.iter().any(|job| !yang_data::valid_id(&job.id)) {
        return Err(error("Invalid saved job identity"));
    }
    Ok(saved)
}

fn parse_saved_definitions(bytes: &[u8]) -> Result<Vec<yang_experiments::ExperimentDefinition>, HostError> {
    if bytes.len() as u64 > MAX_DEFINITION_BYTES {
        return Err(error("Experiment definitions oversized"));
    }
    let saved: Vec<yang_experiments::ExperimentDefinition> = serde_json::from_value(
        crate::runtime::strict_json(bytes).map_err(error)?).map_err(error)?;
    if saved.len() > 128 {
        return Err(error("Experiment limit reached"));
    }
    for definition in &saved {
        yang_experiments::validate_definition(definition).map_err(error)?;
    }
    Ok(saved)
}

// Pure preflight shared with legacy-location adoption. Never open a Workbench
// just to validate: opening may clean staging and rewrite interrupted jobs.
pub(super) fn validate_saved_journal(name: &str, bytes: &[u8]) -> Result<(), HostError> {
    match name {
        "jobs.json" => parse_saved_jobs(bytes).map(|_| ()),
        "experiments.json" => parse_saved_definitions(bytes).map(|_| ()),
        _ => Err(error("Unknown Workbench journal")),
    }
}
struct JobBook {
    path: PathBuf,
    jobs: BTreeMap<String, Job>,
    cancel: BTreeMap<String, Arc<AtomicBool>>,
}
impl JobBook {
    fn value(&self, id: &str, owner: &str) -> Result<Value, HostError> {
        let j = self
            .jobs
            .get(id)
            .filter(|j| j.owner == owner)
            .ok_or_else(|| error("Job not found for this session"))?;
        let mut v = serde_json::to_value(j).map_err(error)?;
        v.as_object_mut().unwrap().remove("owner");
        v["job_id"] = json!(id);
        v["elapsed_ms"] = json!(j
            .finished_at_unix_ms
            .unwrap_or_else(yang_data::unix_ms)
            .saturating_sub(j.started_at_unix_ms));
        Ok(v)
    }
}
// Serialize writers separately; never retain the status/cancel book lock during disk I/O.
fn persist_book(book: &Mutex<JobBook>, gate: &Mutex<()>) -> Result<(), HostError> {
    let _writer = gate.lock().unwrap_or_else(|e| e.into_inner());
    let (path, bytes) = {
        let b = book.lock().unwrap_or_else(|e| e.into_inner());
        (
            b.path.clone(),
            serde_json::to_vec(&b.jobs.values().filter(|j| j.kind != "capture").collect::<Vec<_>>()).map_err(error)?,
        )
    };
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(error("Job journal exceeds16MiB"));
    }
    write_atomic(&path, &bytes).map_err(error)
}
fn commit_finished_job(book: &Mutex<JobBook>, gate: &Mutex<()>, mut finished: Job) {
    if finished.kind == "capture" {
        let mut b = book.lock().unwrap_or_else(|e| e.into_inner());
        b.cancel.remove(&finished.id);
        b.jobs.insert(finished.id.clone(), finished);
        return;
    }
    // Keep the terminal candidate private until the storage attempt has released
    // every handle. Status readers and experiment drain remain nonblocking, but
    // must still see this job as active while its final journal write is pending.
    let _writer = gate.lock().unwrap_or_else(|e| e.into_inner());
    let persisted = (|| {
        let (path, bytes) = {
            let b = book.lock().unwrap_or_else(|e| e.into_inner());
            let jobs: Vec<_> = b
                .jobs
                .values()
                .filter(|job| job.kind != "capture")
                .map(|job| if job.id == finished.id { &finished } else { job })
                .collect();
            (b.path.clone(), serde_json::to_vec(&jobs).map_err(error)?)
        };
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(error("Job journal exceeds16MiB"));
        }
        write_atomic(&path, &bytes).map_err(error)
    })();
    if let Err(e) = persisted {
        finished.status = "uncertain".into();
        finished.stage = "journal_failed".into();
        finished.error = Some(e);
    }
    // Publish under the same writer gate so a concurrent admission/final writer
    // cannot take a stale running snapshot after the terminal disk commit.
    let mut b = book.lock().unwrap_or_else(|e| e.into_inner());
    b.cancel.remove(&finished.id);
    b.jobs.insert(finished.id.clone(), finished);
}
pub struct JobContext {
    id: String,
    cancel: Arc<AtomicBool>,
    book: Arc<Mutex<JobBook>>,
}
impl JobContext {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<(), HostError> {
        if self.cancelled() {
            Err(HostError::new(
                "Cancelled",
                "Cancelled at a confirmed work boundary",
            ))
        } else {
            Ok(())
        }
    }
    pub fn progress(&self, stage: &str, completed: u64, total: u64) {
        let mut b = self.book.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(j) = b.jobs.get_mut(&self.id) {
            j.stage = stage.chars().take(128).collect();
            j.completed_units = completed;
            j.total_units = total;
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }
}
type Work = Box<dyn FnOnce(JobContext) -> Result<Value, HostError> + Send + 'static>;
struct Task {
    context: JobContext,
    work: Work,
    journal_gate: Arc<Mutex<()>>,
}
struct Upload {
    owner: String,
    path: PathBuf,
    original_name: String,
    size: u64,
    received: u64,
}
struct Download {
    owner: String,
    job_id: String,
    path: PathBuf,
    size: u64,
    sha256: String,
}
pub struct Workbench {
    pub store: Arc<DataStore>,
    pub(super) previews: previews::Previews,
    root: PathBuf,
    book: Arc<Mutex<JobBook>>,
    journal_gate: Arc<Mutex<()>>,
    admission_gate: Mutex<()>,
    sender: mpsc::SyncSender<Task>,
    uploads: Mutex<BTreeMap<String, Upload>>,
    downloads: Arc<Mutex<BTreeMap<String, Download>>>,
    export_gate: Arc<Mutex<()>>,
    definitions_gate: Mutex<()>,
}
impl Workbench {
    pub fn open(root: &Path) -> Result<Arc<Self>, HostError> {
        std::fs::create_dir_all(root).map_err(error)?;
        let path = root.join("jobs.json");
        let mut jobs = BTreeMap::new();
        let saved = if path.exists() {
            parse_saved_jobs(&super::registry::read_config(&path, MAX_JOB_JOURNAL_BYTES).map_err(error)?)?
        } else { Vec::new() };
        let definitions = root.join("experiments.json");
        if definitions.exists() {
            parse_saved_definitions(&super::registry::read_config(&definitions, MAX_DEFINITION_BYTES).map_err(error)?)?;
        }
        // Reject both typed journals before any cleanup or durable rewrite.
        clean_stale_transfers(root);
            for mut j in saved {
                if matches!(j.status.as_str(), "queued" | "running") {
                    j.status = "uncertain".into();
                    j.stage = "interrupted_by_restart".into();
                    j.error=Some(HostError::new("OutcomeUnknown","Previous process stopped before the job settled; do not replay automatically"));
                    j.finished_at_unix_ms = Some(yang_data::unix_ms());
                }
                jobs.insert(j.id.clone(), j);
            }
        let book = Arc::new(Mutex::new(JobBook {
            path,
            jobs,
            cancel: BTreeMap::new(),
        }));
        let journal_gate = Arc::new(Mutex::new(()));
        persist_book(&book, &journal_gate)?;
        let (tx, rx) = mpsc::sync_channel::<Task>(16);
        let rx = Arc::new(Mutex::new(rx));
        for index in 0..2 {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("yang-workbench-{index}"))
                .spawn(move || loop {
                    let task = match rx.lock().unwrap().recv() {
                        Ok(task) => task,
                        Err(_) => break,
                    };
                    let id = task.context.id.clone();
                    let book = task.context.book.clone();
                    let journal_gate = task.journal_gate.clone();
                    {
                        let mut b = book.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(j) = b.jobs.get_mut(&id) {
                            j.status = "running".into();
                            j.stage = "starting".into();
                        }
                    }
                    let outcome = catch_unwind(AssertUnwindSafe(|| {
                        task.context.check()?;
                        (task.work)(task.context)
                    }));
                    let mut finished = {
                        let mut b = book.lock().unwrap_or_else(|e| e.into_inner());
                        let Some(j) = b.jobs.get_mut(&id) else {
                            continue;
                        };
                        j.stage = "saving_result".into();
                        j.clone()
                    };
                    match outcome {
                        Ok(Ok(value)) => {
                            finished.status = match value["run"]["status"].as_str() {
                                Some("failed") => "failed",
                                Some("cancelled") => "cancelled",
                                Some("uncertain") => "uncertain",
                                _ => "completed",
                            }
                            .into();
                            finished.stage = finished.status.clone();
                            finished.result = value;
                        }
                        Ok(Err(e)) => {
                            finished.status = if e.code == "Cancelled" {
                                "cancelled"
                            } else if e.code == "OutcomeUnknown" {
                                "uncertain"
                            } else {
                                "failed"
                            }
                            .into();
                            finished.stage = finished.status.clone();
                            finished.error = Some(e);
                        }
                        Err(_) => {
                            finished.status = "uncertain".into();
                            finished.stage = "panic".into();
                            finished.error = Some(HostError::new(
                                "OutcomeUnknown",
                                "Job panicked; no automatic replay",
                            ));
                        }
                    }
                    finished.finished_at_unix_ms = Some(yang_data::unix_ms());
                    commit_finished_job(&book, &journal_gate, finished);
                })
                .map_err(error)?;
        }
        Ok(Arc::new(Self {
            store: Arc::new(DataStore::new(root.join("records")).map_err(error)?),
            previews: previews::Previews::default(),
            root: root.into(),
            book,
            journal_gate,
            admission_gate: Mutex::new(()),
            sender: tx,
            uploads: Mutex::new(BTreeMap::new()),
            downloads: Arc::new(Mutex::new(BTreeMap::new())),
            export_gate: Arc::new(Mutex::new(())),
            definitions_gate: Mutex::new(()),
        }))
    }
    pub fn submit(
        &self,
        owner: &str,
        kind: &str,
        work: impl FnOnce(JobContext) -> Result<Value, HostError> + Send + 'static,
    ) -> Result<Value, HostError> {
        let _admission = self
            .admission_gate
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let id = yang_data::new_id().map_err(error)?;
        let flag = Arc::new(AtomicBool::new(false));
        let mut b = self.book.lock().unwrap();
        if b.cancel.len() >= 18 {
            return Err(HostError::new("Busy", "Workbench queue is full"));
        }
        let evicted = if b.jobs.len() >= 128 {
            if let Some(old) = b
                .jobs
                .values()
                .filter(|j| j.finished_at_unix_ms.is_some())
                .min_by_key(|j| j.started_at_unix_ms)
                .map(|j| j.id.clone())
            {
                b.jobs.remove(&old)
            } else {
                return Err(error("Job history full"));
            }
        } else {
            None
        };
        b.jobs.insert(
            id.clone(),
            Job {
                id: id.clone(),
                owner: owner.into(),
                kind: kind.into(),
                status: "queued".into(),
                stage: "queued".into(),
                completed_units: 0,
                total_units: 0,
                started_at_unix_ms: yang_data::unix_ms(),
                finished_at_unix_ms: None,
                result: Value::Null,
                error: None,
            },
        );
        b.cancel.insert(id.clone(), flag.clone());
        drop(b);
        let task = Task {
            context: JobContext {
                id: id.clone(),
                cancel: flag,
                book: self.book.clone(),
            },
            work: Box::new(work),
            journal_gate: self.journal_gate.clone(),
        };
        let admitted = (if kind == "capture" { Ok(()) } else { persist_book(&self.book, &self.journal_gate) }).and_then(|_| {
            self.sender
                .try_send(task)
                .map_err(|e| HostError::new("Busy", e.to_string()))
        });
        if let Err(e) = admitted {
            let mut b = self.book.lock().unwrap();
            b.jobs.remove(&id);
            b.cancel.remove(&id);
            if let Some(old) = evicted {
                b.jobs.insert(old.id.clone(), old);
            }
            drop(b);
            let _ = persist_book(&self.book, &self.journal_gate);
            return Err(e);
        }
        self.status(&id, owner)
    }
    pub fn status(&self, id: &str, owner: &str) -> Result<Value, HostError> {
        self.book.lock().unwrap().value(id, owner)
    }
    pub fn public_status(&self, id: &str) -> Result<Value, HostError> {
        let b = self.book.lock().unwrap();
        let owner = b
            .jobs
            .get(id)
            .ok_or_else(|| error("Job absent"))?
            .owner
            .clone();
        b.value(id, &owner)
    }
    pub fn visible_status(&self, id: &str, owner: &str) -> Result<Value, HostError> {
        let b = self.book.lock().unwrap();
        let job = b.jobs.get(id).ok_or_else(|| error("Job absent"))?;
        let visible_owner = if matches!(job.kind.as_str(), "capture" | "preview_export") { owner } else { &job.owner };
        b.value(id, visible_owner)
    }
    pub fn cancel(&self, id: &str, owner: &str) -> Result<Value, HostError> {
        let b = self.book.lock().unwrap();
        b.value(id, owner)?;
        if let Some(c) = b.cancel.get(id) {
            c.store(true, Ordering::Release);
        }
        let mut v = b.value(id, owner)?;
        v["cancel_requested"] = json!(b.cancel.contains_key(id));
        Ok(v)
    }
    pub fn cancel_owner(&self, owner: &str) {
        self.previews.cancel_owner(owner);
        let b = self.book.lock().unwrap();
        for (id, flag) in &b.cancel {
            if b.jobs[id].owner == owner {
                flag.store(true, Ordering::Release);
            }
        }
        drop(b);
        let ids: Vec<_> = self
            .uploads
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, u)| u.owner == owner)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let _ = self.import_abort(owner, &id);
        }
        let jobs: std::collections::BTreeSet<_> = self
            .downloads
            .lock()
            .unwrap()
            .values()
            .filter(|d| d.owner == owner)
            .map(|d| d.job_id.clone())
            .collect();
        for id in jobs {
            let _ = self.export_release(owner, &id);
        }
    }
    pub fn cancel_all(&self) {
        for flag in self.book.lock().unwrap().cancel.values() {
            flag.store(true, Ordering::Release);
        }
    }
    pub(crate) fn cancel_internal(&self, id: &str) {
        if let Some(flag) = self.book.lock().unwrap().cancel.get(id) {
            flag.store(true, Ordering::Release);
        }
    }
    pub fn active_experiments(&self, owner: Option<&str>) -> bool {
        self.book.lock().unwrap().jobs.values().any(|j| {
            matches!(j.kind.as_str(), "experiment" | "capture")
                && matches!(j.status.as_str(), "queued" | "running")
                && owner.is_none_or(|o| j.owner == o)
        })
    }
    pub fn list(&self) -> Result<Value, HostError> {
        Ok(json!({"records":self.store.list().map_err(error)?}))
    }
    pub fn metadata(&self, id: &str, offset: usize, limit: usize) -> Result<Value, HostError> {
        let r = self.store.metadata(id).map_err(error)?;
        self.metadata_value(r, offset, limit, true)
    }
    pub fn record_metadata(&self, owner: &str, id: &str) -> Result<yang_data::RecordMetadata, HostError> {
        match self.previews.saved(owner, id)? {
            Some(false) => Ok(previews::metadata(self.previews.record(owner, id)?.ok_or_else(|| error("Preview absent"))?.as_ref())),
            _ => self.store.metadata(id).map_err(error),
        }
    }
    pub fn owner_metadata(&self, owner: &str, id: &str, offset: usize, limit: usize) -> Result<Value, HostError> {
        let saved = self.previews.saved(owner, id)?.unwrap_or(true);
        let preview = self.previews.contains(owner, id)?;
        let mut result = self.metadata_value(self.record_metadata(owner, id)?, offset, limit, saved)?;
        result["preview"] = json!(preview);
        Ok(result)
    }
    fn metadata_value(&self, r: yang_data::RecordMetadata, offset: usize, limit: usize, saved: bool) -> Result<Value, HostError> {
        let frames:Vec<_>=r.frames.iter().skip(offset).take(limit.min(256)).map(|f|json!({"id":f.id,"captured_at_unix_ms":f.captured_at_unix_ms,"nominal_offset_s":f.nominal_offset_s,"channels":f.channels.iter().map(|c|json!({"channel":c.channel,"points":c.sample_count,"dtype":c.dtype,"calibration":c.calibration,"metadata":c.metadata})).collect::<Vec<_>>()})).collect();
        let analyses:Vec<_>=r.analyses.iter().rev().take(32).map(|a|json!({"id":a.id,"schema":a.schema,"record_id":a.record_id,"frame_id":a.frame_id,"channel":a.channel,"method":a.method,"algorithm":a.algorithm,"roi":a.roi,"parameters":a.parameters,"result":a.result,"created_at_unix_ms":a.created_at_unix_ms})).collect();
        Ok(
            json!({"saved":saved,"frame_count":r.frames.len(),"frame_offset":offset,"more":offset+frames.len()<r.frames.len(),"record":{"schema":r.schema,"id":r.id,"name":r.name,"comment":r.comment,"created_at_unix_ms":r.created_at_unix_ms,"source":r.source,"frames":frames,"frame_count":r.frames.len(),"frame_offset":offset,"truncated":offset+frames.len()<r.frames.len(),"analyses":analyses}}),
        )
    }
    pub fn viewport(&self, params: &Value) -> Result<Value, HostError> {
        self.owner_viewport("", params)
    }
    pub fn owner_viewport(&self, owner: &str, params: &Value) -> Result<Value, HostError> {
        let id = text(params, "record_id")?;
        let r = self.record_metadata(owner, id)?;
        let f = r
            .frames
            .iter()
            .find(|f| Some(f.id.as_str()) == params["frame_id"].as_str())
            .ok_or_else(|| error("Frame absent"))?;
        let c = f
            .channels
            .iter()
            .find(|c| Some(c.channel.as_str()) == params["channel"].as_str())
            .ok_or_else(|| error("Channel absent"))?;
        let roi = SampleRange {
            start: params["start"].as_u64().unwrap_or(0) as usize,
            end: params["end"]
                .as_u64()
                .map(|n| n as usize)
                .unwrap_or(c.sample_count),
        };
        let max_points = params["max_points"].as_u64().unwrap_or(2048) as usize;
        let points = if let Some(record) = self.previews.record(owner, id)? {
            let channel = record.frames.iter().find(|frame| frame.id == f.id)
                .and_then(|frame| frame.channels.iter().find(|channel| channel.channel == c.channel))
                .ok_or_else(|| error("Preview channel absent"))?;
            yang_data::reduce_viewport(channel, roi, max_points).map_err(error)?
        } else { self.store.viewport(
                id,
                &f.id,
                &c.channel,
                roi,
                max_points,
            )
            .map_err(error)? };
        Ok(
            json!({"record_id":r.id,"frame_id":f.id,"channel":c.channel,"points":points,"calibration":c.calibration,"total_points":c.sample_count}),
        )
    }
    pub fn preview_save(&self, owner: &str, id: &str, name: &str, comment: &str) -> Result<Value, HostError> {
        self.previews.save(&self.store, owner, id, name, comment)
    }
    pub fn preview_discard(&self, owner: &str, id: &str) -> Result<Value, HostError> {
        self.previews.discard(owner, id)
    }
    pub fn analysis(&self, owner: &str, request: AnalysisRequest) -> Result<Value, HostError> {
        let store = self.store.clone();
        self.submit(owner, "analysis", move |ctx| {
            ctx.progress("read", 0, 1);
            let r = store
                .load_with_cancel(&request.record_id, &|| ctx.cancelled())
                .map_err(data_error)?;
            ctx.check()?;
            ctx.progress("fit", 0, 1);
            let result = yang_data::analyze_with_cancel(&r, &request, &|| ctx.cancelled())
                .map_err(data_error)?;
            ctx.check()?;
            ctx.progress("save", 0, 1);
            store.append_analysis(result.clone()).map_err(data_error)?;
            ctx.progress("save", 1, 1);
            Ok(json!({"record_id":r.id,"analysis":result}))
        })
    }
    pub fn save_run(&self, run: &yang_experiments::RunRecord) -> Result<(), HostError> {
        if !yang_data::valid_id(&run.id) {
            return Err(error("Invalid run identity"));
        }
        let bytes = serde_json::to_vec(run).map_err(error)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(error("Run evidence exceeds2MiB"));
        }
        let directory = self.root.join("runs");
        std::fs::create_dir_all(&directory).map_err(error)?;
        write_atomic(&directory.join(format!("{}.json", run.id)), &bytes).map_err(error)
    }
    pub fn run(&self, id: &str) -> Result<Value, HostError> {
        if !yang_data::valid_id(id) {
            return Err(error("Invalid run identity"));
        }
        let path = self.root.join("runs").join(format!("{id}.json"));
        if std::fs::metadata(&path).map_err(error)?.len() > 2 * 1024 * 1024 {
            return Err(error("Run evidence oversized"));
        }
        serde_json::from_slice(&std::fs::read(path).map_err(error)?).map_err(error)
    }
    pub fn runs(&self) -> Result<Value, HostError> {
        let directory = self.root.join("runs");
        if !directory.exists() {
            return Ok(json!([]));
        }
        let mut paths: Vec<_> = std::fs::read_dir(directory)
            .map_err(error)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        let mut runs = vec![];
        for path in paths.iter().rev().take(128) {
            if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                let mut r = self.run(id)?;
                if let Some(object) = r.as_object_mut() {
                    object.remove("steps");
                    object.remove("definition");
                }
                runs.push(r);
            }
        }
        Ok(json!(runs))
    }
    pub fn definitions(&self) -> Result<Value, HostError> {
        let p = self.root.join("experiments.json");
        if !p.exists() {
            return Ok(json!({"experiments":[]}));
        }
        let definitions = parse_saved_definitions(
            &super::registry::read_config(&p, MAX_DEFINITION_BYTES).map_err(error)?)?;
        Ok(json!({"experiments":definitions}))
    }
    pub fn save_definition(
        &self,
        d: yang_experiments::ExperimentDefinition,
    ) -> Result<Value, HostError> {
        let _gate = self.definitions_gate.lock().unwrap();
        yang_experiments::validate_definition(&d).map_err(error)?;
        let mut list: Vec<yang_experiments::ExperimentDefinition> =
            serde_json::from_value(self.definitions()?["experiments"].clone()).map_err(error)?;
        list.retain(|x| x.id != d.id);
        if list.len() >= 128 {
            return Err(error("Experiment limit reached"));
        }
        list.push(d.clone());
        write_atomic(
            &self.root.join("experiments.json"),
            &serde_json::to_vec(&list).map_err(error)?,
        )
        .map_err(error)?;
        Ok(json!({"saved":true,"definition":d}))
    }
    pub fn import_begin(&self, owner: &str, name: &str, size: u64) -> Result<Value, HostError> {
        let extension = Path::new(name)
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "db" | "csv" | "npz") || size == 0 || size > 536_870_912 {
            return Err(error("Choose a .db, .csv or .npz file up to512MiB"));
        }
        let mut uploads = self.uploads.lock().unwrap();
        if uploads.len() >= 4 || uploads.values().map(|u| u.size).sum::<u64>() + size > 536_870_912
        {
            return Err(HostError::new("Busy", "Import staging budget full"));
        }
        let id = yang_data::new_id().map_err(error)?;
        let dir = self.root.join("imports");
        std::fs::create_dir_all(&dir).map_err(error)?;
        let path = dir.join(format!("{id}.{extension}"));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(error)?;
        uploads.insert(
            id.clone(),
            Upload {
                owner: owner.into(),
                path,
                original_name: Path::new(name)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name)
                    .into(),
                size,
                received: 0,
            },
        );
        Ok(json!({"transfer_id":id,"chunk_bytes":16384}))
    }
    pub fn import_chunk(
        &self,
        owner: &str,
        id: &str,
        offset: u64,
        hex: &str,
    ) -> Result<Value, HostError> {
        use std::io::Write;
        let bytes = decode(hex)?;
        let mut uploads = self.uploads.lock().unwrap();
        let u = uploads
            .get_mut(id)
            .filter(|u| u.owner == owner)
            .ok_or_else(|| error("Import transfer absent"))?;
        if offset != u.received || bytes.is_empty() || u.received + bytes.len() as u64 > u.size {
            return Err(error("Import chunk out of order or outside declared size"));
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(&u.path)
            .map_err(error)?
            .write_all(&bytes)
            .map_err(error)?;
        u.received += bytes.len() as u64;
        Ok(json!({"transfer_id":id,"received":u.received}))
    }
    pub fn import_commit(&self, owner: &str, id: &str, append: bool) -> Result<Value, HostError> {
        let mut uploads = self.uploads.lock().unwrap();
        let u = uploads
            .get(id)
            .filter(|u| u.owner == owner)
            .ok_or_else(|| error("Import transfer absent"))?;
        if u.received != u.size {
            return Err(error("Import transfer incomplete"));
        }
        let path = u.path.clone();
        let original_name = u.original_name.clone();
        let store = self.store.clone();
        let submitted = self.submit(owner, "import", move |ctx| {
            let parsed = yang_data::import_path_with_cancel(
                &path,
                &yang_data::ImportLimits::default(),
                &|| ctx.cancelled(),
            );
            let _ = std::fs::remove_file(&path);
            let mut report = parsed.map_err(data_error)?;
            let original = Path::new(&original_name);
            let generated_label = original.extension().and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("csv") || s.eq_ignore_ascii_case("npz"));
            for record in &mut report.records {
                if generated_label {
                    if let Some(stem) = original.file_stem().and_then(|s| s.to_str()) {
                        record.name = stem.into();
                    }
                }
                record.source["original_file_name"] = json!(original_name);
            }
            ctx.check()?;
            let old = if append {
                vec![]
            } else {
                store.list().map_err(error)?
            };
            let mut ids = vec![];
            for (index, record) in report.records.iter().enumerate() {
                ctx.check()?;
                ctx.progress("save", index as u64, report.records.len() as u64);
                store
                    .save_with_cancel(record, &|| ctx.cancelled())
                    .map_err(data_error)?;
                ids.push(record.id.clone());
            }
            ctx.check()?;
            for r in old {
                if !ids.contains(&r.id) {
                    store.remove(&r.id).map_err(error)?;
                }
            }
            Ok(json!({"record_ids":ids,"warnings":report.warnings}))
        });
        if submitted.is_ok() {
            uploads.remove(id);
        }
        submitted
    }
    pub fn import_abort(&self, owner: &str, id: &str) -> Result<Value, HostError> {
        let mut uploads = self.uploads.lock().unwrap();
        let u = uploads
            .get(id)
            .filter(|u| u.owner == owner)
            .ok_or_else(|| error("Import transfer absent for this session"))?;
        std::fs::remove_file(&u.path).map_err(error)?;
        uploads.remove(id);
        Ok(json!({"released":true}))
    }
    pub fn export(&self, owner: &str, params: &Value) -> Result<Value, HostError> {
        let ids: Vec<String> =
            serde_json::from_value(params["record_ids"].clone()).map_err(error)?;
        if ids.is_empty() || ids.len() > 256 || ids.iter().any(|id| !yang_data::valid_id(id)) {
            return Err(error("Select1..256 record identities"));
        }
        let format = match params["format"].as_str() {
            Some("csv") => yang_data::ExportFormat::Csv,
            Some("npz") => yang_data::ExportFormat::Npz,
            Some("db" | "native") => yang_data::ExportFormat::Native,
            _ => return Err(error("Unknown export format")),
        };
        let frame_ids = params["frame_id"]
            .as_str()
            .map(|v| vec![v.into()])
            .unwrap_or_default();
        let dir = self
            .root
            .join("exports")
            .join(yang_data::new_id().map_err(error)?);
        let store = self.store.clone();
        let downloads = self.downloads.clone();
        let gate = self.export_gate.clone();
        let owner = owner.to_owned();
        let task_owner = owner.clone();
        let previews = self.previews.clone();
        let pinned = ids.iter().map(|id| {
            if previews.saved(&owner, id)? == Some(false) { previews.record(&owner, id) } else { Ok(None) }
        }).collect::<Result<Vec<_>, HostError>>()?;
        let kind = if pinned.iter().any(Option::is_some) { "preview_export" } else { "export" };
        self.submit(&owner,kind,move|ctx|{
   let _gate=gate.try_lock().map_err(|_|HostError::new("Busy","Another export is being prepared"))?;
   if !downloads.lock().unwrap().is_empty(){return Err(HostError::new("Busy","Finish or release the previous export before preparing another"));}
   // Admit aggregate raw payload before allocating records. Display/list paths never read raw arrays.
   let mut raw_bytes=0u64;for (id,preview) in ids.iter().zip(&pinned){ctx.check()?;let metadata=if let Some(record)=preview {previews::metadata(record)}else{store.metadata(id).map_err(error)?};for frame in metadata.frames{for channel in frame.channels{let width=match channel.dtype.as_str(){"|u1"|"|i1"=>1,"<u2"|"<i2"=>2,"<f8"=>8,_=>return Err(error("Unsupported stored raw dtype"))};raw_bytes=raw_bytes.checked_add((channel.sample_count as u64).checked_mul(width).ok_or_else(||error("Export size overflow"))?).ok_or_else(||error("Export size overflow"))?;}}}
   if raw_bytes>536_870_912{return Err(error("Export raw input exceeds512MiB; select fewer records"));}
   std::fs::create_dir_all(&dir).map_err(error)?;
   let dir=dir.canonicalize().map_err(error)?;
   let prepared=(||{
    let mut records=vec![];for (id,preview) in ids.into_iter().zip(pinned){ctx.check()?;records.push(if let Some(record)=preview {record}else{Arc::new(store.load_with_cancel(&id,&||ctx.cancelled()).map_err(data_error)?)});}
    ctx.progress("export",0,records.len()as u64);
    let refs=records.iter().map(AsRef::as_ref).collect::<Vec<_>>();
    let report=yang_data::export_record_refs_with_cancel(&refs,&yang_data::ExportRequest{directory:dir.clone(),format,frame_ids},&||ctx.cancelled());ctx.check()?;
    let mut files=vec![];let mut staged=BTreeMap::new();let mut staged_bytes=0u64;
    for f in report.files{
     if let Some(path)=f.path{
      let mut paths=vec![path.clone()];if matches!(path.extension().and_then(|v|v.to_str()),Some("csv"|"npz")){let ext=path.extension().unwrap().to_string_lossy();let sidecar=path.with_extension(format!("{ext}.metadata.json"));if !sidecar.is_file(){files.push(json!({"record_id":f.record_id,"frame_id":f.frame_id,"error":"Export provenance sidecar missing"}));continue;}paths.push(sidecar);}
      for path in paths{ctx.check()?;let meta=std::fs::metadata(&path).map_err(error)?;if !meta.is_file()||path.parent()!=Some(dir.as_path()){return Err(error("Export did not produce an ordinary staged file"));}staged_bytes=staged_bytes.checked_add(meta.len()).ok_or_else(||error("Export size overflow"))?;if staged_bytes>2_147_483_648{return Err(error("Export staging exceeds2GiB; select fewer records"));}let id=yang_data::new_id().map_err(error)?;let name=path.file_name().unwrap_or_default().to_string_lossy().into_owned();let sha256=file_sha256(&path,&||ctx.cancelled())?;files.push(json!({"record_id":f.record_id,"frame_id":f.frame_id,"file_id":id,"name":name,"byte_count":meta.len(),"sha256":sha256,"error":f.error}));staged.insert(id,Download{owner:task_owner.clone(),job_id:ctx.id().into(),path,size:meta.len(),sha256});}
     }else{files.push(json!({"record_id":f.record_id,"frame_id":f.frame_id,"error":f.error}));}
    }
    // Serialize publication against owner departure: a cancelled owner can never
    // publish unreachable staging after cancel_owner has already swept it.
    let mut targets=downloads.lock().unwrap();ctx.check()?;*targets=staged;Ok(json!({"files":files}))
   })();
   if prepared.is_err(){clean_staging(&dir);}prepared
  })
    }
    pub fn export_release(&self, owner: &str, id: &str) -> Result<Value, HostError> {
        let mut targets = self.downloads.lock().unwrap();
        let ids: Vec<_> = targets
            .iter()
            .filter(|(_, t)| t.owner == owner && t.job_id == id)
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            self.status(id, owner)?;
        }
        let mut directories = std::collections::BTreeSet::new();
        for id in ids {
            if let Some(t) = targets.get(&id) {
                std::fs::remove_file(&t.path).map_err(error)?;
                if let Some(p) = t.path.parent() {
                    directories.insert(p.to_owned());
                }
            }
            targets.remove(&id);
        }
        for dir in directories {
            clean_staging(&dir);
        }
        Ok(json!({"released":true}))
    }
    pub fn export_chunk(&self, owner: &str, params: &Value) -> Result<Value, HostError> {
        use std::io::{Read, Seek, SeekFrom};
        let id = text(params, "file_id")?;
        let targets = self.downloads.lock().unwrap();
        let target = targets
            .get(id)
            .filter(|t| t.owner == owner && Some(t.job_id.as_str()) == params["job_id"].as_str())
            .ok_or_else(|| error("Export file absent for this job/session"))?;
        let offset = params["offset"]
            .as_u64()
            .ok_or_else(|| error("Offset missing"))?;
        let length = params["length"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= 16384)
            .ok_or_else(|| error("Chunk size must be1..16384"))?;
        if offset
            .checked_add(length)
            .is_none_or(|end| end > target.size)
        {
            return Err(error("Export range out of bounds"));
        }
        let mut f = std::fs::File::open(&target.path).map_err(error)?;
        f.seek(SeekFrom::Start(offset)).map_err(error)?;
        let mut bytes = vec![0; length as usize];
        f.read_exact(&mut bytes).map_err(error)?;
        Ok(
            json!({"file_id":id,"job_id":target.job_id,"offset":offset,"length":length,"sha256":target.sha256,"data_hex":bytes.iter().map(|b|format!("{b:02x}")).collect::<String>()}),
        )
    }
}
fn file_sha256(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<String, HostError> {
    use std::io::Read;
    let mut input = std::fs::File::open(path).map_err(error)?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 65536];
    loop {
        if cancelled() {
            return Err(HostError::new("Cancelled", "Export checksum cancelled"));
        }
        let size = input.read(&mut buffer).map_err(error)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(hash
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
// Only remove files created in this random private transfer directory; never recurse.
fn clean_staging(directory: &Path) {
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(directory);
}
fn clean_stale_transfers(root: &Path) {
    if let Ok(entries) = std::fs::read_dir(root.join("imports")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_file())
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(yang_data::valid_id)
                && matches!(
                    path.extension().and_then(|s| s.to_str()),
                    Some("csv" | "npz" | "db")
                )
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(root.join("exports")) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.file_name().to_str().is_some_and(yang_data::valid_id)
            {
                clean_staging(&entry.path());
            }
        }
    }
}
fn decode(hex: &str) -> Result<Vec<u8>, HostError> {
    if hex.len() > 32768 || hex.len() % 2 != 0 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(error("Invalid bounded hexadecimal chunk"));
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).map_err(error))
        .collect()
}
pub(crate) fn text<'a>(p: &'a Value, key: &str) -> Result<&'a str, HostError> {
    p[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 65536)
        .ok_or_else(|| error(format!("Missing {key}")))
}
#[cfg(test)]
mod tests;
#[cfg(test)]
mod preview_tests;
