use super::*;

#[test]
fn manual_capture_is_volatile_until_explicit_save_and_keeps_stable_identity() {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
        let fixture = super::super::native_osa_flow_tests::Fixture::rigol(true);
        let draft = fixture.rpc("create_draft", json!({"name":"Manual preview","model_id":"ds7054","profile_id":"visa","params":{"resource":"GPIB0::7::INSTR"},"expected_rev":0}), false).await;
        let domain = DomainRef { kind: "device".into(), id: draft["device_id"].as_str().unwrap().into() };
        let lease = fixture.rpc("acquire_control", json!({"domain":domain}), false).await;
        let proof = fixture.rpc("test_connection", json!({"draft_id":domain.id,"expected_rev":1,"consent":{"accepted":true,"mode":"real","config_digest":draft["config_digest"],"open_effects":[],"supervised":false,"retain_session":false},"lease_token":lease["token"],"control_epoch":lease["control_epoch"],"request_id":new_id().unwrap(),"sequence":1}), false).await;
        fixture.rpc("save_device", json!({"draft_id":domain.id,"proof_id":proof["proof_id"],"expected_rev":1}), false).await;
        device_operation(&fixture, &domain, &lease, None, 2).await;
        device_operation(&fixture, &domain, &lease, Some("read_status"), 3).await;
        let mut d = definition();
        d.roles[0].host_id = fixture.core().registry.lock().unwrap().host_id.clone();
        d.roles[0].domain_id = domain.id;
        d.parameters["acquisition"] = json!("stopped");
        let accepted = fixture.rpc("capture_start", json!({"definition":d}), false).await;
        let run_id = accepted["run_id"].as_str().unwrap();
        let status = settled_experiment_status(&fixture, run_id).await;
        assert_eq!(status["status"], "completed", "{status}");
        let id = status["record_ids"][0].as_str().unwrap();
        let w = fixture.core().workbench.as_ref().unwrap();
        assert!(w.store.list().unwrap().is_empty(), "Acquire must not save a Record");
        assert_eq!(w.runs().unwrap(), json!([]), "Acquire must not persist a RunRecord");
        let journal: Value = serde_json::from_slice(&std::fs::read(w.store.root().parent().unwrap().join("jobs.json")).unwrap()).unwrap();
        assert!(!journal.as_array().unwrap().iter().any(|j| j["id"] == run_id), "No recovered manual preview job after restart");
        let preview = fixture.rpc("data_get", json!({"record_id":id}), false).await;
        assert_eq!(preview["saved"], false);
        let frame = preview["record"]["frames"][0]["id"].clone();
        let viewport = fixture.rpc("data_viewport", json!({"record_id":id,"frame_id":frame,"channel":"CH1","start":0,"end":1000,"max_points":32}), false).await;
        assert!(viewport["points"].as_array().unwrap().len() <= 32);
        let saved = fixture.rpc("preview_save", json!({"record_id":id,"name":"Saved trace","comment":"first"}), false).await;
        assert_eq!(saved["record_id"], id);
        assert_eq!(w.store.load(id).unwrap().frames[0].id, frame.as_str().unwrap());
        fixture.rpc("preview_save", json!({"record_id":id,"name":"Renamed trace","comment":"edited"}), false).await;
        assert_eq!(w.store.list().unwrap().len(), 1);
        let record = w.store.load(id).unwrap();
        assert_eq!(record.name, "Renamed trace");
        assert_eq!(record.comment, "edited");
        assert_eq!(record.source["kind"], "manual");
        assert_eq!(record.frames[0].channels[0].samples.len(), 1000);
        fixture.rpc("preview_discard", json!({"record_id":id}), false).await;
        assert_eq!(w.store.list().unwrap().len(), 1, "Discard preview must preserve saved Record");
    });
}

#[test]
fn historical_status_scope_cannot_read_another_clients_manual_capture_result() {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
        let fixture = super::super::native_osa_flow_tests::Fixture::rigol(true);
        let w = fixture.core().workbench.as_ref().unwrap();
        let job = w.submit(fixture.local.session.id(), "capture", |_| Ok(json!({"run":{"records":["a".repeat(32)],"definition":{"name":"private unsaved capture"}}}))).unwrap();
        let id = job["job_id"].as_str().unwrap();
        settled_experiment_status(&fixture, id).await;
        let request = HostRequest { v: 1, id: new_id().unwrap(), method: "analysis_status".into(), params: json!({"job_id":id}) };
        let result = fixture.core().dispatch(&request, "different-paired-client").await;
        assert!(result.is_err(), "Historical status access exposed a different client's volatile capture");
        assert!(fixture.core().dispatch(&request, fixture.local.session.id()).await.is_ok());
    });
}

#[test]
fn preview_publication_failure_cannot_downgrade_unknown_hardware_outcome() {
    let mut run = yang_experiments::RunRecord { schema: 1, id: new_id().unwrap(), experiment_id: "e".repeat(32),
        definition: definition(), bindings: vec![], status: RunStatus::Uncertain, stage: "uncertain".into(),
        completed_units: 1, total_units: 1, started_at_unix_ms: 1, finished_at_unix_ms: Some(2), records: vec![],
        analyses: vec![], steps: vec![], error: Some(ExperimentError::new("OutcomeUnknown", "lease cleanup unconfirmed")), cleanup_errors: vec![] };
    preview_publication_failure(&mut run, HostError::new("ControlDenied", "preview owner departed"));
    assert_eq!(run.status, RunStatus::Uncertain, "Storage/session failure must not convert unknown hardware outcome to definite failure");
    assert_eq!(run.stage, "uncertain");
    assert_eq!(run.error.unwrap().code, "ControlDenied");
}

#[test]
fn safety_context_adoption_never_rebinds_a_new_physical_session() {
    let before = json!({"session_id":"a".repeat(32),"domain":{"kind":"device","id":"b".repeat(32)},"connection_id":"c".repeat(32),"epoch":1});
    let mut after = before.clone();
    after["epoch"] = json!(2);
    assert_eq!(owned_safety_context(&before, &after).unwrap(), after);
    for (key, value) in [
        ("session_id", json!("d".repeat(32))),
        ("connection_id", json!("d".repeat(32))),
        ("domain", json!({"kind":"device","id":"d".repeat(32)})),
        ("epoch", json!(1)),
    ] {
        let mut bad = after.clone();
        bad[key] = value;
        assert!(owned_safety_context(&before, &bad).is_err(), "{key}");
    }
}

