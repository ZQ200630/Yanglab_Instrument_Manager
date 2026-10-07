#![allow(dead_code)]
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use yang_drivers::{
    clock::ManualClock,
    mdt::{Mdt693b, MdtConfig},
    transport::{
        serial::SerialConfig,
        serial_abi::{SerialBackend, SerialIo},
        serial_discovery::DeviceRecord,
        CloseReport, Deadline, ResourceBook,
    },
    DriverError, DriverResult,
};
pub const QUERIES: &[&str] = &[
    "?",
    "id?",
    "serial?",
    "friendly?",
    "echo?",
    "vlimit?",
    "intensity?",
    "msenable?",
    "msvoltage?",
    "xvoltage?",
    "yvoltage?",
    "zvoltage?",
    "xmin?",
    "ymin?",
    "zmin?",
    "xmax?",
    "ymax?",
    "zmax?",
    "dacstep?",
    "cm?",
    "rotarymode?",
    "pushdisable?",
];
pub struct Wire {
    pub data: Mutex<Data>,
    pub gate: (Mutex<(bool, bool)>, Condvar),
}
pub struct Data {
    pub commands: Vec<String>,
    pub values: HashMap<String, String>,
    pub reply: VecDeque<u8>,
    pub overrides: HashMap<String, Vec<u8>>,
    pub echo: bool,
    pub ending: String,
    pub closed: usize,
    pub block_read: bool,
    pub fail_close: bool,
    pub reads: usize,
    pub prefix_lf_on: Option<String>,
}
impl Wire {
    pub fn new() -> Arc<Self> {
        let values = HashMap::from(
            [
                ("id?", "MDT693B,1.23"),
                ("serial?", "2110148249-10"),
                ("friendly?", "Left"),
                ("vlimit?", "0"),
                ("intensity?", "7"),
                ("msenable?", "0"),
                ("msvoltage?", "0"),
                ("xvoltage?", "20"),
                ("yvoltage?", "30"),
                ("zvoltage?", "40"),
                ("xmin?", "0"),
                ("ymin?", "0"),
                ("zmin?", "0"),
                ("xmax?", "75"),
                ("ymax?", "75"),
                ("zmax?", "75"),
                ("dacstep?", "10"),
                ("cm?", "0"),
                ("rotarymode?", "2"),
                ("pushdisable?", "1"),
            ]
            .map(|(k, v)| (k.into(), v.into())),
        );
        Arc::new(Self {
            data: Mutex::new(Data {
                commands: vec![],
                values,
                reply: VecDeque::new(),
                overrides: HashMap::new(),
                echo: false,
                ending: "\r\n".into(),
                closed: 0,
                block_read: false,
                fail_close: false,
                reads: 0,
                prefix_lf_on: None,
            }),
            gate: (Mutex::new((false, false)), Condvar::new()),
        })
    }
    pub fn driver(self: &Arc<Self>) -> Mdt693b {
        self.driver_with(Arc::new(ManualClock::default()), false)
    }
    pub fn driver_with(self: &Arc<Self>, clock: Arc<ManualClock>, monitor: bool) -> Mdt693b {
        Mdt693b::with_backend(
            MdtConfig {
                port: "COM15".into(),
                start_monitor: monitor,
                io_timeout: Duration::from_millis(100),
                close_timeout: Duration::from_millis(30),
                ..MdtConfig::default()
            },
            ResourceBook::isolated(),
            clock,
            Arc::new(Backend(self.clone())),
        )
        .unwrap()
    }
    pub fn commands(&self) -> Vec<String> {
        self.data.lock().unwrap().commands.clone()
    }
    pub fn entered(&self) {
        let g = self.gate.0.lock().unwrap();
        let (g, _) = self
            .gate
            .1
            .wait_timeout_while(g, Duration::from_secs(2), |g| !g.0)
            .unwrap();
        assert!(g.0);
    }
    pub fn release(&self) {
        self.gate.0.lock().unwrap().1 = true;
        self.gate.1.notify_all();
    }
}
struct Io(Arc<Wire>);
impl SerialBackend for Wire {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        panic!("explicit binding must not enumerate")
    }
    fn open(&self, _: &str) -> DriverResult<Box<dyn SerialIo>> {
        unreachable!("use shared Backend")
    }
}
// Arc wrapper preserves wire state while returning one owning serial endpoint.
pub struct Backend(pub Arc<Wire>);
impl SerialBackend for Backend {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        panic!("no enum")
    }
    fn open(&self, p: &str) -> DriverResult<Box<dyn SerialIo>> {
        assert_eq!(p, "\\\\.\\COM15");
        Ok(Box::new(Io(self.0.clone())))
    }
}
impl SerialIo for Io {
    fn configure(&mut self, c: &SerialConfig) -> DriverResult<()> {
        assert_eq!(c.baudrate, 115200);
        assert!(!c.dtr && !c.rts);
        Ok(())
    }
    fn write(&mut self, b: &[u8], _: Deadline) -> DriverResult<usize> {
        let c = std::str::from_utf8(b)
            .unwrap()
            .strip_suffix("\r\n")
            .unwrap();
        let mut d = self.0.data.lock().unwrap();
        assert!(d.reply.is_empty(), "old bytes before new command");
        d.commands.push(c.into());
        if let Some(raw) = d.overrides.get(c).cloned() {
            d.reply.extend(raw);
            return Ok(b.len());
        }
        let result = if c == "?" {
            QUERIES.join("\n")
        } else if c == "echo?" {
            if d.echo {
                "1".into()
            } else {
                "0".into()
            }
        } else {
            d.values
                .get(c)
                .unwrap_or_else(|| panic!("unreviewed MDT command: {c}"))
                .clone()
        };
        let e = d.ending.clone();
        let raw = format!(
            "{}{result}{e}*{e}",
            if d.echo {
                format!("{c}{e}")
            } else {
                String::new()
            }
        );
        if d.prefix_lf_on.as_deref() == Some(c) {
            d.reply.push_back(b'\n');
        }
        d.reply.extend(raw.bytes());
        Ok(b.len())
    }
    fn read(&mut self, max: usize, _: Deadline) -> DriverResult<Vec<u8>> {
        if self.0.data.lock().unwrap().block_read {
            let mut g = self.0.gate.0.lock().unwrap();
            g.0 = true;
            self.0.gate.1.notify_all();
            let (g, _) = self
                .0
                .gate
                .1
                .wait_timeout_while(g, Duration::from_secs(2), |g| !g.1)
                .unwrap();
            assert!(g.1);
        }
        let mut d = self.0.data.lock().unwrap();
        d.reads += 1;
        let n = max.min(d.reply.len());
        if n == 0 {
            return Err(DriverError::Timeout {
                operation: "finite MDT wire empty".into(),
                transferred: 0,
            });
        }
        Ok(d.reply.drain(..n).collect())
    }
    fn available(&mut self) -> DriverResult<usize> {
        Ok(self.0.data.lock().unwrap().reply.len())
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        let mut d = self.0.data.lock().unwrap();
        if d.fail_close {
            return Ok(CloseReport {
                released: false,
                status: Some(-1),
            });
        }
        d.closed += 1;
        Ok(CloseReport {
            released: true,
            status: Some(0),
        })
    }
    fn has_pending(&self) -> bool {
        false
    }
}
