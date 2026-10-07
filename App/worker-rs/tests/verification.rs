use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use yang_drivers::{
    clock::{Clock, ManualClock},
    lifecycle::ProbeReport,
};
use yang_protocol::{DomainConfig, DomainRef};
use yang_worker::{
    catalog::{admit, Claims},
    verification::{ProbeAuthorization, ProbePort, Verifier},
    DomainRegistry, WorkerError,
};
fn config(kind: &str, number: u32) -> DomainConfig {
    let (model, profile, params) = match kind {
        "osa" => ("aq6370", "gpib-visa", json!({"resource":"GPIB0::4::INSTR"})),
        "gain" => ("gain", "cp210x-serial", json!({"port":"COM12"})),
        "voltage" => ("voltage", "ch340-serial", json!({"port":"COM13"})),
        _ => ("mdt693b", "serial", json!({"port":format!("COM{number}")})),
    };
    DomainConfig {
        domain: DomainRef {
            kind: "device".into(),
            id: format!("{number:032x}"),
        },
        config_rev: 1,
        driver_kind: kind.into(),
        model_id: model.into(),
        profile_id: Some(profile.into()),
        params,
        expected_identity: json!({}),
        members: vec![],
    }
}
fn consent(config: &DomainConfig) -> ProbeAuthorization {
    ProbeAuthorization {
        stage: "readonly".into(),
        accepted: true,
        supervised: false,
        retain_session: false,
        binding: json!({"mode":"real","domain":config.domain,"config_rev":config.config_rev,"model_id":config.model_id,"profile_id":config.profile_id,"config_digest":"b".repeat(64)}),
    }
}
struct Port {
    registry: DomainRegistry,
    reads: AtomicUsize,
    identity: serde_json::Value,
}
impl ProbePort for Port {
    fn registry(&self) -> DomainRegistry {
        self.registry.clone()
    }
    fn probe_readonly(
        &self,
        _: &DomainConfig,
        _: &ProbeAuthorization,
    ) -> Result<ProbeReport, WorkerError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(ProbeReport::new(
            self.identity.clone(),
            json!({"state":"DISCONNECTED"}),
            true,
        ))
    }
}
fn verifier(config: &DomainConfig) -> (Verifier, Arc<Port>, Arc<ManualClock>) {
    let port = Arc::new(Port {
        registry: DomainRegistry::new(&"a".repeat(32)).unwrap(),
        reads: AtomicUsize::new(0),
        identity: json!({"model":"AQ6370D","serial":"OSA-1"}),
    });
    port.registry.configure(config.clone()).unwrap();
    let clock = Arc::new(ManualClock::default());
    (Verifier::new(port.clone(), clock.clone()), port, clock)
}
#[test]
fn catalog_rejects_model_transport_mismatch() {
    let original = config("osa", 1);
    assert!(admit(&original).is_ok());
    for (profile, resource) in [
        ("usb-visa", "USB0::0x1313::0x807B::TEST::INSTR"),
        ("gpib-visa", "TCPIP0::10.0.0.1::INSTR"),
    ] {
        let mut wrong = original.clone();
        wrong.profile_id = Some(profile.into());
        wrong.params = json!({"resource":resource});
        assert!(admit(&wrong).is_err());
    }
    let mut wrong = original;
    wrong.driver_kind = "gain".into();
    assert!(admit(&wrong).is_err());
}
#[test]
fn only_current_bound_proof_registers() {
    let config = config("osa", 1);
    let (verifier, port, clock) = verifier(&config);
    let proof = verifier.probe(&config, &consent(&config)).unwrap();
    assert!(verifier
        .register_verified(&"f".repeat(32), &"b".repeat(64), 1)
        .is_err());
    assert!(verifier
        .register_verified(&proof.proof_id, &"c".repeat(64), 1)
        .is_err());
    assert!(verifier
        .register_verified(&proof.proof_id, &"b".repeat(64), 2)
        .is_err());
    clock.wait(Duration::from_secs(60));
    assert!(verifier
        .register_verified(&proof.proof_id, &"b".repeat(64), 1)
        .is_err());
    let proof = verifier.probe(&config, &consent(&config)).unwrap();
    port.registry.fence(&config.domain).unwrap();
    assert!(verifier
        .register_verified(&proof.proof_id, &"b".repeat(64), 1)
        .is_err());
    let proof = verifier.probe(&config, &consent(&config)).unwrap();
    verifier
        .register_verified(&proof.proof_id, &"b".repeat(64), 1)
        .unwrap();
    assert!(verifier.registered(&config.domain));
    let verified = port.registry.config(&config.domain).unwrap();
    assert!(verifier.authorize_connection(&verified).is_ok());
    let (unverified, _, _) = self::verifier(&verified);
    assert!(
        unverified.authorize_connection(&verified).is_err(),
        "Saved identity is not native verification authority"
    );
    assert!(verifier
        .register_verified(&proof.proof_id, &"b".repeat(64), 1)
        .is_err());
}
#[test]
fn readonly_stage_cannot_normal_connect_gain_or_voltage() {
    for kind in ["gain", "voltage"] {
        let config = config(kind, 1);
        let (verifier, port, _) = verifier(&config);
        let error = verifier.probe(&config, &consent(&config)).unwrap_err();
        assert_eq!(error.code, "ManualVerificationRequired");
        assert_eq!(port.reads.load(Ordering::SeqCst), 0);
        assert!(!verifier.registered(&config.domain));
    }
}
#[test]
fn setup_requires_registered_distinct_serials() {
    let mut left = config("mdt", 1);
    left.expected_identity = json!({"transport_serial":"2110148249-10"});
    let mut right = config("mdt", 2);
    right.expected_identity = json!({"transport_serial":"160721175410"});
    let mut setup = DomainConfig {
        domain: DomainRef {
            kind: "setup".into(),
            id: "e".repeat(32),
        },
        config_rev: 1,
        driver_kind: "fiber".into(),
        model_id: "fiber-coupling".into(),
        profile_id: None,
        params: json!({}),
        expected_identity: json!({}),
        members: vec![left.clone(), right.clone()],
    };
    assert!(admit(&setup).is_ok());
    setup.members[1].expected_identity = left.expected_identity.clone();
    assert!(admit(&setup).is_err());
    setup.members = vec![right.clone(), right.clone()];
    assert!(admit(&setup).is_err());
    setup.members = vec![right];
    assert!(admit(&setup).is_ok());
    setup.members[0].expected_identity = json!({"serial":"unregistered"});
    assert!(admit(&setup).is_err());
}

