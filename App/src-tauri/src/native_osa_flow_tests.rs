//! Offline integration through the actual Host, native worker, driver and archive.
//! Only the instrument byte transport is the finite reviewed OSA transcript.
use super::*;
use crate::host::{
    archive::{ArchiveRef, NativeExport, SelectedDirectory},
    configuration::*,
    sessions::ClientChannel,
};
use std::fs;

struct Fixture {
    core: Option<Arc<HostCore>>,
    root: PathBuf,
    local: ClientChannel,
    remote: ClientChannel,
    last_attempt: Mutex<Option<Value>>,
}
impl Fixture {
    fn new(storage: bool) -> Self {
        Self::with_registry(storage, None)
    }
    fn with_registry(
        storage: bool,
        saved: Option<crate::host::contracts::RegistrySnapshot>,
    ) -> Self {
        let root = std::env::temp_dir().join(format!("Yang native OSA {}", new_id().unwrap()));
        fs::create_dir_all(root.join("App/worker")).unwrap();
        if let Some(saved) = saved {
            fs::write(
                root.join("devices.json"),
                serde_json::to_vec(&saved).unwrap(),
            )
            .unwrap();
        }
        let registry = Registry::open(&root.join("devices.json")).unwrap();
        let host_id = registry.snapshot().unwrap().host_id;
        let boot = new_id().unwrap();
        let record = Arc::new(Mutex::new(StartupRecord::intent(&root).unwrap()));
        let callback = record.clone();
        let nonce = record.lock().unwrap().nonce().to_owned();
        let worker = WorkerRuntime::spawn(RuntimeConfig {
            launch: crate::native_worker::NativeWorkerLaunch::test_osa_fixture(&root).unwrap(),
            catalog_root: root.clone(),
            mode: "real".into(),
            protocol: 3,
            ownership_nonce: Some(nonce),
            record_child: Some(Arc::new(move |id| {
                callback
                    .lock()
                    .unwrap()
                    .identified(id)
                    .map_err(|e| e.to_string())
            })),
        })
        .unwrap();
        let operations = Arc::new(
            OperationActor::new(
                OperationBook::open(&root.join("operations.json"), boot.clone()).unwrap(),
            )
            .unwrap(),
        );
        let leases = Arc::new(Mutex::new(LeaseBook::new(&boot).unwrap()));
        let bindings = Arc::new(Mutex::new(BTreeMap::new()));
        let query_gate = Arc::new(tokio::sync::Mutex::new(()));
        let control = Arc::new(Mutex::new(BTreeMap::new()));
        let port = WorkerVerificationPort {
            query_gate: query_gate.clone(),
            worker: worker.clone(),
            leases: leases.clone(),
            bindings: bindings.clone(),
            control: control.clone(),
            operations: operations.clone(),
            proofs: Arc::new(Mutex::new(BTreeMap::new())),
            retired: Arc::new(Mutex::new(BTreeMap::new())),
        };
        let configuration = Configuration::new(registry, Box::new(port.clone())).unwrap();
        let registry = configuration.cache.clone();
        for device in &registry.lock().unwrap().devices {
            port.configure_wire(device_wire(device).unwrap()).unwrap();
        }
        let archive_root = root.join("Result");
        if !storage {
            fs::write(&archive_root, b"storage deliberately unavailable").unwrap();
        }
        let (archive, archive_error) =
            match crate::host::archive::ArchiveStore::open(&archive_root, &host_id)
                .and_then(crate::host::archive::ArchiveActor::new)
            {
                Ok(actor) => (Some(actor), None),
                Err(error) => (None, Some(error)),
            };
        let mut clients = crate::host::sessions::ClientSessions::new(boot.clone());
        let local = clients.join(&json!({})).unwrap();
        // Peer ownership is installed at the authenticated-session boundary;
        // TLS authentication itself is covered by the existing remote suite.
        let remote = clients.join_peer(&json!({}), &"9".repeat(32)).unwrap();
        let core = Arc::new(HostCore {
            remote: Arc::new(Mutex::new(
                crate::remote::RemoteStore::open(root.join("remote.dpapi"), host_id.clone())
                    .unwrap(),
            )),
            remote_state: Mutex::new(json!({"state":"DISABLED"})),
            remote_generation: std::sync::atomic::AtomicU64::new(0),
            tray_status: Arc::new(Mutex::new(crate::host::tray::HostStatus {
                mode: "real".into(),
                state: "ONLINE".into(),
            })),
            tray_stop: Arc::new(AtomicBool::new(false)),
            clients: Mutex::new(clients),
            events: crate::host::events::EventHub::new(host_id.clone(), boot.clone(), json!({}))
                .unwrap(),
            results: crate::host::results::ResultActor::new(
                crate::host::results::ResultStore::open(&root.join("results"), boot.clone())
                    .unwrap(),
            )
            .unwrap(),
            archive,
            archive_error,
            archive_root,
            status_cache: Mutex::new(None),
            status_generation: AtomicU64::new(0),
            checks: Mutex::new(crate::host::checks::Checks::default()),
            status_failed: AtomicBool::new(false),
            power_seen: std::sync::atomic::AtomicU64::new(power_generation()),
            boot_id: boot,
            leases,
            bindings,
            cleanup_attempted: Mutex::new(BTreeSet::new()),
            cleanup_results: Mutex::new(BTreeMap::new()),
            query_gate,
            operations,
            safety_audit: SafetyAudit::new(root.join("safety.json")).unwrap(),
            ordinary_admission: tokio::sync::Mutex::new(()),
            configuration_activation: Arc::new(crate::host::laser::ActivationState::default()),
            driver_install: Arc::new(crate::host::driver_install::Installer::default()),
            ordinary_capacity: Arc::new(Semaphore::new(31)),
            cleanup_running: Mutex::new(BTreeSet::new()),
            registry,
            configuration,
            verification_control: control,
            verification_port: port,
            worker,
            record,
            mode: "real".into(),
            startup_error: None,
            stopped: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            event_streams: AtomicUsize::new(0),
            wake: Notify::new(),
        });
        Self {
            core: Some(core),
            root,
            local,
            remote,
            last_attempt: Mutex::new(None),
        }
    }
    fn core(&self) -> &Arc<HostCore> {
        self.core.as_ref().unwrap()
    }
    async fn rpc(&self, method: &str, params: Value, remote: bool) -> Value {
        self.core()
            .dispatch(
                &HostRequest {
                    v: 1,
                    id: new_id().unwrap(),
                    method: method.into(),
                    params,
                },
                if remote {
                    self.remote.session.id()
                } else {
                    self.local.session.id()
                },
            )
            .await
            .unwrap()
    }
    async fn osa(&self) -> (DomainRef, Value) {
        let draft = self
            .rpc(
                "create_draft",
                json!({"name":"Bench OSA","model_id":"aq6370","profile_id":"gpib-visa",
            "params":{"resource":"GPIB0::4::INSTR"},"expected_rev":0}),
                false,
            )
            .await;
        let domain = DomainRef {
            kind: "device".into(),
            id: draft["device_id"].as_str().unwrap().into(),
        };
        let lease = self
            .rpc("acquire_control", json!({"domain":domain}), false)
            .await;
        let proof = self.rpc("test_connection", json!({"draft_id":domain.id,"expected_rev":1,
            "consent":{"accepted":true,"mode":"real","config_digest":draft["config_digest"],"open_effects":[],"supervised":false,"retain_session":false},
            "lease_token":lease["token"],"control_epoch":lease["control_epoch"],"request_id":new_id().unwrap(),"sequence":1}), false).await;
        self.rpc(
            "save_device",
            json!({"draft_id":domain.id,"proof_id":proof["proof_id"],"expected_rev":1}),
            false,
        )
        .await;
        let connected = self
            .operation(
                &domain,
                &lease,
                "connect",
                json!({"acknowledge_lifecycle":true}),
                2,
            )
            .await;
        assert_eq!(connected["phase"], "completed", "{connected}");
        (domain, lease)
    }
    async fn operation(
        &self,
        domain: &DomainRef,
        lease: &Value,
        method: &str,
        params: Value,
        sequence: u64,
    ) -> Value {
        let ctx = self
            .core()
            .worker
            .domain_context(&domain.key())
            .or_else(|| self.core().bindings.lock().unwrap().get(domain).cloned())
            .unwrap();
        let mut intent = json!({"domain":domain,"lease_token":lease["token"],"control_epoch":lease["control_epoch"],
            "config_rev":1,"context":ctx,"method":method,"params":params,"sequence":sequence,"confirmation":null});
        let proof = self.rpc("prepare", json!({"intent":intent}), false).await;
        intent["confirmation"] = proof["token"].clone();
        let id = new_id().unwrap();
        let attempt = json!({"request_id":id,"intent":intent});
        *self.last_attempt.lock().unwrap() = Some(attempt.clone());
        let mut record = self.rpc("execute", attempt, false).await;
        let end = Instant::now() + Duration::from_secs(15);
        while record["status"] != "Terminal" {
            assert!(
                Instant::now() < end,
                "Native operation did not settle: {record}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            record = self.rpc("operation", json!({"request_id":id}), false).await;
        }
        record
    }
    async fn read(&self, domain: &DomainRef, lease: &Value, sequence: u64) -> Value {
        self.operation(
            domain,
            lease,
            "action",
            json!({"name":"read_trace","args":{"trace":"A","archive_name":"OSA"}}),
            sequence,
        )
        .await
    }
    async fn bytes(&self, reference: &Value, remote: bool) -> Vec<u8> {
        let chunk = self.rpc("read_archive", json!({"domain":reference["domain"],"name":reference["name"],"id":reference["id"],"offset":0,"length":32}), remote).await;
        crate::host::results::unhex(chunk["data_hex"].as_str().unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let core = self.core.take().unwrap();
        let result = core.worker.stop();
        if !std::thread::panicking() {
            assert!(result.is_ok(), "Native fixture close: {result:?}");
        }
        drop(core);
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn executor() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}
fn literal_samples() -> Vec<u8> {
    // Hand-reviewed bytes, independent of staging/decoding helpers.
    [1550_f64, -30_f64, 1551_f64, -31_f64]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}
async fn cached_gain_metadata(fixture: &Fixture, age: Duration) -> String {
    // Configure metadata only. The finite OSA worker must never open a Gain port.
    let draft = fixture.rpc("create_draft", json!({"name":"Gain cadence",
        "model_id":"gain","profile_id":"cp210x-serial",
        "params":{"port":"COM13"},"expected_rev":0}), false).await;
    let key = format!("device:{}", draft["device_id"].as_str().unwrap());
    let context = json!({"connection_id":"finite-cadence-observation"});
    let cached = json!({"cadence_marker":true,"domains":{key.clone():{
        "context":context,"state":"READY","responsibility":true}},
        "devices":{key.clone():{"connected":true,"state":"READY"}}});
    *fixture.core().status_cache.lock().unwrap() = Some((
        Instant::now()-age,
        fixture.core().status_generation.load(Ordering::Acquire), cached));
    key
}
#[test]
fn gain_metadata_cache_expires_after_200_ms_without_opening_a_port() {
    let fixture = Fixture::new(true);
    executor().block_on(async {
        cached_gain_metadata(&fixture, Duration::from_millis(201)).await;
        let metadata = fixture.core().worker_cache().await.unwrap();
        assert!(metadata["cadence_marker"].is_null(),
            "Gain must query worker metadata after 200 ms rather than retain the 2.5 s cache");
        assert!(metadata["devices"].as_object().unwrap().is_empty());
    });
}
#[test]
fn released_gain_metadata_preserves_the_idle_cache_interval() {
    let fixture = Fixture::new(true);
    executor().block_on(async {
        let key = cached_gain_metadata(&fixture, Duration::from_millis(201)).await;
        {
            let mut cache = fixture.core().status_cache.lock().unwrap();
            let value = &mut cache.as_mut().unwrap().2;
            value["domains"][&key]["responsibility"] = json!(false);
            value["domains"][&key]["context"]["connection_id"] = Value::Null;
        }
        let metadata = fixture.core().worker_cache().await.unwrap();
        assert_eq!(metadata["cadence_marker"], true);
    });
}
#[test]
fn fast_gain_publisher_queries_metadata_even_with_a_new_cache() {
    let fixture = Fixture::new(true);
    executor().block_on(async {
        cached_gain_metadata(&fixture, Duration::ZERO).await;
        assert_eq!(fixture.core().metadata_interval(), Duration::from_millis(200));
        let metadata = fixture.core().publisher_metadata().await.unwrap();
        assert!(metadata["cadence_marker"].is_null(), "due publications must not beat against cache TTL");
        assert!(metadata["devices"].as_object().unwrap().is_empty());
        assert_eq!(fixture.core().metadata_interval(), Duration::from_millis(2500));
    });
}
#[test]
fn gain_cadence_excludes_unknown_faulted_and_unconnected_metadata() {
    let fixture = Fixture::new(true);
    executor().block_on(async {
        let key = cached_gain_metadata(&fixture, Duration::ZERO).await;
        for (path, replacement) in [
            (vec!["domains", key.as_str(), "state"], json!("FAULT")),
            (vec!["devices", key.as_str(), "connected"], json!(false)),
            (vec!["domains", key.as_str(), "context", "connection_id"], Value::Null),
        ] {
            let original = {
                let mut cache = fixture.core().status_cache.lock().unwrap();
                let mut slot = &mut cache.as_mut().unwrap().2;
                for part in &path { slot = &mut slot[*part]; }
                std::mem::replace(slot, replacement)
            };
            assert_eq!(fixture.core().metadata_interval(), Duration::from_millis(2500));
            let mut cache = fixture.core().status_cache.lock().unwrap();
            let mut slot = &mut cache.as_mut().unwrap().2;
            for part in &path { slot = &mut slot[*part]; }
            *slot = original;
        }
    });
}
#[test]
fn completed_metadata_wakes_the_idle_publisher() {
    let fixture = Fixture::new(true);
    executor().block_on(async {
        fixture.core().completed_operation_metadata().await.unwrap();
        tokio::time::timeout(Duration::from_millis(50), fixture.core().wake.notified())
            .await.expect("completed metadata must wake the publisher without waiting 2.5 seconds");
    });
}
#[test]
fn newly_created_gain_draft_has_released_metadata_before_first_resnapshot() {
    let fixture = Fixture::new(true);
    executor().block_on(async {
        let _subscription = fixture
            .core()
            .events
            .subscribe(&fixture.local.session)
            .unwrap();
        // Keep a separate connected instrument in the Host, as in the reported
        // Gain wizard failure. The byte transport is the bounded OSA fixture.
        let (existing, lease) = fixture.osa().await;
        fixture.core().worker_cache_with_refresh(true).await.unwrap();
        let before = fixture.core().snapshot().unwrap();
        assert_eq!(before["domains"][existing.key()]["state"], "READY");
        let draft = fixture
            .rpc(
                "create_draft",
                json!({"name":"Gain", "model_id":"gain", "profile_id":"cp210x-serial",
                    "params":{"port":"COM4"}, "expected_rev":2}),
                false,
            )
            .await;
        let domain = DomainRef {
            kind: "device".into(),
            id: draft["device_id"].as_str().unwrap().into(),
        };
        fixture.rpc("request_snapshot", json!({}), false).await;
        let snapshot = fixture.core().events.current();
        assert_eq!(snapshot["domains"][domain.key()]["state"], "DISCONNECTED",
            "a newly configured draft must have authoritative released metadata before UI Connect admission: {snapshot}");
        assert!(snapshot["domains"][domain.key()]["device"].is_null());
        assert!(snapshot["domains"][domain.key()]["context"]["connection_id"].is_null());
        assert_eq!(snapshot["domains"][domain.key()]["context"]["epoch"], 0);
        assert_eq!(snapshot["control"][domain.key()]["state"], "AVAILABLE");
        assert_eq!(snapshot["registry"]["drafts"][0]["device_id"], draft["device_id"]);
        assert_eq!(
            snapshot["domains"][existing.key()]["context"],
            before["domains"][existing.key()]["context"]
        );
        assert_eq!(
            snapshot["domains"][existing.key()]["device"],
            before["domains"][existing.key()]["device"]
        );
        assert_eq!(
            snapshot["control"][existing.key()]["controller_session"],
            lease["session_id"]
        );
    });
}
#[test]
fn native_osa_archive_and_export_match_exactly() {
    let f = Fixture::new(true);
    executor().block_on(async {
        let (domain, lease) = f.osa().await;
        let read = f.read(&domain, &lease, 3).await;
        assert_eq!(read["phase"], "completed", "{read}");
        // Cached metadata must enable the existing page without claiming a fresh sweep.
        tokio::time::sleep(Duration::from_millis(75)).await;
        f.core().worker_cache().await.unwrap();
        let snapshot = f.core().snapshot().unwrap();
        assert_eq!(
            snapshot["domains"][domain.key()]["device"]["connected"],
            true,
            "{snapshot}"
        );
        let reference = &read["result"]["result"]["archive_ref"];
        assert_eq!(f.bytes(reference, false).await, literal_samples());
        let manifest = f
            .rpc(
                "archive_manifest_bytes",
                json!({"domain":domain,"name":"OSA","id":reference["id"]}),
                false,
            )
            .await;
        let export_root = f.root.join("Export");
        fs::create_dir(&export_root).unwrap();
        let export = NativeExport::verify(
            &serde_json::from_value::<ArchiveRef>(reference.clone()).unwrap(),
            crate::host::results::unhex(manifest["data_hex"].as_str().unwrap()).unwrap(),
            f.bytes(reference, false).await,
        )
        .unwrap();
        let folder = export
            .write(&SelectedDirectory::open(&export_root).unwrap())
            .unwrap();
        assert_eq!(
            fs::read_to_string(folder.join("spectrum.csv")).unwrap(),
            "wavelength_nm,power_dBm\n1550,-30\n1551,-31\n"
        );
        for line in fs::read_to_string(folder.join("spectrum.csv"))
            .unwrap()
            .lines()
            .skip(1)
        {
            let numbers = line
                .split(',')
                .map(|s| s.parse::<f64>().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(numbers.len(), 2);
        }
        for field in ["instrument_io_ms", "decode_ms", "staging_ms"] {
            assert!(
                read["result"]["result"]["timings"][field]
                    .as_f64()
                    .is_some_and(|n| n.is_finite() && n >= 0.0),
                "Missing measured {field}: {read}"
            );
        }
    });
}
#[test]
fn storage_failure_does_not_replay_read() {
    let f = Fixture::new(false);
    executor().block_on(async {
        let (domain, lease) = f.osa().await;
        let read = f.read(&domain, &lease, 3).await;
        assert_eq!(read["phase"], "completed_readback_failed", "{read}");
        assert_eq!(read["result"]["result"]["hardware_read_completed"], true);
        assert_eq!(read["result"]["result"]["retry_hardware"], false);
        let descriptor = &read["result"]["result"]["capture"];
        let bytes = f
            .core()
            .worker
            .read_capture_chunk(descriptor, 0, 32)
            .unwrap();
        assert_eq!(bytes, literal_samples());
        let restored = f
            .rpc("operation", json!({"request_id":read["request_id"]}), false)
            .await;
        assert_eq!(restored["operation_id"], read["operation_id"]);
        let attempt = f.last_attempt.lock().unwrap().clone().unwrap();
        assert_eq!(
            f.rpc("execute", attempt, false).await["operation_id"],
            read["operation_id"]
        );
        assert_eq!(
            f.core()
                .worker
                .read_capture_chunk(descriptor, 0, 32)
                .unwrap(),
            literal_samples()
        );
    });
}
#[test]
fn native_osa_saved_configuration_requires_current_identity_only_on_connect() {
    let first = Fixture::new(true);
    let executor = executor();
    let (domain, _) = executor.block_on(first.osa());
    let saved = first.core().registry.lock().unwrap().clone();
    drop(first);
    let next = Fixture::with_registry(true, Some(saved));
    executor.block_on(async {
        let status = next.core().worker_cache().await.unwrap();
        assert_eq!(
            status["connected"], false,
            "Startup must not open instruments"
        );
        let lease = next
            .rpc("acquire_control", json!({"domain":domain}), false)
            .await;
        let connected = next
            .operation(
                &domain,
                &lease,
                "connect",
                json!({"acknowledge_lifecycle":true}),
                1,
            )
            .await;
        assert_eq!(connected["phase"], "completed", "{connected}");
        assert_eq!(next.read(&domain, &lease, 2).await["phase"], "completed");
    });
}
#[test]
fn native_osa_saved_identity_mismatch_never_rebinds() {
    let first = Fixture::new(true);
    let executor = executor();
    let (domain, _) = executor.block_on(first.osa());
    let mut saved = first.core().registry.lock().unwrap().clone();
    saved.devices[0].expected_identity["serial"] = json!("OTHER-OSA");
    let expected = saved.devices[0].expected_identity.clone();
    drop(first);
    let next = Fixture::with_registry(true, Some(saved));
    executor.block_on(async {
        let lease = next
            .rpc("acquire_control", json!({"domain":domain}), false)
            .await;
        let connected = next
            .operation(
                &domain,
                &lease,
                "connect",
                json!({"acknowledge_lifecycle":true}),
                1,
            )
            .await;
        assert_ne!(connected["phase"], "completed");
        assert!(
            connected.to_string().contains("IdentityMismatch"),
            "{connected}"
        );
        assert_eq!(
            next.core().registry.lock().unwrap().devices[0].expected_identity,
            expected
        );
        assert!(
            next.core().worker.domain_context(&domain.key()).unwrap()["connection_id"].is_null()
        );
    });
}
#[test]
fn remote_observer_sees_same_capture() {
    let f = Fixture::new(true);
    executor().block_on(async {
        let local = f.core().events.subscribe(&f.local.session).unwrap();
        let remote = f.core().events.subscribe(&f.remote.session).unwrap();
        let (domain, lease) = f.osa().await;
        let read = f.read(&domain, &lease, 3).await;
        assert_eq!(read["phase"], "completed", "{read}");
        async fn terminal(stream: &crate::host::events::SnapshotStream, id: &Value) -> Value {
            // Durable operation lookup can finish before its asynchronous event
            // publication. Wait for this exact event, never replay the read.
            let end = Instant::now() + Duration::from_secs(3);
            loop {
                while let Some(event) = stream.try_next().unwrap() {
                    if event.kind == "operation"
                        && event.data["operation_id"] == *id
                        && event.data["status"] == "Terminal"
                    {
                        return event.data;
                    }
                }
                assert!(Instant::now() < end, "Terminal event was not delivered");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        assert_eq!(
            terminal(&local, &read["operation_id"]).await,
            terminal(&remote, &read["operation_id"]).await
        );
        let reference = &read["result"]["result"]["archive_ref"];
        assert_eq!(
            f.bytes(reference, false).await,
            f.bytes(reference, true).await
        );
        assert_eq!(
            f.core().leases.lock().unwrap().snapshot()[domain.key()]["controller_session"],
            f.local.session.id()
        );
        assert!(f
            .core()
            .clients
            .lock()
            .unwrap()
            .is_remote(f.remote.session.id()));
    });
}
#[test]
fn old_pin_is_never_replaced() {
    let f = Fixture::new(true);
    executor().block_on(async {
        let (domain, lease) = f.osa().await;
        let first = f.read(&domain, &lease, 3).await;
        assert_eq!(first["phase"], "completed", "{first}");
        let pin = first["result"]["result"]["archive_ref"].clone();
        let before = f
            .rpc(
                "archive_manifest_bytes",
                json!({"domain":domain,"name":"OSA","id":pin["id"]}),
                true,
            )
            .await;
        let second = f.read(&domain, &lease, 4).await;
        assert_eq!(second["phase"], "completed", "{second}");
        assert_ne!(pin["id"], second["result"]["result"]["archive_ref"]["id"]);
        assert_eq!(
            before,
            f.rpc(
                "archive_manifest_bytes",
                json!({"domain":domain,"name":"OSA","id":pin["id"]}),
                true
            )
            .await
        );
        assert_eq!(f.bytes(&pin, true).await, literal_samples());
    });
}
#[test]
fn storage_only_recovery_after_disconnect_uses_original_native_read_and_no_new_lease() {
    let f = Fixture::new(true);
    executor().block_on(async {
        let (domain, lease) = f.osa().await;
        let context = f.core().worker.domain_context(&domain.key()).unwrap();
        let original = new_id().unwrap();
        let read = f
            .core()
            .worker
            .submit(WorkerRequest::V3(
                json!({"v":3,"id":original,"method":"action",
            "params":{"name":"read_trace","args":{"trace":"A"}},"context":context}),
            ))
            .unwrap()
            .wait_async(Duration::from_secs(15))
            .await
            .unwrap();
        assert_eq!(read["phase"], "completed");
        let capture = read["result"]["capture"]["capture_id"].clone();
        f.rpc("safe_stop", json!({"domain":domain}), false).await;
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            let snapshot = f.core().worker_cache().await.unwrap();
            if snapshot["domains"][domain.key()]["state"] == "DISCONNECTED" {
                break;
            }
            assert!(Instant::now() < end);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let before = f.core().leases.lock().unwrap().snapshot();
        let saved = f
            .rpc(
                "recover_capture",
                json!({"domain":domain,"capture_id":capture,"name":"Recovered"}),
                true,
            )
            .await;
        assert_eq!(saved["storage_only"], true);
        assert_eq!(saved["original_request_id"], original);
        assert_eq!(saved["staging_release_confirmed"], true);
        assert_eq!(
            f.core().leases.lock().unwrap().snapshot(),
            before,
            "storage recovery granted hardware control"
        );
        assert_eq!(
            f.bytes(&saved["archive_ref"], true).await,
            literal_samples()
        );
        let _ = lease;
    });
}

#[test]
fn native_retained_host_management_is_reachable_and_actions_stay_fenced() {
    let mut fixture = Fixture::new(true);
    Arc::get_mut(fixture.core.as_mut().unwrap()).unwrap().startup_error = Some("configuration startup retained".into());
    executor().block_on(async {
        let ping = fixture.rpc("ping", json!({}), false).await;
        assert_eq!(ping["worker_kind"], "rust");
        assert_eq!(ping["worker_startup_revision"], 1);
        assert_eq!(ping["worker_startup_verified"], true);
        assert_eq!(ping["worker_activation_confirmed"], true);
        let status = fixture.rpc("worker_status", json!({}), false).await;
        assert_eq!(status["host_status"], "RETAINED");
        let domain = DomainRef {kind:"device".into(),id:"a".repeat(32)};
        let intent: ExecuteParams = serde_json::from_value(json!({
            "domain":domain,"lease_token":"b".repeat(32),"control_epoch":0,"config_rev":1,
            "context":{"session_id":"c".repeat(32),"domain":domain,"connection_id":null,"epoch":0},
            "method":"connect","params":{},"sequence":1,"confirmation":null
        })).unwrap();
        assert_eq!(fixture.core().validate_intent(&intent).unwrap_err().code, "HostRetained");
        let scan = fixture.core().dispatch(
            &HostRequest {v:1, id:new_id().unwrap(), method:"scan_lasers".into(), params:json!({})},
            fixture.local.session.id(),
        ).await.unwrap_err();
        assert_eq!(scan.code, "HostRetained");
    });
}

#[test]
fn native_host_keeps_management_and_stop_fenced_for_unconfirmed_installer() {
    let fixture = Fixture::new(true);
    fixture.core().driver_install.begin(&json!({}), "ch340").unwrap();
    executor().block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            for phase in ["running", "unknown"] {
                if phase == "unknown" {
                    fixture.core().driver_install.finish(
                        crate::host::driver_install::finite_unconfirmed_install(&fixture.root.join("finite installer")),
                    );
                }
                let install = fixture.rpc("driver_install_status", json!({}), false).await;
                assert_eq!(install["state"], phase);
                let stopped = fixture.core().dispatch(
                    &HostRequest { v: 1, id: new_id().unwrap(), method: "stop".into(), params: json!({"confirm":true}) },
                    fixture.local.session.id(),
                ).await.unwrap_err();
                assert_eq!(stopped.code, "DriverInstalling");
                assert!(!fixture.core().stopping.load(Ordering::Acquire));
                assert!(!fixture.core().stopped.load(Ordering::Acquire));
                assert!(fixture.core().driver_install.admit().is_err());
                let ping = fixture.rpc("ping", json!({}), false).await;
                assert_eq!(ping["worker_kind"], "rust");
                assert_eq!(ping["worker_activation_confirmed"], true);
                fixture.rpc("worker_status", json!({}), false).await;
            }
            let dependency = fixture.root.join("finite installer/ch340/file.inf");
            assert!(fs::OpenOptions::new().write(true).open(&dependency).is_err());
            assert!(fs::remove_file(&dependency).is_err());
            let retained = fixture.rpc("driver_install_status", json!({}), false).await;
            assert_eq!(retained["process_termination_confirmed"], false);
            assert_eq!(retained["resource_pins_retained"], true);
        }).await.expect("Finite installer Host contract exceeded its deadline");
    });
}
