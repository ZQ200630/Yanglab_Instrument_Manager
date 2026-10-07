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
#[derive(Clone,Debug,Deserialize)]
#[serde(tag="name",content="value",rename_all="snake_case",deny_unknown_fields)]
pub enum Action { Remote(bool),Wavelength(f64),Piezo(f64),Tracking(bool),Output(bool) }
pub mod sdk;
pub mod rpc;

pub fn valid_key(key: &str) -> bool {
    key.strip_prefix("6700 SN").is_some_and(|s| !s.is_empty() && s.len() <= 16 && s.bytes().all(|b| b.is_ascii_digit()))
}
pub fn wavelength_range(head: &str) -> Option<(f64, f64)> {
    match head.strip_prefix("TLB-").unwrap_or(head) {
        "6712" => Some((765.,781.)), "6721" => Some((1030.,1070.)),
        "6722" => Some((1045.,1085.)), "6724" => Some((1270.,1330.)),
        "6728" => Some((1520.,1570.)), "6730" => Some((1550.,1630.)), _ => None,
    }
}
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
struct Laser { identity:Identity, fault:bool }
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
        match result {Ok(id)=>{self.lasers.insert(key.into(),Laser{identity:id.clone(),fault:false});Ok(id)},Err(e)=>{
            if self.lasers.is_empty(){if let Err(close)=self.release_sdk(){return Err(fail("connection",format!("{}; cleanup retained: {}",e.message,close.message)));}}
            Err(e)
        }}
    }
    fn ready(&self,key:&str)->Result<&Laser> {
        let laser=self.lasers.get(key).ok_or_else(||fail("connection","No session for this controller"))?;
        if laser.fault || self.retained {return Err(fail("connection","Controller fault; disconnect before reconnecting"));}
        Ok(laser)
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
    pub fn action(&mut self,key:&str,action:Action,confirm:bool)->Result<()> {
        let laser=self.ready(key)?;let bounds=wavelength_range(&laser.identity.head_model);
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
