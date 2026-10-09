#[path = "support/voltage_wire.rs"]
mod wire;

use std::time::{Duration, Instant};
use yang_drivers::{
    clock::Clock,
    lifecycle::DriverState,
    voltage::{encode_voltages, VoltageConfig, ZeroState},
    DriverError,
};

#[test]
fn startup_after_worker_has_run_for_a_minute_accepts_a_healthy_stream() {
    let peer = wire::Peer::new();
    peer.clock.wait(Duration::from_secs(60));
    let mut source = wire::source(&peer);
    let result = source.connect();
    let state = source.state();
    let zero = source.zero_evidence();
    source.close().unwrap();
    assert!(result.is_ok(), "worker uptime must not exhaust a new connection: {result:?}");
    assert_eq!(state, DriverState::Ready);
    assert_eq!(zero.state(), ZeroState::MeasuredZero);
    assert!(peer.data.lock().unwrap().write_read_counts[0] >= 3,
        "initial streaming alignment needs three complete frames before startup zero");
}

#[test]
fn startup_at_nonzero_clock_accepts_initial_empty_and_fragmented_reads_before_zero() {
    let peer = wire::Peer::new();
    peer.clock.wait(Duration::from_secs(60));
    let frame = wire::telemetry([2.; 8]);
    peer.data.lock().unwrap().chunks.extend([
        vec![], frame[..17].to_vec(), frame[17..].to_vec(), frame.clone(), frame,
    ]);
    let mut source = wire::source(&peer);
    let result = source.connect();
    let state = source.state();
    let receipt = source.close().unwrap();
    assert!(result.is_ok(), "finite initial empty/partial reads are not an expired lifecycle: {result:?}");
    assert_eq!(state, DriverState::Ready);
    assert!(receipt.resources_released());
    let data = peer.data.lock().unwrap();
    assert!(data.write_read_counts[0] >= 4, "no startup/fault zero before complete protocol observations");
    assert!(data.writes.iter().all(|(_, frame)| frame == &encode_voltages([0.; 8], 14.).unwrap()));
}

#[test]
fn aged_clock_read_only_probe_observes_protocol_without_any_output_writes() {
    let peer = wire::Peer::new();
    peer.clock.wait(Duration::from_secs(60));
    let mut source = wire::source(&peer);
    let report = source.probe_identity().unwrap();
    assert!(report.release_confirmed());
    assert!(source.resources_released());
    assert_eq!(source.state(), DriverState::Disconnected);
    assert!(peer.data.lock().unwrap().writes.is_empty());
}

#[test]
fn aged_clock_reconnect_starts_a_new_initial_telemetry_window() {
    let peer = wire::Peer::new();
    let mut source = wire::source(&peer);
    source.connect().unwrap();
    assert!(source.close().unwrap().resources_released());
    peer.clock.wait(Duration::from_secs(60));
    source.connect().unwrap();
    assert_eq!(source.state(), DriverState::Ready);
    assert_eq!(source.zero_evidence().state(), ZeroState::MeasuredZero);
    assert!(source.close().unwrap().resources_released());
}

#[test]
fn startup_deadline_stays_bounded_without_valid_telemetry_even_if_partial_bytes_keep_arriving() {
    for partial_traffic in [false, true] {
        let peer = wire::Peer::new();
        peer.clock.wait(Duration::from_secs(60));
        {
            let mut data = peer.data.lock().unwrap();
            data.auto = false;
            if partial_traffic { data.chunks.extend((0..1000).map(|_| vec![1; 17])); }
        }
        let mut source = wire::source_with(&peer, VoltageConfig {
            port: "COM12".into(), startup_timeout: Duration::from_millis(60),
            close_timeout: Duration::from_millis(40), ..Default::default()
        });
        let started = Instant::now();
        let result = source.connect();
        let elapsed = started.elapsed();
        let receipt = source.close().unwrap();
        assert!(matches!(result, Err(DriverError::Timeout { ref operation, .. }) if operation == "deadline"),
            "startup should expire its own fixed deadline, not worker uptime: {result:?}");
        assert!(elapsed >= Duration::from_millis(60));
        assert!(elapsed < Duration::from_millis(500), "partial traffic must not extend startup indefinitely");
        assert!(receipt.resources_released());
        assert_ne!(receipt.voltage_zero().unwrap()["state"], "measured_zero");
        assert!(peer.data.lock().unwrap().writes.iter().all(|(_, frame)| frame[..16] == [0; 16]));
    }
}

#[test]
fn established_stream_silence_keeps_the_existing_recovery_boundary_at_nonzero_clock() {
    let peer = wire::Peer::new();
    peer.clock.wait(Duration::from_secs(60));
    let mut source = wire::source_with(&peer, VoltageConfig {
        port: "COM12".into(), io_timeout: Duration::from_millis(50),
        communication_recovery_timeout: Duration::from_millis(100),
        communication_retry_interval: Duration::from_millis(5),
        close_timeout: Duration::from_millis(100), ..Default::default()
    });
    source.connect().unwrap();
    peer.data.lock().unwrap().hold = true;
    peer.held();
    let checks = { let mut data = peer.data.lock().unwrap(); data.auto = false; data.availability_checks };
    peer.clock.wait(Duration::from_millis(149));
    peer.release();
    wire::until(|| peer.data.lock().unwrap().availability_checks > checks);
    wire::until(|| source.zero_evidence().state() == ZeroState::Unknown);
    assert_eq!(source.state(), DriverState::Ready);
    assert_eq!(source.zero_evidence().state(), ZeroState::Unknown);
    peer.clock.wait(Duration::from_millis(1));
    wire::until(|| source.state() == DriverState::Fault);
    wire::until(|| peer.data.lock().unwrap().writes.last().unwrap().1[..16] == [0; 16]);
    assert!(source.has_resource_responsibility());
    assert!(source.close().unwrap().resources_released());
}

#[test]
fn connect_returns_the_original_read_fault_when_fault_zero_and_cleanup_also_fail() {
    let peer = wire::Peer::new();
    let original = DriverError::Protocol("injected voltage stream fault".into());
    peer.data.lock().unwrap().read_errors.push_back(original.clone());
    let mut source = wire::source(&peer);
    let result = source.connect();
    let receipt = source.close().unwrap();
    assert_eq!(result.as_ref().err(), Some(&original), "fault zero and cleanup must not replace the immutable connect failure");
    assert!(receipt.resources_released());
    assert!(receipt.steps().iter().any(|step| step.action == "zero_command" && step.error.is_some()));
    assert!(source.cleanup_error().is_some());
}

#[test]
fn connect_keeps_the_original_recovery_timeout_after_cleanup_succeeds() {
    let peer = wire::Peer::new();
    { let mut data = peer.data.lock().unwrap(); data.hold = true; data.chunks.push_back(vec![]); }
    let held_peer = peer.clone();
    let pending = std::thread::spawn(move || {
        let mut source = wire::source(&held_peer);
        let result = source.connect();
        (source, result)
    });
    peer.held();
    peer.clock.wait(Duration::from_secs(4));
    peer.release();
    let (mut source, result) = pending.join().unwrap();
    let receipt = source.close().unwrap();
    assert_eq!(result.as_ref().err(), Some(&DriverError::Timeout {
        operation: "voltage telemetry recovery exhausted".into(), transferred: 0,
    }));
    assert!(receipt.resources_released());
    assert_eq!(receipt.voltage_zero().unwrap()["state"], "measured_zero");
    assert!(source.cleanup_error().is_none());
}
