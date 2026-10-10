#[path = "support/gain_wire.rs"]
mod wire;

use std::{sync::Arc, time::Duration};
use yang_drivers::{
    clock::Clock,
    gain::{GainConfig, GainDriver},
    lifecycle::DriverState,
    transport::ResourceBook,
};

const TICK: Duration = Duration::from_millis(200);

// PID reads settle behind a due monitor cycle without adding temperature samples.
fn settle(driver: &GainDriver) {
    driver.read_pid().unwrap();
}

fn background_reads(peer: &Arc<wire::Peer>, start: usize) -> Vec<(Duration, Vec<u8>)> {
    peer.data.lock().unwrap().writes[start..]
        .iter()
        .filter(|(_, command)| {
            matches!(
                std::str::from_utf8(command).unwrap(),
                "RDTA\r\n" | "RDEA\r\n" | "RDRA\r\n" | "RDCA\r\n" | "RDQA\r\n"
            )
        })
        .cloned()
        .collect()
}

#[test]
fn gain_default_background_interval_is_two_hundred_ms() {
    assert_eq!(GainConfig::default().poll_interval, TICK);
}

#[test]
fn gain_accepts_only_the_reviewed_five_hz_background_interval_before_open() {
    for interval in [Duration::ZERO, Duration::from_millis(199), Duration::from_millis(201), Duration::from_secs(1)] {
        let peer = wire::Peer::new();
        let result = GainDriver::with_backend(
            GainConfig { port: Some("COM13".into()), poll_interval: interval, ..Default::default() },
            ResourceBook::isolated(),
            peer.clock.clone(),
            Arc::new(wire::Backend(peer.clone())),
        );
        assert!(result.is_err(), "unsupported interval {interval:?} must be rejected");
        assert_eq!(peer.data.lock().unwrap().opens, 0);
    }
    let peer = wire::Peer::new();
    assert!(GainDriver::with_backend(
        GainConfig { port: Some("COM13".into()), poll_interval: TICK, ..Default::default() },
        ResourceBook::isolated(),
        peer.clock.clone(),
        Arc::new(wire::Backend(peer.clone())),
    ).is_ok());
    assert_eq!(peer.data.lock().unwrap().opens, 0);
}

#[test]
fn background_reads_start_at_two_hundred_ms_and_repeat_all_five_fields_each_tick() {
    let peer = wire::Peer::new();
    let mut driver = wire::driver(&peer);
    driver.connect().unwrap();
    let start = peer.data.lock().unwrap().writes.len();

    peer.clock.wait(Duration::from_millis(199));
    settle(&driver);
    assert!(background_reads(&peer, start).is_empty(), "no background reads before the first 200 ms deadline");
    peer.clock.wait(Duration::from_millis(1));
    for tick in 1..=10 {
        settle(&driver);
        assert_eq!(driver.status().unwrap().received_at, TICK * tick);
        assert_eq!(background_reads(&peer, start).len(), tick as usize * 5);
        if tick < 10 { peer.clock.wait(TICK); }
    }
    let reads = background_reads(&peer, start);
    driver.close().unwrap();
    for (index, cycle) in reads.chunks_exact(5).enumerate() {
        assert!(cycle.iter().all(|(at, _)| *at == TICK * (index as u32 + 1)));
        assert_eq!(cycle.iter().map(|(_, command)| command.as_slice()).collect::<Vec<_>>(),
            [b"RDTA\r\n".as_slice(), b"RDEA\r\n", b"RDRA\r\n", b"RDCA\r\n", b"RDQA\r\n"]);
    }
}

