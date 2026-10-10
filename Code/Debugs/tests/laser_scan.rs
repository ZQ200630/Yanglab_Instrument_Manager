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
 // Old code called this a successful hold although every read says Tracking On.
 assert!(report["error"].is_string());assert_eq!(report["observation"]["hold_observed"],false);
 let trajectory=report["observation"]["trajectory"].as_array().unwrap();assert!(trajectory.len()>=3);
}

#[test] fn endpoint_before_first_sample_never_proves_requested_rate() {
 let (mut p,_root,s,mut b)=fixture("action");p["probe"]["stop_after_s"]=json!(0.5);
 let plan=parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap();
 let r=run_with(&plan,Some("action"),&mut b,&ManualClock::default()).unwrap();
 assert!(r["error"].is_string());assert_eq!(r["observation"]["rate_verified"],false);
 assert_eq!(s.lock().unwrap().commands.iter().filter(|c|*c=="OUTP:SCAN:RESET").count(),1);
}

struct Trajectory {
 clock:Arc<ManualClock>,origin:f64,target:f64,programmed:f64,requested:f64,actual:f64,
 started:Option<f64>,held:Option<f64>,tracking:bool,stuck_tracking:bool,
 output_changed:bool,blanking_changed:bool,ignore_stop:bool,read_delay_ms:u64,commands:Vec<String>,
}
impl Trajectory {
 fn position(&self)->f64 {
  if let Some(v)=self.held{return v;}
  let distance=(self.clock.now().as_secs_f64()-self.started.unwrap_or(self.clock.now().as_secs_f64())).max(0.)*self.actual;
  self.origin+(self.target-self.origin).signum()*distance.min((self.target-self.origin).abs())
 }
}
struct TrajectoryWire(Arc<Mutex<Trajectory>>);
impl Wire for TrajectoryWire {
 fn open(&mut self)->Result<Vec<String>>{Ok(vec!["6700 SN1012".into()])}
 fn close(&mut self)->Result<()>{Ok(())}
 fn query(&mut self,_:&str,c:&str)->Result<String>{
  let mut s=self.0.lock().unwrap();s.commands.push(c.into());
  s.clock.wait(std::time::Duration::from_millis(s.read_delay_ms));
  if let Some(v)=c.strip_prefix("SOUR:WAVE:START "){s.programmed=v.parse().unwrap();return Ok("OK".into());}
  if c=="OUTP:SCAN:RESET"{s.started=Some(s.clock.now().as_secs_f64());s.tracking=true;return Ok("OK".into());}
  if c=="OUTP:SCAN:STOP"{if !s.ignore_stop{s.held=Some(s.position());}return Ok("OK".into());}
  if c=="OUTP:TRAC 0"{if !s.stuck_tracking&&!s.ignore_stop{s.tracking=false;}return Ok("OK".into());}
  Ok(match c {
   "*IDN?"=>"New_Focus 6700 v2.4 03/19/14 SN1012".into(),"SYST:LAS:MODEL?"=>"6722-P".into(),"SYST:LAS:SN?"=>"P1001".into(),"SYST:MCONT?"=>"LOC".into(),
   "SENS:WAVE"=>s.position().to_string(),"SOUR:WAVE?"=>s.origin.to_string(),
   "SOUR:WAVE:MAXVEL?"=>"10".into(),"SOUR:WAVE:START?"=>s.programmed.to_string(),"SOUR:WAVE:STOP?"=>"1061".into(),
   "SOUR:WAVE:SLEW:FORW?"|"SOUR:WAVE:SLEW:RET?"=>s.requested.to_string(),
   "SOUR:WAVE:DESSCANS?"=>"1".into(),"SOUR:VOLT:PIEZ?"=>"50".into(),
   "*OPC?"=>u8::from(s.started.is_none()||s.held.is_some()||(s.position()-s.target).abs()<1e-8).to_string(),
   "OUTP:TRAC?"=>u8::from(s.tracking&&!(s.ignore_stop&&(s.position()-s.target).abs()<1e-8)).to_string(),
   "OUTP:STAT?"=>u8::from(s.started.is_none()||!s.output_changed).to_string(),
   "SOUR:WAVE:SCANCFG?"=>u8::from(s.started.is_some()&&s.blanking_changed).to_string(),
   v if v.contains(' ')=>"OK".into(),_=>"0".into(),
  })
 }
}
fn dynamic_probe(direction:f64,rate:f64,stop:bool,actual:f64,delay_ms:u64)->(Value,PathBuf,Arc<Mutex<Trajectory>>,Bus<TrajectoryWire>,Arc<ManualClock>){
 let (mut p,root,_,_)=fixture("action");let origin=1060.25;let target=origin+direction*0.24;
 p["probe"]=json!({"origin_nm":origin,"target_nm":target,"speed_nm_s":rate,"stop_after_s":if stop {Some(1.8)}else{None}});
 let clock=Arc::new(ManualClock::default());
 let s=Arc::new(Mutex::new(Trajectory{clock:clock.clone(),origin,target,programmed:target,requested:rate,actual,started:None,held:None,tracking:false,stuck_tracking:false,output_changed:false,blanking_changed:false,ignore_stop:false,read_delay_ms:delay_ms,commands:vec![]}));
 (p,root,s.clone(),Bus::new(TrajectoryWire(s)),clock)
}
#[test] fn both_endpoint_directions_and_two_rates_have_actual_rate_and_tracking_off_hold_evidence(){
 for direction in [-1.,1.] {for rate in [0.05,0.10] {
  let (p,_,s,mut b,clock)=dynamic_probe(direction,rate,false,rate,10);
  let r=run_with(&parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap(),Some("action"),&mut b,clock.as_ref()).unwrap();
  assert_eq!(r["error"],Value::Null,"{r}");assert_eq!(r["schema"],2);assert_eq!(r["observation"]["rate_verified"],true);assert_eq!(r["observation"]["hold_observed"],true);
  assert_eq!(r["observation"]["arrival_observed"],true);assert_eq!(r["observation"]["output_preserved"],true);assert_eq!(r["observation"]["configuration_preserved"],true);
  let s=s.lock().unwrap();assert_eq!(s.commands.iter().filter(|c|*c=="OUTP:SCAN:RESET").count(),1);assert_eq!(s.commands.iter().filter(|c|*c=="OUTP:SCAN:STOP").count(),1);
  assert!(!s.commands.iter().any(|c|c.starts_with("OUTP:STAT ")||c.starts_with("SOUR:WAVE:SCANCFG ")));
 }}
}
#[test] fn authorized_midway_stop_proves_pre_stop_rate_and_holds_without_returning(){
 for direction in [-1.,1.] {
  let (p,_,_,mut b,clock)=dynamic_probe(direction,0.1,true,0.1,10);
  let r=run_with(&parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap(),Some("action"),&mut b,clock.as_ref()).unwrap();
  assert_eq!(r["error"],Value::Null,"{r}");assert_eq!(r["observation"]["rate_verified"],true);assert_eq!(r["observation"]["arrival_observed"],false);assert_eq!(r["observation"]["stop_command_sent"],true);
 }
}
#[test] fn wrong_rate_fast_reset_and_ambiguous_slow_getters_cannot_qualify(){
 for (actual,delay) in [(0.01,10),(0.2,10),(10.,10),(0.1,1100)] {
  let (p,_,_,mut b,clock)=dynamic_probe(1.,0.1,false,actual,delay);
  let r=run_with(&parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap(),Some("action"),&mut b,clock.as_ref()).unwrap();
  assert!(r["error"].is_string(),"{r}");assert_eq!(r["observation"]["rate_verified"],false,"{r}");
 }
}
#[test] fn tracking_on_and_changed_output_or_blanking_reject_qualification(){
 for failure in ["tracking","output","blanking"] {
  let (p,_,s,mut b,clock)=dynamic_probe(1.,0.1,false,0.1,10);
  {let mut s=s.lock().unwrap();s.stuck_tracking=failure=="tracking";s.output_changed=failure=="output";s.blanking_changed=failure=="blanking";}
  let r=run_with(&parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap(),Some("action"),&mut b,clock.as_ref()).unwrap();
  assert!(r["error"].is_string(),"{r}");assert_eq!(r["production_capability_changed"],false);
 }
}

