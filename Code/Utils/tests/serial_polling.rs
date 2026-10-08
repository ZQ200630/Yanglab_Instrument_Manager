#[path = "support/gain_wire.rs"]
mod gain;
#[path = "support/mdt_wire.rs"]
mod mdt;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use yang_drivers::{
    clock::SystemClock,
    gain::{GainConfig, GainDriver},
    mdt::{Mdt693b, MdtConfig},
    transport::{
        serial_abi::{SerialBackend, SerialIo},
        serial_discovery::DeviceRecord,
        CloseReport, Deadline, ResourceBook, SerialConfig,
    },
    DriverResult,
};

// The Windows first-byte timeout is a poll boundary, not the packet deadline.
struct PollBackend {
    inner: Arc<dyn SerialBackend>,
    polls: Arc<AtomicUsize>,
}
struct PollIo {
    inner: Box<dyn SerialIo>,
    polls: Arc<AtomicUsize>,
}
impl SerialBackend for PollBackend {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        panic!("bound fixture must not enumerate")
    }
    fn open(&self, port: &str) -> DriverResult<Box<dyn SerialIo>> {
        Ok(Box::new(PollIo {
            inner: self.inner.open(port)?,
            polls: self.polls.clone(),
        }))
    }
}
impl SerialIo for PollIo {
    fn configure(&mut self, c: &SerialConfig) -> DriverResult<()> {
        self.inner.configure(c)
    }
    fn write(&mut self, b: &[u8], d: Deadline) -> DriverResult<usize> {
        self.inner.write(b, d)
    }
    fn read(&mut self, n: usize, d: Deadline) -> DriverResult<Vec<u8>> {
        if self
            .polls
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_sub(1))
            .is_ok()
        {
            std::thread::sleep(Duration::from_millis(u64::from(
                d.remaining_millis()?.min(50),
            )));
            return Ok(vec![]);
        }
        self.inner.read(n, d)
    }
    fn available(&mut self) -> DriverResult<usize> {
        self.inner.available()
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        self.inner.close()
    }
    fn has_pending(&self) -> bool {
        self.inner.has_pending()
    }
}
fn polling(inner: Arc<dyn SerialBackend>, n: usize) -> Arc<dyn SerialBackend> {
    Arc::new(PollBackend {
        inner,
        polls: Arc::new(AtomicUsize::new(n)),
    })
}
#[test]
fn gain_empty_polls_preserve_packet_deadline_and_do_not_replay() {
    let wire = gain::Peer::new();
    let mut driver = GainDriver::with_backend(
        GainConfig {
            port: Some("COM13".into()),
            io_timeout: Duration::from_millis(400),
            start_watchdog: false,
            ..Default::default()
        },
        ResourceBook::isolated(),
        Arc::new(SystemClock::default()),
        polling(Arc::new(gain::Backend(wire.clone())), 2),
    )
    .unwrap();
    let result = driver.probe_identity();
    assert!(result.is_ok(), "valid reply after two polls: {result:?}");
    let data = wire.data.lock().unwrap();
    assert_eq!(
        data.writes.iter().filter(|(_, w)| w == b"RDTA\r\n").count(),
        1
    );
    assert!(data.writes.iter().all(|(_, w)| w.starts_with(b"RD")));
    assert_eq!(data.closes, 1);
}
#[test]
fn mdt_empty_polls_preserve_packet_deadline_and_do_not_replay() {
    let wire = mdt::Wire::new();
    let mut driver = Mdt693b::with_backend(
        MdtConfig {
            port: "COM15".into(),
            io_timeout: Duration::from_millis(400),
            start_monitor: false,
            ..Default::default()
        },
        ResourceBook::isolated(),
        Arc::new(SystemClock::default()),
        polling(Arc::new(mdt::Backend(wire.clone())), 2),
    )
    .unwrap();
    let result = driver.connect();
    assert!(result.is_ok(), "valid prompt after two polls: {result:?}");
    assert_eq!(
        wire.data
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|s| s.as_str() == "?")
            .count(),
        1
    );
    assert!(driver.close().unwrap().resources_released());
}
#[test]
fn gain_repeated_empty_polls_expire_original_deadline_without_replay() {
    let wire = gain::Peer::new();
    let mut driver = GainDriver::with_backend(
        GainConfig {
            port: Some("COM13".into()),
            io_timeout: Duration::from_millis(85),
            start_watchdog: false,
            ..Default::default()
        },
        ResourceBook::isolated(),
        Arc::new(SystemClock::default()),
        polling(Arc::new(gain::Backend(wire.clone())), usize::MAX),
    )
    .unwrap();
    let start = Instant::now();
    assert!(driver.probe_identity().is_err());
    assert!(
        start.elapsed() >= Duration::from_millis(75),
        "poll was treated as terminal timeout"
    );
    assert!(
        start.elapsed() < Duration::from_millis(400),
        "deadline was extended"
    );
    assert_eq!(wire.data.lock().unwrap().writes.len(), 1);
    assert!(!driver.has_resource_responsibility());
}
#[test]
fn mdt_repeated_empty_polls_expire_original_deadline_without_replay() {
    let wire = mdt::Wire::new();
    let mut driver = Mdt693b::with_backend(
        MdtConfig {
            port: "COM15".into(),
            io_timeout: Duration::from_millis(85),
            start_monitor: false,
            ..Default::default()
        },
        ResourceBook::isolated(),
        Arc::new(SystemClock::default()),
        polling(Arc::new(mdt::Backend(wire.clone())), usize::MAX),
    )
    .unwrap();
    let start = Instant::now();
    assert!(driver.connect().is_err());
    assert!(
        start.elapsed() >= Duration::from_millis(75),
        "poll was treated as terminal timeout"
    );
    assert!(
        start.elapsed() < Duration::from_millis(400),
        "deadline was extended"
    );
    assert_eq!(wire.data.lock().unwrap().commands.len(), 1);
    assert!(driver.close().unwrap().resources_released());
}
