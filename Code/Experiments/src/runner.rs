use crate::*;
use serde_json::json;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::{AtomicBool, Ordering},
};
use yang_data::{new_id, unix_ms, WaveformRecord};

fn check(cancel: &AtomicBool) -> Result<(), ExperimentError> {
    if cancel.load(Ordering::Acquire) {
        Err(ExperimentError::new(
            "Cancelled",
            "Stopped at a confirmed experiment step boundary",
        ))
    } else {
        Ok(())
    }
}
fn guarded<T>(call: impl FnOnce() -> Result<T, ExperimentError>) -> Result<T, ExperimentError> {
    catch_unwind(AssertUnwindSafe(call)).unwrap_or_else(|_| {
        Err(ExperimentError::new(
            "OutcomeUnknown",
            "Experiment adapter panicked; reconcile outcomes before retrying",
        ))
    })
}
// Observers are non-authoritative: a dropped UI must never unwind hardware cleanup.
fn notify(run: &RunRecord, progress: &mut impl FnMut(&RunRecord)) {
    let _ = catch_unwind(AssertUnwindSafe(|| progress(run)));
}
fn stage(run: &mut RunRecord, name: &str, progress: &mut impl FnMut(&RunRecord)) {
    run.stage = name.into();
    notify(run, progress);
}
fn action(
    run: &mut RunRecord,
    port: &mut dyn ExperimentPort,
    binding: &RoleBinding,
    name: &str,
    args: &serde_json::Value,
) -> Result<(), ExperimentError> {
    let started = unix_ms();
    let result = guarded(|| port.action(binding, name, args));
    run.steps.push(StepRecord {
        stage: name.into(),
        role: Some(binding.role.clone()),
        started_at_unix_ms: started,
        completed_at_unix_ms: unix_ms(),
        outcome: if result.is_ok() {
            "completed"
        } else {
            "failed"
        }
        .into(),
        detail: match &result {
            Ok(value) => value.clone(),
            Err(error) => json!(error),
        },
    });
    result.map(|_| ())
}
pub fn run_experiment(
    definition: &ExperimentDefinition,
    port: &mut dyn ExperimentPort,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&RunRecord),
) -> RunRecord {
    let identity = new_id();
    let mut run = RunRecord {
        schema: 1,
        id: identity.clone().unwrap_or_default(),
        experiment_id: definition.id.clone(),
        definition: definition.clone(),
        bindings: vec![],
        status: RunStatus::Running,
        stage: "preflight".into(),
        completed_units: 0,
        total_units: definition.parameters["frames"].as_u64().unwrap_or(0),
        started_at_unix_ms: unix_ms(),
        finished_at_unix_ms: None,
        records: vec![],
        analyses: vec![],
        steps: vec![],
        error: None,
        cleanup_errors: vec![],
    };
    let mut reserved: Vec<RoleBinding> = vec![];
    let mut scan_owned = false;
    let mut record: Option<WaveformRecord> = None;
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), ExperimentError> {
        identity?;
        validate_definition(definition)?;
        check(cancel)?;
        notify(&run, &mut progress);
        for binding in &definition.roles {
            check(cancel)?;
            let info = port.inspect(binding)?;
            let expected = match binding.role.as_str() {
                "detector" => "scope",
                "scan_signal" => "generator",
                _ => "laser",
            };
            if !info.connected || info.family != expected || info.config_rev == 0 {
                return Err(ExperimentError::new(
                    "RoleUnavailable",
                    format!(
                        "{} is disconnected or not a compatible {}",
                        binding.role, expected
                    ),
                ));
            }
            let required = match binding.role.as_str() {
                "detector" => vec![if definition.parameters["source"] == "memory" {
                    "capture_memory"
                } else {
                    "capture_screen"
                }],
                "scan_signal" => vec!["triangle", "configure_signal", "set_output"],
                _ => {
                    if definition.parameters["coarse_wavelength_nm"].is_null() {
                        vec![]
                    } else {
                        vec!["tune_wavelength"]
                    }
                }
            };
            if required
                .iter()
                .any(|cap| !info.capabilities.iter().any(|v| v == cap))
            {
                return Err(ExperimentError::new(
                    "MissingCapability",
                    format!("{} lacks a required measurement capability", binding.role),
                ));
            }
            run.bindings.push(RoleSnapshot {
                binding: binding.clone(),
                info,
            });
        }
        let mut order = definition.roles.clone();
        order.sort_by(|a, b| (&a.host_id, &a.domain_id).cmp(&(&b.host_id, &b.domain_id)));
        stage(&mut run, "reserve", &mut progress);
        for binding in order {
            check(cancel)?;
            port.reserve(&binding)?;
            reserved.push(binding);
        }
        let detector = definition
            .roles
            .iter()
            .find(|b| b.role == "detector")
            .unwrap();
        if let Some(laser) = definition.roles.iter().find(|b| b.role == "laser") {
            if !definition.parameters["coarse_wavelength_nm"].is_null() {
                check(cancel)?;
                stage(&mut run, "coarse_tune", &mut progress);
                action(
                    &mut run,
                    port,
                    laser,
                    "set_wavelength",
                    &json!({"wavelength_nm":definition.parameters["coarse_wavelength_nm"],"confirm":true}),
                )?;
                port.wait_ready(laser, cancel)?;
            }
        }
        if let Some(generator) = definition.roles.iter().find(|b| b.role == "scan_signal") {
            check(cancel)?;
            stage(&mut run, "configure_scan", &mut progress);
            // Start explicitly declares selected-channel disable/configure/enable.
            // Page construction and normal driver close never perform these writes.
            scan_owned = true;
            action(
                &mut run,
                port,
                generator,
                "set_output",
                &json!({"channel":generator.channel,"enabled":false}),
            )?;
            check(cancel)?;
            action(
                &mut run,
                port,
                generator,
                "configure_signal",
                &json!({"channel":generator.channel,"waveform":"triangle","frequency_hz":definition.parameters["frequency_hz"],"amplitude_vpp":definition.parameters["amplitude_vpp"],"offset_v":definition.parameters["offset_v"]}),
            )?;
            check(cancel)?;
            action(
                &mut run,
                port,
                generator,
                "set_output",
                &json!({"channel":generator.channel,"enabled":true}),
            )?;
        }
        record = Some(WaveformRecord {
            schema: 1,
            id: new_id()?,
            name: definition.name.clone(),
            comment: String::new(),
            created_at_unix_ms: unix_ms(),
            source: json!({"kind":"experiment","run_id":run.id,"template_id":definition.template_id,"template_version":definition.template_version,"roles":run.bindings,"parameters":definition.parameters}),
            frames: vec![],
            analyses: vec![],
        });
        port.begin_record(record.as_ref().unwrap())?;
        stage(&mut run, "acquire", &mut progress);
        let interval = definition.parameters["interval_ms"].as_u64().unwrap();
        let stopped = definition.template_id == "waveform_capture"
            && definition.parameters["acquisition"] == "stopped";
        let resume_after_capture = definition.parameters["resume_after_capture"] == true;
        for index in 0..run.total_units {
            check(cancel)?;
            let mut frame = port.capture(detector, &definition.parameters, cancel)?;
            frame.nominal_offset_s = Some(index as f64 * interval as f64 / 1000.);
            record.as_mut().unwrap().frames.push(frame);
            port.frame_completed(record.as_ref().unwrap())?;
            run.completed_units += 1;
            notify(&run, &mut progress);
            let more = index + 1 < run.total_units;
            if stopped && (more || resume_after_capture) {
                // frame_completed includes durable storage and Worker staging ACK.
                // A failed/cancelled transfer or commit cannot restart acquisition.
                check(cancel)?;
                stage(&mut run, "resume_scope", &mut progress);
                check(cancel)?;
                action(&mut run, port, detector, "run", &json!({}))?;
            }
            if more {
                stage(&mut run, "wait_interval", &mut progress);
                port.wait_interval(interval, cancel)?;
                stage(&mut run, "acquire", &mut progress);
            }
        }
        check(cancel)?;
        Ok(())
    }));
    let result = match result {
        Ok(result) => result,
        Err(_) => Err(ExperimentError::new(
            "OutcomeUnknown",
            "Experiment adapter panicked; reconcile outcomes before retrying",
        )),
    };
    if let Err(error) = result {
        run.status = if error.code == "Cancelled" {
            RunStatus::Cancelled
        } else if error.uncertain() {
            RunStatus::Uncertain
        } else {
            RunStatus::Failed
        };
        run.error = Some(error);
    } else {
        run.status = RunStatus::Completed;
    }
    // Stop the run-owned scan before any potentially slow storage or fitting.
    stage(&mut run, "stop_scan", &mut progress);
    if scan_owned {
        if let Some(generator) = definition.roles.iter().find(|b| b.role == "scan_signal") {
            if let Err(error) = action(
                &mut run,
                port,
                generator,
                "set_output",
                &json!({"channel":generator.channel,"enabled":false}),
            ) {
                run.cleanup_errors.push(error);
                run.status = RunStatus::Uncertain;
            }
        }
    }
    // Save completed frames even on interruption; partial != completed measurement.
    if let Some(mut data) = record.filter(|r| !r.frames.is_empty()) {
        stage(&mut run, "persist", &mut progress);
        data.source["run_status"] = json!(run.status);
        let capture_only = definition.template_id == "waveform_capture";
        let mut data = Some(data);
        match guarded(|| if capture_only { port.persist_owned(data.take().unwrap()) } else { port.persist(data.as_ref().unwrap()) }) {
            Ok(id) => run.records.push(id),
            Err(error) => {
                run.status = if error.uncertain() {
                    RunStatus::Uncertain
                } else {
                    RunStatus::Failed
                };
                run.error = Some(error);
            }
        }
        if run.status == RunStatus::Completed && !capture_only {
            stage(&mut run, "analyze", &mut progress);
            let mut params = definition.parameters.clone();
            params["template_id"] = json!(definition.template_id);
            match check(cancel).and_then(|_| guarded(|| port.analyze(data.as_ref().unwrap(), &params))) {
                Ok(ids) => run.analyses = ids,
                Err(error) => {
                    run.status = if error.code == "Cancelled" {
                        RunStatus::Cancelled
                    } else if error.uncertain() {
                        RunStatus::Uncertain
                    } else {
                        RunStatus::Failed
                    };
                    run.error = Some(error);
                }
            }
        }
    }
    stage(&mut run, "cleanup", &mut progress);
    for binding in reserved.iter().rev() {
        if let Err(error) = guarded(|| port.release(binding)) {
            run.cleanup_errors.push(error);
        }
    }
    if !run.cleanup_errors.is_empty() {
        run.status = RunStatus::Uncertain;
    }
    run.stage = match run.status {
        RunStatus::Completed => "completed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Uncertain => "uncertain",
        _ => "failed",
    }
    .into();
    run.finished_at_unix_ms = Some(unix_ms());
    notify(&run, &mut progress);
    run
}
