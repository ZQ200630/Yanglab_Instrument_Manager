use serde_json::json;
use yang_drivers::lifecycle::{CleanupReport, CleanupStep, DriverState};
use yang_protocol::{DomainConfig, DomainRef};
use yang_worker::DomainRegistry;
fn config() -> DomainConfig {
    DomainConfig {
        domain: DomainRef {
            kind: "device".into(),
            id: "a".repeat(32),
        },
        config_rev: 1,
        driver_kind: "osa".into(),
        model_id: "yokogawa.aq6370".into(),
        profile_id: Some("visa-gpib".into()),
        params: json!({"resource":"GPIB0::4::INSTR"}),
        expected_identity: json!({}),
        members: vec![],
    }
}
fn cleanup(released: bool) -> CleanupReport {
    CleanupReport::new(
        "f".repeat(32),
        vec![CleanupStep {
            role: "osa".into(),
            action: "close".into(),
            error: None,
        }],
        None,
        if released { vec![] } else { vec!["osa".into()] },
    )
    .unwrap()
}
#[test]
fn old_generation_cannot_publish() {
    let registry = DomainRegistry::new(&"b".repeat(32)).unwrap();
    let config = config();
    registry.configure(config.clone()).unwrap();
    let old = registry.bind(&config.domain, &"c".repeat(32)).unwrap();
    let current = registry.fence(&config.domain).unwrap();
    assert!(current.epoch > old.epoch);
    assert!(!registry.publish(&old, DriverState::Ready));
    assert!(registry.publish(&current, DriverState::Closing));
}
#[test]
fn retired_domain_cannot_resurrect() {
    let registry = DomainRegistry::new(&"b".repeat(32)).unwrap();
    let config = config();
    registry.configure(config.clone()).unwrap();
    assert!(registry.retire(&config.domain, 0).is_err());
    registry.retire(&config.domain, 1).unwrap();
    assert!(registry.configure(config).is_err());
    assert!(registry.snapshot().is_empty());
}
#[test]
fn pending_close_blocks_reopen() {
    let registry = DomainRegistry::new(&"b".repeat(32)).unwrap();
    let config = config();
    registry.configure(config.clone()).unwrap();
    let context = registry.bind(&config.domain, &"c".repeat(32)).unwrap();
    assert!(registry.release(&context, &cleanup(false)).is_err());
    assert!(registry.retire(&config.domain, 1).is_err());
    let mut newer = config.clone();
    newer.config_rev = 2;
    assert!(registry.configure(newer).is_err());
    assert!(registry.snapshot()[0].responsibility);
    registry.release(&context, &cleanup(true)).unwrap();
    assert!(!registry.snapshot()[0].responsibility);
    let released = registry.context(&config.domain).unwrap();
    assert!(released.epoch > context.epoch);
    assert!(released.connection_id.is_none());
}
