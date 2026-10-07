#[path = "support/pm_wire.rs"]
mod peer;
use peer::Wire;
use std::time::Duration;
use yang_drivers::{lifecycle::DriverState, pm400::MeasurementKind as K, transport::Deadline};
fn deadline() -> Deadline {
    Deadline::after(Duration::from_secs(1))
}
#[test]
fn nine_measurement_kinds_keep_units() {
    let w = Wire::new(371);
    let mut p = w.pm();
    p.connect().unwrap();
    for (k, suffix, unit) in [
        (K::Power, "POWer", "W"),
        (K::Current, "CURRent:DC", "A"),
        (K::Voltage, "VOLTage:DC", "V"),
        (K::Energy, "ENERgy", "J"),
        (K::Frequency, "FREQuency", "Hz"),
        (K::PowerDensity, "PDENsity", "W/cm^2"),
        (K::EnergyDensity, "EDENsity", "J/cm^2"),
        (K::Resistance, "RESistance", "ohm"),
        (K::Temperature, "TEMPerature", "degC"),
    ] {
        let r = p.measure(k, deadline()).unwrap();
        assert_eq!(r.value, 1.25);
        assert_eq!(r.unit, unit);
        assert!(w.commands().contains(&format!("MEASure:SCALar:{suffix}?")));
    }
    w.data
        .lock()
        .unwrap()
        .replies
        .insert("SENSe:POWer:DC:UNIT?".into(), "DBM".into());
    assert_eq!(p.measure_power(deadline()).unwrap().unit, "DBM");
    p.close().unwrap();
}
#[test]
fn unsupported_sensor_never_receives_command() {
    for flags in [0, 1, 2, 49, 81, 256] {
        let w = Wire::new(flags);
        let mut p = w.pm();
        p.connect().unwrap();
        for (k, allowed) in [
            (K::Power, flags & 1 != 0),
            (K::PowerDensity, flags & 1 != 0),
            (K::Energy, flags & 2 != 0),
            (K::EnergyDensity, flags & 2 != 0),
            (K::Temperature, flags & 256 != 0),
        ] {
            let n = w.commands().len();
            assert_eq!(p.measure(k, deadline()).is_ok(), allowed);
            if !allowed {
                assert_eq!(w.commands().len(), n);
                assert!(p.configure(k).is_err());
                assert_eq!(w.commands().len(), n);
            }
        }
        p.close().unwrap();
    }
}
#[test]
fn fetch_read_configuration_and_actions_are_explicit() {
    let w = Wire::new(1);
    let mut p = w.pm();
    p.connect().unwrap();
    assert_eq!(p.get_configuration().unwrap(), K::Power);
    assert_eq!(p.fetch(K::Power).unwrap().unit, "W");
    p.configure(K::Current).unwrap();
    p.initiate().unwrap();
    assert_eq!(p.read(K::Voltage, deadline()).unwrap().unit, "V");
    p.abort().unwrap();
    assert_eq!(p.state(), DriverState::Ready);
    p.close().unwrap();
    assert_eq!(w.commands().iter().filter(|s| *s == "ABORt").count(), 1);
}
#[test]
fn cancel_and_late_measurement_keep_ownership() {
    let w = Wire::new(1);
    let mut p = w.pm();
    p.connect().unwrap();
    let stop = p.stop_handle();
    w.data.lock().unwrap().block_measure = true;
    let job = std::thread::spawn(move || {
        let result = p.measure_current(deadline());
        (p, result)
    });
    w.entered();
    stop.cancel_measurement();
    assert!(!w.data.lock().unwrap().closed.contains(&11));
    w.release();
    let (mut p, result) = job.join().unwrap();
    assert!(result.is_err());
    assert_eq!(p.state(), DriverState::Fault);
    assert!(p.has_resource_responsibility());
    let n = w.commands().len();
    assert!(p.measure_current(deadline()).is_err());
    assert_eq!(w.commands().len(), n);
    assert!(p.close().unwrap().resources_released());
}
#[test]
fn malformed_or_trailing_values_never_become_measurement() {
    for value in ["NaN", "9.9e37", "1\n2", "inf"] {
        let w = Wire::new(371);
        let mut p = w.pm();
        p.connect().unwrap();
        w.data
            .lock()
            .unwrap()
            .replies
            .insert("MEASure:SCALar:CURRent:DC?".into(), value.into());
        assert!(p.measure_current(deadline()).is_err());
        p.close().unwrap();
    }
    let w = Wire::new(1);
    let mut p = w.pm();
    p.connect().unwrap();
    w.data.lock().unwrap().extra = true;
    assert!(p.identify().is_err());
    assert_eq!(p.state(), DriverState::Fault);
    p.close().unwrap();
}

#[test]
fn native_deadline_and_primary_protocol_error_remain_truthful() {
    use yang_drivers::DriverError;
    let w = Wire::new(1);
    let mut p = w.pm();
    p.connect().unwrap();
    w.data
        .lock()
        .unwrap()
        .replies
        .insert("MEASure:SCALar:CURRent:DC?".into(), "1\n2".into());
    assert!(matches!(
        p.measure_current(deadline()),
        Err(DriverError::Protocol(_))
    ));
    p.close().unwrap();
    let w = Wire::new(1);
    let mut p = w.pm();
    p.connect().unwrap();
    w.data.lock().unwrap().block_measure = true;
    let job = std::thread::spawn(move || {
        let r = p.measure_current(Deadline::after(Duration::from_millis(5)));
        (p, r)
    });
    w.entered();
    std::thread::sleep(Duration::from_millis(10));
    w.release();
    let (mut p, r) = job.join().unwrap();
    assert!(matches!(r, Err(DriverError::Timeout { .. })));
    assert!(p.has_resource_responsibility());
    p.close().unwrap();
}
