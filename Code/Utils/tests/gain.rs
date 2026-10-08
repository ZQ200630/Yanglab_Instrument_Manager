#[path = "support/gain_wire.rs"]
mod wire;
use std::time::Duration;
use yang_drivers::{
    clock::ManualClock,
    gain::{
        codec::{build_bool, build_command, parse_ack, parse_reply},
        GainConfig, GainDriver,
    },
    lifecycle::DriverState,
    transport::ResourceBook,
};
#[test]
fn gain_fixed_commands_and_ready_fields_match() {
    assert_eq!(build_command("RDTA", None, 0., 0.).unwrap(), b"RDTA\r\n");
    assert_eq!(
        build_command("STEA", Some(22.), 15., 40.).unwrap(),
        b"STEA022000\r\n"
    );
    assert_eq!(
        build_command("STCA", Some(150.), 0., 200.).unwrap(),
        b"STCA150000\r\n"
    );
    assert_eq!(build_bool("STQA", true).unwrap(), b"STQA000001\r\n");
    assert_eq!(parse_reply(b"READY;T=21.456\r\n", b'T').unwrap(), "21.456");
    assert_eq!(parse_ack(b"READY\r\n").unwrap(), "READY");
    assert_eq!(parse_ack(b"READY;P=0.350\r\n").unwrap(), "READY;P=0.350");
    for bad in [
        b"READY;E=22.000\r\n".as_slice(),
        b"READY;T=22",
        b"ERROR;T=22\r\n",
        b"READY;T=22;Q=1\r\n",
        b"READY;T=\xff\r\n",
    ] {
        assert!(parse_reply(bad, b'T').is_err());
    }
    assert!(build_command("RDTA\r\nSTQA", None, 0., 0.).is_err());
}
#[test]
fn normal_connect_initializes_current_before_tec_and_observes_off() {
    let p = wire::Peer::new();
    p.data.lock().unwrap().enabled = true;
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    let status = d.read_status().unwrap();
    assert_eq!(d.state(), DriverState::Ready);
    assert!(!status.current_enabled && !status.tec_enabled);
    assert_eq!(status.target_c, 22.);
    assert_eq!(status.current_ma, 150.);
    let state = p.data.lock().unwrap();
    assert_eq!(state.pid, [0.35, 0.1, 0.]);
    let commands: Vec<_> = state
        .writes
        .iter()
        .map(|(_, bytes)| bytes.as_slice())
        .collect();
    assert_eq!(
        commands,
        [
            b"STQA000000\r\n".as_slice(),
            b"STRA000000\r\n",
            b"RDTA\r\n",
            b"RDEA\r\n",
            b"RDRA\r\n",
            b"RDCA\r\n",
            b"RDQA\r\n",
        ]
    );
    drop(state);
    assert!(d.close().unwrap().resources_released());
}
#[test]
fn normal_connect_rejects_unconfirmed_startup_outputs() {
    for (command, reply) in [
        ("STQA000000", b"READY;Q=1\r\n".as_slice()),
        ("STRA000000", b"READY;D=1\r\n".as_slice()),
        ("RDRA", b"READY;R=1\r\n".as_slice()),
        ("RDQA", b"READY;Q=1\r\n".as_slice()),
    ] {
        let p = wire::Peer::new();
        p.data
            .lock()
            .unwrap()
            .faults
            .push_back((command.into(), reply.to_vec()));
        let mut d = wire::driver(&p);
        assert!(
            d.connect().is_err(),
            "startup accepted unconfirmed {command}"
        );
        assert!(!matches!(d.state(), DriverState::Ready | DriverState::Active));
        assert!(d.fault_error().is_some());
        assert!(d.close().unwrap().resources_released());
        assert!(p
            .data
            .lock()
            .unwrap()
            .writes
            .iter()
            .all(|(_, bytes)| bytes != b"STQA000001\r\n" && bytes != b"STRA000001\r\n"));
    }
}
#[test]
fn normal_connect_pending_initializer_retains_native_owner() {
    let p = wire::Peer::new();
    p.data.lock().unwrap().hold = true;
    let peer = p.clone();
    let pending = std::thread::spawn(move || {
        let mut d = wire::driver(&peer);
        let result = d.connect();
        (d, result)
    });
    p.held();
    let (mut d, result) = pending.join().unwrap();
    assert!(result.is_err());
    assert!(d.has_resource_responsibility());
    assert!(!matches!(d.state(), DriverState::Ready | DriverState::Active));
    let receipt = d.close().unwrap();
    assert!(!receipt.resources_released());
    p.release();
    wire::until(|| d.resources_released());
    assert!(d.close().unwrap().resources_released());
    assert!(!receipt.resources_released());
}
#[test]
fn limits_reject_before_write() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    let before = p.data.lock().unwrap().writes.len();
    for v in [-0.001, 200.0001, f64::NAN, f64::INFINITY] {
        assert!(d.set_current(v).is_err());
    }
    for v in [14.999, 40.001, f64::NAN] {
        assert!(d.set_temperature(v).is_err());
    }
    assert!(d.set_pid(0.1, 1000., 0.).is_err());
    assert!(d
        .ramp_current(10., 1.001, Duration::from_millis(50))
        .is_err());
    assert!(d.ramp_current(10., 1., Duration::from_millis(49)).is_err());
    assert_eq!(p.data.lock().unwrap().writes.len(), before);
    assert_eq!(d.set_current(200.).unwrap(), 200.);
    assert_eq!(d.set_temperature(15.).unwrap(), 15.);
    assert_eq!(d.set_temperature(40.).unwrap(), 40.);
    d.close().unwrap();
}
#[test]
fn pid_functions_retain_reviewed_protocol() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    let start = p.data.lock().unwrap().writes.len();
    assert_eq!(d.set_pid(0.35, 0.1, 0.).unwrap(), [0.35, 0.1, 0.]);
    assert_eq!(d.read_pid().unwrap(), [0.35, 0.1, 0.]);
    assert_eq!(d.reset_pid().unwrap(), [0.35, 0.1, 0.]);
    d.clear_integral().unwrap();
    let bytes = p.data.lock().unwrap().writes[start..]
        .iter()
        .map(|(_, b)| b.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        bytes,
        [
            b"STPA000350\r\n".to_vec(),
            b"STIA000100\r\n".to_vec(),
            b"STDA000000\r\n".to_vec(),
            b"RDPA\r\n".to_vec(),
            b"RDIA\r\n".to_vec(),
            b"RDDA\r\n".to_vec(),
            b"RST\r\n".to_vec(),
            b"RDPA\r\n".to_vec(),
            b"RDIA\r\n".to_vec(),
            b"RDDA\r\n".to_vec(),
            b"CLR\r\n".to_vec()
        ]
    );
    d.close().unwrap();
}
#[test]
fn read_only_probe_never_runs_gain_shutdown() {
    let p = wire::Peer::new();
    p.data.lock().unwrap().enabled = true;
    let mut d = wire::driver(&p);
    let result = d.probe_identity().unwrap();
    assert!(result.release_confirmed());
    assert_eq!(d.state(), DriverState::Disconnected);
    let state = p.data.lock().unwrap();
    assert!(state.enabled && state.tec);
    drop(state);
    assert_eq!(result.observations()["status"]["current_enabled"], true);
    assert!(p
        .data
        .lock()
        .unwrap()
        .writes
        .iter()
        .all(|(_, b)| b.starts_with(b"RD")));
}
#[test]
fn all_typed_reads_use_confirmed_values() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    assert_eq!(d.read_temperature().unwrap(), 22.);
    assert_eq!(d.read_target().unwrap(), 22.);
    assert!(!d.read_tec_enabled().unwrap());
    assert_eq!(d.read_current().unwrap(), 150.);
    assert!(!d.read_current_enabled().unwrap());
    d.close().unwrap();
}
#[test]
fn invalid_config_opens_nothing() {
    for mut cfg in [
        GainConfig {
            io_timeout: Duration::ZERO,
            ..Default::default()
        },
        GainConfig {
            poll_interval: Duration::from_millis(999),
            ..Default::default()
        },
    ] {
        cfg.port = Some("COM13".into());
        assert!(GainDriver::new(
            cfg,
            ResourceBook::isolated(),
            std::sync::Arc::new(ManualClock::default())
        )
        .is_err());
    }
}
#[test]
fn each_packet_respects_io_timeout_not_the_whole_compound_budget() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    d.connect().unwrap();
    d.read_pid().unwrap();
    d.close().unwrap();
    assert!(
        p.data
            .lock()
            .unwrap()
            .packet_budgets
            .iter()
            .all(|v| *v <= 51),
        "compound operations must not multiply a native packet's timeout"
    );
}
#[test]
fn failed_readonly_probe_and_wrong_enable_ack_never_claim_success() {
    let p = wire::Peer::new();
    let mut d = wire::driver(&p);
    p.data
        .lock()
        .unwrap()
        .faults
        .push_back(("RDTA".into(), b"ERROR\r\n".to_vec()));
    assert!(d.probe_identity().is_err());
    assert!(d.resources_released());
    assert!(p
        .data
        .lock()
        .unwrap()
        .writes
        .iter()
        .all(|(_, b)| b.starts_with(b"RD")));
    d.connect().unwrap();
    d.enable_tec().unwrap();
    wire::stable(&p, &d);
    p.data
        .lock()
        .unwrap()
        .faults
        .push_back(("STQA000001".into(), b"READY;Q=0\r\n".to_vec()));
    assert!(d.enable_current().is_err());
    assert_eq!(d.state(), DriverState::Fault);
    assert!(d.fault_error().is_some());
    assert!(!p.data.lock().unwrap().tec);
    d.close().unwrap();
}
#[test]
fn later_status_queries_cannot_refresh_an_older_temperature() {
    let p = wire::Peer::new();
    p.data
        .lock()
        .unwrap()
        .reply_clock_jumps
        .push_back(("RDEA".into(), Duration::from_secs(3)));
    let mut d = GainDriver::with_backend(
        GainConfig {
            port: Some("COM13".into()),
            start_watchdog: false,
            ..Default::default()
        },
        ResourceBook::isolated(),
        p.clock.clone(),
        std::sync::Arc::new(wire::Backend(p.clone())),
    )
    .unwrap();
    d.connect().unwrap();
    assert_eq!(
        d.status().unwrap().received_at,
        Duration::ZERO,
        "timestamp belongs to RDTA receipt, not the end of subsequent state queries"
    );
    assert!(d.read_status().is_err());
    d.close().unwrap();
}
