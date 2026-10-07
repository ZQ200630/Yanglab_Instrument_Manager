use yang_lab_tlb::*;
use std::{collections::BTreeMap,sync::{Arc,Mutex}};
#[derive(Default)]
struct State { commands:Vec<(String,String)>,opens:usize,closes:usize,close_error:bool,replies:BTreeMap<String,String> }
struct Script(Arc<Mutex<State>>);
impl Wire for Script {
 fn open(&mut self)->Result<Vec<String>> { self.0.lock().unwrap().opens+=1; Ok(vec!["6700 SN1012".into(),"6700 SN1013".into()]) }
 fn query(&mut self,key:&str,command:&str)->Result<String> { let mut s=self.0.lock().unwrap();s.commands.push((key.into(),command.into()));if command=="*IDN?" { return Ok(format!("New_Focus 6700 v2.4 03/19/14 SN{}",key.strip_prefix("6700 SN").unwrap())); } if let Some(v)=s.replies.get(command){return Ok(v.clone())} if command.contains(' '){return Ok("OK".into())} panic!("Unreviewed query {command}") }
 fn close(&mut self)->Result<()> {let mut s=self.0.lock().unwrap();s.closes+=1;if s.close_error{Err(Error{kind:"connection",message:"retained close".into()})}else{Ok(())}}
}
fn fixture(head:&str)->(Bus<Script>,Arc<Mutex<State>>) {let s=Arc::new(Mutex::new(State::default()));s.lock().unwrap().replies=[("SYST:LAS:MODEL?",head),("SYST:LAS:SN?","P1001"),("OUTP:STAT?","0"),("OUTP:TRAC?","1"),("SYST:MCONT?","LOC"),("SOUR:CPOW?","0"),("SENS:WAVE","1060.01"),("SOUR:WAVE?","1060"),("SENS:POW:DIODE","0"),("SOUR:POW:DIODE?","10"),("SENS:CURR:DIODE","0"),("SOUR:CURR:DIODE?","20"),("SOUR:VOLT:PIEZ?","50"),("*OPC?","1"),("*STB?","0")].into_iter().map(|(k,v)|(k.into(),v.into())).collect();(Bus::new(Script(s.clone())),s)}
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
fn scan_fixture()->(Bus<Script>,Arc<Mutex<State>>) {
 let (mut b,s)=fixture("6722-P");b.connect("6700 SN1012").unwrap();
 s.lock().unwrap().replies.extend([("SOUR:WAVE:MAXVEL?","10"),("SOUR:WAVE:START?","1060"),("SOUR:WAVE:STOP?","1061"),("SOUR:WAVE:SLEW:FORW?","1"),("SOUR:WAVE:SLEW:RET?","10"),("SOUR:WAVE:DESSCANS?","1"),("OUTP:SCAN:START","OK"),("OUTP:SCAN:STOP","OK")].into_iter().map(|(k,v)|(k.into(),v.into())));
 (b,s)
}
#[test] fn composite_wavelength_takes_remote_enables_tracking_and_returns_local() {
 let (mut b,s)=scan_fixture();s.lock().unwrap().replies.insert("OUTP:TRAC?".into(),"0".into());
 b.control("6700 SN1012",Control::Wavelength(1060.),true).unwrap();
 let commands=s.lock().unwrap().commands.iter().map(|(_,c)|c.clone()).collect::<Vec<_>>();
 assert_eq!(&commands[3..],["*OPC?","SYST:MCONT?","OUTP:TRAC?","SYST:MCONT REM","OUTP:TRAC 1","SOUR:WAVE 1060","SYST:MCONT LOC"]);
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
 assert!(!st.commands.iter().any(|(_,c)|c=="*OPC?"||c=="OUTP:SCAN:RESET"||c=="OUTP:STAT 0"));
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
 let(mut b,s)=scan_fixture();b.control("6700 SN1012",Control::Target(1060.125),true).unwrap();
 assert!(!s.lock().unwrap().commands.iter().any(|(_,c)|c=="OUTP:TRAC 1"||c=="OUTP:STAT 1"));
 s.lock().unwrap().replies.insert("SOUR:WAVE:SLEW:RET?".into(),"0.5".into());
 b.control("6700 SN1012",Control::ScanStart(ScanPlan{return_speed_nm_s:Some(0.5),..plan()}),true).unwrap();
 assert!(s.lock().unwrap().commands.iter().any(|(_,c)|c=="SOUR:WAVE:SLEW:RET 0.5"));
}
#[test] fn backward_velocity_limits_reject_before_any_write() {
 let(mut b,s)=scan_fixture();
 for v in [f64::NAN,0.,11.] {assert!(b.control("6700 SN1012",Control::ScanStart(ScanPlan{return_speed_nm_s:Some(v),..plan()}),true).is_err());}
 s.lock().unwrap().replies.insert("SOUR:WAVE:MAXVEL?".into(),"0.5".into());
 assert!(b.control("6700 SN1012",Control::ScanStart(ScanPlan{speed_nm_s:0.1,return_speed_nm_s:Some(1.),..plan()}),true).is_err());
 assert!(s.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')));
}
