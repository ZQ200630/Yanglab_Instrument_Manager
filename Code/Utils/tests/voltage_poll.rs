#[path = "support/voltage_wire.rs"]
mod wire;

use std::time::Duration;
use yang_drivers::{clock::Clock, lifecycle::DriverState, DriverError};

#[test]
fn quiet_buffer_is_not_read_or_fenced_and_a_later_healthy_stream_connects() {
    let peer = wire::Peer::new();
    peer.clock.wait(Duration::from_secs(60));
    { let mut data = peer.data.lock().unwrap(); data.auto = false; data.empty_read_retains_pending = true; }
    let waiting_peer = peer.clone();
    let pending = std::thread::spawn(move || {
        let mut source = wire::source(&waiting_peer);
        let result = source.connect();
        (source, result)
    });
    wire::until(|| {
        let data = peer.data.lock().unwrap();
        data.availability_checks > 0 || data.reads > 0
    });
    let quiet_reads = { let mut data = peer.data.lock().unwrap(); let reads = data.reads; data.auto = true; reads };
    let (mut source, result) = pending.join().unwrap();
    let state = source.state();
    let receipt = source.close().unwrap();
    assert_eq!(quiet_reads, 0, "reading an empty buffer would retain canceled native I/O and fence the session");
    assert!(result.is_ok(), "a later finite valid stream must remain usable: {result:?}");
    assert_eq!(state, DriverState::Ready);
    assert!(receipt.resources_released());
}

#[test]
fn telemetry_reads_only_the_available_bytes_up_to_the_existing_sixty_eight_byte_bound() {
    let peer = wire::Peer::new();
    peer.data.lock().unwrap().chunks.push_back(wire::telemetry([2.; 8]).repeat(3));
    let mut source = wire::source(&peer);
    source.connect().unwrap();
    source.close().unwrap();
    let data = peer.data.lock().unwrap();
    assert_eq!(&data.read_limits[..2], &[68, 34]);
    assert!(data.read_limits.iter().all(|count| *count <= 68));
}

#[test]
fn unavailable_or_failed_receive_count_faults_without_falling_back_to_a_serial_read() {
    for error in [
        DriverError::DependencyUnavailable("injected receive-count inspection unsupported".into()),
        DriverError::Native { operation: "injected ClearCommError".into(), status: 5, transferred: 0 },
    ] {
        let peer = wire::Peer::new();
        peer.data.lock().unwrap().available_error = Some(error.clone());
        let mut source = wire::source(&peer);
        let result = source.connect();
        let receipt = source.close().unwrap();
        assert_eq!(result.as_ref().err(), Some(&error));
        assert_eq!(peer.data.lock().unwrap().reads, 0, "failed capability inspection must never issue a fallback read");
        assert!(receipt.resources_released());
    }
}
