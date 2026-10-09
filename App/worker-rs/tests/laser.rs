use serde_json::{
    json,
    Value
};
use yang_protocol::{
    ContextV3,
    DomainConfig,
    DomainRef
};
use yang_worker::{
    actions,
    catalog,
    DomainRegistry
};
use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::{
        Arc,
        Mutex,
        Condvar
    },
    time::Duration
};
use yang_drivers::clock::{
    Clock,
    ManualClock
};
use yang_worker::{
    newport::{
        NewportFactory,
        OwnerWire
    },
    session::{
        DriverFactory,
        SystemFactory
    }
};

#[derive(Default)]
struct WireState {
    opens: usize,
    closes: usize,
    close_fails: bool,
    head: String,
    commands: Vec<(String, String)>,
    threads: Vec<std::thread::ThreadId>,
    replies: BTreeMap<String, String>,
    block: Option<String>,
    query_delay: Option<(String, Duration, Arc<ManualClock>)>,
}
struct TestFactory {
    state: Arc<Mutex<WireState>>,
    gate: Arc<(Mutex<bool>, Condvar)>
}
struct TestWire {
    factory: TestFactory,
    _not_send: Rc<()>
}
impl NewportFactory for TestFactory {
    fn create(&self) -> yang_lab_tlb::Result<OwnerWire> {
        self.state.lock().unwrap().threads.push(std::thread::current().id());
        Ok(OwnerWire {
            wire: Box::new(TestWire {
                factory: TestFactory {
                    state: self.state.clone(),
                    gate: self.gate.clone()
                },
                _not_send: Rc::new(())
            }),
            guard: Box::new(Rc::new(()))
        })
    }
}
impl yang_lab_tlb::Wire for TestWire {
    fn open(&mut self) -> yang_lab_tlb::Result<Vec<String>> {
        let mut s = self.factory.state.lock().unwrap();
        s.threads.push(std::thread::current().id());
        s.opens += 1;
        Ok(vec!["6700 SN1012".into(), "6700 SN1013".into()])
    }
    fn query(&mut self, key: &str, command: &str) -> yang_lab_tlb::Result<String> {
        let mut s = self.factory.state.lock().unwrap();
        s.threads.push(std::thread::current().id());
        s.commands.push((key.into(), command.into()));
        let blocked = s.block.as_deref() == Some(command);
        drop(s);
        if blocked {
            let (guard, timed) = self.factory.gate.1.wait_timeout_while(self.factory.gate.0.lock().unwrap(), Duration::from_secs(3), |ready|!*ready).unwrap();
            assert!(!timed.timed_out() || *guard, "finite test gate was not released");
        }
        let mut s = self.factory.state.lock().unwrap();
        if let Some((delayed, duration, clock)) = &s.query_delay {
            if delayed == command { clock.wait(*duration); }
        }
        if let Some(value) = s.replies.get(command) {
            return Ok(value.clone());
        }
        match command {
            "*IDN?" => Ok(format!("New_Focus 6700 v2.4 03/19/14 SN{}", &key[7..])),
            "SYST:LAS:MODEL?" => Ok(s.head.clone()),
            "SYST:LAS:SN?" => Ok(format!("P{}", &key[7..])),
            "*OPC?" => Ok("1".into()),
            "OUTP:STAT?" | "SOUR:CPOW?" | "*STB?" => Ok("0".into()),
            "OUTP:TRAC?" => Ok("1".into()),
            "SYST:MCONT?" => Ok("LOC".into()),
            "SENS:WAVE" => Ok("1060.01".into()),
            "SOUR:WAVE?" => Ok("1060".into()),
            "SENS:POW:DIODE" => Ok("0.2".into()),
            "SOUR:POW:DIODE?" => Ok("10".into()),
            "SENS:CURR:DIODE" => Ok("2".into()),
            "SOUR:CURR:DIODE?" => Ok("20".into()),
            "SOUR:VOLT:PIEZ?" => Ok("50".into()),
            _ if command.contains(' ') || command.starts_with("OUTP:SCAN:") => {
                if command=="OUTP:TRAC 0" {s.replies.insert("OUTP:TRAC?".into(),"0".into());s.replies.insert("*OPC?".into(),"1".into());}
                if command=="OUTP:TRAC 1" {s.replies.insert("OUTP:TRAC?".into(),"1".into());}
                if let Some(v) = command.strip_prefix("SOUR:WAVE ") {
                    s.replies.insert("SOUR:WAVE?".into(), v.into());
                }
                Ok("OK".into())
            }
            _ => panic!("unreviewed test query {command}"),
        }
    }
    fn close(&mut self) -> yang_lab_tlb::Result<()> {
        let mut s = self.factory.state.lock().unwrap();
        s.threads.push(std::thread::current().id());
        s.closes += 1;
        if s.close_fails {
            Err(yang_lab_tlb::Error {
                kind: "connection",
                message: "injected retained release".into()
            })
        } else {
            Ok(())
        }
    }
}
impl Drop for TestWire {
    fn drop(&mut self) {
        self.factory.state.lock().unwrap().threads.push(std::thread::current().id());
    }
}
fn factory(timeout: Duration) -> (SystemFactory, Arc<Mutex<WireState>>, Arc<ManualClock>, Arc<(Mutex<bool>, Condvar)>) {
    let state = Arc::new(Mutex::new(WireState {
        head: "6722-P".into(),
        .. WireState::default()
    }));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let clock = Arc::new(ManualClock::default());
    let f = SystemFactory::with_newport(clock.clone(), Arc::new(TestFactory {
        state: state.clone(),
        gate: gate.clone()
    }), timeout);
    (f, state, clock, gate)
}
fn context(c: &DomainConfig) -> ContextV3 {
    ContextV3 {
        session_id: "d".repeat(32),
        domain: Some(c.domain.clone()),
        connection_id: Some("e".repeat(32)),
        epoch: 1
    }
}

