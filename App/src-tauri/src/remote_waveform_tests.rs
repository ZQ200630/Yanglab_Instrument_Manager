//! Historical data through the actual authenticated Host/TLS boundary.
//! Native startup and the close-drain connection use the private finite worker
//! fixture. No real instrument is enumerated, connected or actuated.
use super::*;
use crate::remote_client::{RemoteHostClient, RemotePeer};
use yang_data::{Calibration, ChannelWaveform, Samples, WaveformFrame, WaveformRecord};

fn request(method: &str, params: Value) -> HostRequest {
    HostRequest {
        v: 1,
        id: new_id().unwrap(),
        method: method.into(),
        params,
    }
}

async fn call(client: &RemoteHostClient, method: &str, params: Value) -> Value {
    let reply = client.call(request(method, params)).await.unwrap();
    assert!(reply.ok, "{method}: {:?}", reply.error);
    reply.result
}

async fn denied(client: &RemoteHostClient, method: &str, params: Value) {
    let reply = client.call(request(method, params)).await.unwrap();
    assert!(
        !reply.ok,
        "{method} unexpectedly crossed the ownership boundary"
    );
    assert!(reply.result.is_null());
}

fn history() -> WaveformRecord {
    // A fixed finite sequence keeps the compressed export larger than one chunk.
    let mut seed = 0x7269_676fu32;
    let mut codes = (0..32_768)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u16
        })
        .collect::<Vec<_>>();
    codes[0] = 0;
    codes[32_767] = 65_535;
    WaveformRecord {
        schema: 1,
        id: "1".repeat(32),
        name: "Literal historical unsigned waveform".into(),
        comment: "Native hand-authored fixture, not a physical measurement".into(),
        created_at_unix_ms: 100,
        source: json!({"kind":"imported_history","hardware_authority":false}),
        frames: vec![WaveformFrame {
            id: "2".repeat(32),
            captured_at_unix_ms: 101,
            nominal_offset_s: None,
            channels: vec![ChannelWaveform {
                channel: "CH1".into(),
                samples: Samples::U16(codes),
                calibration: Calibration {
                    x_increment_s: 1e-6,
                    x_origin_s: -0.001,
                    x_reference: 100.,
                    y_increment_v: 0.01,
                    y_origin: -1.,
                    y_reference: 0.,
                },
                metadata: json!({"fixture":"literal historical raw codes"}),
            }],
        }],
        analyses: vec![],
    }
}

struct ExportCopy(PathBuf);
impl Drop for ExportCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn listener(
    core: Arc<HostCore>,
) -> (
    RemotePeer,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = socket.local_addr().unwrap().to_string();
    let peer = {
        let mut trust = core.remote.lock().unwrap();
        let peer_id = "b".repeat(32);
        let code = trust.begin_pairing().unwrap();
        let ticket = trust
            .request_pair(&peer_id, "Historical data fixture", &code)
            .unwrap();
        trust.approve(&ticket).unwrap();
        RemotePeer {
            host_id: trust.host_id().into(),
            name: "Finite historical Host".into(),
            endpoint,
            fingerprint: trust.fingerprint().unwrap(),
            peer_id,
            credential: trust.take_approved(&ticket).unwrap().unwrap(),
        }
    };
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                accepted = socket.accept() => {
                    let (tcp, source) = accepted.unwrap();
                    let owner = core.clone();
                    connections.spawn(async move {
                        serve_remote(tcp, owner, 0, source.ip(), Arc::new(Semaphore::new(2))).await
                    });
                }
                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    // Rejected credentials and closed/revoked streams are expected.
                    assert!(result.is_ok(), "TLS Host task panicked: {result:?}");
                }
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    });
    (peer, stop, server)
}

