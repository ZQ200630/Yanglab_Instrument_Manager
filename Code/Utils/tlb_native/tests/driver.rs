use yang_lab_tlb::*;
use std::{collections::BTreeMap,sync::{Arc,Mutex}};
#[derive(Default)]
struct State { commands:Vec<(String,String)>,opens:usize,closes:usize,close_error:bool,single_scan_qualified:bool,replies:BTreeMap<String,String>,hold_replies:Option<BTreeMap<String,String>>,query_error:Option<String> }
struct Script(Arc<Mutex<State>>);
impl Wire for Script {
 fn qualified_single_scan(&self,id:&Identity)->bool {self.0.lock().unwrap().single_scan_qualified&&id.serial=="1012"&&id.firmware=="2.4"&&id.head_model=="6722-P"&&id.head_serial=="P1001"}
 fn open(&mut self)->Result<Vec<String>> { self.0.lock().unwrap().opens+=1; Ok(vec!["6700 SN1012".into(),"6700 SN1013".into()]) }
 fn query(&mut self,key:&str,command:&str)->Result<String> { let mut s=self.0.lock().unwrap();s.commands.push((key.into(),command.into()));if s.query_error.as_deref()==Some(command){return Err(Error{kind:"connection",message:"injected uncertain exchange".into()});}if command=="*IDN?" { return Ok(format!("New_Focus 6700 v2.4 03/19/14 SN{}",key.strip_prefix("6700 SN").unwrap())); } if let Some(v)=s.replies.get(command){return Ok(v.clone())} if command=="OUTP:TRAC 0" {if let Some(replies)=s.hold_replies.take(){s.replies.extend(replies);}}if command.contains(' '){return Ok("OK".into())} panic!("Unreviewed query {command}") }
 fn close(&mut self)->Result<()> {let mut s=self.0.lock().unwrap();s.closes+=1;if s.close_error{Err(Error{kind:"connection",message:"retained close".into()})}else{Ok(())}}
}
fn fixture(head:&str)->(Bus<Script>,Arc<Mutex<State>>) {let s=Arc::new(Mutex::new(State::default()));s.lock().unwrap().replies=[("SYST:LAS:MODEL?",head),("SYST:LAS:SN?","P1001"),("OUTP:STAT?","0"),("OUTP:TRAC?","1"),("SYST:MCONT?","LOC"),("SOUR:CPOW?","0"),("SENS:WAVE","1060.01"),("SOUR:WAVE?","1060"),("SENS:POW:DIODE","0"),("SOUR:POW:DIODE?","10"),("SENS:CURR:DIODE","0"),("SOUR:CURR:DIODE?","20"),("SOUR:VOLT:PIEZ?","50"),("*OPC?","1"),("*STB?","0")].into_iter().map(|(k,v)|(k.into(),v.into())).collect();(Bus::new(Script(s.clone())),s)}
#[test] fn bounded_diagnostic_probe_does_not_enable_production_single_scan() {
 let (mut b,s)=fixture("6722-P");let id=b.connect("6700 SN1012").unwrap();
 s.lock().unwrap().replies.extend([("OUTP:SCAN:RESET","OK"),("SOUR:WAVE:MAXVEL?","10"),("SOUR:WAVE:START?","1060.25"),("SOUR:WAVE:SLEW:FORW?","0.05"),("SOUR:WAVE:SLEW:RET?","0.05")].into_iter().map(|(k,v)|(k.into(),v.into())));
 b.probe_single_scan("6700 SN1012",&id,1060.01,1060.25,0.05,true).unwrap();
 assert!(!b.status("6700 SN1012").unwrap().single_scan_supported);
 let count=s.lock().unwrap().commands.len();
 assert!(b.control("6700 SN1012",Control::ScanTo(SingleScanPlan{target_nm:1060.25,speed_nm_s:0.05}),true).is_err());
 assert_eq!(s.lock().unwrap().commands.len(),count);
 let writes=s.lock().unwrap().commands.iter().filter(|(_,c)|c.contains(' ')||c=="OUTP:SCAN:RESET").map(|(_,c)|c.clone()).collect::<Vec<_>>();
 assert_eq!(writes,["SYST:MCONT REM","SOUR:WAVE:START 1060.25","SOUR:WAVE:SLEW:FORW 0.05","SOUR:WAVE:SLEW:RET 0.05","OUTP:SCAN:RESET","SYST:MCONT LOC"]);
}
#[test] fn diagnostic_probe_requires_exact_identity_consent_and_small_displacement_before_io() {
 let (mut b,s)=fixture("6722-P");let id=b.connect("6700 SN1012").unwrap();let count=s.lock().unwrap().commands.len();
 let mut wrong=id.clone();wrong.head_serial="OTHER".into();
 for (identity,origin,target,speed,consent) in [(&id,1060.01,1060.25,0.05,false),(&wrong,1060.01,1060.25,0.05,true),(&id,1060.01,1061.,0.05,true),(&id,1060.01,1060.25,1.,true),(&id,f64::NAN,1060.25,0.05,true)] {
  assert!(b.probe_single_scan("6700 SN1012",identity,origin,target,speed,consent).is_err());
 }
 assert_eq!(s.lock().unwrap().commands.len(),count);
}
#[test] fn diagnostic_probe_rejects_changed_origin_and_narrow_speed_ceiling_without_writes() {
 for narrow in [false,true] {
  let (mut b,s)=fixture("6722-P");let id=b.connect("6700 SN1012").unwrap();
  s.lock().unwrap().replies.insert("SOUR:WAVE:MAXVEL?".into(),"10".into());
  if narrow {b.set_limits("6700 SN1012",ControlLimits{min_nm:1060.,max_nm:1061.,max_speed_nm_s:0.5}).unwrap();}
  assert!(b.probe_single_scan("6700 SN1012",&id,if narrow {1060.01}else{1060.1},1060.25,0.05,true).is_err());
  assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')));
 }
}
#[test] fn probe_verified_start_cannot_expand_the_approved_span_in_either_direction() {
 for (origin,target,verified) in [(1060.01,1060.26,"1060.264"),(1060.01,1059.76,"1059.756")] {
  let (mut b,s)=fixture("6722-P");let id=b.connect("6700 SN1012").unwrap();
  s.lock().unwrap().replies.extend([("OUTP:SCAN:RESET","OK"),("SOUR:WAVE:MAXVEL?","10"),("SOUR:WAVE:START?",verified),("SOUR:WAVE:SLEW:FORW?","0.05"),("SOUR:WAVE:SLEW:RET?","0.05")].into_iter().map(|(k,v)|(k.into(),v.into())));
  assert!(b.probe_single_scan("6700 SN1012",&id,origin,target,0.05,true).is_err());
  assert!(!s.lock().unwrap().commands.iter().any(|(_,c)|c=="OUTP:SCAN:RESET"));
 }
}
#[test] fn motion_reads_only_motion_and_does_not_invent_output_evidence() {
 let (mut b,s)=fixture("6722-P");b.connect("6700 SN1012").unwrap();s.lock().unwrap().commands.clear();
 let m=b.motion("6700 SN1012").unwrap();assert!(m.operation_complete);assert_eq!(m.wavelength_nm,1060.01);
 let commands=s.lock().unwrap().commands.iter().map(|(_,c)|c.clone()).collect::<Vec<_>>();
 assert_eq!(commands,["*OPC?","SENS:WAVE","SOUR:WAVE?","OUTP:TRAC?","*OPC?"]);
 s.lock().unwrap().replies.insert("SOUR:WAVE?".into(),"NaN".into());assert!(b.motion("6700 SN1012").is_err());
 assert!(b.control("6700 SN1012",Control::Target(1060.),true).is_err());
}
#[test] fn preserving_identity_and_status(){let(mut b,s)=fixture("6722-P");let id=b.connect("6700 SN1012").unwrap();assert_eq!(id.serial,"1012");assert_eq!(id.head_model,"6722-P");let st=b.status("6700 SN1012").unwrap();assert_eq!(st.wavelength_nm,1060.01);assert!(!st.output_enabled);b.disconnect("6700 SN1012").unwrap();assert!(b.resources_released());assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')));}
#[test] fn shared_bus_duplicate_and_scan_preserve_other_session(){let(mut b,s)=fixture("TLB-6721");b.connect("6700 SN1012").unwrap();assert!(b.connect("6700 SN1012").is_err());b.connect("6700 SN1013").unwrap();assert!(b.enumerate().is_err());b.disconnect("6700 SN1012").unwrap();assert_eq!(s.lock().unwrap().closes,0);assert!(b.status("6700 SN1013").is_ok());b.disconnect("6700 SN1013").unwrap();assert_eq!(s.lock().unwrap().closes,1);assert_eq!(s.lock().unwrap().opens,1);}
#[test] fn failed_last_close_retains_and_can_be_retried(){let(mut b,s)=fixture("6722-P");b.connect("6700 SN1012").unwrap();s.lock().unwrap().close_error=true;assert!(b.disconnect("6700 SN1012").is_err());assert!(!b.resources_released());assert!(b.connect("6700 SN1013").is_err());assert!(b.status("6700 SN1012").is_err());s.lock().unwrap().close_error=false;b.disconnect("6700 SN1012").unwrap();assert!(b.resources_released());}
#[test] fn suffix_head_blocks_control_before_getters(){let(mut b,s)=fixture("6722-CUSTOM");b.connect("6700 SN1012").unwrap();let n=s.lock().unwrap().commands.len();for a in [Action::Remote(true),Action::Wavelength(1060.),Action::Piezo(50.),Action::Tracking(true),Action::Output(true)]{assert_eq!(b.action("6700 SN1012",a,true).unwrap_err().kind,"safety");}assert_eq!(s.lock().unwrap().commands.len(),n);}
#[test] fn fresh_remote_busy_tracking_and_confirmation_gates(){let(mut b,s)=fixture("6722");b.connect("6700 SN1012").unwrap();assert!(b.action("6700 SN1012",Action::Wavelength(1060.),false).is_err());assert!(b.action("6700 SN1012",Action::Wavelength(1060.),true).is_err());s.lock().unwrap().replies.insert("SYST:MCONT?".into(),"REM".into());s.lock().unwrap().replies.insert("OUTP:TRAC?".into(),"0".into());assert!(b.action("6700 SN1012",Action::Wavelength(1060.),true).is_err());s.lock().unwrap().replies.insert("OUTP:TRAC?".into(),"1".into());b.action("6700 SN1012",Action::Wavelength(1060.),true).unwrap();s.lock().unwrap().replies.insert("*OPC?".into(),"0".into());assert!(b.action("6700 SN1012",Action::Output(true),true).is_err());b.action("6700 SN1012",Action::Output(false),true).unwrap();assert!(s.lock().unwrap().commands.iter().any(|(_,c)|c=="OUTP:STAT 0"));}
#[test] fn malformed_numeric_latches_fault_and_never_writes(){let(mut b,s)=fixture("6722");b.connect("6700 SN1012").unwrap();s.lock().unwrap().replies.insert("SOUR:WAVE?".into(),"NaN".into());assert!(b.status("6700 SN1012").is_err());assert!(b.action("6700 SN1012",Action::Output(false),true).is_err());b.disconnect("6700 SN1012").unwrap();assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')));}
#[test] fn rejected_write_is_single_attempt_and_faulted(){let(mut b,s)=fixture("6722");b.connect("6700 SN1012").unwrap();s.lock().unwrap().replies.insert("SYST:MCONT?".into(),"REM".into());s.lock().unwrap().replies.insert("OUTP:TRAC 1".into(),"COMMAND NOT VALID".into());assert!(b.action("6700 SN1012",Action::Tracking(true),true).is_err());assert!(b.action("6700 SN1012",Action::Tracking(true),true).is_err());assert_eq!(s.lock().unwrap().commands.iter().filter(|(_,c)|c=="OUTP:TRAC 1").count(),1);}
#[test] fn invalid_key_and_limit_never_open_or_write(){let(mut b,s)=fixture("6722");assert!(b.connect("USB0").is_err());assert_eq!(s.lock().unwrap().opens,0);b.connect("6700 SN1012").unwrap();let n=s.lock().unwrap().commands.len();for v in [f64::NAN,f64::INFINITY,1044.9,1085.1]{assert!(b.action("6700 SN1012",Action::Wavelength(v),true).is_err());}assert_eq!(s.lock().unwrap().commands.len(),n);}
#[test] fn missing_key_releases_temporary_open(){let(mut b,s)=fixture("6722");assert!(b.connect("6700 SN9999").is_err());assert!(b.resources_released());assert_eq!(s.lock().unwrap().closes,1);}
#[test] fn enumeration_opens_and_releases_without_session(){let(mut b,s)=fixture("6722");assert_eq!(b.enumerate().unwrap().len(),2);assert!(b.resources_released());assert_eq!(s.lock().unwrap().opens,1);assert_eq!(s.lock().unwrap().closes,1);}

