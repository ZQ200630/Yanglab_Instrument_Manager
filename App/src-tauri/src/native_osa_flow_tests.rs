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
        let terminal = |stream: &crate::host::events::SnapshotStream| {
            let mut value = None;
            while let Some(event) = stream.try_next().unwrap() {
                if event.kind == "operation"
                    && event.data["operation_id"] == read["operation_id"]
                    && event.data["status"] == "Terminal"
                {
                    value = Some(event.data);
                }
            }
            value.unwrap()
        };
        assert_eq!(terminal(&local), terminal(&remote));
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