fn review_reports()->(Vec<PathBuf>,Vec<Value>){
 let mut paths=vec![];let mut reports=vec![];
 for (direction,rate,stop) in [(-1.,0.05,false),(1.,0.05,false),(-1.,0.1,false),(1.,0.1,false),(-1.,0.1,true),(1.,0.1,true)] {
  let (p,root,_,mut b,clock)=dynamic_probe(direction,rate,stop,rate,10);
  let mut r=run_with(&parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap(),Some("action"),&mut b,clock.as_ref()).unwrap();assert_eq!(r["error"],Value::Null);
  // Development-only file review input: relabelled fixture files are never
  // installed or treated as actual hardware qualification in this test.
  r["source_kind"]=json!("real");r["sdk_sha256"]=json!("a".repeat(64));
  let path=root.join("Result/review.json");std::fs::write(&path,serde_json::to_vec(&r).unwrap()).unwrap();paths.push(path);reports.push(r);
 }
 (paths,reports)
}
#[test] fn six_unique_matching_reports_make_only_an_offline_candidate(){
 let (paths,_)=review_reports();let candidate=yang_debug::laser_scan::review_candidate(&paths).unwrap();
 assert_eq!(candidate["schema"],1);assert_eq!(candidate["records"][0]["rates_nm_s"],json!([0.05,0.1]));assert_eq!(candidate["records"][0]["protocol_revision"],1);
 assert_eq!(candidate["records"][0]["sdk_sha256"],"a".repeat(64));assert_eq!(candidate["records"][0]["evidence_sha256"].as_array().unwrap().len(),6);
 for path in &paths {assert!(!path.parent().unwrap().join("profile.json").exists());}
}
#[test] fn review_rejects_fixture_duplicate_missing_case_identity_and_sdk_changes(){
 let (paths,reports)=review_reports();assert!(yang_debug::laser_scan::review_candidate(&paths[..5]).is_err());
 let mut repeated=paths.clone();repeated[5]=repeated[4].clone();assert!(yang_debug::laser_scan::review_candidate(&repeated).is_err());
 for (field,value) in [("source_kind",json!("bounded_transport_test")),("sdk_sha256",json!("b".repeat(64))), ("sdk_sha256",json!("A".repeat(64))), ("error",json!("failed")),("resources_released",json!(false))] {
  let mut changed=reports[0].clone();changed[field]=value;
  std::fs::write(&paths[0],serde_json::to_vec(&changed).unwrap()).unwrap();assert!(yang_debug::laser_scan::review_candidate(&paths).is_err(),"{field}");
 }
 let mut changed=reports[0].clone();changed["observation"]["identity"]["firmware"]=json!("2.5");std::fs::write(&paths[0],serde_json::to_vec(&changed).unwrap()).unwrap();assert!(yang_debug::laser_scan::review_candidate(&paths).is_err());
}
#[test] fn review_cli_reads_six_reports_without_sdk_and_writes_only_a_new_candidate(){
 let (paths,_)=review_reports();
 let candidate=paths[0].parent().unwrap().join("candidate/profile.json");
 let output=std::process::Command::new(env!("CARGO_BIN_EXE_yang-laser-scan")).arg("--review").args(&paths).arg("--candidate").arg(&candidate).output().unwrap();
 assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
 let shown:Value=serde_json::from_slice(&output.stdout).unwrap();let saved:Value=serde_json::from_slice(&std::fs::read(&candidate).unwrap()).unwrap();assert_eq!(shown,saved);assert_eq!(saved["schema"],1);
 let repeated=std::process::Command::new(env!("CARGO_BIN_EXE_yang-laser-scan")).arg("--review").args(&paths).arg("--candidate").arg(&candidate).output().unwrap();assert!(!repeated.status.success());
 assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&candidate).unwrap()).unwrap(),saved);
}
#[test] fn invalid_review_input_never_creates_a_candidate(){
 let (paths,reports)=review_reports();let candidate=paths[0].parent().unwrap().join("bad/profile.json");
 let mut invalid=reports[0].clone();invalid["source_kind"]=json!("bounded_transport_test");std::fs::write(&paths[0],serde_json::to_vec(&invalid).unwrap()).unwrap();
 let output=std::process::Command::new(env!("CARGO_BIN_EXE_yang-laser-scan")).arg("--review").args(&paths).arg("--candidate").arg(&candidate).output().unwrap();assert!(!output.status.success());assert!(!candidate.parent().unwrap().exists());
}
#[test] fn candidate_accepts_coarse_endpoint_readback_but_rejects_final_bounds_loss(){
 let (paths,reports)=review_reports();let mut r=reports[0].clone();let target=r["plan"]["probe"]["target_nm"].as_f64().unwrap();
 for sample in r["observation"]["trajectory"].as_array_mut().unwrap(){
  if sample["phase"]=="holding"||sample["motion"]["operation_complete"]==true{sample["motion"]["wavelength_nm"]=json!(target+0.014);}
 }
 r["observation"]["after"]["wavelength_nm"]=json!(target+0.014);
 std::fs::write(&paths[0],serde_json::to_vec(&r).unwrap()).unwrap();assert!(yang_debug::laser_scan::review_candidate(&paths).is_ok());
 r["observation"]["after"]["wavelength_nm"]=json!(1100.);
 std::fs::write(&paths[0],serde_json::to_vec(&r).unwrap()).unwrap();assert!(yang_debug::laser_scan::review_candidate(&paths).is_err());
}
#[test] fn review_recomputes_raw_trajectory_and_preservation_instead_of_trusting_success_flags(){
 let (paths,reports)=review_reports();
 for change in ["rate","tracking","output","blanking","time","endpoint","cleanup"] {
  let mut r=reports[0].clone();
  match change {
   "rate"=>{for s in r["observation"]["trajectory"].as_array_mut().unwrap(){s["motion"]["wavelength_nm"]=json!(1060.25);}},
   "tracking"=>{for s in r["observation"]["trajectory"].as_array_mut().unwrap(){if s["phase"]=="holding"{s["motion"]["tracking"]=json!(true);}}},
   "output"=>r["observation"]["after"]["output_enabled"]=json!(false),
   "blanking"=>r["observation"]["scan_settings_after"]["scan_configuration"]=json!(1),
   "time"=>r["observation"]["trajectory"][2]["read_start_s"]=json!(-1.),
   "endpoint"=>r["plan"]["probe"]["target_nm"]=json!(1060.03),
   "cleanup"=>r["cleanup"]["resources_released"]=json!(false),_=>unreachable!(),
  }
  std::fs::write(&paths[0],serde_json::to_vec(&r).unwrap()).unwrap();assert!(yang_debug::laser_scan::review_candidate(&paths).is_err(),"{change}");
 }
}

