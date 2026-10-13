use super::*;
use yang_data::{Calibration, ChannelWaveform, Samples, WaveformFrame, WaveformRecord};
fn record() -> WaveformRecord {
    WaveformRecord { schema: 1, id: yang_data::new_id().unwrap(), name: "Preview".into(), comment: String::new(), created_at_unix_ms: 1,
        source: json!({"kind":"manual"}), analyses: vec![], frames: vec![WaveformFrame {
            id: yang_data::new_id().unwrap(), captured_at_unix_ms: 1, nominal_offset_s: None,
            channels: vec![ChannelWaveform { channel: "CH1".into(), samples: Samples::U16(vec![0, 100, 65535]),
                calibration: Calibration { x_increment_s: 0.000001, x_origin_s: -0.000002, x_reference: 0.0,
                    y_increment_v: 0.01, y_origin: 0.0, y_reference: 100.0 }, metadata: json!({}) }] }] }
}
fn preview(w: &Workbench, owner: &str) -> String {
    w.previews.reserve(owner, 262).unwrap().publish(record()).unwrap()
}
#[test]
fn preview_save_failure_preserves_samples_and_success_only_updates_labels_on_resave() {
    let path = super::tests::root();
    let w = Workbench::open(&path).unwrap();
    let id = preview(&w, "owner");
    let record_path = w.store.root().to_owned();
    std::fs::remove_dir(&record_path).unwrap();
    std::fs::write(&record_path, b"finite storage failure").unwrap();
    assert!(w.preview_save("owner", &id, "First save", "comment").is_err());
    let original = w.previews.record("owner", &id).unwrap().unwrap();
    assert_eq!(original.frames[0].channels[0].samples, Samples::U16(vec![0, 100, 65535]));
    assert_eq!(w.previews.saved("owner", &id).unwrap(), Some(false));
    std::fs::remove_file(&record_path).unwrap();
    std::fs::create_dir(&record_path).unwrap();
    w.preview_save("owner", &id, "Saved", "first").unwrap();
    let raw: Vec<_> = std::fs::read_dir(record_path.join(&id)).unwrap().flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().starts_with("raw-")).collect();
    w.preview_save("owner", &id, "Renamed", "second").unwrap();
    let new_raw: Vec<_> = std::fs::read_dir(record_path.join(&id)).unwrap().flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().starts_with("raw-")).collect();
    assert_eq!(new_raw, raw, "Repeated Save must not duplicate raw payloads");
    assert_eq!(w.store.metadata(&id).unwrap().name, "Renamed");
    assert_eq!(w.store.metadata(&id).unwrap().comment, "second");
    assert_eq!(w.owner_metadata("owner", &id, 0, 64).unwrap()["saved"], true);
    assert_eq!(w.store.list().unwrap().len(), 1);
    drop(original); drop(w); std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn previews_are_owner_bound_unsaved_not_listed_and_export_without_persistence() {
    let path = super::tests::root(); let w = Workbench::open(&path).unwrap(); let id = preview(&w, "owner");
    assert_eq!(w.list().unwrap()["records"], json!([]));
    assert!(w.owner_metadata("stranger", &id, 0, 64).is_err());
    let metadata = w.owner_metadata("owner", &id, 0, 64).unwrap();
    let params = json!({"record_id":id,"frame_id":metadata["record"]["frames"][0]["id"],"channel":"CH1","start":0,"end":3,"max_points":4});
    assert!(w.owner_viewport("stranger", &params).is_err());
    assert_eq!(w.owner_viewport("owner", &params).unwrap()["points"][2]["y"], 654.35);
    assert!(w.preview_save("stranger", &id, "stolen", "").is_err());
    assert!(w.preview_discard("stranger", &id).is_err());
    assert!(w.export("stranger", &json!({"record_ids":[id],"format":"csv"})).is_err());
    for format in ["csv", "npz", "native"] {
        let job = w.export("owner", &json!({"record_ids":[id],"format":format})).unwrap();
        let job_id = job["job_id"].as_str().unwrap();
        let status = super::tests::finished(&w, job_id, "owner");
        assert_eq!(status["status"], "completed", "{status}");
        assert!(status["result"]["files"].as_array().unwrap().iter().all(|f| f["error"].is_null() && f["byte_count"].as_u64().is_some_and(|n| n > 0)));
        w.export_release("owner", job_id).unwrap();
        assert!(w.store.list().unwrap().is_empty());
    }
    w.preview_save("owner", &id, "Shared historical record", "").unwrap();
    assert_eq!(w.owner_metadata("paired-historical", &id, 0, 64).unwrap()["saved"], true);
    let unsaved = preview(&w, "owner");
    drop(w); let restarted = Workbench::open(&path).unwrap();
    assert_eq!(restarted.list().unwrap()["records"].as_array().unwrap().len(), 1);
    assert!(restarted.owner_metadata("owner", &unsaved, 0, 64).is_err(), "Restart must not recover an unsaved preview as a Record");
    drop(restarted); std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn new_capture_cannot_retarget_save_or_export_of_the_selected_preview() {
    let path = super::tests::root(); let w = Workbench::open(&path).unwrap();
    let selected = preview(&w, "owner");
    let newer = preview(&w, "owner");
    assert_ne!(selected, newer);
    let job = w.export("owner", &json!({"record_ids":[selected],"format":"native"})).unwrap();
    let job_id = job["job_id"].as_str().unwrap();
    w.preview_save("owner", &selected, "Selected trace", "bound identity").unwrap();
    let status = super::tests::finished(&w, job_id, "owner");
    assert_eq!(status["status"], "completed", "{status}");
    assert_eq!(status["result"]["files"][0]["record_id"], selected);
    w.export_release("owner", job_id).unwrap();
    assert_eq!(w.store.list().unwrap().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec![selected.as_str()]);
    assert_eq!(w.owner_metadata("owner", &newer, 0, 64).unwrap()["saved"], false);
    assert!(w.store.metadata(&newer).is_err());
    drop(w); std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn preview_reservations_enforce_aggregate_bytes_count_and_departed_owner_cleanup() {
    let path = super::tests::root(); let w = Workbench::open(&path).unwrap();
    let full = w.previews.reserve("owner", 536_870_912).unwrap();
    assert!(w.previews.reserve("other", 1).is_err());
    drop(full);
    let mut reservations = vec![];
    for _ in 0..32 { reservations.push(w.previews.reserve("owner", 1).unwrap()); }
    assert!(w.previews.reserve("other", 1).is_err());
    w.cancel_owner("owner");
    assert!(reservations.pop().unwrap().publish(record()).is_err());
    drop(reservations);
    let id = preview(&w, "owner");
    let displayed = w.previews.record("owner", &id).unwrap().unwrap();
    assert!(w.preview_discard("owner", &id).is_err(), "Do not free the budget while another task holds samples");
    w.cancel_owner("owner");
    drop(displayed);
    // A departed owner must not leak preview budget after in-flight readers drain.
    let reservation = w.previews.reserve("new-owner", 536_870_912);
    assert!(reservation.is_ok(), "Released historical reader still consumes preview budget");
    drop(reservation); drop(w); std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn cancelled_inflight_capture_keeps_its_memory_reservation_until_actual_drop() {
    let previews = previews::Previews::default();
    let in_flight = previews.reserve("departed", 536_870_912).unwrap();
    previews.cancel_owner("departed");
    assert!(previews.reserve("new-owner", 1).is_err(), "Owner departure must not uncharge in-flight native memory");
    drop(in_flight);
    assert!(previews.reserve("new-owner", 536_870_912).is_ok());
}
#[test]
fn capture_jobs_remain_drainable_without_durable_job_evidence() {
    let path = super::tests::root(); let w = Workbench::open(&path).unwrap();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let job = w.submit("owner", "capture", move |ctx| {
        entered_tx.send(()).unwrap();
        while !ctx.cancelled() { std::thread::sleep(std::time::Duration::from_millis(1)); }
        ctx.check()?; Ok(json!({}))
    }).unwrap();
    entered_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
    assert!(w.active_experiments(Some("owner")));
    assert!(!w.active_experiments(Some("stranger")));
    w.cancel_owner("owner");
    let id = job["job_id"].as_str().unwrap();
    assert_eq!(super::tests::finished(&w, id, "owner")["status"], "cancelled");
    assert!(!w.active_experiments(None));
    let persisted: Value = serde_json::from_slice(&std::fs::read(path.join("jobs.json")).unwrap()).unwrap();
    assert_eq!(persisted, json!([]));
    drop(w); let restarted = Workbench::open(&path).unwrap();
    assert!(restarted.status(id, "owner").is_err());
    drop(restarted); std::fs::remove_dir_all(path).unwrap();
}
