use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use yang_drivers::lifecycle::{CleanupReport, CleanupStep, ResponsibilityRegistry};
use yang_drivers::transport::owner::OwnerGuard;
use yang_drivers::transport::{ByteTransport, CloseReport, Deadline};
use yang_drivers::{DriverError, DriverResult};
struct Io(Arc<AtomicBool>);
#[test]
fn deserialized_cleanup_cannot_bypass_evidence_bounds() {
    for value in [
        json!({"attempt_id":"bad","steps":[{"role":"osa","action":"close","error":null}],"voltage_zero":null,"unreleased":[]}),
        json!({"attempt_id":"a".repeat(32),"steps":[],"voltage_zero":null,"unreleased":[]}),
        json!({"attempt_id":"a".repeat(32),"steps":[{"role":"osa","action":"close","error":null}],"voltage_zero":null,"unreleased":[],"extra":true}),
    ] {
        assert!(serde_json::from_value::<CleanupReport>(value).is_err());
    }
}
impl ByteTransport for Io {
    fn write_all(&mut self, _: &[u8], _: Deadline) -> DriverResult<()> {
        Ok(())
    }
    fn read_bounded(&mut self, _: usize, _: Deadline) -> DriverResult<Vec<u8>> {
        Ok(vec![])
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        Ok(CloseReport {
            released: !self.0.load(Ordering::SeqCst),
            status: Some(0),
        })
    }
    fn has_responsibility(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
#[test]
fn cleanup_attempt_is_immutable() {
    let step = CleanupStep {
        role: "osa".into(),
        action: "close".into(),
        error: Some("close incomplete".into()),
    };
    let report =
        CleanupReport::new("a".repeat(32), vec![step.clone()], None, vec!["osa".into()]).unwrap();
    let serialized = serde_json::to_value(&report).unwrap();
    let retry = CleanupReport::new(
        "b".repeat(32),
        vec![CleanupStep {
            error: None,
            ..step
        }],
        None,
        vec![],
    )
    .unwrap();
    assert!(retry.resources_released());
    assert!(!report.resources_released());
    assert_eq!(serde_json::to_value(&report).unwrap(), serialized);
    assert!(CleanupReport::new(
        "invalid".into(),
        vec![],
        Some(json!({"state":"unknown"})),
        vec![]
    )
    .is_err());
}
#[test]
fn pending_close_blocks_reopen() {
    let registry = ResponsibilityRegistry::default();
    let pending = Arc::new(AtomicBool::new(true));
    let mut owner = registry.adopt(Box::new(Io(pending.clone())));
    assert!(!owner.close().unwrap().released);
    drop(owner);
    assert_eq!(registry.pending(), 1);
    assert!(Arc::strong_count(&pending) > 1);
    assert_eq!(
        registry
            .retry(Deadline::after(Duration::from_secs(1)))
            .unwrap(),
        1
    );
    pending.store(false, Ordering::SeqCst);
    assert_eq!(
        registry
            .retry(Deadline::after(Duration::from_secs(1)))
            .unwrap(),
        0
    );
}
#[test]
fn diagnostic_owner_guard_conflicts_with_host() {
    let name = format!("Global\\YangLab-test-native-{}", std::process::id());
    let host = OwnerGuard::acquire_named(&name).unwrap();
    assert!(matches!(
        OwnerGuard::acquire_named(&name),
        Err(DriverError::Busy(_))
    ));
    drop(host);
    assert!(OwnerGuard::acquire_named(&name).is_ok());
}
