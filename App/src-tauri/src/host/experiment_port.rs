//! Real, session-bound adapter for the fixed native experiment templates.
use super::super::leases::Lease;
use super::super::workbench::JobContext;
use super::*;
use yang_data::{
    AnalysisMethod, AnalysisRequest, DataStore, SampleRange, WaveformFrame, WaveformRecord,
};
use yang_experiments::{
    ExperimentDefinition, ExperimentError, ExperimentPort, RoleBinding, RoleInfo, RunStatus,
};

pub(super) fn start(
    core: Arc<HostCore>,
    owner: &str,
    definition: ExperimentDefinition,
) -> Result<Value, HostError> {
    start_mode(core, owner, definition, false)
}
pub(super) fn capture_start(core: Arc<HostCore>, owner: &str, definition: ExperimentDefinition) -> Result<Value, HostError> {
    if definition.template_id != "waveform_capture" {
        return Err(HostError::new("ManualCapture", "Manual Acquire requires the waveform capture template"));
    }
    start_mode(core, owner, definition, true)
}
fn start_mode(core: Arc<HostCore>, owner: &str, definition: ExperimentDefinition, manual: bool) -> Result<Value, HostError> {
    let snapshot = core.snapshot()?;
    let baseline = preflight(&definition, &snapshot)?;
    if !core.clients.lock().unwrap().control_allowed(owner) {
        return Err(HostError::new(
            "ControlDenied",
            "The requesting local client is no longer active",
        ));
    }
    let session = Session::local(owner.into(), core.boot_id.clone())?;
    let journal = Session::local(new_id()?, core.boot_id.clone())?;
    let contexts = definition
        .roles
        .iter()
        .map(|b| {
            (
                b.role.clone(),
                snapshot["domains"][format!("device:{}", b.domain_id)]["context"].clone(),
            )
        })
        .collect();
    let workbench = core.workbench.as_ref().map_err(Clone::clone)?.clone();
    let handle = tokio::runtime::Handle::try_current().map_err(|_| {
        HostError::new("HostRuntime", "Experiment Start requires the Host executor")
    })?;
    let save = workbench.clone();
    // Reserve the aggregate budget before admitting any acquisition or configuration.
    let frame_budget = frame_budget(&definition, &snapshot)?;
    let reservation = if manual { Some(workbench.previews.reserve(owner,
        frame_budget.checked_mul(definition.parameters["frames"].as_u64().unwrap_or(1))
            .ok_or_else(|| HostError::new("PreviewBudget", "Preview budget overflow"))?)?) } else { None };
    let mut accepted = workbench.submit(owner, if manual { "capture" } else { "experiment" }, move |ctx| {
        let held = Arc::new(Mutex::new(BTreeMap::<DomainRef, Held>::new()));
        let heartbeat_stop = AtomicBool::new(false);
        let heartbeat_error = Mutex::new(None::<HostError>);
        let mut port = HostExperimentPort {
            core: core.clone(),
            handle,
            owner: session.clone(),
            journal,
            definition: definition.clone(),
            baseline,
            contexts,
            sequence: 0,
            job: &ctx,
            held: held.clone(),
            pending: None,
            record_id: None,
            raw_bytes: 0,
            frame_budget,
            depth_configured: false,
            prepared_generators: BTreeSet::new(),
            manual,
            preview: reservation,
            completed_preview: None,
        };
        let mut run = std::thread::scope(|scope| {
            let _stop = StopHeartbeat(&heartbeat_stop);
            scope.spawn(|| {
                let mut next = Instant::now();
                while !heartbeat_stop.load(Ordering::Acquire) {
                    if Instant::now() >= next {
                        if !core.clients.lock().unwrap().control_allowed(session.id())
                            || core.stopping.load(Ordering::Acquire)
                            || core.stopped.load(Ordering::Acquire)
                        {
                            ctx.cancel_flag().store(true, Ordering::Release);
                        }
                        // Keep original leases alive until run-owned output cleanup
                        // drains, even after the GUI requests stop/disconnect.
                        let failure = {
                            let held = held.lock().unwrap();
                            let mut book = core.leases.lock().unwrap();
                            held.values().find_map(|h| {
                                book.renew(&h.lease.token, &session, Instant::now()).err()
                            })
                        };
                        if let Some(error) = failure {
                            *heartbeat_error.lock().unwrap() = Some(error);
                            ctx.cancel_flag().store(true, Ordering::Release);
                            break;
                        }
                        next = Instant::now() + Duration::from_secs(1);
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            });
            yang_experiments::run_experiment(&definition, &mut port, ctx.cancel_flag(), |r| {
                ctx.progress(&r.stage, r.completed_units, r.total_units)
            })
        });
        if let Some(error) = heartbeat_error.into_inner().unwrap() {
            if run.status == RunStatus::Completed {
                run.status = RunStatus::Uncertain;
                run.stage = "uncertain".into();
                run.error = Some(experiment_error(error));
            }
        }
        if manual {
            if let Some(mut record) = port.completed_preview.take() {
                record.source["run_status"] = json!(run.status);
                let id = record.id.clone();
                let published = port.preview.take().ok_or_else(|| HostError::new("CaptureState", "Preview reservation absent"))
                    .and_then(|reservation| reservation.publish(record));
                if let Err(error) = published {
                    run.records.retain(|record_id| record_id != &id);
                    preview_publication_failure(&mut run, error);
                }
            }
        }
        if let Some(id) = port.record_id.as_ref() {
            match if manual { Ok(None) } else { finalize_evidence(&save.store, id, run.status.clone()) } {
                Ok(Some(id)) => {
                    if !run.records.contains(&id) {
                        run.records.push(id);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    if run.status != RunStatus::Uncertain {
                        run.status = RunStatus::Failed;
                    }
                    run.stage = if run.status == RunStatus::Uncertain {
                        "uncertain"
                    } else {
                        "failed"
                    }
                    .into();
                    run.error = Some(experiment_error(error));
                }
            }
        }
        if !manual { save.save_run(&run)?; }
        Ok(json!({"run":run}))
    })?;
    accepted["run_id"] = accepted["job_id"].clone();
    Ok(accepted)
}

fn preview_publication_failure(run: &mut yang_experiments::RunRecord, error: HostError) {
    if run.status != RunStatus::Uncertain { run.status = RunStatus::Failed; }
    run.stage = if run.status == RunStatus::Uncertain { "uncertain" } else { "failed" }.into();
    run.error = Some(experiment_error(error));
}

struct StopHeartbeat<'a>(&'a AtomicBool);
impl Drop for StopHeartbeat<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
#[derive(Clone)]
struct Held {
    lease: Lease,
    acquired: bool,
}
struct HostExperimentPort<'a> {
    core: Arc<HostCore>,
    handle: tokio::runtime::Handle,
    owner: Session,
    journal: Session,
    definition: ExperimentDefinition,
    baseline: BTreeMap<String, RoleInfo>,
    contexts: BTreeMap<String, Value>,
    sequence: u64,
    job: &'a JobContext,
    held: Arc<Mutex<BTreeMap<DomainRef, Held>>>,
    pending: Option<Value>,
    record_id: Option<String>,
    raw_bytes: u64,
    frame_budget: u64,
    depth_configured: bool,
    prepared_generators: BTreeSet<String>,
    manual: bool,
    preview: Option<super::super::workbench::previews::Reservation>,
    completed_preview: Option<WaveformRecord>,
}
fn experiment_error(e: HostError) -> ExperimentError {
    ExperimentError::new(&e.code, e.message)
}
fn data_error(e: yang_data::DataError) -> ExperimentError {
    ExperimentError::new(
        if e == yang_data::DataError::Cancelled {
            "Cancelled"
        } else {
            "DataError"
        },
        e.to_string(),
    )
}
fn owned_safety_context(before: &Value, after: &Value) -> Result<Value, HostError> {
    if !crate::reply_broker::valid_context_v3(after)
        || before["connection_id"].is_null()
        || ["session_id", "domain", "connection_id"]
            .iter()
            .any(|k| before[*k] != after[*k])
        || after["epoch"]
            .as_u64()
            .zip(before["epoch"].as_u64())
            .is_none_or(|(a, b)| a <= b)
    {
        return Err(HostError::new("StaleContext","Own safety receipt changed physical session or did not advance its fence; no rebind permitted"));
    }
    Ok(after.clone())
}
fn domain(b: &RoleBinding) -> DomainRef {
    DomainRef {
        kind: "device".into(),
        id: b.domain_id.clone(),
    }
}

fn borrow_or_acquire(
    book: &mut LeaseBook,
    session: &Session,
    domain: &DomainRef,
) -> Result<(Lease, bool), HostError> {
    if let Some(lease) = book.for_session(session, domain, Instant::now())? {
        Ok((lease, false))
    } else {
        Ok((book.acquire(session, domain, Instant::now())?, true))
    }
}
fn commit_frame(
    store: &DataStore,
    record: &WaveformRecord,
    pending: &mut Option<Value>,
    mut ack: impl FnMut(&Value) -> Result<(), HostError>,
) -> Result<(), HostError> {
    let frame = record
        .frames
        .last()
        .ok_or_else(|| HostError::new("DataStore", "No completed frame to commit"))?;
    store
        .append_frame(record, frame)
        .map_err(|e| HostError::new("DataStore", e.to_string()))?;
    if let Some(d) = pending.as_ref() {
        ack(d)?;
        pending.take();
    }
    Ok(())
}

fn finalize_evidence(
    store: &DataStore,
    id: &str,
    status: RunStatus,
) -> Result<Option<String>, HostError> {
    let mut source = match store.metadata(id) {
        Ok(metadata) => metadata.source,
        Err(yang_data::DataError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(HostError::new("DataStore", error.to_string())),
    };
    source["run_status"] = json!(status);
    store
        .finalize_record_source(id, &source)
        .map_err(|e| HostError::new("DataStore", e.to_string()))?;
    Ok(Some(id.into()))
}

impl HostExperimentPort<'_> {
    fn current(&self, b: &RoleBinding, ready: bool) -> Result<(RoleInfo, Value), HostError> {
        if ready
            && !self
                .core
                .clients
                .lock()
                .unwrap()
                .control_allowed(self.owner.id())
        {
            return Err(HostError::new(
                "ControlDenied",
                "Experiment owner disconnected",
            ));
        }
        self.handle
            .block_on(self.core.worker_cache_with_refresh(true))?;
        let snapshot = self.core.snapshot()?;
        let key = domain(b).key();
        let info = if ready {
            if self.definition.template_id != "waveform_capture" {
                validate_role_parameters(b, &self.definition, &snapshot)?;
            }
            inspect_role(b, &snapshot)?
        } else {
            self.baseline
                .get(&b.role)
                .cloned()
                .ok_or_else(|| HostError::new("RoleUnavailable", "Unknown run role"))?
        };
        let current_rev = snapshot["registry"]["devices"]
            .as_array()
            .and_then(|v| v.iter().find(|d| d["device_id"] == b.domain_id))
            .and_then(|d| d["config_rev"].as_u64());
        if current_rev != self.baseline.get(&b.role).map(|i| i.config_rev) {
            return Err(HostError::new(
                "ConfigConflict",
                "Experiment role configuration changed after Start",
            ));
        }
        let context = self
            .core
            .worker
            .domain_context(&key)
            .ok_or_else(|| HostError::new("RoleUnavailable", "Worker context absent"))?;
        if self.contexts.get(&b.role) != Some(&context) {
            return Err(HostError::new("StaleContext","Experiment connection/safety generation changed after Start; no action sent to a reconnected device"));
        }
        if snapshot["domains"][&key]["device"]["identity"] != info.identity {
            return Err(HostError::new(
                "IdentityMismatch",
                "Experiment role identity changed; no action sent",
            ));
        }
        Ok((info, context))
    }
    fn submit_typed(
        &mut self,
        b: &RoleBinding,
        name: &str,
        args: &Value,
        safety: bool,
    ) -> Result<String, HostError> {
        if !safety {
            self.job.check()?;
        }
        // A run may inspect and logically resume only its already-adopted
        // generator hold. These are ordinary operations, not cleanup: keep
        // every cancellation, client, lease and original-context admission.
        let held_generator_read_or_resume = b.role == "scan_signal"
            && matches!(name, "read_status" | "resume");
        let (info, context) = self.current(b, !safety && !held_generator_read_or_resume)?;
        // Metadata refresh can wait on Worker I/O. Re-admit ordinary actions
        // only after that boundary; cleanup must still drain after cancellation.
        if !safety {
            self.job.check()?;
            if !self.core.clients.lock().unwrap().control_allowed(self.owner.id()) {
                return Err(HostError::new("ControlDenied", "Experiment owner disconnected"));
            }
        }
        let d = domain(b);
        let lease = self
            .held
            .lock()
            .unwrap()
            .get(&d)
            .map(|h| h.lease.clone())
            .ok_or_else(|| HostError::new("ControlDenied", "Experiment role was not reserved"))?;
        self.core.leases.lock().unwrap().admit(
            &lease.token,
            &self.owner,
            &d,
            lease.control_epoch,
            Instant::now(),
        )?;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| HostError::new("OperationInvalid", "Run sequence exhausted"))?;
        let mut intent = ExecuteParams {
            domain: d,
            lease_token: lease.token,
            control_epoch: lease.control_epoch,
            config_rev: info.config_rev,
            context,
            method: if name == "resume" { "resume" } else { "action" }.into(),
            params: if name == "resume" {
                json!({"confirm":true})
            } else {
                json!({"name":name,"args":args})
            },
            sequence: self.sequence,
            confirmation: None,
        };
        let prepared = intent.clone();
        let journal = self.journal.clone();
        let proof: super::super::operations::Confirmation =
            self.handle
                .block_on(self.core.operations.call(Box::new(move |book| {
                    Ok(
                        serde_json::to_value(book.prepare(&journal, prepared, Instant::now())?)
                            .unwrap(),
                    )
                })))?;
        intent.confirmation = Some(proof.token);
        let id = new_id()?;
        // Journal preparation can also wait. Cancellation confirmed before this
        // submission boundary must prevent a new ordinary instrument operation.
        if !safety {
            self.job.check()?;
        }
        self.handle.block_on(self.core.execute_for_run(
            self.owner.clone(),
            self.journal.clone(),
            id.clone(),
            intent,
        ))?;
        Ok(id)
    }
    fn invoke(
        &mut self,
        b: &RoleBinding,
        name: &str,
        args: &Value,
        safety: bool,
    ) -> Result<Value, ExperimentError> {
        let id = self
            .submit_typed(b, name, args, safety)
            .map_err(experiment_error)?;
        let deadline = Instant::now() + Duration::from_secs(185);
        let mut cancel_request: Option<String> = None;
        loop {
            let record = self
                .core
                .operations
                .read(|book| book.find(&self.journal, &id))
                .map_err(experiment_error)?;
            if record.terminal_at.is_some() {
                if let Some(cancel_id) = cancel_request.as_ref() {
                    let stop = self
                        .core
                        .operations
                        .read(|book| book.find(&self.journal, cancel_id))
                        .map_err(experiment_error)?;
                    if stop.terminal_at.is_none() {
                        if Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(20));
                            continue;
                        }
                        return Err(ExperimentError::new(
                            "OutcomeUnknown",
                            "Capture stopped settling but scope Stop outcome is still unknown",
                        ));
                    }
                    if stop.phase != "completed" {
                        return Err(ExperimentError::new(
                            "OutcomeUnknown",
                            format!("Scope cancellation Stop unconfirmed: {}", stop.phase),
                        ));
                    }
                    self.adopt_receipt(b, &stop)?;
                }
                if safety && name != "resume" && record.phase == "completed" {
                    self.adopt_receipt(b, &record)?;
                }
                return operation_outcome(&record);
            }
            if name == "capture" && self.job.cancelled() && cancel_request.is_none() {
                // Typed safety-priority Stop cancels the active operation latch,
                // not the connection. No hardware capture is replayed.
                cancel_request = Some(
                    self.submit_typed(b, "stop", &json!({}), true)
                        .map_err(experiment_error)?,
                );
            }
            if Instant::now() >= deadline {
                return Err(ExperimentError::new("OutcomeUnknown","Native operation did not settle; query its original journal request before retrying"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn adopt_receipt(
        &mut self,
        b: &RoleBinding,
        record: &OperationRecord,
    ) -> Result<(), ExperimentError> {
        let old = self
            .contexts
            .get(&b.role)
            .ok_or_else(|| ExperimentError::new("StaleContext", "Run role context absent"))?;
        let next =
            owned_safety_context(old, &record.result["context"]).map_err(experiment_error)?;
        if self.core.worker.domain_context(&domain(b).key()).as_ref() != Some(&next) {
            return Err(ExperimentError::new(
                "StaleContext",
                "Another safety/connection generation followed this run's receipt",
            ));
        }
        self.contexts.insert(b.role.clone(), next);
        Ok(())
    }
    fn resume_owned_generator(&mut self, b: &RoleBinding) -> Result<(), ExperimentError> {
        // Off preserves a historical cache; it is not proof that the owned
        // session is healthy. Read both channels once on the adopted fence,
        // then resume only logical admission. Never replay either operation.
        self.invoke(b, "read_status", &json!({}), false)?;
        self.invoke(b, "resume", &json!({"confirm":true}), false)?;
        Ok(())
    }
}

fn operation_outcome(record: &OperationRecord) -> Result<Value, ExperimentError> {
    if record.phase == "completed" {
        return Ok(record.result["result"].clone());
    }
    if matches!(
        record.phase.as_str(),
        "cancelled_before_call" | "superseded_by_safety"
    ) || record.result["error"]["code"] == "Canceled"
    {
        return Err(ExperimentError::new(
            "Cancelled",
            "Native operation confirmed cancellation",
        ));
    }
    let unknown = matches!(
        record.phase.as_str(),
        "failed_after_call_started" | "timed_out_unknown" | "completed_readback_failed"
    );
    Err(ExperimentError::new(
        if unknown {
            "OutcomeUnknown"
        } else {
            record.result["error"]["code"]
                .as_str()
                .unwrap_or("OperationRejected")
        },
        format!("{}: {}", record.phase, record.result),
    ))
}

impl ExperimentPort for HostExperimentPort<'_> {
    fn inspect(&mut self, b: &RoleBinding) -> Result<RoleInfo, ExperimentError> {
        self.current(b, true).map(|v| v.0).map_err(experiment_error)
    }
    fn reserve(&mut self, b: &RoleBinding) -> Result<(), ExperimentError> {
        let _ordered = self.handle.block_on(self.core.ordinary_admission.lock());
        self.current(b, true).map_err(experiment_error)?;
        let d = domain(b);
        let mut gate = self.core.experiment_domains.lock().unwrap();
        if gate.get(&d).is_some_and(|id| id != self.job.id()) {
            return Err(ExperimentError::new(
                "ExperimentOwned",
                "Another run owns this role",
            ));
        }
        let (lease, acquired) =
            borrow_or_acquire(&mut self.core.leases.lock().unwrap(), &self.owner, &d)
                .map_err(experiment_error)?;
        gate.insert(d.clone(), self.job.id().into());
        self.held
            .lock()
            .unwrap()
            .insert(d, Held { lease, acquired });
        Ok(())
    }
    fn release(&mut self, b: &RoleBinding) -> Result<(), ExperimentError> {
        let d = domain(b);
        let held = self.held.lock().unwrap().remove(&d);
        let result = if let Some(held) = held.filter(|h| h.acquired) {
            let ticket = {
                let mut book = self.core.leases.lock().unwrap();
                book.admit(
                    &held.lease.token,
                    &self.owner,
                    &d,
                    held.lease.control_epoch,
                    Instant::now(),
                )
                .and_then(|_| book.revoke(&d, RevokeCause::Released))
            };
            match ticket {
                Ok(ticket) => {
                    self.handle.block_on(async {
                        self.core.schedule_cleanups();
                    });
                    let deadline = Instant::now() + Duration::from_secs(95);
                    loop {
                        if !self
                            .core
                            .leases
                            .lock()
                            .unwrap()
                            .pending_cleanups()
                            .iter()
                            .any(|t| t.ticket_id == ticket.ticket_id)
                        {
                            break Ok(());
                        }
                        if Instant::now() >= deadline {
                            break Err(ExperimentError::new(
                                "CleanupRetained",
                                "New experiment lease release remains unconfirmed",
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
                Err(e) => Err(experiment_error(e)),
            }
        } else {
            Ok(())
        };
        let mut gate = self.core.experiment_domains.lock().unwrap();
        if gate.get(&d).is_some_and(|id| id == self.job.id()) {
            gate.remove(&d);
        }
        result
    }
    fn action(
        &mut self,
        b: &RoleBinding,
        name: &str,
        args: &Value,
    ) -> Result<Value, ExperimentError> {
        let safety = name == "set_output" && args["enabled"] == false;
        let name = if b.role == "laser" && name == "set_wavelength" {
            "goto_wavelength"
        } else {
            name
        };
        let result = self.invoke(b, name, args, safety)?;
        if safety
            && b.role == "scan_signal"
            && !self.prepared_generators.contains(&b.role)
            && !self.job.cancelled()
        {
            self.resume_owned_generator(b)?;
            self.prepared_generators.insert(b.role.clone());
        }
        Ok(result)
    }
    fn wait_ready(&mut self, b: &RoleBinding, cancel: &AtomicBool) -> Result<(), ExperimentError> {
        let deadline = Instant::now() + Duration::from_secs(125);
        loop {
            if cancel.load(Ordering::Acquire) {
                let stop = self.invoke(b, "stop_scan", &json!({"confirm":true}), true);
                return match stop {
                    Ok(_) => Err(ExperimentError::new(
                        "Cancelled",
                        "Coarse-tune Stop was accepted; inspect laser hold evidence",
                    )),
                    Err(e) => Err(e),
                };
            }
            self.current(b, true).map_err(experiment_error)?;
            let snapshot = self.core.snapshot().map_err(experiment_error)?;
            let s = &snapshot["domains"][domain(b).key()]["device"];
            if s["move"]["phase"] == "arrived" && s["motion_pending"] == false {
                return Ok(());
            }
            if matches!(
                s["move"]["phase"].as_str(),
                Some("failed" | "held_off_target" | "interrupted" | "timed_out" | "hold_timed_out")
            ) {
                return Err(ExperimentError::new(
                    "OutcomeUnknown",
                    format!("Coarse tune arrival unconfirmed: {}", s["move"]),
                ));
            }
            if Instant::now() >= deadline {
                return Err(ExperimentError::new("OutcomeUnknown","Coarse wavelength arrival timed out; no automatic replay or further scan actuation"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    fn capture(
        &mut self,
        b: &RoleBinding,
        p: &Value,
        cancel: &AtomicBool,
    ) -> Result<WaveformFrame, ExperimentError> {
        if self.record_id.is_none() || self.pending.is_some() {
            return Err(ExperimentError::new(
                "CaptureState",
                "Previous capture staging is not committed/acknowledged",
            ));
        }
        if self
            .raw_bytes
            .checked_add(self.frame_budget)
            .is_none_or(|n| n > 536_870_912)
        {
            return Err(ExperimentError::new("CaptureBudget","Cumulative run exceeds the native512MiB store budget; next acquisition did not start"));
        }
        if p["source"] == "memory" && !self.depth_configured && !p["memory_depth"].is_null() {
            self.invoke(
                b,
                "set_memory_depth",
                &json!({"points":p["memory_depth"]}),
                false,
            )?;
            self.depth_configured = true;
        }
        // Omitted choice preserves saved experiments' fresh-acquisition semantics.
        // Only validated waveform_capture definitions may select a stopped-buffer
        // snapshot or existing screen; automated measurements retain SINGLE.
        let stopped = self.definition.template_id == "waveform_capture"
            && p["acquisition"] == "stopped";
        let fresh = self.definition.template_id != "waveform_capture"
            || (!stopped && p["acquisition"] != "existing_screen");
        let mut args = json!({"channels":p["channels"],"source":p["source"],"byte_budget":536_870_912u64,"fresh":fresh});
        if stopped {
            // Snapshot STOP is part of capture. The public safety Stop would enter
            // STOP_HELD and revoke ordinary acquisition authority for this run.
            args["stop_before"] = json!(true);
        }
        if p["source"] == "memory" && !p["memory_depth"].is_null() {
            args["points"] = p["memory_depth"].clone();
        }
        let value = self.invoke(b, "capture", &args, false)?;
        let descriptor = value["capture"].clone();
        let bytes = descriptor["byte_count"]
            .as_u64()
            .ok_or_else(|| ExperimentError::new("CaptureBudget", "Capture byte count missing"))?;
        if self
            .raw_bytes
            .checked_add(bytes)
            .is_none_or(|n| n > 536_870_912)
        {
            return Err(ExperimentError::new(
                "CaptureBudget",
                "Completed capture exceeds cumulative budget; Worker staging remains retained",
            ));
        }
        let frame = super::super::waveform_transfer::receive_waveform(
            &descriptor,
            |offset, length| {
                self.core
                    .worker
                    .read_capture_chunk(&descriptor, offset, length)
                    .map_err(|e| HostError::new("WaveformTransfer", e))
            },
            &|| cancel.load(Ordering::Acquire),
            |done, total| self.job.progress("transfer_bytes", done, total),
        )
        .map_err(experiment_error)?;
        let charge = frame
            .channels
            .iter()
            .try_fold(0u64, |n, c| {
                n.checked_add(c.samples.byte_len() as u64 + 256)
            })
            .ok_or_else(|| {
                ExperimentError::new("CaptureBudget", "Capture byte accounting overflow")
            })?;
        self.raw_bytes = self
            .raw_bytes
            .checked_add(charge)
            .filter(|n| *n <= 536_870_912)
            .ok_or_else(|| {
                ExperimentError::new(
                    "CaptureBudget",
                    "Completed capture exceeds store byte budget; staging retained",
                )
            })?;
        self.pending = Some(descriptor);
        Ok(frame)
    }
    fn begin_record(&mut self, r: &WaveformRecord) -> Result<(), ExperimentError> {
        self.record_id = Some(r.id.clone());
        Ok(())
    }
    fn frame_completed(&mut self, r: &WaveformRecord) -> Result<(), ExperimentError> {
        if self.record_id.as_ref() != Some(&r.id) {
            return Err(ExperimentError::new(
                "CaptureState",
                "Grouped capture record identity changed",
            ));
        }
        if self.manual {
            // The runner owns the fully verified native sample vectors. Explicit
            // manual Acquire permits releasing only this temporary Worker staging.
            self.preview.as_ref().ok_or_else(|| ExperimentError::new("CaptureState", "Preview reservation absent"))?
                .validate(r).map_err(experiment_error)?;
            if let Some(d) = self.pending.as_ref() {
                self.core.worker.ack_capture(d).map_err(|e| ExperimentError::new("StagingReleaseUnconfirmed", e))?;
                self.pending.take();
            }
            return Ok(());
        }
        let store = self
            .core
            .workbench
            .as_ref()
            .map_err(Clone::clone)
            .map_err(experiment_error)?
            .store
            .clone();
        commit_frame(&store, r, &mut self.pending, |d| {
            self.core
                .worker
                .ack_capture(d)
                .map_err(|e| HostError::new("StagingReleaseUnconfirmed", e))
        })
        .map_err(experiment_error)
    }
    fn persist(&mut self, r: &WaveformRecord) -> Result<String, ExperimentError> {
        if self.manual { return Err(ExperimentError::new("CaptureState", "Manual preview requires owned finalization")); }
        self.core
            .workbench
            .as_ref()
            .map_err(Clone::clone)
            .map_err(experiment_error)?
            .store
            .finalize_record_source(&r.id, &r.source)
            .map_err(data_error)?;
        Ok(r.id.clone())
    }
    fn persist_owned(&mut self, mut r: WaveformRecord) -> Result<String, ExperimentError> {
        if !self.manual { return self.persist(&r); }
        r.source["kind"] = json!("manual");
        self.preview.as_ref().ok_or_else(|| ExperimentError::new("CaptureState", "Preview reservation absent"))?
            .validate(&r).map_err(experiment_error)?;
        let id = r.id.clone();
        // The runner has drained run-owned cleanup before start_mode publishes
        // the moved record with the final, truthful outcome in its provenance.
        self.completed_preview = Some(r);
        Ok(id)
    }
    fn analyze(&mut self, r: &WaveformRecord, p: &Value) -> Result<Vec<String>, ExperimentError> {
        let store = self
            .core
            .workbench
            .as_ref()
            .map_err(Clone::clone)
            .map_err(experiment_error)?
            .store
            .clone();
        let detector = self
            .definition
            .roles
            .iter()
            .find(|b| b.role == "detector")
            .unwrap();
        let channel = format!("CH{}", detector.channel.unwrap());
        let mut ids = vec![];
        for (index, frame) in r.frames.iter().enumerate() {
            self.job.check().map_err(experiment_error)?;
            self.job
                .progress("analyze", index as u64, r.frames.len() as u64);
            let c = frame
                .channels
                .iter()
                .find(|c| c.channel == channel)
                .ok_or_else(|| {
                    ExperimentError::new(
                        "AnalysisChannelAbsent",
                        "Detector channel was not captured",
                    )
                })?;
            let roi = if p["roi"].is_null() {
                SampleRange {
                    start: 0,
                    end: c.samples.len(),
                }
            } else {
                serde_json::from_value(p["roi"].clone())
                    .map_err(|e| ExperimentError::new("InvalidROI", e.to_string()))?
            };
            let request = AnalysisRequest {
                record_id: r.id.clone(),
                frame_id: frame.id.clone(),
                channel: channel.clone(),
                roi,
                method: if p["template_id"] == "q_scan" {
                    AnalysisMethod::SingleDrop
                } else {
                    AnalysisMethod::Fsr
                },
                excursion_pm: p["excursion_pm"].as_f64().unwrap_or(0.),
                ramp_hz: p["ramp_hz"].as_f64().unwrap_or(0.),
                wavelength_nm: p["wavelength_nm"].as_f64().unwrap_or(0.),
                scan_speed_nm_s: p["scan_speed_nm_s"].as_f64().unwrap_or(0.),
                prominence_fraction: p["prominence_fraction"].as_f64().unwrap_or(0.1),
                min_distance_samples: p["min_distance_samples"].as_u64().unwrap_or(1) as usize,
            };
            let result = yang_data::analyze_with_cancel(r, &request, &|| self.job.cancelled())
                .map_err(data_error)?;
            let id = result.id.clone();
            store.append_analysis(result).map_err(data_error)?;
            ids.push(id);
        }
        Ok(ids)
    }
    fn wait_interval(&mut self, millis: u64, cancel: &AtomicBool) -> Result<(), ExperimentError> {
        let end = Instant::now() + Duration::from_millis(millis);
        while Instant::now() < end {
            if cancel.load(Ordering::Acquire) {
                return Err(ExperimentError::new(
                    "Cancelled",
                    "Interval stopped at a confirmed boundary",
                ));
            }
            std::thread::sleep(
                end.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(25)),
            );
        }
        Ok(())
    }
}

fn preflight(
    definition: &ExperimentDefinition,
    snapshot: &Value,
) -> Result<BTreeMap<String, RoleInfo>, HostError> {
    yang_experiments::validate_definition(definition)
        .map_err(|e| HostError::new(&e.code, e.message))?;
    // Check the complete topology before inspecting/admitting any local role.
    if definition
        .roles
        .iter()
        .any(|b| Some(b.host_id.as_str()) != snapshot["host_id"].as_str())
    {
        return Err(HostError::new(
            "RemoteExperimentUnsupported",
            "Cross-Host experiments are not enabled; bind every role to this Host before Start",
        ));
    }
    let infos = definition
        .roles
        .iter()
        .map(|b| inspect_role(b, snapshot).map(|info| (b.role.clone(), info)))
        .collect::<Result<_, _>>()?;
    for binding in &definition.roles {
        validate_role_parameters(binding, definition, snapshot)?;
    }
    let bytes = frame_budget(definition, snapshot)?;
    if bytes
        .checked_mul(definition.parameters["frames"].as_u64().unwrap_or(1))
        .is_none_or(|n| n > 536_870_912)
    {
        return Err(HostError::new("CaptureBudget","All completed frames must fit the native512MiB record budget; reduce frames, depth or channels"));
    }
    Ok(infos)
}

fn validate_role_parameters(
    binding: &RoleBinding,
    definition: &ExperimentDefinition,
    snapshot: &Value,
) -> Result<(), HostError> {
    let status = &snapshot["domains"][domain(binding).key()]["device"];
    if binding.role == "scan_signal" {
        use yang_drivers::generator::{ChannelSignal, GeneratorModel, SignalConfig, Waveform};
        let device = snapshot["registry"]["devices"]
            .as_array()
            .and_then(|v| v.iter().find(|d| d["device_id"] == binding.domain_id))
            .ok_or_else(|| HostError::new("RoleUnavailable", "Generator device absent"))?;
        let model = match device["model_id"].as_str() {
            Some("dg4162") => GeneratorModel::Dg4162,
            Some("dg4202") => GeneratorModel::Dg4202,
            Some("dg922pro") => GeneratorModel::Dg922Pro,
            _ => {
                return Err(HostError::new(
                    "MissingCapability",
                    "Unreviewed generator model",
                ))
            }
        };
        if status["quality"] != "confirmed" || status["previous"] != false {
            return Err(HostError::new("PreflightReadbackRequired","Read the selected generator status explicitly before Start; historical or absent load/levels cannot admit a scan"));
        }
        let channel = status["channels"]
            .as_array()
            .and_then(|v| {
                v.iter()
                    .find(|c| c["channel"].as_u64() == binding.channel.map(u64::from))
            })
            .ok_or_else(|| {
                HostError::new(
                    "PreflightReadbackRequired",
                    "Selected generator channel load and levels have not been read",
                )
            })?;
        let current: ChannelSignal = serde_json::from_value(channel.clone()).map_err(|_| {
            HostError::new(
                "PreflightReadbackRequired",
                "Selected generator readback is incomplete or malformed",
            )
        })?;
        let p = &definition.parameters;
        let requested = SignalConfig {
            waveform: Waveform::Triangle,
            frequency_hz: p["frequency_hz"].as_f64().unwrap_or(f64::NAN),
            amplitude_vpp: p["amplitude_vpp"].as_f64().unwrap_or(f64::NAN),
            offset_v: p["offset_v"].as_f64().unwrap_or(f64::NAN),
        };
        model
            .validate_signal_transition(&requested, &current)
            .map_err(|e| HostError::new("InvalidExperimentSignal", e.to_string()))?;
    }
    if binding.role == "laser" && !definition.parameters["coarse_wavelength_nm"].is_null() {
        let requested = definition.parameters["coarse_wavelength_nm"]
            .as_f64()
            .unwrap_or(f64::NAN);
        let range = status["wavelength_range_nm"]
            .as_array()
            .filter(|v| v.len() == 2)
            .and_then(|v| v[0].as_f64().zip(v[1].as_f64()))
            .filter(|(a, b)| a.is_finite() && b.is_finite() && a < b)
            .ok_or_else(|| {
                HostError::new(
                    "PreflightReadbackRequired",
                    "Read the laser's verified wavelength range before Start",
                )
            })?;
        if !(range.0..=range.1).contains(&requested) {
            return Err(HostError::new(
                "InvalidExperimentWavelength",
                "Coarse wavelength is outside the selected laser's verified range",
            ));
        }
    }
    Ok(())
}

fn frame_budget(definition: &ExperimentDefinition, snapshot: &Value) -> Result<u64, HostError> {
    let detector = definition
        .roles
        .iter()
        .find(|b| b.role == "detector")
        .ok_or_else(|| HostError::new("RoleUnavailable", "Detector role absent"))?;
    let device = snapshot["registry"]["devices"]
        .as_array()
        .and_then(|v| v.iter().find(|d| d["device_id"] == detector.domain_id))
        .ok_or_else(|| HostError::new("RoleUnavailable", "Detector device absent"))?;
    let memory = definition.parameters["source"] == "memory";
    let requested = definition.parameters["memory_depth"].as_u64();
    if memory {
        if let Some(points) = requested {
            let model = match device["model_id"].as_str() {
                Some("ds7054") => yang_drivers::scope::ScopeModel::Ds7054,
                Some("mho5054") => yang_drivers::scope::ScopeModel::Mho5054,
                _ => return Err(HostError::new("MissingCapability", "Scope model is not reviewed")),
            };
            yang_drivers::scope::validate_memory_depth(model, points).map_err(|_| {
                HostError::new("MemoryDepthUnsupported", "Select a reviewed memory-depth value for the selected scope")
            })?;
        }
    }
    let points = if memory {
        requested
            .or_else(|| {
                snapshot["domains"][domain(detector).key()]["device"]["memory_depth"].as_u64()
            })
            .filter(|n| *n > 0)
            .unwrap_or(250_000_000)
    } else {
        1000
    };
    let channels = definition.parameters["channels"]
        .as_array()
        .map_or(0, |v| v.len()) as u64;
    if channels == 0
        || definition.parameters["channels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_u64().is_none_or(|n| n > 4))
    {
        return Err(HostError::new(
            "MissingCapability",
            "Scope capture channels must be1..4",
        ));
    }
    if definition.template_id != "waveform_capture"
        && !definition.parameters["channels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_u64() == detector.channel.map(u64::from))
    {
        return Err(HostError::new(
            "AnalysisChannelAbsent",
            "Selected detector channel must be included in capture channels",
        ));
    }
    let width = if device["model_id"] == "mho5054" {
        2
    } else {
        1
    };
    points
        .checked_mul(width)
        .and_then(|n| n.checked_add(256))
        .and_then(|n| n.checked_mul(channels))
        .ok_or_else(|| HostError::new("CaptureBudget", "Run byte budget overflow"))
}

fn inspect_role(binding: &RoleBinding, snapshot: &Value) -> Result<RoleInfo, HostError> {
    if snapshot["mode"] != "real"
        || snapshot["host_status"] != "ONLINE"
        || !snapshot["startup_error"].is_null()
    {
        return Err(HostError::new(
            "RoleUnavailable",
            "The real Host is not ready",
        ));
    }
    let device = snapshot["registry"]["devices"]
        .as_array()
        .and_then(|v| {
            v.iter()
                .find(|d| Some(binding.domain_id.as_str()) == d["device_id"].as_str())
        })
        .ok_or_else(|| {
            HostError::new(
                "RoleUnavailable",
                format!("{} device is not registered", binding.role),
            )
        })?;
    let model = super::super::catalog::Catalog::load(super::super::catalog::DOCUMENT)?;
    let model = model.model(device["model_id"].as_str().unwrap_or(""))?;
    let expected = match binding.role.as_str() {
        "detector" => "scope",
        "scan_signal" => "generator",
        "laser" => "laser",
        _ => {
            return Err(HostError::new(
                "MissingCapability",
                "Unknown experiment role",
            ))
        }
    };
    let reviewed = match model.id.as_str() {
        "ds7054" | "mho5054" => "scope",
        "dg4162" | "dg4202" | "dg922pro" => "generator",
        "tlb6700" => "laser",
        _ => "unsupported",
    };
    if model.driver_kind != expected
        || reviewed != expected
        || binding
            .channel
            .is_some_and(|c| c > if expected == "scope" { 4 } else { 2 })
    {
        return Err(HostError::new(
            "MissingCapability",
            format!(
                "{} requires a reviewed {expected} model/channel",
                binding.role
            ),
        ));
    }
    let domain = &snapshot["domains"][format!("device:{}", binding.domain_id)];
    let status = &domain["device"];
    if device["verified_mode"] != "real"
        || device["config_rev"].as_u64().unwrap_or(0) == 0
        || domain["state"] != "READY"
        || domain["pending"].as_u64().unwrap_or(0) != 0
        || domain["communication"] == "UNKNOWN"
        || domain["context"]["connection_id"].is_null()
        || status["connected"] != true
        || status["state"] != "READY"
    {
        return Err(HostError::new("RoleUnavailable",format!("{} must already be connected and Ready; connect it explicitly in Devices before Start",binding.role)));
    }
    let identity = &status["identity"];
    let expected_identity = device["expected_identity"]
        .as_object()
        .filter(|o| !o.is_empty())
        .ok_or_else(|| HostError::new("IdentityMismatch", "Saved verified identity is absent"))?;
    if !expected_identity
        .iter()
        .all(|(k, v)| identity.get(k) == Some(v))
    {
        return Err(HostError::new(
            "IdentityMismatch",
            format!(
                "{} current identity differs from saved verification",
                binding.role
            ),
        ));
    }
    let capabilities: Vec<String> = match expected {
        "scope" if model.operations.iter().any(|n| n == "capture") => {
            vec!["capture_screen".into(), "capture_memory".into()]
        }
        "generator"
            if ["configure_signal", "set_output"]
                .iter()
                .all(|n| model.operations.iter().any(|v| v == n)) =>
        {
            vec![
                "triangle".into(),
                "configure_signal".into(),
                "set_output".into(),
            ]
        }
        "laser" if model.operations.iter().any(|n| n == "goto_wavelength") => {
            vec!["tune_wavelength".into()]
        }
        _ => vec![],
    };
    if capabilities.is_empty() {
        return Err(HostError::new(
            "MissingCapability",
            format!(
                "{} required operation is absent from the trusted catalog",
                binding.role
            ),
        ));
    }
    Ok(RoleInfo {
        family: expected.into(),
        identity: identity.clone(),
        config_rev: device["config_rev"].as_u64().unwrap(),
        connected: true,
        capabilities,
    })
}

#[cfg(test)]
#[path = "experiment_port_tests.rs"]
mod tests;
