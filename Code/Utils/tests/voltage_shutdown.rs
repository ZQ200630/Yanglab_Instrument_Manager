#[path = "support/voltage_wire.rs"]
mod wire;
use std::time::Duration;
use yang_drivers::{
    clock::Clock,
    lifecycle::DriverState,
    voltage::{encode_voltages, ZeroState},
};
#[test]
fn fault_zeroes_all_channels() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    d.set_all([1.; 8]).unwrap();
    p.data.lock().unwrap().auto = false;
    wire::until(|| d.zero_evidence().state() == ZeroState::Unknown);
    p.clock.wait(Duration::from_secs(4));
    wire::until(|| d.state() == DriverState::Fault);
    wire::until(|| {
        p.data.lock().unwrap().writes.last().unwrap().1 == encode_voltages([0.; 8], 14.).unwrap()
    });
    assert!(d.has_resource_responsibility());
    d.close().unwrap();
}
#[test]
fn late_zero_evidence_does_not_mutate_old_report() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    p.data.lock().unwrap().hold = true;
    p.held();
    let old = d.close().unwrap();
    let old_bytes = serde_json::to_vec(&old).unwrap();
    assert!(!old.resources_released());
    assert_ne!(old.voltage_zero().unwrap()["state"], "measured_zero");
    p.release();
    let new = d.close().unwrap();
    assert!(new.resources_released());
    assert_eq!(new.voltage_zero().unwrap()["state"], "measured_zero");
    assert_eq!(serde_json::to_vec(&old).unwrap(), old_bytes);
}
#[test]
fn failed_close_only_retries_release_without_fabricating_late_zero() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    {
        let mut s = p.data.lock().unwrap();
        s.auto = false;
        s.fail_close = true;
    }
    let old = d.close().unwrap();
    assert!(!old.resources_released());
    let before = p.data.lock().unwrap().writes.len();
    {
        let mut s = p.data.lock().unwrap();
        s.auto = true;
        s.fail_close = false;
    }
    let new = d.close().unwrap();
    assert!(new.resources_released());
    assert_ne!(new.voltage_zero().unwrap()["state"], "measured_zero");
    assert_eq!(p.data.lock().unwrap().writes.len(), before);
}
#[test]
fn incomplete_close_keeps_port_until_reader_finishes() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    p.data.lock().unwrap().hold = true;
    p.held();
    let first = d.close().unwrap();
    assert!(!first.resources_released());
    assert_eq!(
        p.data.lock().unwrap().closes,
        0,
        "close must not destroy an in-flight read"
    );
    p.release();
    let second = d.close().unwrap();
    assert!(second.resources_released());
    assert!(d.resources_released());
}
#[test]
fn emergency_zero_is_immediate_and_new_normal_work_is_explicit() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    d.set_all([2.; 8]).unwrap();
    let before = p.data.lock().unwrap().writes.len();
    d.zero(true).unwrap();
    let writes = p.data.lock().unwrap().writes.clone();
    assert_eq!(writes.len(), before + 1);
    assert_eq!(
        writes.last().unwrap().1,
        encode_voltages([0.; 8], 14.).unwrap()
    );
    d.set_channel(1, 0.2).unwrap();
    d.close().unwrap();
}
#[test]
fn late_completed_close_reconciles_when_release_precedes_retry() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    p.data.lock().unwrap().hold = true;
    p.held();
    let pending = d.close().unwrap();
    assert!(!pending.resources_released());
    p.release();
    wire::until(|| d.resources_released());
    let complete = d.close().unwrap();
    assert!(complete.resources_released(),"late native receipt must replace only the driver's current view, not the immutable old report");
    assert!(!pending.resources_released());
    assert_eq!(complete.voltage_zero().unwrap()["state"], "measured_zero");
}
#[test]
fn writer_failure_is_not_replayed_and_release_does_not_erase_zero_error() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    let before = p.data.lock().unwrap().writes.len();
    p.data.lock().unwrap().fail_write = true;
    assert!(d.set_channel(1, 0.1).is_err());
    assert_eq!(d.state(), DriverState::Fault);
    assert_eq!(
        p.data.lock().unwrap().writes.len(),
        before + 1,
        "do not replay a failed nonzero frame"
    );
    assert!(d.has_resource_responsibility());
    let cleanup = d.close().unwrap();
    assert!(cleanup.resources_released());
    assert_ne!(cleanup.voltage_zero().unwrap()["state"], "measured_zero");
    assert!(
        d.cleanup_error().is_some(),
        "resource release is not evidence of successful zero cleanup"
    );
}
#[test]
fn metadata_stop_fences_queued_nonzero_while_read_is_pending() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    let stop = d.stop_handle();
    p.data.lock().unwrap().hold = true;
    p.held();
    let before = p.data.lock().unwrap().writes.len();
    let pending = std::thread::spawn(move || {
        let result = d.set_all([1.; 8]);
        (d, result)
    });
    std::thread::sleep(Duration::from_millis(10));
    stop.request_stop();
    p.release();
    let (mut d, result) = pending.join().unwrap();
    assert!(result.is_err());
    let writes = p.data.lock().unwrap().writes.clone();
    assert!(writes[before..]
        .iter()
        .all(|(_, bytes)| bytes[..16] == [0; 16]));
    assert!(d.close().unwrap().resources_released());
}
#[test]
fn drop_retains_pending_reader_until_zero_and_actual_release() {
    let p = wire::Peer::new();
    let book = yang_drivers::transport::ResourceBook::isolated();
    let mut d = yang_drivers::voltage::VoltageSource::with_backend(
        yang_drivers::voltage::VoltageConfig {
            port: "COM12".into(),
            close_timeout: Duration::from_millis(100),
            ..Default::default()
        },
        book.clone(),
        p.clock.clone(),
        std::sync::Arc::new(wire::Backend(p.clone())),
    )
    .unwrap();
    d.connect().unwrap();
    p.data.lock().unwrap().hold = true;
    p.held();
    drop(d);
    assert_eq!(p.data.lock().unwrap().closes, 0);
    let resource = yang_drivers::transport::serial::canonical_com("COM12").unwrap();
    assert!(book.is_reserved(&resource));
    p.release();
    wire::until(|| p.data.lock().unwrap().closes > 0);
    yang_drivers::voltage::retry_retained();
    wire::until(|| !book.is_reserved(&resource));
    assert_eq!(
        p.data.lock().unwrap().writes.last().unwrap().1[..16],
        [0; 16]
    );
}