async fn device_operation(
    fixture: &super::super::native_osa_flow_tests::Fixture,
    domain: &DomainRef,
    lease: &Value,
    name: Option<&str>,
    sequence: u64,
) -> Value {
    let context = fixture.core().worker.domain_context(&domain.key()).unwrap();
    let mut intent = json!({"domain":domain,"lease_token":lease["token"],"control_epoch":lease["control_epoch"],"config_rev":1,"context":context,"method":if name.is_some(){"action"}else{"connect"},"params":name.map(|n|json!({"name":n,"args":{}})).unwrap_or(json!({})),"sequence":sequence,"confirmation":null});
    intent["confirmation"] = fixture
        .rpc("prepare", json!({"intent":intent}), false)
        .await["token"]
        .clone();
    let id = new_id().unwrap();
    let mut operation = fixture
        .rpc("execute", json!({"request_id":id,"intent":intent}), false)
        .await;
    let end = Instant::now() + Duration::from_secs(20);
    while operation["status"] != "Terminal" {
        assert!(Instant::now() < end, "{operation}");
        tokio::time::sleep(Duration::from_millis(10)).await;
        operation = fixture
            .rpc("operation", json!({"request_id":id}), false)
            .await;
    }
    assert_eq!(operation["phase"], "completed", "{operation}");
    operation
}

#[test]
fn real_host_journal_worker_driver_capture_commits_two_fresh_grouped_frames_and_keeps_borrowed_lease(
) {
    assert_two_fresh_grouped_frames("memory");
}

#[test]
fn stopped_scope_screen_collect_acquires_two_new_confirmed_frames() {
    assert_two_fresh_grouped_frames("screen");
}

#[test]
fn existing_screen_read_commits_waveform_without_single_or_depth_change() {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
        let fixture = super::super::native_osa_flow_tests::Fixture::rigol(true);
        let draft = fixture.rpc("create_draft", json!({"name":"Screen read","model_id":"ds7054","profile_id":"visa","params":{"resource":"GPIB0::7::INSTR"},"expected_rev":0}), false).await;
        let domain = DomainRef { kind: "device".into(), id: draft["device_id"].as_str().unwrap().into() };
        let lease = fixture.rpc("acquire_control", json!({"domain":domain}), false).await;
        let proof = fixture.rpc("test_connection", json!({"draft_id":domain.id,"expected_rev":1,"consent":{"accepted":true,"mode":"real","config_digest":draft["config_digest"],"open_effects":[],"supervised":false,"retain_session":false},"lease_token":lease["token"],"control_epoch":lease["control_epoch"],"request_id":new_id().unwrap(),"sequence":1}), false).await;
        fixture.rpc("save_device", json!({"draft_id":domain.id,"proof_id":proof["proof_id"],"expected_rev":1}), false).await;
        device_operation(&fixture, &domain, &lease, None, 2).await;
        device_operation(&fixture, &domain, &lease, Some("read_status"), 3).await;
        let mut d = definition();
        d.roles[0].host_id = fixture.core().registry.lock().unwrap().host_id.clone();
        d.roles[0].domain_id = domain.id.clone();
        d.parameters["source"] = json!("screen");
        d.parameters["memory_depth"] = Value::Null;
        d.parameters["acquisition"] = json!("existing_screen");
        let accepted = fixture.rpc("experiment_start", json!({"definition":d}), false).await;
        let status = settled_experiment_status(&fixture, accepted["run_id"].as_str().unwrap()).await;
        assert_eq!(status["status"], "completed", "{status}");
        let record = fixture.core().workbench.as_ref().unwrap().store.load(status["record_ids"][0].as_str().unwrap()).unwrap();
        assert_eq!(record.frames.len(), 1);
        assert_eq!(record.frames[0].channels[0].samples.len(), 1000);
        assert!(record.frames[0].channels[0].metadata["fresh_acquisition"].is_null(), "Existing screen must not be relabeled as new trigger");
        assert_eq!(record.source["parameters"]["acquisition"], "existing_screen");
        let journal_path = fixture.core().workbench.as_ref().unwrap().store.root().parent().unwrap().parent().unwrap().join("operations.json");
        let journal: Value = serde_json::from_slice(&std::fs::read(journal_path).unwrap()).unwrap();
        let entries = journal["records"].as_array().unwrap();
        let captures: Vec<_> = entries.iter().filter(|r| r["command"]["params"]["name"] == "capture").collect();
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0]["command"]["params"]["args"]["fresh"], false);
        assert!(!entries.iter().any(|r| matches!(r["command"]["params"]["name"].as_str(), Some("set_memory_depth" | "run" | "stop"))));
    });
}

