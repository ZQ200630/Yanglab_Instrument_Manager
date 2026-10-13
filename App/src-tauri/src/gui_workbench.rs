use super::*;
use std::io::Read;
use std::{
    collections::{BTreeMap, BTreeSet},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use std::{future::Future, pin::Pin};
struct FileJob {
    owner: String,
    value: Value,
    cancel: Arc<AtomicBool>,
    started: Instant,
    finished: Option<u64>,
}
#[derive(Clone, Default)]
pub(crate) struct FileJobs {
    jobs: Arc<Mutex<BTreeMap<String, FileJob>>>,
    fenced_hosts: Arc<Mutex<BTreeSet<String>>>,
}
#[derive(Clone)]
struct FileContext {
    id: String,
    jobs: FileJobs,
    cancel: Arc<AtomicBool>,
}
fn settled(status: Option<&str>) -> bool {
    matches!(
        status,
        Some("completed" | "failed" | "cancelled" | "uncertain")
    )
}
impl FileJobs {
    pub(crate) async fn close(&self) -> Result<(), HostError> {
        self.drain(None, Duration::from_secs(120)).await
    }
    pub(crate) async fn close_host(&self, host: &str) -> Result<(), HostError> {
        if !crate::host::contracts::valid_id(host) {
            return Err(error("Invalid owning Host identity"));
        }
        self.fenced_hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(host.into());
        self.drain(Some(host), Duration::from_secs(120)).await
    }
    pub(crate) fn reopen_host(&self, host: &str) {
        self.fenced_hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(host);
    }
    fn check_host(&self, host: Option<&str>) -> Result<(), HostError> {
        if host.is_some_and(|host| {
            self.fenced_hosts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(host)
        }) {
            Err(HostError::new("NativeFileHostClosing", "The owning Host is disconnecting or disconnected. Reconnect before starting another file job."))
        } else {
            Ok(())
        }
    }
    async fn drain(&self, host: Option<&str>, timeout: Duration) -> Result<(), HostError> {
        if let Some(host) = host {
            self.cancel_host(host);
        } else {
            self.cancel_all();
        }
        let suffix = host.map(|host| format!(":{host}"));
        let deadline = Instant::now() + timeout;
        loop {
            let pending: Vec<String> = self
                .jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|(_, job)| {
                    suffix
                        .as_ref()
                        .is_none_or(|suffix| job.owner.ends_with(suffix))
                        && !settled(job.value["status"].as_str())
                })
                .map(|(id, _)| id.clone())
                .collect();
            if pending.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(HostError::new("NativeFileCleanupRetained",format!("Native file cleanup boundary remains unconfirmed for {}. Keep this window and retry close; do not replay the transfer.",pending.join(", "))));
            }
            tokio::time::sleep(
                Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }
    fn status(&self, id: &str, owner: &str) -> Result<Value, HostError> {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let j = jobs
            .get(id)
            .filter(|j| j.owner == owner)
            .ok_or_else(|| error("Native file job not found for this window"))?;
        let mut v = j.value.clone();
        v["elapsed_ms"] = json!(j
            .finished
            .unwrap_or_else(|| j.started.elapsed().as_millis() as u64));
        Ok(v)
    }
    fn cancel(&self, id: &str, owner: &str) -> Result<Value, HostError> {
        {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            let j = jobs
                .get_mut(id)
                .filter(|j| j.owner == owner)
                .ok_or_else(|| error("Native file job not found for this window"))?;
            if !settled(j.value["status"].as_str()) {
                j.cancel.store(true, Ordering::Release);
                j.value["cancel_requested"] = json!(true);
            }
        }
        self.status(id, owner)
    }
    pub(crate) fn cancel_all(&self) {
        for j in self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            if !settled(j.value["status"].as_str()) {
                j.cancel.store(true, Ordering::Release);
                j.value["cancel_requested"] = json!(true);
            }
        }
    }
    pub(crate) fn cancel_host(&self, host: &str) {
        let suffix = format!(":{host}");
        for j in self
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            if j.owner.ends_with(&suffix) && !settled(j.value["status"].as_str()) {
                j.cancel.store(true, Ordering::Release);
                j.value["cancel_requested"] = json!(true);
            }
        }
    }
    fn submit(
        &self,
        owner: &str,
        kind: &str,
        work: impl FnOnce(FileContext) -> Result<Value, HostError> + Send + 'static,
    ) -> Result<Value, HostError> {
        // Keep admission and insertion ordered with close_host's fence. A picker
        // opened before Disconnect must not enqueue work after its drain ends.
        let fences = self.fenced_hosts.lock().unwrap_or_else(|e| e.into_inner());
        if owner
            .rsplit_once(':')
            .is_some_and(|(_, host)| fences.contains(host))
        {
            return Err(HostError::new("NativeFileHostClosing", "The owning Host is disconnecting or disconnected. Reconnect before starting another file job."));
        }
        let id = new_id()?;
        let flag = Arc::new(AtomicBool::new(false));
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs
            .values()
            .filter(|j| !settled(j.value["status"].as_str()))
            .count()
            >= 2
        {
            return Err(HostError::new(
                "Busy",
                "Two native file jobs are already active",
            ));
        }
        if jobs.len() >= 64 {
            if let Some(old) = jobs
                .iter()
                .filter(|(_, j)| settled(j.value["status"].as_str()))
                .min_by_key(|(_, j)| j.started)
                .map(|(id, _)| id.clone())
            {
                jobs.remove(&old);
            }
        }
        jobs.insert(id.clone(),FileJob{owner:owner.into(),value:json!({"id":id,"kind":"native_file","operation":kind,"status":"queued","stage":"Starting native file transfer","completed_units":0,"total_units":0,"result":null,"error":null}),cancel:flag.clone(),started:Instant::now(),finished:None});
        drop(jobs);
        drop(fences);
        let context = FileContext {
            id: id.clone(),
            jobs: self.clone(),
            cancel: flag,
        };
        let worker = context.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("yang-native-file".into())
            .spawn(move || {
                worker.progress("Starting native file transfer", 0, 0);
                let result = catch_unwind(AssertUnwindSafe(|| {
                    worker.check()?;
                    work(context)
                }));
                let mut jobs = worker.jobs.jobs.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(j) = jobs.get_mut(&worker.id) {
                    match result {
                        Ok(Ok(value)) => {
                            j.value["status"] = json!("completed");
                            j.value["stage"] = json!("completed");
                            j.value["result"] = value;
                        }
                        Ok(Err(e)) => {
                            let status = if e.code == "Cancelled" {
                                "cancelled"
                            } else if matches!(
                                e.code.as_str(),
                                "OutcomeUnknown" | "HostOffline" | "HostProtocol"
                            ) {
                                "uncertain"
                            } else {
                                "failed"
                            };
                            j.value["status"] = json!(status);
                            j.value["stage"] = json!(status);
                            j.value["error"] = json!(e);
                        }
                        Err(_) => {
                            j.value["status"] = json!("uncertain");
                            j.value["error"] = json!(HostError::new(
                                "OutcomeUnknown",
                                "Native file task panicked; no automatic retry"
                            ));
                        }
                    }
                    j.finished = Some(j.started.elapsed().as_millis() as u64);
                }
            })
        {
            self.jobs.lock().unwrap().remove(&id);
            return Err(error(e));
        }
        self.status(&id, owner)
    }
}
impl FileContext {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    fn check(&self) -> Result<(), HostError> {
        if self.cancelled() {
            Err(HostError::new(
                "Cancelled",
                "File transfer cancelled at a confirmed boundary",
            ))
        } else {
            Ok(())
        }
    }
    fn progress(&self, stage: &str, completed: u64, total: u64) {
        let mut jobs = self.jobs.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(j) = jobs.get_mut(&self.id) {
            j.value["status"] = json!("running");
            j.value["stage"] = json!(stage);
            j.value["completed_units"] = json!(completed);
            j.value["total_units"] = json!(total);
        }
    }
    fn result(&self, value: Value) {
        if let Some(j) = self
            .jobs
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&self.id)
        {
            j.value["result"] = value;
        }
    }
}
#[tauri::command]
pub async fn workbench_file_status(
    window: tauri::Window,
    state: tauri::State<'_, GuiState>,
    job_id: String,
    host_id: Option<String>,
) -> Result<Value, HostError> {
    state
        .file_jobs
        .status(&job_id, &file_owner(window.label(), host_id.as_deref())?)
}
#[tauri::command]
pub async fn workbench_file_cancel(
    window: tauri::Window,
    state: tauri::State<'_, GuiState>,
    job_id: String,
    host_id: Option<String>,
) -> Result<Value, HostError> {
    state
        .file_jobs
        .cancel(&job_id, &file_owner(window.label(), host_id.as_deref())?)
}
fn error(e: impl std::fmt::Display) -> HostError {
    HostError::new("WorkbenchFile", e.to_string())
}
type Rpc = Arc<
    dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = Result<Value, HostError>> + Send>>
        + Send
        + Sync,
