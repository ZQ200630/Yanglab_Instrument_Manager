#[path = "support/gain_wire.rs"]
mod wire;
use std::time::Duration;
use yang_drivers::{
    clock::Clock, gain::interlock::ThermalInterlock, lifecycle::DriverState, transport::Deadline,
};
#[test]
fn five_seconds_requires_fresh_continuous_samples() {
    let mut s = ThermalInterlock::default();
    for i in 0..5 {
        s.observe(22., 22., true, Duration::from_secs(i));
    }
    assert!(!s.ready(Duration::from_secs(4)));
    s.observe(22., 22., true, Duration::from_secs(5));
    assert!(s.ready(Duration::from_secs(5)));
    assert!(!s.ready(Duration::from_millis(6501)));
    s.observe(22., 22., true, Duration::from_secs(7));
    assert!(!s.ready(Duration::from_secs(7)));
    for i in 8..13 {
        s.observe(22., 22., true, Duration::from_secs(i));
    }
    assert!(s.ready(Duration::from_secs(12)));
    s.observe(22.201, 22., true, Duration::from_secs(13));
    assert!(!s.ready(Duration::from_secs(13)));
    for i in 14..20 {
        s.observe(22., 22., false, Duration::from_secs(i));
    }
    assert!(!s.ready(Duration::from_secs(19)));
}
#[test]
fn rapid_reads_cannot_fabricate_stability() {
    let mut s = ThermalInterlock::default();
    for i in 0..100 {
        s.observe(22., 22., true, Duration::from_millis(i));
    }
    assert!(!s.ready(Duration::from_secs(5)));
}
#[test]
fn enable_reads_actual_reset_current_and_ramp_is_bounded() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    assert!(d.enable_current().is_err());
    d.enable_tec().unwrap();
    wire::stable(&p, &d);
    assert!(d.enable_current().unwrap());
    assert_eq!(d.status().unwrap().current_ma, 3.);
    assert_eq!(d.state(), DriverState::Active);
    let before = p.data.lock().unwrap().writes.len();
    assert_eq!(
        d.ramp_current(6., 1., Duration::from_millis(50)).unwrap(),
        6.
    );
    let writes = p.data.lock().unwrap().writes.clone();
    let steps = writes[before..]
        .iter()
        .filter(|(_, b)| b.starts_with(b"STCA"))
        .collect::<Vec<_>>();
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0].1, b"STCA004000\r\n");
    assert_eq!(steps[1].1, b"STCA005000\r\n");
    assert_eq!(steps[2].1, b"STCA006000\r\n");
    assert!(steps
        .windows(2)
        .all(|v| v[1].0 - v[0].0 >= Duration::from_millis(50)));
    d.close().unwrap();
}
#[test]
fn shutdown_orders_current_before_tec() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    let receipt = d.close().unwrap();
    assert!(receipt.resources_released());
    let s = p.data.lock().unwrap();
    assert_eq!(s.writes[s.writes.len() - 2].1, b"STQA000000\r\n");
    assert_eq!(s.writes.last().unwrap().1, b"STRA000000\r\n");
    assert!(!s.enabled && !s.tec);
}
#[test]
fn tec_off_or_monitor_failure_disables_current() {
    for fail in [false, true] {
        let p = wire::Peer::new();
        let mut d = wire::driver(&p);
        d.connect().unwrap();
        d.enable_tec().unwrap();
        wire::stable(&p, &d);
        d.enable_current().unwrap();
        {
            let mut s = p.data.lock().unwrap();
            if fail {
                s.faults.push_back(("RDTA".into(), b"ERROR\r\n".to_vec()));
            } else {
                s.tec = false;
            }
        }
        p.clock.wait(Duration::from_secs(1));
        wire::until(|| d.state() == DriverState::Fault && !p.data.lock().unwrap().enabled);
        assert!(d.fault_error().is_some());
        assert!(!p.data.lock().unwrap().enabled);
        d.close().unwrap();
    }
}
#[test]
fn target_change_invalidates_the_entire_sequence() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    d.enable_tec().unwrap();
    wire::stable(&p, &d);
    d.set_temperature(22.).unwrap();
    assert!(d
        .wait_stable(Deadline::after(Duration::from_millis(1)))
        .is_err());
    assert!(d.enable_current().is_err());
    d.close().unwrap();
}
#[test]
fn pending_poll_close_preserves_immutable_receipt_and_resource() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    p.data.lock().unwrap().hold = true;
    p.clock.wait(Duration::from_secs(1));
    p.held();
    let pending = d.close().unwrap();
    assert!(!pending.resources_released());
    assert_eq!(p.data.lock().unwrap().closes, 0);
    p.release();
    wire::until(|| d.resources_released());
    let complete = d.close().unwrap();
    assert!(complete.resources_released());
    assert!(!pending.resources_released());
}
#[test]
fn moderate_and_severe_thresholds_preserve_shutdown_order() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    d.enable_tec().unwrap();
    wire::stable(&p, &d);
    d.enable_current().unwrap();
    p.data.lock().unwrap().temperature = 23.001;
    for _ in 0..3 {
        p.sample(&d);
    }
    assert!(!d.status().unwrap().current_enabled);
    assert!(d.status().unwrap().tec_enabled);
    p.data.lock().unwrap().temperature = 25.001;
    p.clock.wait(Duration::from_secs(1));
    wire::until(|| d.state() == DriverState::Fault && !p.data.lock().unwrap().tec);
    assert!(!p.data.lock().unwrap().tec);
    d.close().unwrap();
}
#[test]
fn repeated_enable_never_reissues_q1_or_resets_running_current() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    d.enable_tec().unwrap();
    wire::stable(&p, &d);
    d.enable_current().unwrap();
    d.set_current(20.).unwrap();
    let before = p.data.lock().unwrap().writes.len();
    assert!(d.enable_current().unwrap());
    assert_eq!(d.status().unwrap().current_ma, 20.);
    assert!(p.data.lock().unwrap().writes[before..]
        .iter()
        .all(|(_, b)| b != b"STQA000001\r\n"));
    d.close().unwrap();
}
#[test]
fn stop_during_enable_read_cannot_send_q1() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    d.enable_tec().unwrap();
    wire::stable(&p, &d);
    let stop = d.stop_handle();
    p.data.lock().unwrap().hold = true;
    let pending = std::thread::spawn(move || {
        let result = d.enable_current();
        (d, result)
    });
    p.held();
    stop.request_stop();
    p.release();
    let (mut d, result) = pending.join().unwrap();
    assert!(result.is_err());
    assert!(p
        .data
        .lock()
        .unwrap()
        .writes
        .iter()
        .all(|(_, b)| b != b"STQA000001\r\n"));
    assert!(d.close().unwrap().resources_released());
}
#[test]
fn failed_shutdown_cannot_reuse_an_earlier_off_ack() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    assert!(!d.disable_current().unwrap());
    p.data
        .lock()
        .unwrap()
        .faults
        .push_back(("STQA000000".into(), b"READY;Q=1\r\n".to_vec()));
    let receipt = d.close().unwrap();
    assert!(receipt.resources_released());
    assert!(receipt
        .steps()
        .iter()
        .any(|s| s.action == "current_off_unconfirmed" && s.error.is_some()));
    assert!(d.cleanup_error().is_some());
}
#[test]
fn long_ramp_waits_are_interruptible_at_fifty_ms() {
    use std::sync::{Arc, Mutex};
    struct CancelClock {
        clock: Arc<yang_drivers::clock::ManualClock>,
        stop: Mutex<Option<yang_drivers::gain::StopHandle>>,
        waits: Mutex<Vec<Duration>>,
    }
    impl Clock for CancelClock {
        fn now(&self) -> Duration {
            self.clock.now()
        }
        fn wait(&self, d: Duration) {
            self.waits.lock().unwrap().push(d);
            self.clock.wait(d);
            if let Some(stop) = self.stop.lock().unwrap().as_ref() {
                stop.request_stop();
            }
        }
    }
    let p = wire::Peer::new();
    let clock = Arc::new(CancelClock {
        clock: p.clock.clone(),
        stop: Mutex::new(None),
        waits: Mutex::new(vec![]),
    });
    let mut d = yang_drivers::gain::GainDriver::with_backend(
        yang_drivers::gain::GainConfig {
            port: Some("COM13".into()),
            ..Default::default()
        },
        yang_drivers::transport::ResourceBook::isolated(),
        clock.clone(),
        Arc::new(wire::Backend(p.clone())),
    )
    .unwrap();
    d.connect().unwrap();
    d.enable_tec().unwrap();
    wire::stable(&p, &d);
    d.enable_current().unwrap();
    *clock.stop.lock().unwrap() = Some(d.stop_handle());
    assert!(d.ramp_current(4., 1., Duration::from_secs(10)).is_err());
    assert!(
        clock
            .waits
            .lock()
            .unwrap()
            .iter()
            .all(|d| *d <= Duration::from_millis(50)),
        "a ten-second interval must not hide stop for ten seconds"
    );
    d.close().unwrap();
}