async fn settled_experiment_status(
    fixture: &super::super::native_osa_flow_tests::Fixture,
    run_id: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = fixture
            .rpc("experiment_status", json!({"run_id":run_id}), false)
            .await;
        if !matches!(status["status"].as_str(), Some("queued" | "running")) {
            return status;
        }
        assert!(Instant::now() < deadline, "{status}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn stopped_buffer_cycles_use_capture_then_run_and_preserve_grouped_stop_evidence() {
    for resume in [false, true] {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
            let fixture = super::super::native_osa_flow_tests::Fixture::rigol(true);
            let draft = fixture.rpc("create_draft", json!({"name":"Stopped buffer","model_id":"ds7054","profile_id":"visa","params":{"resource":"GPIB0::7::INSTR"},"expected_rev":0}), false).await;
            let domain = DomainRef { kind: "device".into(), id: draft["device_id"].as_str().unwrap().into() };
            let lease = fixture.rpc("acquire_control", json!({"domain":domain}), false).await;
            let proof = fixture.rpc("test_connection", json!({"draft_id":domain.id,"expected_rev":1,"consent":{"accepted":true,"mode":"real","config_digest":draft["config_digest"],"open_effects":[],"supervised":false,"retain_session":false},"lease_token":lease["token"],"control_epoch":lease["control_epoch"],"request_id":new_id().unwrap(),"sequence":1}), false).await;
            fixture.rpc("save_device", json!({"draft_id":domain.id,"proof_id":proof["proof_id"],"expected_rev":1}), false).await;
            device_operation(&fixture, &domain, &lease, None, 2).await;
            device_operation(&fixture, &domain, &lease, Some("read_status"), 3).await;
            let mut d = definition();
            d.roles[0].host_id = fixture.core().registry.lock().unwrap().host_id.clone();
            d.roles[0].domain_id = domain.id.clone();
            d.parameters["frames"] = json!(2);
            d.parameters["acquisition"] = json!("stopped");
            d.parameters["resume_after_capture"] = json!(resume);
            let accepted = fixture.rpc("experiment_start", json!({"definition":d}), false).await;
            let status = settled_experiment_status(&fixture, accepted["run_id"].as_str().unwrap()).await;
            assert_eq!(status["status"], "completed", "{status}");
            assert_eq!(status["result"]["run"]["completed_units"], 2);
            let store = &fixture.core().workbench.as_ref().unwrap().store;
            let record = store.load(status["record_ids"][0].as_str().unwrap()).unwrap();
            assert_eq!(record.frames.len(), 2);
            for frame in &record.frames {
                assert_eq!(frame.channels[0].samples.len(), 1000);
                assert_eq!(frame.channels[0].metadata["snapshot"]["confirmed"], true);
                assert_eq!(frame.channels[0].metadata["snapshot"]["method"], "stop_buffer");
                assert!(frame.channels[0].metadata["fresh_acquisition"].is_null());
            }
            let journal_path = store.root().parent().unwrap().parent().unwrap().join("operations.json");
            let journal: Value = serde_json::from_slice(&std::fs::read(journal_path).unwrap()).unwrap();
            let mut actions: Vec<_> = journal["records"].as_array().unwrap().iter()
                .filter(|r| matches!(r["command"]["params"]["name"].as_str(), Some("capture" | "run" | "stop" | "set_memory_depth"))).collect();
            actions.sort_by_key(|r| r["sequence"].as_u64().unwrap());
            let names: Vec<_> = actions.iter().map(|r| r["command"]["params"]["name"].as_str().unwrap()).collect();
            let expected = if resume { vec!["set_memory_depth", "capture", "run", "capture", "run"] } else { vec!["set_memory_depth", "capture", "run", "capture"] };
            assert_eq!(names, expected);
            for capture in actions.iter().filter(|r| r["command"]["params"]["name"] == "capture") {
                assert_eq!(capture["command"]["params"]["args"]["fresh"], false);
                assert_eq!(capture["command"]["params"]["args"]["stop_before"], true);
                assert_eq!(capture["command"]["params"]["args"]["points"], 1000);
            }
            device_operation(&fixture, &domain, &lease, Some("read_status"), 4).await;
            fixture.core().worker_cache_with_refresh(true).await.unwrap();
            let snapshot = fixture.core().snapshot().unwrap();
            assert_eq!(snapshot["domains"][domain.key()]["state"], "READY");
            assert_eq!(snapshot["domains"][domain.key()]["device"]["acquisition"], if resume { "running" } else { "stopped" });
        });
    }
}

#[test]
fn cancellation_during_run_metadata_boundary_keeps_saved_frame_without_run_submission() {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
        let fixture = super::super::native_osa_flow_tests::Fixture::rigol(true);
        let core = fixture.core().clone();
        let draft = fixture.rpc("create_draft", json!({"name":"Cancel boundary","model_id":"ds7054","profile_id":"visa","params":{"resource":"GPIB0::7::INSTR"},"expected_rev":0}), false).await;
        let domain = DomainRef { kind: "device".into(), id: draft["device_id"].as_str().unwrap().into() };
        let lease = fixture.rpc("acquire_control", json!({"domain":domain}), false).await;
        let proof = fixture.rpc("test_connection", json!({"draft_id":domain.id,"expected_rev":1,"consent":{"accepted":true,"mode":"real","config_digest":draft["config_digest"],"open_effects":[],"supervised":false,"retain_session":false},"lease_token":lease["token"],"control_epoch":lease["control_epoch"],"request_id":new_id().unwrap(),"sequence":1}), false).await;
        fixture.rpc("save_device", json!({"draft_id":domain.id,"proof_id":proof["proof_id"],"expected_rev":1}), false).await;
        device_operation(&fixture, &domain, &lease, None, 2).await;
        device_operation(&fixture, &domain, &lease, Some("read_status"), 3).await;
        core.worker_cache_with_refresh(true).await.unwrap();
        let binding = RoleBinding { role: "detector".into(), host_id: core.registry.lock().unwrap().host_id.clone(), domain_id: domain.id.clone(), channel: Some(1) };
        let info = inspect_role(&binding, &core.snapshot().unwrap()).unwrap();
        let context = core.worker.domain_context(&domain.key()).unwrap();
        let workbench = core.workbench.as_ref().unwrap().clone();
        let handle = tokio::runtime::Handle::current();
        let owner = fixture.local.session.clone();
        let journal = Session::local(new_id().unwrap(), core.boot_id.clone()).unwrap();
        let owner_id = owner.id().to_owned();
        let task_core = core.clone();
        let (committed_tx, committed_rx) = std::sync::mpsc::sync_channel(1);
        let (run_tx, run_rx) = std::sync::mpsc::sync_channel(1);
        let accepted = workbench.submit(&owner_id, "experiment", move |ctx| {
            let mut definition = definition();
            definition.parameters["acquisition"] = json!("stopped");
            let parameters = definition.parameters.clone();
            let mut port = HostExperimentPort { core: task_core, handle, owner, journal, definition, baseline: BTreeMap::from([("detector".into(), info)]), contexts: BTreeMap::from([("detector".into(), context)]), sequence: 0, job: &ctx, held: Arc::new(Mutex::new(BTreeMap::new())), pending: None, record_id: None, raw_bytes: 0, frame_budget: 1256, depth_configured: false, prepared_generators: BTreeSet::new(), manual: false, preview: None, completed_preview: None };
            let adapt = |e: ExperimentError| HostError::new(&e.code, e.message);
            port.reserve(&binding).map_err(adapt)?;
            let mut data = record();
            data.frames.clear();
            port.begin_record(&data).map_err(adapt)?;
            let frame = port.capture(&binding, &parameters, ctx.cancel_flag()).map_err(adapt)?;
            data.frames.push(frame);
            port.frame_completed(&data).map_err(adapt)?;
            committed_tx.send(data.id.clone()).unwrap();
            run_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let outcome = port.action(&binding, "run", &json!({}));
            data.source["run_status"] = json!(if outcome.as_ref().err().is_some_and(|e| e.code == "Cancelled") { "cancelled" } else { "completed" });
            port.persist(&data).map_err(adapt)?;
            port.release(&binding).map_err(adapt)?;
            outcome.map(|_| json!({"record_id":data.id})).map_err(adapt)
        }).unwrap();
        let record_id = committed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(workbench.store.load(&record_id).unwrap().frames.len(), 1);
        let before = core.status_cache.lock().unwrap().as_ref().unwrap().0;
        // Inject a finite delay into current()'s actual snapshot boundary. The
        // status refresh below proves RUN's initial cancellation check passed;
        // snapshot cannot return while this lease snapshot mutex is retained.
        let held_snapshot = core.leases.lock().unwrap();
        run_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if core.status_cache.lock().unwrap().as_ref().unwrap().0 > before { break; }
            assert!(Instant::now() < deadline, "RUN metadata did not reach the injected boundary");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let job_id = accepted["job_id"].as_str().unwrap();
        workbench.cancel(job_id, fixture.local.session.id()).unwrap();
        drop(held_snapshot);
        let status = settled_experiment_status(&fixture, job_id).await;
        assert_eq!(status["status"], "cancelled", "{status}");
        assert_eq!(status["error"]["code"], "Cancelled");
        let saved = workbench.store.load(&record_id).unwrap();
        assert_eq!(saved.frames.len(), 1);
        assert_eq!(saved.source["run_status"], "cancelled");
        let path = workbench.store.root().parent().unwrap().parent().unwrap().join("operations.json");
        let journal: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(!journal["records"].as_array().unwrap().iter().any(|r| r["command"]["params"]["name"] == "run"), "Cancelled RUN must never enter the execution journal");
    });
}

