#[path = "support/voltage_wire.rs"]
mod wire;

use std::time::Duration;
use yang_drivers::{clock::Clock, lifecycle::DriverState};

#[test]
fn cache_snapshot_is_connection_bound_and_never_uses_serial_io() {
    let peer=wire::Peer::new();let mut source=wire::source(&peer);source.connect().unwrap();
    let cache=source.cache_handle();
    peer.data.lock().unwrap().hold=true;peer.held();
    let before={let data=peer.data.lock().unwrap();(data.reads,data.writes.len(),data.availability_checks)};
    for _ in 0..20 {
        let snapshot=cache.snapshot().unwrap();assert_eq!(snapshot.state,DriverState::Ready);
        assert_eq!(snapshot.status.unwrap().received_at,peer.clock.now());
        assert_eq!(snapshot.commanded_voltage_v,[0.;8]);
    }
    let after={let data=peer.data.lock().unwrap();(data.reads,data.writes.len(),data.availability_checks)};
    peer.release();source.close().unwrap();assert!(cache.snapshot().is_none());
    // A release receipt can precede the reader thread's final return. Preserve
    // that owner guard, waiting only for its exact finite termination boundary.
    let reopen_deadline=std::time::Instant::now()+Duration::from_secs(2);
    loop {
        match source.connect() {
            Ok(_) => break,
            Err(yang_drivers::DriverError::Responsibility(message)) if message=="prior reader has not exited" => {
                assert!(std::time::Instant::now()<reopen_deadline,"prior finite reader did not exit");
                std::thread::yield_now();
            }
            Err(error) => panic!("unexpected finite reconnect error: {error:?}"),
        }
    }
    assert!(cache.snapshot().is_none(),"old cache cannot observe a reopened connection");
    assert!(source.cache_handle().snapshot().is_some());source.close().unwrap();
    assert_eq!(before,after,"cache snapshots must not call poll/read/write/available");
}

#[test]
fn cache_retains_real_sample_time_and_expires_only_its_cloned_zero_evidence() {
    let peer=wire::Peer::new();let mut source=wire::source(&peer);source.connect().unwrap();
    let cache=source.cache_handle();peer.data.lock().unwrap().hold=true;peer.held();
    let initial=cache.snapshot().unwrap();peer.data.lock().unwrap().auto=false;
    peer.clock.wait(Duration::from_millis(1100));
    let stale=cache.snapshot().unwrap();
    assert_eq!(stale.status.as_ref().unwrap().received_at,initial.status.unwrap().received_at);
    assert_eq!(stale.telemetry_timeout,Duration::from_secs(1));
    assert_eq!(stale.zero_evidence.state(),yang_drivers::voltage::ZeroState::Unknown);
    peer.release();source.close().unwrap();
}
