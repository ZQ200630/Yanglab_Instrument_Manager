#[path = "support/mdt_wire.rs"]
mod peer;
use peer::Wire;
use yang_drivers::mdt::{Axis, RotaryMode, VoltageLimit};
#[test]
fn settings_do_not_relax_project_ceiling() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    let before = w.commands().len();
    assert!(m.set_axis_maximum(Axis::X, 76.).is_err());
    assert!(m.set_axis_minimum(Axis::X, 21.).is_err());
    assert!(m.set_axis_maximum(Axis::Y, 29.).is_err());
    assert!(w.commands()[before..].iter().all(|c| !c.contains('=')));
    assert_eq!(m.set_axis_maximum(Axis::X, 70.).unwrap().maximum_v, 70.);
    m.close().unwrap();
}
#[test]
fn complete_typed_settings_have_confirmed_readback() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    assert_eq!(m.set_friendly_name("Left stage").unwrap(), "Left stage");
    assert_eq!(m.get_friendly_name().unwrap(), "Left stage");
    assert!(m.set_echo_enabled(true).unwrap());
    assert!(m.get_echo_enabled().unwrap());
    assert!(!m.set_echo_enabled(false).unwrap());
    assert_eq!(m.get_hardware_voltage_limit().unwrap(), VoltageLimit::V75);
    assert_eq!(m.set_display_intensity(15).unwrap(), 15);
    assert_eq!(m.get_display_intensity().unwrap(), 15);
    assert_eq!(m.set_dac_step(45).unwrap(), 45);
    assert_eq!(m.get_dac_step().unwrap(), 45);
    assert!(m.set_compatibility_enabled(true).unwrap());
    assert!(m.get_compatibility_enabled().unwrap());
    assert_eq!(
        m.set_rotary_mode(RotaryMode::TenTurn).unwrap(),
        RotaryMode::TenTurn
    );
    assert_eq!(m.get_rotary_mode().unwrap(), RotaryMode::TenTurn);
    assert!(!m.set_push_to_adjust_disabled(false).unwrap());
    assert!(!m.get_push_to_adjust_disabled().unwrap());
    assert!(!m.get_master_scan_enabled().unwrap());
    assert_eq!(m.get_master_scan_voltage().unwrap(), 0.);
    assert_eq!(m.set_axis_minimum(Axis::X, 10.).unwrap().minimum_v, 10.);
    assert!(!m.status().unwrap().axis_command_known);
    m.close().unwrap();
}
#[test]
fn settings_validate_before_any_setter_and_factory_never_arms() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    let at = w.commands().len();
    assert!(m.set_friendly_name("bad\nname").is_err());
    assert!(m.set_display_intensity(16).is_err());
    assert!(m.set_dac_step(0).is_err());
    assert!(m.restore_factory_defaults(false).is_err());
    assert!(w.commands()[at..]
        .iter()
        .all(|c| !c.contains('=') && c != "restore"));
    let s = m.restore_factory_defaults(true).unwrap();
    assert!(!s.axis_command_known);
    assert!(m.set_axis_voltage(Axis::X, 0.1).is_err());
    m.close().unwrap();
}
#[test]
fn setter_readback_mismatch_stops_and_holds() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    w.data
        .lock()
        .unwrap()
        .overrides
        .insert("intensity?".into(), b"7\n*\n".to_vec());
    assert!(m.set_display_intensity(8).is_err());
    assert!(!m.status().unwrap().axis_command_known);
    assert!(!w.commands().iter().any(|c| c == "allvoltage=0"));
    m.close().unwrap();
}
