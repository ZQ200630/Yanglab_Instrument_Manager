#[path = "support/native.rs"]
mod support;
use serde_json::json;
use std::sync::atomic::Ordering;
use support::*;
use yang_protocol::Phase;
use yang_worker::{scheduler::Backend, session::DriverFactory};
#[test]
fn unknown_action_opens_nothing() {
    let (b, factory) = backend();
    let c = configured(&b);
    let context = b.registry().context(&c.domain).unwrap();
    let result = b.execute(&request(
        "bad",
        "action",
        json!({"name":"raw_scpi","args":{"command":"*RST"}}),
        Some(context),
    ));
    assert_eq!(result.phase, Phase::RejectedBeforeCall);
    assert_eq!(result.error.unwrap().kind, "UnsupportedAction");
    assert_eq!(factory.creates.load(Ordering::SeqCst), 0);
}
#[test]
fn session_retries_retained_release() {
    use yang_drivers::lifecycle::DriverLifecycle;
    use yang_worker::session::InstrumentSession;
    let factory = std::sync::Arc::new(Factory::default());
    let mut device = FactoryPort(factory.clone()).create(&config()).unwrap();
    device.connect().unwrap();
    factory.failed_closes.store(1, Ordering::SeqCst);
    let mut session = InstrumentSession::new(device);
    let first = session.close().unwrap();
    assert!(!first.resources_released());
    assert!(session.has_responsibility());
    let second = session.close().unwrap();
    assert!(second.resources_released());
    assert!(!session.has_responsibility());
    assert_ne!(first.attempt_id(), second.attempt_id());
    assert_eq!(first.unreleased(), &["osa".to_owned()]);
}
#[test]
fn unbound_connect_opens_nothing_even_with_current_verification() {
    let (b, factory) = backend();
    let c = registered(&b);
    let creates = factory.creates.load(Ordering::SeqCst);
    let context = b.registry().context(&c.domain).unwrap();
    assert!(context.connection_id.is_none());
    let reply = b.execute(&request("unbound", "connect", json!({}), Some(context)));
    assert_eq!(reply.phase, Phase::RejectedBeforeCall);
    assert_eq!(factory.creates.load(Ordering::SeqCst), creates);
    assert_eq!(factory.open.load(Ordering::SeqCst), 0);
}
#[test]
fn all_unported_factories_reject_without_loading_native_transports() {
    let factory = yang_worker::session::SystemFactory::new(std::sync::Arc::new(
        yang_drivers::clock::SystemClock::default(),
    ));
    for (kind, model, profile, params) in [
        ("voltage", "voltage", "ch340-serial", json!({"port":"COM7"})),
        ("gain", "gain", "cp210x-serial", json!({"port":"COM8"})),
        (
            "pm400",
            "pm400",
            "usb-visa",
            json!({"resource":"USB0::0x1313::0x8075::P1::INSTR"}),
        ),
        ("mdt", "mdt693b", "serial", json!({"port":"COM9"})),
    ] {
        let mut c = config();
        c.driver_kind = kind.into();
        c.model_id = model.into();
        c.profile_id = Some(profile.into());
        c.params = params;
        let error = match factory.create(&c) {
            Ok(_) => panic!("unported driver must not be created"),
            Err(error) => error,
        };
        assert_eq!(error.code, "UnsupportedDriver");
        assert!(!factory.auxiliary_responsibility());
    }
}
#[test]
fn osa_action_stages_not_inline_samples() {
    let f = Fixture::new();
    let (b, factory) = backend();
    let _worker = worker(b.clone(), &f);
    let context = connected(&b);
    let result = b.execute(&request(
        "read",
        "action",
        json!({"name":"read_trace","args":{}}),
        Some(context),
    ));
    assert_eq!(result.phase, Phase::Completed, "{:?}", result.error);
    let data = result.result.unwrap();
    assert_eq!(data["capture"]["byte_count"], 32);
    assert_eq!(data["capture"]["metadata"]["native_unit"], "dBm");
    assert!(data.get("samples").is_none());
    assert!(data.get("wavelength_nm").is_none());
    assert_eq!(factory.reads.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read_dir(&f.root).unwrap().count(), 2);
}

