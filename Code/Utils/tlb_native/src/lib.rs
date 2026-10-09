use std::collections::BTreeMap;
use serde::{Serialize,Deserialize};
#[derive(Clone,Debug,Serialize,PartialEq)]
pub struct Error { pub kind: &'static str, pub message: String }
pub type Result<T> = std::result::Result<T,Error>;
fn fail(kind: &'static str, message: impl Into<String>) -> Error { Error {kind,message:message.into()} }
pub trait Wire {
 fn open(&mut self)->Result<Vec<String>>; fn query(&mut self,key:&str,command:&str)->Result<String>; fn close(&mut self)->Result<()>;
 // Only an identity-specific qualified transport may claim RESET's slew/hold
 // contract. The production SDK inherits false; there is no operator override.
 fn qualified_single_scan(&self,_identity:&Identity)->bool {false}
}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Identity { pub manufacturer:String,pub model:String,pub serial:String,pub firmware:String,pub head_model:String,pub head_serial:String }
#[derive(Debug,Serialize)]
pub struct Status { pub wavelength_nm:f64,pub wavelength_setpoint_nm:f64,pub power_mw:f64,pub power_setpoint_mw:f64,pub current_ma:f64,pub current_setpoint_ma:f64,pub piezo_percent:f64,pub output_enabled:bool,pub tracking:bool,pub remote:bool,pub constant_power:bool,pub operation_complete:bool,pub single_scan_supported:bool,pub status_byte:u8,pub read_interval_s:f64 }
#[derive(Debug,Serialize)]
pub struct Motion { pub wavelength_nm:f64,pub wavelength_setpoint_nm:f64,pub tracking:bool,pub operation_complete:bool,pub read_interval_s:f64 }
/// Application coarse-arrival band: two 0.01 nm motorized resolution steps.
/// This is neither optical accuracy nor evidence that Tracking Off completed.
pub const GOTO_ARRIVAL_TOLERANCE_NM:f64=0.020001;
#[derive(Clone,Copy,Debug,Serialize,PartialEq,Eq)]
#[serde(rename_all="snake_case")]
pub enum MoveProgress { Moving,VerifyingHold,Held }
#[derive(Clone,Debug,Deserialize)]
#[serde(tag="name",content="value",rename_all="snake_case",deny_unknown_fields)]
pub enum Action { Remote(bool),Wavelength(f64),Piezo(f64),Tracking(bool),Output(bool) }
#[derive(Clone,Debug,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanPlan { pub start_nm:f64, pub stop_nm:f64, pub speed_nm_s:f64, pub return_speed_nm_s:Option<f64> }
#[derive(Clone,Debug,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SingleScanPlan { pub target_nm:f64, pub speed_nm_s:f64 }
#[derive(Clone,Copy,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlLimits { pub min_nm:f64,pub max_nm:f64,pub max_speed_nm_s:f64 }
#[derive(Clone,Debug,Deserialize)]
#[serde(tag="name",content="value",rename_all="snake_case",deny_unknown_fields)]
pub enum Control { Target(f64),Wavelength(f64),Piezo(f64),Tracking(bool),Output(bool),ScanStart(ScanPlan),ScanTo(SingleScanPlan),ScanStop }
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
#[derive(Clone,Copy)]
struct PendingHold { target:f64,check_setpoint:bool }
#[derive(Clone)]
struct Laser { identity:Identity, fault:bool, limits:Option<ControlLimits>,pending_hold:Option<PendingHold> }
/// One SDK lifetime, serialized by the owning executor. Retained release blocks new work.
pub struct Bus<T:Wire> { wire:T, opened:bool, retained:bool, keys:Vec<String>, lasers:BTreeMap<String,Laser> }
impl<T:Wire> Bus<T> {
    pub fn new(wire:T)->Self {Self {wire,opened:false,retained:false,keys:Vec::new(),lasers:BTreeMap::new()}}
    pub fn resources_released(&self)->bool { !self.opened && !self.retained && self.lasers.is_empty() }
    /// Cached ownership only; never enters the SDK. A failed first open may own
    /// global cleanup even before a controller session was published.
    pub fn session_responsibility(&self,key:&str)->bool {
        self.lasers.contains_key(key) || (self.retained && self.lasers.is_empty())
    }
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
        match result {Ok(id)=>{self.lasers.insert(key.into(),Laser{identity:id.clone(),fault:false,limits:None,pending_hold:None});Ok(id)},Err(e)=>{
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
        let single_scan_supported=self.wire.qualified_single_scan(&self.ready(key)?.identity);let started=std::time::Instant::now();
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
            piezo_percent,output_enabled,tracking,remote,constant_power,operation_complete,single_scan_supported,status_byte:byte as u8,read_interval_s:started.elapsed().as_secs_f64()})
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
    /// Read scan settings while preserving output and front-panel mode.
    pub fn scan_settings(&mut self,key:&str)->Result<serde_json::Value> {
        self.ready(key)?;
        let start=self.number(key,"SOUR:WAVE:START?",Some(0.),None)?;
        let stop=self.number(key,"SOUR:WAVE:STOP?",Some(0.),None)?;
        let forward=self.number(key,"SOUR:WAVE:SLEW:FORW?",Some(0.01),None)?;
        let backward=self.number(key,"SOUR:WAVE:SLEW:RET?",Some(0.01),None)?;
        let maximum=self.number(key,"SOUR:WAVE:MAXVEL?",Some(0.01),None)?;
        let cycles=self.number(key,"SOUR:WAVE:DESSCANS?",Some(1.),Some(9999.))?;
        let blanking=self.number(key,"SOUR:WAVE:SCANCFG?",Some(0.),Some(255.))?;
        if cycles.fract()!=0.||blanking.fract()!=0. {return Err(self.invalid(key,"Invalid scan count/configuration response".into()));}
        Ok(serde_json::json!({"start_nm":start,"stop_nm":stop,"forward_speed_nm_s":forward,"backward_speed_nm_s":backward,"maximum_speed_nm_s":maximum,"cycles":cycles as u32,"scan_configuration":blanking as u8}))
    }
    pub fn action(&mut self,key:&str,action:Action,confirm:bool)->Result<()> {
        let laser=self.ready(key)?;let bounds=Self::effective_bounds(laser);
        if !confirm {return Err(fail("safety","Explicit operator confirmation is required"));}
        if laser.pending_hold.is_some()&&matches!(&action,Action::Wavelength(_)|Action::Piezo(_)|Action::Tracking(_)) {
            return Err(fail("safety","The owned Tracking-Off hold is still being verified; motion settings cannot change"));
        }
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
        self.control_inner(key,control,confirm,None,false)
    }
    /// Begin a newly authorized Goto or Full Scan. The caller must exclude its
    /// active owned moves. Only this path may stop inherited tracking, verify a
    /// fresh hold and start the requested move in the same serialized exchange.
    pub fn begin_move(&mut self,key:&str,control:Control,confirm:bool)->Result<()> {
        if !matches!(&control,Control::Wavelength(_)|Control::ScanStart(_)) {
            return Err(fail("safety","Only Goto and Full Scan may begin an explicitly owned move"));
        }
        if self.ready(key)?.pending_hold.is_some() {
            return Err(fail("safety","The owned Tracking-Off hold is still being verified; no new move was started"));
        }
        self.control_inner(key,control,confirm,None,true)
    }
    fn owned_motion(&mut self,key:&str,target:f64)->Result<Motion> {
        let bounds=Self::effective_bounds(self.ready(key)?);
        let sample=self.motion(key).map_err(|mut error|{
            error.message=format!("{}; owned target {target:.6} nm",error.message);error
        })?;
        if !bounds.is_some_and(|(a,b)|sample.wavelength_nm>=a&&sample.wavelength_nm<=b) {
            return Err(self.invalid(key,format!("Owned motion is outside operating bounds {bounds:?}: actual {:.6} nm, setpoint {:.6} nm, target {target:.6} nm, Tracking {}, OPC {}",
                sample.wavelength_nm,sample.wavelength_setpoint_nm,sample.tracking,sample.operation_complete)));
        }
        Ok(sample)
    }
    fn verify_owned_hold(&mut self,key:&str,target:f64)->Result<(Motion,MoveProgress)> {
        let held=self.owned_motion(key,target)?;
        let progress=if held.operation_complete&&!held.tracking {
            self.lasers.get_mut(key).unwrap().pending_hold=None;MoveProgress::Held
        } else {MoveProgress::VerifyingHold};
        Ok((held,progress))
    }
    /// Complete an explicitly owned move. Ordinary status/motion getters never
    /// write. Check the endpoint and (for Goto) the owned setpoint in the same
    /// serialized exchange before turning tracking off once. ACK starts a hold
    /// verification lifecycle; later exchanges only read, preserving actual
    /// position independently of endpoint proximity. Hard faults never replay.
    pub fn finish_move(&mut self,key:&str,target:f64,check_setpoint:bool,settled:bool)->Result<(Motion,MoveProgress)> {
        let laser=self.ready(key)?;let bounds=Self::effective_bounds(laser);
        if let Some(pending)=laser.pending_hold {
            if target!=pending.target||check_setpoint!=pending.check_setpoint {
                return Err(fail("safety","Pending Tracking-Off hold belongs to a different owned endpoint; no command was sent"));
            }
            return self.verify_owned_hold(key,target);
        }
        if !target.is_finite()||!bounds.is_some_and(|(a,b)|target>=a&&target<=b) {
            return Err(fail("safety","Owned endpoint is outside the operating range"));
        }
        let sample=self.owned_motion(key,target)?;
        let tolerance=if check_setpoint {GOTO_ARRIVAL_TOLERANCE_NM} else {0.005001};
        let endpoint=|m:&Motion|(m.operation_complete||check_setpoint&&settled)&&
            (m.wavelength_nm-target).abs()<=tolerance&&
            (!check_setpoint||(m.wavelength_setpoint_nm-target).abs()<=0.000001);
        // Tracking Off without our acknowledged hold is external evidence;
        // the owning Worker decides whether the move was interrupted.
        if !endpoint(&sample)||!sample.tracking {return Ok((sample,MoveProgress::Moving));}
        self.command(key,"OUTP:TRAC 0")?;
        self.lasers.get_mut(key).unwrap().pending_hold=Some(PendingHold{target,check_setpoint});
        self.verify_owned_hold(key,target)
    }
    /// Development-only bounded motion probe. Not exposed through App/RPC controls.
    /// An ACK is not qualification evidence and never changes production capability.
    pub fn probe_single_scan(&mut self,key:&str,expected:&Identity,origin_nm:f64,target_nm:f64,speed_nm_s:f64,confirm:bool)->Result<()> {
        let laser=self.ready(key)?;
        let bounds=Self::effective_bounds(laser);
        let head_cap=max_scan_speed(&laser.identity.head_model).unwrap_or(0.);
        let cap=laser.limits.map_or(head_cap,|l|l.max_speed_nm_s.min(head_cap));
        if !confirm || laser.identity!=*expected || self.lasers.len()!=1 ||
            !origin_nm.is_finite() || !target_nm.is_finite() ||
            !(0.099999..=0.250001).contains(&(origin_nm-target_nm).abs()) ||
            ![0.05,0.10].contains(&speed_nm_s) ||
            bounds.is_none_or(|(a,b)|origin_nm<a||origin_nm>b||target_nm<a||target_nm>b) ||
            cap<head_cap || head_cap==0. {
            return Err(fail("safety","Probe requires exact identity, consent, a 0.10–0.25 nm span, 0.05 or 0.10 nm/s and a ceiling covering the head's maximum possible RESET rate"));
        }
        if !self.switch(key,"*OPC?")? {return Err(fail("safety","Controller is busy; no probe was started"));}
        let current=self.number(key,"SENS:WAVE",Some(0.),None)?;
        let actual=self.number(key,"SOUR:WAVE:MAXVEL?",Some(0.01),None)?;
        if (current-origin_nm).abs()>0.005001 || (current-target_nm).abs()>0.250001 || actual>cap {
            return Err(fail("safety","Probe origin changed or possible RESET speed exceeds the existing operating ceiling"));
        }
        self.control_inner(key,Control::ScanTo(SingleScanPlan{target_nm,speed_nm_s}),true,Some(origin_nm),false)
    }
    fn scan_speed_limits(&mut self,key:&str,p:&ScanPlan,speed_cap:f64)->Result<(f64,f64)> {
        let actual=self.number(key,"SOUR:WAVE:MAXVEL?",Some(0.01),None)?;
        if p.speed_nm_s>actual||p.return_speed_nm_s.is_some_and(|v|v>actual) {
            return Err(fail("safety","Scan speed exceeds the controller's maximum"));
        }
        let cap=actual.min(speed_cap);
        Ok((p.return_speed_nm_s.unwrap_or(cap),cap))
    }
    fn control_inner(&mut self,key:&str,control:Control,confirm:bool,probe:Option<f64>,begin:bool)->Result<()> {
        let laser=self.ready(key)?;let head=laser.identity.head_model.clone();let bounds=Self::effective_bounds(laser);
        let speed_cap=max_scan_speed(&head).map(|v|laser.limits.map_or(v,|l|v.min(l.max_speed_nm_s)));
        if !confirm {return Err(fail("safety","Explicit operator confirmation is required"));}
        if laser.pending_hold.is_some()&&!matches!(&control,Control::Output(_)|Control::ScanStop) {
            return Err(fail("safety","The owned Tracking-Off hold is still being verified; use explicit Stop Scan before changing motion settings"));
        }
        if matches!(&control,Control::ScanTo(_))&&probe.is_none()&&!self.wire.qualified_single_scan(&laser.identity) {
            return Err(fail("safety","Single-pass scan speed and stopping behavior are not qualified for this controller and laser head"));
        }
        let stopping=matches!(&control,Control::Output(false)|Control::Tracking(false)|Control::ScanStop);
        if bounds.is_none() && !stopping {return Err(fail("safety","Unknown laser-head control limits"));}
        let inside=|v:f64|v.is_finite()&&bounds.is_some_and(|(a,b)|v>=a&&v<=b);
        match &control {
            Control::Wavelength(v)|Control::Target(v) if !inside(*v)=>return Err(fail("safety","Wavelength is outside this head's range")),
            Control::Piezo(v) if !v.is_finite()||!(0. ..=100.).contains(v)=>return Err(fail("safety","Piezo must be 0–100 percent")),
            Control::ScanStart(p) if !inside(p.start_nm)||!inside(p.stop_nm)||(p.start_nm-p.stop_nm).abs()<0.009999||
                !p.speed_nm_s.is_finite()||p.speed_nm_s<0.01||p.speed_nm_s>speed_cap.unwrap_or(0.)||p.return_speed_nm_s.is_some_and(|v|!v.is_finite()||v<0.01||v>speed_cap.unwrap_or(0.))=>
                return Err(fail("safety","Scan wavelengths or speed are outside this head's limits")),
            Control::ScanTo(p) if !inside(p.target_nm)||!p.speed_nm_s.is_finite()||p.speed_nm_s<0.01||p.speed_nm_s>speed_cap.unwrap_or(0.)=>
                return Err(fail("safety","Single-pass wavelength or speed is outside this head's limits")),_=>{}
        }
        // A newly owned Full Scan must pass the controller maximum check before
        // any preparatory hold setter. Preserve ordinary control's query order.
        let checked_scan_max=if begin {if let Control::ScanStart(p)=&control {
            Some(self.scan_speed_limits(key,p,speed_cap.unwrap())?)
        } else {None}} else {None};
        // Emission is independent of the motor's OPC state. The controller's
        // key, interlock and ONDELAY still govern the physical output.
        if !stopping&&!matches!(&control,Control::Output(_))&&!self.switch(key,"*OPC?")? {
            if !begin||!self.switch(key,"OUTP:TRAC?")? {
                return Err(fail("safety","Controller is moving; use Stop Scan before starting another move"));
            }
            self.control_inner(key,Control::ScanStop,true,None,false)?;
            let held=self.motion(key)?;
            if !held.operation_complete||held.tracking {
                return Err(self.invalid(key,"Preparatory hold could not be verified; the new move was not started".into()));
            }
        }
        if matches!(&control,Control::Tracking(true)) {
            let target=self.number(key,"SOUR:WAVE?",Some(0.),None)?;
            if !inside(target) {return Err(fail("safety","Existing tracking target is outside the operating range"));}
        }
        // These SCPI setpoints work in Local; no panel mode transition is needed.
        // Firmware 2.4 can return Ready after settling. An explicit following
        // edit therefore starts motor tracking for this new target after writing it.
        match &control {
            Control::Target(v)=>{
                self.command(key,&format!("SOUR:WAVE {v}"))?;
                return Ok(());
            },
            Control::Wavelength(v)=>{
                self.command(key,&format!("SOUR:WAVE {v}"))?;
                self.command(key,"OUTP:TRAC 1")?;return Ok(());
            },
            Control::Tracking(v)=>{
                self.command(key,&format!("OUTP:TRAC {}",u8::from(*v)))?;
                return Ok(());
            },
            _=>{}
        }
        let remote=self.remote(key)?;
        if let Control::ScanTo(p)=&control {
            let current=self.number(key,"SENS:WAVE",Some(0.),None)?;
            let target=self.number(key,"SOUR:WAVE?",Some(0.),None)?;
            let actual=self.number(key,"SOUR:WAVE:MAXVEL?",Some(0.01),None)?;
            if !inside(current)||!inside(target)||p.speed_nm_s>actual||
                probe.is_some_and(|origin|(current-origin).abs()>0.005001||(current-p.target_nm).abs()>0.250001||actual>speed_cap.unwrap_or(0.))||
                !self.switch(key,"*OPC?")? {
                return Err(fail("safety","Single-pass origin or speed is outside the limits, or the controller is busy"));
            }
        }
        let scan_max=if let Control::ScanStart(p)=&control {
            Some(match checked_scan_max {Some(limits)=>limits,None=>self.scan_speed_limits(key,p,speed_cap.unwrap())?})
        } else {None};
        if !remote {self.command(key,"SYST:MCONT REM")?;}
        let explicit_stop=matches!(&control,Control::ScanStop);
        match control {
            Control::Wavelength(_)|Control::Target(_)|Control::Tracking(_)=>unreachable!("handled without panel mode changes"),
            Control::Piezo(v)=>self.command(key,&format!("SOUR:VOLT:PIEZ {v}"))?,
            Control::Output(v)=>self.command(key,&format!("OUTP:STAT {}",u8::from(v)))?,
            Control::ScanStop=>{
                self.command(key,"OUTP:SCAN:STOP")?;
                // STOP ends the scan engine; motor tracking can otherwise keep
                // pursuing its previous target and leave OPC busy indefinitely.
                // Holding the position explicitly stops motor tracking. This
                // neither changes emission nor commands a return wavelength.
                self.command(key,"OUTP:TRAC 0")?;
            },
            Control::ScanTo(p)=>{
                // RESET is the documented single move to the programmed Start.
                // Both configured slew rates stay bounded; no cycle START, timed
                // STOP, emission setter, or reverse-blanking change is substituted.
                self.command(key,&format!("SOUR:WAVE:START {}",p.target_nm))?;
                self.command(key,&format!("SOUR:WAVE:SLEW:FORW {}",p.speed_nm_s))?;
                self.command(key,&format!("SOUR:WAVE:SLEW:RET {}",p.speed_nm_s))?;
                let target=self.number(key,"SOUR:WAVE:START?",None,None)?;
                let forward=self.number(key,"SOUR:WAVE:SLEW:FORW?",Some(0.01),speed_cap)?;
                let backward=self.number(key,"SOUR:WAVE:SLEW:RET?",Some(0.01),speed_cap)?;
                if !inside(target)||(target-p.target_nm).abs()>0.005001||
                    (forward-p.speed_nm_s).abs()>0.000001||(backward-p.speed_nm_s).abs()>0.000001 {
                    return Err(self.invalid(key,"Single-pass setting verification failed; motion was not started".into()));
                }
                if let Some(origin)=probe {
                    let current=self.number(key,"SENS:WAVE",Some(0.),None)?;
                    if !inside(current)||(current-origin).abs()>0.005001||
                        (target-origin).abs()>0.250001||(target-current).abs()>0.250001 {
                        return Err(self.invalid(key,"Verified probe span exceeds consent or origin changed; motion was not started".into()));
                    }
                }
                self.command(key,"OUTP:SCAN:RESET")?;
            },
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
        self.command(key,"SYST:MCONT LOC")?;
        if explicit_stop {self.lasers.get_mut(key).unwrap().pending_hold=None;}
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
