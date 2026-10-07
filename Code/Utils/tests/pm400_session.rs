#[path = "support/pm_wire.rs"]
mod peer;
use peer::Wire;
use std::{sync::Arc, time::Duration};
use yang_drivers::{
    clock::ManualClock,
    lifecycle::DriverState,
    pm400::{InstrumentInfo, Pm400, SensorInfo, StatusGroup},
    transport::Deadline,
};
#[test]
fn connect_close_preserve_measurement_settings() {
    let w = Wire::new(1);
    let mut p = w.pm();
    let proof = p.connect().unwrap();
    assert_eq!(proof.identity()["model"], "PM400");
    assert_eq!(p.sensor_info().unwrap().name, "S,130C");
    assert_eq!(w.commands(), ["*IDN?", "SYSTem:SENSor:IDN?"]);
    assert!(p.close().unwrap().resources_released());
    assert_eq!(w.commands().len(), 2);
    assert_eq!(p.state(), DriverState::Disconnected);
}
#[test]
fn two_visa_resources_are_independent() {
    let w = Wire::new(1);
    let m = w.manager();
    let mut a = Pm400::new(
        m.clone(),
        "GPIB0::1::INSTR".into(),
        Arc::new(ManualClock::default()),
    )
    .unwrap();
    let mut b = Pm400::new(
        m.clone(),
        "GPIB0::2::INSTR".into(),
        Arc::new(ManualClock::default()),
    )
    .unwrap();
    let mut duplicate = Pm400::new(
        m,
        "gpib0::1::instr".into(),
        Arc::new(ManualClock::default()),
    )
    .unwrap();
    a.connect().unwrap();
    b.connect().unwrap();
    assert!(duplicate.connect().is_err());
    a.close().unwrap();
    assert_eq!(b.identify().unwrap().serial_number, "P400-1");
    b.close().unwrap();
}
#[test]
fn strict_csv_identity_sensor_and_register_evidence() {
    assert_eq!(
        SensorInfo::parse("\"S,130\",SN,cal,1,2,1025")
            .unwrap()
            .raw_flags,
        1025
    );
    for bad in ["S,SN,cal,1,2,-1", "S,SN,cal,1,2,1,2", "\"S,SN,cal,1,2,1"] {
        assert!(SensorInfo::parse(bad).is_err());
    }
    assert!(InstrumentInfo::parse("Thorlabs,PM400,,1").is_err());
    let w = Wire::new(0);
    let mut p = w.pm();
    p.connect().unwrap();
    assert_eq!(p.read_standard_event_status().unwrap(), 32);
    assert_eq!(p.read_status_byte().unwrap(), 16);
    assert_eq!(p.status().read_event(StatusGroup::Operation).unwrap(), 7);
    assert_eq!(
        p.status().read_condition(StatusGroup::Operation).unwrap(),
        3
    );
    p.wait_operation_complete(Deadline::after(Duration::from_secs(1)))
        .unwrap();
    p.close().unwrap();
}
#[test]
fn readonly_probe_and_wrong_identity_do_not_change_settings() {
    let w = Wire::new(0);
    let mut p = w.pm();
    assert!(p.probe_identity().unwrap().release_confirmed());
    assert_eq!(w.commands(), ["*IDN?", "SYSTem:SENSor:IDN?"]);
    w.data
        .lock()
        .unwrap()
        .replies
        .insert("*IDN?".into(), "Other,PM400,SN,1".into());
    assert!(p.connect().is_err());
    assert!(!p.has_resource_responsibility());
}
#[test]
fn late_native_close_retains_immutable_attempt_and_reservation() {
    let w = Wire::new(1);
    let manager = w.manager();
    let mut p = Pm400::with_options(
        manager.clone(),
        "USB0::0x1313::0x8078::P1::INSTR".into(),
        Arc::new(ManualClock::default()),
        yang_drivers::pm400::PmOptions {
            timeout: Duration::from_secs(1),
            close_timeout: Duration::from_millis(30),
        },
    )
    .unwrap();
    p.connect().unwrap();
    w.data.lock().unwrap().block_close = true;
    let old = p.close().unwrap();
    assert!(!old.resources_released());
    w.entered();
    let mut duplicate = Pm400::new(
        manager,
        "USB0::0x1313::0x8078::P1::INSTR".into(),
        Arc::new(ManualClock::default()),
    )
    .unwrap();
    assert!(p.has_resource_responsibility());
    assert!(duplicate.connect().is_err());
    w.release();
    std::thread::sleep(Duration::from_millis(5));
    assert!(p.close().unwrap().resources_released());
    assert!(!old.resources_released());
    duplicate.connect().unwrap();
    duplicate.close().unwrap();
}

#[test]
fn query_packets_never_inflate_configured_timeout() {
    let w = Wire::new(1);
    let mut p = Pm400::with_options(
        w.manager(),
        "GPIB0::4::INSTR".into(),
        Arc::new(ManualClock::default()),
        yang_drivers::pm400::PmOptions {
            timeout: Duration::from_millis(20),
            close_timeout: Duration::from_millis(30),
        },
    )
    .unwrap();
    p.connect().unwrap();
    w.data.lock().unwrap().timeouts.clear();
    p.measure_current(Deadline::after(Duration::from_secs(5)))
        .unwrap();
    assert!(w.data.lock().unwrap().timeouts.iter().all(|t| *t <= 21));
    p.close().unwrap();
}
