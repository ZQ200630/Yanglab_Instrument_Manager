#[path = "support/pm_wire.rs"]
mod peer;
use peer::{settings, Wire};
use yang_drivers::pm400::{
    AdapterType, Date, LimitSelector as L, NumericValue as V, PowerUnit, StatusGroup, Time,
};
#[test]
fn all_property_table_rows_have_typed_rust_methods() {
    let w = Wire::new(371);
    settings(&w);
    let mut p = w.pm();
    p.connect().unwrap();
    macro_rules! num {
        ($fac:ident,$set:ident,$get:ident,$header:literal) => {
            assert_eq!(p.$fac().$set(5.0).unwrap(), 5.0);
            assert_eq!(p.$fac().$get(None).unwrap(), 5.0);
            assert!(w.commands().contains(&format!("{} 5", $header)));
        };
    }
    num!(
        sense,
        set_loss_db,
        get_loss_db,
        "SENSe:CORRection:LOSS:INPut:MAGNitude"
    );
    num!(
        sense,
        set_beam_diameter_mm,
        get_beam_diameter_mm,
        "SENSe:CORRection:BEAMdiameter"
    );
    num!(
        sense,
        set_wavelength_nm,
        get_wavelength_nm,
        "SENSe:CORRection:WAVelength"
    );
    num!(
        sense,
        set_current_range_a,
        get_current_range_a,
        "SENSe:CURRent:DC:RANGe:UPPer"
    );
    num!(
        sense,
        set_current_reference_a,
        get_current_reference_a,
        "SENSe:CURRent:DC:REFerence"
    );
    num!(
        sense,
        set_energy_range_j,
        get_energy_range_j,
        "SENSe:ENERgy:RANGe:UPPer"
    );
    num!(
        sense,
        set_energy_reference_j,
        get_energy_reference_j,
        "SENSe:ENERgy:REFerence"
    );
    num!(
        sense,
        set_power_range_w,
        get_power_range_w,
        "SENSe:POWer:DC:RANGe:UPPer"
    );
    num!(
        sense,
        set_power_reference_w,
        get_power_reference_w,
        "SENSe:POWer:DC:REFerence"
    );
    num!(
        sense,
        set_voltage_range_v,
        get_voltage_range_v,
        "SENSe:VOLTage:DC:RANGe:UPPer"
    );
    num!(
        sense,
        set_voltage_reference_v,
        get_voltage_reference_v,
        "SENSe:VOLTage:DC:REFerence"
    );
    num!(
        sense,
        set_peak_threshold_percent,
        get_peak_threshold_percent,
        "SENSe:PEAKdetector:THReshold"
    );
    num!(
        input,
        set_thermopile_tau_s,
        get_thermopile_tau_s,
        "INPut:THERmopile:ACCelerator:TAU"
    );
    macro_rules! boolean {
        ($fac:ident,$set:ident,$get:ident,$header:literal) => {
            assert!(p.$fac().$set(true).unwrap());
            assert!(p.$fac().$get().unwrap());
            assert!(w.commands().contains(&format!("{} 1", $header)));
        };
    }
    boolean!(
        sense,
        set_current_auto_range,
        get_current_auto_range,
        "SENSe:CURRent:DC:RANGe:AUTO"
    );
    boolean!(
        sense,
        set_current_delta_enabled,
        get_current_delta_enabled,
        "SENSe:CURRent:DC:REFerence:STATe"
    );
    boolean!(
        sense,
        set_energy_delta_enabled,
        get_energy_delta_enabled,
        "SENSe:ENERgy:REFerence:STATe"
    );
    boolean!(
        sense,
        set_power_auto_range,
        get_power_auto_range,
        "SENSe:POWer:DC:RANGe:AUTO"
    );
    boolean!(
        sense,
        set_power_delta_enabled,
        get_power_delta_enabled,
        "SENSe:POWer:DC:REFerence:STATe"
    );
    boolean!(
        sense,
        set_voltage_auto_range,
        get_voltage_auto_range,
        "SENSe:VOLTage:DC:RANGe:AUTO"
    );
    boolean!(
        sense,
        set_voltage_delta_enabled,
        get_voltage_delta_enabled,
        "SENSe:VOLTage:DC:REFerence:STATe"
    );
    boolean!(
        input,
        set_photodiode_lowpass_enabled,
        get_photodiode_lowpass_enabled,
        "INPut:PDIOde:FILTer:LPASs:STATe"
    );
    boolean!(
        input,
        set_thermopile_accelerator_enabled,
        get_thermopile_accelerator_enabled,
        "INPut:THERmopile:ACCelerator:STATe"
    );
    boolean!(
        input,
        set_thermopile_accelerator_auto,
        get_thermopile_accelerator_auto,
        "INPut:THERmopile:ACCelerator:AUTO"
    );
    boolean!(
        system,
        set_beeper_enabled,
        get_beeper_enabled,
        "SYSTem:BEEPer:STATe"
    );
    assert_eq!(p.sense().set_average_count(8).unwrap(), 8);
    assert_eq!(p.sense().get_average_count().unwrap(), 8);
    assert_eq!(
        p.sense()
            .set_photodiode_response_a_per_w(5.0, true)
            .unwrap(),
        5.0
    );
    assert_eq!(
        p.sense().get_photodiode_response_a_per_w(None).unwrap(),
        5.0
    );
    assert_eq!(
        p.sense()
            .set_thermopile_response_v_per_w(5.0, true)
            .unwrap(),
        5.0
    );
    assert_eq!(
        p.sense().get_thermopile_response_v_per_w(None).unwrap(),
        5.0
    );
    assert_eq!(p.sense().set_pyro_response_v_per_j(5.0, true).unwrap(), 5.0);
    assert_eq!(p.sense().get_pyro_response_v_per_j(None).unwrap(), 5.0);
    assert_eq!(
        p.sense().set_power_unit(PowerUnit::Dbm).unwrap(),
        PowerUnit::Dbm
    );
    assert_eq!(p.sense().get_power_unit().unwrap(), PowerUnit::Dbm);
    assert_eq!(p.sense().get_frequency_upper_hz().unwrap(), 5.0);
    assert_eq!(p.sense().get_frequency_lower_hz().unwrap(), 5.0);
    p.sense().start_zero_collection(true).unwrap();
    p.sense().abort_zero_collection().unwrap();
    assert!(!p.sense().get_zero_state().unwrap());
    assert_eq!(p.sense().get_zero_magnitude().unwrap(), 5.0);
    assert_eq!(
        p.input().set_adapter_type(AdapterType::Pyro, true).unwrap(),
        AdapterType::Pyro
    );
    assert_eq!(p.input().get_adapter_type().unwrap(), AdapterType::Pyro);
    assert_eq!(p.display().set_brightness(0.5).unwrap(), 0.5);
    assert_eq!(p.display().get_brightness().unwrap(), 0.5);
    assert_eq!(p.display().set_contrast(0.75).unwrap(), 0.75);
    assert_eq!(p.display().get_contrast().unwrap(), 0.75);
    assert_eq!(p.calibration().get_string().unwrap(), "calibrated 2025");
    assert_eq!(p.system().get_scpi_version().unwrap(), "1999.0");
    assert_eq!(p.system().get_sensor_info().unwrap().raw_flags, 371);
    assert_eq!(p.system().get_line_frequency_hz().unwrap(), 60);
    assert_eq!(p.system().next_error().unwrap().code, 0);
    assert!(p.system().drain_errors(32).unwrap().is_empty());
    p.system().beep().unwrap();
    p.close().unwrap();
}
#[test]
fn maintenance_requires_confirmation_before_write() {
    let w = Wire::new(371);
    settings(&w);
    let mut p = w.pm();
    p.connect().unwrap();
    let n = w.commands().len();
    assert!(p.reset(false).is_err());
    assert!(p.sense().start_zero_collection(false).is_err());
    assert!(p
        .sense()
        .set_photodiode_response_a_per_w(5.0, false)
        .is_err());
    assert!(p
        .sense()
        .set_thermopile_response_v_per_w(5.0, false)
        .is_err());
    assert!(p.sense().set_pyro_response_v_per_j(5.0, false).is_err());
    assert!(p
        .input()
        .set_adapter_type(AdapterType::Thermal, false)
        .is_err());
    assert!(p.status().preset(false).is_err());
    assert_eq!(w.commands().len(), n);
    p.reset(true).unwrap();
    p.status().preset(true).unwrap();
    p.close().unwrap();
}
#[test]
fn capability_checks_precede_any_write() {
    for flags in [0, 1, 2, 16, 32, 64] {
        let w = Wire::new(flags);
        settings(&w);
        let mut p = w.pm();
        p.connect().unwrap();
        let n = w.commands().len();
        if flags & 32 == 0 {
            assert!(p.sense().set_wavelength_nm(5.0).is_err());
        }
        if flags & 1 == 0 {
            assert!(p.sense().set_power_unit(PowerUnit::Watts).is_err());
            assert!(p.sense().set_current_range_a(5.0).is_err());
            assert!(p.input().set_photodiode_lowpass_enabled(true).is_err());
        }
        if flags & 2 == 0 {
            assert!(p.sense().set_energy_range_j(5.0).is_err());
        }
        if flags & 17 != 17 {
            assert!(p
                .sense()
                .set_photodiode_response_a_per_w(5.0, true)
                .is_err());
            assert!(p
                .sense()
                .set_thermopile_response_v_per_w(5.0, true)
                .is_err());
        }
        if flags & 18 != 18 {
            assert!(p.sense().set_pyro_response_v_per_j(5.0, true).is_err());
        }
        if flags & 65 != 65 {
            assert!(p.input().set_thermopile_tau_s(5.0).is_err());
        }
        if flags & 3 == 0 {
            assert!(p.sense().start_zero_collection(true).is_err());
        }
        assert_eq!(w.commands().len(), n);
        p.close().unwrap();
    }
}
#[test]
fn range_selectors_and_readbacks_match() {
    let w = Wire::new(371);
    settings(&w);
    let mut p = w.pm();
    p.connect().unwrap();
    assert_eq!(p.sense().set_loss_db(V::Limit(L::Default)).unwrap(), 3.0);
    assert_eq!(p.sense().get_loss_db(Some(L::Maximum)).unwrap(), 10.0);
    assert_eq!(
        p.sense().set_current_range_a(V::Limit(L::Minimum)).unwrap(),
        0.0
    );
    let n = w.commands().len();
    assert!(p.sense().set_current_range_a(V::Limit(L::Default)).is_err());
    assert!(p.sense().get_wavelength_nm(Some(L::Default)).is_err());
    assert!(p.sense().set_loss_db(f64::NAN).is_err());
    assert_eq!(w.commands().len(), n);
    assert!(p.sense().set_loss_db(10.01).is_err());
    assert!(!w
        .commands()
        .contains(&"SENSe:CORRection:LOSS:INPut:MAGNitude 10.01".into()));
    w.data.lock().unwrap().ignored_header = Some("SENSe:CORRection:LOSS:INPut:MAGNitude".into());
    assert!(p.sense().set_loss_db(4.0).is_err());
    p.close().unwrap();
}
#[test]
fn date_time_masks_and_adapter_limits_are_strict() {
    let w = Wire::new(371);
    settings(&w);
    let mut p = w.pm();
    p.connect().unwrap();
    assert!(Date::new(2025, 2, 29).is_err());
    let date = Date::new(2024, 2, 29).unwrap();
    assert_eq!(p.system().set_date(date).unwrap(), date);
    assert_eq!(p.system().get_date().unwrap(), date);
    assert!(Time::new(24, 0, 0, 0).is_err());
    let time = Time::new(1, 2, 3, 450000).unwrap();
    assert_eq!(p.system().set_time(time).unwrap(), time);
    assert_eq!(p.system().get_time().unwrap(), time);
    assert_eq!(p.system().set_line_frequency_hz(50).unwrap(), 50);
    let n = w.commands().len();
    assert!(p.system().set_line_frequency_hz(55).is_err());
    assert!(p.set_standard_event_enable(256).is_err());
    assert!(p.set_service_request_enable(256).is_err());
    assert!(p
        .status()
        .set_enable(StatusGroup::Operation, 65536)
        .is_err());
    assert!(p.sense().set_average_count(0).is_err());
    assert_eq!(w.commands().len(), n);
    assert_eq!(p.set_standard_event_enable(255).unwrap(), 255);
    assert_eq!(p.get_standard_event_enable().unwrap(), 255);
    assert_eq!(p.set_service_request_enable(128).unwrap(), 128);
    assert_eq!(p.get_service_request_enable().unwrap(), 128);
    for g in [
        StatusGroup::Measurement,
        StatusGroup::Auxiliary,
        StatusGroup::Operation,
        StatusGroup::Questionable,
    ] {
        assert_eq!(p.status().set_positive_transition(g, 65535).unwrap(), 65535);
        assert_eq!(p.status().get_positive_transition(g).unwrap(), 65535);
        assert_eq!(p.status().set_negative_transition(g, 1).unwrap(), 1);
        assert_eq!(p.status().get_negative_transition(g).unwrap(), 1);
        assert_eq!(p.status().set_enable(g, 42).unwrap(), 42);
        assert_eq!(p.status().get_enable(g).unwrap(), 42);
    }
    p.clear_status().unwrap();
    p.mark_operation_complete().unwrap();
    p.wait_to_continue().unwrap();
    assert_eq!(p.self_test().unwrap(), 0);
    for bad in ["phot", "PHOT;*RST", "X"] {
        w.data
            .lock()
            .unwrap()
            .replies
            .insert("INPut:ADAPter:TYPE?".into(), bad.into());
        assert!(p.input().get_adapter_type().is_err());
    }
    p.close().unwrap();
}