#[test]
fn experiment_status_exposes_driver_failure_after_partial_capture() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let fixture = super::super::native_osa_flow_tests::Fixture::rigol(true);
            let draft = fixture
                .rpc("create_draft", json!({"name":"Finite scope","model_id":"ds7054","profile_id":"visa","params":{"resource":"GPIB0::7::INSTR"},"expected_rev":0}), false)
                .await;
            let domain = DomainRef {
                kind: "device".into(),
                id: draft["device_id"].as_str().unwrap().into(),
            };
            let lease = fixture
                .rpc("acquire_control", json!({"domain":domain}), false)
                .await;
            let proof = fixture
                .rpc("test_connection", json!({"draft_id":domain.id,"expected_rev":1,"consent":{"accepted":true,"mode":"real","config_digest":draft["config_digest"],"open_effects":[],"supervised":false,"retain_session":false},"lease_token":lease["token"],"control_epoch":lease["control_epoch"],"request_id":new_id().unwrap(),"sequence":1}), false)
                .await;
            fixture
                .rpc("save_device", json!({"draft_id":domain.id,"proof_id":proof["proof_id"],"expected_rev":1}), false)
                .await;
            device_operation(&fixture, &domain, &lease, None, 2).await;
            device_operation(&fixture, &domain, &lease, Some("read_status"), 3).await;
            let mut d = definition();
            d.roles[0].host_id = fixture.core().registry.lock().unwrap().host_id.clone();
            d.roles[0].domain_id = domain.id;
            d.parameters["source"] = json!("screen");
            // The bounded transport admits four complete frames, then fails the
            // next Single write. The real runner must retain its error and data.
            d.parameters["frames"] = json!(5);
            let accepted = fixture
                .rpc("experiment_start", json!({"definition":d}), false)
                .await;
            let status = settled_experiment_status(&fixture, accepted["run_id"].as_str().unwrap()).await;
            assert_eq!(status["status"], "uncertain", "{status}");
            assert_eq!(status["result"]["run"]["completed_units"], 4);
            assert_eq!(status["error"]["code"], "OutcomeUnknown", "{status}");
            assert!(status["error"]["message"]
                .as_str()
                .unwrap()
                .contains("failed_after_call_started"));
            assert_eq!(status["error"], status["result"]["run"]["error"]);
            assert_eq!(status["cleanup_errors"], json!([]));
            let record_id = status["record_ids"][0].as_str().unwrap();
            let record = fixture.core().workbench.as_ref().unwrap().store.load(record_id).unwrap();
            assert_eq!(record.frames.len(), 4);
            assert_eq!(record.source["run_status"], "uncertain");
        });
}

#[test]
fn experiment_status_exposes_persisted_nested_run_error_and_cleanup_evidence() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let fixture = super::super::native_osa_flow_tests::Fixture::rigol(true);
            let w = fixture.core().workbench.as_ref().unwrap();
            let run = yang_experiments::RunRecord {
                schema: 1,
                id: "1".repeat(32),
                experiment_id: "e".repeat(32),
                definition: definition(),
                bindings: vec![],
                status: RunStatus::Uncertain,
                stage: "uncertain".into(),
                completed_units: 0,
                total_units: 1,
                started_at_unix_ms: 100,
                finished_at_unix_ms: Some(10100),
                records: vec![],
                analyses: vec![],
                steps: vec![],
                error: Some(ExperimentError::new(
                    "OutcomeUnknown",
                    "failed_after_call_started: single acquisition; state uncertain, no retry",
                )),
                cleanup_errors: vec![ExperimentError::new(
                    "CleanupRetained",
                    "Lease release remains unconfirmed",
                )],
            };
            let evidence = serde_json::to_value(run).unwrap();
            let saved_evidence = evidence.clone();
            let accepted = w
                .submit(fixture.local.session.id(), "experiment", move |_| {
                    Ok(json!({"run":saved_evidence}))
                })
                .unwrap();
            let id = accepted["id"].as_str().unwrap();
            let status = settled_experiment_status(&fixture, id).await;
            // Existing journals contain a null job error and all original
            // failure evidence only in result.run. Reopening preserves that form.
            let reloaded = super::super::super::workbench::Workbench::open(w.store.root().parent().unwrap()).unwrap();
            let raw = reloaded.status(id, fixture.local.session.id()).unwrap();
            assert!(raw["error"].is_null());
            assert_eq!(raw["result"]["run"], evidence);
            assert_eq!(raw, w.status(id, fixture.local.session.id()).unwrap());
            assert_eq!(status["error"]["code"], "OutcomeUnknown", "{status}");
            assert_eq!(status["error"]["message"], "failed_after_call_started: single acquisition; state uncertain, no retry");
            assert_eq!(status["cleanup_errors"][0]["code"], "CleanupRetained");
            assert_eq!(status["cleanup_errors"][0]["message"], "Lease release remains unconfirmed");
            assert_eq!(status["result"]["run"], evidence);
        });
}