#[test]
fn read_and_decode_timings_survive_staging() {
    let f = Fixture::new();
    let (b, _) = backend();
    let _w = worker(b.clone(), &f);
    let context = connected(&b);
    let result = b.execute(&request(
        "timed",
        "action",
        json!({"name":"read_trace","args":{}}),
        Some(context),
    ));
    let data = result.result.unwrap();
    assert_eq!(data["timing"]["io_s"], 0.9);
    assert_eq!(data["timing"]["decode_s"], 0.001);
    assert!(data["timing"]["stage_s"].as_f64().is_some_and(|n| n >= 0.));
}

#[test]
fn failed_probe_close_evidence_releases_probe_claim() {
    let (b, factory) = backend();
    let c = configured(&b);
    let context = b.registry().context(&c.domain).unwrap();
    let params = json!({"authorization":{"stage":"readonly","accepted":true,"supervised":false,"retain_session":false,"binding":{"mode":"real","domain":c.domain,"config_rev":1,"model_id":"aq6370","profile_id":"gpib-visa","config_digest":"a".repeat(64)}}});
    factory.failed_connects.store(1, Ordering::SeqCst);
    let failed = b.execute(&request(
        "failed",
        "probe",
        params.clone(),
        Some(context.clone()),
    ));
    assert_eq!(failed.phase, Phase::FailedAfterCallStarted);
    assert_eq!(factory.open.load(Ordering::SeqCst), 0);
    let next = b.execute(&request("new", "probe", params, Some(context)));
    assert_eq!(next.phase, Phase::Completed, "{:?}", next.error);
    assert_eq!(factory.creates.load(Ordering::SeqCst), 2);
}

#[test]
fn stale_session_epoch_and_probe_revision_open_nothing() {
    let (b, factory) = backend();
    let c = configured(&b);
    let original = b.registry().context(&c.domain).unwrap();
    let mut stale_epoch = original.clone();
    stale_epoch.epoch += 1;
    let reply = b.execute(&request("stale", "connect", json!({}), Some(stale_epoch)));
    assert_eq!(reply.phase, Phase::RejectedBeforeCall);
    let mut context = original.clone();
    context.session_id = "f".repeat(32);
    assert_eq!(
        b.execute(&request("session", "connect", json!({}), Some(context)))
            .phase,
        Phase::RejectedBeforeCall
    );
    let reply=b.execute(&request("rev","probe",json!({"authorization":{"stage":"readonly","accepted":true,"supervised":false,"retain_session":false,"binding":{"mode":"real","domain":c.domain,"config_rev":2,"model_id":"aq6370","profile_id":"gpib-visa","config_digest":"a".repeat(64)}}}),Some(original)));
    assert_eq!(reply.phase, Phase::RejectedBeforeCall);
    assert_eq!(factory.creates.load(Ordering::SeqCst), 0);
}

#[test]
fn full_capture_budget_rejects_before_another_instrument_read() {
    let f = Fixture::new();
    let (b, factory) = backend();
    let _w = worker(b.clone(), &f);
    let context = connected(&b);
    let mut last = None;
    for i in 0..32 {
        let result = b.execute(&request(
            &format!("read-{i}"),
            "action",
            json!({"name":"read_trace","args":{}}),
            Some(context.clone()),
        ));
        assert_eq!(result.phase, Phase::Completed);
        last = result.result;
    }
    let full = b.execute(&request(
        "full",
        "action",
        json!({"name":"read_trace","args":{}}),
        Some(context.clone()),
    ));
    assert_eq!(full.phase, Phase::RejectedBeforeCall);
    assert_eq!(factory.reads.load(Ordering::SeqCst), 32);
    let last = last.unwrap();
    let ack=b.execute(&request("ack","ack_capture",json!({"ownership_nonce":f.nonce,"capture_id":last["capture"]["capture_id"],"sha256":last["capture"]["sha256"]}),Some(b.global_context())));
    assert_eq!(ack.phase, Phase::Completed);
    assert_eq!(
        b.execute(&request(
            "after-ack",
            "action",
            json!({"name":"read_trace","args":{}}),
            Some(context)
        ))
        .phase,
        Phase::Completed
    );
    assert_eq!(factory.reads.load(Ordering::SeqCst), 33);
}
