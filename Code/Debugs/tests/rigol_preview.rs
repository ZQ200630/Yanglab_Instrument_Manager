use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    },
};
use yang_debug::{execute_with, parse_args, DiagnosticAuthorization, DiagnosticEnvironment};
use yang_drivers::{
    clock::ManualClock,
    lifecycle::{CleanupReport, CleanupStep, DriverLifecycle, DriverState, ProbeReport},
    scope::{Scope, ScopeModel, ScopeOptions},
    transport::{ByteTransport, CloseReport, Deadline},
    DriverError, DriverResult,
};
use yang_worker::{
    rigol_sessions::RigolSession,
    session::{DeviceSession, DriverFactory},
    WorkerError,
};

fn binding(family: &str, model: &str) -> Value {
    json!({"domain":{"kind":"device","id":"a".repeat(32)},"driver_kind":family,"model_id":model,"profile_id":"visa","params":{"resource":"GPIB0::7::INSTR"},"expected_identity":{},"config_rev":1,"members":[]})
}
fn args(family: &str, model: &str, stage: &str, out: &Path) -> Vec<String> {
    vec![
        family.into(),
        "--stage".into(),
        stage.into(),
        "--out".into(),
        out.to_str().unwrap().into(),
        "--binding".into(),
        binding(family, model).to_string(),
    ]
}
fn capture() -> Value {
    json!({"name":"capture","args":{"channels":[1],"source":"screen","fresh":false,"byte_budget":4096}})
}
fn action_args(model: &str, out: &Path, actions: Vec<Value>) -> Vec<String> {
    let mut a = args("scope", model, "action", out);
    a.extend(["--actions".into(), json!(actions).to_string()]);
    a
}
fn output() -> PathBuf {
    PathBuf::from("D:/Qian/Codex_Project/SIL_Experiments/Result/rigol-check")
}

#[test]
fn native_rigol_readonly_diagnostics_preview_without_opening_any_resource() {
    for (family, model) in [
        ("scope", "ds7054"),
        ("scope", "mho5054"),
        ("generator", "dg4162"),
        ("generator", "dg4202"),
        ("generator", "dg922pro"),
    ] {
        let plan = parse_args(&args(family, model, "readonly", &output())).unwrap();
        assert!(!plan.execute_requested());
        assert_eq!(plan.preview()["output_changes"], false);
        assert!(DiagnosticAuthorization::from_plan(&plan).is_err());
    }
    let default = std::process::Command::new(env!("CARGO_BIN_EXE_yang-debug"))
        .output()
        .unwrap();
    assert!(default.status.success());
    assert!(String::from_utf8(default.stdout)
        .unwrap()
        .contains("No resources opened. Preview only."));
}

#[test]
fn only_bounded_explicit_existing_screen_capture_is_admitted() {
    for model in ["ds7054", "mho5054"] {
        let plan = parse_args(&action_args(model, &output(), vec![capture()])).unwrap();
        assert!(!plan.execute_requested());
        assert!(DiagnosticAuthorization::from_plan(&plan).is_err());
        assert_eq!(plan.preview()["actions"][0]["args"]["fresh"], false);
    }
    let mut boundary = capture();
    boundary["args"]["channels"] = json!([1, 2, 3, 4]);
    assert!(parse_args(&action_args("ds7054", &output(), vec![boundary.clone()])).is_ok());
    assert!(
        parse_args(&action_args("mho5054", &output(), vec![boundary])).is_err(),
        "aggregate native sample bytes exceed 4096"
    );
}