#[test] fn discovery_reads_bound_heads_in_one_sdk_lifetime_without_status_or_writes(){
 let(mut b,s)=fixture("6722-P");let heads=b.discover().unwrap();
 assert_eq!(heads.iter().map(|id|id.serial.as_str()).collect::<Vec<_>>(),["1012","1013"]);
 assert!(heads.iter().all(|id|id.head_model=="6722-P" && id.head_serial=="P1001"));
 assert!(b.resources_released());let state=s.lock().unwrap();assert_eq!(state.opens,1);assert_eq!(state.closes,1);
 assert_eq!(state.commands.len(),6);assert!(state.commands.iter().all(|(_,c)|matches!(c.as_str(),"*IDN?"|"SYST:LAS:MODEL?"|"SYST:LAS:SN?")));
}
#[test] fn discovery_cannot_replace_an_active_owner(){let(mut b,s)=fixture("6722-P");b.connect("6700 SN1012").unwrap();let count=s.lock().unwrap().commands.len();assert!(b.discover().is_err());assert_eq!(s.lock().unwrap().commands.len(),count);assert!(!b.resources_released());b.disconnect("6700 SN1012").unwrap();}
#[test] fn discovery_failed_release_retains_responsibility_without_retry(){let(mut b,s)=fixture("6722-P");s.lock().unwrap().close_error=true;assert!(b.discover().is_err());assert!(!b.resources_released());assert_eq!(s.lock().unwrap().closes,1);assert!(b.discover().is_err());assert_eq!(s.lock().unwrap().closes,1);s.lock().unwrap().close_error=false;b.close_all().unwrap();assert!(b.resources_released());}
#[test] fn malformed_discovery_releases_once_and_never_returns_partial_heads(){let(mut b,s)=fixture("INVALID");assert!(b.discover().is_err());assert!(b.resources_released());assert_eq!(s.lock().unwrap().closes,1);}