#[test]
fn encrypted_peer_reads_large_historical_viewport_and_owner_scoped_export_chunks() {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            let fixture = native_osa_flow_tests::Fixture::new(true);
            let core = fixture.core().clone();
            let record = history();
            core.workbench.as_ref().unwrap().store.save(&record).unwrap();
            let (peer, stop, server) = listener(core.clone()).await;
            let control = RemoteHostClient::connect(peer.clone()).await.unwrap();
            let results = control.attached("results").await.unwrap();
            let status = control.attached("status").await.unwrap();
            let safety = control.attached("safety").await.unwrap();
            let stranger = RemoteHostClient::connect(peer.clone()).await.unwrap();
            let stranger_results = stranger.attached("results").await.unwrap();
            let stranger_safety = stranger.attached("safety").await.unwrap();

            let metadata = call(&results, "data_get", json!({"record_id":record.id,"frame_offset":0,"frame_limit":64})).await;
            assert_eq!(metadata["record"]["frames"][0]["channels"][0]["points"], 32_768);
            assert!(metadata["record"]["frames"][0]["channels"][0].get("samples").is_none());
            let viewport = call(&results, "data_viewport", json!({
                "record_id":record.id,"frame_id":record.frames[0].id,"channel":"CH1",
                "start":0,"end":32_768,"max_points":8192
            })).await;
            assert!(serde_json::to_vec(&viewport).unwrap().len() > 65_536,
                "fixture must cross the former ordinary reply ceiling");
            let points = viewport["points"].as_array().unwrap();
            assert!(points.len() <= 8192);
            assert_eq!(points.first().unwrap()["x"], 0.);
            assert_eq!(points.first().unwrap()["y"], -1.);
            assert_eq!(points.last().unwrap()["x"], 32_767.);
            assert!((points.last().unwrap()["y"].as_f64().unwrap() - 654.35).abs() < 1e-10);
            assert_eq!(viewport["total_points"], 32_768);
            // Status remains a distinct authenticated stream, not a bulk channel.
            denied(&status, "data_get", json!({"record_id":record.id})).await;

            let accepted = call(&results, "data_export", json!({
                "record_ids":[record.id],"format":"db"
            })).await;
            let job = accepted["job_id"].as_str().unwrap();
            let export = loop {
                let current = call(&status, "analysis_status", json!({"job_id":job})).await;
                if !matches!(current["status"].as_str(), Some("queued" | "running")) {
                    assert_eq!(current["status"], "completed", "{current}");
                    break current["result"].clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            let files = export["files"].as_array().unwrap();
            assert_eq!(files.len(), 1);
            let file = &files[0];
            assert!(file["error"].is_null(), "{file}");
            assert!(file.get("path").is_none(), "private staging paths must stay on their owning Host");
            let file_id = file["file_id"].as_str().unwrap();
            let size = file["byte_count"].as_u64().unwrap();
            assert!(size > 16_384, "export fixture must require multiple bounded chunks");
            let read = |offset, length| json!({"job_id":job,"file_id":file_id,"offset":offset,"length":length});
            denied(&stranger_results, "data_export_chunk", read(0, 16)).await;
            denied(&stranger_safety, "data_export_release", json!({"job_id":job})).await;
            denied(&results, "data_export_chunk", read(0, 16_385)).await;
            denied(&results, "data_export_chunk", read(size, 1)).await;

            let mut raw = Vec::new();
            let mut offset = 0;
            while offset < size {
                let length = (size - offset).min(16_384);
                let chunk = call(&results, "data_export_chunk", read(offset, length)).await;
                assert_eq!(chunk["offset"], offset);
                assert_eq!(chunk["length"], length);
                assert_eq!(chunk["sha256"], file["sha256"]);
                let hex = chunk["data_hex"].as_str().unwrap();
                assert_eq!(hex.len(), length as usize * 2);
                raw.extend(hex.as_bytes().chunks_exact(2).map(|pair| {
                    u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()
                }));
                offset += length;
            }
            assert_eq!(&raw[..4], b"PK\x03\x04");
            assert_eq!(raw.len() as u64, size);
            assert_eq!(crate::host::verification::sha256_bytes(&raw).unwrap(), file["sha256"]);
            let downloaded = ExportCopy(std::env::temp_dir().join(format!("yang-tls-history-{}.db", new_id().unwrap())));
            std::fs::write(&downloaded.0, &raw).unwrap();
            let imported = yang_data::import_path(&downloaded.0, &yang_data::ImportLimits::default()).unwrap();
            assert_eq!(imported.records.len(), 1);
            let recovered = &imported.records[0].frames[0].channels[0];
            assert_eq!(recovered.samples, record.frames[0].channels[0].samples);
            assert_eq!(recovered.calibration, record.frames[0].channels[0].calibration);
            assert_eq!(imported.records[0].source["hardware_authority"], false);
            assert_eq!(call(&safety, "data_export_release", json!({"job_id":job})).await["released"], true);
            denied(&results, "data_export_chunk", read(0, 1)).await;
            assert_eq!(core.registry.lock().unwrap().devices.len(), 0);
            assert_eq!(core.workbench.as_ref().unwrap().store.load(&record.id).unwrap().frames[0].channels[0].samples.value(32_767), Some(65_535.));

            call(&safety, "close_client", json!({})).await;
            call(&stranger_safety, "close_client", json!({})).await;
            drop((control, results, status, safety, stranger, stranger_results, stranger_safety));
            stop.send(()).unwrap();
            server.await.unwrap();
        }).await.expect("finite historical TLS flow exceeded its test deadline");
    });
}