#[test]
fn contradictory_or_actuating_actions_are_rejected_before_effects() {
    let mut bad = vec![];
    for (key, value) in [
        ("fresh", json!(true)),
        ("stop_before", json!(true)),
        ("source", json!("memory")),
        ("byte_budget", json!(4097)),
        ("byte_budget", json!(0)),
        ("byte_budget", json!(999)),
        ("channels", json!([0])),
        ("channels", json!([5])),
        ("channels", json!([1, 1])),
        ("channels", json!([])),
        ("points", json!(1001)),
        ("points", json!(0)),
        ("points", json!(1.5)),
        ("run", json!(true)),
    ] {
        let mut c = capture();
        c["args"][key] = value;
        bad.push(c);
    }
    for key in ["fresh", "byte_budget", "source", "channels"] {
        let mut c = capture();
        c["args"].as_object_mut().unwrap().remove(key);
        bad.push(c);
    }
    bad.extend([
        json!({"name":"run","args":{}}),
        json!({"name":"stop","args":{}}),
        json!({"name":"set_memory_depth","args":{"points":1000}}),
        json!({"name":"set_channel_display","args":{"channel":1,"enabled":true}}),
        json!({"name":"read_status","args":{}}),
    ]);
    for c in bad {
        assert!(
            parse_args(&action_args("ds7054", &output(), vec![c.clone()])).is_err(),
            "admitted {c}"
        );
    }
    assert!(parse_args(&action_args(
        "ds7054",
        &output(),
        vec![capture(), capture()]
    ))
    .is_err());
    let mut a = action_args("ds7054", &output(), vec![capture()]);
    a.extend([
        "--execute".into(),
        "--confirm-stage".into(),
        "readonly".into(),
    ]);
    assert!(parse_args(&a).is_err());
    let mut generator = args("generator", "dg4162", "action", &output());
    generator.extend([
        "--actions".into(),
        r#"[{"name":"set_output","args":{"channel":1,"enabled":true}}]"#.into(),
    ]);
    assert!(parse_args(&generator).is_err());
}