fn assert_two_fresh_grouped_frames(source: &str) {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async{
        let fixture=super::super::native_osa_flow_tests::Fixture::rigol(true);
        let draft=fixture.rpc("create_draft",json!({"name":"Finite scope","model_id":"ds7054","profile_id":"visa","params":{"resource":"GPIB0::7::INSTR"},"expected_rev":0}),false).await;
        let domain=DomainRef{kind:"device".into(),id:draft["device_id"].as_str().unwrap().into()};
        let lease=fixture.rpc("acquire_control",json!({"domain":domain}),false).await;
        let proof=fixture.rpc("test_connection",json!({"draft_id":domain.id,"expected_rev":1,"consent":{"accepted":true,"mode":"real","config_digest":draft["config_digest"],"open_effects":[],"supervised":false,"retain_session":false},"lease_token":lease["token"],"control_epoch":lease["control_epoch"],"request_id":new_id().unwrap(),"sequence":1}),false).await;
        fixture.rpc("save_device",json!({"draft_id":domain.id,"proof_id":proof["proof_id"],"expected_rev":1}),false).await;
        device_operation(&fixture,&domain,&lease,None,2).await;device_operation(&fixture,&domain,&lease,Some("read_status"),3).await;
        let mut d=definition();d.roles[0].host_id=fixture.core().registry.lock().unwrap().host_id.clone();d.roles[0].domain_id=domain.id.clone();d.parameters["frames"]=json!(2);d.parameters["source"]=json!(source);
        let accepted=fixture.rpc("experiment_start",json!({"definition":d}),false).await;let run_id=accepted["run_id"].as_str().unwrap();
        let end=Instant::now()+Duration::from_secs(20);let status=loop{
            let status=fixture.rpc("experiment_status",json!({"run_id":run_id}),false).await;
            if !matches!(status["status"].as_str(),Some("running"|"queued")){break status;}
            assert!(Instant::now()<end,"{status}");tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(status["status"],"completed","{status}");assert_eq!(status["result"]["run"]["completed_units"],2);
        let id=status["record_ids"][0].as_str().unwrap();let record=fixture.core().workbench.as_ref().unwrap().store.load(id).unwrap();
        assert_eq!(record.frames.len(),2);assert_eq!(record.frames[0].channels[0].samples.value(0),Some(128.));assert_eq!(record.frames[1].channels[0].samples.value(0),Some(129.));
        for frame in &record.frames {assert_eq!(frame.channels[0].samples.len(),1000);assert_eq!(frame.channels[0].samples.value(999),Some(255.));assert_eq!(frame.channels[0].metadata["fresh_acquisition"]["confirmed"],true);assert_eq!(frame.channels[0].metadata["source"],source);}
        assert!(fixture.core().worker.domain_context(&domain.key()).unwrap()["connection_id"].is_string());
        let borrowed=fixture.core().leases.lock().unwrap().for_session(&fixture.local.session,&domain,Instant::now()).unwrap().unwrap();assert_eq!(borrowed.token,lease["token"]);
        assert!(fixture.core().experiment_domains.lock().unwrap().is_empty());
    });
}

