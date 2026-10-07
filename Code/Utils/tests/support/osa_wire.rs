#![allow(dead_code)]
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use yang_drivers::{
    clock::ManualClock,
    osa::{Osa, OsaOptions, TransferFormat},
    transport::{visa_abi::*, Deadline, ResourceBook, VisaManager},
    DriverResult,
};
pub struct Wire {
    pub data: Mutex<Data>,
    pub gate: (Mutex<(bool, bool)>, Condvar),
}
pub struct Data {
    pub commands: Vec<String>,
    pub read_sizes: Vec<usize>,
    pub counts: HashMap<String, usize>,
    pub metadata: HashMap<String, String>,
    pub buffer: VecDeque<u8>,
    pub points: usize,
    pub format: TransferFormat,
    pub fragment: usize,
    pub changed_center: bool,
    pub opc_timeout: bool,
    pub close_failures: usize,
    pub closed: Vec<u32>,
    pub block_y: bool,
    pub block_close: bool,
    pub open_error_with_handle: bool,
}
impl Wire {
    pub fn new(points: usize, format: TransferFormat) -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(Data {
                commands: vec![],
                read_sizes: vec![],
                counts: HashMap::new(),
                metadata: HashMap::new(),
                buffer: VecDeque::new(),
                points,
                format,
                fragment: 3,
                changed_center: false,
                opc_timeout: false,
                close_failures: 0,
                closed: vec![],
                block_y: false,
                block_close: false,
                open_error_with_handle: false,
            }),
            gate: (Mutex::new((false, false)), Condvar::new()),
        })
    }
    pub fn manager(self: &Arc<Self>) -> VisaManager {
        VisaManager::from_api(self.clone(), ResourceBook::isolated()).unwrap()
    }
    pub fn osa(self: &Arc<Self>) -> Osa {
        Osa::new(
            self.manager(),
            "GPIB0::4::INSTR".into(),
            Arc::new(ManualClock::default()),
        )
        .unwrap()
    }
    pub fn entered(&self) {
        let gate = self.gate.0.lock().unwrap();
        let (gate, _) = self
            .gate
            .1
            .wait_timeout_while(gate, Duration::from_secs(2), |g| !g.0)
            .unwrap();
        assert!(gate.0);
    }
    pub fn release(&self) {
        self.gate.0.lock().unwrap().1 = true;
        self.gate.1.notify_all();
    }
}
impl VisaApi for Wire {
    fn set_termination_enabled(&self, _: u32, enabled: bool) -> i32 {
        assert!(!enabled);
        0
    }
    fn open_manager(&self) -> (i32, u32) {
        (0, 1)
    }
    fn parse(&self, _: u32, name: &str) -> DriverResult<String> {
        Ok(name.into())
    }
    fn find(&self, _: u32) -> (i32, u32, u32, DriverResult<String>) {
        panic!("OSA must never enumerate during connect")
    }
    fn find_next(&self, _: u32) -> DriverResult<String> {
        panic!("OSA must never enumerate")
    }
    fn open(&self, _: u32, _: &str, mode: u32, _: u32) -> (i32, u32) {
        assert_eq!(mode, VI_EXCLUSIVE_LOCK);
        (
            if self.data.lock().unwrap().open_error_with_handle {
                -1
            } else {
                0
            },
            11,
        )
    }
    fn close(&self, handle: u32) -> i32 {
        if handle == 11 && self.data.lock().unwrap().block_close {
            let mut gate = self.gate.0.lock().unwrap();
            gate.0 = true;
            self.gate.1.notify_all();
            let (gate, wait) = self
                .gate
                .1
                .wait_timeout_while(gate, Duration::from_secs(2), |g| !g.1)
                .unwrap();
            assert!(gate.1 && !wait.timed_out());
        }
        let mut data = self.data.lock().unwrap();
        data.closed.push(handle);
        if handle == 11 && data.close_failures > 0 {
            data.close_failures -= 1;
            -1
        } else {
            0
        }
    }
    fn set_timeout(&self, _: u32, millis: u32) -> i32 {
        assert!(millis > 0 && millis < u32::MAX);
        0
    }
    fn write(&self, _: u32, bytes: &[u8]) -> (i32, u32) {
        let command = std::str::from_utf8(bytes)
            .unwrap()
            .trim_end_matches('\n')
            .to_owned();
        let mut data = self.data.lock().unwrap();
        data.commands.push(command.clone());
        let count = data.counts.entry(command.clone()).or_default();
        *count += 1;
        let count = *count;
        let text = if let Some(value) = data.metadata.get(&command) {
            value.clone()
        } else {
            match command.as_str() {
                "*IDN?" => "YOKOGAWA,AQ6370E,SN,FW".into(),
                "*OPC?" => "1".into(),
                ":TRACe:ACTive?" => "TRA".into(),
                ":UNIT:X?" => "0".into(),
                ":FORMat:DATA?" => data.format.wire_name().into(),
                ":TRACe:DATA:SNUMber? TRA" => data.points.to_string(),
                ":DISPlay:TRACe:Y1:SCALe:SPACing?"
                | ":DISPlay:TRACe:Y1:SCALe:UNIT?"
                | ":TRACe:ATTRibute?" => "0".into(),
                ":SENSe:WAVelength:CENTer?" => if data.changed_center && count > 1 {
                    "1.56e-6"
                } else {
                    "1.55e-6"
                }
                .into(),
                ":SENSe:WAVelength:SPAN?" => "2e-9".into(),
                ":SENSe:BWIDth:RESolution?" => "2e-11".into(),
                ":INITiate:SMODe?" => "2".into(),
                ":INITiate:IMMediate" | ":ABORt" => return (0, bytes.len() as u32),
                _ => {
                    let (axis, range) = command
                        .split_once("? TRA,")
                        .unwrap_or_else(|| panic!("Unreviewed command: {command}"));
                    assert!(matches!(axis, ":TRACe:X" | ":TRACe:Y"));
                    let (first, last) = range.split_once(',').unwrap();
                    let first: usize = first.parse().unwrap();
                    let last: usize = last.parse().unwrap();
                    assert!(
                        first >= 1 && last <= data.points && last >= first && last - first < 1024
                    );
                    let values: Vec<_> = (first..=last)
                        .map(|i| {
                            if axis.ends_with('X') {
                                1.55e-6 + (i - 1) as f64 * 1e-9
                            } else {
                                -40. + (i - 1) as f64
                            }
                        })
                        .collect();
                    if data.format == TransferFormat::Ascii {
                        values
                            .iter()
                            .map(|v| format!("{v:.17e}"))
                            .collect::<Vec<_>>()
                            .join(",")
                    } else {
                        let mut payload = Vec::new();
                        for value in values {
                            if data.format == TransferFormat::Real32 {
                                payload.extend((value as f32).to_le_bytes())
                            } else {
                                payload.extend(value.to_le_bytes())
                            }
                        }
                        let length = payload.len().to_string();
                        let mut reply = format!("#{}{length}", length.len()).into_bytes();
                        reply.extend(payload);
                        reply.push(b'\n');
                        data.buffer = reply.into();
                        return (0, bytes.len() as u32);
                    }
                }
            }
        };
        data.buffer = format!("{text}\n").into_bytes().into();
        (0, bytes.len() as u32)
    }
    fn read(&self, _: u32, bytes: &mut [u8]) -> (i32, u32) {
        let data = self.data.lock().unwrap();
        let last = data.commands.last().unwrap().clone();
        let block = data.block_y && last.starts_with(":TRACe:Y?");
        let timeout = data.opc_timeout && last == "*OPC?";
        drop(data);
        if block {
            let mut gate = self.gate.0.lock().unwrap();
            gate.0 = true;
            self.gate.1.notify_all();
            let (gate, wait) = self
                .gate
                .1
                .wait_timeout_while(gate, Duration::from_secs(2), |g| !g.1)
                .unwrap();
            assert!(gate.1 && !wait.timed_out());
        }
        if timeout {
            return (VI_ERROR_TMO, 0);
        }
        let mut data = self.data.lock().unwrap();
        assert!(bytes.len() <= 4096);
        data.read_sizes.push(bytes.len());
        let count = bytes.len().min(data.fragment).min(data.buffer.len());
        for byte in &mut bytes[..count] {
            *byte = data.buffer.pop_front().unwrap();
        }
        (
            if data.buffer.is_empty() {
                0
            } else {
                0x3fff0006
            },
            count as u32,
        )
    }
}
pub fn deadline() -> Deadline {
    Deadline::after(Duration::from_secs(2))
}
pub fn quick_options() -> OsaOptions {
    OsaOptions {
        timeout: Duration::from_secs(2),
        close_timeout: Duration::from_millis(50),
    }
}