// Only identity, read-only transfer inspection and one existing Screen transfer
// are admitted. Acquisition/settings changes fail this finite transcript.
struct Wire {
    expected: VecDeque<(&'static str, Vec<u8>)>,
    pending: VecDeque<u8>,
    events: Arc<Mutex<Vec<String>>>,
    live: bool,
    fail_data: bool,
    fail_probe: bool,
}
impl Wire {
    fn new(action: bool, fail_data: bool, fail_probe: bool, events: Arc<Mutex<Vec<String>>>) -> Self {
        let mut expected = VecDeque::from([("*IDN?", b"RIGOL,DS7054,SCREEN-7,1.0\n".to_vec())]);
        if action {
            let mut block = b"#41000".to_vec();
            block.extend(std::iter::repeat_n(128, 999));
            block.extend([255, b'\n']);
            expected.extend([
                (":TRIGger:STATus?", b"RUN\n".to_vec()),
                (":WAVeform:SOURce CHANnel1", vec![]),
                (":WAVeform:MODE NORMal", vec![]),
                (":WAVeform:FORMat BYTE", vec![]),
                (":WAVeform:STARt 1", vec![]),
                (":WAVeform:POINts 1000", vec![]),
                (":WAVeform:STOP 1000", vec![]),
                (
                    ":WAVeform:PREamble?",
                    b"0,0,1000,1,1e-9,0,0,0.01,10,128\n".to_vec(),
                ),
                (":WAVeform:STARt 1", vec![]),
                (":WAVeform:STOP 1000", vec![]),
                (":WAVeform:DATA?", block),
            ]);
        } else if fail_probe {
            expected.push_back((":TRIGger:STATus?", vec![]));
        } else {
            expected.extend([
                (":TRIGger:STATus?", b"RUN\n".to_vec()),
                (":ACQuire:MDEPth?", b"10K\n".to_vec()),
                (":WAVeform:SOURce?", b"CHAN4\n".to_vec()),
                (":WAVeform:MODE?", b"RAW\n".to_vec()),
                (":WAVeform:FORMat?", b"BYTE\n".to_vec()),
                (":WAVeform:POINts?", b"1000\n".to_vec()),
                (":WAVeform:STARt?", b"1\n".to_vec()),
                (":WAVeform:STOP?", b"1000\n".to_vec()),
                (":WAVeform:PREamble?", b"0,2,1000,1,1e-9,0,0,0.01,10,128\n".to_vec()),
                (":CHANnel1:DISPlay?", b"ON\n".to_vec()),
                (":CHANnel2:DISPlay?", b"0\n".to_vec()),
                (":CHANnel3:DISPlay?", b"1\n".to_vec()),
                (":CHANnel4:DISPlay?", b"OFF\n".to_vec()),
            ]);
        }
        Self {
            expected,
            pending: VecDeque::new(),
            events,
            live: true,
            fail_data,
            fail_probe,
        }
    }
}
impl ByteTransport for Wire {
    fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<()> {
        deadline.remaining_millis()?;
        assert!(self.live && self.pending.is_empty());
        let command = std::str::from_utf8(bytes)
            .unwrap()
            .strip_suffix('\n')
            .unwrap();
        let (expected, reply) = self.expected.pop_front().expect("unexpected extra command");
        assert_eq!(
            command, expected,
            "diagnostic crossed finite Screen-only transcript"
        );
        self.events.lock().unwrap().push(command.into());
        if self.fail_probe && command == ":TRIGger:STATus?" {
            return Err(DriverError::Timeout {
                operation: "finite readonly trigger-state query".into(),
                transferred: 0,
            });
        }
        if self.fail_data && command == ":WAVeform:DATA?" {
            return Err(DriverError::Timeout {
                operation: "finite waveform data write".into(),
                transferred: 0,
            });
        }
        self.pending = reply.into();
        Ok(())
    }
    fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
        deadline.remaining_millis()?;
        assert!(self.live && maximum > 0 && maximum <= 16384 && !self.pending.is_empty());
        Ok(self
            .pending
            .drain(..maximum.min(127).min(self.pending.len()))
            .collect())
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        assert!(
            self.expected.is_empty(),
            "diagnostic skipped expected Screen transcript"
        );
        self.events.lock().unwrap().push("close".into());
        self.live = false;
        Ok(CloseReport {
            released: true,
            status: Some(0),
        })
    }
    fn has_responsibility(&self) -> bool {
        self.live
    }
}
struct Factory {
    events: Arc<Mutex<Vec<String>>>,
    fail_data: bool,
    fail_probe: bool,
}
impl DriverFactory for Factory {
    fn create(
        &self,
        _: &yang_protocol::DomainConfig,
    ) -> Result<Box<dyn DeviceSession>, WorkerError> {
        let action = self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| *e == "create")
            .count()
            == 1;
        self.events.lock().unwrap().push("create".into());
        let clock = Arc::new(ManualClock::default());
        let scope = Scope::with_transport(
            ScopeModel::Ds7054,
            Box::new(Wire::new(action, self.fail_data, self.fail_probe, self.events.clone())),
            clock.clone(),
            ScopeOptions::default(),
        )?;
        Ok(Box::new(RigolSession::from_scope(scope, clock)?))
    }
}
struct Owner(Arc<Mutex<Vec<String>>>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("owner_release".into());
    }
}
struct Env(Arc<Mutex<Vec<String>>>);
impl DiagnosticEnvironment for Env {
    fn acquire_owner(&self) -> Result<Box<dyn Send>, String> {
        self.0.lock().unwrap().push("owner".into());
        Ok(Box::new(Owner(self.0.clone())))
    }
    fn enumerate(&self) -> Result<Value, String> {
        panic!("explicit Screen diagnostic must not enumerate")
    }
}
fn run(fail_data: bool) -> (PathBuf, Value, Vec<String>) {
    let root = std::env::temp_dir().join(format!(
        "yang-diag-screen-{}",
        yang_worker::new_id().unwrap()
    ));
    std::fs::create_dir_all(root.join("Result")).unwrap();
    let out = root.join("Result/check");
    let mut a = action_args("ds7054", &out, vec![capture()]);
    a.extend([
        "--execute".into(),
        "--confirm-stage".into(),
        "action".into(),
    ]);
    let plan = parse_args(&a).unwrap();
    let authorization = DiagnosticAuthorization::from_plan(&plan).unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let report = execute_with(
        plan,
        authorization,
        Arc::new(Factory {
            events: events.clone(),
            fail_data,
            fail_probe: false,
        }),
        &Env(events.clone()),
    )
    .unwrap();
    assert!(report.resources_released());
    assert_eq!(report.success(), !fail_data, "{}", serde_json::to_string(&report).unwrap());
    let report = serde_json::from_slice(&std::fs::read(out.join("report.json")).unwrap()).unwrap();
    let observed = events.lock().unwrap().clone();
    assert_eq!(
        &observed[..19],
        ["owner", "create", "*IDN?", ":TRIGger:STATus?", ":ACQuire:MDEPth?",
         ":WAVeform:SOURce?", ":WAVeform:MODE?", ":WAVeform:FORMat?",
         ":WAVeform:POINts?", ":WAVeform:STARt?", ":WAVeform:STOP?",
          ":WAVeform:PREamble?", ":CHANnel1:DISPlay?", ":CHANnel2:DISPlay?",
          ":CHANnel3:DISPlay?", ":CHANnel4:DISPlay?", "close", "create", "*IDN?"]
    );
    assert_eq!(&observed[observed.len() - 2..], ["close", "owner_release"]);
    (root, report, observed)
}

