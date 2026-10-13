use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use yang_data::{Calibration, ChannelWaveform, Samples, WaveformFrame, WaveformRecord};
use yang_experiments::{
    run_experiment, ExperimentDefinition, ExperimentError, ExperimentPort, RoleBinding, RoleInfo,
    RunStatus,
};

fn definition(template: &str) -> ExperimentDefinition {
    let mut roles = vec![
        json!({"role":"detector","host_id":"a".repeat(32),"domain_id":"b".repeat(32),"channel":1}),
    ];
    if template != "waveform_capture" {
        roles.extend([
        json!({"role":"scan_signal","host_id":"a".repeat(32),"domain_id":"c".repeat(32),"channel":1}),
        json!({"role":"laser","host_id":"a".repeat(32),"domain_id":"d".repeat(32),"channel":null})]);
    }
    serde_json::from_value(json!({"schema":1,"id":"e".repeat(32),"name":"Q test","template_id":template,"template_version":1,"roles":roles,
        "parameters":{"source":"screen","memory_depth":null,"frames":2,"interval_ms":100,"channels":[1],"waveform":"triangle","frequency_hz":50.0,"amplitude_vpp":2.0,"offset_v":0.0,"coarse_wavelength_nm":null,
        "excursion_pm":20.0,"ramp_hz":50.0,"wavelength_nm":780.0,"scan_speed_nm_s":0.05,"prominence_fraction":0.1,"min_distance_samples":2,"roi":null}})).unwrap()
}
#[derive(Default)]
struct Port {
    calls: Vec<String>,
    saved: Vec<WaveformRecord>,
    capture_count: usize,
    fail_reserve: Option<String>,
    fail_capture: Option<usize>,
    missing_cap: bool,
    fail_cleanup: bool,
    cancel_after_capture: Option<Arc<AtomicBool>>,
    cancel_after_commit: Option<Arc<AtomicBool>>,
    cancel_during_wait: bool,
    fail_commit: bool,
    fail_run: bool,
    committed: Vec<WaveformRecord>,
}
impl ExperimentPort for Port {
    fn inspect(&mut self, b: &RoleBinding) -> Result<RoleInfo, ExperimentError> {
        self.calls.push(format!("inspect:{}", b.role));
        Ok(RoleInfo {
            family: match b.role.as_str() {
                "detector" => "scope",
                "scan_signal" => "generator",
                _ => "laser",
            }
            .into(),
            identity: json!({"serial":b.domain_id}),
            config_rev: 1,
            connected: true,
            capabilities: if self.missing_cap {
                vec![]
            } else {
                vec![
                    "capture_screen".into(),
                    "capture_memory".into(),
                    "triangle".into(),
                    "configure_signal".into(),
                    "set_output".into(),
                    "tune_wavelength".into(),
                ]
            },
        })
    }
    fn reserve(&mut self, b: &RoleBinding) -> Result<(), ExperimentError> {
        self.calls.push(format!("reserve:{}", b.role));
        if self.fail_reserve.as_deref() == Some(&b.role) {
            Err(ExperimentError::new("Owned", "already owned"))
        } else {
            Ok(())
        }
    }
    fn release(&mut self, b: &RoleBinding) -> Result<(), ExperimentError> {
        self.calls.push(format!("release:{}", b.role));
        Ok(())
    }
    fn action(
        &mut self,
        b: &RoleBinding,
        name: &str,
        args: &Value,
    ) -> Result<Value, ExperimentError> {
        self.calls.push(format!("action:{}:{name}:{args}", b.role));
        if self.fail_run && name == "run" {
            return Err(ExperimentError::new("OutcomeUnknown", "RUN outcome unknown"));
        }
        if self.fail_cleanup
            && name == "set_output"
            && args["enabled"] == false
            && self.capture_count > 0
        {
            return Err(ExperimentError::new("Unknown", "output-off unconfirmed"));
        }
        Ok(json!({"completed":true}))
    }
    fn wait_ready(&mut self, b: &RoleBinding, _cancel: &AtomicBool) -> Result<(), ExperimentError> {
        self.calls.push(format!("ready:{}", b.role));
        Ok(())
    }
    fn capture(
        &mut self,
        b: &RoleBinding,
        _params: &Value,
        _cancel: &AtomicBool,
    ) -> Result<WaveformFrame, ExperimentError> {
        self.capture_count += 1;
        self.calls.push(format!("capture:{}", b.role));
        if self.fail_capture == Some(self.capture_count) {
            return Err(ExperimentError::new("ReadFailed", "truncated waveform"));
        }
        if let Some(cancel) = &self.cancel_after_capture {
            cancel.store(true, Ordering::Release);
        }
        Ok(WaveformFrame {
            id: format!("{:032x}", self.capture_count),
            captured_at_unix_ms: 1000 + self.capture_count as u64 * 20,
            nominal_offset_s: None,
            channels: vec![ChannelWaveform {
                channel: "CH1".into(),
                samples: Samples::U16(vec![10, 20, 30]),
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
        })
    }
    fn persist(&mut self, r: &WaveformRecord) -> Result<String, ExperimentError> {
        self.saved.push(r.clone());
        self.calls.push("persist".into());
        Ok(r.id.clone())
    }
    fn frame_completed(&mut self, r: &WaveformRecord) -> Result<(), ExperimentError> {
        self.calls.push("commit".into());
        if self.fail_commit {
            return Err(ExperimentError::new("DataError", "durable frame commit failed"));
        }
        self.committed.push(r.clone());
        if let Some(cancel) = &self.cancel_after_commit {
            cancel.store(true, Ordering::Release);
        }
        Ok(())
    }
    fn analyze(
        &mut self,
        _record: &WaveformRecord,
        _params: &Value,
    ) -> Result<Vec<String>, ExperimentError> {
        self.calls.push("analyze".into());
        Ok(vec!["f".repeat(32)])
    }
    fn wait_interval(&mut self, ms: u64, cancel: &AtomicBool) -> Result<(), ExperimentError> {
        self.calls.push(format!("wait:{ms}"));
        if self.cancel_during_wait {
            cancel.store(true, Ordering::Release);
        }
        if cancel.load(Ordering::Acquire) {
            Err(ExperimentError::new("Cancelled", "cancel confirmed"))
        } else {
            Ok(())
        }
    }
}

fn stopped_definition(resume: Option<bool>) -> ExperimentDefinition {
    let mut d = definition("waveform_capture");
    d.parameters["acquisition"] = json!("stopped");
    d.parameters["source"] = json!("memory");
    d.parameters["memory_depth"] = json!(10_000);
    if let Some(resume) = resume {
        d.parameters["resume_after_capture"] = json!(resume);
    }
    d
}

fn cycle_calls(p: &Port) -> Vec<&str> {
    p.calls.iter().filter(|s| s.starts_with("capture:") || s.as_str() == "commit"
        || s.starts_with("action:detector:run:") || s.starts_with("wait:")).map(String::as_str).collect()
}

#[test]
fn stopped_capture_commits_before_running_scope_and_waiting_for_next_frame() {
    let mut p = Port::default();
    let mut stages = vec![];
    let r = run_experiment(&stopped_definition(None), &mut p, &AtomicBool::new(false), |run| stages.push(run.stage.clone()));
    assert_eq!(r.status, RunStatus::Completed, "{:?}", r.error);
    assert_eq!(cycle_calls(&p), ["capture:detector", "commit", "action:detector:run:{}", "wait:100", "capture:detector", "commit"]);
    assert_eq!(p.committed.len(), 2);
    assert_eq!(p.saved[0].frames.len(), 2);
    let resume = stages.iter().position(|s| s == "resume_scope").unwrap();
    assert_eq!(&stages[resume..resume + 3], ["resume_scope", "wait_interval", "acquire"]);
}

#[test]
fn stopped_capture_final_run_requires_explicit_resume_policy() {
    for (resume, want) in [(None, 0), (Some(false), 0), (Some(true), 1)] {
        let mut d = stopped_definition(resume);
        d.parameters["frames"] = json!(1);
        let mut p = Port::default();
        let r = run_experiment(&d, &mut p, &AtomicBool::new(false), |_| {});
        assert_eq!(r.status, RunStatus::Completed, "{:?}", r.error);
        assert_eq!(p.calls.iter().filter(|s| s.starts_with("action:detector:run:")).count(), want);
        assert_eq!(p.committed.len(), 1);
        assert!(!p.calls.iter().any(|s| s.starts_with("wait:")));
    }
}

#[test]
fn stopped_capture_cancel_after_durable_commit_never_runs_scope() {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut p = Port { cancel_after_commit: Some(cancel.clone()), ..Port::default() };
    let r = run_experiment(&stopped_definition(Some(true)), &mut p, &cancel, |_| {});
    assert_eq!(r.status, RunStatus::Cancelled, "{:?}", r.error);
    assert_eq!(cycle_calls(&p), ["capture:detector", "commit"]);
    assert_eq!(p.committed.len(), 1);
    assert_eq!(p.saved[0].frames.len(), 1);
}

#[test]
fn stopped_capture_cancel_during_interval_retains_completed_run_without_next_capture() {
    let mut p = Port { cancel_during_wait: true, ..Port::default() };
    let r = run_experiment(&stopped_definition(Some(true)), &mut p, &AtomicBool::new(false), |_| {});
    assert_eq!(r.status, RunStatus::Cancelled, "{:?}", r.error);
    assert_eq!(cycle_calls(&p), ["capture:detector", "commit", "action:detector:run:{}", "wait:100"]);
    assert_eq!(r.completed_units, 1);
    assert_eq!(p.saved[0].frames.len(), 1);
    assert_eq!(r.steps.last().unwrap().stage, "run");
    assert_eq!(r.steps.last().unwrap().outcome, "completed");
    assert_eq!(p.saved[0].source["run_status"], "cancelled");
}

#[test]
fn stopped_capture_transfer_or_storage_failure_never_runs_scope() {
    for fail_commit in [false, true] {
        let mut p = Port { fail_commit, fail_capture: (!fail_commit).then_some(1), ..Port::default() };
        let r = run_experiment(&stopped_definition(Some(true)), &mut p, &AtomicBool::new(false), |_| {});
        assert_eq!(r.status, RunStatus::Failed, "{:?}", r.error);
        assert_eq!(r.error.as_ref().unwrap().code, if fail_commit { "DataError" } else { "ReadFailed" });
        assert_eq!(p.capture_count, 1);
        assert!(p.committed.is_empty());
        assert!(!p.calls.iter().any(|s| s.starts_with("action:detector:run:")));
    }
}

#[test]
fn stopped_capture_unknown_run_outcome_retains_durable_frame_and_never_replays() {
    let mut p = Port { fail_run: true, ..Port::default() };
    let r = run_experiment(&stopped_definition(Some(true)), &mut p, &AtomicBool::new(false), |_| {});
    assert_eq!(r.status, RunStatus::Uncertain, "{:?}", r.error);
    assert_eq!(cycle_calls(&p), ["capture:detector", "commit", "action:detector:run:{}"]);
    assert_eq!(r.completed_units, 1);
    assert_eq!(p.committed[0].frames.len(), 1);
    assert_eq!(p.saved[0].frames.len(), 1);
    assert_eq!(r.records, [p.saved[0].id.clone()]);
}

#[test]
fn stopped_capture_policy_is_rejected_outside_stopped_waveform_capture_before_effects() {
    for (template, acquisition, resume) in [
        ("q_scan", Some("stopped"), json!(false)),
        ("fsr_scan", None, json!(true)),
        ("waveform_capture", Some("fresh"), json!(true)),
        ("waveform_capture", Some("stopped"), json!("false")),
        ("waveform_capture", Some("stopped"), Value::Null),
    ] {
        let mut d = definition(template);
        if let Some(acquisition) = acquisition { d.parameters["acquisition"] = json!(acquisition); }
        d.parameters["resume_after_capture"] = resume;
        let mut p = Port::default();
        let r = run_experiment(&d, &mut p, &AtomicBool::new(false), |_| {});
        assert_eq!(r.status, RunStatus::Failed, "{template}: {:?}", r.error);
        assert!(p.calls.is_empty(), "{:?}", p.calls);
    }
}
#[test]
fn missing_role_fails_before_any_instrument_effect() {
    let mut d = definition("q_scan");
    d.roles.retain(|r| r.role != "laser");
    let mut p = Port::default();
    let r = run_experiment(&d, &mut p, &AtomicBool::new(false), |_| {});
    assert_eq!(r.status, RunStatus::Failed);
    assert!(p.calls.is_empty());
}

#[test]
fn existing_screen_is_one_saved_frame_without_experiment_actuation() {
    let mut d = definition("waveform_capture");
    d.parameters["acquisition"] = json!("existing_screen");
    d.parameters["frames"] = json!(1);
    d.parameters["interval_ms"] = json!(0);
    let mut p = Port::default();
    let r = run_experiment(&d, &mut p, &AtomicBool::new(false), |_| {});
    assert_eq!(r.status, RunStatus::Completed, "{:?}", r.error);
    assert_eq!(p.capture_count, 1);
    assert_eq!(p.saved.len(), 1);
    assert!(!p.calls.iter().any(|c| c.starts_with("action:")));
    assert_eq!(p.saved[0].source["parameters"]["acquisition"], "existing_screen");
}

#[test]
fn contradictory_existing_screen_requests_fail_before_any_port_call() {
    for (key, value) in [
        ("source", json!("memory")),
        ("frames", json!(2)),
        ("interval_ms", json!(1)),
        ("memory_depth", json!(1000)),
        ("acquisition", json!("anything")),
        ("acquisition", json!(false)),
        ("acquisition", json!(null)),
    ] {
        let mut d = definition("waveform_capture");
        d.parameters["acquisition"] = json!("existing_screen");
        d.parameters["frames"] = json!(1);
        d.parameters["interval_ms"] = json!(0);
        d.parameters[key] = value;
        let mut p = Port::default();
        let r = run_experiment(&d, &mut p, &AtomicBool::new(false), |_| {});
        assert_eq!(r.status, RunStatus::Failed, "{key}");
        assert!(p.calls.is_empty(), "{key}: {:?}", p.calls);
    }
}

#[test]
fn acquisition_override_is_rejected_for_automated_measurements_before_effects() {
    for template in ["q_scan", "fsr_scan"] {
        let mut d = definition(template);
        d.parameters["acquisition"] = json!("existing_screen");
        d.parameters["frames"] = json!(1);
        d.parameters["interval_ms"] = json!(0);
        let mut p = Port::default();
        let r = run_experiment(&d, &mut p, &AtomicBool::new(false), |_| {});
        assert_eq!(r.status, RunStatus::Failed);
        assert!(p.calls.is_empty());
    }
}

#[test]
fn explicit_fresh_and_legacy_definitions_keep_multi_frame_collection() {
    for acquisition in [None, Some("fresh")] {
        let mut d = definition("waveform_capture");
        if let Some(value) = acquisition { d.parameters["acquisition"] = json!(value); }
        let mut p = Port::default();
        let r = run_experiment(&d, &mut p, &AtomicBool::new(false), |_| {});
        assert_eq!(r.status, RunStatus::Completed, "{:?}", r.error);
        assert_eq!(p.capture_count, 2);
        assert!(!p.calls.iter().any(|s| s.starts_with("action:detector:")));
    }
}

#[test]
fn legacy_automated_measurements_do_not_add_scope_run_cycles() {
    for template in ["q_scan", "fsr_scan"] {
        let mut p = Port::default();
        let r = run_experiment(&definition(template), &mut p, &AtomicBool::new(false), |_| {});
        assert_eq!(r.status, RunStatus::Completed, "{:?}", r.error);
        assert_eq!(p.capture_count, 2);
        assert!(!p.calls.iter().any(|s| s.starts_with("action:detector:")));
        assert!(p.calls.iter().any(|s| s == "analyze"));
    }
}
#[test]
fn missing_capability_fails_before_reservations_or_writes() {
    let mut p = Port {
        missing_cap: true,
        ..Port::default()
    };
    let r = run_experiment(
        &definition("q_scan"),
        &mut p,
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(r.status, RunStatus::Failed);
    assert!(!p
        .calls
        .iter()
        .any(|c| c.starts_with("reserve:") || c.starts_with("action:")));
}
#[test]
fn reservation_failure_releases_prior_roles_without_actuation() {
    let mut p = Port {
        fail_reserve: Some("scan_signal".into()),
        ..Port::default()
    };
    let r = run_experiment(
        &definition("q_scan"),
        &mut p,
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(r.status, RunStatus::Failed);
    assert!(!p
        .calls
        .iter()
        .any(|c| c.starts_with("action:") || c.starts_with("capture:")));
    assert!(p.calls.iter().any(|c| c.starts_with("release:")));
}
#[test]
fn capture_run_persists_actual_times_separately_from_nominal_interval() {
    let mut p = Port::default();
    let mut states = vec![];
    let r = run_experiment(
        &definition("waveform_capture"),
        &mut p,
        &AtomicBool::new(false),
        |r| states.push(r.stage.clone()),
    );
    assert_eq!(r.status, RunStatus::Completed);
    assert_eq!(p.saved.len(), 1);
    assert_eq!(p.saved[0].frames.len(), 2);
    assert_eq!(p.saved[0].frames[1].nominal_offset_s, Some(0.1));
    assert_eq!(p.saved[0].frames[1].captured_at_unix_ms, 1040);
    assert_eq!(r.records, vec![p.saved[0].id.clone()]);
    assert!(states.contains(&"acquire".to_string()));
}
#[test]
fn partial_failure_saves_completed_frames_and_disables_owned_scan_output() {
    let mut p = Port {
        fail_capture: Some(2),
        ..Port::default()
    };
    let r = run_experiment(
        &definition("q_scan"),
        &mut p,
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(r.status, RunStatus::Failed);
    assert_eq!(p.saved.len(), 1);
    assert_eq!(p.saved[0].frames.len(), 1);
    assert!(!p.calls.contains(&"analyze".into()));
    let last = p
        .calls
        .iter()
        .rfind(|s| s.starts_with("action:scan_signal:"))
        .unwrap();
    assert!(last.contains("\"enabled\":false"));
    assert!(!r.records.is_empty());
}
#[test]
fn cancelled_capture_preserves_data_and_never_starts_next_frame() {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut p = Port {
        cancel_after_capture: Some(cancel.clone()),
        ..Port::default()
    };
    let r = run_experiment(&definition("waveform_capture"), &mut p, &cancel, |_| {});
    assert_eq!(r.status, RunStatus::Cancelled);
    assert_eq!(p.capture_count, 1);
    assert_eq!(p.saved[0].frames.len(), 1);
}
#[test]
fn unconfirmed_cleanup_is_uncertain_not_success() {
    let mut p = Port {
        fail_cleanup: true,
        ..Port::default()
    };
    let r = run_experiment(
        &definition("q_scan"),
        &mut p,
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(r.status, RunStatus::Uncertain);
    assert!(!r.cleanup_errors.is_empty());
    assert!(!r.records.is_empty());
}
#[test]
fn unknown_template_and_invalid_parameters_fail_without_instrument_calls() {
    for (template, frames) in [
        ("not_a_template", 2),
        ("waveform_capture", 0),
        ("waveform_capture", 10000),
    ] {
        let mut d = definition("waveform_capture");
        d.template_id = template.into();
        d.parameters["frames"] = json!(frames);
        let mut p = Port::default();
        assert_eq!(
            run_experiment(&d, &mut p, &AtomicBool::new(false), |_| {}).status,
            RunStatus::Failed
        );
        assert!(p.calls.is_empty());
    }
}
#[test]
fn scan_output_is_disabled_before_disk_and_analysis_work() {
    let mut p = Port::default();
    let r = run_experiment(
        &definition("q_scan"),
        &mut p,
        &AtomicBool::new(false),
        |_| {},
    );
    assert_eq!(r.status, RunStatus::Completed);
    let off = p
        .calls
        .iter()
        .rposition(|c| {
            c.starts_with("action:scan_signal:set_output:") && c.contains("\"enabled\":false")
        })
        .unwrap();
    let persist = p.calls.iter().position(|c| c == "persist").unwrap();
    assert!(
        off < persist,
        "scan output should not remain active during potentially slow disk/analysis work: {:?}",
        p.calls
    );
}
#[test]
fn progress_observer_panic_cannot_skip_output_cleanup_or_release() {
    let mut p = Port::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_experiment(
            &definition("q_scan"),
            &mut p,
            &AtomicBool::new(false),
            |run| {
                if run.stage == "stop_scan" {
                    panic!("UI observer disappeared");
                }
            },
        )
    }));
    assert!(result.is_ok(), "progress observer must not unwind cleanup");
    let last = p
        .calls
        .iter()
        .rfind(|s| s.starts_with("action:scan_signal:"))
        .unwrap();
    assert!(last.contains("\"enabled\":false"));
    assert_eq!(
        p.calls.iter().filter(|c| c.starts_with("release:")).count(),
        3
    );
}
