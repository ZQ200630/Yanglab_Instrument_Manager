#[path = "support/mdt_wire.rs"]
mod peer;
use peer::Wire;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use yang_drivers::{
    clock::{Clock, ManualClock},
    lifecycle::DriverState,
    mdt::Axis,
};
#[test]
fn connect_close_recover_write_no_settings_or_voltage() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    assert!(!m.status().unwrap().axis_command_known);
    m.recover().unwrap();
    assert_eq!(m.get_all_voltages().unwrap(), [20., 30., 40.]);
    assert_eq!(m.get_axis_state(Axis::Y).unwrap().actual_v, 30.);
    assert!(m.get_supported_commands().unwrap().contains("xmin?"));
    assert_eq!(
        m.get_product_information().unwrap(),
        ("MDT693B".into(), "1.23".into())
    );
    assert!(m.close().unwrap().resources_released());
    assert!(w.commands().iter().all(|c| c == "?" || c.ends_with('?')));
}
#[test]
fn external_voltage_is_truthfully_reported() {
    let w = Wire::new();
    w.data
        .lock()
        .unwrap()
        .values
        .insert("xvoltage?".into(), "90".into());
    let mut m = w.driver();
    m.connect().unwrap();
    let s = m.status().unwrap();
    assert!(s.restricted);
    assert_eq!(s.axes[&Axis::X].actual_v, 90.);
    assert_eq!(m.get_axis_voltage(Axis::X).unwrap(), 90.);
    assert!(!m.status().unwrap().axis_command_known);
    m.close().unwrap();
    assert!(w.commands().iter().all(|c| !c.contains('=')));
}
#[test]
fn recover_leaves_motion_disarmed() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    w.data
        .lock()
        .unwrap()
        .values
        .insert("yvoltage?".into(), "31".into());
    let s = m.recover().unwrap();
    assert_eq!(s.axes[&Axis::Y].actual_v, 31.);
    assert!(!s.axis_command_known);
    assert_eq!(m.state(), DriverState::Ready);
    m.close().unwrap();
}
#[test]
fn monitor_fault_holds_not_zeroes() {
    let w = Wire::new();
    let clock = Arc::new(ManualClock::default());
    let mut m = w.driver_with(clock.clone(), true);
    m.connect().unwrap();
    w.data
        .lock()
        .unwrap()
        .overrides
        .insert("xvoltage?".into(), b"bad\n*\n".to_vec());
    for _ in 0..4 {
        clock.wait(Duration::from_millis(500));
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end && m.state() != DriverState::Fault {
            std::thread::sleep(Duration::from_millis(2));
            if m.status().unwrap().fault_evidence.is_some() {
                break;
            }
        }
    }
    let end = Instant::now() + Duration::from_secs(2);
    while m.state() != DriverState::Fault && Instant::now() < end {
        clock.wait(Duration::from_millis(500));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(m.state(), DriverState::Fault);
    assert!(!m.status().unwrap().axis_command_known);
    m.close().unwrap();
    assert!(w.commands().iter().all(|c| !c.contains('=')));
}
#[test]
fn close_during_pending_query_never_destroys_inflight_port() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    w.data.lock().unwrap().block_read = true;
    let worker = m;
    let stop = worker.stop_handle();
    let job = std::thread::spawn(move || {
        let r = worker.get_axis_voltage(Axis::X);
        (worker, r)
    });
    w.entered();
    stop.request_stop();
    assert_eq!(w.data.lock().unwrap().closed, 0);
    w.release();
    let (mut m, r) = job.join().unwrap();
    assert!(r.is_err());
    assert!(m.close().unwrap().resources_released());
}

#[test]
fn read_observation_does_not_clear_restriction_without_recovery() {
    let w = Wire::new();
    w.data
        .lock()
        .unwrap()
        .values
        .insert("xvoltage?".into(), "90".into());
    let mut m = w.driver();
    m.connect().unwrap();
    w.data
        .lock()
        .unwrap()
        .values
        .insert("xvoltage?".into(), "20".into());
    assert_eq!(m.get_axis_voltage(Axis::X).unwrap(), 20.);
    assert!(m.status().unwrap().restricted);
    assert!(!m.recover().unwrap().restricted);
    m.close().unwrap();
}

#[test]
fn failed_release_retries_only_release_and_keeps_old_receipt() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    w.data.lock().unwrap().fail_close = true;
    let old = m.close().unwrap();
    assert!(!old.resources_released());
    assert!(m.has_resource_responsibility());
    let n = w.commands().len();
    w.data.lock().unwrap().fail_close = false;
    assert!(m.close().unwrap().resources_released());
    assert_eq!(w.commands().len(), n);
    assert!(!old.resources_released());
}

#[test]
fn identity_requires_nonempty_device_serial() {
    let w = Wire::new();
    w.data
        .lock()
        .unwrap()
        .values
        .insert("serial?".into(), "".into());
    let mut m = w.driver();
    assert!(m.connect().is_err());
    assert!(!m.has_resource_responsibility());
}