#[test]
fn encrypted_host_rejects_untrusted_credentials_and_remote_import_or_experiment_authority() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(20), async {
                let fixture = native_osa_flow_tests::Fixture::new(true);
                let core = fixture.core().clone();
                let (peer, stop, server) = listener(core.clone()).await;
                let mut untrusted = peer.clone();
                untrusted.credential = "0".repeat(64);
                assert!(RemoteHostClient::connect(untrusted).await.is_err());
                let mut tls = crate::remote::connect_tls(&peer.endpoint, &peer.fingerprint)
                    .await
                    .unwrap();
                write_frame(
                    &mut tls,
                    &request(
                        "remote_auth",
                        json!({
                            "peer_id":peer.peer_id,"credential":peer.credential,"join":{}
                        }),
                    ),
                )
                .await
                .unwrap();
                let authenticated: HostReply =
                    serde_json::from_slice(&read_frame(&mut tls).await.unwrap().unwrap()).unwrap();
                assert!(authenticated.ok, "{:?}", authenticated.error);
                // Send directly to the real server so the client allowlist cannot make
                // these assertions pass while the owning Host admits forbidden work.
                for (method, params) in [
                    (
                        "data_import_begin",
                        json!({"name":"unapproved.csv","byte_count":10}),
                    ),
                    (
                        "data_import_chunk",
                        json!({"transfer_id":"1".repeat(32),"offset":0,"data_hex":"00"}),
                    ),
                    (
                        "data_import_commit",
                        json!({"transfer_id":"1".repeat(32),"append":true}),
                    ),
                    ("experiment_start", json!({"definition":{}})),
                    ("capture_start", json!({"definition":{}})),
                    ("preview_save", json!({"record_id":"1".repeat(32),"name":"unapproved","comment":""})),
                    ("preview_discard", json!({"record_id":"1".repeat(32)})),
                    ("experiment_save", json!({"definition":{}})),
                    ("save_settings", json!({})),
                ] {
                    let denied = request(method, params);
                    let id = denied.id.clone();
                    write_frame(&mut tls, &denied).await.unwrap();
                    let reply: HostReply =
                        serde_json::from_slice(&read_frame(&mut tls).await.unwrap().unwrap())
                            .unwrap();
                    assert_eq!(reply.id, id);
                    assert!(!reply.ok, "Host admitted {method}");
                    assert_eq!(reply.error.unwrap().code, "SessionRevoked", "{method}");
                }
                assert!(core
                    .workbench
                    .as_ref()
                    .unwrap()
                    .store
                    .list()
                    .unwrap()
                    .is_empty());
                assert!(!core.workbench.as_ref().unwrap().active_experiments(None));
                assert!(core.registry.lock().unwrap().devices.is_empty());
                drop(tls);
                stop.send(()).unwrap();
                server.await.unwrap();
            })
            .await
            .expect("finite TLS authority flow exceeded its test deadline");
        });
}