#[test]
fn stop_scan_releases_busy_tracking_and_the_next_manual_target_can_move() {
    let (factory,wire,_,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let context=context(&config);
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();
    wire.lock().unwrap().replies.insert("*OPC?".into(),"0".into());
    let stopped=session.action("stop_scan",&json!({"confirm":true}),&context);
    assert_eq!(stopped.phase,yang_protocol::Phase::Completed);
    assert_eq!(stopped.result.as_ref().unwrap()["status"]["motion_pending"],true,"ACK cannot invent readiness");
    let readback=session.observe(&context).status;
    assert_eq!(readback["motion"]["operation_complete"],true);
    assert_eq!(readback["motion"]["tracking"],false);
    assert_eq!(readback["target_following_enabled"],true,"hold preserves the operator's future manual-following preference");
    assert_eq!(session.action("set_target_wavelength",&json!({"wavelength_nm":1060.2,"confirm":true}),&context).phase,yang_protocol::Phase::Completed);
    let commands=wire.lock().unwrap().commands.clone();
    assert!(commands.iter().any(|(_,c)|c=="SOUR:WAVE 1060.2"));assert!(commands.iter().any(|(_,c)|c=="OUTP:TRAC 1"));
    assert!(session.close().unwrap().resources_released());
}

#[test]
fn native_laser_factory_is_lazy_and_two_controllers_share_one_owner_thread() {
    let (f, state, _, _) = factory(Duration::from_secs(1));
    let caller = std::thread::current().id();
    let mut a = f.create(&config('a', "1012")).unwrap();
    let mut b = f.create(&config('b', "1013")).unwrap();
    assert_eq!(state.lock().unwrap().opens, 0);
    assert!(f.newport_resources_released());
    a.connect().unwrap();
    b.connect().unwrap();
    assert!(!f.newport_resources_released());
    assert!(a.close().unwrap().resources_released());
    assert_eq!(state.lock().unwrap().closes, 0);
    assert_eq!(b.observe(&context(&config('b', "1013"))).status["connected"], true);
    assert!(b.close().unwrap().resources_released());
    assert!(f.newport_resources_released());
    let s = state.lock().unwrap();
    assert_eq!(s.opens, 1);
    assert_eq!(s.closes, 1);
    assert!(s.threads.iter().all(|id| *id == s.threads[0] && *id != caller));
    assert!(s.commands.iter().all(|(_, c)|!c.contains(' ') && !c.starts_with("OUTP:SCAN:")));
}

#[test]
fn goto_is_one_owned_move_and_turns_tracking_off_only_after_confirmed_arrival() {
    let (factory,wire,clock,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let context=context(&config);
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();wire.lock().unwrap().commands.clear();
    let outcome=session.action("goto_wavelength",&json!({"wavelength_nm":1060.2,"confirm":true}),&context);
    assert_eq!(outcome.phase,yang_protocol::Phase::Completed);
    assert_eq!(outcome.result.as_ref().unwrap()["acknowledged"],true);
    wire.lock().unwrap().replies.insert("*OPC?".into(),"0".into());clock.wait(Duration::from_secs(3));
    let moving=session.observe(&context).status;
    assert_eq!(moving["move"]["phase"],"moving");assert_eq!(moving["move"]["elapsed_s"],3.0);
    assert!(!wire.lock().unwrap().commands.iter().any(|(_,c)|c=="OUTP:TRAC 0"));
    assert_ne!(session.action("goto_wavelength",&json!({"wavelength_nm":1060.3,"confirm":true}),&context).phase,yang_protocol::Phase::Completed);
    wire.lock().unwrap().replies.insert("*OPC?".into(),"1".into());
    assert_eq!(session.observe(&context).status["move"]["phase"],"moving","OPC alone is not arrival");
    wire.lock().unwrap().replies.insert("SENS:WAVE".into(),"1060.2".into());
    let arrived=session.observe(&context).status;
    assert_eq!(arrived["move"]["phase"],"arrived");assert_eq!(arrived["motion"]["tracking"],false);
    session.observe(&context);
    let writes=wire.lock().unwrap().commands.iter().filter(|(_,c)|c.contains(' ')).map(|(_,c)|c.clone()).collect::<Vec<_>>();
    assert_eq!(writes,["SOUR:WAVE 1060.2","OUTP:TRAC 1","OUTP:TRAC 0"],"no replay or emission write");
    assert!(session.close().unwrap().resources_released());
}

#[test]
fn reconnect_preserves_imported_tracking_until_explicit_goto_or_full_scan() {
 for full in [false,true] {
    let (factory,wire,_,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let ctx=context(&config);
    wire.lock().unwrap().replies.extend([("*OPC?","0"),("OUTP:TRAC?","1"),("OUTP:STAT?","1"),("SOUR:WAVE:MAXVEL?","10"),("SOUR:WAVE:START?","1060"),("SOUR:WAVE:STOP?","1061"),("SOUR:WAVE:SLEW:FORW?","0.1"),("SOUR:WAVE:SLEW:RET?","0.1"),("SOUR:WAVE:DESSCANS?","1")].into_iter().map(|(k,v)|(k.into(),v.into())));
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();
    let before=session.observe(&ctx).status;
    assert_eq!(before["move"],Value::Null);assert_eq!(before["motion_pending"],false);
    assert_eq!(before["motion"]["operation_complete"],false);assert_eq!(before["motion"]["tracking"],true);
    assert!(wire.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')),"connect and idle observation must preserve imported tracking");
    wire.lock().unwrap().commands.clear();
    let (name,args)=if full {("start_scan",json!({"start_nm":1060.,"stop_nm":1061.,"speed_nm_s":0.1,"return_speed_nm_s":0.1,"confirm":true}))}
        else {("goto_wavelength",json!({"wavelength_nm":1060.2,"confirm":true}))};
    let started=session.action(name,&args,&ctx);
    assert_eq!(started.phase,yang_protocol::Phase::Completed,"explicit new motion must take over inherited tracking");
    let status=&started.result.as_ref().unwrap()["status"];
    assert_eq!(status["move"]["phase"],"moving");assert_eq!(status["move"]["kind"],if full {"full_scan"}else{"goto"});
    let commands=wire.lock().unwrap().commands.clone();
    let stop=commands.iter().position(|(_,c)|c=="OUTP:SCAN:STOP").unwrap();
    let off=commands.iter().position(|(_,c)|c=="OUTP:TRAC 0").unwrap();
    let movement=commands.iter().position(|(_,c)|if full {c=="OUTP:SCAN:START"}else{c=="SOUR:WAVE 1060.2"}).unwrap();
    assert!(stop<off&&off<movement);
    assert!(commands[off+1..movement].iter().any(|(_,c)|c=="*OPC?"),"hold must be verified before new movement");
    assert!(!commands.iter().any(|(_,c)|c.starts_with("OUTP:STAT ")));
    let count=commands.len();
    assert_eq!(session.action(name,&args,&ctx).phase,yang_protocol::Phase::RejectedBeforeCall,"active owned motion still requires explicit Stop");
    assert_eq!(wire.lock().unwrap().commands.len(),count);
    assert!(session.close().unwrap().resources_released());
 }
}

#[test]
fn unconfirmed_imported_tracking_hold_faults_without_replaying_new_motion() {
    let (factory,wire,_,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let ctx=context(&config);
    wire.lock().unwrap().replies.extend([("*OPC?","0"),("OUTP:TRAC?","1"),("OUTP:TRAC 0","OK")].into_iter().map(|(k,v)|(k.into(),v.into())));
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();wire.lock().unwrap().commands.clear();
    let args=json!({"wavelength_nm":1060.2,"confirm":true});
    let failed=session.action("goto_wavelength",&args,&ctx);
    assert_eq!(failed.phase,yang_protocol::Phase::FailedAfterCallStarted,"the hold setter has started; failure is not a before-call rejection");
    assert_eq!(session.action("goto_wavelength",&args,&ctx).phase,yang_protocol::Phase::RejectedBeforeCall);
    let commands=wire.lock().unwrap().commands.clone();
    assert_eq!(commands.iter().filter(|(_,c)|c=="OUTP:TRAC 0").count(),1);
    assert!(!commands.iter().any(|(_,c)|c.starts_with("SOUR:WAVE ")||c=="OUTP:TRAC 1"||c.starts_with("OUTP:STAT ")));
    assert!(session.close().unwrap().resources_released());
}

#[test]
fn owned_goto_does_not_stop_external_motion_and_times_out_truthfully() {
 for case in ["changed-target","tracking-lost","timeout"] {
    let (factory,wire,clock,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let ctx=context(&config);
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();
    assert_eq!(session.action("goto_wavelength",&json!({"wavelength_nm":1060.2,"confirm":true}),&ctx).phase,yang_protocol::Phase::Completed);
    let mut state=wire.lock().unwrap();state.commands.clear();state.replies.insert("*OPC?".into(),"0".into());
    if case=="changed-target" {state.replies.insert("SOUR:WAVE?".into(),"1060.3".into());}
    if case=="tracking-lost" {state.replies.insert("OUTP:TRAC?".into(),"0".into());}drop(state);
    if case=="timeout" {clock.wait(Duration::from_secs(121));}
    let observed=session.observe(&ctx).status;
    assert_eq!(observed["move"]["phase"],if case=="timeout" {"timed_out"}else{"interrupted"});
    assert!(wire.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')),"{case}: no automatic setter");
    assert!(session.close().unwrap().resources_released());
 }
}

#[test]
fn stable_goto_endpoint_can_hold_when_firmware_keeps_opc_busy() {
    let (factory,wire,clock,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let ctx=context(&config);
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();
    assert_eq!(session.action("goto_wavelength",&json!({"wavelength_nm":1060.2,"confirm":true}),&ctx).phase,yang_protocol::Phase::Completed);
    wire.lock().unwrap().replies.extend([("*OPC?".into(),"0".into()),("SENS:WAVE".into(),"1060.2".into())]);
    assert_eq!(session.observe(&ctx).status["move"]["phase"],"moving");
    clock.wait(Duration::from_secs(2));
    let held=session.observe(&ctx).status;
    assert_eq!(held["move"]["phase"],"arrived");assert_eq!(held["motion"]["operation_complete"],true);
    assert_eq!(held["motion"]["tracking"],false);
    assert!(session.close().unwrap().resources_released());
}

#[test]
fn slow_endpoint_read_does_not_count_toward_the_goto_stability_interval() {
    let (factory,wire,clock,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let ctx=context(&config);
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();
    assert_eq!(session.action("goto_wavelength",&json!({"wavelength_nm":1060.2,"confirm":true}),&ctx).phase,yang_protocol::Phase::Completed);
    {
        let mut state=wire.lock().unwrap();state.commands.clear();
        state.replies.extend([("*OPC?".into(),"0".into()),("SENS:WAVE".into(),"1060.2".into())]);
        state.query_delay=Some(("SENS:WAVE".into(),Duration::from_secs(2),clock.clone()));
    }
    assert_eq!(session.observe(&ctx).status["move"]["phase"],"moving");
    wire.lock().unwrap().query_delay=None;
    assert_eq!(session.observe(&ctx).status["move"]["phase"],"moving","two immediate observations do not establish one second of stability");
    assert!(!wire.lock().unwrap().commands.iter().any(|(_,c)|c=="OUTP:TRAC 0"));
    clock.wait(Duration::from_secs(1));
    assert_eq!(session.observe(&ctx).status["move"]["phase"],"arrived");
    assert!(session.close().unwrap().resources_released());
}

#[test]
fn external_tracking_off_after_a_near_goto_observation_is_not_arrival() {
    let (factory,wire,clock,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let ctx=context(&config);
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();
    assert_eq!(session.action("goto_wavelength",&json!({"wavelength_nm":1060.2,"confirm":true}),&ctx).phase,yang_protocol::Phase::Completed);
    wire.lock().unwrap().replies.extend([("*OPC?".into(),"0".into()),("SENS:WAVE".into(),"1060.2".into())]);
    assert_eq!(session.observe(&ctx).status["move"]["phase"],"moving");
    clock.wait(Duration::from_secs(2));wire.lock().unwrap().replies.insert("OUTP:TRAC?".into(),"0".into());wire.lock().unwrap().commands.clear();
    assert_eq!(session.observe(&ctx).status["move"]["phase"],"interrupted");
    assert!(wire.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')));
    assert!(session.close().unwrap().resources_released());
}

#[test]
fn external_full_scan_stop_before_the_endpoint_is_reported_as_interrupted() {
    let (factory,wire,_,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let ctx=context(&config);
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();
    wire.lock().unwrap().replies.extend([("SOUR:WAVE:MAXVEL?","10"),("SOUR:WAVE:START?","1060"),("SOUR:WAVE:STOP?","1061"),("SOUR:WAVE:SLEW:FORW?","0.1"),("SOUR:WAVE:SLEW:RET?","0.1"),("SOUR:WAVE:DESSCANS?","1")].into_iter().map(|(k,v)|(k.into(),v.into())));
    assert_eq!(session.action("start_scan",&json!({"start_nm":1060.,"stop_nm":1061.,"speed_nm_s":0.1,"return_speed_nm_s":0.1,"confirm":true}),&ctx).phase,yang_protocol::Phase::Completed);
    wire.lock().unwrap().replies.extend([("*OPC?".into(),"1".into()),("OUTP:TRAC?".into(),"0".into()),("SENS:WAVE".into(),"1060.5".into())]);wire.lock().unwrap().commands.clear();
    assert_eq!(session.observe(&ctx).status["move"]["phase"],"interrupted","a stopped controller away from the endpoint must not remain moving forever");
    assert!(wire.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')));
    assert!(session.close().unwrap().resources_released());
}

#[test]
fn full_scan_does_not_stop_on_initial_start_or_use_the_goto_timeout() {
    let (factory,wire,clock,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let ctx=context(&config);
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();
    wire.lock().unwrap().replies.extend([("SOUR:WAVE:MAXVEL?","10"),("SOUR:WAVE:START?","1060"),("SOUR:WAVE:STOP?","1061"),("SOUR:WAVE:SLEW:FORW?","0.1"),("SOUR:WAVE:SLEW:RET?","0.1"),("SOUR:WAVE:DESSCANS?","1")].into_iter().map(|(k,v)|(k.into(),v.into())));
    assert_eq!(session.action("start_scan",&json!({"start_nm":1060.,"stop_nm":1061.,"speed_nm_s":0.1,"return_speed_nm_s":0.1,"confirm":true}),&ctx).phase,yang_protocol::Phase::Completed);
    wire.lock().unwrap().commands.clear();wire.lock().unwrap().replies.extend([("*OPC?".into(),"0".into()),("SENS:WAVE".into(),"1060".into())]);
    session.observe(&ctx);clock.wait(Duration::from_secs(300));
    assert_eq!(session.observe(&ctx).status["move"]["phase"],"moving");
    assert!(!wire.lock().unwrap().commands.iter().any(|(_,c)|c=="OUTP:TRAC 0"));
    wire.lock().unwrap().replies.insert("*OPC?".into(),"1".into());
    assert_eq!(session.observe(&ctx).status["move"]["phase"],"arrived");
    assert!(session.close().unwrap().resources_released());
}

#[test]
fn stop_cancels_owned_goto_without_resuming_and_close_preserves_motion() {
 for close in [false,true] {
    let (factory,wire,_,_)=factory(Duration::from_secs(1));let config=config('a',"1012");let ctx=context(&config);
    let mut session=factory.create(&config).unwrap();session.connect().unwrap();
    assert_eq!(session.action("goto_wavelength",&json!({"wavelength_nm":1060.2,"confirm":true}),&ctx).phase,yang_protocol::Phase::Completed);
    wire.lock().unwrap().commands.clear();wire.lock().unwrap().replies.insert("*OPC?".into(),"0".into());
    if close {assert!(session.close().unwrap().resources_released());assert!(wire.lock().unwrap().commands.iter().all(|(_,c)|!c.contains(' ')));}
    else {
      assert_eq!(session.action("stop_scan",&json!({"confirm":true}),&ctx).phase,yang_protocol::Phase::Completed);
      assert_eq!(session.observe(&ctx).status["move"]["phase"],"stopped");session.observe(&ctx);
      let commands=wire.lock().unwrap().commands.clone();
      assert_eq!(commands.iter().filter(|(_,c)|c=="OUTP:TRAC 0").count(),1);
      assert!(!commands.iter().any(|(_,c)|c.starts_with("SOUR:WAVE ")||c=="OUTP:TRAC 1"));
      assert!(session.close().unwrap().resources_released());
    }
 }
}

#[test]
fn native_target_ack_and_motion_preserve_full_sample_and_following_preference() {
    let (f, state, clock, _) = factory(Duration::from_secs(1));
    let c = config('a', "1012");
    let ctx = context(&c);
    let mut d = f.create(&c).unwrap();
    let first = d.connect().unwrap().observations().clone();
    state.lock().unwrap().replies.insert("OUTP:TRAC?".into(), "0".into());
    clock.wait(Duration::from_secs(2));
    let motion = d.observe(&ctx);
    assert_eq!(motion.status["target_following_enabled"], true);
    let outcome = d.action("set_target_wavelength", &json!({
        "wavelength_nm": 1060.5,
        "confirm": true
    }), &ctx);
    assert_eq!(outcome.phase, yang_protocol::Phase::Completed);
    assert_eq!(outcome.result.as_ref().unwrap() ["acknowledged"], true);
    assert_eq!(outcome.result.as_ref().unwrap() ["status"]["motion_pending"], true);
    assert!(state.lock().unwrap().commands.iter().any(|(_, v)| v == "OUTP:TRAC 1"));
    state.lock().unwrap().replies.insert("*OPC?".into(), "0".into());
    clock.wait(Duration::from_secs(2));
    let later = d.observe(&ctx).status;
    assert_eq!(later["laser"], first["laser"]);
    assert_eq!(later["sample_age_s"], 4.0);
    assert_eq!(later["motion"]["received_at"], 4.0);
    assert_eq!(later["motion_pending"], false);
    assert_eq!(later["motion"]["operation_complete"], false);
    state.lock().unwrap().replies.insert("*OPC?".into(), "1".into());
    let off = d.action("control_tracking", &json!({
        "enabled": false,
        "confirm": true
    }), &ctx);
    assert_eq!(off.phase, yang_protocol::Phase::Completed);
    d.observe(&ctx);
    state.lock().unwrap().commands.clear();
    d.action("set_target_wavelength", &json!({
        "wavelength_nm": 1060.6,
        "confirm": true
    }), &ctx);
    assert!(!state.lock().unwrap().commands.iter().any(|(_, v)| v == "OUTP:TRAC 1"));
    assert!(d.close().unwrap().resources_released());
}

#[test]
fn saved_limits_and_swapped_head_prevent_control_without_writes() {
    let (f, state, _, _) = factory(Duration::from_secs(1));
    let mut c = config('a', "1012");
    c.params = json!({
        "device_key": "6700 SN1012",
        "operating_min_nm": 1059,
        "operating_max_nm": 1062,
        "scan_speed_limit_nm_s": 1
    });
    let mut d = f.create(&c).unwrap();
    d.connect().unwrap();
    let n = state.lock().unwrap().commands.len();
    assert_ne!(d.action("set_target_wavelength", &json!({
        "wavelength_nm": 1063,
        "confirm": true
    }), &context(&c)).phase, yang_protocol::Phase::Completed);
    assert_eq!(state.lock().unwrap().commands.len(), n);
    d.close().unwrap();
    state.lock().unwrap().head = "6713".into();
    let mut d = f.create(&c).unwrap();
    assert!(d.probe_readonly().is_err());
    d.close().unwrap();
    assert!(state.lock().unwrap().commands.iter().all(|(_, v)|!v.contains(' ')));
}

#[test]
fn failed_global_scan_retains_release_obligation_and_never_returns_partial_heads() {
    let (f, state, _, _) = factory(Duration::from_secs(1));
    state.lock().unwrap().close_fails = true;
    assert!(f.scan_lasers().is_err());
    assert!(!f.newport_resources_released());
    assert!(f.auxiliary_responsibility());
    let n = state.lock().unwrap().opens;
    assert!(f.scan_lasers().is_err());
    assert_eq!(state.lock().unwrap().opens, n);
    let failed = f.finish_shutdown().unwrap();
    assert!(!failed.resources_released());
    state.lock().unwrap().close_fails = false;
    assert!(f.finish_shutdown().unwrap().resources_released());
    assert!(!failed.resources_released());
    assert!(f.newport_resources_released());
    let scan = f.scan_lasers().unwrap();
    assert_eq!(scan, json!({
        "controllers": [{
            "device_key": "6700 SN1012",
            "serial": "1012",
            "head_model": "6722-P",
            "head_serial": "P1012"
        }, {
            "device_key": "6700 SN1013",
            "serial": "1013",
            "head_model": "6722-P",
            "head_serial": "P1013"
        }]
    }));
    assert!(state.lock().unwrap().commands.iter().all(|(_, v)| matches!(v.as_str(), "*IDN?" | "SYST:LAS:MODEL?" | "SYST:LAS:SN?")));
}

#[test]
fn blocked_setter_retains_exact_exchange_until_explicit_preserving_close() {
    let (f, state, _, gate) = factory(Duration::from_millis(30));
    let c = config('a', "1012");
    let mut d = f.create(&c).unwrap();
    d.connect().unwrap();
    state.lock().unwrap().block = Some("SOUR:WAVE 1060.5".into());
    let out = d.action("set_target_wavelength", &json!({
        "wavelength_nm": 1060.5,
        "confirm": true
    }), &context(&c));
    assert_eq!(out.phase, yang_protocol::Phase::FailedAfterCallStarted);
    assert!(d.has_responsibility());
    assert!(!f.newport_resources_released());
    let failed = d.close().unwrap();
    assert!(!failed.resources_released());
    assert_eq!(state.lock().unwrap().closes, 0);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    assert!(d.close().unwrap().resources_released());
    assert!(f.newport_resources_released());
    assert!(!failed.resources_released());
    assert_eq!(state.lock().unwrap().commands.iter().filter(|(_, v)| v == "SOUR:WAVE 1060.5").count(), 1);
}

struct EmptyInventory;
impl yang_worker::discovery::InventoryPort for EmptyInventory {
    fn serial(&self) -> Result<Vec<yang_drivers::transport::serial_discovery::SerialDeviceInfo>, yang_worker::WorkerError> {
        Ok(vec![])
    }
    fn visa(&self) -> Result<Vec<String>, yang_worker::WorkerError> {
        Ok(vec![])
    }
}
fn request(method: &str, params: Value, context: ContextV3) -> yang_protocol::RequestV3 {
    yang_protocol::RequestV3 {
        v: 3,
        id: yang_worker::new_id().unwrap(),
        method: method.into(),
        params,
        context: Some(context)
    }
}
#[test]
fn native_backend_scan_retention_and_shutdown_status_are_global() {
    use yang_worker::scheduler::Backend;
    let (f, state, clock, _) = factory(Duration::from_secs(1));
    let registry = DomainRegistry::new(&"d".repeat(32)).unwrap();
    let backend = yang_worker::backend::NativeBackend::with_ports(registry, clock.clone(), Arc::new(f), Arc::new(EmptyInventory));
    let scheduler = yang_worker::scheduler::Scheduler::new(backend.clone(), clock, yang_protocol::Limits::default()).unwrap();
    let global = backend.global_context();
    state.lock().unwrap().close_fails = true;
    let scan = scheduler.submit(request("scan_lasers", json!({}), global.clone())).unwrap();
    let result = scan.wait(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap();
    scan.acknowledge();
    assert_ne!(result.phase, yang_protocol::Phase::Completed);
    let status = scheduler.submit(request("status", json!({}), global.clone())).unwrap();
    assert_eq!(status.wait(yang_drivers::transport::Deadline::after(Duration::from_secs(1))).unwrap().result.unwrap() ["newport_resources_released"], false);
    status.acknowledge();
    state.lock().unwrap().close_fails = false;
    scheduler.begin_shutdown();
    scheduler.join_when_released(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap();
    backend.finish_shutdown().unwrap();
    assert!(!backend.auxiliary_responsibility());
    assert!(backend.cleanup_reports().iter().any(|r| r.steps().iter().any(|s| s.role == "newport:native")));
}
#[test]
fn cached_status_is_responsive_and_release_is_false_during_global_scan() {
    use yang_worker::scheduler::Backend;
    let (f, state, clock, gate) = factory(Duration::from_secs(1));
    state.lock().unwrap().block = Some("*IDN?".into());
    let backend = yang_worker::backend::NativeBackend::with_ports(DomainRegistry::new(&"d".repeat(32)).unwrap(), clock.clone(), Arc::new(f), Arc::new(EmptyInventory));
    let scheduler = yang_worker::scheduler::Scheduler::new(backend.clone(), clock, yang_protocol::Limits::default()).unwrap();
    let scan = scheduler.submit(request("scan_lasers", json!({}), backend.global_context())).unwrap();
    until(|| state.lock().unwrap().commands.iter().any(|(_, v)| v == "*IDN?"));
    let pending = scheduler.submit(request("status", json!({}), backend.global_context())).unwrap();
    let cached = pending.wait(yang_drivers::transport::Deadline::after(Duration::from_millis(200))).unwrap();
    pending.acknowledge();
    assert_eq!(cached.result.unwrap() ["newport_resources_released"], false);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    assert_eq!(scan.wait(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap().phase, yang_protocol::Phase::Completed);
    scan.acknowledge();
    assert_eq!(submit_wait(&scheduler, request("ping", json!({}), backend.global_context())).result.unwrap() ["newport_resources_released"], true);
    scheduler.begin_shutdown();
    scheduler.join_when_released(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap();
    backend.finish_shutdown().unwrap();
}

#[test]
fn stop_intent_cancels_a_setter_queued_behind_another_controller() {
    let (f, state, _, gate) = factory(Duration::from_secs(1));
    let ca = config('a', "1012");
    let cb = config('b', "1013");
    let mut a = f.create(&ca).unwrap();
    let mut b = f.create(&cb).unwrap();
    a.connect().unwrap();
    b.connect().unwrap();
    state.lock().unwrap().block = Some("SENS:WAVE".into());
    state.lock().unwrap().commands.clear();
    let signal = b.stop_signal();
    let state_check = state.clone();
    let at = std::thread::spawn(move || {
        a.observe(&context(&ca));
        a
    });
    until(|| state_check.lock().unwrap().commands.iter().any(|(_, v)| v == "SENS:WAVE"));
    let bt = std::thread::spawn(move || {
        let out = b.action("set_target_wavelength", &json!({
            "wavelength_nm": 1060.7,
            "confirm": true
        }), &context(&cb));
        (b, out)
    });
    std::thread::sleep(Duration::from_millis(20));
    signal.request_stop();
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let mut a = at.join().unwrap();
    let (mut b, out) = bt.join().unwrap();
    assert_ne!(out.phase, yang_protocol::Phase::Completed);
    assert!(!state.lock().unwrap().commands.iter().any(|(_, v)| v == "SOUR:WAVE 1060.7"));
    a.close().unwrap();
    b.close().unwrap();
    assert!(f.newport_resources_released());
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(std::time::Instant::now() < deadline, "finite condition did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn registered_backend(f: SystemFactory, clock: Arc<ManualClock>, c: DomainConfig) -> Arc<yang_worker::backend::NativeBackend> {
    use yang_worker::scheduler::Backend;
    let backend = yang_worker::backend::NativeBackend::with_ports(DomainRegistry::new(&"d".repeat(32)).unwrap(), clock, Arc::new(f), Arc::new(EmptyInventory));
    assert_eq!(backend.execute(&request("configure_domain", json!({
        "config": c
    }), backend.global_context())).phase, yang_protocol::Phase::Completed);
    let ctx = backend.registry().context(&c.domain).unwrap();
    let proof = backend.execute(&request("probe", json!({
        "authorization": {
            "stage": "readonly",
            "accepted": true,
            "supervised": false,
            "retain_session": false,
            "binding": {
                "mode": "real",
                "domain": c.domain,
                "config_rev": 1,
                "model_id": "tlb6700",
                "profile_id": "newport-usb",
                "config_digest": "a".repeat(64)
            }
        }
    }), ctx));
    assert_eq!(proof.phase, yang_protocol::Phase::Completed, "{:?}", proof.error);
    let registration = backend.execute(&request("register_verified", json!({
        "domain": c.domain,
        "proof_id": proof.result.unwrap() ["proof"]["proof_id"],
        "config_digest": "a".repeat(64),
        "config_rev": 1
    }), backend.global_context()));
    assert_eq!(registration.phase, yang_protocol::Phase::Completed);
    backend
}
fn submit_wait(scheduler: &yang_worker::scheduler::Scheduler, request: yang_protocol::RequestV3) -> yang_protocol::OutcomeV3 {
    let pending = scheduler.submit(request).unwrap();
    let result = pending.wait(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap();
    pending.acknowledge();
    result
}
#[test]
fn failed_laser_connect_with_confirmed_release_publishes_new_disconnected_context() {
    use yang_worker::scheduler::Backend;
    let (f, state, clock, _) = factory(Duration::from_secs(1));
    let c = config('a', "1012");
    let b = registered_backend(f, clock.clone(), c.clone());
    state.lock().unwrap().replies.insert("OUTP:STAT?".into(), "INVALID".into());
    let scheduler = yang_worker::scheduler::Scheduler::new(b.clone(), clock, yang_protocol::Limits::default()).unwrap();
    let before = b.registry().context(&c.domain).unwrap();
    let result = submit_wait(&scheduler, request("connect", json!({}), before.clone()));
    assert_eq!(result.phase, yang_protocol::Phase::FailedAfterCallStarted);
    let released = result.context.unwrap();
    assert!(released.connection_id.is_none());
    assert!(released.epoch > before.epoch);
    assert!(!b.registry().snapshot() [0].responsibility);
    let stale = ContextV3 {
        connection_id: Some("f".repeat(32)),
        epoch: released.epoch - 1,
        .. released
    };
    assert_eq!(submit_wait(&scheduler, request("action", json!({
        "name": "read_status",
        "args": {}
    }), stale)).phase, yang_protocol::Phase::RejectedBeforeCall);
    let cached = submit_wait(&scheduler, request("status", json!({}), b.global_context())).result.unwrap();
    assert_eq!(cached["domains"][format!("device:{}", c.domain.id)]["state"], "DISCONNECTED");
    assert_eq!(cached["newport_resources_released"], true);
    scheduler.begin_shutdown();
    scheduler.join_when_released(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap();
    b.finish_shutdown().unwrap();
}
#[test]
fn failed_laser_connect_with_unconfirmed_close_keeps_connection_and_responsibility() {
    use yang_worker::scheduler::Backend;
    let (f, state, clock, _) = factory(Duration::from_secs(1));
    let c = config('a', "1012");
    let b = registered_backend(f, clock.clone(), c.clone());
    {
        let mut s = state.lock().unwrap();
        s.replies.insert("OUTP:STAT?".into(), "INVALID".into());
        s.close_fails = true;
    }
    let scheduler = yang_worker::scheduler::Scheduler::new(b.clone(), clock, yang_protocol::Limits::default()).unwrap();
    let result = submit_wait(&scheduler, request("connect", json!({}), b.registry().context(&c.domain).unwrap()));
    assert_eq!(result.phase, yang_protocol::Phase::FailedAfterCallStarted);
    assert!(result.context.unwrap().connection_id.is_some());
    assert!(b.registry().snapshot() [0].responsibility);
    let failed = b.cleanup_reports().into_iter().find(|r| r.steps().iter().any(|s| s.role == "laser")).unwrap();
    assert!(!failed.resources_released());
    let status = submit_wait(&scheduler, request("status", json!({}), b.global_context())).result.unwrap();
    assert_eq!(status["newport_resources_released"], false);
    state.lock().unwrap().close_fails = false;
    scheduler.begin_shutdown();
    scheduler.join_when_released(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap();
    b.finish_shutdown().unwrap();
    assert!(!failed.resources_released());
    assert!(state.lock().unwrap().commands.iter().all(|(_, v)|!v.contains(' ')));
}
#[test]
fn acknowledged_setter_with_failed_full_readback_is_never_replayed() {
    use yang_worker::scheduler::Backend;
    let (f, state, clock, _) = factory(Duration::from_secs(1));
    let c = config('a', "1012");
    let b = registered_backend(f, clock.clone(), c.clone());
    let scheduler = yang_worker::scheduler::Scheduler::new(b.clone(), clock, yang_protocol::Limits::default()).unwrap();
    let connected = submit_wait(&scheduler, request("connect", json!({}), b.registry().context(&c.domain).unwrap()));
    let ctx = connected.context.unwrap();
    let sample = connected.result.unwrap() ["status"]["laser"].clone();
    {
        let mut s = state.lock().unwrap();
        s.commands.clear();
        s.replies.insert("OUTP:STAT?".into(), "INVALID".into());
    }
    let outcome = submit_wait(&scheduler, request("action", json!({
        "name": "set_remote",
        "args": {
            "remote": true,
            "confirm": true
        }
    }), ctx));
    assert_eq!(outcome.phase, yang_protocol::Phase::CompletedReadbackFailed);
    let cached = submit_wait(&scheduler, request("status", json!({}), b.global_context())).result.unwrap();
    let key = format!("device:{}", c.domain.id);
    assert_eq!(cached["devices"][&key]["laser"], sample);
    assert!(!cached["devices"][&key]["observation_error"].is_null());
    assert_eq!(state.lock().unwrap().commands.iter().filter(|(_, v)| v == "SYST:MCONT REM").count(), 1);
    scheduler.begin_shutdown();
    scheduler.join_when_released(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap();
    b.finish_shutdown().unwrap();
}
#[test]
fn cached_worker_age_advances_while_full_getter_is_blocked() {
    use yang_worker::scheduler::Backend;
    let (f, state, clock, gate) = factory(Duration::from_secs(1));
    let c = config('a', "1012");
    let b = registered_backend(f, clock.clone(), c.clone());
    let scheduler = yang_worker::scheduler::Scheduler::new(b.clone(), clock.clone(), yang_protocol::Limits::default()).unwrap();
    let connected = submit_wait(&scheduler, request("connect", json!({}), b.registry().context(&c.domain).unwrap()));
    assert_eq!(connected.phase, yang_protocol::Phase::Completed);
    let ctx = connected.context.unwrap();
    let key = format!("device:{}", c.domain.id);
    let first = submit_wait(&scheduler, request("status", json!({}), b.global_context())).result.unwrap() ["devices"][&key]["laser"].clone();
    state.lock().unwrap().commands.clear();
    state.lock().unwrap().block = Some("OUTP:STAT?".into());
    clock.wait(Duration::from_secs(1));
    let refresh = scheduler.submit(request("action", json!({
        "name": "read_status",
        "args": {}
    }), ctx)).unwrap();
    until(|| state.lock().unwrap().commands.iter().any(|(_, v)| v == "OUTP:STAT?"));
    clock.wait(Duration::from_secs(3));
    let blocked = submit_wait(&scheduler, request("status", json!({}), b.global_context())).result.unwrap();
    assert_eq!(blocked["devices"][&key]["laser"], first);
    assert_eq!(blocked["devices"][&key]["sample_age_s"], 4.0);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let completed = refresh.wait(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap();
    refresh.acknowledge();
    assert_eq!(completed.phase, yang_protocol::Phase::Completed);
    assert_eq!(completed.result.unwrap() ["status"]["laser"]["received_at"], 1.0);
    scheduler.begin_shutdown();
    scheduler.join_when_released(yang_drivers::transport::Deadline::after(Duration::from_secs(2))).unwrap();
    b.finish_shutdown().unwrap();
}
#[test]
fn unknown_head_composite_stop_allowances_and_no_confirmation_preserve_commands() {
    let (f, state, _, _) = factory(Duration::from_secs(1));
    state.lock().unwrap().head = "6722-CUSTOM".into();
    let mut c = config('a', "1012");
    c.expected_identity["head_model"] = json!("6722-CUSTOM");
    let ctx = context(&c);
    let mut d = f.create(&c).unwrap();
    d.connect().unwrap();
    state.lock().unwrap().commands.clear();
    for(name, args) in[("set_target_wavelength", json!({
        "wavelength_nm": 1060.5,
        "confirm": true
    })),
    ("control_output", json!({
        "enabled": true,
        "confirm": true
    })),("control_tracking", json!({
        "enabled": true,
        "confirm": true
    })),
    ("set_remote", json!({
        "remote": true,
        "confirm": true
    })),("control_output", json!({
        "enabled": false,
        "confirm": false
    }))] {
        assert_eq!(d.action(name, &args, &ctx).phase, yang_protocol::Phase::RejectedBeforeCall);
    }
    assert!(state.lock().unwrap().commands.is_empty());
    for(name, args) in[("control_output", json!({
        "enabled": false,
        "confirm": true
    })),("control_tracking", json!({
        "enabled": false,
        "confirm": true
    })),("stop_scan", json!({
        "confirm": true
    }))] {
        assert_eq!(d.action(name, &args, &ctx).phase, yang_protocol::Phase::Completed);
    }
    assert!(d.close().unwrap().resources_released());
    let writes = state.lock().unwrap().commands.iter().filter(|(_, v)| v.contains(' ') || v.starts_with("OUTP:SCAN:")).map(|(_, v)| v.clone()).collect::<Vec<_>>();
    assert_eq!(writes, ["SYST:MCONT REM", "OUTP:STAT 0", "SYST:MCONT LOC", "OUTP:TRAC 0", "SYST:MCONT REM", "OUTP:SCAN:STOP", "OUTP:TRAC 0", "SYST:MCONT LOC"]);
}

fn config(id: char, serial: &str) -> DomainConfig {
    DomainConfig {
        domain: DomainRef {
            kind: "device".into(),
            id: id.to_string().repeat(32)
        },
        config_rev: 1,
        driver_kind: "laser".into(),
        model_id: "tlb6700".into(),
        profile_id: Some("newport-usb".into()),
        params: json!({
            "device_key": format!("6700 SN{serial}")
        }),
        expected_identity: json!({
            "model": "TLB-6700",
            "serial": serial,
            "head_model": "6722-P",
            "head_serial": format!("P{serial}")
        }),
        members: vec![],
    }
}

#[test]
fn laser_catalog_claims_and_limits_are_bound_to_exact_controller() {
    let c = config('a', "1012");
    assert!(catalog::admit(&c).is_ok());
    let claims = catalog::Claims::default();
    let _owner = claims.reserve(&c).unwrap();
    assert!(claims.reserve(&config('b', "1012")).is_err());
    assert!(claims.reserve(&config('b', "1013")).is_ok());
    for params in[json!({
        "device_key": "6700 sn1012"
    }), json!({
        "device_key": "6700 SN1012 "
    }),
    json!({
        "device_key": "6700 SN1012",
        "operating_min_nm": 1050
    }),
    json!({
        "device_key": "6700 SN1012",
        "operating_min_nm": 1044,
        "operating_max_nm": 1080,
        "scan_speed_limit_nm_s": 1
    })] {
        let mut bad = c.clone();
        bad.params = params;
        assert!(catalog::admit(&bad).is_err());
    }
    let mut bad = c;
    bad.expected_identity["serial"] = json!("1013");
    assert!(catalog::admit(&bad).is_err());
}

#[test]
fn laser_registry_allows_only_released_limit_revision_changes() {
    let registry = DomainRegistry::new(&"d".repeat(32)).unwrap();
    let c = catalog::admit(&config('a', "1012")).unwrap();
    registry.configure(c.clone()).unwrap();
    let mut revised = c.clone();
    revised.config_rev = 2;
    revised.params = json!({
        "device_key": "6700 SN1012",
        "operating_min_nm": 1059,
        "operating_max_nm": 1062,
        "scan_speed_limit_nm_s": 1
    });
    registry.configure(revised.clone()).unwrap();
    assert!(registry.configure(c).is_err());
    let mut rebound = revised.clone();
    rebound.config_rev = 3;
    rebound.params["device_key"] = json!("6700 SN1013");
    assert!(registry.configure(rebound).is_err());
    registry.bind(&revised.domain, &"e".repeat(32)).unwrap();
    revised.config_rev = 3;
    assert!(registry.configure(revised).is_err());
}

#[test]
fn fixed_laser_actions_require_exact_confirmed_typed_arguments() {
    for(name, args) in[("read_status", json!({})),("read_motion", json!({})),
    ("set_remote", json!({
        "remote": true,
        "confirm": true
    })),
    ("set_output", json!({
        "enabled": false,
        "confirm": true
    })),
    ("control_tracking", json!({
        "enabled": false,
        "confirm": true
    })),
    ("set_target_wavelength", json!({
        "wavelength_nm": 1060.1,
        "confirm": true
    })),
    ("start_scan", json!({
        "start_nm": 1060,
        "stop_nm": 1061,
        "speed_nm_s": 1,
        "return_speed_nm_s": 0.5,
        "confirm": true
    })),
    ("stop_scan", json!({
        "confirm": true
    }))] {
        assert!(actions::parse("laser", name, &args).is_ok(), "{name}");
    }
    for args in[json!({
        "wavelength_nm": 1060
    }), json!({
        "wavelength_nm": 1060,
        "confirm": false
    }),
    json!({
        "wavelength_nm": true,
        "confirm": true
    }), json!({
        "wavelength_nm": 1060,
        "confirm": 1
    }),
    json!({
        "wavelength_nm": 1060,
        "confirm": true,
        "raw": "*RST"
    })] {
        assert!(actions::parse("laser", "set_target_wavelength", &args).is_err());
    }
    assert!(actions::parse("laser", "raw", &json!({
        "confirm": true
    })).is_err());
}

#[test]
fn single_pass_scan_actions_require_exact_endpoint_velocity_and_consent() {
    for name in ["scan_forward", "scan_backward"] {
        assert!(actions::parse("laser", name, &json!({"target_nm":1061,"speed_nm_s":0.5,"confirm":true})).is_ok());
        for args in [json!({"target_nm":1061,"speed_nm_s":0.5}),json!({"target_nm":1061,"speed_nm_s":0.5,"confirm":false}),json!({"target_nm":1061,"speed_nm_s":0,"confirm":true}),json!({"target_nm":true,"speed_nm_s":0.5,"confirm":true}),json!({"target_nm":1061,"speed_nm_s":0.5,"confirm":true,"raw":"*RST"})] {
            assert!(actions::parse("laser", name, &args).is_err());
        }
    }
}

#[test]
fn laser_wire_scan_is_global_empty_and_laser_config_is_admitted() {
    let global = ContextV3 {
        session_id: "d".repeat(32),
        domain: None,
        connection_id: None,
        epoch: 0
    };
    let encode = | method: &str,
    params: Value,
    context: Value | serde_json::to_vec(&json!({
        "v": 3,
        "id": "q1",
        "method": method,
        "params": params,
        "context": context
    })).unwrap();
    assert!(yang_protocol::parse_request(&encode("configure_domain", json!({
        "config": config('a', "1012")
    }), json!(global))).is_ok());
    assert!(yang_protocol::parse_request(&encode("scan_lasers", json!({}), json!(global))).is_ok());
    assert!(yang_protocol::parse_request(&encode("scan_lasers", json!({
        "key": "6700 SN1012"
    }), json!(global))).is_err());
    assert!(yang_protocol::parse_request(&encode("scan_lasers", json!({}), json!({
        "session_id": "d".repeat(32),
        "domain": config('a', "1012").domain,
        "connection_id": null,
        "epoch": 0
    }))).is_err());
}
