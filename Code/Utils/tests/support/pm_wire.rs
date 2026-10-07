#![allow(dead_code)]
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use yang_drivers::{
    clock::ManualClock,
    pm400::{Pm400, PmOptions},
    transport::{visa_abi::*, ResourceBook, VisaManager},
    DriverResult,
};
pub struct Wire {
    pub data: Mutex<Data>,
    pub gate: (Mutex<(bool, bool)>, Condvar),
}
pub struct Data {
    pub commands: Vec<(u32, String)>,
    pub replies: HashMap<String, String>,
    pub buffers: HashMap<u32, VecDeque<u8>>,
    pub next: u32,
    pub closed: Vec<u32>,
    pub block_measure: bool,
    pub block_close: bool,
    pub close_failures: usize,
    pub extra: bool,
    pub timeouts: Vec<u32>,
    pub sensor: String,
    pub error_queue: VecDeque<String>,
    pub ignored_header: Option<String>,
}
impl Wire {
    pub fn new(flags: u32) -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(Data {
                commands: vec![],
                replies: HashMap::from([
                    ("*IDN?".into(), "Thorlabs,PM400,P400-1,1.6".into()),
                    ("SYSTem:ERRor?".into(), "0,\"No error\"".into()),
                    ("SENSe:POWer:DC:UNIT?".into(), "W".into()),
                    ("CONFigure?".into(), "POW".into()),
                    ("*OPC?".into(), "1".into()),
                    ("*ESR?".into(), "32".into()),
                    ("*STB?".into(), "16".into()),
                    ("STATus:OPERation:EVENt?".into(), "7".into()),
                    ("STATus:OPERation:CONDition?".into(), "3".into()),
                ]),
                buffers: HashMap::new(),
                next: 10,
                closed: vec![],
                block_measure: false,
                block_close: false,
                close_failures: 0,
                extra: false,
                timeouts: vec![],
                sensor: format!("\"S,130C\",SN1,cal,1,2,{flags}"),
                error_queue: VecDeque::new(),
                ignored_header: None,
            }),
            gate: (Mutex::new((false, false)), Condvar::new()),
        })
    }
    pub fn manager(self: &Arc<Self>) -> VisaManager {
        VisaManager::from_api(self.clone(), ResourceBook::isolated()).unwrap()
    }
    pub fn pm(self: &Arc<Self>) -> Pm400 {
        Pm400::with_options(
            self.manager(),
            "USB0::0x1313::0x8078::P1::INSTR".into(),
            Arc::new(ManualClock::default()),
            PmOptions {
                timeout: Duration::from_secs(1),
                close_timeout: Duration::from_millis(30),
            },
        )
        .unwrap()
    }
    pub fn commands(&self) -> Vec<String> {
        self.data
            .lock()
            .unwrap()
            .commands
            .iter()
            .map(|(_, s)| s.clone())
            .collect()
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
    fn hold(&self) {
        let mut g = self.gate.0.lock().unwrap();
        g.0 = true;
        self.gate.1.notify_all();
        let (g, _) = self
            .gate
            .1
            .wait_timeout_while(g, Duration::from_secs(2), |g| !g.1)
            .unwrap();
        assert!(g.1);
    }
}
impl VisaApi for Wire {
    fn open_manager(&self) -> (i32, u32) {
        (0, 1)
    }
    fn parse(&self, _: u32, n: &str) -> DriverResult<String> {
        Ok(n.trim().to_uppercase())
    }
    fn find(&self, _: u32) -> (i32, u32, u32, DriverResult<String>) {
        panic!("no enumeration")
    }
    fn find_next(&self, _: u32) -> DriverResult<String> {
        panic!("no enumeration")
    }
    fn open(&self, _: u32, _: &str, m: u32, _: u32) -> (i32, u32) {
        assert_eq!(m, VI_EXCLUSIVE_LOCK);
        let mut d = self.data.lock().unwrap();
        d.next += 1;
        let h = d.next;
        d.buffers.insert(h, VecDeque::new());
        (0, h)
    }
    fn close(&self, h: u32) -> i32 {
        if h != 1 && self.data.lock().unwrap().block_close {
            self.hold();
        }
        let mut d = self.data.lock().unwrap();
        if h != 1 && d.close_failures > 0 {
            d.close_failures -= 1;
            return -1;
        }
        d.closed.push(h);
        0
    }
    fn set_timeout(&self, _: u32, t: u32) -> i32 {
        self.data.lock().unwrap().timeouts.push(t);
        0
    }
    fn set_termination_enabled(&self, _: u32, e: bool) -> i32 {
        assert!(!e);
        0
    }
    fn write(&self, h: u32, b: &[u8]) -> (i32, u32) {
        let command = std::str::from_utf8(b).unwrap().strip_suffix('\n').unwrap();
        let mut d = self.data.lock().unwrap();
        assert!(
            d.buffers[&h].is_empty(),
            "new query before previous response drained"
        );
        d.commands.push((h, command.into()));
        let response = if command == "SYSTem:ERRor?" && !d.error_queue.is_empty() {
            d.error_queue.pop_front()
        } else if command == "SYSTem:SENSor:IDN?" {
            Some(d.sensor.clone())
        } else if let Some(r) = d.replies.get(command) {
            Some(r.clone())
        } else if command.starts_with("MEASure:SCALar:")
            || command == "READ?"
            || command == "FETCh?"
        {
            Some("1.25".into())
        } else if command.starts_with("CONFigure:SCALar:") {
            None
        } else if ["INITiate:IMMediate", "ABORt"].contains(&command) {
            None
        } else if [
            "*RST",
            "*CLS",
            "*OPC",
            "*WAI",
            "STATus:PRESet",
            "SYSTem:BEEPer",
            "SENSe:CORRection:COLLect:ZERO:INITiate",
            "SENSe:CORRection:COLLect:ZERO:ABORt",
        ]
        .contains(&command)
        {
            None
        } else if let Some((header, selector)) = command.split_once("? ") {
            assert!(
                d.replies.contains_key(&format!("{header}?")),
                "unknown property {header}"
            );
            Some(
                match selector {
                    "MINimum" => "0",
                    "MAXimum" => "10",
                    "DEFault" => "3",
                    _ => panic!("unknown limit"),
                }
                .into(),
            )
        } else if let Some((header, value)) = command.split_once(' ') {
            let key = format!("{header}?");
            assert!(d.replies.contains_key(&key), "unknown setter {header}");
            if d.ignored_header.as_deref() != Some(header) {
                let value = match value {
                    "MINimum" => "0",
                    "MAXimum" => "10",
                    "DEFault" => "3",
                    "PHOTodiode" => "PHOT",
                    "THERmal" => "THER",
                    "PYRo" => "PYR",
                    v => v,
                };
                d.replies.insert(key, value.into());
            }
            None
        } else {
            panic!("unreviewed PM command: {command}")
        };
        if let Some(r) = response {
            d.buffers
                .get_mut(&h)
                .unwrap()
                .extend(format!("{r}\n").bytes());
            if d.extra {
                d.buffers.get_mut(&h).unwrap().extend(b"old\n");
            }
        }
        (0, b.len() as u32)
    }
    fn read(&self, h: u32, b: &mut [u8]) -> (i32, u32) {
        let held = {
            let d = self.data.lock().unwrap();
            d.block_measure
                && !d.buffers[&h].is_empty()
                && d.commands
                    .last()
                    .is_some_and(|(_, c)| c.starts_with("MEASure:") || c == "READ?")
        };
        if held {
            self.hold();
        }
        let mut d = self.data.lock().unwrap();
        let q = d.buffers.get_mut(&h).unwrap();
        if q.is_empty() {
            return (VI_ERROR_TMO, 0);
        }
        let n = b.len().min(4).min(q.len());
        for slot in &mut b[..n] {
            *slot = q.pop_front().unwrap();
        }
        (if q.is_empty() { 0 } else { VI_SUCCESS_MAX_CNT }, n as u32)
    }
}