fn run_readonly(fail_probe: bool) -> (PathBuf, Value, Vec<String>) {
    let root = std::env::temp_dir().join(format!("yang-diag-transfer-{}", yang_worker::new_id().unwrap()));
    std::fs::create_dir_all(root.join("Result")).unwrap();
    let out = root.join("Result/check");
    let mut a = args("scope", "ds7054", "readonly", &out);
    a.extend(["--execute".into(), "--confirm-stage".into(), "readonly".into()]);
    let plan = parse_args(&a).unwrap();
    let authorization = DiagnosticAuthorization::from_plan(&plan).unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let report = execute_with(plan, authorization, Arc::new(Factory {
        events: events.clone(), fail_data: false, fail_probe,
    }), &Env(events.clone())).unwrap();
    assert!(report.resources_released());
    assert_eq!(report.success(), !fail_probe);
    let saved = serde_json::from_slice(&std::fs::read(out.join("report.json")).unwrap()).unwrap();
    let observed = events.lock().unwrap().clone();
    (root, saved, observed)
}

#[test]
fn readonly_scope_diagnostic_saves_transfer_evidence_without_changing_settings_or_reading_data() {
    let (root, report, events) = run_readonly(false);
    assert_eq!(report["results"][0]["probe"]["observations"]["transfer_state"], json!({
        "model":"DS7054", "trigger_status":"RUN", "acquisition_memory_depth":"10K",
        "waveform_source":"CHAN4", "waveform_mode":"RAW", "waveform_format":"BYTE",
        "waveform_points":"1000", "waveform_start":"1", "waveform_stop":"1000",
        "waveform_preamble":"0,2,1000,1,1e-9,0,0,0.01,10,128",
        "channel_1_display":"ON", "channel_2_display":"0", "channel_3_display":"1", "channel_4_display":"OFF",
    }));
    assert_eq!(events, [
        "owner", "create", "*IDN?", ":TRIGger:STATus?", ":ACQuire:MDEPth?",
        ":WAVeform:SOURce?", ":WAVeform:MODE?", ":WAVeform:FORMat?", ":WAVeform:POINts?",
        ":WAVeform:STARt?", ":WAVeform:STOP?", ":WAVeform:PREamble?",
        ":CHANnel1:DISPlay?", ":CHANnel2:DISPlay?", ":CHANnel3:DISPlay?", ":CHANnel4:DISPlay?", "close", "owner_release",
    ]);
    assert!(!root.join("Result/check/records").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn readonly_scope_diagnostic_query_failure_closes_once_without_replay() {
    let (root, report, events) = run_readonly(true);
    assert!(report["error"].as_str().unwrap().contains("finite readonly trigger-state query"));
    assert_eq!(report["results"], json!([]));
    assert_eq!(events, ["owner", "create", "*IDN?", ":TRIGger:STATus?", "close", "owner_release"]);
    assert!(!root.join("Result/check/records").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn finite_existing_screen_capture_saves_raw_calibration_atomically_and_releases() {
    let (root, report, _) = run(false);
    let saved = report["results"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|r| r.get("waveform"))
        .expect("durably saved waveform summary");
    assert_eq!(saved["channels"][0]["channel"], "CH1");
    assert_eq!(saved["channels"][0]["points"], 1000);
    assert!(saved.get("samples").is_none());
    let store = yang_data::DataStore::new(root.join("Result/check/records")).unwrap();
    let record = store.load(saved["record_id"].as_str().unwrap()).unwrap();
    assert_eq!(record.frames.len(), 1);
    assert_eq!(record.frames[0].id, saved["frame_id"].as_str().unwrap());
    assert_eq!(
        record.frames[0].channels[0].samples,
        yang_data::Samples::U8([vec![128; 999], vec![255]].concat())
    );
    let calibration = &record.frames[0].channels[0].calibration;
    assert_eq!(calibration.x_increment_s, 1e-9);
    assert_eq!(calibration.y_increment_v, 0.01);
    assert_eq!(calibration.y_origin, -0.1);
    assert_eq!(calibration.y_reference, 128.);
    assert_eq!(record.frames[0].channels[0].metadata["source"], "screen");
    assert!(record.frames[0].channels[0]
        .metadata
        .get("fresh_acquisition")
        .is_none());
    let files: Vec<_> = std::fs::read_dir(root.join("Result/check/records").join(&record.id))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        files
            .iter()
            .filter(|f| f.starts_with("raw-") && f.ends_with(".npy"))
            .count(),
        1
    );
    assert_eq!(
        files
            .iter()
            .filter(|f| f.starts_with("commit-") && f.ends_with(".json"))
            .count(),
        1
    );
    assert!(!files.iter().any(|f| f.ends_with(".part")));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_screen_capture_is_not_replayed_and_cleanup_is_recorded() {
    let (root, report, events) = run(true);
    assert!(report["error"]
        .as_str()
        .unwrap()
        .contains("no automatic replay"));
    assert_eq!(events.iter().filter(|e| *e == ":WAVeform:DATA?").count(), 1);
    assert!(!report["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r.get("waveform").is_some()));
    assert!(!root.join("Result/check/records").exists());
    std::fs::remove_dir_all(root).unwrap();
}

// A session retains a pool dependency even after its instrument handle closes.
// Pool release is possible only after the confirmed-closed session is dropped.
struct PoolFactory {
    dependency: Arc<()>,
    events: Arc<Mutex<Vec<String>>>,
    close_mode: Arc<AtomicU8>,
}
fn pool_receipt(unreleased: Vec<String>) -> DriverResult<CleanupReport> {
    CleanupReport::new(
        yang_worker::new_id().unwrap(),
        vec![CleanupStep {
            role: "pool-fixture".into(),
            action: "release".into(),
            error: None,
        }],
        None,
        unreleased,
    )
}
impl DriverFactory for PoolFactory {
    fn create(
        &self,
        _: &yang_protocol::DomainConfig,
    ) -> Result<Box<dyn DeviceSession>, WorkerError> {
        Ok(Box::new(PoolSession {
            _dependency: self.dependency.clone(),
            events: self.events.clone(),
            close_mode: self.close_mode.clone(),
            live: true,
        }))
    }
    fn finish_shutdown(&self) -> DriverResult<CleanupReport> {
        self.events.lock().unwrap().push("finish_pool".into());
        pool_receipt(if Arc::strong_count(&self.dependency) == 1 {
            vec![]
        } else {
            vec!["dependent_session_clone".into()]
        })
    }
}
struct PoolSession {
    _dependency: Arc<()>,
    events: Arc<Mutex<Vec<String>>>,
    close_mode: Arc<AtomicU8>,
    live: bool,
}
impl Drop for PoolSession {
    fn drop(&mut self) {
        self.events.lock().unwrap().push("session_drop".into());
    }
}
impl DriverLifecycle for PoolSession {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        self.events.lock().unwrap().push("session_close".into());
        match self.close_mode.load(Ordering::SeqCst) {
            0 => {
                self.live = false;
                pool_receipt(vec![])
            }
            mode => {
                if mode == 2 {
                    self.live = false;
                }
                Err(DriverError::Responsibility(
                    "fixture close unconfirmed".into(),
                ))
            }
        }
    }
    fn has_responsibility(&self) -> bool {
        self.live
    }
}
struct PoolStop;
impl yang_worker::session::StopSignal for PoolStop {
    fn request_stop(&self) {}
}
impl DeviceSession for PoolSession {
    fn connect(&mut self) -> DriverResult<ProbeReport> {
        Ok(ProbeReport::new(json!({}), json!({}), false))
    }
    fn probe_readonly(&mut self) -> DriverResult<ProbeReport> {
        self.connect()
    }
    fn state(&self) -> DriverState {
        if self.live {
            DriverState::Ready
        } else {
            DriverState::Disconnected
        }
    }
    fn identity(&self) -> Value {
        json!({})
    }
    fn stop_signal(&self) -> Arc<dyn yang_worker::session::StopSignal> {
        Arc::new(PoolStop)
    }
    fn action(
        &mut self,
        _: &str,
        _: &Value,
        _: &yang_protocol::ContextV3,
    ) -> yang_protocol::OutcomeV3 {
        panic!("readonly must not act")
    }
    fn observe(&mut self, _: &yang_protocol::ContextV3) -> yang_worker::observations::Observation {
        panic!("readonly probe must not observe")
    }
}
fn pool_run(
    close_mode: u8,
) -> (
    PathBuf,
    yang_debug::DiagnosticReport,
    Arc<Mutex<Vec<String>>>,
    Arc<AtomicU8>,
) {
    let root =
        std::env::temp_dir().join(format!("yang-diag-pool-{}", yang_worker::new_id().unwrap()));
    std::fs::create_dir_all(root.join("Result")).unwrap();
    let mut a = args("scope", "ds7054", "readonly", &root.join("Result/check"));
    a.extend([
        "--execute".into(),
        "--confirm-stage".into(),
        "readonly".into(),
    ]);
    let plan = parse_args(&a).unwrap();
    let authorization = DiagnosticAuthorization::from_plan(&plan).unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let mode = Arc::new(AtomicU8::new(close_mode));
    let report = execute_with(
        plan,
        authorization,
        Arc::new(PoolFactory {
            dependency: Arc::new(()),
            events: events.clone(),
            close_mode: mode.clone(),
        }),
        &Env(events.clone()),
    )
    .unwrap();
    (root, report, events, mode)
}
static POOL_TEST_GATE: Mutex<()> = Mutex::new(());

#[test]
fn confirmed_closed_scope_session_drops_before_pool_shutdown() {
    let _gate = POOL_TEST_GATE.lock().unwrap_or_else(|e| e.into_inner());
    let (root, report, events, _) = pool_run(0);
    // Drain fixture cleanup even on the red branch; do not leak its owner.
    yang_debug::retry_pending();
    let observed = events.lock().unwrap().clone();
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        report.success(),
        "closed session clone must not prevent pool release"
    );
    assert_eq!(
        observed,
        [
            "owner",
            "session_close",
            "session_drop",
            "finish_pool",
            "owner_release"
        ]
    );
}

#[test]
fn close_errors_retain_owner_and_session_until_a_confirmed_cleanup_receipt() {
    let _gate = POOL_TEST_GATE.lock().unwrap_or_else(|e| e.into_inner());
    for close_mode in [1, 2] {
        let (root, report, events, mode) = pool_run(close_mode);
        assert!(!report.resources_released());
        let stored = serde_json::from_slice::<Value>(
            &std::fs::read(root.join("Result/check/report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored["cleanup"][0]["resources_released"], false);
        assert!(!events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e == "session_drop" || e == "owner_release"));
        mode.store(0, Ordering::SeqCst);
        assert_eq!(yang_debug::retry_pending(), 0);
        assert!(
            !report.resources_released(),
            "the failed-attempt receipt is immutable"
        );
        assert_eq!(
            &events.lock().unwrap()[3..],
            [
                "session_close",
                "session_drop",
                "finish_pool",
                "owner_release"
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