#[test]
fn client_close_waits_for_cancelled_experiment_boundary_before_revoking_its_device_lease() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(20), async {
                let fixture = native_osa_flow_tests::Fixture::new(true);
                // This is the existing finite, explicitly injected OSA transport. The
                // connection is read-only and never reaches a real VISA resource.
                let (domain, lease) = fixture.osa().await;
                let core = fixture.core().clone();
                let owner = fixture.local.session.clone();
                let workbench = core.workbench.as_ref().unwrap().clone();
                let entered = Arc::new(AtomicBool::new(false));
                let observed_cancel = Arc::new(AtomicBool::new(false));
                let held_at_cancel = Arc::new(AtomicBool::new(false));
                let finish_boundary = Arc::new(AtomicBool::new(false));
                let (started, cancelled, held, finish) = (
                    entered.clone(),
                    observed_cancel.clone(),
                    held_at_cancel.clone(),
                    finish_boundary.clone(),
                );
                let (task_core, task_owner, task_domain) =
                    (core.clone(), owner.clone(), domain.clone());
                let accepted = workbench
                    .submit(owner.id(), "experiment", move |ctx| {
                        started.store(true, Ordering::Release);
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !ctx.cancelled() {
                            if Instant::now() >= deadline {
                                return Err(HostError::new(
                                    "TestBoundary",
                                    "Host did not cancel departing owner's run",
                                ));
                            }
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        held.store(
                            task_core
                                .leases
                                .lock()
                                .unwrap()
                                .admit(
                                    lease["token"].as_str().unwrap(),
                                    &task_owner,
                                    &task_domain,
                                    lease["control_epoch"].as_u64().unwrap(),
                                    Instant::now(),
                                )
                                .is_ok(),
                            Ordering::Release,
                        );
                        cancelled.store(true, Ordering::Release);
                        while !finish.load(Ordering::Acquire) {
                            if Instant::now() >= deadline {
                                return Err(HostError::new(
                                    "TestBoundary",
                                    "Confirmed cleanup boundary was never released",
                                ));
                            }
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        ctx.check()?;
                        Ok(json!({}))
                    })
                    .unwrap();
                while !entered.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                let closing_core = core.clone();
                let closing_owner = owner.id().to_owned();
                let closing = tokio::spawn(async move {
                    closing_core
                        .dispatch(&request("close_client", json!({})), &closing_owner)
                        .await
                });
                while !observed_cancel.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                assert!(
                    held_at_cancel.load(Ordering::Acquire),
                    "client close revoked the lease before run-owned cleanup observed cancellation"
                );
                assert!(
                    !closing.is_finished(),
                    "close claimed completion before the experiment's confirmed boundary"
                );
                assert!(core
                    .leases
                    .lock()
                    .unwrap()
                    .for_session(&owner, &domain, Instant::now())
                    .unwrap()
                    .is_some());
                assert!(
                    core.worker.domain_context(&domain.key()).unwrap()["connection_id"].is_string()
                );
                finish_boundary.store(true, Ordering::Release);
                let receipt = closing.await.unwrap().unwrap();
                assert_eq!(receipt["released"], true, "{receipt}");
                assert_eq!(
                    workbench
                        .status(accepted["job_id"].as_str().unwrap(), owner.id())
                        .unwrap()["status"],
                    "cancelled"
                );
                assert!(core
                    .leases
                    .lock()
                    .unwrap()
                    .for_session(&owner, &domain, Instant::now())
                    .unwrap()
                    .is_none());
            })
            .await
            .expect("finite client-close drain flow exceeded its test deadline");
        });
}
