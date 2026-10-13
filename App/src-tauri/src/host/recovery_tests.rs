use super::*;
use serde_json::{json, Value};

struct TestFolder(PathBuf);
impl TestFolder {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("yang-owner-recovery-{}", new_id().unwrap()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestFolder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn original() -> WorkerRecord {
    WorkerRecord {
        version: 2,
        nonce: "a".repeat(32),
        phase: "identified".into(),
        pid: Some(u32::MAX),
        creation_time: Some("00000000000000ff".into()),
        image_path: Some(PathBuf::from("D:/fixed-package/yang-worker.exe")),
        image_sha256: Some("b".repeat(64)),
        package_revision: Some("test-package".into()),
    }
}
fn unknown_cleanup() -> Value {
    json!({"steps":[{"role":"worker","action":"shutdown","ok":false,"error":"No cleanup receipt"}],
        "unreleased":["unknown"],"voltage_zero":null,"resource_release_verified":false})
}
fn install_authorized_fixture(dir: &Path) {
    let original = original();
    let incident = json!({"version":1,"original":original,
        "process_exit":{"confirmed":true,"success":null,"code":null,"source":"recorded_pid_absent"},
        "cleanup":unknown_cleanup(),"resource_released":false,"physical_zero_verified":false});
    let bytes = serde_json::to_vec(&incident).unwrap();
    let incident_dir = dir.join("worker-incidents");
    fs::create_dir(&incident_dir).unwrap();
    fs::write(
        incident_dir.join(format!("{}.json", original.nonce)),
        &bytes,
    )
    .unwrap();
    let receipt = json!({"version":1,"nonce":original.nonce,
        "incident_sha256":super::super::verification::sha256_bytes(&bytes).unwrap(),
        "confirmed":true,"cleanup_unknown_acknowledged":true,"require_readonly_verification":true,
        "resource_released":false,"physical_zero_verified":false});
    fs::write(
        incident_dir.join(format!("{}.recovery.json", original.nonce)),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let mut authorized = original;
    authorized.phase = "recovery_authorized".into();
    fs::write(
        dir.join("worker.json"),
        serde_json::to_vec(&authorized).unwrap(),
    )
    .unwrap();
}

#[test]
fn explicit_recovery_receipt_admits_startup_without_claiming_cleanup() {
    let folder = TestFolder::new();
    install_authorized_fixture(&folder.0);
    assert!(
        verify_record(&folder.0).is_ok(),
        "An acknowledged, validated absent-child incident must admit replacement"
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(folder.0.join("worker.json")).unwrap()).unwrap();
    assert_eq!(saved["phase"], "recovery_authorized");
    assert_ne!(saved["phase"], "released");
}

fn legacy_fixture() -> (TestFolder, StartupRecord) {
    let folder = TestFolder::new();
    let dir = folder.0.join("host");
    fs::create_dir(&dir).unwrap();
    let startup = StartupRecord {
        path: dir.join("worker.json"),
        record: original(),
    };
    startup.persist().unwrap();
    (folder, startup)
}
fn exited_report() -> crate::runtime::StopReport {
    crate::runtime::StopReport {
        cleanup: unknown_cleanup(),
        process_exit: Some(json!({"confirmed":true,"success":false,"code":19})),
        resource_released: false,
    }
}
fn test_guard() -> InstanceGuard {
    InstanceGuard::acquire_named(&format!(
        "Global\\Yang-owner-recovery-test-{}",
        new_id().unwrap()
    ))
    .unwrap()
}

#[test]
fn contradictory_child_exit_does_not_write_an_incident() {
    let (_folder, mut startup) = legacy_fixture();
    let mut report = exited_report();
    report.process_exit = Some(json!({"confirmed":true,"success":true,"code":19}));
    assert!(
        startup.exited_unverified(&report).is_err(),
        "Exit success must agree with the original child exit code"
    );
    assert_eq!(startup.record.phase, "identified");
    assert!(!startup
        .path
        .parent()
        .unwrap()
        .join("worker-incidents")
        .exists());
}

#[test]
fn exited_incident_preserves_first_cleanup_and_keeps_ordinary_startup_fenced() {
    let (_folder, mut startup) = legacy_fixture();
    let report = exited_report();
    startup.exited_unverified(&report).unwrap();
    let dir = startup.path.parent().unwrap();
    let incident_path = dir
        .join("worker-incidents")
        .join(format!("{}.json", startup.record.nonce));
    let first = fs::read(&incident_path).unwrap();
    let incident: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(incident["original"]["phase"], "identified");
    assert_eq!(incident["cleanup"], unknown_cleanup());
    assert_eq!(incident["resource_released"], false);
    assert_eq!(incident["process_exit"]["source"], "owned_child");
    assert!(verify_record(dir).is_err());
    let mut conflicting = report;
    conflicting.process_exit = Some(json!({"confirmed":true,"success":false,"code":20}));
    assert!(startup.exited_unverified(&conflicting).is_err());
    assert_eq!(fs::read(incident_path).unwrap(), first);
    assert_eq!(startup.record.phase, "exited_unverified");
}

#[test]
fn later_cleanup_for_the_same_worker_exit_preserves_the_original_incident() {
    let (folder, mut startup) = legacy_fixture();
    startup.exited_unverified(&exited_report()).unwrap();
    let dir = startup.path.parent().unwrap().to_path_buf();
    let incident_path = dir
        .join("worker-incidents")
        .join(format!("{}.json", startup.record.nonce));
    let first = fs::read(&incident_path).unwrap();
    let mut later = exited_report();
    later.cleanup =
        json!({"steps":[],"unreleased":[],"voltage_zero":null,"resource_release_verified":true});
    later.resource_released = true;
    let receipt = startup
        .exited_unverified(&later)
        .expect("Later cleanup must not obstruct explicit recovery of the same exited worker");
    assert_eq!(receipt["phase"], "exited_unverified");
    assert_eq!(receipt["cleanup_verified"], false);
    assert_eq!(receipt["resource_released"], false);
    assert_eq!(fs::read(&incident_path).unwrap(), first);
    let original: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(original["cleanup"], unknown_cleanup());
    let guard = test_guard();
    recovery::recover_matching_under_guard(
        &dir,
        &folder.0,
        &guard,
        Some(&startup.record.nonce),
        |_| Ok(recovery::ProcessState::Absent),
    )
    .unwrap();
    assert_eq!(fs::read(&incident_path).unwrap(), first);
}

#[test]
fn identified_retry_reuses_original_incident_after_phase_publish_failure() {
    let (_folder, mut startup) = legacy_fixture();
    fs::remove_file(&startup.path).unwrap();
    fs::create_dir(&startup.path).unwrap();
    assert!(startup.exited_unverified(&exited_report()).is_err());
    assert_eq!(startup.record.phase, "identified");
    let incident_path = startup
        .path
        .parent()
        .unwrap()
        .join("worker-incidents")
        .join(format!("{}.json", startup.record.nonce));
    let first = fs::read(&incident_path).unwrap();
    fs::remove_dir(&startup.path).unwrap();
    startup.persist().unwrap();
    let mut later = exited_report();
    later.cleanup["unreleased"] = json!(["later cleanup state"]);
    let receipt = startup
        .exited_unverified(&later)
        .expect("A durable first incident must survive phase publication retry");
    assert_eq!(receipt["phase"], "exited_unverified");
    assert_eq!(startup.record.phase, "exited_unverified");
    assert_eq!(fs::read(&incident_path).unwrap(), first);
}

#[test]
fn orphan_recovery_reuses_owned_child_incident_after_phase_publish_failure() {
    let (folder, mut startup) = legacy_fixture();
    fs::remove_file(&startup.path).unwrap();
    fs::create_dir(&startup.path).unwrap();
    assert!(startup.exited_unverified(&exited_report()).is_err());
    let dir = startup.path.parent().unwrap().to_path_buf();
    let incident_path = dir
        .join("worker-incidents")
        .join(format!("{}.json", startup.record.nonce));
    let first = fs::read(&incident_path).unwrap();
    fs::remove_dir(&startup.path).unwrap();
    startup.persist().unwrap();
    let guard = test_guard();
    let receipt = recovery::recover_matching_under_guard(
        &dir,
        &folder.0,
        &guard,
        Some(&startup.record.nonce),
        |_| Ok(recovery::ProcessState::Absent),
    )
    .expect("Orphan recovery must preserve the already-durable owned-child evidence");
    assert_eq!(receipt["recovery_authorized"], true);
    assert_eq!(receipt["cleanup_verified"], false);
    assert_eq!(fs::read(&incident_path).unwrap(), first);
    let saved: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(saved["process_exit"]["source"], "owned_child");
}

#[test]
fn legacy_absent_child_recovery_records_unknown_exit_status_and_cleanup() {
    let (folder, startup) = legacy_fixture();
    let guard = test_guard();
    let dir = startup.path.parent().unwrap();
    let reply = recovery::recover_under_guard(dir, &folder.0, &guard, |_| {
        Ok(recovery::ProcessState::Absent)
    })
    .unwrap();
    assert_eq!(reply["recovery_authorized"], true);
    assert_eq!(reply["resource_released"], false);
    assert_eq!(reply["require_readonly_verification"], true);
    let saved: Value = serde_json::from_slice(&fs::read(&startup.path).unwrap()).unwrap();
    assert_eq!(saved["phase"], "recovery_authorized");
    let incident: Value = serde_json::from_slice(
        &fs::read(
            dir.join("worker-incidents")
                .join(format!("{}.json", startup.record.nonce)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        incident["process_exit"],
        json!({"confirmed":true,"success":null,"code":null,"source":"recorded_pid_absent"})
    );
    assert_eq!(incident["cleanup"]["resource_release_verified"], false);
    assert_eq!(incident["physical_zero_verified"], false);
    recovery::verify_authorized_with(dir, &serde_json::from_value(saved).unwrap(), |_| {
        Ok(recovery::ProcessState::Absent)
    })
    .unwrap();
}

#[test]
fn recovery_rejects_live_reused_and_inaccessible_processes_without_writes() {
    for case in 0..4 {
        let (folder, startup) = legacy_fixture();
        let guard = test_guard();
        let before = fs::read(&startup.path).unwrap();
        let dir = startup.path.parent().unwrap();
        let result = recovery::recover_under_guard(dir, &folder.0, &guard, |_| match case {
            0 => Ok(recovery::ProcessState::Live("00000000000000ff".into())),
            1 => Ok(recovery::ProcessState::Live("00000000000000aa".into())),
            2 => Ok(recovery::ProcessState::Exited("00000000000000aa".into(), 0)),
            _ => Err(HostError::new(
                "OwnershipUnknown",
                "Process identity access denied",
            )),
        });
        assert!(result.is_err(), "case {case} must remain fenced");
        assert_eq!(fs::read(&startup.path).unwrap(), before);
        assert!(!dir.join("worker-incidents").exists());
    }
}

#[test]
fn recovery_receipt_tampering_and_incident_changes_keep_startup_fenced() {
    for case in 0..5 {
        let folder = TestFolder::new();
        install_authorized_fixture(&folder.0);
        let incident_path = folder
            .0
            .join("worker-incidents")
            .join(format!("{}.json", "a".repeat(32)));
        let receipt_path = folder
            .0
            .join("worker-incidents")
            .join(format!("{}.recovery.json", "a".repeat(32)));
        if case == 0 {
            fs::remove_file(receipt_path).unwrap();
        } else if case == 1 {
            let mut incident: Value =
                serde_json::from_slice(&fs::read(&incident_path).unwrap()).unwrap();
            incident["cleanup"]["unreleased"] = json!([]);
            fs::write(incident_path, serde_json::to_vec(&incident).unwrap()).unwrap();
        } else {
            let mut receipt: Value =
                serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
            receipt[match case {
                2 => "cleanup_unknown_acknowledged",
                3 => "require_readonly_verification",
                _ => "resource_released",
            }] = json!(case == 4);
            fs::write(receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
        }
        assert!(
            verify_record(&folder.0).is_err(),
            "case {case} must remain fenced"
        );
    }
}

#[test]
fn recovery_does_not_modify_operation_journals_or_overwrite_an_incident() {
    let (folder, startup) = legacy_fixture();
    let guard = test_guard();
    let dir = startup.path.parent().unwrap();
    let journal = b"original unknown operation evidence";
    fs::write(dir.join("operations.json"), journal).unwrap();
    recovery::recover_under_guard(dir, &folder.0, &guard, |_| {
        Ok(recovery::ProcessState::Absent)
    })
    .unwrap();
    let incident_path = dir
        .join("worker-incidents")
        .join(format!("{}.json", startup.record.nonce));
    let incident = fs::read(&incident_path).unwrap();
    recovery::recover_under_guard(dir, &folder.0, &guard, |_| {
        Ok(recovery::ProcessState::Absent)
    })
    .unwrap();
    assert_eq!(fs::read(&incident_path).unwrap(), incident);
    assert_eq!(fs::read(dir.join("operations.json")).unwrap(), journal);
}

#[test]
fn recovery_rechecks_process_identity_before_authorization_commit() {
    let (folder, startup) = legacy_fixture();
    let guard = test_guard();
    let checks = std::cell::Cell::new(0);
    let dir = startup.path.parent().unwrap();
    let result = recovery::recover_under_guard(dir, &folder.0, &guard, |_| {
        checks.set(checks.get() + 1);
        Ok(if checks.get() == 1 {
            recovery::ProcessState::Absent
        } else {
            recovery::ProcessState::Live("00000000000000aa".into())
        })
    });
    assert!(result.is_err());
    let saved: Value = serde_json::from_slice(&fs::read(&startup.path).unwrap()).unwrap();
    assert_eq!(saved["phase"], "exited_unverified");
    assert!(!dir
        .join("worker-incidents")
        .join(format!("{}.recovery.json", startup.record.nonce))
        .exists());
}

#[test]
fn unknown_exit_and_malformed_identity_never_create_recovery_evidence() {
    for case in 0..5 {
        let (_folder, mut startup) = legacy_fixture();
        let mut report = exited_report();
        match case {
            0 => report.process_exit = None,
            1 => report.process_exit = Some(json!({"confirmed":false,"success":false,"code":19})),
            2 => startup.record.creation_time = Some("malformed".into()),
            3 => startup.record.image_sha256 = Some("not-a-digest".into()),
            _ => startup.record.pid = None,
        }
        assert!(
            startup.exited_unverified(&report).is_err(),
            "case {case} must remain fenced"
        );
        assert_eq!(startup.record.phase, "identified");
        assert!(!startup
            .path
            .parent()
            .unwrap()
            .join("worker-incidents")
            .exists());
    }
}

#[test]
fn phase_commit_failure_preserves_original_record_and_durable_incident() {
    let (_folder, mut startup) = legacy_fixture();
    fs::remove_file(&startup.path).unwrap();
    fs::create_dir(&startup.path).unwrap();
    assert!(startup.exited_unverified(&exited_report()).is_err());
    assert_eq!(startup.record.phase, "identified");
    let incident = startup
        .path
        .parent()
        .unwrap()
        .join("worker-incidents")
        .join(format!("{}.json", startup.record.nonce));
    let saved: Value = serde_json::from_slice(&fs::read(incident).unwrap()).unwrap();
    assert_eq!(saved["original"]["phase"], "identified");
    assert_eq!(saved["resource_released"], false);
}

#[test]
fn unrelated_unresolved_owner_prevents_explicit_recovery_writes() {
    let (folder, startup) = legacy_fixture();
    let other = folder.0.join("profiles").join("other").join("host");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("worker.json"),
        serde_json::to_vec(&original()).unwrap(),
    )
    .unwrap();
    let guard = test_guard();
    let dir = startup.path.parent().unwrap();
    let before = fs::read(&startup.path).unwrap();
    assert!(
        recovery::recover_under_guard(dir, &folder.0, &guard, |_| Ok(
            recovery::ProcessState::Absent
        ))
        .is_err()
    );
    assert_eq!(fs::read(&startup.path).unwrap(), before);
    assert!(!dir.join("worker-incidents").exists());
}

#[test]
fn recovered_record_never_admits_a_live_or_reused_worker_on_startup() {
    let folder = TestFolder::new();
    install_authorized_fixture(&folder.0);
    let record: WorkerRecord =
        serde_json::from_slice(&fs::read(folder.0.join("worker.json")).unwrap()).unwrap();
    for state in [
        recovery::ProcessState::Live("00000000000000ff".into()),
        recovery::ProcessState::Live("00000000000000aa".into()),
        recovery::ProcessState::Exited("00000000000000aa".into(), 0),
    ] {
        assert!(recovery::verify_authorized_with(&folder.0, &record, |_| Ok(state)).is_err());
    }
    assert!(
        recovery::verify_authorized_with(&folder.0, &record, |_| Err(HostError::new(
            "OwnershipUnknown",
            "access denied"
        )))
        .is_err()
    );
}

#[test]
fn explicit_recovery_requires_operator_acknowledgment_before_machine_ownership() {
    let (folder, startup) = legacy_fixture();
    let before = fs::read(&startup.path).unwrap();
    let error = InstanceGuard::recover_exited(startup.path.parent().unwrap(), false).unwrap_err();
    assert_eq!(error.code, "ConfirmationRequired");
    assert_eq!(fs::read(&startup.path).unwrap(), before);
    assert!(!folder.0.join("host/worker-incidents").exists());
}

#[test]
fn changed_incident_nonce_prevents_recovery_writes_under_the_machine_guard() {
    let (folder, startup) = legacy_fixture();
    let guard = test_guard();
    let dir = startup.path.parent().unwrap();
    let before = fs::read(&startup.path).unwrap();
    let expected_incident = "c".repeat(32);
    let result = recovery::recover_matching_under_guard(
        dir,
        &folder.0,
        &guard,
        Some(&expected_incident),
        |_| panic!("A different incident must be rejected before probing or writing"),
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&startup.path).unwrap(), before);
    assert!(!dir.join("worker-incidents").exists());
}