#[test] fn close_all_failed_release_retains_multiple_sessions_then_releases_once(){let(mut b,s)=fixture("6722");b.connect("6700 SN1012").unwrap();b.connect("6700 SN1013").unwrap();s.lock().unwrap().close_error=true;assert!(b.close_all().is_err());assert!(!b.resources_released());assert!(b.status("6700 SN1012").is_err());assert!(b.status("6700 SN1013").is_err());s.lock().unwrap().close_error=false;b.close_all().unwrap();assert!(b.resources_released());assert_eq!(s.lock().unwrap().closes,2);b.close_all().unwrap();assert_eq!(s.lock().unwrap().closes,2);}

#[test] fn published_fiber_head_limits_are_automatic_and_unknown_suffixes_stay_unknown() {
 assert_eq!(wavelength_range("6722-P"),Some((1045.,1085.)));
 assert_eq!(wavelength_range("TLB-6713-P"),Some((792.,810.)));
 assert_eq!(wavelength_range("6722-CUSTOM"),None);
 assert_eq!(wavelength_range("6722-P-EXT"),None);
 assert_eq!(max_scan_speed("6722-P"),Some(10.));
}
fn plan()->ScanPlan {ScanPlan{start_nm:1060.,stop_nm:1061.,speed_nm_s:1.,return_speed_nm_s:None}}
fn single_plan(target:f64,speed:f64)->Control {
 serde_json::from_value(serde_json::json!({"name":"scan_to","value":{"target_nm":target,"speed_nm_s":speed}})).expect("typed single-pass scan")
}
#[test] fn single_scan_moves_from_fresh_position_to_one_endpoint_without_round_trip() {
 for target in [1059.,1061.] {
  let (mut b,s)=scan_fixture();
  s.lock().unwrap().replies.extend([("SOUR:WAVE:START?".into(),target.to_string()),("SOUR:WAVE:SLEW:FORW?".into(),"0.5".into()),("SOUR:WAVE:SLEW:RET?".into(),"0.5".into()),("OUTP:SCAN:RESET".into(),"OK".into())]);
  b.set_limits("6700 SN1012",ControlLimits{min_nm:1059.,max_nm:1062.,max_speed_nm_s:1.}).unwrap();
  s.lock().unwrap().commands.clear();
  b.control("6700 SN1012",single_plan(target,0.5),true).unwrap();
  let st=s.lock().unwrap();let commands=st.commands.iter().map(|(_,c)|c.as_str()).collect::<Vec<_>>();
  assert_eq!(commands.iter().filter(|c|**c=="OUTP:SCAN:RESET").count(),1);
  assert!(commands.iter().position(|c|*c=="SENS:WAVE").unwrap()<commands.iter().position(|c|c.contains(' ')).unwrap());
  assert!(commands.iter().position(|c|*c=="SOUR:WAVE:SLEW:RET?").unwrap()<commands.iter().position(|c|*c=="OUTP:SCAN:RESET").unwrap());
  assert!(!commands.iter().any(|c|*c=="OUTP:SCAN:START"||*c=="OUTP:SCAN:STOP"||c.starts_with("OUTP:STAT ")||c.starts_with("SOUR:WAVE:SCANCFG")||c.starts_with("SOUR:WAVE:STOP ")));
 }
}
#[test] fn unqualified_single_scan_rejects_before_any_write() {
 let (mut b,s)=scan_fixture();
 s.lock().unwrap().single_scan_qualified=false;
 s.lock().unwrap().replies.extend([("SOUR:WAVE:START?".into(),"1061".into()),("SOUR:WAVE:SLEW:RET?".into(),"1".into()),("OUTP:SCAN:RESET".into(),"OK".into())]);
 s.lock().unwrap().commands.clear();
 assert!(b.control("6700 SN1012",single_plan(1061.,1.),true).is_err());
 assert!(s.lock().unwrap().commands.is_empty());
}
#[test] fn single_scan_rejects_unbounded_origin_speed_and_mismatch_without_motion() {
 for (query,reply) in [("SENS:WAVE","1058"),("SOUR:WAVE?","1063"),("SOUR:WAVE:MAXVEL?","0.1"),("*OPC?","0")] {
  let (mut b,s)=scan_fixture();b.set_limits("6700 SN1012",ControlLimits{min_nm:1059.,max_nm:1062.,max_speed_nm_s:1.}).unwrap();
  s.lock().unwrap().commands.clear();s.lock().unwrap().replies.insert(query.into(),reply.into());
  assert!(b.control("6700 SN1012",single_plan(1061.,0.5),true).is_err());
  assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')&&!c.starts_with("OUTP:SCAN")));
 }
 let (mut b,s)=scan_fixture();s.lock().unwrap().replies.insert("SOUR:WAVE:START?".into(),"1062".into());
 assert!(b.control("6700 SN1012",single_plan(1061.,1.),true).is_err());
 assert!(!s.lock().unwrap().commands.iter().any(|(_,c)|c=="OUTP:SCAN:RESET"||c=="SYST:MCONT LOC"));
}
#[test] fn single_scan_never_replays_uncertain_motion_or_unconfirmed_operator_intent() {
 let (mut b,s)=scan_fixture();s.lock().unwrap().commands.clear();
 assert!(b.control("6700 SN1012",single_plan(1061.,1.),false).is_err());assert!(s.lock().unwrap().commands.is_empty());
 s.lock().unwrap().replies.extend([("SOUR:WAVE:START?".into(),"1061".into()),("SOUR:WAVE:SLEW:RET?".into(),"1".into()),("OUTP:SCAN:RESET".into(),"COMMAND NOT VALID".into())]);
 assert!(b.control("6700 SN1012",single_plan(1061.,1.),true).is_err());
 assert!(b.control("6700 SN1012",single_plan(1061.,1.),true).is_err());
 assert_eq!(s.lock().unwrap().commands.iter().filter(|(_,c)|c=="OUTP:SCAN:RESET").count(),1);
}
fn scan_fixture()->(Bus<Script>,Arc<Mutex<State>>) {
 let (mut b,s)=fixture("6722-P");b.connect("6700 SN1012").unwrap();
 // This finite byte double models a qualified contract; it is not evidence
 // that RESET's physical rate/blanking is qualified on a real controller.
 s.lock().unwrap().single_scan_qualified=true;
 s.lock().unwrap().replies.extend([("SOUR:WAVE:MAXVEL?","10"),("SOUR:WAVE:START?","1060"),("SOUR:WAVE:STOP?","1061"),("SOUR:WAVE:SLEW:FORW?","1"),("SOUR:WAVE:SLEW:RET?","10"),("SOUR:WAVE:DESSCANS?","1"),("OUTP:SCAN:START","OK"),("OUTP:SCAN:STOP","OK")].into_iter().map(|(k,v)|(k.into(),v.into())));
 (b,s)
}
fn inherited_motion_fixture()->(Bus<Script>,Arc<Mutex<State>>) {
 let (b,s)=scan_fixture();let mut state=s.lock().unwrap();
 state.replies.extend([("*OPC?","0"),("OUTP:TRAC?","1"),("SENS:WAVE","1069.324"),("SOUR:WAVE?","1069.310")].into_iter().map(|(k,v)|(k.into(),v.into())));
 state.hold_replies=Some([("*OPC?","1"),("OUTP:TRAC?","0")].into_iter().map(|(k,v)|(k.into(),v.into())).collect());
 state.commands.clear();drop(state);(b,s)
}
#[test] fn begin_goto_holds_inherited_tracking_before_the_new_target() {
 let (mut b,s)=inherited_motion_fixture();
 b.begin_move("6700 SN1012",Control::Wavelength(1060.1),true).unwrap();
 let state=s.lock().unwrap();let commands=state.commands.iter().map(|(_,c)|c.as_str()).collect::<Vec<_>>();
 let stop=commands.iter().position(|c|*c=="OUTP:SCAN:STOP").unwrap();
 let off=commands.iter().position(|c|*c=="OUTP:TRAC 0").unwrap();
 let held=commands.iter().rposition(|c|*c=="OUTP:TRAC?").unwrap();
 let target=commands.iter().position(|c|*c=="SOUR:WAVE 1060.1").unwrap();
 assert!(stop<off&&off<held&&held<target,"fresh hold evidence must precede the new target");
 assert_eq!(commands.iter().filter(|c|**c=="OUTP:SCAN:STOP").count(),1);
 assert_eq!(commands.iter().filter(|c|**c=="OUTP:TRAC 0").count(),1);
 assert_eq!(commands.iter().filter(|c|**c=="SOUR:WAVE 1060.1").count(),1);
 assert!(commands.iter().any(|c|*c=="OUTP:TRAC 1"));
 assert!(!commands.iter().any(|c|c.starts_with("OUTP:STAT ")||c.starts_with("SOUR:WAVE:SCANCFG")||*c=="OUTP:SCAN:RESET"));
}
#[test] fn begin_full_scan_checks_maximum_before_hold_and_then_starts_one_round_trip() {
 let (mut b,s)=inherited_motion_fixture();
 b.begin_move("6700 SN1012",Control::ScanStart(plan()),true).unwrap();
 let state=s.lock().unwrap();let commands=state.commands.iter().map(|(_,c)|c.as_str()).collect::<Vec<_>>();
 let max=commands.iter().position(|c|*c=="SOUR:WAVE:MAXVEL?").unwrap();
 let stop=commands.iter().position(|c|*c=="OUTP:SCAN:STOP").unwrap();
 let held=commands.iter().rposition(|c|*c=="OUTP:TRAC?").unwrap();
 let program=commands.iter().position(|c|*c=="SOUR:WAVE:START 1060").unwrap();
 let verified=commands.iter().position(|c|*c=="SOUR:WAVE:DESSCANS?").unwrap();
 let start=commands.iter().position(|c|*c=="OUTP:SCAN:START").unwrap();
 assert!(max<stop&&stop<held&&held<program&&verified<start);
 assert_eq!(commands.iter().filter(|c|**c=="OUTP:SCAN:START").count(),1);
 assert!(commands.iter().any(|c|*c=="SOUR:WAVE:SLEW:FORW 1"));
 assert!(commands.iter().any(|c|*c=="SOUR:WAVE:SLEW:RET 10"));
 assert!(!commands.iter().any(|c|c.starts_with("OUTP:STAT ")||c.starts_with("SOUR:WAVE:SCANCFG")||*c=="OUTP:SCAN:RESET"));
}
#[test] fn begin_move_invalid_intent_and_fresh_scan_maximum_never_hold_or_write() {
 let (mut b,s)=inherited_motion_fixture();
 for (control,confirm) in [(Control::Wavelength(1060.),false),(Control::Wavelength(1086.),true),(Control::Wavelength(f64::NAN),true),
  (Control::ScanStart(ScanPlan{stop_nm:1086.,..plan()}),true),(Control::ScanStart(ScanPlan{speed_nm_s:11.,..plan()}),true),
  (Control::ScanStart(ScanPlan{return_speed_nm_s:Some(11.),..plan()}),true),(Control::Piezo(50.),true),(single_plan(1061.,1.),true)] {
  assert!(b.begin_move("6700 SN1012",control,confirm).is_err());
 }
 s.lock().unwrap().replies.insert("SOUR:WAVE:MAXVEL?".into(),"0.5".into());
 assert!(b.begin_move("6700 SN1012",Control::ScanStart(plan()),true).is_err());
 assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')&&!c.starts_with("OUTP:SCAN:")));
}
#[test] fn begin_move_never_replaces_busy_motion_without_tracking() {
 let (mut b,s)=inherited_motion_fixture();s.lock().unwrap().replies.insert("OUTP:TRAC?".into(),"0".into());
 for control in [Control::Wavelength(1060.1),Control::ScanStart(plan())] {
  assert_eq!(b.begin_move("6700 SN1012",control,true).unwrap_err().kind,"safety");
 }
 assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')&&!c.starts_with("OUTP:SCAN:")));
}
#[test] fn begin_move_failed_hold_faults_without_start_or_replay() {
 for case in ["stop-ack","tracking-ack","uncertain-stop","busy-hold","tracking-hold","invalid-hold"] {
  let (mut b,s)=inherited_motion_fixture();{
   let mut state=s.lock().unwrap();
   match case {
    "stop-ack"=>{state.replies.insert("OUTP:SCAN:STOP".into(),"COMMAND NOT VALID".into());},
    "tracking-ack"=>{state.replies.insert("OUTP:TRAC 0".into(),"COMMAND NOT VALID".into());},
    "uncertain-stop"=>state.query_error=Some("OUTP:SCAN:STOP".into()),
    "busy-hold"=>{state.hold_replies.as_mut().unwrap().insert("*OPC?".into(),"0".into());},
    "tracking-hold"=>{state.hold_replies.as_mut().unwrap().insert("OUTP:TRAC?".into(),"1".into());},
    "invalid-hold"=>{state.hold_replies.as_mut().unwrap().insert("SENS:WAVE".into(),"NaN".into());},
    _=>unreachable!(),
   }
  }
  let error=b.begin_move("6700 SN1012",Control::Wavelength(1060.1),true).unwrap_err();
  assert_ne!(error.kind,"safety","{case}: a preparatory setter was entered");
  let count=s.lock().unwrap().commands.len();
  assert!(b.begin_move("6700 SN1012",Control::Wavelength(1060.1),true).is_err());
  assert_eq!(s.lock().unwrap().commands.len(),count,"{case}: faulted hold must never replay");
  assert!(!s.lock().unwrap().commands.iter().any(|(_,c)|c.starts_with("SOUR:WAVE ")||c=="OUTP:TRAC 1"||c=="OUTP:SCAN:START"||c.starts_with("OUTP:STAT ")),"{case}");
 }
}
#[test] fn begin_move_ready_controller_needs_no_preparatory_hold_and_direct_control_stays_strict() {
 let (mut b,s)=scan_fixture();s.lock().unwrap().commands.clear();
 b.begin_move("6700 SN1012",Control::Wavelength(1060.1),true).unwrap();
 assert!(!s.lock().unwrap().commands.iter().any(|(_,c)|c=="OUTP:SCAN:STOP"||c=="OUTP:TRAC 0"));
 s.lock().unwrap().commands.clear();s.lock().unwrap().replies.insert("*OPC?".into(),"0".into());
 assert!(b.control("6700 SN1012",Control::Wavelength(1060.2),true).is_err());
 assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')&&!c.starts_with("OUTP:SCAN:")));
}
#[test] fn composite_following_moves_to_new_target_without_mode_changes() {
 let (mut b,s)=scan_fixture();s.lock().unwrap().replies.insert("OUTP:TRAC?".into(),"0".into());
 b.control("6700 SN1012",Control::Wavelength(1060.),true).unwrap();
 let commands=s.lock().unwrap().commands.iter().map(|(_,c)|c.clone()).collect::<Vec<_>>();
 assert_eq!(&commands[3..],["*OPC?","SOUR:WAVE 1060","OUTP:TRAC 1"]);
}
#[test] fn output_enable_disable_is_independent_of_motor_completion() {
 let (mut b,s)=scan_fixture();b.control("6700 SN1012",Control::Tracking(true),true).unwrap();
 s.lock().unwrap().replies.insert("*OPC?".into(),"0".into());s.lock().unwrap().commands.clear();
 b.control("6700 SN1012",Control::Output(false),true).unwrap();
 b.control("6700 SN1012",Control::Output(true),true).unwrap();
 let commands=s.lock().unwrap().commands.clone();let writes=commands.iter().filter(|(_,c)|c.contains(' ')||c.starts_with("OUTP:SCAN:")).map(|(_,c)|c.as_str()).collect::<Vec<_>>();
 assert_eq!(writes,["SYST:MCONT REM","OUTP:STAT 0","SYST:MCONT LOC","SYST:MCONT REM","OUTP:STAT 1","SYST:MCONT LOC"]);
 assert!(!commands.iter().any(|(_,c)|c=="*OPC?"||c.starts_with("OUTP:TRAC ")||c.starts_with("SOUR:WAVE ")));
}
#[test] fn conditional_goto_hold_rechecks_the_endpoint_and_never_replays_a_failed_hold() {
 for case in ["changed","moving","failed-hold"] {
  let (mut b,s)=scan_fixture();s.lock().unwrap().commands.clear();
  s.lock().unwrap().replies.insert("SENS:WAVE".into(),"1060".into());
  if case=="changed" {s.lock().unwrap().replies.insert("SOUR:WAVE?".into(),"1061".into());}
  if case=="moving" {s.lock().unwrap().replies.insert("*OPC?".into(),"0".into());}
  let result=b.finish_move("6700 SN1012",1060.,true,false);
  if case=="failed-hold" {
   assert!(result.is_err(),"an ACK followed by tracking still on cannot report arrival");
   assert!(b.finish_move("6700 SN1012",1060.,true,true).is_err());
   assert_eq!(s.lock().unwrap().commands.iter().filter(|(_,c)|c=="OUTP:TRAC 0").count(),1);
  } else {
   assert!(!result.unwrap().1);assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')));
  }
 }
}
#[test] fn settled_endpoint_with_tracking_off_still_requires_opc_completion() {
 let (mut b,s)=scan_fixture();s.lock().unwrap().commands.clear();
 s.lock().unwrap().replies.extend([("SENS:WAVE","1060"),("SOUR:WAVE?","1060"),("OUTP:TRAC?","0"),("*OPC?","0")].into_iter().map(|(k,v)|(k.into(),v.into())));
 let (sample,held)=b.finish_move("6700 SN1012",1060.,true,true).unwrap();
 assert!(!sample.operation_complete);assert!(!held,"Tracking Off does not prove arrival when OPC is still false");
 assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')),"external tracking-off must not cause an automatic setter");
}
#[test] fn busy_imported_or_scan_motion_and_changed_manual_target_reject_before_writes() {
 for case in ["imported","scan","changed-target","tracking-lost"] {
  let (mut b,s)=scan_fixture();
  if case!="imported" {b.control("6700 SN1012",Control::Tracking(true),true).unwrap();}
  if case=="scan" {b.control("6700 SN1012",Control::ScanStart(plan()),true).unwrap();}
  let mut state=s.lock().unwrap();state.replies.insert("*OPC?".into(),"0".into());state.commands.clear();
  if case=="changed-target" {state.replies.insert("SOUR:WAVE?".into(),"1060.3".into());}
  if case=="tracking-lost" {state.replies.insert("OUTP:TRAC?".into(),"0".into());}drop(state);
  assert!(b.control("6700 SN1012",Control::Wavelength(1060.1),true).is_err(),"{case}");
  assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')&&!c.starts_with("OUTP:SCAN:")),"{case}");
 }
}
#[test] fn native_scan_config_is_verified_before_start_and_never_enables_output() {
 let(mut b,s)=scan_fixture();b.control("6700 SN1012",Control::ScanStart(plan()),true).unwrap();
 let commands=s.lock().unwrap().commands.iter().map(|(_,c)|c.clone()).collect::<Vec<_>>();
 let writes=commands.iter().filter(|c|c.contains(' ')||c.starts_with("OUTP:SCAN")).map(String::as_str).collect::<Vec<_>>();
 assert_eq!(writes,["SYST:MCONT REM","SOUR:WAVE:START 1060","SOUR:WAVE:STOP 1061","SOUR:WAVE:SLEW:FORW 1","SOUR:WAVE:SLEW:RET 10","SOUR:WAVE:DESSCANS 1","OUTP:SCAN:START","SYST:MCONT LOC"]);
 assert!(commands.iter().position(|c|c=="SOUR:WAVE:DESSCANS?").unwrap()<commands.iter().position(|c|c=="OUTP:SCAN:START").unwrap());
 assert!(!commands.iter().any(|c|c.starts_with("OUTP:STAT ")||c.starts_with("SOUR:WAVE:SCANCFG")));
}
#[test] fn invalid_scan_plan_and_fresh_speed_cap_never_write() {
 let(mut b,s)=scan_fixture();
 for p in [ScanPlan{start_nm:1044.,..plan()},ScanPlan{stop_nm:1086.,..plan()},ScanPlan{speed_nm_s:f64::NAN,..plan()},ScanPlan{speed_nm_s:11.,..plan()},ScanPlan{stop_nm:1060.,..plan()}] {assert!(b.control("6700 SN1012",Control::ScanStart(p),true).is_err());}
 s.lock().unwrap().replies.insert("SOUR:WAVE:MAXVEL?".into(),"0.5".into());
 assert!(b.control("6700 SN1012",Control::ScanStart(plan()),true).is_err());
 assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')&&!c.starts_with("OUTP:SCAN")));
}
#[test] fn scan_setting_failure_and_mismatch_never_start_or_cleanup_write() {
 for (command,reply) in [("SOUR:WAVE:STOP 1061","COMMAND NOT VALID"),("SOUR:WAVE:STOP?","1062")] {
  let(mut b,s)=scan_fixture();s.lock().unwrap().replies.insert(command.into(),reply.into());
  assert!(b.control("6700 SN1012",Control::ScanStart(plan()),true).is_err());
  assert!(b.control("6700 SN1012",Control::ScanStart(plan()),true).is_err());
  let st=s.lock().unwrap();assert!(!st.commands.iter().any(|(_,c)|c=="OUTP:SCAN:START"||c=="SYST:MCONT LOC"));
 }
}
#[test] fn stop_is_available_while_busy_and_never_resets_or_disables_output() {
 let(mut b,s)=scan_fixture();s.lock().unwrap().replies.insert("*OPC?".into(),"0".into());
 b.control("6700 SN1012",Control::ScanStop,true).unwrap();
 let st=s.lock().unwrap();assert_eq!(st.commands.iter().filter(|(_,c)|c=="OUTP:SCAN:STOP").count(),1);
 let stop=st.commands.iter().position(|(_,c)|c=="OUTP:SCAN:STOP").unwrap();
 assert_eq!(st.commands[stop+1].1,"OUTP:TRAC 0","Stop must hold the motor after stopping the scan engine");
 assert!(!st.commands.iter().any(|(_,c)|c=="*OPC?"||c=="OUTP:SCAN:RESET"||c=="OUTP:STAT 0"));
}
#[test] fn uncertain_hold_after_stop_is_not_replayed_or_reported_successfully() {
 let (mut b,s)=scan_fixture();s.lock().unwrap().replies.insert("OUTP:TRAC 0".into(),"COMMAND NOT VALID".into());
 assert!(b.control("6700 SN1012",Control::ScanStop,true).is_err());
 assert!(b.control("6700 SN1012",Control::ScanStop,true).is_err());
 let state=s.lock().unwrap();assert_eq!(state.commands.iter().filter(|(_,c)|c=="OUTP:SCAN:STOP").count(),1);
 assert!(!state.commands.iter().any(|(_,c)|c=="SYST:MCONT LOC"||c.starts_with("SOUR:WAVE ")||c.starts_with("OUTP:STAT ")));
}
#[test] fn local_return_failure_latches_fault_without_replaying_action() {
 let(mut b,s)=scan_fixture();s.lock().unwrap().replies.insert("SYST:MCONT LOC".into(),"COMMAND NOT VALID".into());
 assert!(b.control("6700 SN1012",Control::Piezo(50.),true).is_err());assert!(b.control("6700 SN1012",Control::Piezo(50.),true).is_err());
 assert_eq!(s.lock().unwrap().commands.iter().filter(|(_,c)|c=="SOUR:VOLT:PIEZ 50").count(),1);
}

