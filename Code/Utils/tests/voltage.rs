#[path = "support/voltage_wire.rs"]
mod wire;
use std::time::Duration;
use yang_drivers::{
    clock::Clock,
    lifecycle::DriverState,
    transport::Deadline,
    voltage::{decode_telemetry, encode_voltages, TelemetryDecoder, ZeroState, BOARD_SCALE_V},
};
#[test]
fn encode_eight_channels_matches_wire() {
    assert_eq!(BOARD_SCALE_V, 28.);
    assert_eq!(
        encode_voltages([14.; 8], 14.).unwrap(),
        [vec![0x7f, 0xff].repeat(8), b"\r\n".to_vec()].concat()
    );
    assert!(encode_voltages([14.0001; 8], 14.).is_err());
    assert!(encode_voltages([f64::NAN; 8], 14.).is_err());
    assert!(encode_voltages([0.; 8], 14.0001).is_err());
    let status = decode_telemetry(&wire::telemetry([1.6; 8]), Duration::from_secs(1)).unwrap();
    assert_eq!(status.voltage_v, [1.6; 8]);
    assert_eq!(status.current_ma[7], -1.75);
}
#[test]
fn telemetry_resynchronizes_without_false_success() {
    let frame = wire::telemetry([3.2; 8]);
    let mut d = TelemetryDecoder::default();
    assert!(d.feed(&[99, 88, 77], Duration::ZERO).is_empty());
    assert!(d.feed(&frame[..20], Duration::ZERO).is_empty());
    assert!(d.feed(&frame[20..], Duration::ZERO).is_empty());
    assert!(
        d.feed(&frame, Duration::ZERO).is_empty(),
        "inspect all possible offsets before selecting a frame boundary"
    );
    let s = d.feed(&frame, Duration::from_secs(1));
    assert_eq!(s.len(), 3);
    assert!(s.iter().all(|v| v.voltage_v == [3.2; 8]));
    assert!(d.feed(b"junk\r\n", Duration::ZERO).is_empty());
}
#[test]
fn ambiguous_payload_delimiters_remain_unknown_instead_of_fabricating_zero() {
    let mut frame = wire::telemetry([3.2; 8]);
    for i in 0..8 {
        frame[i * 4 + 2] = 0;
        frame[i * 4 + 3] = 0;
    }
    frame[30] = 13;
    frame[31] = 10;
    let mut d = TelemetryDecoder::default();
    assert!(d
        .feed(
            &[vec![55, 66, 77], frame.repeat(4)].concat(),
            Duration::ZERO
        )
        .is_empty());
}
#[test]
fn ramp_is_at_most_point_one_every_fifty_ms() {
    let peer = wire::Peer::new();
    let mut driver = wire::source(&peer);
    driver.connect().unwrap();
    assert_eq!(peer.data.lock().unwrap().configurations, [115200]);
    driver.set_channel(1, 0.3).unwrap();
    driver.set_channel(2, 0.2).unwrap();
    driver.set_all([14.; 8]).unwrap();
    let writes = peer.data.lock().unwrap().writes.clone();
    let limit = (0.1 * 65535. / 28.) as i32;
    for pair in writes.windows(2) {
        assert!(pair[1].0 - pair[0].0 >= Duration::from_millis(50));
        for ch in 0..8 {
            let n = |frame: &Vec<u8>| u16::from_be_bytes([frame[ch * 2], frame[ch * 2 + 1]]) as i32;
            assert!((n(&pair[1].1) - n(&pair[0].1)).abs() <= limit);
        }
    }
    assert_eq!(
        writes.last().unwrap().1,
        encode_voltages([14.; 8], 14.).unwrap()
    );
    driver.close().unwrap();
}
#[test]
fn invalid_channel_or_voltage_writes_nothing() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    let before = p.data.lock().unwrap().writes.len();
    for ch in [0, 9, 255] {
        assert!(d.set_channel(ch, 1.).is_err());
    }
    for value in [-0.001, 14.0001, f64::NAN, f64::INFINITY] {
        assert!(d.set_channel(1, value).is_err());
    }
    assert_eq!(p.data.lock().unwrap().writes.len(), before);
    d.close().unwrap();
}
#[test]
fn read_only_probe_never_zeroes_or_sets_ready() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    let report = d.probe_identity().unwrap();
    assert!(report.release_confirmed());
    assert_eq!(d.state(), DriverState::Disconnected);
    assert!(p.data.lock().unwrap().writes.is_empty());
    assert!(d.resources_released());
}
#[test]
fn fresh_wait_and_channel_confirmations_are_post_command() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    let first = d
        .wait_for_status(Deadline::after(Duration::from_secs(1)))
        .unwrap();
    d.set_channel(1, 1.).unwrap();
    let s = d
        .wait_for_channel(1, 1., 0.01, Deadline::after(Duration::from_secs(1)))
        .unwrap();
    assert!((s.voltage_v[0] - 1.).abs() < 0.01);
    assert!(s.received_at >= first.received_at);
    assert!(d
        .wait_for_channel(0, 0., 0.1, Deadline::after(Duration::from_secs(1)))
        .is_err());
    assert!(d
        .wait_for_channel(1, 0., 0., Deadline::after(Duration::from_secs(1)))
        .is_err());
    d.close().unwrap();
}
#[test]
fn stale_telemetry_and_zero_evidence_are_unusable() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    assert_eq!(d.zero_evidence().state(), ZeroState::MeasuredZero);
    p.data.lock().unwrap().auto = false;
    p.clock.wait(Duration::from_secs(2));
    assert!(d.read_status(Duration::from_millis(1)).is_err());
    assert_eq!(d.zero_evidence().state(), ZeroState::Unknown);
    d.close().unwrap();
}
#[test]
fn changing_one_channel_preserves_every_other_dac_code() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    for code in 1..100 {
        d.set_channel(1, (code as f64 + 0.25) * 28. / 65535.)
            .unwrap();
        let before = p.data.lock().unwrap().writes.last().unwrap().1[..2].to_vec();
        d.set_channel(2, 0.1).unwrap();
        assert_eq!(
            p.data.lock().unwrap().writes.last().unwrap().1[..2],
            before,
            "changing CH2 must not requantize CH1"
        );
    }
    d.close().unwrap();
}
#[test]
fn stale_or_recovering_telemetry_blocks_nonzero_commands() {
    let p = wire::Peer::new();
    let mut d = wire::source(&p);
    d.connect().unwrap();
    let reads = {
        let mut s = p.data.lock().unwrap();
        s.auto = false;
        s.reads
    };
    p.clock.wait(Duration::from_secs(2));
    wire::until(|| p.data.lock().unwrap().reads > reads + 2);
    let before = p.data.lock().unwrap().writes.len();
    assert!(d.set_channel(1, 0.1).is_err());
    assert_eq!(p.data.lock().unwrap().writes.len(), before);
    d.close().unwrap();
}
#[test]
fn embedded_crlf_with_an_unaligned_prefix_is_not_a_voltage_frame() {
    let mut frame = wire::telemetry([3.2; 8]);
    frame[30] = 13;
    frame[31] = 10;
    let mut decoder = TelemetryDecoder::default();
    let stream = [
        vec![55, 66, 77],
        frame.clone(),
        frame.clone(),
        frame.clone(),
    ]
    .concat();
    let statuses = decoder.feed(&stream, Duration::ZERO);
    assert!(!statuses.is_empty());
    for status in statuses {
        assert_eq!(
            status.voltage_v, [3.2; 8],
            "a payload CRLF must not fabricate shifted ADC channels"
        );
    }
}
#[test]
fn recovery_backoff_does_not_spin_and_close_preempts_it() {
    let p = wire::Peer::new();
    let mut d = wire::source_with(
        &p,
        yang_drivers::voltage::VoltageConfig {
            port: "COM12".into(),
            io_timeout: Duration::from_millis(50),
            communication_retry_interval: Duration::from_secs(1),
            close_timeout: Duration::from_millis(100),
            ..Default::default()
        },
    );
    d.connect().unwrap();
    p.data.lock().unwrap().auto = false;
    p.clock.wait(Duration::from_millis(100));
    wire::until(|| d.zero_evidence().state() == ZeroState::Unknown);
    std::thread::sleep(Duration::from_millis(20));
    let before = p.data.lock().unwrap().reads;
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(
        p.data.lock().unwrap().reads,
        before,
        "honor configured recovery backoff instead of spinning"
    );
    let began = std::time::Instant::now();
    assert!(d.close().unwrap().resources_released());
    assert!(
        began.elapsed() < Duration::from_millis(400),
        "cleanup metadata must preempt the one-second recovery backoff"
    );
}
