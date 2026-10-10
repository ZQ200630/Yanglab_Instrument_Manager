#![allow(dead_code)]
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use yang_drivers::{
    clock::{Clock, ManualClock},
    transport::{
        serial_abi::{SerialBackend, SerialIo},
        serial_discovery::DeviceRecord,
        CloseReport, Deadline, ResourceBook, SerialConfig,
    },
    voltage::{VoltageConfig, VoltageSource},
    DriverError, DriverResult,
};
pub struct Peer {
    pub data: Mutex<Data>,
    pub clock: Arc<ManualClock>,
    pub gate: (Mutex<(bool, bool)>, Condvar),
}
pub struct Data {
    pub writes: Vec<(Duration, Vec<u8>)>,
    pub write_read_counts: Vec<usize>,
    pub voltages: [f64; 8],
    pub auto: bool,
    pub chunks: VecDeque<Vec<u8>>,
    pub read_errors: VecDeque<DriverError>,
    pub available_error: Option<DriverError>,
    pub availability_checks: usize,
    pub read_limits: Vec<usize>,
    pub empty_read_retains_pending: bool,
    pub pending: bool,
    pub fail_write: bool,
    pub fail_close: bool,
    pub closes: usize,
    pub reads: usize,
    pub configurations: Vec<u32>,
    pub hold: bool,
}
impl Peer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            clock: Arc::new(ManualClock::default()),
            data: Mutex::new(Data {
                writes: vec![],
                write_read_counts: vec![],
                voltages: [2.; 8],
                auto: true,
                chunks: VecDeque::new(),
                read_errors: VecDeque::new(),
                available_error: None,
                availability_checks: 0,
                read_limits: vec![],
                empty_read_retains_pending: false,
                pending: false,
                fail_write: false,
                fail_close: false,
                closes: 0,
                reads: 0,
                configurations: vec![],
                hold: false,
            }),
            gate: (Mutex::new((false, false)), Condvar::new()),
        })
    }
    pub fn release(&self) {
        self.gate.0.lock().unwrap().1 = true;
        self.gate.1.notify_all();
    }
    pub fn held(&self) {
        let ready = self.gate.0.lock().unwrap();
        let (_guard, wait) = self
            .gate
            .1
            .wait_timeout_while(ready, Duration::from_secs(2), |v| !v.0)
            .unwrap();
        assert!(!wait.timed_out());
    }
}
pub struct Backend(pub Arc<Peer>);
struct Io(Arc<Peer>);
impl SerialBackend for Backend {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        panic!("voltage tests never enumerate hardware")
    }
    fn open(&self, port: &str) -> DriverResult<Box<dyn SerialIo>> {
        assert_eq!(port, "\\\\.\\COM12");
        Ok(Box::new(Io(self.0.clone())))
    }
}
pub fn telemetry(values: [f64; 8]) -> Vec<u8> {
    let mut bytes = vec![];
    for (i, value) in values.iter().enumerate() {
        bytes.extend_from_slice(&((*value / 0.0016).round() as u16).to_be_bytes());
        bytes.extend_from_slice(&(-(i as i16) * 100).to_be_bytes());
    }
    bytes.extend_from_slice(b"\r\n");
    bytes
}
impl SerialIo for Io {
    fn configure(&mut self, cfg: &SerialConfig) -> DriverResult<()> {
        self.0
            .data
            .lock()
            .unwrap()
            .configurations
            .push(cfg.baudrate);
        Ok(())
    }
    fn write(&mut self, bytes: &[u8], _: Deadline) -> DriverResult<usize> {
        let mut s = self.0.data.lock().unwrap();
        s.writes.push((self.0.clock.now(), bytes.to_vec()));
        let reads = s.reads;
        s.write_read_counts.push(reads);
        if s.fail_write {
            return Err(DriverError::Native {
                operation: "finite write".into(),
                status: 5,
                transferred: 0,
            });
        }
        assert_eq!(bytes.len(), 18);
        assert_eq!(&bytes[16..], b"\r\n");
        for i in 0..8 {
            s.voltages[i] =
                u16::from_be_bytes([bytes[i * 2], bytes[i * 2 + 1]]) as f64 * 28. / 65535.;
        }
        Ok(bytes.len())
    }
    fn read(&mut self, maximum: usize, _: Deadline) -> DriverResult<Vec<u8>> {
        if self.0.data.lock().unwrap().hold {
            let mut g = self.0.gate.0.lock().unwrap();
            g.0 = true;
            self.0.gate.1.notify_all();
            g = self.0.gate.1.wait_while(g, |v| !v.1).unwrap();
            drop(g);
        }
        let mut s = self.0.data.lock().unwrap();
        s.reads += 1;
        s.read_limits.push(maximum);
        if let Some(error) = s.read_errors.pop_front() {
            return Err(error);
        }
        if let Some(mut chunk) = s.chunks.pop_front() {
            if chunk.len() > maximum {
                let rest = chunk.split_off(maximum);
                s.chunks.push_front(rest);
            }
            return Ok(chunk);
        }
        if s.auto {
            return Ok(telemetry(s.voltages));
        }
        if s.empty_read_retains_pending {
            s.pending = true;
        }
        Err(DriverError::Timeout {
            operation: "finite empty poll".into(),
            transferred: 0,
        })
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        let mut s = self.0.data.lock().unwrap();
        s.closes += 1;
        if !s.fail_close {
            s.pending = false;
        }
        Ok(CloseReport {
            released: !s.fail_close,
            status: None,
        })
    }
    fn has_pending(&self) -> bool {
        self.0.data.lock().unwrap().pending
    }
    fn available(&mut self) -> DriverResult<usize> {
        let mut s = self.0.data.lock().unwrap();
        s.availability_checks += 1;
        if let Some(error) = &s.available_error {
            return Err(error.clone());
        }
        if !s.read_errors.is_empty() {
            return Ok(1);
        }
        if s.chunks.front().is_some_and(|chunk| chunk.is_empty()) {
            s.chunks.pop_front();
            return Ok(0);
        }
        Ok(s.chunks.front().map_or(if s.auto { 34 } else { 0 }, Vec::len))
    }
}
pub fn source(peer: &Arc<Peer>) -> VoltageSource {
    source_with(
        peer,
        VoltageConfig {
            port: "COM12".into(),
            close_timeout: Duration::from_millis(100),
            startup_timeout: Duration::from_millis(500),
            ..VoltageConfig::default()
        },
    )
}
pub fn source_with(peer: &Arc<Peer>, cfg: VoltageConfig) -> VoltageSource {
    VoltageSource::with_backend(
        cfg,
        ResourceBook::isolated(),
        peer.clock.clone(),
        Arc::new(Backend(peer.clone())),
    )
    .unwrap()
}
pub fn until(predicate: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "finite peer did not settle"
        );
        std::thread::yield_now();
    }
}