#[test]
fn delayed_background_cycle_skips_missed_ticks_without_a_catch_up_burst() {
    let peer = wire::Peer::new();
    let mut driver = wire::driver(&peer);
    driver.connect().unwrap();
    let start = peer.data.lock().unwrap().writes.len();
    peer.data.lock().unwrap().reply_clock_jumps.push_back(("RDQA".into(), Duration::from_millis(650)));
    peer.clock.wait(TICK);
    settle(&driver);
    assert_eq!(peer.clock.now(), Duration::from_millis(850));
    assert_eq!(background_reads(&peer, start).len(), 5);
    settle(&driver);
    assert_eq!(background_reads(&peer, start).len(), 5, "an overdue monitor must not replay missed ticks");
    peer.clock.wait(Duration::from_millis(199));
    settle(&driver);
    assert_eq!(background_reads(&peer, start).len(), 5);
    peer.clock.wait(Duration::from_millis(1));
    settle(&driver);
    let reads = background_reads(&peer, start);
    driver.close().unwrap();
    assert_eq!(reads.len(), 10);
    assert_eq!(reads[0].0, Duration::from_millis(200));
    assert_eq!(reads[5].0, Duration::from_millis(1050));
}

#[test]
fn five_hz_samples_cannot_enable_current_before_five_elapsed_stable_seconds() {
    let peer = wire::Peer::new();
    let mut driver = wire::driver(&peer);
    driver.connect().unwrap();
    driver.enable_tec().unwrap();
    for _ in 1..=25 {
        peer.clock.wait(TICK);
        settle(&driver);
        assert!(driver.enable_current().is_err(), "25 fast samples still span only 4.8 seconds from the first valid sample");
    }
    assert!(peer.data.lock().unwrap().writes.iter().all(|(_, command)| command != b"STQA000001\r\n"));
    peer.clock.wait(TICK);
    settle(&driver);
    assert!(driver.enable_current().unwrap());
    assert_eq!(driver.status().unwrap().current_ma, 3., "Q=1 reset current must remain honestly read back");
    driver.close().unwrap();
}

#[test]
fn subsecond_temperature_excursion_restarts_the_entire_five_second_qualification() {
    let peer = wire::Peer::new();
    let mut driver = wire::driver(&peer);
    driver.connect().unwrap();
    driver.enable_tec().unwrap();
    for tick in 1..=38 {
        // This excursion falls between the one-second qualification samples at
        // 2.2 s and 3.2 s, and must still invalidate the complete sequence.
        peer.data.lock().unwrap().temperature = if tick == 12 { 22.201 } else { 22. };
        peer.clock.wait(TICK);
        settle(&driver);
        if tick < 38 {
            assert!(driver.enable_current().is_err(), "qualification resumed too early at tick {tick}");
        }
    }
    assert!(driver.enable_current().unwrap(), "five seconds since the first post-excursion sample at 2.6 s");
    driver.close().unwrap();
}

#[test]
fn dense_moderate_deviation_samples_keep_the_time_window_and_emit_current_off_once() {
    let peer = wire::Peer::new();
    let mut driver = wire::driver(&peer);
    driver.connect().unwrap();
    driver.enable_tec().unwrap();
    wire::stable(&peer, &driver);
    driver.enable_current().unwrap();
    let start = peer.data.lock().unwrap().writes.len();
    peer.data.lock().unwrap().temperature = 23.001;
    for tick in 1..=16 {
        peer.clock.wait(TICK);
        settle(&driver);
        let current_off = peer.data.lock().unwrap().writes[start..].iter()
            .filter(|(_, command)| command == b"STQA000000\r\n").count();
        assert_eq!(current_off, usize::from(tick >= 11), "moderate deviation threshold/edge at tick {tick}");
    }
    assert_eq!(driver.state(), DriverState::Ready);
    assert!(!driver.status().unwrap().current_enabled);
    assert!(driver.status().unwrap().tec_enabled);
    assert!(peer.data.lock().unwrap().writes[start..].iter().all(|(_, command)| command != b"STRA000000\r\n"));
    driver.close().unwrap();
    let writes = &peer.data.lock().unwrap().writes;
    assert_eq!(writes[writes.len() - 2].1, b"STQA000000\r\n");
    assert_eq!(writes.last().unwrap().1, b"STRA000000\r\n");
}
