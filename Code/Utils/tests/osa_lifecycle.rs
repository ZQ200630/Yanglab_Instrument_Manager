#[path = "support/osa_wire.rs"]
mod support;
use support::*;
use yang_drivers::{
    lifecycle::DriverState,
    osa::{Osa, TraceId, TransferFormat},
};
#[test]
fn close_never_aborts_panel_sweep() {
    let wire = Wire::new(2, TransferFormat::Ascii);
    let mut osa = wire.osa();
    osa.connect().unwrap();
    assert!(osa.has_resource_responsibility());
    assert!(osa.close().unwrap().resources_released());
    assert!(!osa.has_resource_responsibility());
    assert_eq!(wire.data.lock().unwrap().commands, vec!["*IDN?"]);
}
#[test]
fn owned_sweep_timeout_retains_responsibility() {
    let wire = Wire::new(2, TransferFormat::Ascii);
    {
        let mut data = wire.data.lock().unwrap();
        data.metadata.insert(":INITiate:SMODe?".into(), "1".into());
        data.opc_timeout = true;
    }
    let mut osa = wire.osa();
    osa.connect().unwrap();
    assert!(osa.acquire_trace(TraceId::A, deadline()).is_err());
    assert!(osa.has_resource_responsibility());
    assert_eq!(osa.state(), DriverState::Fault);
    {
        let data = wire.data.lock().unwrap();
        assert_eq!(
            data.commands
                .iter()
                .filter(|c| c.as_str() == ":INITiate:IMMediate")
                .count(),
            1
        );
        assert!(!data.commands.contains(&":ABORt".into()));
    }
    assert!(osa.close().unwrap().resources_released());
    assert!(wire
        .data
        .lock()
        .unwrap()
        .commands
        .contains(&":ABORt".into()));
}
#[test]
fn partial_close_can_retry() {
    let wire = Wire::new(2, TransferFormat::Ascii);
    let mut osa = wire.osa();
    osa.connect().unwrap();
    wire.data.lock().unwrap().close_failures = 1;
    let first = osa.close().unwrap();
    assert!(!first.resources_released());
    assert!(osa.has_resource_responsibility());
    assert_eq!(osa.state(), DriverState::Fault);
    let next = osa.close().unwrap();
    assert!(next.resources_released());
    assert_ne!(first.attempt_id(), next.attempt_id());
    assert!(!first.resources_released());
    assert!(!osa.has_resource_responsibility());
}
#[test]
fn stop_cancels_a_held_read_without_close_overlap_or_late_capture() {
    let wire = Wire::new(2, TransferFormat::Ascii);
    let mut osa = Osa::with_options(
        wire.manager(),
        "GPIB0::4::INSTR".into(),
        std::sync::Arc::new(yang_drivers::clock::ManualClock::default()),
        quick_options(),
    )
    .unwrap();
    osa.connect().unwrap();
    wire.data.lock().unwrap().block_y = true;
    let stop = osa.stop_handle();
    let reader = std::thread::spawn(move || {
        let result = osa.read_trace(TraceId::A, deadline());
        (osa, result)
    });
    wire.entered();
    stop.request_stop();
    assert!(!wire.data.lock().unwrap().closed.contains(&11));
    wire.release();
    let (mut osa, result) = reader.join().unwrap();
    assert!(result.is_err());
    assert!(osa.has_resource_responsibility());
    assert!(osa.close().unwrap().resources_released());
}
#[test]
fn read_only_probe_returns_identity_and_confirmed_release() {
    let wire = Wire::new(2, TransferFormat::Ascii);
    let mut osa = wire.osa();
    let report = osa.probe_identity().unwrap();
    assert_eq!(report.identity()["model"], "AQ6370E");
    assert!(report.release_confirmed());
    assert!(!osa.has_resource_responsibility());
    assert_eq!(wire.data.lock().unwrap().commands, vec!["*IDN?"]);
}
#[test]
fn sweep_gates_and_success_preserve_native_capture() {
    for (mode, trace) in [("2", TraceId::A), ("1", TraceId::B)] {
        let wire = Wire::new(2, TransferFormat::Ascii);
        wire.data
            .lock()
            .unwrap()
            .metadata
            .insert(":INITiate:SMODe?".into(), mode.into());
        let mut osa = wire.osa();
        osa.connect().unwrap();
        assert!(osa.acquire_trace(trace, deadline()).is_err());
        assert!(!wire
            .data
            .lock()
            .unwrap()
            .commands
            .iter()
            .any(|c| c.as_str() == ":INITiate:IMMediate"));
        assert!(osa.close().unwrap().resources_released());
    }
    for mode in ["1", "3"] {
        let wire = Wire::new(2, TransferFormat::Real64);
        wire.data
            .lock()
            .unwrap()
            .metadata
            .insert(":INITiate:SMODe?".into(), mode.into());
        let mut osa = wire.osa();
        osa.connect().unwrap();
        let spectrum = osa.acquire(TraceId::A, deadline()).unwrap();
        assert_eq!(spectrum.power_dbm(), &[-40., -39.]);
        assert!(osa.close().unwrap().resources_released());
        assert!(!wire
            .data
            .lock()
            .unwrap()
            .commands
            .contains(&":ABORt".into()));
    }
}
#[test]
fn native_close_deadline_retains_one_helper_until_explicit_reap() {
    let wire = Wire::new(2, TransferFormat::Ascii);
    let mut osa = Osa::with_options(
        wire.manager(),
        "GPIB0::4::INSTR".into(),
        std::sync::Arc::new(yang_drivers::clock::ManualClock::default()),
        quick_options(),
    )
    .unwrap();
    osa.connect().unwrap();
    wire.data.lock().unwrap().block_close = true;
    let first = osa.close().unwrap();
    wire.entered();
    assert!(!first.resources_released());
    assert!(osa.has_resource_responsibility());
    let second = osa.close().unwrap();
    assert!(!second.resources_released());
    wire.release();
    assert!(osa.close().unwrap().resources_released());
    assert_eq!(
        wire.data
            .lock()
            .unwrap()
            .closed
            .iter()
            .filter(|h| **h == 11)
            .count(),
        1
    );
    assert!(!first.resources_released());
}
#[test]
fn failed_open_with_live_handle_can_be_closed_without_reopening() {
    let wire = Wire::new(2, TransferFormat::Ascii);
    wire.data.lock().unwrap().open_error_with_handle = true;
    let mut osa = wire.osa();
    assert!(osa.connect().is_err());
    assert!(osa.has_resource_responsibility());
    assert!(osa.close().unwrap().resources_released());
    assert!(wire.data.lock().unwrap().commands.is_empty());
}
