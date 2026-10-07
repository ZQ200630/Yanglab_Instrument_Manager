use crate::native_worker::NativeWorkerLaunch;
use std::{fs, path::PathBuf};
use yang_protocol::PackageIdentity;
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "Yang native 包 {}",
            crate::host::registry::new_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn package(&self, bytes: &[u8]) -> PackageIdentity {
        fs::write(self.0.join("yang-worker.exe"), bytes).unwrap();
        PackageIdentity {
            schema: 1,
            source_revision: "test-source".into(),
            package_revision: "0.1.0".into(),
            protocol_version: 3,
            startup_revision: 1,
            worker_sha256: crate::host::verification::sha256_bytes(bytes).unwrap(),
        }
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn native_image_and_package_must_match() {
    let f = Folder::new();
    let mut package = f.package(b"literal native package identity fixture");
    let launch = NativeWorkerLaunch::resolve(&f.0, &package).unwrap();
    assert_eq!(
        launch.executable(),
        f.0.join("yang-worker.exe").canonicalize().unwrap()
    );
    assert_eq!(launch.package().package_revision, "0.1.0");
    drop(launch);
    package.worker_sha256 = "0".repeat(64);
    assert!(NativeWorkerLaunch::resolve(&f.0, &package).is_err());
    package.worker_sha256 =
        crate::host::verification::sha256_bytes(b"literal native package identity fixture")
            .unwrap();
    package.protocol_version = 2;
    assert!(NativeWorkerLaunch::resolve(&f.0, &package).is_err());
}
#[test]
fn no_interpreter_search_or_fallback() {
    let f = Folder::new();
    let package = f.package(b"native");
    fs::remove_file(f.0.join("yang-worker.exe")).unwrap();
    fs::create_dir_all(f.0.join("App/worker")).unwrap();
    fs::write(f.0.join("App/worker/main.py"), b"legacy must never execute").unwrap();
    fs::write(
        f.0.join("python.exe"),
        b"legacy executable must never execute",
    )
    .unwrap();
    assert!(NativeWorkerLaunch::resolve(&f.0, &package).is_err());
}
#[test]
fn unicode_paths_and_pinned_image_replacement_are_truthful() {
    let f = Folder::new();
    let package = f.package(b"native unicode path");
    let launch = NativeWorkerLaunch::resolve(&f.0, &package).unwrap();
    assert!(fs::write(f.0.join("yang-worker.exe"), b"replacement").is_err());
    assert!(fs::rename(f.0.join("yang-worker.exe"), f.0.join("old.exe")).is_err());
    drop(launch);
    fs::write(f.0.join("yang-worker.exe"), b"replacement").unwrap();
    assert!(NativeWorkerLaunch::resolve(&f.0, &package).is_err());
}
#[test]
fn retained_child_blocks_replacement() {
    use crate::{
        host::instance::StartupRecord,
        runtime::{RuntimeConfig, WorkerRuntime},
    };
    use std::sync::{Arc, Mutex};
    let f = Folder::new();
    let record = Arc::new(Mutex::new(StartupRecord::intent(&f.0).unwrap()));
    let nonce = record.lock().unwrap().nonce().to_owned();
    let callback = record.clone();
    let result = WorkerRuntime::spawn(RuntimeConfig {
        launch: NativeWorkerLaunch::test_native().unwrap(),
        catalog_root: f.0.clone(),
        mode: "real".into(),
        protocol: 3,
        ownership_nonce: Some(nonce.clone()),
        record_child: Some(Arc::new(move |child| {
            callback
                .lock()
                .unwrap()
                .identified(child)
                .map_err(|e| e.to_string())?;
            Err("Activation deliberately refused by test".into())
        })),
    });
    let error = match result {
        Ok(_) => panic!("must remain disarmed"),
        Err(e) => e,
    };
    let runtime = error.retained_runtime.unwrap();
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(f.0.join("worker.json")).unwrap()).unwrap();
    assert_eq!(stored["version"], 2);
    assert_eq!(stored["nonce"], nonce);
    assert!(stored["image_sha256"]
        .as_str()
        .is_some_and(|s| s.len() == 64));
    assert!(stored["image_path"]
        .as_str()
        .is_some_and(|s| PathBuf::from(s).is_absolute()));
    let blocked = crate::host::instance::verify_record(&f.0).unwrap_err();
    assert_eq!(blocked.code, "OwnershipUnknown");
    assert!(blocked.message.contains("live PID/creation match=true"));
    let stopped = runtime.stop().unwrap();
    assert!(stopped.resource_released);
    assert_eq!(stopped.process_exit.unwrap()["confirmed"], true);
    record.lock().unwrap().released().unwrap();
    assert!(crate::host::instance::verify_record(&f.0).is_ok());
}
