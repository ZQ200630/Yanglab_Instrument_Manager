use serde_json::{json,Value};
use std::{sync::{Arc,Mutex},path::PathBuf};
use yang_drivers::clock::{ManualClock,Clock};
use yang_debug::laser_scan::{parse_plan,run_with};
use yang_lab_tlb::{Bus,Wire,Result};
#[derive(Default)] struct State {opens:usize,closes:usize,commands:Vec<String>,slow:Option<Arc<ManualClock>>}
struct Script(Arc<Mutex<State>>);
impl Wire for Script {
 fn open(&mut self)->Result<Vec<String>> {self.0.lock().unwrap().opens+=1;Ok(vec!["6700 SN1012".into()])}
 fn close(&mut self)->Result<()> {self.0.lock().unwrap().closes+=1;Ok(())}
 fn query(&mut self,_key:&str,c:&str)->Result<String> {
  self.0.lock().unwrap().commands.push(c.into());
  if c=="SENS:WAVE" {if let Some(clock)=self.0.lock().unwrap().slow.clone(){clock.wait(std::time::Duration::from_millis(1100));}}
  Ok(match c {"*IDN?"=>"New_Focus 6700 v2.4 03/19/14 SN1012","SYST:LAS:MODEL?"=>"6722-P","SYST:LAS:SN?"=>"P1001","SYST:MCONT?"=>"LOC","SENS:WAVE"=>"1060.01","SOUR:WAVE?"=>"1060","SOUR:WAVE:MAXVEL?"=>"10","SOUR:WAVE:START?"=>"1060.25","SOUR:WAVE:STOP?"=>"1061","SOUR:WAVE:SLEW:FORW?"|"SOUR:WAVE:SLEW:RET?"=>"0.05","SOUR:WAVE:DESSCANS?"|"*OPC?"|"OUTP:TRAC?"=>"1","SOUR:VOLT:PIEZ?"=>"50","OUTP:SCAN:STOP"|"OUTP:SCAN:RESET"=>"OK",v if v.contains(' ')=>"OK",_=>"0"}.into())
 }
}
fn fixture(stage:&str)->(Value,PathBuf,Arc<Mutex<State>>,Bus<Script>) {
 let root=std::env::temp_dir().join(format!("yang-laser-probe-{}",yang_worker::new_id().unwrap()));std::fs::create_dir_all(root.join("Result")).unwrap();
 let mut p=json!({"stage":stage,"output":root.join("Result/probe")});
 if stage!="enumerate" {
  p["identity"]=json!({"manufacturer":"New Focus","model":"TLB-6700","serial":"1012","firmware":"2.4","head_model":"6722-P","head_serial":"P1001"});
  p["limits"]=json!({"min_nm":1045,"max_nm":1085,"max_speed_nm_s":10});
 }
 if stage=="action" {p["probe"]=json!({"origin_nm":1060.01,"target_nm":1060.25,"speed_nm_s":0.05,"stop_after_s":null});}
 let state=Arc::new(Mutex::new(State::default()));let bus=Bus::new(Script(state.clone()));(p,root,state,bus)
}
#[test] fn preview_and_wrong_stage_confirmation_open_nothing() {
 let (p,root,s,mut b)=fixture("action");let plan=parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap();
 let preview=run_with(&plan,None,&mut b,&ManualClock::default()).unwrap();assert_eq!(preview["preview_only"],true);assert_eq!(s.lock().unwrap().opens,0);assert!(!root.join("Result/probe").exists());
 assert!(run_with(&plan,Some("readonly"),&mut b,&ManualClock::default()).is_err());assert_eq!(s.lock().unwrap().opens,0);
}
#[test] fn unknown_fields_and_invalid_probe_are_rejected_before_open() {
 let (mut p,_root,s,_b)=fixture("action");p["probe"]["override_capability"]=json!(true);assert!(parse_plan(&serde_json::to_vec(&p).unwrap()).is_err());
 p["probe"].as_object_mut().unwrap().remove("override_capability");p["probe"]["target_nm"]=json!(1062);assert!(parse_plan(&serde_json::to_vec(&p).unwrap()).is_err());assert_eq!(s.lock().unwrap().opens,0);
}
#[test] fn enumeration_and_readonly_have_separate_preserving_cleanup() {
 for stage in ["enumerate","readonly"] {
  let (p,root,s,mut b)=fixture(stage);let plan=parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap();let r=run_with(&plan,Some(stage),&mut b,&ManualClock::default()).unwrap();
  assert_eq!(r["resources_released"],true);assert_eq!(r["source_kind"],"bounded_transport_test");assert!(root.join("Result/probe/report.json").is_file());
  let state=s.lock().unwrap();assert_eq!(state.opens,1);assert_eq!(state.closes,1);assert!(state.commands.iter().all(|c|!c.contains(' ')));
  if stage=="enumerate" {assert!(state.commands.is_empty());}else {assert!(state.commands.iter().any(|c|c=="SOUR:WAVE:SCANCFG?"));}
 }
}
#[test] fn probe_timeout_attempts_one_authorized_stop_without_claiming_arrival_or_support() {
 let (p,_root,s,mut b)=fixture("action");let plan=parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap();let r=run_with(&plan,Some("action"),&mut b,&ManualClock::default()).unwrap();
 assert_eq!(r["resources_released"],true);assert_eq!(r["production_capability_changed"],false);assert_eq!(r["observation"]["arrival_observed"],false);assert!(r["error"].is_string());
 let commands=&s.lock().unwrap().commands;assert_eq!(commands.iter().filter(|c|*c=="OUTP:SCAN:RESET").count(),1);assert_eq!(commands.iter().filter(|c|*c=="OUTP:SCAN:STOP").count(),1);assert!(!commands.iter().any(|c|c.starts_with("OUTP:STAT ")||c.starts_with("SOUR:WAVE:SCANCFG ")));
}
#[test] fn slow_post_stop_read_still_requires_two_samples_spanning_a_full_hold_second() {
 let (mut p,_root,s,mut b)=fixture("action");p["probe"]["stop_after_s"]=json!(0.5);
 let clock=Arc::new(ManualClock::default());s.lock().unwrap().slow=Some(clock.clone());
 let plan=parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap();let report=run_with(&plan,Some("action"),&mut b,clock.as_ref()).unwrap();
 assert_eq!(report["error"],Value::Null);assert_eq!(report["observation"]["hold_observed"],true);
 let trajectory=report["observation"]["trajectory"].as_array().unwrap();assert!(trajectory.len()>=3);
}
