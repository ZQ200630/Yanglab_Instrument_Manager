use serde_json::json;
use std::{sync::Arc, time::Duration};
use yang_drivers::clock::{Clock, ManualClock};
use yang_protocol::{ContextV3, DomainRef};
use yang_worker::observations::EvidenceStore;
#[test]
fn cached_view_does_not_renew_freshness_or_reverse_revision() {
    let clock = Arc::new(ManualClock::default());
    let context = ContextV3 {
        session_id: "a".repeat(32),
        domain: Some(DomainRef {
            kind: "device".into(),
            id: "b".repeat(32),
        }),
        connection_id: Some("c".repeat(32)),
        epoch: 1,
    };
    let mut evidence = EvidenceStore::new(context, clock.clone()).unwrap();
    evidence
        .record("temperature_c", json!(22.0), Duration::ZERO, 1)
        .unwrap();
    assert_eq!(evidence.snapshot()["temperature_c"]["quality"], "fresh");
    clock.wait(Duration::from_secs(6));
    assert_eq!(evidence.snapshot()["temperature_c"]["quality"], "stale");
    assert_eq!(evidence.snapshot()["temperature_c"]["observed_age_s"], 6.0);
    evidence
        .invalidate(&["temperature_c"], "command pending")
        .unwrap();
    evidence
        .record("temperature_c", json!(99.0), Duration::ZERO, 1)
        .unwrap();
    assert_eq!(evidence.snapshot()["temperature_c"]["value"], 22.0);
    assert_eq!(evidence.snapshot()["temperature_c"]["quality"], "unknown");
    assert!(evidence
        .record("tec_enabled", json!(1), clock.now(), 3)
        .is_err());
}
