#[path = "support/gain_wire.rs"]
mod wire;
use std::{sync::mpsc, time::{Duration, Instant}};
use yang_drivers::{clock::Clock,lifecycle::DriverState, DriverError};
#[test]
fn cancellation_during_cached_preflight_cannot_become_a_new_ramp_intent() {
    use std::sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}};
    use yang_drivers::{clock::{Clock,ManualClock},gain::{GainDriver,GainConfig,StopHandle},transport::ResourceBook};
    struct CancelClock {clock:Arc<ManualClock>,stop:Mutex<Option<StopHandle>>,armed:AtomicBool}
    impl Clock for CancelClock {
        fn now(&self)->Duration {
            let now=self.clock.now();
            if self.armed.swap(false,Ordering::AcqRel) {self.stop.lock().unwrap().as_ref().unwrap().cancel_operation();}
            now
        }
        fn wait(&self,duration:Duration){self.clock.wait(duration);}
    }
    let peer=wire::Peer::new();let clock=Arc::new(CancelClock {clock:peer.clock.clone(),stop:Mutex::new(None),armed:AtomicBool::new(false)});
    let mut driver=GainDriver::with_backend(GainConfig {port:Some("COM13".into()),..Default::default()},ResourceBook::isolated(),clock.clone(),Arc::new(wire::Backend(peer.clone()))).unwrap();
    driver.connect().unwrap();driver.enable_tec().unwrap();wire::stable(&peer,&driver);driver.enable_current().unwrap();
    *clock.stop.lock().unwrap()=Some(driver.stop_handle());clock.armed.store(true,Ordering::Release);
    let before=peer.data.lock().unwrap().writes.len();let result=driver.ramp_current(4.,1.,Duration::from_millis(50));let after=peer.data.lock().unwrap().writes.len();
    driver.close().unwrap();assert_eq!(result,Err(DriverError::Canceled));assert_eq!(after,before,"the interrupted ramp must not adopt a new safety generation");
}