#[test]
fn strict_units_and_consuming_error_attribution() {
    let w = Wire::new(371);
    settings(&w);
    let mut p = w.pm();
    p.connect().unwrap();
    w.data
        .lock()
        .unwrap()
        .replies
        .insert("SENSe:POWer:DC:UNIT?".into(), "dbm".into());
    assert!(p.sense().get_power_unit().is_err());
    w.data.lock().unwrap().error_queue.extend([
        "-100,\"old error\"".into(),
        "0,\"No error\"".into(),
        "-200,\"new error\"".into(),
        "0,\"No error\"".into(),
    ]);
    assert!(p.system().beep().is_err());
    assert_eq!(p.last_preexisting_errors().len(), 1);
    assert_eq!(p.last_preexisting_errors()[0].code, -100);
    assert_eq!(p.system().next_error().unwrap().code, 0);
    for (header, bad) in [
        ("SYSTem:DATE?", "2025,2,29"),
        ("SYSTem:TIME?", "12,0,0.1234567"),
        ("SYSTem:LFRequency?", "55"),
    ] {
        w.data
            .lock()
            .unwrap()
            .replies
            .insert(header.into(), bad.into());
    }
    assert!(p.system().get_date().is_err());
    assert!(p.system().get_time().is_err());
    assert!(p.system().get_line_frequency_hz().is_err());
    p.close().unwrap();
}
