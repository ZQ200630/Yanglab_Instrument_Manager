use serde_json::json;
use std::{io::Write, path::PathBuf};
use yang_data::*;

#[test]
fn name_only_rename_preserves_latest_comment_analysis_and_raw_references() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let rec = record();
    store.save(&rec).unwrap();
    let stale_scope = store.metadata(&rec.id).unwrap();
    store.rename(&rec.id, "Record/Data label", "Newest comment from Record/Data").unwrap();
    let analysis = AnalysisResult {
        schema: 1, id: "3".repeat(32), record_id: rec.id.clone(), frame_id: rec.frames[0].id.clone(), channel: "CH1".into(),
        roi: SampleRange { start: 0, end: 3 }, method: AnalysisMethod::Fsr, algorithm: "fixture-v1".into(),
        parameters: json!({}), result: json!({"fsr_nm":1.0}), curve: vec![], markers: vec![], created_at_unix_ms: 7,
    };
    store.append_analysis(analysis.clone()).unwrap();
    let prior = store.metadata(&rec.id).unwrap();
    let raw = std::fs::read_dir(dir.join(&rec.id)).unwrap().flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with("raw-")).unwrap().path();
    std::fs::remove_file(&raw).unwrap(); // Label updates must never read or rewrite raw data.
    store.rename_name(&stale_scope.id, "New Scope name").unwrap();
    let current = store.metadata(&rec.id).unwrap();
    assert_eq!(current.name, "New Scope name");
    assert_eq!(current.comment, "Newest comment from Record/Data");
    assert_eq!(current.analyses, vec![analysis]);
    assert_eq!(current.source, prior.source);
    assert_eq!(current.frames[0].id, prior.frames[0].id);
    assert_eq!(current.frames[0].channels[0].sample_count, 3);
    assert!(!raw.exists());
    store.rename(&rec.id, "Explicit comment update", "").unwrap();
    assert_eq!(store.metadata(&rec.id).unwrap().comment, "", "Existing rename API must still allow explicitly clearing comments");
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn finalizing_run_source_preserves_operator_edits_analysis_and_raw_refs() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let mut rec = record();
    rec.source = json!({"kind":"experiment","run_id":"a".repeat(32),"parameters":{"frames":2}});
    store.append_frame(&rec, &rec.frames[0]).unwrap();
    store
        .rename(&rec.id, "operator rename", "keep newest comment")
        .unwrap();
    let analysis = AnalysisResult {
        schema: 1,
        id: "3".repeat(32),
        record_id: rec.id.clone(),
        frame_id: rec.frames[0].id.clone(),
        channel: "CH1".into(),
        roi: SampleRange { start: 0, end: 3 },
        method: AnalysisMethod::Fsr,
        algorithm: "fixture-v1".into(),
        parameters: json!({}),
        result: json!({"fsr_nm":1.}),
        curve: vec![],
        markers: vec![],
        created_at_unix_ms: 7,
    };
    store.append_analysis(analysis.clone()).unwrap();
    let raw = std::fs::read_dir(dir.join(&rec.id))
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("raw-"))
        .unwrap()
        .path();
    std::fs::remove_file(raw).unwrap(); // Finalization must not touch immutable payloads.
    rec.source["run_status"] = json!("cancelled");
    store.finalize_record_source(&rec.id, &rec.source).unwrap();
    let saved = store.metadata(&rec.id).unwrap();
    assert_eq!(saved.name, "operator rename");
    assert_eq!(saved.comment, "keep newest comment");
    assert_eq!(saved.source, rec.source);
    assert_eq!(saved.analyses, vec![analysis]);
    rec.source["run_id"] = json!("b".repeat(32));
    assert!(store.finalize_record_source(&rec.id, &rec.source).is_err());
    assert_eq!(
        store.metadata(&rec.id).unwrap().source["run_id"],
        "a".repeat(32)
    );
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn append_frame_preserves_latest_metadata_and_never_rereads_prior_raw() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let rec = record();
    store.append_frame(&rec, &rec.frames[0]).unwrap();
    store
        .rename(&rec.id, "edited during run", "keep comment")
        .unwrap();
    let prior = std::fs::read_dir(dir.join(&rec.id))
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("raw-"))
        .unwrap()
        .path();
    std::fs::remove_file(prior).unwrap(); // Appending must not read any prior payload.
    let mut frame = rec.frames[0].clone();
    frame.id = "f".repeat(32);
    frame.channels[0].samples = Samples::U16(vec![4, 5, 6]);
    store.append_frame(&rec, &frame).unwrap();
    let metadata = store.metadata(&rec.id).unwrap();
    assert_eq!(metadata.frames.len(), 2);
    assert_eq!(metadata.name, "edited during run");
    assert_eq!(metadata.comment, "keep comment");
    let view = store
        .viewport(
            &rec.id,
            &frame.id,
            "CH1",
            SampleRange { start: 0, end: 3 },
            6,
        )
        .unwrap();
    assert_eq!(view.len(), 3);
    assert!(store.append_frame(&rec, &frame).is_err());
    assert_eq!(store.metadata(&rec.id).unwrap().frames.len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn concurrent_analysis_append_preserves_latest_labels_without_reading_raw() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let rec = record();
    store.save(&rec).unwrap();
    store
        .rename(&rec.id, "operator label", "operator comment")
        .unwrap();
    let make = |id: &str| AnalysisResult {
        schema: 1,
        id: id.into(),
        record_id: rec.id.clone(),
        frame_id: rec.frames[0].id.clone(),
        channel: "CH1".into(),
        roi: SampleRange { start: 0, end: 3 },
        method: AnalysisMethod::SingleDrop,
        algorithm: "test-evidence-v1".into(),
        parameters: json!({}),
        result: json!({"q":1.0}),
        curve: vec![],
        markers: vec![],
        created_at_unix_ms: 1,
    };
    let first = make(&"1".repeat(32));
    let second = make(&"2".repeat(32));
    let raw = std::fs::read_dir(dir.join(&rec.id))
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("raw-"))
        .unwrap()
        .path();
    std::fs::remove_file(raw).unwrap(); // Append must touch manifests only.
    let worker = store.clone();
    let a = first.clone();
    let thread = std::thread::spawn(move || worker.append_analysis(a));
    store.append_analysis(second.clone()).unwrap();
    thread.join().unwrap().unwrap();
    let metadata = store.metadata(&rec.id).unwrap();
    assert_eq!(metadata.name, "operator label");
    assert_eq!(metadata.comment, "operator comment");
    assert_eq!(metadata.analyses.len(), 2);
    assert!(metadata.analyses.contains(&first));
    assert!(metadata.analyses.contains(&second));
    let mut invalid = first;
    invalid.id = "3".repeat(32);
    invalid.roi.end = 4;
    assert!(store.append_analysis(invalid).is_err());
    assert_eq!(store.metadata(&rec.id).unwrap().analyses.len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn metadata_and_streaming_viewport_do_not_load_unselected_channel_payload() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let mut rec = record();
    let mut values = vec![32768u16; 1000];
    values[25] = 65535;
    values[72] = 0;
    rec.frames[0].channels[0].samples = Samples::U16(values);
    let mut second = rec.frames[0].channels[0].clone();
    second.channel = "CH2".into();
    second.samples = Samples::U8(vec![1, 2, 3]);
    rec.frames[0].channels.push(second);
    store.save(&rec).unwrap();
    let meta = store.metadata(&rec.id).unwrap();
    let json = serde_json::to_value(&meta).unwrap();
    assert_eq!(json["frames"][0]["channels"][0]["sample_count"], 1000);
    assert!(json["frames"][0]["channels"][0].get("samples").is_none());
    let raw = std::fs::read_dir(dir.join(&rec.id))
        .unwrap()
        .filter_map(Result::ok)
        .find(|p| {
            p.file_name().to_string_lossy().starts_with("raw-") && p.metadata().unwrap().len() < 200
        })
        .unwrap()
        .path();
    let mut bytes = std::fs::read(&raw).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(raw, bytes).unwrap();
    let points = store
        .viewport(
            &rec.id,
            &rec.frames[0].id,
            "CH1",
            SampleRange {
                start: 0,
                end: 1000,
            },
            6,
        )
        .unwrap();
    assert!(points.len() <= 6);
    assert_eq!(points.first().unwrap().x, 0.0);
    assert_eq!(points.last().unwrap().x, 999.0);
    assert!(points
        .iter()
        .any(|p| p.x == 25.0 && (p.y - 31.767).abs() < 1e-9));
    assert!(points
        .iter()
        .any(|p| p.x == 72.0 && (p.y + 33.768).abs() < 1e-9));
    assert!(store.load(&rec.id).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn streaming_viewport_cancellation_is_confirmed() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let rec = record();
    store.save(&rec).unwrap();
    assert!(matches!(
        store.viewport_with_cancel(
            &rec.id,
            &rec.frames[0].id,
            "CH1",
            SampleRange { start: 0, end: 3 },
            6,
            &|| true
        ),
        Err(DataError::Cancelled)
    ));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn metadata_remains_readable_while_a_viewport_is_delayed() {
    use std::sync::{mpsc, Arc, Barrier};
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let rec = record();
    store.save(&rec).unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let worker_store = store.clone();
    let id = rec.id.clone();
    let frame = rec.frames[0].id.clone();
    let e = entered.clone();
    let r = release.clone();
    let worker = std::thread::spawn(move || {
        let calls = std::cell::Cell::new(0);
        worker_store.viewport_with_cancel(
            &id,
            &frame,
            "CH1",
            SampleRange { start: 0, end: 3 },
            6,
            &|| {
                calls.set(calls.get() + 1);
                if calls.get() == 2 {
                    e.wait();
                    r.wait();
                }
                false
            },
        )
    });
    entered.wait();
    let (tx, rx) = mpsc::channel();
    let metadata_store = store.clone();
    let id = rec.id.clone();
    let metadata_worker = std::thread::spawn(move || {
        tx.send(metadata_store.metadata(&id)).unwrap();
    });
    let during = rx.recv_timeout(std::time::Duration::from_millis(200));
    release.wait();
    worker.join().unwrap().unwrap();
    metadata_worker.join().unwrap();
    assert!(during.is_ok(), "metadata waited for an unrelated raw scan");
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn raw_load_can_cancel_before_and_during_binary_read() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let mut rec = record();
    rec.frames[0].channels[0].samples = Samples::U16(vec![123; 50000]);
    store.save(&rec).unwrap();
    assert!(matches!(
        store.load_with_cancel(&rec.id, &|| true),
        Err(DataError::Cancelled)
    ));
    let calls = std::cell::Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() >= 5
    };
    assert!(matches!(
        store.load_with_cancel(&rec.id, &cancel),
        Err(DataError::Cancelled)
    ));
    std::fs::remove_dir_all(dir).unwrap();
}
fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!("yang-data-test-{}", new_id().unwrap()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn record() -> WaveformRecord {
    WaveformRecord {
        schema: 1,
        id: "11111111111111111111111111111111".into(),
        name: "duplicate / name".into(),
        comment: "raw evidence".into(),
        created_at_unix_ms: 5,
        source: json!({"model":"fixture"}),
        analyses: vec![],
        frames: vec![WaveformFrame {
            id: "22222222222222222222222222222222".into(),
            captured_at_unix_ms: 6,
            nominal_offset_s: Some(0.1),
            channels: vec![ChannelWaveform {
                channel: "CH1".into(),
                samples: Samples::U16(vec![0, 32768, 65535]),
                calibration: Calibration {
                    x_increment_s: 1e-6,
                    x_origin_s: 2e-6,
                    x_reference: 1.0,
                    y_increment_v: 0.001,
                    y_origin: -1.0,
                    y_reference: 32768.0,
                },
                metadata: json!({"original_preamble":"kept"}),
            }],
        }],
    }
}
fn npy_u8() -> Vec<u8> {
    let header = "{'descr': '|u1', 'fortran_order': False, 'shape': (3,), }\n";
    let mut b = b"\x93NUMPY\x01\x00".to_vec();
    b.extend((header.len() as u16).to_le_bytes());
    b.extend(header.as_bytes());
    b.extend([0, 128, 255]);
    b
}
fn archive(path: &std::path::Path, entries: &[(&str, Vec<u8>)]) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    for (name, data) in entries {
        z.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
}
#[test]
fn native_round_trip_keeps_dtype_raw_evidence_and_committed_labels() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let rec = record();
    store.save(&rec).unwrap();
    assert_eq!(store.load(&rec.id).unwrap(), rec);
    store.rename(&rec.id, "renamed", "comment").unwrap();
    let loaded = store.load(&rec.id).unwrap();
    assert_eq!(loaded.name, "renamed");
    assert_eq!(loaded.frames, rec.frames);
    assert_eq!(store.list().unwrap().len(), 1);
    assert!(store.load("../escape").is_err());
    store.remove(&rec.id).unwrap();
    assert!(store.list().unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn legacy_unsigned_npy_header_overrides_conflicting_dtype_and_normalizes_origin() {
    let dir = temp();
    let path = dir.join("old.db");
    let db = json!({"records":[{"name":"old","comment":"ok","source":"Rigol","created_time":"2026-01-01","metadata":{"CH1":{"format":"BYTE","dtype":"int8","points":3,"count":3,"xincrement":1e-6,"xorigin":0.0,"xreference":0,"yincrement":0.1,"yorigin":2.0,"yreference":128}},"frames":[{"time_offset":0.0,"channels":{"CH1":"raw.npy"}}]}]});
    archive(
        &path,
        &[
            ("database.json", serde_json::to_vec(&db).unwrap()),
            ("raw.npy", npy_u8()),
        ],
    );
    let report = import_path(&path, &ImportLimits::default()).unwrap();
    assert_eq!(
        report.records[0].frames[0].channels[0].samples,
        Samples::U8(vec![0, 128, 255])
    );
    assert_eq!(
        report.records[0].frames[0].channels[0]
            .calibration
            .voltage_v(128.0),
        -0.2
    );
    assert_eq!(report.warnings.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn traversal_archive_is_rejected_without_extraction() {
    let dir = temp();
    let path = dir.join("evil.db");
    archive(&path, &[("../outside.npy", npy_u8())]);
    assert!(import_path(&path, &ImportLimits::default()).is_err());
    assert!(!dir.parent().unwrap().join("outside.npy").exists());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn calibrated_csv_npz_round_trip_and_collision_safe_batch_reports() {
    let dir = temp();
    let rec = record();
    for format in [ExportFormat::Csv, ExportFormat::Npz] {
        let req = ExportRequest {
            directory: dir.clone(),
            format,
            frame_ids: vec![],
        };
        let first = export_records(&[rec.clone()], &req);
        assert_eq!(first.files.len(), 1);
        assert!(first.files[0].error.is_none(), "{:?}", first);
        let path = first.files[0].path.as_ref().unwrap();
        let imported = import_path(path, &ImportLimits::default()).unwrap();
        let ch = &imported.records[0].frames[0].channels[0];
        assert_eq!(ch.samples.value(1), Some(-1.0));
        assert!((ch.calibration.time_s(2) - 3e-6).abs() < 1e-15);
        let second = export_records(&[rec.clone()], &req);
        assert_ne!(first.files[0].path, second.files[0].path);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn bounded_import_cancellation_and_unknown_or_object_arrays_are_errors() {
    let dir = temp();
    let path = dir.join("bounded.npz");
    archive(&path, &[("time_us.npy", npy_u8())]);
    let mut limits = ImportLimits::default();
    limits.max_uncompressed_bytes = 1;
    assert!(import_path(&path, &limits).is_err());
    assert_eq!(
        import_path_with_cancel(&path, &ImportLimits::default(), &|| true).unwrap_err(),
        DataError::Cancelled
    );
    let csv = dir.join("bad.csv");
    std::fs::write(&csv, "Time (us),CH1 (V)\n0,1\n1,NaN\n").unwrap();
    assert!(import_path(&csv, &ImportLimits::default()).is_err());
    let csv = dir.join("irregular.csv");
    std::fs::write(&csv, "Time (us),CH1 (V)\n0,1\n1,2\n3,3\n").unwrap();
    assert!(import_path(&csv, &ImportLimits::default()).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn unequal_axes_export_reports_csv_error_and_npz_keeps_each_channel_axis() {
    let dir = temp();
    let mut rec = record();
    let mut second = rec.frames[0].channels[0].clone();
    second.channel = "CH2".into();
    second.samples = Samples::U8(vec![1, 2]);
    second.calibration.x_origin_s = 12e-6;
    rec.frames[0].channels.push(second);
    let csv = export_records(
        &[rec.clone()],
        &ExportRequest {
            directory: dir.clone(),
            format: ExportFormat::Csv,
            frame_ids: vec![],
        },
    );
    assert_eq!(csv.files.len(), 1);
    assert!(csv.files[0].error.is_some());
    let npz = export_records(
        &[rec.clone()],
        &ExportRequest {
            directory: dir.clone(),
            format: ExportFormat::Npz,
            frame_ids: vec![],
        },
    );
    let loaded = import_path(
        npz.files[0].path.as_ref().unwrap(),
        &ImportLimits::default(),
    )
    .unwrap();
    assert_eq!(loaded.records[0].frames[0].channels[1].samples.len(), 2);
    assert!(
        (loaded.records[0].frames[0].channels[1]
            .calibration
            .time_s(0)
            - 11e-6)
            .abs()
            < 1e-15
    );
    std::fs::remove_dir_all(dir).unwrap();
}
fn custom_npy(dtype: &str, shape: &str, data: &[u8]) -> Vec<u8> {
    let header = format!("{{'descr': '{dtype}', 'fortran_order': False, 'shape': {shape}, }}\n");
    let mut bytes = b"\x93NUMPY\x01\x00".to_vec();
    bytes.extend((header.len() as u16).to_le_bytes());
    bytes.extend(header.as_bytes());
    bytes.extend(data);
    bytes
}
#[test]
fn archive_preflight_rejects_duplicate_members_before_dtype_read() {
    let dir = temp();
    let path = dir.join("dupe.db");
    archive(&path, &[("x.npy", npy_u8()), ("y.npy", npy_u8())]);
    let mut bytes = std::fs::read(&path).unwrap();
    for i in 0..bytes.len() - 5 {
        if &bytes[i..i + 5] == b"y.npy" {
            bytes[i] = b'x';
        }
    }
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        import_path(&path, &ImportLimits::default()),
        Err(DataError::Invalid(_))
    ));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn npy_object_multidimensional_and_payload_mismatch_are_rejected() {
    let dir = temp();
    for (label, array) in [
        ("object", custom_npy("|O8", "(1,)", &[0; 8])),
        ("matrix", custom_npy("|u1", "(1,3)", &[0, 1, 2])),
        ("truncated", custom_npy("<f8", "(3,)", &[0; 8])),
        ("trailing", custom_npy("|u1", "(2,)", &[1, 2, 3])),
    ] {
        let path = dir.join(format!("{label}.npz"));
        archive(&path, &[("channel_names.npy", array)]);
        assert!(
            import_path(&path, &ImportLimits::default()).is_err(),
            "{label}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn interrupted_save_preserves_last_commit_and_raw_mutation_is_rejected() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let old = record();
    store.save(&old).unwrap();
    let mut next = old.clone();
    next.name = "uncommitted".into();
    let checks = std::cell::Cell::new(0);
    let cancelled = || {
        checks.set(checks.get() + 1);
        checks.get() >= 3
    };
    assert_eq!(
        store.save_with_cancel(&next, &cancelled).unwrap_err(),
        DataError::Cancelled
    );
    assert_eq!(store.load(&old.id).unwrap(), old);
    next.frames[0].channels[0].samples = Samples::U16(vec![1, 2, 3]);
    assert!(matches!(store.save(&next), Err(DataError::Conflict(_))));
    assert_eq!(store.load(&old.id).unwrap(), old);
    std::fs::write(dir.join(&old.id).join(".interrupted.part"), "incomplete").unwrap();
    assert_eq!(DataStore::new(&dir).unwrap().load(&old.id).unwrap(), old);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn native_analysis_metadata_reloads_and_corrupt_raw_bytes_fail() {
    let dir = temp();
    let store = DataStore::new(&dir).unwrap();
    let mut rec = record();
    rec.analyses.push(AnalysisResult {
        schema: 1,
        id: "33333333333333333333333333333333".into(),
        record_id: rec.id.clone(),
        frame_id: rec.frames[0].id.clone(),
        channel: "CH1".into(),
        roi: SampleRange { start: 0, end: 3 },
        method: AnalysisMethod::Fsr,
        algorithm: "fixture-provenance-v1".into(),
        parameters: json!({"scan_speed_nm_s":1.0}),
        result: json!({"fsr_nm":0.1}),
        curve: vec![],
        markers: vec![PlotPoint { x: 1.0, y: -1.0 }],
        created_at_unix_ms: 7,
    });
    store.save(&rec).unwrap();
    assert_eq!(store.load(&rec.id).unwrap().analyses, rec.analyses);
    let raw = std::fs::read_dir(dir.join(&rec.id))
        .unwrap()
        .filter_map(Result::ok)
        .find(|p| p.file_name().to_string_lossy().starts_with("raw-"))
        .unwrap()
        .path();
    let mut bytes = std::fs::read(&raw).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(raw, bytes).unwrap();
    assert!(matches!(store.load(&rec.id), Err(DataError::Invalid(_))));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn import_budgets_reject_decoded_float32_expansion_before_allocation() {
    let dir = temp();
    let path = dir.join("float32.npz");
    let names = custom_npy(
        "<U3",
        "(1,)",
        &[b'C', 0, 0, 0, b'H', 0, 0, 0, b'1', 0, 0, 0],
    );
    let raw: Vec<u8> = (0..1000).flat_map(|i| (i as f32).to_le_bytes()).collect();
    let times = custom_npy("<f4", "(1000,)", &raw);
    archive(
        &path,
        &[
            ("channel_names.npy", names),
            ("time_us.npy", times.clone()),
            ("CH1.npy", times),
        ],
    );
    let mut limits = ImportLimits::default();
    limits.max_member_bytes = 5000;
    assert!(matches!(
        import_path(&path, &limits),
        Err(DataError::Limit(_))
    ));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn native_db_export_is_self_contained_lossless_and_reloads_analysis() {
    let dir = temp();
    let mut rec = record();
    rec.analyses.push(AnalysisResult {
        schema: 1,
        id: "33333333333333333333333333333333".into(),
        record_id: rec.id.clone(),
        frame_id: rec.frames[0].id.clone(),
        channel: "CH1".into(),
        roi: SampleRange { start: 0, end: 3 },
        method: AnalysisMethod::Fsr,
        algorithm: "fixture-v1".into(),
        parameters: json!({}),
        result: json!({"fsr_nm":1.0}),
        curve: vec![],
        markers: vec![],
        created_at_unix_ms: 7,
    });
    let exported = export_records(
        &[rec.clone()],
        &ExportRequest {
            directory: dir.clone(),
            format: ExportFormat::Native,
            frame_ids: vec![],
        },
    );
    let path = exported.files[0].path.as_ref().unwrap();
    assert!(path.is_file());
    assert_eq!(path.extension().unwrap(), "db");
    let imported = import_path(path, &ImportLimits::default()).unwrap();
    assert_eq!(imported.records[0].frames, rec.frames);
    assert_eq!(imported.records[0].analyses, rec.analyses);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn malformed_or_duplicate_npy_header_fields_are_not_accepted_as_real_arrays() {
    let dir = temp();
    let names = custom_npy(
        "<U3",
        "(1,)",
        &[b'C', 0, 0, 0, b'H', 0, 0, 0, b'1', 0, 0, 0],
    );
    let raw: Vec<u8> = [0.0f64, 1.0, 2.0]
        .into_iter()
        .flat_map(f64::to_le_bytes)
        .collect();
    let time = custom_npy("<f8", "(3,)", &raw);
    let badtuple = custom_npy("<f8", "(3)", &raw);
    let header = "{'descr': '<f8', 'descr': '<f8', 'fortran_order': False, 'shape': (3,), }\n";
    let mut duplicate = b"\x93NUMPY\x01\x00".to_vec();
    duplicate.extend((header.len() as u16).to_le_bytes());
    duplicate.extend(header.as_bytes());
    duplicate.extend(&raw);
    for (label, array) in [("tuple", badtuple), ("duplicate", duplicate)] {
        let path = dir.join(format!("{label}.npz"));
        archive(
            &path,
            &[
                ("channel_names.npy", names.clone()),
                ("time_us.npy", time.clone()),
                ("CH1.npy", array),
            ],
        );
        assert!(
            matches!(
                import_path(&path, &ImportLimits::default()),
                Err(DataError::Invalid(_))
            ),
            "{label}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