#[test]
fn real_generator_initial_off_resumes_only_its_own_fenced_context_then_final_off_holds() {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async{
        let fixture=super::super::native_osa_flow_tests::Fixture::rigol(true);let core=fixture.core().clone();
        let draft=fixture.rpc("create_draft",json!({"name":"Finite generator","model_id":"dg4162","profile_id":"visa","params":{"resource":"GPIB0::8::INSTR"},"expected_rev":0}),false).await;
        let domain=DomainRef{kind:"device".into(),id:draft["device_id"].as_str().unwrap().into()};let lease=fixture.rpc("acquire_control",json!({"domain":domain}),false).await;
        let proof=fixture.rpc("test_connection",json!({"draft_id":domain.id,"expected_rev":1,"consent":{"accepted":true,"mode":"real","config_digest":draft["config_digest"],"open_effects":[],"supervised":false,"retain_session":false},"lease_token":lease["token"],"control_epoch":lease["control_epoch"],"request_id":new_id().unwrap(),"sequence":1}),false).await;
        fixture.rpc("save_device",json!({"draft_id":domain.id,"proof_id":proof["proof_id"],"expected_rev":1}),false).await;
        device_operation(&fixture,&domain,&lease,None,2).await;device_operation(&fixture,&domain,&lease,Some("read_status"),3).await;core.worker_cache_with_refresh(true).await.unwrap();
        let binding=RoleBinding{role:"scan_signal".into(),host_id:core.registry.lock().unwrap().host_id.clone(),domain_id:domain.id.clone(),channel:Some(1)};
        let snapshot=core.snapshot().unwrap();let info=inspect_role(&binding,&snapshot).unwrap();let context=core.worker.domain_context(&domain.key()).unwrap();let before=context.clone();
        let workbench=core.workbench.as_ref().unwrap().clone();let handle=tokio::runtime::Handle::current();let owner=fixture.local.session.clone();let journal=Session::local(new_id().unwrap(),core.boot_id.clone()).unwrap();
        let journal_id=journal.id().to_owned();let owner_id=owner.id().to_owned();let task_core=core.clone();let accepted=workbench.submit(&owner_id,"experiment",move|ctx|{
            let mut port=HostExperimentPort{core:task_core,handle,owner,journal,definition:definition(),baseline:BTreeMap::from([("scan_signal".into(),info)]),contexts:BTreeMap::from([("scan_signal".into(),context)]),sequence:0,job:&ctx,held:Arc::new(Mutex::new(BTreeMap::new())),pending:None,record_id:None,raw_bytes:0,frame_budget:1000,depth_configured:false,prepared_generators:BTreeSet::new(),manual:false,preview:None,completed_preview:None};
            port.reserve(&binding).map_err(|e|HostError::new(&e.code,e.message))?;
            let outcome=(||{port.action(&binding,"set_output",&json!({"channel":1,"enabled":false}))?;
                port.action(&binding,"configure_signal",&json!({"channel":1,"waveform":"triangle","frequency_hz":1000.,"amplitude_vpp":2.,"offset_v":0.}))?;
                port.action(&binding,"set_output",&json!({"channel":1,"enabled":true}))?;
                ctx.cancel_flag().store(true,Ordering::Release);
                port.action(&binding,"set_output",&json!({"channel":1,"enabled":false}))?;
                Ok::<_,ExperimentError>(())})();
            port.release(&binding).map_err(|e|HostError::new(&e.code,e.message))?;outcome.map_err(|e|HostError::new(&e.code,e.message))?;Ok(json!({"off_confirmed":true}))
        }).unwrap();
        let id=accepted["job_id"].as_str().unwrap();let end=Instant::now()+Duration::from_secs(20);let settled=loop{let s=workbench.status(id,fixture.local.session.id()).unwrap();if !matches!(s["status"].as_str(),Some("queued"|"running")){break s;}assert!(Instant::now()<end,"{s}");tokio::time::sleep(Duration::from_millis(10)).await;};
        assert_eq!(settled["status"],"completed","{settled}");let after=core.worker.domain_context(&domain.key()).unwrap();assert_eq!(after["connection_id"],before["connection_id"]);assert!(after["epoch"].as_u64().unwrap()>before["epoch"].as_u64().unwrap());
        let cache=core.worker_cache_with_refresh(true).await.unwrap();assert_eq!(cache["domains"][domain.key()]["state"],"STOP_HELD");assert!(core.experiment_domains.lock().unwrap().is_empty());
        assert_eq!(core.leases.lock().unwrap().for_session(&fixture.local.session,&domain,Instant::now()).unwrap().unwrap().token,lease["token"]);
        let path=workbench.store.root().parent().unwrap().parent().unwrap().join("operations.json");
        let journal:Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut operations:Vec<_>=journal["records"].as_array().unwrap().iter().filter(|r|r["session_id"]==journal_id).collect();
        operations.sort_by_key(|r|r["sequence"].as_u64().unwrap());
        let names:Vec<_>=operations.iter().map(|r|if r["command"]["method"]=="resume" {"resume"}else{r["command"]["params"]["name"].as_str().unwrap()}).collect();
        assert_eq!(names,["set_output","read_status","resume","configure_signal","set_output","set_output"],"One fresh read must precede logical resume, with no read or resume after final cancelled cleanup Off");
        let fenced=&operations[0]["result"]["context"];
        assert_eq!(operations[1]["result"]["context"],*fenced);
        assert_eq!(operations[2]["result"]["context"],*fenced);
        assert_eq!(fenced["connection_id"],before["connection_id"]);
        assert_eq!(operations[1]["result"]["result"]["status"]["quality"],"confirmed");
        assert_eq!(operations[1]["result"]["result"]["status"]["previous"],false);
        assert_eq!(operations[1]["result"]["result"]["status"]["channels"][0]["output_enabled"],false);
    });
}

#[test]
fn actual_host_rejects_remote_or_absent_roles_without_creating_job_or_reserving_control() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let fixture = super::super::native_osa_flow_tests::Fixture::new(true);
            let core = fixture.core().clone();
            let control = core.leases.lock().unwrap().snapshot();
            let w = core.workbench.as_ref().unwrap();
            let mut d = definition();
            assert_eq!(
                start(core.clone(), fixture.local.session.id(), d.clone())
                    .unwrap_err()
                    .code,
                "RemoteExperimentUnsupported"
            );
            d.roles[0].host_id = core.registry.lock().unwrap().host_id.clone();
            assert_eq!(
                start(core.clone(), fixture.local.session.id(), d)
                    .unwrap_err()
                    .code,
                "RoleUnavailable"
            );
            assert_eq!(core.leases.lock().unwrap().snapshot(), control);
            assert!(core.experiment_domains.lock().unwrap().is_empty());
            assert!(w.store.list().unwrap().is_empty());
        });
}

#[test]
fn reservation_borrows_only_the_same_clients_live_lease() {
    let mut book = LeaseBook::new(&"a".repeat(32)).unwrap();
    let domain = DomainRef {
        kind: "device".into(),
        id: "b".repeat(32),
    };
    book.register(&domain).unwrap();
    let owner = Session::local("c".repeat(32), "a".repeat(32)).unwrap();
    let other = Session::local("d".repeat(32), "a".repeat(32)).unwrap();
    let (fresh, acquired) = borrow_or_acquire(&mut book, &owner, &domain).unwrap();
    assert!(acquired);
    let (borrowed, acquired) = borrow_or_acquire(&mut book, &owner, &domain).unwrap();
    assert!(!acquired);
    assert_eq!(borrowed.token, fresh.token);
    assert_eq!(
        borrow_or_acquire(&mut book, &other, &domain)
            .unwrap_err()
            .code,
        "ControlOwned"
    );
    book.admit(
        &fresh.token,
        &owner,
        &domain,
        fresh.control_epoch,
        Instant::now(),
    )
    .unwrap();
}