#[test]
fn startup_rechecks_remaining_ramp_budget_and_shuts_down_after_enable() {
    let peer=wire::Peer::new();let mut driver=wire::driver(&peer);
    driver.connect().unwrap();driver.enable_tec().unwrap();wire::stable(&peer,&driver);
    peer.data.lock().unwrap().reply_clock_jumps.push_back(("STQA000001".into(),Duration::from_millis(800)));
    let result=driver.start_current(6.,true,1.,Duration::from_millis(100),Duration::from_secs(1));
    let invalid=matches!(result,Err(DriverError::Invalid(_)));
    if !invalid { wire::until(||driver.state()==DriverState::Fault&&!peer.data.lock().unwrap().enabled); }
    let enabled=peer.data.lock().unwrap().enabled;driver.close().unwrap();
    assert!(!invalid,"an incomplete ramp after Q=1 is an output liability, not an input rejection");
    assert!(!enabled,"handled startup failure must turn current off");
}
#[test]
fn direct_start_reads_reset_current_then_verifies_the_requested_target() {
    let peer=wire::Peer::new();let mut driver=wire::driver(&peer);
    driver.connect().unwrap();driver.enable_tec().unwrap();wire::stable(&peer,&driver);
    let before=peer.data.lock().unwrap().writes.len();
    assert_eq!(driver.start_current(8.,false,1.,Duration::from_millis(100),Duration::from_secs(1)).unwrap(),8.);
    let writes=peer.data.lock().unwrap().writes[before..].to_vec();
    assert_eq!(writes.iter().filter(|(_,w)|w.starts_with(b"STCA")).count(),1);
    assert_eq!(driver.status().unwrap().current_ma,8.);assert!(driver.status().unwrap().current_enabled);
    driver.close().unwrap();
}
#[test]
fn standalone_ramp_rejects_a_plan_longer_than_three_minutes_before_writing() {
    let peer=wire::Peer::new();let mut driver=wire::driver(&peer);
    driver.connect().unwrap();driver.enable_tec().unwrap();wire::stable(&peer,&driver);driver.enable_current().unwrap();
    let before=peer.data.lock().unwrap().writes.len();
    let result=driver.ramp_current(200.,0.001,Duration::from_secs(180));
    let after=peer.data.lock().unwrap().writes.len();driver.close().unwrap();
    assert!(matches!(result,Err(DriverError::Invalid(_))));assert_eq!(after,before);
}
#[test]
fn old_cache_handle_never_returns_pid_or_status_from_a_reconnected_driver() {
    let peer=wire::Peer::new();let mut driver=wire::driver(&peer);driver.connect().unwrap();driver.read_pid().unwrap();
    let old=driver.cache_handle();assert!(old.snapshot().unwrap().pid.is_some());driver.close().unwrap();driver.connect().unwrap();
    assert!(old.snapshot().is_none());assert!(driver.cache_handle().snapshot().unwrap().pid.is_none());driver.close().unwrap();
}
#[test]
fn stalled_start_is_bounded_retains_owner_and_never_replays_enable() {
    let peer=wire::Peer::new();let mut driver=wire::driver(&peer);driver.connect().unwrap();driver.enable_tec().unwrap();wire::stable(&peer,&driver);
    {let mut data=peer.data.lock().unwrap();data.hold=true;data.whole_reply=true;}
    let (tx,rx)=mpsc::channel();let started=Instant::now();
    let pending=std::thread::spawn(move||{let result=driver.start_current(3.,true,1.,Duration::from_millis(100),Duration::from_millis(100));tx.send((driver,result)).unwrap();});
    peer.held();let (mut driver,result)=rx.recv_timeout(Duration::from_secs(1)).expect("total budget must bound a stalled native call");
    assert!(started.elapsed()<Duration::from_secs(1));assert!(matches!(result,Err(DriverError::Timeout{..})));assert!(driver.has_resource_responsibility());
    assert!(peer.data.lock().unwrap().writes.iter().all(|(_,w)|w!=b"STQA000001\r\n"));
    peer.release();pending.join().unwrap();wire::until(||driver.state()==DriverState::Fault);driver.close().unwrap();
    assert_eq!(peer.data.lock().unwrap().opens,1);
}
#[test]
fn startup_bad_acknowledgements_fault_and_shutdown_current_before_tec() {
    for (prefix,reply) in [("STQA000001",b"READY;Q=0\r\n".as_slice()),("STCA008000",b"READY;C=7.000\r\n".as_slice())] {
        let peer=wire::Peer::new();let mut driver=wire::driver(&peer);
        driver.connect().unwrap();driver.enable_tec().unwrap();wire::stable(&peer,&driver);
        let before=peer.data.lock().unwrap().writes.len();peer.data.lock().unwrap().faults.push_back((prefix.into(),reply.to_vec()));
        let result=driver.start_current(8.,false,1.,Duration::from_millis(100),Duration::from_secs(1));
        wire::until(||driver.state()==DriverState::Fault&&!peer.data.lock().unwrap().tec);
        let cache=driver.cache_handle().snapshot().unwrap();let writes=peer.data.lock().unwrap().writes[before..].to_vec();driver.close().unwrap();
        assert!(result.is_err());assert_eq!(cache.current_operation.unwrap().phase,"failed");
        let q=writes.iter().rposition(|(_,w)|w==b"STQA000000\r\n").unwrap();let r=writes.iter().rposition(|(_,w)|w==b"STRA000000\r\n").unwrap();assert!(q<r);
    }
}
#[test]
fn partial_pid_failure_never_renews_the_previous_confirmed_pid_cache() {
    let peer=wire::Peer::new();let mut driver=wire::driver(&peer);driver.connect().unwrap();driver.read_pid().unwrap();
    let before=driver.cache_handle().snapshot().unwrap().pid.unwrap();peer.clock.wait(Duration::from_millis(200));
    peer.data.lock().unwrap().faults.push_back(("STIA".into(),b"READY;I=broken\r\n".to_vec()));
    assert!(driver.set_pid(0.8,0.2,0.01).is_err());
    let after=driver.cache_handle().snapshot().unwrap().pid.unwrap();driver.close().unwrap();
    assert_eq!(after.values,before.values);assert_eq!(after.revision,before.revision);assert_eq!(after.received_at,before.received_at);
}