>;
fn file_owner(window: &str, host: Option<&str>) -> Result<String, HostError> {
    if host.is_some_and(|id| !crate::host::contracts::valid_id(id)) {
        return Err(error("Invalid owning Host identity"));
    }
    Ok(format!("{window}:{}", host.unwrap_or("local")))
}
fn native_port(
    state: &GuiState,
    remote: &crate::remote_gui::RemoteGuiState,
    host: Option<&str>,
) -> Result<Rpc, HostError> {
    if let Some(host) = host {
        let [results, status, safety] = remote.workbench_clients(host)?;
        return Ok(Arc::new(move |method, params| {
            let client = match method.as_str() {
                "analysis_status" => status.clone(),
                "job_cancel" | "data_import_abort" | "data_export_release" => safety.clone(),
                _ => results.clone(),
            };
            Box::pin(async move {
                let r = client
                    .call(HostRequest {
                        v: 1,
                        id: new_id()?,
                        method,
                        params,
                    })
                    .await?;
                if r.ok {
                    Ok(r.result)
                } else {
                    Err(r.error.unwrap_or_else(|| {
                        HostError::new("HostProtocol", "Missing remote error reply")
                    }))
                }
            })
        }));
    }
    let results = state.client("data_export")?;
    let status = state.client("analysis_status")?;
    let safety = state.client("job_cancel")?;
    Ok(Arc::new(move |method, params| {
        let client = match method.as_str() {
            "analysis_status" => status.clone(),
            "job_cancel" | "data_import_abort" | "data_export_release" => safety.clone(),
            _ => results.clone(),
        };
        Box::pin(async move {
            let r = client
                .call(HostRequest {
                    v: 1,
                    id: new_id()?,
                    method,
                    params,
                })
                .await?;
            if r.ok {
                Ok(r.result)
            } else {
                Err(r
                    .error
                    .unwrap_or_else(|| HostError::new("HostProtocol", "Missing error reply")))
            }
        })
    }))
}
fn runtime() -> Result<tokio::runtime::Runtime, HostError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(error)
}
fn host_job_id(job: &Value) -> Result<String, HostError> {
    job["id"]
        .as_str()
        .or_else(|| job["job_id"].as_str())
        .filter(|id| crate::host::contracts::valid_id(id))
        .map(str::to_owned)
        .ok_or_else(|| {
            HostError::new(
                "OutcomeUnknown",
                "Host did not return a valid file job identity; do not replay automatically",
            )
        })
}
async fn wait_host(ctx: &FileContext, id: &str, port: &Rpc) -> Result<Value, HostError> {
    let mut cancel_sent = false;
    loop {
        if ctx.cancelled() && !cancel_sent {
            port("job_cancel".into(), json!({"job_id":id})).await?;
            cancel_sent = true;
        }
        let s = port("analysis_status".into(), json!({"job_id":id})).await?;
        if s["id"] != id && s["job_id"] != id {
            return Err(HostError::new(
                "OutcomeUnknown",
                "Host file job status identity mismatch",
            ));
        }
        ctx.progress(
            &format!("Host: {}", s["stage"].as_str().unwrap_or("processing")),
            s["completed_units"].as_u64().unwrap_or(0),
            s["total_units"].as_u64().unwrap_or(0),
        );
        match s["status"].as_str() {
            Some("completed") => return Ok(s["result"].clone()),
            Some("cancelled") => {
                return Err(HostError::new(
                    "Cancelled",
                    "Host confirmed file job cancellation",
                ))
            }
            Some("failed" | "uncertain") => {
                let code = if s["status"] == "uncertain" {
                    "OutcomeUnknown"
                } else {
                    s["error"]["code"].as_str().unwrap_or("WorkbenchFile")
                };
                return Err(HostError::new(
                    code,
                    s["error"]["message"]
                        .as_str()
                        .unwrap_or("Host file job failed"),
                ));
            }
            Some("queued" | "running") => {}
            _ => {
                return Err(HostError::new(
                    "OutcomeUnknown",
                    "Invalid Host file job status",
                ))
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn import_file(
    ctx: FileContext,
    mut file: std::fs::File,
    name: String,
    append: bool,
    port: Rpc,
) -> Result<Value, HostError> {
    ctx.check()?;
    let size = file.metadata().map_err(error)?.len();
    if size == 0 || size > 536_870_912 {
        return Err(error("Import must contain1..512MiB"));
    }
    ctx.progress("Starting import upload", 0, size);
    let begin = port(
        "data_import_begin".into(),
        json!({"name":name,"byte_count":size}),
    )
    .await?;
    let id = begin["transfer_id"]
        .as_str()
        .filter(|id| crate::host::contracts::valid_id(id))
        .ok_or_else(|| HostError::new("OutcomeUnknown", "Missing import transfer identity"))?
        .to_owned();
    let mut committed = false;
    let outcome=async{let mut offset=0;let mut buffer=[0u8;16384];while offset<size{ctx.check()?;let length=(size-offset).min(buffer.len()as u64)as usize;let n=file.read(&mut buffer[..length]).map_err(error)?;if n==0{return Err(error("Selected import file was truncated"));}let reply=port("data_import_chunk".into(),json!({"transfer_id":id,"offset":offset,"data_hex":buffer[..n].iter().map(|b|format!("{b:02x}")).collect::<String>()})).await?;offset+=n as u64;if reply["received"]!=offset{return Err(error("Import acknowledgement mismatch"));}ctx.progress("Uploading selected file",offset,size);}
  let mut extra=[0u8;1];if file.read(&mut extra).map_err(error)?!=0{return Err(error("Selected import file length changed"));}ctx.check()?;ctx.progress("Submitting import",0,0);let job=port("data_import_commit".into(),json!({"transfer_id":id,"append":append})).await?;committed=true;let job_id=host_job_id(&job)?;wait_host(&ctx,&job_id,&port).await
 }.await;
    if !committed {
        let released = port("data_import_abort".into(), json!({"transfer_id":id}))
            .await
            .map_err(|e| {
                HostError::new(
                    "OutcomeUnknown",
                    format!("Import staging release is unconfirmed: {}", e.message),
                )
            })?;
        if released["released"] != true {
            return Err(HostError::new(
                "OutcomeUnknown",
                "Import staging release is unconfirmed",
            ));
        }
    }
    outcome
}
async fn export_files(
    ctx: FileContext,
    selected: crate::host::archive::SelectedDirectory,
    params: Value,
    port: Rpc,
) -> Result<Value, HostError> {
    ctx.check()?;
    ctx.progress("Submitting export", 0, 0);
    let job = port("data_export".into(), params).await?;
    let id = host_job_id(&job)?;
    let outcome = async {
        let result = wait_host(&ctx, &id, &port).await?;
        ctx.check()?;
        let files = result["files"]
            .as_array()
            .filter(|files| files.len() <= 4096)
            .ok_or_else(|| error("Missing or oversized export manifest"))?;
        let total = files
            .iter()
            .filter(|f| f["error"].is_null())
            .try_fold(0u64, |sum, f| {
                sum.checked_add(
                    f["byte_count"]
                        .as_u64()
                        .ok_or_else(|| error("Missing export byte count"))?,
                )
                .ok_or_else(|| error("Export byte count overflow"))
            })?;
        if total > 2_147_483_648 {
            return Err(error("Export transfer exceeds2GiB; select fewer records"));
        }
        let mut completed = 0u64;
        let mut reports = vec![];
        ctx.progress("Downloading export files", 0, total);
        for f in files {
            ctx.check()?;
            if !f["error"].is_null() {
                reports.push(f.clone());
                ctx.result(json!({"files":reports,"export_job_id":id}));
                continue;
            }
            let name = f["name"]
                .as_str()
                .ok_or_else(|| error("Missing export filename"))?;
            let file_id = f["file_id"]
                .as_str()
                .filter(|id| crate::host::contracts::valid_id(id))
                .ok_or_else(|| error("Invalid export identity"))?;
            let size = f["byte_count"].as_u64().unwrap();
            let sha = f["sha256"]
                .as_str()
                .filter(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
                .ok_or_else(|| error("Invalid export SHA256"))?;
            let transfer: Result<std::path::PathBuf, HostError> = async {
                let mut output = crate::native_files::AtomicOutput::begin(&selected, name)?;
                let mut offset = 0;
                while offset < size {
                    ctx.check()?;
                    let length = (size - offset).min(16384);
                    let chunk = port(
                        "data_export_chunk".into(),
                        json!({"job_id":id,"file_id":file_id,"offset":offset,"length":length}),
                    )
                    .await?;
                    if chunk["job_id"] != id
                        || chunk["file_id"] != file_id
                        || chunk["offset"] != offset
                        || chunk["length"] != length
                        || chunk["sha256"] != sha
                    {
                        return Err(error("Export chunk scope, length or SHA256 mismatch"));
                    }
                    let hex = chunk["data_hex"]
                        .as_str()
                        .filter(|s| {
                            s.len() == length as usize * 2
                                && s.bytes().all(|c| c.is_ascii_hexdigit())
                        })
                        .ok_or_else(|| error("Invalid bounded export bytes"))?;
                    let bytes: Result<Vec<_>, _> = hex
                        .as_bytes()
                        .chunks_exact(2)
                        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16))
                        .collect();
                    output.write(&bytes.map_err(error)?)?;
                    offset += length;
                    completed += length;
                    ctx.progress("Downloading export files", completed, total);
                }
                ctx.check()?;
                output.publish(size, sha)
            }
            .await;
            match transfer {
                Ok(path) => {
                    reports.push(json!({"name":name,"path":path,"byte_count":size,"sha256":sha}))
                }
                Err(e) => {
                    reports.push(json!({"name":name,"error":e.clone()}));
                    ctx.result(json!({"files":reports,"export_job_id":id}));
                    if matches!(
                        e.code.as_str(),
                        "Cancelled" | "OutcomeUnknown" | "HostOffline" | "HostProtocol"
                    ) {
                        return Err(e);
                    }
                }
            }
            ctx.result(json!({"files":reports,"export_job_id":id}));
        }
        Ok(json!({"files":reports,"export_job_id":id}))
    }
    .await;
    ctx.progress("Releasing Host export staging", 0, 0);
    let released = port("data_export_release".into(), json!({"job_id":id}))
        .await
        .map_err(|e| {
            HostError::new(
                "OutcomeUnknown",
                format!("Export staging release is unconfirmed: {}", e.message),
            )
        })?;
    if released["released"] != true {
        return Err(HostError::new(
            "OutcomeUnknown",
            "Export staging release is unconfirmed",
        ));
    }
    outcome
}
fn check(state: &GuiState) -> Result<(), HostError> {
    if state.closing.load(std::sync::atomic::Ordering::Acquire) {
        Err(error("Client is closing; file job interrupted"))
    } else {
        Ok(())
    }
}
#[tauri::command]
pub async fn workbench_import(
    window: tauri::Window,
    state: tauri::State<'_, GuiState>,
    remote: tauri::State<'_, crate::remote_gui::RemoteGuiState>,
    params: Value,
    host_id: Option<String>,
) -> Result<Value, HostError> {
    if host_id.is_some() {
        return Err(HostError::new(
            "RemoteMethod",
            "Remote import requires separate authorization; use the owning computer for now",
        ));
    }
    check(&state)?;
    let owner_key = file_owner(window.label(), host_id.as_deref())?;
    state.file_jobs.check_host(host_id.as_deref())?;
    native_port(&state, &remote, host_id.as_deref())?;
    let guard = crate::native_files::SelectionGuard::begin(&state.file_dialog_open)?;
    let owner = window.hwnd().map_err(error)?.0 as isize;
    let (choice, guard) = crate::native_files::choose_waveform_file(owner, guard).await?;
    drop(guard);
    check(&state)?;
    let Some((file, name)) = choice else {
        return Ok(json!({"cancelled":true}));
    };
    let port = native_port(&state, &remote, host_id.as_deref())?;
    let append = params["append"].as_bool().unwrap_or(true);
    state.file_jobs.submit(&owner_key, "import", move |ctx| {
        runtime()?.block_on(import_file(ctx, file, name, append, port))
    })
}
#[tauri::command]
pub async fn workbench_export(
    window: tauri::Window,
    state: tauri::State<'_, GuiState>,
    remote: tauri::State<'_, crate::remote_gui::RemoteGuiState>,
    params: Value,
    host_id: Option<String>,
) -> Result<Value, HostError> {
    check(&state)?;
    let owner_key = file_owner(window.label(), host_id.as_deref())?;
    state.file_jobs.check_host(host_id.as_deref())?;
    native_port(&state, &remote, host_id.as_deref())?;
    let guard = crate::native_files::SelectionGuard::begin(&state.file_dialog_open)?;
    let owner = window.hwnd().map_err(error)?.0 as isize;
    let (selected, guard) =
        crate::native_files::choose_folder(owner, "Export waveform records", guard).await?;
    drop(guard);
    check(&state)?;
    let Some(selected) = selected else {
        return Ok(json!({"cancelled":true}));
    };
    let port = native_port(&state, &remote, host_id.as_deref())?;
    state.file_jobs.submit(&owner_key, "export", move |ctx| {
        runtime()?.block_on(export_files(ctx, selected, params, port))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_file_closed_host_rejects_a_late_picker_submission() {
        let jobs = FileJobs::default();
        let host = "a".repeat(32);
        let owner = file_owner("main", Some(&host)).unwrap();
        runtime().unwrap().block_on(jobs.close_host(&host)).unwrap();
        let result = jobs.submit(&owner, "export", |_| Ok(json!({})));
        assert!(
            result.is_err(),
            "a disconnected Host must reject a late picker result"
        );
        assert!(jobs.check_host(Some(&host)).is_err());
        assert!(jobs.check_host(Some(&"b".repeat(32))).is_ok());
        jobs.reopen_host(&host);
        assert!(jobs.check_host(Some(&host)).is_ok());
        let reopened = jobs.submit(&owner, "export", |_| Ok(json!({}))).unwrap();
        runtime().unwrap().block_on(jobs.close_host(&host)).unwrap();
        assert!(settled(
            jobs.status(reopened["id"].as_str().unwrap(), &owner)
                .unwrap()["status"]
                .as_str()
        ));
    }
    #[test]
    fn native_file_close_drains_confirmed_workers_and_only_the_selected_host() {
        let jobs = FileJobs::default();
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        let oa = file_owner("main", Some(&a)).unwrap();
        let ob = file_owner("main", Some(&b)).unwrap();
        let worker = |ctx: FileContext| {
            while !ctx.cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            ctx.check()?;
            Ok(json!({}))
        };
        let first = jobs.submit(&oa, "test", worker).unwrap();
        let second = jobs.submit(&ob, "test", worker).unwrap();
        let runtime = runtime().unwrap();
        runtime.block_on(jobs.close_host(&a)).unwrap();
        assert_eq!(
            jobs.status(first["id"].as_str().unwrap(), &oa).unwrap()["status"],
            "cancelled"
        );
        let unrelated = jobs.status(second["id"].as_str().unwrap(), &ob).unwrap();
        assert!(!settled(unrelated["status"].as_str()));
        assert_ne!(unrelated["cancel_requested"], true);
        runtime.block_on(jobs.close()).unwrap();
        assert_eq!(
            jobs.status(second["id"].as_str().unwrap(), &ob).unwrap()["status"],
            "cancelled"
        );
    }
    #[test]
    fn native_file_close_timeout_retains_unfinished_boundary_until_worker_confirms() {
        let jobs = FileJobs::default();
        let release = Arc::new(AtomicBool::new(false));
        let gate = release.clone();
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let job = jobs
            .submit("main", "test", move |ctx| {
                entered.send(()).unwrap();
                while !gate.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                ctx.check()?;
                Ok(json!({}))
            })
            .unwrap();
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let id = job["id"].as_str().unwrap();
        let runtime = runtime().unwrap();
        let error = runtime
            .block_on(jobs.drain(None, Duration::from_millis(10)))
            .unwrap_err();
        assert_eq!(error.code, "NativeFileCleanupRetained");
        let pending = jobs.status(id, "main").unwrap();
        assert!(!settled(pending["status"].as_str()));
        assert_eq!(pending["cancel_requested"], true);
        release.store(true, Ordering::Release);
        runtime.block_on(jobs.close()).unwrap();
        assert_eq!(jobs.status(id, "main").unwrap()["status"], "cancelled");
    }
    fn wait(jobs: &FileJobs, id: &str) -> Value {
        for _ in 0..200 {
            let v = jobs.status(id, "main").unwrap();
            if settled(v["status"].as_str()) {
                return v;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("native file job did not settle")
    }
    fn run<F: Future<Output = Result<Value, HostError>>>(f: F) -> Result<Value, HostError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }
    #[test]
    fn native_file_job_is_bounded_owner_bound_and_cancellation_requires_worker_boundary() {
        let jobs = FileJobs::default();
        let release = Arc::new(AtomicBool::new(false));
        let stop = release.clone();
        let first = jobs
            .submit("main", "test", move |ctx| {
                while !stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                ctx.check()?;
                Ok(json!({}))
            })
            .unwrap();
        let id = first["id"].as_str().unwrap();
        assert!(!settled(first["status"].as_str()));
        assert!(jobs.status(id, "another-window").is_err());
        assert!(jobs.cancel(id, "another-window").is_err());
        let stop = release.clone();
        jobs.submit("main", "test", move |ctx| {
            while !stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
            ctx.check()?;
            Ok(json!({}))
        })
        .unwrap();
        assert!(jobs.submit("main", "test", |_| Ok(json!({}))).is_err());
        let pending = jobs.cancel(id, "main").unwrap();
        assert!(!settled(pending["status"].as_str()));
        assert_eq!(pending["cancel_requested"], true);
        release.store(true, Ordering::Release);
        assert_eq!(wait(&jobs, id)["status"], "cancelled");
        jobs.cancel_all();
    }
    #[test]
    fn native_import_cancel_aborts_owned_staging_without_committing() {
        let path = std::env::temp_dir().join(format!("yang-ui-upload-{}", new_id().unwrap()));
        std::fs::write(&path, vec![42u8; 32768]).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let seen = calls.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let port: Rpc = Arc::new(move |method, params| {
            seen.lock().unwrap().push(method.clone());
            let flag = flag.clone();
            Box::pin(async move {
                match method.as_str() {
                    "data_import_begin" => Ok(json!({"transfer_id":"a".repeat(32)})),
                    "data_import_chunk" => {
                        flag.store(true, Ordering::Release);
                        Ok(
                            json!({"received":params["offset"].as_u64().unwrap()+params["data_hex"].as_str().unwrap().len()as u64/2}),
                        )
                    }
                    "data_import_abort" => Ok(json!({"released":true})),
                    _ => panic!("unexpected import write {method}"),
                }
            })
        });
        let jobs = FileJobs::default();
        let id = new_id().unwrap();
        jobs.jobs.lock().unwrap().insert(
            id.clone(),
            FileJob {
                owner: "main".into(),
                value: json!({"status":"running"}),
                cancel: cancelled.clone(),
                started: Instant::now(),
                finished: None,
            },
        );
        let ctx = FileContext {
            id,
            jobs,
            cancel: cancelled,
        };
        let result = run(import_file(ctx, file, "source.csv".into(), true, port));
        assert_eq!(result.unwrap_err().code, "Cancelled");
        assert_eq!(
            *calls.lock().unwrap(),
            [
                "data_import_begin",
                "data_import_chunk",
                "data_import_abort"
            ]
        );
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn native_export_verifies_bytes_reports_file_failures_and_releases_staging() {
        let root = std::env::temp_dir().join(format!("yang-ui-download-{}", new_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let selected = crate::host::archive::SelectedDirectory::open(&root).unwrap();
        let bytes = b"waveform data";
        let sha = crate::host::verification::sha256_bytes(bytes).unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let seen = calls.clone();
        let port: Rpc = Arc::new(move |method, params| {
            seen.lock().unwrap().push(method.clone());
            let sha = sha.clone();
            Box::pin(async move {
                match method.as_str() {
                    "data_export" => Ok(json!({"id":"b".repeat(32)})),
                    "analysis_status" => Ok(
                        json!({"id":"b".repeat(32),"status":"completed","result":{"files":[{"file_id":"a".repeat(32),"name":"trace.csv","byte_count":bytes.len(),"sha256":sha},{"name":"other.npz","error":"fixture denied"}]}}),
                    ),
                    "data_export_chunk" => Ok(
                        json!({"job_id":params["job_id"],"file_id":params["file_id"],"offset":params["offset"],"length":params["length"],"sha256":sha,"data_hex":bytes.iter().map(|b|format!("{b:02x}")).collect::<String>()}),
                    ),
                    "data_export_release" => Ok(json!({"released":true})),
                    _ => panic!("unexpected export write {method}"),
                }
            })
        });
        let jobs = FileJobs::default();
        let job = jobs
            .submit("main", "export", move |ctx| {
                run(export_files(
                    ctx,
                    selected,
                    json!({"record_ids":["c".repeat(32)],"format":"csv"}),
                    port,
                ))
            })
            .unwrap();
        let result = wait(&jobs, job["id"].as_str().unwrap());
        assert_eq!(result["status"], "completed", "{result}");
        assert_eq!(result["result"]["files"][1]["error"], "fixture denied");
        assert_eq!(std::fs::read(root.join("trace.csv")).unwrap(), bytes);
        assert_eq!(calls.lock().unwrap().last().unwrap(), "data_export_release");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_export_cancel_or_offline_never_publishes_partial_file_and_keeps_outcome_evidence() {
        for fault in ["cancel", "offline", "hash"] {
            let root =
                std::env::temp_dir().join(format!("yang-ui-download-fault-{}", new_id().unwrap()));
            std::fs::create_dir(&root).unwrap();
            let selected = crate::host::archive::SelectedDirectory::open(&root).unwrap();
            let released = Arc::new(AtomicBool::new(false));
            let release = released.clone();
            let jobs = FileJobs::default();
            let tracker = jobs.clone();
            let job=jobs.submit("main","export",move|ctx|{let flag=ctx.cancel.clone();let sha=crate::host::verification::sha256_bytes(b"waveform data").unwrap();let port:Rpc=Arc::new(move|method,params|{let flag=flag.clone();let release=release.clone();let sha=sha.clone();Box::pin(async move{match method.as_str(){"data_export"=>Ok(json!({"id":"b".repeat(32)})),"analysis_status"=>Ok(json!({"id":"b".repeat(32),"status":"completed","result":{"files":[{"file_id":"a".repeat(32),"name":"trace.csv","byte_count":13,"sha256":sha}]}})),"data_export_chunk"=>{if fault=="offline"{return Err(HostError::new("HostOffline","fixture offline"));}if fault=="cancel"{flag.store(true,Ordering::Release);}Ok(json!({"job_id":params["job_id"],"file_id":params["file_id"],"offset":params["offset"],"length":params["length"],"sha256":sha,"data_hex":if fault=="hash"{"ff".repeat(13)}else{b"waveform data".iter().map(|b|format!("{b:02x}")).collect::<String>()}}))},"data_export_release"=>{release.store(true,Ordering::Release);Ok(json!({"released":true}))},_=>panic!("unexpected fault request {method}")}})});run(export_files(ctx,selected,json!({}),port))}).unwrap();
            let result = wait(&tracker, job["id"].as_str().unwrap());
            assert_eq!(
                result["status"],
                match fault {
                    "cancel" => "cancelled",
                    "offline" => "uncertain",
                    _ => "completed",
                },
                "{result}"
            );
            assert!(
                result["result"]["files"][0].get("error").is_some(),
                "partial outcome evidence lost: {result}"
            );
            assert!(released.load(Ordering::Acquire));
            assert_eq!(
                std::fs::read_dir(&root).unwrap().count(),
                0,
                "partial file escaped: {fault}"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn native_file_owner_includes_owning_host_without_opening_or_discovering_peers() {
        let local = file_owner("main", None).unwrap();
        let remote = file_owner("main", Some(&"a".repeat(32))).unwrap();
        assert_ne!(local, remote);
        assert!(file_owner("main", Some("../path")).is_err());
        let peers = crate::remote_gui::RemoteGuiState::default();
        assert!(peers.workbench_clients(&"a".repeat(32)).is_err());
        assert!(peers.workbench_clients("../path").is_err());
    }
    #[test]
    fn native_file_preserves_host_uncertainty_even_when_cause_is_a_storage_error() {
        let jobs = FileJobs::default();
        let job=jobs.submit("main","export",|ctx|{
            let port:Rpc=Arc::new(|_,_|Box::pin(async{Ok(json!({"id":"b".repeat(32),"status":"uncertain","error":{"code":"Workbench","message":"journal durability unconfirmed"}}))}));
            run(wait_host(&ctx,&"b".repeat(32),&port))
        }).unwrap();
        assert_eq!(
            wait(&jobs, job["id"].as_str().unwrap())["status"],
            "uncertain"
        );
    }
}