fn record() -> WaveformRecord {
    use yang_data::*;
    WaveformRecord {
        schema: 1,
        id: "1".repeat(32),
        name: "literal capture".into(),
        comment: String::new(),
        created_at_unix_ms: 100,
        source: json!({"kind":"experiment","run_id":"a".repeat(32),"run_status":"running"}),
        analyses: vec![],
        frames: vec![WaveformFrame {
            id: "2".repeat(32),
            captured_at_unix_ms: 101,
            nominal_offset_s: Some(0.),
            channels: vec![ChannelWaveform {
                channel: "CH1".into(),
                samples: Samples::U16(vec![0, 32767, 65535]),
                calibration: Calibration {
                    x_increment_s: 1e-6,
                    x_origin_s: 0.,
                    x_reference: 0.,
                    y_increment_v: 0.01,
                    y_origin: 0.,
                    y_reference: 0.,
                },
                metadata: json!({}),
            }],
        }],
    }
}

#[test]
fn staging_ack_follows_durable_grouped_save_and_failed_save_retains_descriptor() {
    let root = std::env::temp_dir().join(format!("host-experiment-{}", new_id().unwrap()));
    let store = DataStore::new(&root).unwrap();
    let r = record();
    let mut pending = Some(json!({"capture_id":"f".repeat(32)}));
    commit_frame(&store, &r, &mut pending, |_| {
        assert_eq!(
            store.load(&r.id).unwrap().frames[0].channels[0].samples,
            yang_data::Samples::U16(vec![0, 32767, 65535])
        );
        Ok(())
    })
    .unwrap();
    assert!(pending.is_none());
    let mut pending = Some(json!({"capture_id":"e".repeat(32)}));
    let mut bad = r;
    bad.frames.clear();
    assert!(commit_frame(&store, &bad, &mut pending, |_| panic!(
        "failed storage must never acknowledge Worker responsibility"
    ))
    .is_err());
    assert!(pending.is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn terminal_evidence_retains_durable_frame_after_unconfirmed_staging_release() {
    let root = std::env::temp_dir().join(format!("host-terminal-evidence-{}", new_id().unwrap()));
    let store = DataStore::new(&root).unwrap();
    let r = record();
    let mut pending = Some(json!({"capture_id":"f".repeat(32)}));
    assert!(
        commit_frame(&store, &r, &mut pending, |_| Err(HostError::new(
            "StagingReleaseUnconfirmed",
            "finite lost acknowledgement"
        )))
        .is_err()
    );
    store
        .rename(&r.id, "edited during run", "keep comment")
        .unwrap();
    assert_eq!(
        finalize_evidence(&store, &r.id, RunStatus::Uncertain).unwrap(),
        Some(r.id.clone())
    );
    let m = store.metadata(&r.id).unwrap();
    assert_eq!(m.source["run_status"], "uncertain");
    assert_eq!(m.name, "edited during run");
    assert_eq!(m.frames.len(), 1);
    assert!(pending.is_some());
    assert_eq!(
        finalize_evidence(&store, &"0".repeat(32), RunStatus::Failed).unwrap(),
        None
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn definition() -> ExperimentDefinition {
    ExperimentDefinition {
        schema: 1,
        id: "e".repeat(32),
        name: "capture".into(),
        template_id: "waveform_capture".into(),
        template_version: 1,
        roles: vec![RoleBinding {
            role: "detector".into(),
            host_id: "a".repeat(32),
            domain_id: "b".repeat(32),
            channel: Some(1),
        }],
        parameters: json!({"source":"memory","memory_depth":1000,"frames":1,"interval_ms":0,"channels":[1]}),
    }
}
fn snapshot() -> Value {
    json!({"host_id":"a".repeat(32),"mode":"real","host_status":"ONLINE","startup_error":null,
    "registry":{"devices":[{"device_id":"b".repeat(32),"model_id":"mho5054","config_rev":7,"verified_mode":"real","expected_identity":{"model":"MHO5054","serial":"LITERAL-SN"}}]},
    "domains":{format!("device:{}","b".repeat(32)):{"state":"READY","pending":0,"context":{"connection_id":"c".repeat(32)},"device":{"state":"READY","connected":true,"identity":{"model":"MHO5054","serial":"LITERAL-SN"}}}}})
}

fn scan_definition_and_snapshot() -> (ExperimentDefinition, Value) {
    let mut d = definition();
    d.template_id = "q_scan".into();
    d.parameters = json!({"source":"memory","memory_depth":1000,"frames":1,"interval_ms":0,"channels":[1],"waveform":"triangle","frequency_hz":1000.,"amplitude_vpp":2.,"offset_v":0.,"coarse_wavelength_nm":1550.,"excursion_pm":10.,"ramp_hz":1000.,"wavelength_nm":1550.});
    let mut s = snapshot();
    for (role, id, model, identity, channel) in [
        ("scan_signal", "d", "dg4162", "DG4162", Some(1)),
        ("laser", "f", "tlb6700", "TLB6700", None),
    ] {
        d.roles.push(RoleBinding {
            role: role.into(),
            host_id: "a".repeat(32),
            domain_id: id.repeat(32),
            channel,
        });
        s["registry"]["devices"].as_array_mut().unwrap().push(json!({"device_id":id.repeat(32),"model_id":model,"config_rev":1,"verified_mode":"real","expected_identity":{"model":identity,"serial":"LITERAL"}}));
        s["domains"][format!("device:{}", id.repeat(32))] = json!({"state":"READY","pending":0,"context":{"connection_id":"c".repeat(32)},"device":{"state":"READY","connected":true,"identity":{"model":identity,"serial":"LITERAL"},"quality":"confirmed","previous":false,"wavelength_range_nm":[1500.,1600.],"channels":[{"channel":1,"waveform":"sine","frequency_hz":1000.,"amplitude_vpp":2.,"offset_v":0.,"output_enabled":false,"phase_deg":0.,"load":{"kind":"high_z"},"polarity":"normal"}]}});
    }
    (d, s)
}

#[test]
fn preflight_rejects_invalid_model_signal_and_load_before_any_run_action() {
    let (d, s) = scan_definition_and_snapshot();
    assert!(preflight(&d, &s).is_ok());
    for (frequency, amplitude, offset, load) in [
        (5e6, 2., 0., json!({"kind":"high_z"})),
        (1000., 0.001, 0., json!({"kind":"high_z"})),
        (1000., 8., 2., json!({"kind":"ohms","ohms":50.})),
    ] {
        let mut bad = d.clone();
        bad.parameters["frequency_hz"] = json!(frequency);
        bad.parameters["ramp_hz"] = json!(frequency);
        bad.parameters["amplitude_vpp"] = json!(amplitude);
        bad.parameters["offset_v"] = json!(offset);
        let mut status = s.clone();
        status["domains"][format!("device:{}", "d".repeat(32))]["device"]["channels"][0]["load"] =
            load;
        assert_eq!(
            preflight(&bad, &status).unwrap_err().code,
            "InvalidExperimentSignal"
        );
    }
    let mut missing = s.clone();
    missing["domains"][format!("device:{}", "d".repeat(32))]["device"]["channels"] = json!([]);
    assert_eq!(
        preflight(&d, &missing).unwrap_err().code,
        "PreflightReadbackRequired"
    );
    let mut historical = s.clone();
    historical["domains"][format!("device:{}", "d".repeat(32))]["device"]["previous"] = json!(true);
    assert_eq!(
        preflight(&d, &historical).unwrap_err().code,
        "PreflightReadbackRequired"
    );
    let mut bad = d.clone();
    bad.parameters["coarse_wavelength_nm"] = json!(1499.);
    assert_eq!(
        preflight(&bad, &s).unwrap_err().code,
        "InvalidExperimentWavelength"
    );
}

#[test]
fn invalid_model_preflight_does_not_change_actual_host_action_journal_or_reservations() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let fixture = super::super::native_osa_flow_tests::Fixture::new(true);
            let core = fixture.core().clone();
            // Explicit bounded metadata double at the admission boundary only. No
            // transport/backend is selected or invoked for these rejected settings.
            let (mut d, s) = scan_definition_and_snapshot();
            let host = core.registry.lock().unwrap().host_id.clone();
            for b in &mut d.roles {
                b.host_id = host.clone();
            }
            let mut devices = vec![];
            let mut cache = json!({"domains":{},"devices":{}});
            for device in s["registry"]["devices"].as_array().unwrap() {
                let mut device = device.clone();
                device["name"] = json!("Bounded preflight metadata");
                device["category"] = json!("test-only");
                device["driver_id"] = json!("test-only");
                device["profile_id"] = json!("test-only");
                device["params"] = json!({});
                device["identity_strength"] = json!("verified");
                device["check_policy"] =
                    json!({"interval_s":30,"enumeration":null,"readonly":null});
                let id = device["device_id"].as_str().unwrap().to_owned();
                let domain = DomainRef {
                    kind: "device".into(),
                    id,
                };
                let key = domain.key();
                core.bindings
                    .lock()
                    .unwrap()
                    .insert(domain, s["domains"][&key]["context"].clone());
                cache["domains"][&key] = s["domains"][&key].clone();
                cache["devices"][&key] = s["domains"][&key]["device"].clone();
                devices.push(serde_json::from_value(device).unwrap());
            }
            core.registry.lock().unwrap().devices = devices;
            *core.status_cache.lock().unwrap() = Some((
                Instant::now(),
                core.status_generation.load(Ordering::Acquire),
                cache,
            ));
            d.parameters["frequency_hz"] = json!(5e6);
            d.parameters["ramp_hz"] = json!(5e6);
            let book_path = core
                .workbench
                .as_ref()
                .unwrap()
                .store
                .root()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("operations.json");
            let before = std::fs::read(&book_path).ok();
            let leases = core.leases.lock().unwrap().snapshot();
            assert_eq!(
                start(core.clone(), fixture.local.session.id(), d)
                    .unwrap_err()
                    .code,
                "InvalidExperimentSignal"
            );
            assert_eq!(
                std::fs::read(book_path).ok(),
                before,
                "No laser, generator or scope action may reach the journal"
            );
            assert_eq!(core.leases.lock().unwrap().snapshot(), leases);
            assert!(core.experiment_domains.lock().unwrap().is_empty());
            assert!(!core.workbench.as_ref().unwrap().active_experiments(None));
        });
}

#[test]
fn preflight_rejects_remote_roles_before_considering_local_ready_devices() {
    let mut d = definition();
    d.roles[0].host_id = "f".repeat(32);
    assert_eq!(
        preflight(&d, &snapshot()).unwrap_err().code,
        "RemoteExperimentUnsupported"
    );
}

#[test]
fn preflight_requires_real_ready_identity_and_reviewed_family() {
    let d = definition();
    let s = snapshot();
    let accepted = preflight(&d, &s).unwrap();
    assert_eq!(accepted["detector"].config_rev, 7);
    assert!(accepted["detector"]
        .capabilities
        .contains(&"capture_memory".into()));
    for (path, value, code) in [
        ("state", json!("STOP_HELD"), "RoleUnavailable"),
        ("connected", json!(false), "RoleUnavailable"),
        (
            "identity",
            json!({"model":"MHO5054","serial":"CHANGED"}),
            "IdentityMismatch",
        ),
    ] {
        let mut bad = s.clone();
        let key = format!("device:{}", "b".repeat(32));
        if path == "state" {
            bad["domains"][&key][path] = value;
        } else {
            bad["domains"][&key]["device"][path] = value;
        }
        assert_eq!(preflight(&d, &bad).unwrap_err().code, code);
    }
    let mut bad = s;
    bad["registry"]["devices"][0]["model_id"] = json!("aq6370");
    assert_eq!(preflight(&d, &bad).unwrap_err().code, "MissingCapability");
}

#[test]
fn complete_run_raw_budget_is_admitted_before_any_capture() {
    let mut d = definition();
    d.parameters["memory_depth"] = json!(250_000_000);
    d.parameters["frames"] = json!(2);
    assert_eq!(
        preflight(&d, &snapshot()).unwrap_err().code,
        "CaptureBudget"
    );
    d.parameters["memory_depth"] = json!(1000);
    d.parameters["frames"] = json!(1);
    assert!(preflight(&d, &snapshot()).is_ok());
    d.parameters["memory_depth"] = json!(2);
    assert_eq!(
        preflight(&d, &snapshot()).unwrap_err().code,
        "MemoryDepthUnsupported"
    );
}
