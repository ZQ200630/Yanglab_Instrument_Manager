#[path = "support/mdt_wire.rs"]
mod peer;
use peer::Wire;
use yang_drivers::{lifecycle::DriverState, mdt::Axis};
fn armed(w: &std::sync::Arc<Wire>) -> yang_drivers::mdt::Mdt693b {
    let mut m = w.driver();
    m.connect().unwrap();
    let a = m.baseline_attestation(true).unwrap();
    m.adopt_current_axis_baseline(a).unwrap();
    m
}
#[test]
fn read_only_baseline_cannot_move() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    assert!(m.set_axis_voltage(Axis::X, 20.1).is_err());
    assert!(m.set_all_voltages(20., true).is_err());
    assert!(m.ramp_to_zero().is_err());
    assert!(w.commands().iter().all(|c| !c.contains('=')));
    m.close().unwrap();
}
#[test]
fn ramp_steps_and_intervals_are_bounded() {
    let w = Wire::new();
    let mut m = armed(&w);
    m.set_axis_voltage(Axis::X, 20.25).unwrap();
    m.set_axis_voltage(Axis::X, 20.3).unwrap();
    let d = w.data.lock().unwrap();
    let steps: Vec<_> = d
        .writes_at
        .iter()
        .filter(|(c, _)| c.starts_with("xvoltage="))
        .collect();
    assert!(steps.len() >= 4);
    let mut previous = 20.;
    for (c, _) in &steps {
        let v = c.split_once('=').unwrap().1.parse::<f64>().unwrap();
        assert!(v - previous <= 0.100001);
        previous = v;
    }
    for pair in steps.windows(2) {
        assert!(pair[1].1 - pair[0].1 >= std::time::Duration::from_millis(50));
    }
    drop(d);
    assert!(m.set_axis_voltage(Axis::X, 75.01).is_err());
    m.close().unwrap();
}
#[test]
fn arrow_and_master_scan_are_capability_gated() {
    let w = Wire::new();
    let mut m = armed(&w);
    assert!(m.set_master_scan_enabled(true, false).is_err());
    assert!(m.set_master_scan_voltage(0.1, true).is_err());
    m.set_master_scan_enabled(true, true).unwrap();
    m.set_master_scan_voltage(0.2, true).unwrap();
    assert!(m.set_master_scan_enabled(false, true).is_err());
    m.set_master_scan_voltage(0., true).unwrap();
    m.set_master_scan_enabled(false, true).unwrap();
    assert_eq!(m.select_next_channel().unwrap(), Axis::Y);
    let s = m.increment_selected().unwrap();
    assert!((s.axes[&Axis::Y].actual_v - 30.011444266422522).abs() < 1e-6);
    m.decrement_selected().unwrap();
    m.set_dac_step(1000).unwrap();
    let before = w.commands().len();
    assert!(m.increment_selected().is_err());
    assert!(w.commands()[before..]
        .iter()
        .all(|c| !c.starts_with('\u{1b}')));
    m.close().unwrap();
    let w = Wire::new();
    w.data.lock().unwrap().overrides.insert(
        "?".into(),
        format!("{}\n*\n", peer::QUERIES.join("\n")).into_bytes(),
    );
    let mut m = w.driver();
    m.connect().unwrap();
    assert!(m.select_next_channel().is_err());
    assert!(m.emergency_zero(true).is_err());
    assert!(w
        .commands()
        .iter()
        .all(|c| !c.starts_with('\u{1b}') && !c.contains('=')));
    m.close().unwrap();
}
#[test]
fn partial_move_holds_and_invalidates() {
    let w = Wire::new();
    let mut m = armed(&w);
    w.data.lock().unwrap().fail_setter = Some(2);
    assert!(m.set_axis_voltage(Axis::X, 20.4).is_err());
    assert_eq!(m.state(), DriverState::Fault);
    assert!(!m.status().unwrap().axis_command_known);
    assert!(w.commands().contains(&"xvoltage=20.1".into()));
    assert!(!w
        .commands()
        .iter()
        .any(|c| c == "allvoltage=0" || c == "xvoltage=20"));
    m.close().unwrap();
}
#[test]
fn emergency_zero_requires_confirmation_and_full_readback() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    assert!(m.emergency_zero(false).is_err());
    let at = w.commands().len();
    let s = m.emergency_zero(true).unwrap();
    assert!(s.axis_command_known);
    assert_eq!(m.get_all_voltages().unwrap(), [0.; 3]);
    let writes: Vec<_> = w.commands()[at..]
        .iter()
        .filter(|c| c.contains('='))
        .cloned()
        .collect();
    assert_eq!(writes, ["msvoltage=0", "allvoltage=0", "msenable=0"]);
    m.close().unwrap();
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    w.data
        .lock()
        .unwrap()
        .overrides
        .insert("zvoltage?".into(), b"0.2\n*\n".to_vec());
    assert!(m.emergency_zero(true).is_err());
    assert!(!m.status().unwrap().axis_command_known);
    assert!(m.set_axis_voltage(Axis::X, 0.1).is_err());
    m.close().unwrap();
}
#[test]
fn external_change_and_recovery_revoke_attestation() {
    let w = Wire::new();
    let mut m = armed(&w);
    let old = m.baseline_attestation(true).unwrap();
    m.recover().unwrap();
    assert!(m.adopt_current_axis_baseline(old).is_err());
    let fresh = m.baseline_attestation(true).unwrap();
    m.adopt_current_axis_baseline(fresh).unwrap();
    w.data
        .lock()
        .unwrap()
        .values
        .insert("zvoltage?".into(), "40.1".into());
    assert!(m.set_axis_voltage(Axis::X, 20.1).is_err());
    assert!(!m.status().unwrap().axis_command_known);
    assert!(w.commands().iter().all(|c| !c.contains('=')));
    m.close().unwrap();
}
#[test]
fn unequal_all_voltage_and_explicit_zero_hold_on_residual() {
    let w = Wire::new();
    for k in ["xvoltage?", "yvoltage?", "zvoltage?"] {
        w.data.lock().unwrap().values.insert(k.into(), "0.2".into());
    }
    w.data
        .lock()
        .unwrap()
        .values
        .insert("yvoltage?".into(), "0.1".into());
    let mut m = armed(&w);
    m.set_all_voltages(0.3, true).unwrap();
    assert_eq!(m.get_all_voltages().unwrap(), [0.3; 3]);
    m.ramp_to_zero().unwrap();
    assert_eq!(m.get_all_voltages().unwrap(), [0.; 3]);
    m.close().unwrap();
}
#[test]
fn stop_during_native_read_never_allows_next_setter() {
    let w = Wire::new();
    let mut m = armed(&w);
    w.data.lock().unwrap().block_read = true;
    std::thread::scope(|scope| {
        let active = scope.spawn(|| m.set_axis_voltage(Axis::X, 20.2));
        w.entered();
        m.stop_handle().request_stop();
        assert!(!m.status().unwrap().axis_command_known);
        w.release();
        assert!(active.join().unwrap().is_err());
    });
    assert!(w.commands().iter().all(|c| !c.contains('=')));
    w.data.lock().unwrap().block_read = false;
    m.recover().unwrap();
    assert!(!m.status().unwrap().axis_command_known);
    m.close().unwrap();
}
#[test]
fn serial_identity_change_disarms_even_when_voltage_is_unchanged() {
    let w = Wire::new();
    let mut m = armed(&w);
    w.data
        .lock()
        .unwrap()
        .values
        .insert("serial?".into(), "160721175410".into());
    assert!(m.set_axis_voltage(Axis::X, 20.1).is_err());
    assert!(!m.status().unwrap().axis_command_known);
    assert!(w.commands().iter().all(|c| !c.contains('=')));
    m.close().unwrap();
}
#[test]
fn equal_all_voltage_needs_no_unused_individual_setter_capability() {
    let w = Wire::new();
    for k in ["xvoltage?", "yvoltage?", "zvoltage?"] {
        w.data.lock().unwrap().values.insert(k.into(), "1".into());
    }
    let mut allowed: Vec<_> = peer::QUERIES.to_vec();
    allowed.extend(
        peer::SETTERS
            .iter()
            .copied()
            .filter(|s| !["xvoltage=", "yvoltage=", "zvoltage="].contains(s)),
    );
    w.data.lock().unwrap().overrides.insert(
        "?".into(),
        format!("{}\n*\n", allowed.join("\n")).into_bytes(),
    );
    let mut m = armed(&w);
    assert_eq!(
        m.set_all_voltages(1.1, true).unwrap().axes[&Axis::Z].actual_v,
        1.1
    );
    m.close().unwrap();
}
#[test]
fn settings_changed_during_ramp_wait_are_rechecked_before_writing() {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };
    use yang_drivers::{
        clock::{Clock, ManualClock},
        mdt::{Mdt693b, MdtConfig},
        transport::ResourceBook,
    };
    struct ChangeClock {
        base: ManualClock,
        wire: Arc<Wire>,
        changed: AtomicBool,
    }
    impl Clock for ChangeClock {
        fn now(&self) -> Duration {
            self.base.now()
        }
        fn wait(&self, d: Duration) {
            self.base.wait(d);
            if !self.changed.swap(true, Ordering::AcqRel) {
                self.wire
                    .data
                    .lock()
                    .unwrap()
                    .values
                    .insert("xmax?".into(), "20.1".into());
            }
        }
    }
    let w = Wire::new();
    let clock = Arc::new(ChangeClock {
        base: ManualClock::default(),
        wire: w.clone(),
        changed: AtomicBool::new(false),
    });
    let mut m = Mdt693b::with_backend(
        MdtConfig {
            port: "COM15".into(),
            start_monitor: false,
            ..MdtConfig::default()
        },
        ResourceBook::isolated(),
        clock,
        Arc::new(peer::Backend(w.clone())),
    )
    .unwrap();
    m.connect().unwrap();
    m.adopt_current_axis_baseline(m.baseline_attestation(true).unwrap())
        .unwrap();
    assert!(m.set_axis_voltage(Axis::X, 20.4).is_err());
    let writes: Vec<_> = w
        .commands()
        .into_iter()
        .filter(|c| c.starts_with("xvoltage="))
        .collect();
    assert_eq!(writes, ["xvoltage=20.1"]);
    assert!(!m.status().unwrap().axis_command_known);
    m.close().unwrap();
}
#[test]
fn emergency_fences_queued_old_motion_without_revoking_new_zero_authority() {
    let w = Wire::new();
    let mut m = armed(&w);
    w.data.lock().unwrap().block_read = true;
    std::thread::scope(|scope| {
        let held = scope.spawn(|| m.get_all_voltages());
        w.entered();
        let queued = scope.spawn(|| m.set_axis_voltage(Axis::X, 20.1));
        std::thread::sleep(std::time::Duration::from_millis(15));
        let urgent = scope.spawn(|| m.emergency_zero(true));
        let end = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while m.status().unwrap().axis_command_known && std::time::Instant::now() < end {
            std::thread::yield_now();
        }
        assert!(!m.status().unwrap().axis_command_known);
        w.release();
        assert!(held.join().unwrap().is_err());
        assert!(queued.join().unwrap().is_err());
        assert!(urgent.join().unwrap().unwrap().axis_command_known);
    });
    assert!(m.status().unwrap().axis_command_known);
    assert!(!w.commands().iter().any(|c| c.starts_with("xvoltage=")));
    m.close().unwrap();
}
#[test]
fn failed_emergency_sequence_never_arms_or_rolls_back() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    w.data.lock().unwrap().fail_setter = Some(2);
    assert!(m.emergency_zero(true).is_err());
    assert!(!m.status().unwrap().axis_command_known);
    assert_eq!(m.state(), DriverState::Fault);
    let writes: Vec<_> = w
        .commands()
        .into_iter()
        .filter(|c| c.contains('='))
        .collect();
    assert_eq!(writes, ["msvoltage=0", "allvoltage=0"]);
    m.close().unwrap();
}
