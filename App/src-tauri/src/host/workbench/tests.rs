use super::*;
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
pub(super) fn root() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("yang-workbench-{}", yang_data::new_id().unwrap()))
}

#[test]
fn typed_journal_rejection_precedes_stale_transfer_cleanup() {
    for journal in ["jobs.json", "experiments.json"] {
        let path = root();
        std::fs::create_dir_all(path.join("imports")).unwrap();
        let staging = path.join("imports").join(format!("{}.csv", "a".repeat(32)));
        std::fs::write(&staging, b"unfinished original transfer").unwrap();
        std::fs::write(path.join(journal), b"[{}]").unwrap();
        let result = Workbench::open(&path);
        assert!(result.is_err(), "Malformed {journal} must be rejected before opening Workbench");
        assert_eq!(std::fs::read(staging).unwrap(), b"unfinished original transfer");
        assert_eq!(std::fs::read(path.join(journal)).unwrap(), b"[{}]");
        std::fs::remove_dir_all(path).unwrap();
    }
}
pub(super) fn finished(w: &Workbench, id: &str, owner: &str) -> serde_json::Value {
    for _ in 0..100 {
        let s = w.status(id, owner).unwrap();
        if !matches!(s["status"].as_str(), Some("queued" | "running")) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("job failed to settle")
}
#[test]
fn jobs_are_async_owner_bound_cancelled_and_persisted() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let entered = Arc::new(AtomicBool::new(false));
    let marker = entered.clone();
    let job = w
        .submit("a", "fixture", move |ctx| {
            marker.store(true, Ordering::Release);
            while !ctx.cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            ctx.check()?;
            Ok(json!({}))
        })
        .unwrap();
    let id = job["id"].as_str().unwrap();
    assert!(w.cancel(id, "b").is_err());
    assert!(w.status(id, "b").is_err());
    w.cancel(id, "a").unwrap();
    assert_eq!(finished(&w, id, "a")["status"], "cancelled");
    drop(w);
    let reopened = Workbench::open(&path).unwrap();
    assert_eq!(reopened.status(id, "a").unwrap()["status"], "cancelled");
    drop(reopened);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn job_panic_is_uncertain_and_does_not_kill_executor() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let j = w.submit("a", "panic", |_| panic!("test adapter")).unwrap();
    assert_eq!(
        finished(&w, j["id"].as_str().unwrap(), "a")["status"],
        "uncertain"
    );
    let j = w.submit("a", "ok", |_| Ok(json!({"answer":42}))).unwrap();
    assert_eq!(
        finished(&w, j["id"].as_str().unwrap(), "a")["result"]["answer"],
        42
    );
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn stale_running_jobs_are_not_reported_completed_after_restart() {
    let path = root();
    std::fs::create_dir_all(&path).unwrap();
    let id = "a".repeat(32);
    std::fs::write(path.join("jobs.json"),serde_json::to_vec(&json!([{"id":id,"owner":"a","kind":"analysis","status":"running","stage":"fit","completed_units":0,"total_units":1,"started_at_unix_ms":1,"finished_at_unix_ms":null,"result":null,"error":null}])).unwrap()).unwrap();
    let w = Workbench::open(&path).unwrap();
    assert_eq!(w.status(&id, "a").unwrap()["status"], "uncertain");
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn import_transfer_is_bounded_and_export_keeps_metadata_sidecars() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let bytes = b"Time (us),CH1 (V)\n0,1\n1,2\n2,3\n";
    let upload = w.import_begin("a", "test.csv", bytes.len() as u64).unwrap();
    let id = upload["transfer_id"].as_str().unwrap();
    assert!(w.import_chunk("b", id, 0, "00").is_err());
    assert!(w.import_chunk("a", id, 1, "00").is_err());
    assert!(w.import_commit("a", id, true).is_err());
    let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    w.import_chunk("a", id, 0, &hex).unwrap();
    let job = w.import_commit("a", id, true).unwrap();
    let r = finished(&w, job["id"].as_str().unwrap(), "a");
    assert_eq!(r["status"], "completed", "{r}");
    let record = r["result"]["record_ids"][0].as_str().unwrap();
    let imported = w.store.load(record).unwrap();
    assert_eq!(imported.name, "test", "use the selected file, not its staging UUID");
    assert_eq!(imported.source["original_file_name"], "test.csv");
    assert_eq!(imported.frames[0].captured_at_unix_ms, 0);
    let job = w
        .export("a", &json!({"record_ids":[record],"format":"csv"}))
        .unwrap();
    let r = finished(&w, job["id"].as_str().unwrap(), "a");
    assert_eq!(r["status"], "completed", "{r}");
    let files = r["result"]["files"].as_array().unwrap();
    assert_eq!(
        files.len(),
        2,
        "CSV and provenance sidecar must both reach the selected folder"
    );
    for f in files {
        assert!(
            f.get("path").is_none(),
            "private staging paths must not be published"
        );
        let params = json!({"job_id":job["id"],"file_id":f["file_id"],"offset":0,"length":f["byte_count"].as_u64().unwrap().min(16384)});
        assert!(w.export_chunk("b", &params).is_err());
        assert!(w.export_chunk("a", &params).is_ok());
    }
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn staged_npz_uses_the_selected_file_name_and_native_db_retains_user_labels() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let csv = path.join("source.csv");
    std::fs::write(&csv, b"Time (us),CH1 (V)\n0,1\n1,2\n2,3\n").unwrap();
    let mut source = yang_data::import_path(&csv, &yang_data::ImportLimits::default())
        .unwrap().records.remove(0);
    source.name = "Saved panel sweep".into();
    source.comment = "User comment".into();
    for (format, filename, expected_name) in [
        (yang_data::ExportFormat::Npz, "calibrated.trace.npz", "calibrated.trace"),
        (yang_data::ExportFormat::Native, "archive.db", "Saved panel sweep"),
    ] {
        let report = yang_data::export_records(&[source.clone()], &yang_data::ExportRequest {
            directory: path.join("source-export"), format, frame_ids: vec![],
        });
        let bytes = std::fs::read(report.files[0].path.as_ref().unwrap()).unwrap();
        let upload = w.import_begin("a", filename, bytes.len() as u64).unwrap();
        let id = upload["transfer_id"].as_str().unwrap();
        let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        w.import_chunk("a", id, 0, &hex).unwrap();
        let job = w.import_commit("a", id, true).unwrap();
        let settled = finished(&w, job["id"].as_str().unwrap(), "a");
        assert_eq!(settled["status"], "completed", "{settled}");
        let imported = w.store.load(settled["result"]["record_ids"][0].as_str().unwrap()).unwrap();
        assert_eq!(imported.name, expected_name);
        assert_eq!(imported.source["original_file_name"], filename);
        assert_eq!(imported.frames[0].captured_at_unix_ms, 0);
        assert_eq!(imported.frames[0].channels[0].samples, source.frames[0].channels[0].samples);
        if filename.ends_with(".db") {
            assert_eq!(imported.comment, "User comment");
            assert_eq!(imported.frames, source.frames);
        }
    }
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn abandoned_uploads_are_owner_bound_and_released_without_restart() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let mut ids = vec![];
    for _ in 0..4 {
        ids.push(
            w.import_begin("a", "data.csv", 32).unwrap()["transfer_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert!(w.import_begin("a", "data.csv", 32).is_err());
    assert!(w.import_abort("b", &ids[0]).is_err());
    assert_eq!(w.import_abort("a", &ids[0]).unwrap()["released"], true);
    assert!(w.import_begin("a", "data.csv", 32).is_ok());
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn failed_experiment_run_is_not_mislabeled_as_completed_job() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    for status in ["failed", "cancelled", "uncertain"] {
        let expected = status.to_owned();
        let j = w
            .submit("a", "experiment", move |_| {
                Ok(json!({"run":{"status":expected}}))
            })
            .unwrap();
        let r = finished(&w, j["id"].as_str().unwrap(), "a");
        assert_eq!(r["status"], status);
        assert_eq!(r["result"]["run"]["status"], status);
    }
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn experiment_stays_active_until_terminal_journal_commit() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let job = w
        .submit("a", "experiment", move |_| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(json!({"run":{"status":"failed"}}))
        })
        .unwrap();
    let id = job["id"].as_str().unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    // Hold the real journal writer after admission and let the work return.
    // Any terminal status observed here necessarily precedes its disk commit.
    let writer = w.journal_gate.lock().unwrap();
    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let pending = loop {
        let status = w.status(id, "a").unwrap();
        if status["stage"] != "starting" {
            break status;
        }
        assert!(std::time::Instant::now() < deadline, "work did not return");
        std::thread::yield_now();
    };
    let active = w.active_experiments(Some("a"));
    let cancellable = w.book.lock().unwrap().cancel.contains_key(id);
    drop(writer);

    assert_eq!(pending["status"], "running", "{pending}");
    assert!(pending["finished_at_unix_ms"].is_null());
    assert!(active, "drain must include the pending journal commit");
    assert!(
        cancellable,
        "the job must retain its active budget until commit"
    );
    let settled = finished(&w, id, "a");
    assert_eq!(settled["status"], "failed");
    assert!(!w.active_experiments(Some("a")));
    let saved: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(path.join("jobs.json")).unwrap()).unwrap();
    assert_eq!(saved[0]["status"], "failed");
    assert_eq!(saved[0]["result"]["run"]["status"], "failed");
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn failed_terminal_journal_commit_is_uncertain_and_releases_active_budget() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let job = w
        .submit("a", "experiment", move |_| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(json!({"answer":42}))
        })
        .unwrap();
    let id = job["id"].as_str().unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let blocker = path.join("blocked");
    std::fs::write(&blocker, b"not a directory").unwrap();
    w.book.lock().unwrap().path = blocker.join("jobs.json");
    release_tx.send(()).unwrap();

    let settled = finished(&w, id, "a");
    assert_eq!(settled["status"], "uncertain");
    assert_eq!(settled["stage"], "journal_failed");
    assert!(!settled["error"].is_null());
    assert_eq!(settled["result"]["answer"], 42);
    assert!(!w.active_experiments(Some("a")));
    assert!(w.book.lock().unwrap().cancel.is_empty());
    let saved: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(path.join("jobs.json")).unwrap()).unwrap();
    assert_eq!(saved[0]["status"], "queued");
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn failed_journal_admission_leaves_no_ghost_job_or_cancel_budget() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let original = w.book.lock().unwrap().path.clone();
    let blocker = path.join("blocked");
    std::fs::write(&blocker, b"not a directory").unwrap();
    w.book.lock().unwrap().path = blocker.join("jobs.json");
    assert!(w
        .submit("a", "experiment", |_| panic!(
            "failed admission must never execute"
        ))
        .is_err());
    assert!(!w.active_experiments(None));
    assert!(w.book.lock().unwrap().cancel.is_empty());
    assert!(w.book.lock().unwrap().jobs.is_empty());
    w.book.lock().unwrap().path = original;
    let j = w.submit("a", "fixture", |_| Ok(json!({}))).unwrap();
    assert_eq!(
        finished(&w, j["id"].as_str().unwrap(), "a")["status"],
        "completed"
    );
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn export_release_survives_job_history_pruning() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let dir = path.join("exports").join("a".repeat(32));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("data.csv");
    std::fs::write(&file, b"data").unwrap();
    let job = "b".repeat(32);
    w.downloads.lock().unwrap().insert(
        "c".repeat(32),
        Download {
            owner: "a".into(),
            job_id: job.clone(),
            path: file.clone(),
            size: 4,
            sha256: "0".repeat(64),
        },
    );
    assert!(w.export_release("b", &job).is_err());
    assert_eq!(w.export_release("a", &job).unwrap()["released"], true);
    assert!(!file.exists());
    assert!(w.downloads.lock().unwrap().is_empty());
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn departed_export_owner_releases_staging_and_new_sessions_can_export() {
    let path = root();
    let w = Workbench::open(&path).unwrap();
    let dir = path.join("exports").join("a".repeat(32));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("data.csv");
    std::fs::write(&file, b"data").unwrap();
    let job = "b".repeat(32);
    w.downloads.lock().unwrap().insert(
        "c".repeat(32),
        Download {
            owner: "a".into(),
            job_id: job,
            path: file.clone(),
            size: 4,
            sha256: "0".repeat(64),
        },
    );
    w.cancel_owner("a");
    assert!(w.downloads.lock().unwrap().is_empty());
    assert!(!file.exists());
    drop(w);
    std::fs::remove_dir_all(path).unwrap();
}
