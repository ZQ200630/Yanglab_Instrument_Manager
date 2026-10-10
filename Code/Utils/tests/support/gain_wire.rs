#![allow(dead_code)]
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use yang_drivers::{
    clock::{Clock, ManualClock},
    gain::{GainConfig, GainDriver},
    transport::{
        serial_abi::{SerialBackend, SerialIo},
        serial_discovery::DeviceRecord,
        CloseReport, Deadline, ResourceBook, SerialConfig,
    },
    DriverError, DriverResult,
};
pub struct Peer {
    pub data: Mutex<Data>,
    pub clock: Arc<ManualClock>,
    pub gate: (Mutex<(bool, bool)>, Condvar),
}
pub struct Data {
    pub writes: Vec<(Duration, Vec<u8>)>,
    pub pending: VecDeque<u8>,
    pub temperature: f64,
    pub target: f64,
    pub current: f64,
    pub tec: bool,
    pub enabled: bool,
    pub pid: [f64; 3],
    pub faults: VecDeque<(String, Vec<u8>)>,
    pub hold: bool,
    pub whole_reply: bool,
    pub fail_close: bool,
    pub closes: usize,
    pub opens: usize,
    pub packet_budgets: Vec<u32>,
    pub reply_clock_jumps: VecDeque<(String, Duration)>,
}
impl Peer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            clock: Arc::new(ManualClock::default()),
            gate: (Mutex::new((false, false)), Condvar::new()),
            data: Mutex::new(Data {
                writes: vec![],
                pending: VecDeque::new(),
                temperature: 22.,
                target: 22.,
                current: 150.,
                tec: true,
                enabled: false,
                pid: [0.35, 0.1, 0.],
                faults: VecDeque::new(),
                hold: false,
                whole_reply: false,
                fail_close: false,
                closes: 0,
                opens: 0,
                packet_budgets: vec![],
                reply_clock_jumps: VecDeque::new(),
            }),
        })
    }
    pub fn release(&self) {
        self.gate.0.lock().unwrap().1 = true;
        self.gate.1.notify_all();
    }
    pub fn held(&self) {
        let g = self.gate.0.lock().unwrap();
        let (_guard, wait) = self
            .gate
            .1
            .wait_timeout_while(g, Duration::from_secs(2), |v| !v.0)
            .unwrap();
        assert!(!wait.timed_out());
    }
    pub fn sample(&self, driver: &GainDriver) {
        let before = driver.status().unwrap().received_at;
        self.clock.wait(Duration::from_secs(1));
        until(|| driver.status().is_some_and(|s| s.received_at > before));
        // A public read queued after publication settles after the complete
        // monitor iteration, including threshold processing and its next deadline.
        driver.read_temperature().unwrap();
    }
}
pub struct Backend(pub Arc<Peer>);
struct Io(Arc<Peer>);
impl SerialBackend for Backend {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        panic!("explicit Gain binding must not enumerate")
    }
    fn open(&self, port: &str) -> DriverResult<Box<dyn SerialIo>> {
        assert_eq!(port, "\\\\.\\COM13");
        self.0.data.lock().unwrap().opens += 1;
        Ok(Box::new(Io(self.0.clone())))
    }
}
impl SerialIo for Io {
    fn configure(&mut self, config: &SerialConfig) -> DriverResult<()> {
        assert_eq!(config.baudrate, 115200);
        assert!(!config.dtr && !config.rts);
        Ok(())
    }
    fn write(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<usize> {
        let mut s = self.0.data.lock().unwrap();
        s.packet_budgets.push(deadline.remaining_millis()?);
        s.writes.push((self.0.clock.now(), bytes.to_vec()));
        assert!(
            s.pending.is_empty(),
            "a Gain request must consume its entire reply before another write"
        );
        let command = std::str::from_utf8(bytes)
            .unwrap()
            .strip_suffix("\r\n")
            .expect("CRLF");
        let response = if s
            .faults
            .front()
            .is_some_and(|(prefix, _)| command.starts_with(prefix))
        {
            s.faults.pop_front().unwrap().1
        } else {
            let (field, value) = match command {
                "RDTA" => ("T", format!("{:.3}", s.temperature)),
                "RDEA" => ("E", format!("{:.3}", s.target)),
                "RDRA" => ("R", u8::from(s.tec).to_string()),
                "RDCA" => ("C", format!("{:.3}", s.current)),
                "RDQA" => ("Q", u8::from(s.enabled).to_string()),
                "RDPA" => ("P", format!("{:.3}", s.pid[0])),
                "RDIA" => ("I", format!("{:.3}", s.pid[1])),
                "RDDA" => ("D", format!("{:.3}", s.pid[2])),
                "STRA000000" => {
                    s.tec = false;
                    ("D", "0".into())
                }
                "STRA000001" => {
                    s.tec = true;
                    ("D", "1".into())
                }
                "STQA000000" => {
                    s.enabled = false;
                    ("Q", "0".into())
                }
                "STQA000001" => {
                    s.enabled = true;
                    s.current = 3.;
                    ("Q", "1".into())
                }
                "RST" => {
                    s.pid = [0.35, 0.1, 0.];
                    ("", "".into())
                }
                "CLR" => ("", "".into()),
                cmd if cmd.len() == 10 => {
                    let v = cmd[4..].parse::<u32>().unwrap() as f64 / 1000.;
                    match &cmd[..4] {
                        "STEA" => {
                            s.target = v;
                            ("E", format!("{v:.3}"))
                        }
                        "STCA" => {
                            s.current = v;
                            ("C", format!("{v:.3}"))
                        }
                        "STPA" => {
                            s.pid[0] = v;
                            ("P", format!("{v:.3}"))
                        }
                        "STIA" => {
                            s.pid[1] = v;
                            ("I", format!("{v:.3}"))
                        }
                        "STDA" => {
                            s.pid[2] = v;
                            ("D", format!("{v:.3}"))
                        }
                        _ => panic!("unexpected {cmd}"),
                    }
                }
                _ => panic!("unexpected {command}"),
            };
            if field.is_empty() {
                b"READY\r\n".to_vec()
            } else {
                format!("READY;{field}={value}\r\n").into_bytes()
            }
        };
        s.pending.extend(response);
        Ok(bytes.len())
    }
    fn read(&mut self, max: usize, _: Deadline) -> DriverResult<Vec<u8>> {
        if self.0.data.lock().unwrap().hold {
            let mut g = self.0.gate.0.lock().unwrap();
            g.0 = true;
            self.0.gate.1.notify_all();
            g = self.0.gate.1.wait_while(g, |v| !v.1).unwrap();
            drop(g);
        }
        let mut s = self.0.data.lock().unwrap();
        let n = max.min(if s.whole_reply { 512 } else { 3 }).min(s.pending.len());
        if n == 0 {
            return Err(DriverError::Timeout {
                operation: "finite Gain peer".into(),
                transferred: 0,
            });
        }
        let bytes = s.pending.drain(..n).collect();
        if s.pending.is_empty()
            && s.reply_clock_jumps.front().is_some_and(|(prefix, _)| {
                s.writes.last().unwrap().1.starts_with(prefix.as_bytes())
            })
        {
            let (_, duration) = s.reply_clock_jumps.pop_front().unwrap();
            self.0.clock.wait(duration);
        }
        Ok(bytes)
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        let mut s = self.0.data.lock().unwrap();
        s.closes += 1;
        Ok(CloseReport {
            released: !s.fail_close,
            status: None,
        })
    }
    fn has_pending(&self) -> bool {
        false
    }
}
pub fn driver(p: &Arc<Peer>) -> GainDriver {
    GainDriver::with_backend(
        GainConfig {
            port: Some("COM13".into()),
            io_timeout: Duration::from_millis(50),
            close_timeout: Duration::from_millis(100),
            ..Default::default()
        },
        ResourceBook::isolated(),
        p.clock.clone(),
        Arc::new(Backend(p.clone())),
    )
    .unwrap()
}
pub fn until(condition: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(Instant::now() < end, "finite Gain peer did not settle");
        std::thread::yield_now();
    }
}
pub fn stable(p: &Arc<Peer>, d: &GainDriver) {
    for _ in 0..6 {
        p.sample(d)
    }
    d.wait_stable(Deadline::after(Duration::from_millis(100)))
        .unwrap();
}