pub fn settings(w: &Wire) {
    let mut d = w.data.lock().unwrap();
    for header in [
        "SENSe:CORRection:LOSS:INPut:MAGNitude",
        "SENSe:CORRection:BEAMdiameter",
        "SENSe:CORRection:WAVelength",
        "SENSe:CORRection:POWer:PDIOde:RESPonse",
        "SENSe:CORRection:POWer:THERmopile:RESPonse",
        "SENSe:CORRection:ENERgy:PYRO:RESPonse",
        "SENSe:CURRent:DC:RANGe:UPPer",
        "SENSe:CURRent:DC:REFerence",
        "SENSe:ENERgy:RANGe:UPPer",
        "SENSe:ENERgy:REFerence",
        "SENSe:POWer:DC:RANGe:UPPer",
        "SENSe:POWer:DC:REFerence",
        "SENSe:VOLTage:DC:RANGe:UPPer",
        "SENSe:VOLTage:DC:REFerence",
        "SENSe:PEAKdetector:THReshold",
        "INPut:THERmopile:ACCelerator:TAU",
        "DISPlay:BRIGhtness",
        "DISPlay:CONTrast",
        "SENSe:FREQuency:RANGe:UPPer",
        "SENSe:FREQuency:RANGe:LOWer",
        "SENSe:CORRection:COLLect:ZERO:MAGNitude",
    ] {
        d.replies.insert(format!("{header}?"), "5".into());
    }
    for header in [
        "SENSe:CURRent:DC:RANGe:AUTO",
        "SENSe:CURRent:DC:REFerence:STATe",
        "SENSe:ENERgy:REFerence:STATe",
        "SENSe:POWer:DC:RANGe:AUTO",
        "SENSe:POWer:DC:REFerence:STATe",
        "SENSe:VOLTage:DC:RANGe:AUTO",
        "SENSe:VOLTage:DC:REFerence:STATe",
        "INPut:PDIOde:FILTer:LPASs:STATe",
        "INPut:THERmopile:ACCelerator:STATe",
        "INPut:THERmopile:ACCelerator:AUTO",
        "SYSTem:BEEPer:STATe",
        "SENSe:CORRection:COLLect:ZERO:STATe",
    ] {
        d.replies.insert(format!("{header}?"), "0".into());
    }
    for (key, val) in [
        ("SENSe:AVERage:COUNt?", "4"),
        ("INPut:ADAPter:TYPE?", "PHOT"),
        ("SYSTem:DATE?", "2026,10,7"),
        ("SYSTem:TIME?", "12,30,1.25"),
        ("SYSTem:LFRequency?", "60"),
        ("SYSTem:VERSion?", "1999.0"),
        ("CALibration:STRing?", "calibrated 2025"),
        ("*ESE?", "0"),
        ("*SRE?", "0"),
        ("*TST?", "0"),
    ] {
        d.replies.insert(key.into(), val.into());
    }
    for group in ["MEASurement", "AUXiliary", "OPERation", "QUEStionable"] {
        for suffix in ["PTRansition", "NTRansition", "ENABle"] {
            d.replies
                .insert(format!("STATus:{group}:{suffix}?"), "0".into());
        }
    }
}