#[test] fn operator_limits_can_only_narrow_hardware_and_gate_all_scan_legs() {
 let(mut b,s)=scan_fixture();
 assert!(b.set_limits("6700 SN1012",ControlLimits{min_nm:1044.,max_nm:1085.,max_speed_nm_s:1.}).is_err());
 b.set_limits("6700 SN1012",ControlLimits{min_nm:1059.,max_nm:1062.,max_speed_nm_s:1.}).unwrap();
 assert!(b.control("6700 SN1012",Control::Wavelength(1063.),true).is_err());
 assert!(b.action("6700 SN1012",Action::Wavelength(1063.),true).is_err());
 assert!(b.control("6700 SN1012",Control::ScanStart(ScanPlan{speed_nm_s:2.,..plan()}),true).is_err());
 s.lock().unwrap().replies.insert("SOUR:WAVE:SLEW:RET?".into(),"1".into());
 b.control("6700 SN1012",Control::ScanStart(plan()),true).unwrap();
 assert!(s.lock().unwrap().commands.iter().any(|(_,c)|c=="SOUR:WAVE:SLEW:RET 1"));
 assert!(!s.lock().unwrap().commands.iter().any(|(_,c)|c=="SOUR:WAVE:SLEW:RET 10"));
}

#[test] fn ready_target_preserves_tracking_and_backward_velocity_is_independent() {
 let(mut b,s)=scan_fixture();s.lock().unwrap().replies.insert("OUTP:TRAC?".into(),"0".into());b.control("6700 SN1012",Control::Target(1060.125),true).unwrap();
 assert!(!s.lock().unwrap().commands.iter().any(|(_,c)|c=="OUTP:TRAC 1"||c=="OUTP:STAT 1"));
 s.lock().unwrap().replies.insert("SOUR:WAVE:SLEW:RET?".into(),"0.5".into());
 b.control("6700 SN1012",Control::ScanStart(ScanPlan{return_speed_nm_s:Some(0.5),..plan()}),true).unwrap();
 assert!(s.lock().unwrap().commands.iter().any(|(_,c)|c=="SOUR:WAVE:SLEW:RET 0.5"));
}
#[test] fn following_target_preserves_panel_and_tracking_without_output_write() {
 let(mut b,s)=scan_fixture();b.control("6700 SN1012",Control::Target(1060.125),true).unwrap();
 let c=s.lock().unwrap().commands.iter().map(|(_,c)|c.clone()).collect::<Vec<_>>();
 assert_eq!(&c[3..],["*OPC?","SOUR:WAVE 1060.125"]);
 assert!(!c.iter().any(|c|c.starts_with("OUTP:STAT ")));
}
#[test] fn backward_velocity_limits_reject_before_any_write() {
 let(mut b,s)=scan_fixture();
 for v in [f64::NAN,0.,11.] {assert!(b.control("6700 SN1012",Control::ScanStart(ScanPlan{return_speed_nm_s:Some(v),..plan()}),true).is_err());}
 s.lock().unwrap().replies.insert("SOUR:WAVE:MAXVEL?".into(),"0.5".into());
 assert!(b.control("6700 SN1012",Control::ScanStart(ScanPlan{speed_nm_s:0.1,return_speed_nm_s:Some(1.),..plan()}),true).is_err());
 assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')));
}