#[test] fn ignored_midway_stop_cannot_qualify_from_a_later_natural_endpoint_hold(){
 for direction in [-1.,1.] {
  let (p,_,s,mut b,clock)=dynamic_probe(direction,0.1,true,0.1,10);
  s.lock().unwrap().ignore_stop=true;
  let r=run_with(&parse_plan(&serde_json::to_vec(&p).unwrap()).unwrap(),Some("action"),&mut b,clock.as_ref()).unwrap();
  assert!(r["error"].is_string(),"Ignored Stop must not qualify: {r}");
  assert_eq!(r["observation"]["hold_observed"],true);
  assert_eq!(s.lock().unwrap().commands.iter().filter(|c|*c=="OUTP:SCAN:STOP").count(),1);
 }
}
#[test] fn candidate_rejects_interrupt_reports_that_hold_at_the_endpoint(){
 let (paths,reports)=review_reports();let mut r=reports[5].clone();
 let target=r["plan"]["probe"]["target_nm"].clone();
 for sample in r["observation"]["trajectory"].as_array_mut().unwrap(){
  if sample["phase"]=="holding" {sample["motion"]["wavelength_nm"]=target.clone();}
 }
 r["observation"]["after"]["wavelength_nm"]=target;
 std::fs::write(&paths[5],serde_json::to_vec(&r).unwrap()).unwrap();
 assert!(yang_debug::laser_scan::review_candidate(&paths).is_err());
}
#[test] fn candidate_rejects_fast_step_then_stall_hidden_by_endpoint_average(){
 let (paths,reports)=review_reports();let mut r=reports[3].clone();
 let origin=r["plan"]["probe"]["origin_nm"].as_f64().unwrap();
 let original=r["observation"]["trajectory"].as_array().unwrap();
 let mut changed=Vec::new();
 for (start,end,offset) in [(0.30,0.35,0.03),(0.40,0.45,0.12),(1.50,1.55,0.15)] {
  let mut sample=original[0].clone();sample["read_start_s"]=json!(start);sample["read_end_s"]=json!(end);
  sample["motion"]["wavelength_nm"]=json!(origin+offset);sample["motion"]["operation_complete"]=json!(false);sample["motion"]["tracking"]=json!(true);
  changed.push(sample);
 }
 changed.extend(original.iter().filter(|sample|sample["phase"]!="moving"||sample["motion"]["operation_complete"]==true).cloned());
 r["observation"]["trajectory"]=json!(changed);
 std::fs::write(&paths[3],serde_json::to_vec(&r).unwrap()).unwrap();
 assert!(yang_debug::laser_scan::review_candidate(&paths).is_err());
}

