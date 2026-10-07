#[path = "../../Utils/tests/support/gain_wire.rs"]
mod gain;
#[path = "../../Utils/tests/support/pm_wire.rs"]
mod pm;
#[path = "../../Utils/tests/support/voltage_wire.rs"]
mod voltage;
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use yang_debug::{execute_with, parse_args, DiagnosticAuthorization, DiagnosticEnvironment};
use yang_drivers::{
    transport::{
        owner::OwnerGuard,
        serial_abi::{SerialBackend, SerialIo},
        serial_discovery::DeviceRecord,
    },
    DriverResult,
};
use yang_worker::session::SystemFactory;
struct Serial {
    g: Arc<gain::Peer>,
    v: Arc<voltage::Peer>,
}
impl SerialBackend for Serial {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        panic!("no inventory during explicit diagnostic");
    }
    fn open(&self, p: &str) -> DriverResult<Box<dyn SerialIo>> {
        match p {
            "\\\\.\\COM12" => voltage::Backend(self.v.clone()).open(p),
            "\\\\.\\COM13" => gain::Backend(self.g.clone()).open(p),
            _ => panic!("unexpected binding"),
        }
    }
}
struct Env(String);
impl DiagnosticEnvironment for Env {
    fn acquire_owner(&self) -> Result<Box<dyn Send>, String> {
        OwnerGuard::acquire_named(&self.0)
            .map(|g| Box::new(g) as Box<dyn Send>)
            .map_err(|e| e.to_string())
    }
    fn enumerate(&self) -> Result<serde_json::Value, String> {
        panic!("readonly is not enumeration");
    }
}
fn fixture(kind: &str) -> (Vec<String>, PathBuf, Env) {
    let root = std::env::temp_dir().join(format!("yang-diag-{}", yang_worker::new_id().unwrap()));
    std::fs::create_dir_all(root.join("Result")).unwrap();
    let binding = json!({"domain":{"kind":"device","id":"a".repeat(32)},"driver_kind":kind,"model_id":kind,"profile_id":if kind=="gain" {"cp210x-serial"}else{"ch340-serial"},"params":{"port":if kind=="gain" {"COM13"}else{"COM12"}},"expected_identity":{},"config_rev":1,"members":[]});
    let args = vec![
        kind.into(),
        "--stage".into(),
        "readonly".into(),
        "--out".into(),
        root.join("Result/check").to_str().unwrap().into(),
        "--binding".into(),
        binding.to_string(),
    ];
    let env = Env(format!(
        "Global\\YangLab-test-diag-{}",
        yang_worker::new_id().unwrap()
    ));
    (args, root, env)
}
#[test]
fn unknown_or_output_stage_args_open_nothing() {
    let (mut a, root, _) = fixture("gain");
    assert!(!parse_args(&a).unwrap().execute_requested());
    for extra in [
        vec!["--simulate"],
        vec!["--action", "enable_current"],
        vec!["--execute"],
    ] {
        let mut bad = a.clone();
        bad.extend(extra.into_iter().map(str::to_owned));
        assert!(parse_args(&bad).is_err());
    }
    a.extend([
        "--execute".into(),
        "--confirm-stage".into(),
        "action".into(),
    ]);
    assert!(parse_args(&a).is_err());
    assert!(!root.join("Result/check").exists());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn readonly_voltage_gain_do_not_start_normal_driver() {
    let g = gain::Peer::new();
    let v = voltage::Peer::new();
    let factory = Arc::new(SystemFactory::with_backends(
        g.clock.clone(),
        Arc::new(Serial {
            g: g.clone(),
            v: v.clone(),
        }),
        pm::Wire::new(1),
    ));
    for kind in ["gain", "voltage"] {
        let (mut a, root, env) = fixture(kind);
        a.extend([
            "--execute".into(),
            "--confirm-stage".into(),
            "readonly".into(),
        ]);
        let p = parse_args(&a).unwrap();
        let auth = DiagnosticAuthorization::from_plan(&p).unwrap();
        let report = execute_with(p, auth, factory.clone(), &env).unwrap();
        assert!(report.resources_released());
        assert!(report.success());
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("Result/check/report.json")).unwrap())
                .unwrap();
        assert_eq!(stored["stage"], "readonly");
        assert_eq!(stored["source_kind"], "real");
        assert_eq!(stored["output_changes_authorized"], false);
        std::fs::remove_dir_all(root).unwrap();
    }
    assert!(
        g.data
            .lock()
            .unwrap()
            .writes
            .iter()
            .all(|(_, s)| s.starts_with(b"RD")),
        "normal connect/shutdown writes crossed readonly boundary"
    );
    assert!(
        v.data.lock().unwrap().writes.is_empty(),
        "voltage readonly probe wrote output data"
    );
}
#[test]
fn diagnostic_refuses_live_owner() {
    let (mut a, root, env) = fixture("gain");
    a.extend([
        "--execute".into(),
        "--confirm-stage".into(),
        "readonly".into(),
    ]);
    let guard = OwnerGuard::acquire_named(&env.0).unwrap();
    let g = gain::Peer::new();
    let v = voltage::Peer::new();
    let f = Arc::new(SystemFactory::with_backends(
        g.clock.clone(),
        Arc::new(Serial { g: g.clone(), v }),
        pm::Wire::new(1),
    ));
    let p = parse_args(&a).unwrap();
    let auth = DiagnosticAuthorization::from_plan(&p).unwrap();
    assert!(execute_with(p, auth, f, &env).is_err());
    assert!(g.data.lock().unwrap().writes.is_empty());
    assert!(!root.join("Result/check").exists());
    drop(guard);
    std::fs::remove_dir_all(root).unwrap();
}