#[test]
fn physical_alias_and_setup_member_claims_overlap_until_confirmed_release() {
    use yang_drivers::lifecycle::{CleanupReport, CleanupStep};
    let claims = Claims::default();
    let osa = config("osa", 1);
    let lease = claims.reserve(&osa).unwrap();
    let mut alias = config("osa", 2);
    alias.params = json!({"resource":"GPIB::04::INSTR"});
    assert!(claims.reserve(&alias).is_err());
    let report = CleanupReport::new(
        "a".repeat(32),
        vec![CleanupStep {
            role: "osa".into(),
            action: "close".into(),
            error: None,
        }],
        None,
        vec![],
    )
    .unwrap();
    claims.confirm_release(&lease, &report).unwrap();
    assert!(claims.confirm_release(&lease, &report).is_err());
    claims.reserve(&alias).unwrap();
    let mut member = config("mdt", 3);
    member.expected_identity = json!({"transport_serial":"2110148249-10"});
    let lease = claims.reserve(&member).unwrap();
    let mut alias = member.clone();
    alias.domain.id = "c".repeat(32);
    alias.params = json!({"port":"COM4"});
    assert!(claims.reserve(&alias).is_err());
    drop(lease);
    assert!(
        claims.reserve(&alias).is_err(),
        "Drop must not manufacture resource release"
    );
}
#[test]
fn automatic_check_defers_owned_domain_and_expires_changed_binding() {
    let config = config("osa", 1);
    let (verifier, port, clock) = verifier(&config);
    assert!(verifier.due_check().is_none());
    verifier
        .authorize_checks(&config, &consent(&config), 10)
        .unwrap();
    let context = port.registry.bind(&config.domain, &"c".repeat(32)).unwrap();
    clock.wait(Duration::from_secs(11));
    assert!(
        verifier.due_check().is_none(),
        "owned observation lane remains sole reader"
    );
    let cleanup = yang_drivers::lifecycle::CleanupReport::new(
        "c".repeat(32),
        vec![yang_drivers::lifecycle::CleanupStep {
            role: "osa".into(),
            action: "close".into(),
            error: None,
        }],
        None,
        vec![],
    )
    .unwrap();
    port.registry.release(&context, &cleanup).unwrap();
    assert!(verifier.due_check().is_some());
    let mut revision = config.clone();
    revision.config_rev = 2;
    port.registry.configure(revision).unwrap();
    clock.wait(Duration::from_secs(11));
    assert!(
        verifier.due_check().is_none(),
        "revision change invalidates consent"
    );
    let gain = self::config("gain", 5);
    port.registry.configure(gain.clone()).unwrap();
    assert!(verifier
        .authorize_checks(&gain, &consent(&gain), 10)
        .is_err());
}
