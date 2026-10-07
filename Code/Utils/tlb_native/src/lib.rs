use std::collections::BTreeMap;
use serde::{Serialize,Deserialize};
#[derive(Clone,Debug,Serialize,PartialEq)]
pub struct Error { pub kind: &'static str, pub message: String }
pub type Result<T> = std::result::Result<T,Error>;
fn fail(kind: &'static str, message: impl Into<String>) -> Error { Error {kind,message:message.into()} }
pub trait Wire { fn open(&mut self)->Result<Vec<String>>; fn query(&mut self,key:&str,command:&str)->Result<String>; fn close(&mut self)->Result<()>; }
#[derive(Clone,Debug,Serialize,PartialEq)]
pub struct Identity { pub manufacturer:String,pub model:String,pub serial:String,pub firmware:String,pub head_model:String,pub head_serial:String }
#[derive(Debug,Serialize)]
pub struct Status { pub wavelength_nm:f64,pub wavelength_setpoint_nm:f64,pub power_mw:f64,pub power_setpoint_mw:f64,pub current_ma:f64,pub current_setpoint_ma:f64,pub piezo_percent:f64,pub output_enabled:bool,pub tracking:bool,pub remote:bool,pub constant_power:bool,pub operation_complete:bool,pub status_byte:u8,pub read_interval_s:f64 }
#[derive(Debug,Serialize)]
pub struct Motion { pub wavelength_nm:f64,pub wavelength_setpoint_nm:f64,pub tracking:bool,pub operation_complete:bool,pub read_interval_s:f64 }
#[derive(Clone,Debug,Deserialize)]
#[serde(tag="name",content="value",rename_all="snake_case",deny_unknown_fields)]
pub enum Action { Remote(bool),Wavelength(f64),Piezo(f64),Tracking(bool),Output(bool) }
#[derive(Clone,Debug,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanPlan { pub start_nm:f64, pub stop_nm:f64, pub speed_nm_s:f64, pub return_speed_nm_s:Option<f64> }
#[derive(Clone,Copy,Debug,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlLimits { pub min_nm:f64,pub max_nm:f64,pub max_speed_nm_s:f64 }
#[derive(Clone,Debug,Deserialize)]
#[serde(tag="name",content="value",rename_all="snake_case",deny_unknown_fields)]
pub enum Control { Target(f64),Wavelength(f64),Piezo(f64),Tracking(bool),Output(bool),ScanStart(ScanPlan),ScanStop }
pub mod sdk;
pub mod rpc;

pub fn valid_key(key: &str) -> bool {
    key.strip_prefix("6700 SN").is_some_and(|s| !s.is_empty() && s.len() <= 16 && s.bytes().all(|b| b.is_ascii_digit()))
}
pub fn model_spec(head:&str)->Option<(f64,f64,f64)> {
    let model=head.strip_prefix("TLB-").unwrap_or(head);
    // Only the documented permanent fiber-coupling suffix is normalized.
    let model=model.strip_suffix("-P").unwrap_or(model);
    let table:serde_json::Value=serde_json::from_str(include_str!("../../tlb_models.json")).ok()?;
    Some((table[model][0].as_f64()?,table[model][1].as_f64()?,table[model][2].as_f64()?))
}
pub fn wavelength_range(head:&str)->Option<(f64,f64)> {model_spec(head).map(|(a,b,_)|(a,b))}
pub fn max_scan_speed(head:&str)->Option<f64> {model_spec(head).map(|(_,_,v)|v)}
pub fn controller_identity(raw: &str) -> Result<(String, String)> {
    let words: Vec<_> = raw.split_whitespace().collect();
    if words.len()!=5 || !words[0].eq_ignore_ascii_case("New_Focus") || words[1]!="6700" ||
        !words[2].starts_with('v') || words[2].len()<2 {
        return Err(fail("protocol","Invalid Newport controller identity"));
    }
    let serial=words[4].strip_prefix("SN").ok_or_else(||fail("protocol","Missing controller serial"))?;
    let key=format!("6700 SN{serial}");
    if !valid_key(&key) {return Err(fail("protocol","Invalid controller serial"));}
    Ok((serial.into(),words[2][1..].into()))
}
fn valid_head(head:&str)->bool {
    let head=head.strip_prefix("TLB-").unwrap_or(head);
    let mut parts=head.split('-');let first=parts.next().unwrap_or("");
    first.len()==4 && first.bytes().all(|b|b.is_ascii_digit()) &&
        parts.all(|p|!p.is_empty() && p.bytes().all(|b|b.is_ascii_alphanumeric()))
}
#[derive(Clone)]
struct Laser { identity:Identity, fault:bool, limits:Option<ControlLimits> }
/// One SDK lifetime, serialized by the owning executor. Retained release blocks new work.
pub struct Bus<T:Wire> { wire:T, opened:bool, retained:bool, keys:Vec<String>, lasers:BTreeMap<String,Laser> }
impl<T:Wire> Bus<T> {
    pub fn new(wire:T)->Self {Self {wire,opened:false,retained:false,keys:Vec::new(),lasers:BTreeMap::new()}}
    pub fn resources_released(&self)->bool { !self.opened && !self.retained && self.lasers.is_empty() }
    fn release_sdk(&mut self)->Result<()> {
        match self.wire.close() {Ok(())=>{self.opened=false;self.retained=false;self.keys.clear();Ok(())},Err(e)=>{self.retained=true;Err(e)}}
    }
    fn open_sdk(&mut self)->Result<()> {
        if self.retained {return Err(fail("connection","Newport cleanup remains unresolved"));}
        if self.opened {return Ok(());}
        // open may have acquired SDK handles before failing; always attempt preserving release.
        self.opened=true;
        let opened=self.wire.open().and_then(|keys|{
            let unique: std::collections::BTreeSet<_>=keys.iter().collect();
            if keys.len()>32 || unique.len()!=keys.len() || keys.iter().any(|k|!valid_key(k)) {
                Err(fail("protocol","Missing, duplicate or invalid Newport controllers"))
            } else {Ok(keys)}
        });
        match opened {Ok(keys)=>{self.keys=keys;Ok(())},Err(e)=>{if let Err(close)=self.release_sdk(){return Err(fail("connection",format!("{}; cleanup retained: {}",e.message,close.message)));}Err(e)}}
    }
    pub fn enumerate(&mut self)->Result<Vec<String>> {
        if self.opened || self.retained || !self.lasers.is_empty() {
            return Err(fail("connection","Disconnect Newport controllers before refreshing the device list"));
        }
        self.open_sdk()?;let keys=self.keys.clone();self.release_sdk()?;Ok(keys)
    }
    pub fn discover(&mut self)->Result<Vec<Identity>> {
        if self.opened || self.retained || !self.lasers.is_empty() {
            return Err(fail("connection","Disconnect Newport controllers before refreshing the device list"));
        }
        self.open_sdk()?;
        let identities=self.keys.clone().into_iter().map(|key|self.identify(&key)).collect::<Result<Vec<_>>>();
        // One preserving release attempt, including a failed head read. Never return partial identities.
        let release=self.release_sdk();
        match (identities,release) {
            (Ok(heads),Ok(()))=>Ok(heads), (Err(error),Ok(()))=>Err(error),
            (result,Err(cleanup))=>Err(fail("connection",format!("{}; cleanup retained: {}",
                result.err().map_or_else(||"Head discovery completed".into(),|e|e.message),cleanup.message))),
        }
    }
    fn identify(&mut self,key:&str)->Result<Identity> {
        if !self.keys.iter().any(|k|k==key){return Err(fail("connection","Selected controller was not found"));}
        let raw=self.wire.query(key,"*IDN?")?;
        if raw.len()>64 {return Err(fail("protocol","Controller identity exceeds capacity"));}
        let (serial,firmware)=controller_identity(&raw)?;
        if format!("6700 SN{serial}")!=key {return Err(fail("connection","Controller identity differs from the selected serial"));}
        let head=self.wire.query(key,"SYST:LAS:MODEL?")?;
        let head_serial=self.wire.query(key,"SYST:LAS:SN?")?;
        if head.len()>64 || !valid_head(&head) || head_serial.is_empty() || head_serial.len()>64 ||
            !head_serial.bytes().all(|b|b.is_ascii_alphanumeric() || b==b'_' || b==b'-') {
            return Err(fail("protocol","Invalid laser-head identity"));
        }
        Ok(Identity{manufacturer:"New Focus".into(),model:"TLB-6700".into(),serial,firmware,head_model:head,head_serial})
    }
    pub fn connect(&mut self,key:&str)->Result<Identity> {
        if !valid_key(key){return Err(fail("connection","Select a detected controller serial"));}
        if self.lasers.contains_key(key){return Err(fail("connection","This controller already has a session"));}
        self.open_sdk()?;
        let result=self.identify(key);
        match result {Ok(id)=>{self.lasers.insert(key.into(),Laser{identity:id.clone(),fault:false,limits:None});Ok(id)},Err(e)=>{
            if self.lasers.is_empty(){if let Err(close)=self.release_sdk(){return Err(fail("connection",format!("{}; cleanup retained: {}",e.message,close.message)));}}
            Err(e)
        }}
    }
    fn ready(&self,key:&str)->Result<&Laser> {
        let laser=self.lasers.get(key).ok_or_else(||fail("connection","No session for this controller"))?;
        if laser.fault || self.retained {return Err(fail("connection","Controller fault; disconnect before reconnecting"));}
        Ok(laser)
    }
    pub fn set_limits(&mut self,key:&str,limits:ControlLimits)->Result<()> {
        let laser=self.ready(key)?;
        let (a,b)=wavelength_range(&laser.identity.head_model).ok_or_else(||fail("safety","Unknown laser-head limits"))?;
        if !limits.min_nm.is_finite()||!limits.max_nm.is_finite()||!limits.max_speed_nm_s.is_finite()||
            limits.min_nm<a||limits.max_nm>b||limits.min_nm>=limits.max_nm||limits.max_speed_nm_s<0.01||
            limits.max_speed_nm_s>max_scan_speed(&laser.identity.head_model).unwrap() {
            return Err(fail("safety","Operating limits can only narrow the hardware envelope"));
        }
        self.lasers.get_mut(key).unwrap().limits=Some(limits);Ok(())
    }
    fn effective_bounds(laser:&Laser)->Option<(f64,f64)> {
        wavelength_range(&laser.identity.head_model).map(|(a,b)|laser.limits.map_or((a,b),|l|(a.max(l.min_nm),b.min(l.max_nm))))
    }
    fn query(&mut self,key:&str,command:&str)->Result<String> {
        let result=self.wire.query(key,command).and_then(|s|{
            if s.is_empty() || s.len()>64 || !s.bytes().all(|b|(32..=126).contains(&b)) {
                Err(fail("protocol","Invalid scalar TLB response"))
            } else {Ok(s)}
        });
        if result.is_err(){if let Some(l)=self.lasers.get_mut(key){l.fault=true;}}
        result
    }
    fn invalid(&mut self,key:&str,message:String)->Error {
        if let Some(l)=self.lasers.get_mut(key){l.fault=true;}fail("protocol",message)
    }
    fn number(&mut self,key:&str,command:&str,min:Option<f64>,max:Option<f64>)->Result<f64> {
        let text=self.query(key,command)?;
        match text.parse::<f64>() {Ok(v) if v.is_finite() && min.map_or(true,|b|v>=b) && max.map_or(true,|b|v<=b)=>Ok(v),
            _=>Err(self.invalid(key,format!("Invalid numeric TLB response for {command}: {text}")))}
    }
    fn switch(&mut self,key:&str,command:&str)->Result<bool> {
        let text=self.query(key,command)?;match text.to_ascii_uppercase().as_str(){"0"|"OFF"=>Ok(false),"1"|"ON"=>Ok(true),
            _=>Err(self.invalid(key,format!("Invalid TLB switch response for {command}")))}
    }
    fn remote(&mut self,key:&str)->Result<bool> {
        let text=self.query(key,"SYST:MCONT?")?;match text.to_ascii_uppercase().as_str(){"LOC"=>Ok(false),"REM"=>Ok(true),
            _=>Err(self.invalid(key,"Invalid TLB remote/local response".into()))}
    }
    pub fn status(&mut self,key:&str)->Result<Status> {
        self.ready(key)?;let started=std::time::Instant::now();
        let output_enabled=self.switch(key,"OUTP:STAT?")?;
        let tracking=self.switch(key,"OUTP:TRAC?")?;
        let remote=self.remote(key)?;let constant_power=self.switch(key,"SOUR:CPOW?")?;
        let wavelength_nm=self.number(key,"SENS:WAVE",Some(0.),None)?;
        let wavelength_setpoint_nm=self.number(key,"SOUR:WAVE?",Some(0.),None)?;
        let power_mw=self.number(key,"SENS:POW:DIODE",None,None)?;
        let power_setpoint_mw=self.number(key,"SOUR:POW:DIODE?",Some(0.),None)?;
        let current_ma=self.number(key,"SENS:CURR:DIODE",None,None)?;
        let current_setpoint_ma=self.number(key,"SOUR:CURR:DIODE?",Some(0.),None)?;
        let piezo_percent=self.number(key,"SOUR:VOLT:PIEZ?",Some(0.),Some(100.))?;
        let operation_complete=self.switch(key,"*OPC?")?;
        let byte=self.number(key,"*STB?",Some(0.),Some(255.))?;
        if byte.fract()!=0. {return Err(self.invalid(key,"Invalid TLB status byte".into()));}
        Ok(Status{wavelength_nm,wavelength_setpoint_nm,power_mw,power_setpoint_mw,current_ma,current_setpoint_ma,
            piezo_percent,output_enabled,tracking,remote,constant_power,operation_complete,status_byte:byte as u8,read_interval_s:started.elapsed().as_secs_f64()})
    }
    pub fn motion(&mut self,key:&str)->Result<Motion> {
        self.ready(key)?;let started=std::time::Instant::now();
        let was_complete=self.switch(key,"*OPC?")?;
        let wavelength_nm=self.number(key,"SENS:WAVE",Some(0.),None)?;
        let wavelength_setpoint_nm=self.number(key,"SOUR:WAVE?",Some(0.),None)?;
        let tracking=self.switch(key,"OUTP:TRAC?")?;
        let still_complete=self.switch(key,"*OPC?")?;
        let operation_complete=was_complete&&still_complete;
        Ok(Motion{wavelength_nm,wavelength_setpoint_nm,tracking,operation_complete,read_interval_s:started.elapsed().as_secs_f64()})
    }
    pub fn action(&mut self,key:&str,action:Action,confirm:bool)->Result<()> {
        let laser=self.ready(key)?;let bounds=Self::effective_bounds(laser);
        if !confirm {return Err(fail("safety","Explicit operator confirmation is required"));}
        let output_off=matches!(&action,Action::Output(false));
        if bounds.is_none() && !output_off {return Err(fail("safety","This laser head is read-only until its limits are reviewed"));}
        match &action {
            Action::Wavelength(v) if !v.is_finite() || bounds.map_or(true,|(a,b)|*v<a || *v>b)=>return Err(fail("safety","Wavelength is outside the reviewed head envelope")),
            Action::Piezo(v) if !v.is_finite() || !(0. ..=100.).contains(v)=>return Err(fail("safety","Piezo must be 0–100 percent")),_=>{}
        }
        if !matches!(&action,Action::Remote(_)) && !self.remote(key)? {return Err(fail("safety","Select Remote before changing laser settings"));}
        if !output_off && !self.switch(key,"*OPC?")? {return Err(fail("safety","Controller is busy; no control command was sent"));}
        if matches!(&action,Action::Wavelength(_)) && !self.switch(key,"OUTP:TRAC?")? {return Err(fail("safety","Enable wavelength tracking before setting wavelength"));}
        let command=match action {Action::Remote(v)=>format!("SYST:MCONT {}",if v{"REM"}else{"LOC"}),
            Action::Wavelength(v)=>format!("SOUR:WAVE {v}"),Action::Piezo(v)=>format!("SOUR:VOLT:PIEZ {v}"),
            Action::Tracking(v)=>format!("OUTP:TRAC {}",u8::from(v)),Action::Output(v)=>format!("OUTP:STAT {}",u8::from(v))};
        if !self.query(key,&command)?.eq_ignore_ascii_case("OK"){return Err(self.invalid(key,"TLB rejected the command; no retry was attempted".into()));}
        Ok(())
    }
    fn command(&mut self,key:&str,command:&str)->Result<()> {
        if !self.query(key,command)?.eq_ignore_ascii_case("OK") {
            return Err(self.invalid(key,"TLB rejected the command; no retry was attempted".into()));
        }
        Ok(())
    }
    /// An explicit operator action. All preflight precedes writes; the owner executes
    /// the whole composite serially. A fault holds state, with no cleanup write/replay.
    pub fn control(&mut self,key:&str,control:Control,confirm:bool)->Result<()> {
        let laser=self.ready(key)?;let head=laser.identity.head_model.clone();let bounds=Self::effective_bounds(laser);
        let speed_cap=max_scan_speed(&head).map(|v|laser.limits.map_or(v,|l|v.min(l.max_speed_nm_s)));
        if !confirm {return Err(fail("safety","Explicit operator confirmation is required"));}
        let stopping=matches!(&control,Control::Output(false)|Control::Tracking(false)|Control::ScanStop);
        if bounds.is_none() && !stopping {return Err(fail("safety","Unknown laser-head control limits"));}
        let inside=|v:f64|v.is_finite()&&bounds.is_some_and(|(a,b)|v>=a&&v<=b);
        match &control {
            Control::Wavelength(v)|Control::Target(v) if !inside(*v)=>return Err(fail("safety","Wavelength is outside this head's range")),
            Control::Piezo(v) if !v.is_finite()||!(0. ..=100.).contains(v)=>return Err(fail("safety","Piezo must be 0–100 percent")),
            Control::ScanStart(p) if !inside(p.start_nm)||!inside(p.stop_nm)||(p.start_nm-p.stop_nm).abs()<0.009999||
                !p.speed_nm_s.is_finite()||p.speed_nm_s<0.01||p.speed_nm_s>speed_cap.unwrap_or(0.)||p.return_speed_nm_s.is_some_and(|v|!v.is_finite()||v<0.01||v>speed_cap.unwrap_or(0.))=>
                return Err(fail("safety","Scan wavelengths or speed are outside this head's limits")),_=>{}
        }
        if !stopping && !self.switch(key,"*OPC?")? {return Err(fail("safety","Controller is busy"));}
        let remote=self.remote(key)?;
        let tracking=if matches!(&control,Control::Wavelength(_)){self.switch(key,"OUTP:TRAC?")?}else{true};
        let scan_max=if let Control::ScanStart(p)=&control {
            let actual=self.number(key,"SOUR:WAVE:MAXVEL?",Some(0.01),None)?;
            if p.speed_nm_s>actual||p.return_speed_nm_s.is_some_and(|v|v>actual) {return Err(fail("safety","Scan speed exceeds the controller's maximum"));}
            Some((p.return_speed_nm_s.unwrap_or(actual.min(speed_cap.unwrap())),actual.min(speed_cap.unwrap())))
        } else {None};
        if !remote {self.command(key,"SYST:MCONT REM")?;}
        match control {
            Control::Wavelength(v)=>{if !tracking {self.command(key,"OUTP:TRAC 1")?;}self.command(key,&format!("SOUR:WAVE {v}"))?;},
            Control::Target(v)=>self.command(key,&format!("SOUR:WAVE {v}"))?,
            Control::Piezo(v)=>self.command(key,&format!("SOUR:VOLT:PIEZ {v}"))?,
            Control::Tracking(v)=>self.command(key,&format!("OUTP:TRAC {}",u8::from(v)))?,
            Control::Output(v)=>self.command(key,&format!("OUTP:STAT {}",u8::from(v)))?,
            Control::ScanStop=>self.command(key,"OUTP:SCAN:STOP")?,
            Control::ScanStart(p)=>{
                let (return_speed,cap)=scan_max.unwrap();
                self.command(key,&format!("SOUR:WAVE:START {}",p.start_nm))?;
                self.command(key,&format!("SOUR:WAVE:STOP {}",p.stop_nm))?;
                self.command(key,&format!("SOUR:WAVE:SLEW:FORW {}",p.speed_nm_s))?;
                self.command(key,&format!("SOUR:WAVE:SLEW:RET {}",return_speed.to_string()))?;
                self.command(key,"SOUR:WAVE:DESSCANS 1")?;
                let start=self.number(key,"SOUR:WAVE:START?",None,None)?;
                let stop=self.number(key,"SOUR:WAVE:STOP?",None,None)?;
                let speed=self.number(key,"SOUR:WAVE:SLEW:FORW?",Some(0.01),Some(cap))?;
                let ret=self.number(key,"SOUR:WAVE:SLEW:RET?",Some(0.01),Some(cap))?;
                let cycles=self.number(key,"SOUR:WAVE:DESSCANS?",Some(1.),Some(9999.))?;
                if !inside(start)||!inside(stop)||(start-p.start_nm).abs()>0.005001||(stop-p.stop_nm).abs()>0.005001||
                    (start-stop).abs()<0.009999||(speed-p.speed_nm_s).abs()>0.000001||(ret-return_speed).abs()>0.000001||cycles!=1. {
                    return Err(self.invalid(key,"Scan setting verification failed; scanning was not started".into()));
                }
                self.command(key,"OUTP:SCAN:START")?;
            },
        }
        // Do not wait for physical motor completion. Return the front panel after ACK.
        self.command(key,"SYST:MCONT LOC")
    }
    pub fn disconnect(&mut self,key:&str)->Result<()> {
        // A failed connect can retain a global SDK without publishing a controller session.
        if !self.lasers.contains_key(key) {
            if self.retained && self.lasers.is_empty(){return self.release_sdk();}
            return Ok(());
        }
        if self.lasers.len()==1 {self.release_sdk()?;}
        self.lasers.remove(key);Ok(())
    }
    pub fn close_all(&mut self)->Result<()> {
        if self.opened || self.retained {self.release_sdk()?;}
        self.lasers.clear();Ok(())
    }
}