#[test] fn review_recomputes_midway_stop_timing_drift_and_final_position(){
 let (paths,reports)=review_reports();
 for case in ["missing_time","late_stop","slow_stop","late_hold","excess_drift","inconsistent_final"] {
  let mut r=reports[5].clone();
  let start=r["observation"]["stop_start_s"].as_f64().unwrap();
  let end=r["observation"]["stop_end_s"].as_f64().unwrap();
  match case {
   "missing_time"=>{r["observation"].as_object_mut().unwrap().remove("stop_start_s");},
   "late_stop"=>r["observation"]["stop_start_s"]=json!(start+0.6),
   "slow_stop"=>r["observation"]["stop_end_s"]=json!(start+2.1),
   "late_hold"=>{for sample in r["observation"]["trajectory"].as_array_mut().unwrap(){if sample["phase"]!="moving"{sample["read_start_s"]=json!(sample["read_start_s"].as_f64().unwrap()+2.1);sample["read_end_s"]=json!(sample["read_end_s"].as_f64().unwrap()+2.1);}}},
   "excess_drift"=>{let prior=r["observation"]["trajectory"].as_array().unwrap().iter().rev().find(|sample|sample["phase"]=="moving").unwrap()["motion"]["wavelength_nm"].as_f64().unwrap();for sample in r["observation"]["trajectory"].as_array_mut().unwrap(){if sample["phase"]!="moving"{sample["motion"]["wavelength_nm"]=json!(prior-0.07);}}r["observation"]["after"]["wavelength_nm"]=json!(prior-0.07);},
   "inconsistent_final"=>r["observation"]["after"]["wavelength_nm"]=r["plan"]["probe"]["target_nm"].clone(),
   _=>unreachable!(),
  }
  assert!(end>=start);
  std::fs::write(&paths[5],serde_json::to_vec(&r).unwrap()).unwrap();
  assert!(yang_debug::laser_scan::review_candidate(&paths).is_err(),"{case}");
 }
}
