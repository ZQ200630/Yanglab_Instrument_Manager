#[path = "../../../Code/Utils/tests/support/gain_wire.rs"]
mod gain;
#[path = "../../../Code/Utils/tests/support/osa_wire.rs"]
mod osa;
#[path = "../../../Code/Utils/tests/support/pm_wire.rs"]
mod pm;
#[path = "support/native.rs"]
mod support;
#[path = "../../../Code/Utils/tests/support/voltage_wire.rs"]
mod voltage;
use serde_json::json;
use std::{sync::{Arc, Condvar, Mutex}, time::Duration};
use yang_drivers::{
    clock::{Clock, ManualClock},
    lifecycle::{CleanupReport, CleanupStep, DriverState},
    transport::{
        serial_abi::{SerialBackend, SerialIo},
        serial_discovery::DeviceRecord,
        visa_abi::VisaApi,
        Deadline,
    },
    DriverResult,
};
use yang_protocol::{ContextV3, DomainConfig, DomainRef, Limits, OutcomeV3, Phase, RequestV3};
use yang_worker::{
    observations::Observation,
    scheduler::{Backend, Scheduler},
    session::{DriverFactory, SystemFactory},
    DomainRegistry,
};
// Explicit cache-only observer: no serial/VISA, driver or device reads are available.
struct CacheCadenceBackend {
    registry: DomainRegistry,
    clock: Arc<ManualClock>,
    observations: (Mutex<Vec<(String, Duration)>>, Condvar),
}
impl CacheCadenceBackend {
    fn new() -> Arc<Self> {
        let registry = DomainRegistry::new(&"a".repeat(32)).unwrap();
        for (number, kind) in [(1, "gain"), (2, "voltage"), (3, "pm400")] {
            let cfg = config(kind, number);
            registry.configure(cfg.clone()).unwrap();
            let ctx = registry.bind(&cfg.domain, &format!("{number:032x}")).unwrap();
            registry.publish(&ctx, DriverState::Ready);
        }
        Arc::new(Self { registry, clock: Arc::new(ManualClock::default()), observations: (Mutex::new(vec![]), Condvar::new()) })
    }
    fn count(&self, kind: &str) -> usize {
        self.observations.0.lock().unwrap().iter().filter(|(driver, _)| driver == kind).count()
    }
    fn wait_count(&self, kind: &str, count: usize) -> bool {
        let observations = self.observations.0.lock().unwrap();
        let (observations, _) = self.observations.1.wait_timeout_while(observations, Duration::from_secs(2), |items| items.iter().filter(|(driver, _)| driver == kind).count() < count).unwrap();
        observations.iter().filter(|(driver, _)| driver == kind).count() >= count
    }
}
impl Backend for CacheCadenceBackend {
    fn registry(&self) -> DomainRegistry { self.registry.clone() }
    fn has_cached_observer(&self, context: &ContextV3) -> bool {
        matches!(self.registry.config(context.domain.as_ref().unwrap()).unwrap().driver_kind.as_str(), "gain" | "voltage")
    }
    fn execute(&self, request: &RequestV3) -> OutcomeV3 {
        let kind = self.registry.config(request.context.as_ref().unwrap().domain.as_ref().unwrap()).unwrap().driver_kind;
        let result = if request.method == "disconnect" {
            let cleanup = CleanupReport::new(yang_worker::new_id().unwrap(), vec![CleanupStep { role: kind, action: "bounded_cache_close".into(), error: None }], None, vec![]).unwrap();
            json!({"connected":false,"cleanup":cleanup})
        } else {
            assert_eq!(request.method, "connect", "the bounded fixture supports no instrument actions");
            json!({"connected":true,"status":{"state":"READY"}})
        };
        OutcomeV3 { phase: Phase::Completed, context: request.context.clone(), result: Some(result), error: None }
    }
    fn observe(&self, context: &ContextV3) -> Observation {
        let driver = self.registry.config(context.domain.as_ref().unwrap()).unwrap().driver_kind;
        self.observations.0.lock().unwrap().push((driver, self.clock.now()));
        self.observations.1.notify_all();
        Observation { status: json!({"state":"READY"}), more: false, sampled_at: Some(self.clock.now()) }
    }
}
#[test]
fn gain_cache_publication_is_200_ms_and_other_observer_cadences_are_unchanged() {
    let backend = CacheCadenceBackend::new();
    let scheduler = Scheduler::new(backend.clone(), backend.clock.clone(), Limits::default()).unwrap();
    backend.clock.wait(Duration::from_millis(2500));
    let initial = ["gain", "voltage", "pm400"].map(|kind| backend.wait_count(kind, 1));
    backend.clock.wait(Duration::from_millis(200));
    let gain_due = backend.wait_count("gain", 2);
    let at_200 = ["gain", "voltage", "pm400"].map(|kind| backend.count(kind));
    backend.clock.wait(Duration::from_millis(50));
    let other_cache_due = backend.wait_count("voltage", 2);
    let at_250 = ["gain", "voltage", "pm400"].map(|kind| backend.count(kind));
    backend.clock.wait(Duration::from_millis(2250));
    let normal_due = backend.wait_count("pm400", 2);
    let timestamps = backend.observations.0.lock().unwrap().clone();
    scheduler.begin_shutdown();scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();
    assert_eq!(initial, [true; 3]);assert!(gain_due, "Gain cache must publish the new observation at 200 ms");
    assert_eq!(at_200, [2, 1, 1]);assert!(other_cache_due);assert_eq!(at_250, [2, 2, 1]);assert!(normal_due);
    let times = |kind: &str| timestamps.iter().filter(|(driver, _)| driver == kind).map(|(_, at)| *at).collect::<Vec<_>>();
    assert_eq!(times("gain")[1]-times("gain")[0], Duration::from_millis(200));
    assert_eq!(times("voltage")[1]-times("voltage")[0], Duration::from_millis(250));
    assert_eq!(times("pm400")[1]-times("pm400")[0], Duration::from_millis(2500));
}
#[test]
fn an_existing_gain_cache_is_first_published_within_200_ms() {
    let backend = CacheCadenceBackend::new();
    let scheduler = Scheduler::new(backend.clone(), backend.clock.clone(), Limits::default()).unwrap();
    backend.clock.wait(Duration::from_millis(200));
    let ready = backend.wait_count("gain", 1);
    let observations = backend.observations.0.lock().unwrap().clone();
    scheduler.begin_shutdown();scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();
    assert!(ready, "an existing healthy Gain cache must not wait 2.5 seconds for its first publication");
    assert_eq!(observations, vec![("gain".into(), Duration::from_millis(200))]);
}
#[test]
fn normal_gain_connect_refreshes_the_cache_without_waiting_for_a_poll_deadline() {
    let backend = CacheCadenceBackend::new();
    let old = backend.registry.context(&config("gain", 1).domain).unwrap();
    let cleanup = CleanupReport::new(yang_worker::new_id().unwrap(), vec![CleanupStep { role: "gain".into(), action: "bounded_cache_close".into(), error: None }], None, vec![]).unwrap();
    backend.registry.release(&old, &cleanup).unwrap();
    let scheduler = Scheduler::new(backend.clone(), backend.clock.clone(), Limits::default()).unwrap();
    let context = backend.registry.context(&config("gain", 1).domain).unwrap();
    let result = scheduler.submit(support::request("gain-cache-connect", "connect", json!({}), Some(context))).unwrap().wait(Deadline::after(Duration::from_secs(2))).unwrap();
    let ready = backend.wait_count("gain", 1);
    let observations = backend.observations.0.lock().unwrap().clone();
    scheduler.begin_shutdown();scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();
    assert_eq!(result.phase, Phase::Completed);assert!(ready);
    assert!(!observations.is_empty());assert!(observations.iter().all(|(driver, at)| driver=="gain" && *at==Duration::ZERO));
}
struct Serial {
    g: Arc<gain::Peer>,
    v: Arc<voltage::Peer>,
}
impl SerialBackend for Serial {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        panic!("no enumeration in explicit-bound tests")
    }
    fn open(&self, p: &str) -> DriverResult<Box<dyn SerialIo>> {
        match p {
            "\\\\.\\COM12" => voltage::Backend(self.v.clone()).open(p),
            "\\\\.\\COM13" => gain::Backend(self.g.clone()).open(p),
            _ => panic!("unexpected serial"),
        }
    }
}
fn config(kind: &str, n: u32) -> DomainConfig {
    DomainConfig {
        domain: DomainRef {
            kind: "device".into(),
            id: format!("{n:032x}"),
        },
        config_rev: 1,
        driver_kind: kind.into(),
        model_id: match kind {
            "osa" => "aq6370",
            "pm400" => "pm400",
            _ => kind,
        }
        .into(),
        profile_id: Some(
            match kind {
                "gain" => "cp210x-serial",
                "voltage" => "ch340-serial",
                "osa" => "gpib-visa",
                _ => "usb-visa",
            }
            .into(),
        ),
        params: match kind {
            "gain" => json!({"port":"COM13"}),
            "voltage" => json!({"port":"COM12"}),
            "osa" => json!({"resource":"GPIB0::4::INSTR"}),
            _ => json!({"resource":"USB0::0x1313::0x8075::P1::INSTR"}),
        },
        expected_identity: json!({}),
        members: vec![],
    }
}
fn context(c: &DomainConfig) -> ContextV3 {
    ContextV3 {
        session_id: "a".repeat(32),
        domain: Some(c.domain.clone()),
        connection_id: Some("b".repeat(32)),
        epoch: 1,
    }
}
fn factory(
    clock: Arc<dyn Clock>,
    api: Arc<dyn VisaApi>,
) -> (Arc<SystemFactory>, Arc<gain::Peer>, Arc<voltage::Peer>) {
    let g = gain::Peer::new();
    let v = voltage::Peer::new();
    (
        Arc::new(SystemFactory::with_backends(
            clock,
            Arc::new(Serial {
                g: g.clone(),
                v: v.clone(),
            }),
            api,
        )),
        g,
        v,
    )
}
#[test]
fn telemetry_freshness_units_and_cleanup_are_truthful() {
    let g = gain::Peer::new();
    let v = voltage::Peer::new();
    let factory = SystemFactory::with_backends(
        g.clock.clone(),
        Arc::new(Serial { g: g.clone(), v }),
        pm::Wire::new(1),
    );
    let c = config("gain", 1);
    let mut s = factory.create(&c).unwrap();
    s.connect().unwrap();
    let ctx = context(&c);
    let fresh = s.observe(&ctx);
    assert_eq!(fresh.status["fields"]["temperature_c"]["value"], 22.0);
    assert_eq!(fresh.status["fields"]["temperature_c"]["quality"], "fresh");
    assert_eq!(
        fresh.status["fields"]["temperature_c"]["connection_id"],
        ctx.connection_id.as_ref().unwrap().as_str()
    );
    assert_eq!(fresh.status["temperature_unit"], "degC");
    assert_eq!(fresh.status["current_unit"], "mA");
    s.stop_signal().request_stop();
    let unknown = s.observe(&ctx);
    assert_eq!(
        unknown.status["fields"]["current_enabled"]["quality"],
        "unknown"
    );
    let receipt = s.close().unwrap();
    assert!(receipt.resources_released());
    assert!(receipt.voltage_zero().is_none());
    assert_eq!(s.state(), DriverState::Disconnected);
}
#[test]
fn explicit_readonly_probes_never_run_startup_writes() {
    let clock = Arc::new(ManualClock::default());
    let (f, g, v) = factory(clock, pm::Wire::new(1));
    for (kind, n) in [("gain", 1), ("voltage", 2)] {
        let mut s = f.create(&config(kind, n)).unwrap();
        let report = s.probe_readonly().unwrap();
        assert!(report.release_confirmed());
        assert!(!s.has_responsibility());
    }
    assert!(g
        .data
        .lock()
        .unwrap()
        .writes
        .iter()
        .all(|(_, w)| w.starts_with(b"RD")));
    assert!(v.data.lock().unwrap().writes.is_empty());
}
#[test]
fn pm_results_match_shared_gui_scalar_contract() {
    let clock = Arc::new(ManualClock::default());
    let (f, _, _) = factory(clock, pm::Wire::new(1));
    let c = config("pm400", 1);
    let ctx = context(&c);
    let mut s = f.create(&c).unwrap();
    s.connect().unwrap();
    let r = s.action("measure_kind", &json!({"kind":"power"}), &ctx);
    assert_eq!(r.phase, Phase::Completed);
    let result = r.result.unwrap();
    assert_eq!(result["result"]["value"], 1.25);
    assert_eq!(result["result"]["unit"], "W");
    assert_eq!(result["result"]["kind"], "power");
    s.close().unwrap();
}
#[test]
fn supervised_proof_retains_the_existing_session_without_claiming_release() {
    let clock = Arc::new(yang_drivers::clock::SystemClock::default());
    let (f, g, _) = factory(clock.clone(), pm::Wire::new(1));
    let b = backend(f, clock);
    let c = config("gain", 1);
    let ctx = supervised_connect(&b, c.clone());
    let before = g.data.lock().unwrap().writes.len();
    let r=b.execute(&support::request("proof","probe",json!({"authorization":{"stage":"supervised","accepted":true,"supervised":true,"retain_session":true,"binding":{"mode":"real","domain":c.domain,"config_rev":1,"model_id":c.model_id,"profile_id":c.profile_id,"config_digest":"d".repeat(64),"controller":"e".repeat(32)}}}),Some(ctx.clone())));
    assert_eq!(r.phase, Phase::Completed, "{:?}", r.error);
    let result = r.result.unwrap();
    assert_eq!(result["proof"]["retained_session"], true);
    assert_eq!(result["release_confirmed"], false);
    assert_eq!(g.data.lock().unwrap().writes.len(), before);
    b.execute(&support::request(
        "close",
        "disconnect",
        json!({}),
        Some(ctx),
    ));
}
fn gain_backend() -> (Arc<yang_worker::backend::NativeBackend>, Arc<gain::Peer>) {
    let g = gain::Peer::new();
    let clock = g.clock.clone();
    let f = Arc::new(SystemFactory::with_backends(
        clock.clone(),
        Arc::new(Serial { g: g.clone(), v: voltage::Peer::new() }),
        pm::Wire::new(1),
    ));
    (backend(f, clock), g)
}
#[test]
fn gain_pid_actions_publish_only_observed_coefficients_and_independent_age() {
    let (b,g)=gain_backend();let c=config("gain",1);let ctx=supervised_connect(&b,c);
    assert!(b.observe(&ctx).status["pid"].is_null());
    let read=b.execute(&support::request("pid-read","action",json!({"name":"read_pid","args":{}}),Some(ctx.clone())));
    assert_eq!(read.phase,Phase::Completed,"{:?}",read.error);
    assert_eq!(b.observe(&ctx).status["pid"]["values"],json!([0.35,0.1,0.]));
    let applied=b.execute(&support::request("pid-set","action",json!({"name":"set_pid","args":{"p":0.8,"i":0.2,"d":0.01}}),Some(ctx.clone())));
    assert_eq!(applied.phase,Phase::Completed,"{:?}",applied.error);
    let first=b.observe(&ctx).status["pid"].clone();
    assert_eq!(first["values"],json!([0.8,0.2,0.01]));assert_eq!(first["connection_id"],ctx.connection_id.as_ref().unwrap().as_str());
    g.clock.wait(Duration::from_secs(1));gain::until(|| b.observe(&ctx).status["fields"]["temperature_c"]["revision"]!=json!(1));
    let later=b.observe(&ctx).status["pid"].clone();assert_eq!(later["revision"],first["revision"]);
    assert!(later["observed_age_s"].as_f64().unwrap()>=1.0,"thermal polling must not refresh PID evidence");
    b.execute(&support::request("close","disconnect",json!({}),Some(ctx)));
}
#[test]
fn gain_new_action_schemas_reject_invalid_values_before_any_io() {
    use yang_worker::actions::parse;
    for (name,args) in [("read_pid",json!({})),("set_pid",json!({"p":999.999,"i":0.,"d":1.})),
        ("ramp_current",json!({"current_ma":200.,"step_ma":0.001,"interval_s":0.05})),("start_current",json!({"current_ma":6.}))] {
        assert!(parse("gain",name,&args).is_ok(),"{name} valid contract missing");
    }
    for (name,args) in [("read_pid",json!({"force":true})),("set_pid",json!({"p":1000.,"i":0.,"d":0.})),
        ("set_pid",json!({"p":0.1,"i":0.})),("ramp_current",json!({"current_ma":4.,"step_ma":1.001,"interval_s":0.05})),
        ("ramp_current",json!({"current_ma":4.,"step_ma":1.,"interval_s":0.049})),("start_current",json!({"current_ma":201.})),
        ("start_current",json!({"current_ma":4.,"soft_start":1})),("start_current",json!({"current_ma":4.,"timeout_s":180.001})),
        ("start_current",json!({"current_ma":4.,"unexpected":true}))] {assert!(parse("gain",name,&args).is_err(),"{name} accepted {args}");}
}
fn gain_action(b:&yang_worker::backend::NativeBackend,ctx:&ContextV3,name:&str,args:serde_json::Value)->yang_protocol::OutcomeV3 {
    b.execute(&support::request(&yang_worker::new_id().unwrap(),"action",json!({"name":name,"args":args}),Some(ctx.clone())))
}
fn gain_stable(b:&yang_worker::backend::NativeBackend,g:&gain::Peer,ctx:&ContextV3) {
    assert_eq!(gain_action(b,ctx,"enable_tec",json!({})).phase,Phase::Completed);
    for _ in 0..6 {
        let at=b.observe(ctx).status["last_status"]["received_at"].clone();g.clock.wait(Duration::from_secs(1));
        gain::until(|| b.observe(ctx).status["last_status"]["received_at"]!=at);
        assert_eq!(gain_action(b,ctx,"read_pid",json!({})).phase,Phase::Completed);
    }
}
#[test]
fn gain_start_uses_reset_readback_and_defaults_to_bounded_soft_start() {
    let (b,g)=gain_backend();let ctx=supervised_connect(&b,config("gain",1));gain_stable(&b,&g,&ctx);
    let before=g.data.lock().unwrap().writes.len();
    let result=gain_action(&b,&ctx,"start_current",json!({"current_ma":6.}));
    assert_eq!(result.phase,Phase::Completed,"{:?}",result.error);
    let writes=g.data.lock().unwrap().writes[before..].to_vec();
    let steps:Vec<_>=writes.iter().filter(|(_,w)|w.starts_with(b"STCA")).collect();
    assert_eq!(steps.iter().map(|(_,w)|w.as_slice()).collect::<Vec<_>>(),vec![b"STCA004000\r\n".as_slice(),b"STCA005000\r\n".as_slice(),b"STCA006000\r\n".as_slice()]);
    assert!(steps.windows(2).all(|v|v[1].0-v[0].0>=Duration::from_millis(100)));
    let observed=b.observe(&ctx).status;assert_eq!(observed["current_operation"]["phase"],"completed");
    assert_eq!(observed["current_operation"]["steps_completed"],3);assert_eq!(observed["current_operation"]["steps_total"],3);
    assert_eq!(observed["last_status"]["current_ma"],6.);assert_eq!(observed["last_status"]["current_enabled"],true);
    let elapsed=observed["current_operation"]["elapsed_s"].clone();g.clock.wait(Duration::from_secs(1));
    assert_eq!(b.observe(&ctx).status["current_operation"]["elapsed_s"],elapsed,"completed operation elapsed time must stop");
    b.execute(&support::request("close","disconnect",json!({}),Some(ctx)));
}
#[test]
fn gain_start_requires_tec_and_rejects_impossible_ramp_before_q_on() {
    let (b,g)=gain_backend();let ctx=supervised_connect(&b,config("gain",1));
    let no_tec=gain_action(&b,&ctx,"start_current",json!({"current_ma":6.}));assert_eq!(no_tec.phase,Phase::FailedAfterCallStarted);
    gain_stable(&b,&g,&ctx);
    let impossible=gain_action(&b,&ctx,"start_current",json!({"current_ma":200.,"step_ma":0.001,"interval_s":180.}));
    assert_eq!(impossible.phase,Phase::FailedAfterCallStarted);
    assert!(g.data.lock().unwrap().writes.iter().all(|(_,w)|w!=b"STQA000001\r\n"));
    b.execute(&support::request("close","disconnect",json!({}),Some(ctx)));
}
#[test]
fn gain_observation_remains_live_without_io_while_action_slot_is_held() {
    let (b,g)=gain_backend();let ctx=supervised_connect(&b,config("gain",1));g.data.lock().unwrap().hold=true;
    let action_backend=b.clone();let action_context=ctx.clone();
    let pending=std::thread::spawn(move||gain_action(&action_backend,&action_context,"set_temperature",json!({"temperature_c":23.})));
    g.held();let before=g.data.lock().unwrap().writes.len();
    let (tx,rx)=std::sync::mpsc::channel();let observed_backend=b.clone();let observed_context=ctx.clone();
    let observed=std::thread::spawn(move||tx.send(observed_backend.observe(&observed_context)).unwrap());
    let live=rx.recv_timeout(Duration::from_millis(150));let after=g.data.lock().unwrap().writes.len();
    g.release();pending.join().unwrap();observed.join().unwrap();b.execute(&support::request("close","disconnect",json!({}),Some(ctx)));
    assert!(live.is_ok(),"cached Gain observation must not wait for the native action slot");assert_eq!(after,before,"cache sampling must not emit serial requests");
}
#[test]
fn gain_start_wait_is_canceled_without_q_on_and_keeps_live_progress() {
    let (b,g)=gain_backend();let ctx=supervised_connect(&b,config("gain",1));
    assert_eq!(gain_action(&b,&ctx,"enable_tec",json!({})).phase,Phase::Completed);
    let action_backend=b.clone();let action_context=ctx.clone();
    let pending=std::thread::spawn(move||gain_action(&action_backend,&action_context,"start_current",json!({"current_ma":6.,"timeout_s":1.})));
    std::thread::sleep(Duration::from_millis(30));
    let (tx,rx)=std::sync::mpsc::channel();let observed_backend=b.clone();let observed_context=ctx.clone();
    let observed=std::thread::spawn(move||tx.send(observed_backend.observe(&observed_context)).unwrap());
    let live=rx.recv_timeout(Duration::from_millis(150));
    b.request_safety(&ctx,yang_worker::safety::SafetyIntent::CurrentOff);
    let result=pending.join().unwrap();observed.join().unwrap();
    let writes=g.data.lock().unwrap().writes.clone();b.execute(&support::request("close","disconnect",json!({}),Some(ctx)));
    assert_eq!(live.unwrap().status["current_operation"]["phase"],"waiting_stable");
    assert_eq!(result.error.unwrap().kind,"Canceled");assert!(writes.iter().all(|(_,w)|w!=b"STQA000001\r\n"));
}
#[test]
fn scheduler_original_generation_reaches_native_through_the_lazy_session() {
    // Cancel after scheduler dispatch, before NativeBackend enters the session.
    // Keep the registry context unchanged: this exercises the native intent fence,
    // rather than letting an outer stale-context check hide an adapter omission.
    struct CancelAtDispatch(Arc<yang_worker::backend::NativeBackend>);
    impl Backend for CancelAtDispatch {
        fn registry(&self)->yang_worker::DomainRegistry {self.0.registry()}
        fn execute(&self,r:&yang_protocol::RequestV3)->yang_protocol::OutcomeV3 {self.0.execute(r)}
        fn capture_action_fence(&self,r:&yang_protocol::RequestV3)->Option<u64> {self.0.capture_action_fence(r)}
        fn execute_fenced(&self,r:&yang_protocol::RequestV3,fence:Option<u64>)->yang_protocol::OutcomeV3 {
            if r.params["name"]=="start_current" {
                assert!(fence.is_some(),"scheduler must capture the original Gain generation");
                self.0.request_safety(r.context.as_ref().unwrap(),yang_worker::safety::SafetyIntent::CurrentOff);
            }
            self.0.execute_fenced(r,fence)
        }
        fn observe(&self,c:&ContextV3)->yang_worker::observations::Observation {self.0.observe(c)}
        fn has_cached_observer(&self,c:&ContextV3)->bool {self.0.has_cached_observer(c)}
        fn request_stop(&self,c:&ContextV3) {self.0.request_stop(c)}
        fn request_safety(&self,c:&ContextV3,i:yang_worker::safety::SafetyIntent) {self.0.request_safety(c,i)}
        fn begin_shutdown(&self) {self.0.begin_shutdown()}
        fn finish_shutdown(&self)->Result<(),yang_worker::WorkerError> {self.0.finish_shutdown()}
    }
    let (b,g)=gain_backend();let ctx=supervised_connect(&b,config("gain",1));gain_stable(&b,&g,&ctx);
    let scheduler=Scheduler::new(Arc::new(CancelAtDispatch(b)),g.clock.clone(),Limits::default()).unwrap();
    let result=scheduler.submit(support::request("cancel-before-native","action",json!({"name":"start_current","args":{"current_ma":6.}}),Some(ctx))).unwrap().wait(Deadline::after(Duration::from_secs(2))).unwrap();
    let enabled=g.data.lock().unwrap().writes.iter().any(|(_,w)|w==b"STQA000001\r\n");
    scheduler.begin_shutdown();scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();
    assert_eq!(result.phase,Phase::FailedAfterCallStarted);assert_eq!(result.error.unwrap().kind,"Canceled");
    assert!(!enabled,"a later generation must not become the dispatched startup intent");
}
#[test]
fn idle_gain_cache_poll_keeps_watchdog_samples_fresh_without_transport_io() {
    let (b,g)=gain_backend();let ctx=supervised_connect(&b,config("gain",1));
    let scheduler=Scheduler::new(b.clone(),g.clock.clone(),Limits::default()).unwrap();let key=format!("device:{}",ctx.domain.as_ref().unwrap().id);
    g.clock.wait(Duration::from_millis(2500));
    gain::until(||scheduler_status(&scheduler)["devices"][&key]["fields"]["temperature_c"]["revision"].is_number());
    let first=scheduler_status(&scheduler)["devices"][&key]["fields"]["temperature_c"]["revision"].clone();
    g.data.lock().unwrap().temperature=22.1;g.clock.wait(Duration::from_secs(1));
    gain::until(||b.observe(&ctx).status["last_status"]["temperature_c"]==22.1);
    // Settle the driver's monitor before counting pure scheduler/cache work.
    gain_action(&b,&ctx,"read_pid",json!({}));
    let before=g.data.lock().unwrap().writes.len();
    for _ in 0..10 { b.observe(&ctx); }
    let after=g.data.lock().unwrap().writes.len();
    // Advancing the driver clock may legitimately trigger its next serial sample;
    // only frozen-clock cached observations are used for the no-I/O assertion.
    g.clock.wait(Duration::from_millis(200));
    let end=std::time::Instant::now()+Duration::from_millis(300);
    while scheduler_status(&scheduler)["devices"][&key]["fields"]["temperature_c"]["revision"]==first && std::time::Instant::now()<end {std::thread::yield_now();}
    let status=scheduler_status(&scheduler);
    scheduler.begin_shutdown();scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();
    assert_ne!(status["devices"][&key]["fields"]["temperature_c"]["revision"],first,"idle Gain cache metadata must poll within 200 ms");
    assert_eq!(status["devices"][&key]["fields"]["temperature_c"]["quality"],"fresh");
    assert_eq!(after,before,"the faster cache cadence must not issue serial I/O");
}
fn scheduler_status(scheduler:&Scheduler)->serde_json::Value {
    scheduler.submit(support::request(&yang_worker::new_id().unwrap(),"status",json!({}),None)).unwrap().wait(Deadline::after(Duration::from_secs(1))).unwrap().result.unwrap()
}
#[test]
fn gain_scheduler_updates_cached_samples_while_stability_wait_is_active() {
    let (b,g)=gain_backend();let ctx=supervised_connect(&b,config("gain",1));gain_action(&b,&ctx,"enable_tec",json!({}));
    let scheduler=Scheduler::new(b.clone(),g.clock.clone(),Limits::default()).unwrap();let key=format!("device:{}",ctx.domain.as_ref().unwrap().id);
    let pending=scheduler.submit(support::request("wait-live","action",json!({"name":"start_current","args":{"current_ma":3.,"timeout_s":10.}}),Some(ctx.clone()))).unwrap();
    gain::until(|| b.observe(&ctx).status["current_operation"]["phase"]=="waiting_stable");
    g.data.lock().unwrap().temperature=22.1;g.clock.wait(Duration::from_millis(600));
    gain::until(||scheduler_status(&scheduler)["devices"][&key]["current_operation"]["phase"]=="waiting_stable");
    g.clock.wait(Duration::from_millis(600));
    gain::until(||b.observe(&ctx).status["last_status"]["temperature_c"]==22.1);
    g.clock.wait(Duration::from_millis(500));
    gain::until(||scheduler_status(&scheduler)["devices"][&key]["last_status"]["temperature_c"]==22.1);
    let status=scheduler_status(&scheduler);assert_eq!(status["domains"][&key]["active_request_id"],"wait-live");
    scheduler.submit(support::request("off-live","action",json!({"name":"disable_current","args":{}}),Some(ctx.clone()))).unwrap().wait(Deadline::after(Duration::from_secs(2))).unwrap();
    assert!(pending.wait(Deadline::after(Duration::from_secs(2))).unwrap().error.is_some());
    scheduler.begin_shutdown();scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();
}
#[test]
fn gain_scheduler_status_rebases_age_without_a_new_observation_or_io() {
    let (b,g)=gain_backend();let ctx=supervised_connect(&b,config("gain",1));gain_action(&b,&ctx,"read_pid",json!({}));
    let scheduler=Scheduler::new(b.clone(),g.clock.clone(),Limits::default()).unwrap();let key=format!("device:{}",ctx.domain.as_ref().unwrap().id);
    g.clock.wait(Duration::from_millis(2500));
    gain::until(||scheduler_status(&scheduler)["devices"][&key]["pid"]["values"].is_array());
    let before=scheduler_status(&scheduler);let writes=g.data.lock().unwrap().writes.len();g.clock.wait(Duration::from_millis(100));let after=scheduler_status(&scheduler);
    assert_eq!(g.data.lock().unwrap().writes.len(),writes,"status must remain metadata-only");
    scheduler.begin_shutdown();scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();
    assert_eq!(after["devices"][&key]["fields"]["temperature_c"]["revision"],before["devices"][&key]["fields"]["temperature_c"]["revision"]);
    let old_age=before["devices"][&key]["fields"]["temperature_c"]["observed_age_s"].as_f64().unwrap();
    let new_age=after["devices"][&key]["fields"]["temperature_c"]["observed_age_s"].as_f64().unwrap();
    assert!((new_age-old_age-0.1).abs()<1e-9);
    assert!((after["devices"][&key]["pid"]["observed_age_s"].as_f64().unwrap()-2.6).abs()<1e-9);
}
#[test]
fn gain_scheduler_keeps_temperature_live_during_ramp_and_off_stops_later_steps() {
    use std::sync::{Mutex,Condvar,atomic::{AtomicBool,Ordering}};
    struct GateClock {clock:Arc<ManualClock>,hold:AtomicBool,gate:(Mutex<(bool,bool)>,Condvar)}
    impl Clock for GateClock {
        fn now(&self)->Duration {self.clock.now()}
        fn wait(&self,duration:Duration) {
            self.clock.wait(duration);
            if self.hold.swap(false,Ordering::AcqRel) {
                let mut gate=self.gate.0.lock().unwrap();gate.0=true;self.gate.1.notify_all();
                drop(self.gate.1.wait_while(gate,|gate|!gate.1).unwrap());
            }
        }
    }
    let g=gain::Peer::new();let clock=Arc::new(GateClock {clock:g.clock.clone(),hold:AtomicBool::new(false),gate:(Mutex::new((false,false)),Condvar::new())});
    let factory=Arc::new(SystemFactory::with_backends(clock.clone(),Arc::new(Serial {g:g.clone(),v:voltage::Peer::new()}),pm::Wire::new(1)));
    let b=backend(factory,clock.clone());let ctx=supervised_connect(&b,config("gain",1));gain_stable(&b,&g,&ctx);
    let scheduler=Scheduler::new(b.clone(),clock.clone(),Limits::default()).unwrap();let key=format!("device:{}",ctx.domain.as_ref().unwrap().id);
    clock.hold.store(true,Ordering::Release);
    let pending=scheduler.submit(support::request("ramp-live","action",json!({"name":"start_current","args":{"current_ma":8.}}),Some(ctx.clone()))).unwrap();
    {let gate=clock.gate.0.lock().unwrap();let (_guard,wait)=clock.gate.1.wait_timeout_while(gate,Duration::from_secs(2),|gate|!gate.0).unwrap();assert!(!wait.timed_out());}
    g.data.lock().unwrap().temperature=22.1;g.clock.wait(Duration::from_secs(1));
    gain::until(||b.observe(&ctx).status["last_status"]["temperature_c"]==22.1);
    g.clock.wait(Duration::from_millis(500));
    gain::until(||scheduler_status(&scheduler)["devices"][&key]["last_status"]["temperature_c"]==22.1);
    let observed=scheduler_status(&scheduler);assert_eq!(observed["devices"][&key]["current_operation"]["phase"],"ramping");assert_eq!(observed["domains"][&key]["active_request_id"],"ramp-live");
    let before=g.data.lock().unwrap().writes.iter().filter(|(_,w)|w.starts_with(b"STCA")).count();
    let off=scheduler.submit(support::request("off-ramp","action",json!({"name":"disable_current","args":{}}),Some(ctx.clone()))).unwrap();
    {clock.gate.0.lock().unwrap().1=true;clock.gate.1.notify_all();}
    assert_eq!(off.wait(Deadline::after(Duration::from_secs(2))).unwrap().phase,Phase::Completed);assert_ne!(pending.wait(Deadline::after(Duration::from_secs(2))).unwrap().phase,Phase::Completed);
    assert_eq!(g.data.lock().unwrap().writes.iter().filter(|(_,w)|w.starts_with(b"STCA")).count(),before,"Off must fence later ramp steps");
    scheduler.begin_shutdown();scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();
}
#[test]
fn stale_gain_context_is_rejected_without_waiting_for_a_held_native_action() {
    let (b,g)=gain_backend();let ctx=supervised_connect(&b,config("gain",1));g.data.lock().unwrap().hold=true;
    let action_backend=b.clone();let action_context=ctx.clone();
    let pending=std::thread::spawn(move||gain_action(&action_backend,&action_context,"set_temperature",json!({"temperature_c":23.})));
    g.held();b.registry().fence(ctx.domain.as_ref().unwrap()).unwrap();
    let (tx,rx)=std::sync::mpsc::channel();let observed_backend=b.clone();let observed_context=ctx.clone();
    let observed=std::thread::spawn(move||tx.send(observed_backend.observe(&observed_context)).unwrap());
    let stale=rx.recv_timeout(Duration::from_millis(150));g.release();pending.join().unwrap();observed.join().unwrap();
    let current=b.registry().context(ctx.domain.as_ref().unwrap()).unwrap();b.execute(&support::request("close","disconnect",json!({}),Some(current)));
    assert_eq!(stale.expect("stale cache query must not lock the action slot").status["state"],"DISCONNECTED");
}
fn supervised_authorization(c: &DomainConfig, controller: &str) -> serde_json::Value {
    json!({"stage":"supervised","accepted":true,"supervised":true,"retain_session":true,
        "binding":{"mode":"real","domain":c.domain,"config_rev":c.config_rev,
        "model_id":c.model_id,"profile_id":c.profile_id,"config_digest":"d".repeat(64),"controller":controller}})
}
fn supervised_probe(b: &yang_worker::backend::NativeBackend, c: &DomainConfig, ctx: &ContextV3, controller: &str) -> yang_protocol::OutcomeV3 {
    b.execute(&support::request("proof", "probe", json!({"authorization":supervised_authorization(c,controller)}), Some(ctx.clone())))
}
fn fault_gain(b: &yang_worker::backend::NativeBackend, g: &gain::Peer, ctx: &ContextV3) {
    g.data.lock().unwrap().faults.push_back(("RDTA".into(),b"READY;T=broken\r\n".to_vec()));
    g.clock.wait(Duration::from_secs(1));
    gain::until(|| b.observe(ctx).status["state"]=="FAULT");
    // Wait for the driver's own mandatory current-off then TEC-off attempt;
    // later cached health/proof assertions must not count that as their I/O.
    gain::until(|| {
        let d=g.data.lock().unwrap();
        d.writes.iter().filter(|(_,w)|w==b"STRA000000\r\n").count()>=2 && d.pending.is_empty()
    });
}
#[test]
fn supervised_gain_probe_preserves_cached_fault_without_reopening_or_io() {
    let (b,g)=gain_backend();let c=config("gain",1);let ctx=supervised_connect(&b,c.clone());
    fault_gain(&b,&g,&ctx);
    let before=g.data.lock().unwrap().writes.len();
    let failed=supervised_probe(&b,&c,&ctx,&"e".repeat(32));
    let retained=b.registry().snapshot()[0].clone();
    let observed=b.observe(&ctx).status;
    let (after,opens,closes)={let d=g.data.lock().unwrap();(d.writes.len(),d.opens,d.closes)};
    let released=b.execute(&support::request("close","disconnect",json!({}),Some(ctx.clone())));
    assert_eq!(released.phase,Phase::Completed);
    assert_eq!(failed.phase,Phase::RejectedBeforeCall);
    let error=failed.error.unwrap();assert_eq!(error.kind,"Unhealthy");assert!(error.message.contains("Gain invalid numeric reply"),"{error:?}");
    assert_eq!(retained.context,ctx);assert!(retained.responsibility);
    assert_eq!(observed["state"],"FAULT");assert_eq!(observed["connected"],false);
    assert_eq!(after,before,"cached health/proof must never read or replay a command");assert_eq!(opens,1);assert_eq!(closes,0);
}
#[test]
fn gain_proof_cannot_register_after_the_retained_driver_faults() {
    let (b,g)=gain_backend();let c=config("gain",1);let ctx=supervised_connect(&b,c.clone());
    let proof=supervised_probe(&b,&c,&ctx,&"e".repeat(32));assert_eq!(proof.phase,Phase::Completed);
    let proof=proof.result.unwrap()["proof"]["proof_id"].clone();
    fault_gain(&b,&g,&ctx);let before=g.data.lock().unwrap().writes.len();
    let rejected=b.execute(&support::request("register","register_verified",json!({"domain":c.domain,"proof_id":proof,"config_digest":"d".repeat(64),"config_rev":1}),Some(b.global_context())));
    let approved=b.registry().config(&c.domain).unwrap();let after=g.data.lock().unwrap().writes.len();
    let released=b.execute(&support::request("close","disconnect",json!({}),Some(ctx)));
    assert_eq!(released.phase,Phase::Completed);assert_eq!(rejected.phase,Phase::RejectedBeforeCall);
    let error=rejected.error.unwrap();assert_eq!(error.kind,"Unhealthy");assert!(error.message.contains("Gain invalid numeric reply"));
    assert_eq!(approved.expected_identity,json!({}),"unhealthy proof must not refine the saved identity");
    assert_eq!(after,before);assert_eq!(g.data.lock().unwrap().opens,1);
}
#[test]
fn supervised_gain_probe_distinguishes_missing_wrong_controller_and_healthy_sessions() {
    let (b,g)=gain_backend();let c=config("gain",1);
    b.execute(&support::request("configure","configure_domain",json!({"config":c}),Some(b.global_context())));
    let unbound=b.registry().context(&c.domain).unwrap();
    let missing=supervised_probe(&b,&c,&unbound,&"e".repeat(32));assert_eq!(missing.error.unwrap().kind,"ManualVerificationRequired");assert_eq!(g.data.lock().unwrap().opens,0);
    let ctx=b.registry().bind(&c.domain,&yang_worker::new_id().unwrap()).unwrap();
    let connected=b.execute(&support::request("connect","connect",json!({"authorization":supervised_authorization(&c,&"e".repeat(32))}),Some(ctx.clone())));assert_eq!(connected.phase,Phase::Completed);
    let before=g.data.lock().unwrap().writes.len();let wrong=supervised_probe(&b,&c,&ctx,&"f".repeat(32));
    let healthy=supervised_probe(&b,&c,&ctx,&"e".repeat(32));let after=g.data.lock().unwrap().writes.len();
    let proof=healthy.result.as_ref().unwrap()["proof"]["proof_id"].clone();
    let registered=b.execute(&support::request("register","register_verified",json!({"domain":c.domain,"proof_id":proof,"config_digest":"d".repeat(64),"config_rev":1}),Some(b.global_context())));
    b.execute(&support::request("close","disconnect",json!({}),Some(ctx)));
    assert_eq!(wrong.error.unwrap().kind,"ManualVerificationRequired");assert_eq!(healthy.phase,Phase::Completed);assert_eq!(registered.phase,Phase::Completed);
    assert_eq!(healthy.result.unwrap()["release_confirmed"],false);assert_eq!(after,before);assert_eq!(g.data.lock().unwrap().opens,1);
}
#[test]
fn busy_gain_supervised_probe_reports_resource_busy_without_reopening() {
    let (b,g)=gain_backend();let c=config("gain",1);let ctx=supervised_connect(&b,c.clone());
    g.data.lock().unwrap().hold=true;
    let action_backend=b.clone();let action_context=ctx.clone();
    let pending=std::thread::spawn(move||action_backend.execute(&support::request("temperature","action",json!({"name":"set_temperature","args":{"temperature_c":23.}}),Some(action_context))));
    g.held();let rejected=supervised_probe(&b,&c,&ctx,&"e".repeat(32));g.release();let finished=pending.join().unwrap();
    b.execute(&support::request("close","disconnect",json!({}),Some(ctx)));
    assert_eq!(finished.phase,Phase::Completed);assert_eq!(rejected.error.unwrap().kind,"ResourceBusy");assert_eq!(g.data.lock().unwrap().opens,1);
}
#[test]
fn asynchronous_gain_fault_publishes_domain_fault_and_keeps_connection_responsibility() {
    let (b,g)=gain_backend();let c=config("gain",1);let ctx=supervised_connect(&b,c.clone());
    let scheduler=Scheduler::new(b.clone(),g.clock.clone(),Limits::default()).unwrap();
    fault_gain(&b,&g,&ctx);g.clock.wait(Duration::from_secs(3));
    let key=format!("device:{}",c.domain.id);
    let status=||scheduler.submit(support::request(&yang_worker::new_id().unwrap(),"status",json!({}),None)).unwrap().wait(Deadline::after(Duration::from_secs(1))).unwrap().result.unwrap();
    gain::until(|| status()["devices"][&key]["state"]=="FAULT");
    let observed=status();let registered=scheduler.snapshot().domains[0].clone();
    let (opens,closes)={let d=g.data.lock().unwrap();(d.opens,d.closes)};
    let rejected=scheduler.submit(support::request("stale-action","action",json!({"name":"set_current","args":{"current_ma":10.}}),Some(ctx.clone()))).unwrap().wait(Deadline::after(Duration::from_secs(1))).unwrap();
    scheduler.begin_shutdown();scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();
    assert_eq!(observed["domains"][&key]["state"],"FAULT");assert_eq!(registered.state,DriverState::Fault);
    assert_eq!(registered.context,ctx);assert!(registered.responsibility);assert_eq!(observed["domains"][&key]["responsibility"],true);
    assert_eq!(observed["devices"][&key]["connected"],false);assert!(observed["devices"][&key]["fault"].as_str().unwrap().contains("Gain invalid numeric reply"));
    assert_eq!(opens,1);assert_eq!(closes,0,"publishing an asynchronous fault must not close or replay the session");
    assert_eq!(rejected.error.unwrap().kind,"NotReady","outer fault must reject ordinary work before entering the driver");
}
struct Inventory;
impl yang_worker::discovery::InventoryPort for Inventory {
    fn serial(
        &self,
    ) -> Result<
        Vec<yang_drivers::transport::serial_discovery::SerialDeviceInfo>,
        yang_worker::WorkerError,
    > {
        panic!("no inventory")
    }
    fn visa(&self) -> Result<Vec<String>, yang_worker::WorkerError> {
        panic!("no inventory")
    }
}
fn backend(
    f: Arc<SystemFactory>,
    clock: Arc<dyn Clock>,
) -> Arc<yang_worker::backend::NativeBackend> {
    yang_worker::backend::NativeBackend::with_ports(
        yang_worker::DomainRegistry::new(&"a".repeat(32)).unwrap(),
        clock,
        f,
        Arc::new(Inventory),
    )
}
fn supervised_connect(b: &yang_worker::backend::NativeBackend, c: DomainConfig) -> ContextV3 {
    let configured = b.execute(&support::request(
        "configure",
        "configure_domain",
        json!({"config":c}),
        Some(b.global_context()),
    ));
    assert_eq!(configured.phase, Phase::Completed, "{:?}", configured.error);
    let ctx = b
        .registry()
        .bind(&c.domain, &yang_worker::new_id().unwrap())
        .unwrap();
    let reply=b.execute(&support::request("connect","connect",json!({"authorization":{"stage":"supervised","accepted":true,"supervised":true,"retain_session":true,"binding":{"mode":"real","domain":c.domain,"config_rev":1,"model_id":c.model_id,"profile_id":c.profile_id,"config_digest":"d".repeat(64),"controller":"e".repeat(32)}}}),Some(ctx.clone())));
    assert_eq!(reply.phase, Phase::Completed, "{:?}", reply.error);
    ctx
}
#[test]
fn gain_voltage_cleanup_order_on_eof() {
    let clock = Arc::new(yang_drivers::clock::SystemClock::default());
    let (f, g, v) = factory(clock.clone(), pm::Wire::new(1));
    let b = backend(f, clock.clone());
    supervised_connect(&b, config("gain", 1));
    supervised_connect(&b, config("voltage", 2));
    let fixture = support::Fixture::new();
    let w = support::worker(b.clone(), &fixture);
    let receipt = w
        .run_io(
            std::io::Cursor::new(Vec::<u8>::new()),
            support::Output::default(),
        )
        .unwrap();
    assert!(receipt.all_resources_released, "{:?}", receipt);
    let writes = g.data.lock().unwrap().writes.clone();
    let commands: Vec<_> = writes
        .iter()
        .map(|(_, w)| std::str::from_utf8(w).unwrap())
        .collect();
    let last_q = commands
        .iter()
        .rposition(|c| *c == "STQA000000\r\n")
        .unwrap();
    let last_d = commands
        .iter()
        .rposition(|c| *c == "STRA000000\r\n")
        .unwrap();
    assert!(last_q < last_d);
    assert!(v.data.lock().unwrap().writes.last().unwrap().1[..16]
        .iter()
        .all(|v| *v == 0));
    assert!(b.registry().snapshot().iter().all(|d| !d.responsibility));
}
#[test]
fn typed_safety_keeps_native_session_resumable_and_records_original_attempt() {
    let mut ui_cases = Vec::new();
    for (kind, action, interrupted) in [
        ("gain", "disable_current", false),
        ("gain", "disable_tec", false),
        ("voltage", "zero", false),
        ("gain", "disable_current", true),
        ("gain", "disable_tec", true),
        ("voltage", "zero", true),
    ] {
        let clock = Arc::new(ManualClock::default());
        let (f, g, v) = factory(clock.clone(), pm::Wire::new(1));
        let b = backend(f, clock.clone());
        let c = config(kind, 1);
        let ctx = supervised_connect(&b, c.clone());
        if kind == "gain" {
            let r = b.execute(&support::request(
                "tec",
                "action",
                json!({"name":"enable_tec","args":{}}),
                Some(ctx.clone()),
            ));
            assert_eq!(r.phase, Phase::Completed);
        }
        let scheduler = Scheduler::new(b.clone(), clock, Limits::default()).unwrap();
        let normal = if interrupted {
            if kind == "gain" {
                g.data.lock().unwrap().hold = true;
            } else {
                v.data.lock().unwrap().hold = true;
            }
            let args = if kind == "gain" {
                json!({"name":"set_current","args":{"current_ma":10.}})
            } else {
                json!({"name":"set_channel","args":{"channel":1,"voltage":14.}})
            };
            let p = scheduler
                .submit(support::request("prior", "action", args, Some(ctx.clone())))
                .unwrap();
            if kind == "gain" {
                g.held();
            } else {
                v.held();
            }
            gain::until(|| scheduler.snapshot().active > 0);
            Some(p)
        } else {
            None
        };
        let p = scheduler
            .submit(support::request(
                "safe",
                "action",
                json!({"name":action,"args":{}}),
                Some(ctx),
            ))
            .unwrap();
        if interrupted {
            if kind == "gain" {
                g.release();
            } else {
                v.release();
            }
        }
        let r = p.wait(Deadline::after(Duration::from_secs(2))).unwrap();
        assert_eq!(
            r.phase,
            Phase::Completed,
            "{kind}/{action}/{interrupted}: {:?}",
            r.error
        );
        if let Some(p) = normal {
            assert_ne!(
                p.wait(Deadline::after(Duration::from_secs(2)))
                    .unwrap()
                    .phase,
                Phase::Completed
            );
        }
        if kind == "gain" {
            let d = g.data.lock().unwrap();
            assert!(!d.enabled);
            assert_eq!(
                d.tec,
                action == "disable_current",
                "typed off shut down unrelated TEC"
            );
        } else {
            assert!(v.data.lock().unwrap().voltages.iter().all(|x| *x == 0.));
        }
        let current = b.registry().context(&c.domain).unwrap();
        let held = scheduler
            .submit(support::request("held-status", "status", json!({}), None))
            .unwrap()
            .wait(Deadline::after(Duration::from_secs(1)))
            .unwrap()
            .result
            .unwrap();
        gain::until(|| {
            scheduler
                .submit(support::request(
                    &yang_worker::new_id().unwrap(),
                    "resume",
                    json!({"confirm":true}),
                    Some(current.clone()),
                ))
                .unwrap()
                .wait(Deadline::after(Duration::from_secs(1)))
                .unwrap()
                .phase
                == Phase::Completed
        });
        let status = scheduler
            .submit(support::request("status-after", "status", json!({}), None))
            .unwrap()
            .wait(Deadline::after(Duration::from_secs(1)))
            .unwrap()
            .result
            .unwrap();
        let key = format!("device:{}", c.domain.id);
        assert_eq!(
            status["domains"][&key]["safety"]["attempt_id"],
            r.result.as_ref().unwrap()["attempt_id"]
        );
        assert_eq!(status["domains"][&key]["safety"]["phase"], "completed");
        ui_cases.push(json!({"kind":kind,"key":key,"held":held,"resumed":status}));
        let name = if kind == "gain" {
            "enable_tec"
        } else {
            "set_channel"
        };
        let args = if kind == "gain" {
            json!({})
        } else {
            json!({"channel":1,"voltage":0.1})
        };
        let resumed = scheduler
            .submit(support::request(
                "next",
                "action",
                json!({"name":name,"args":args}),
                Some(current),
            ))
            .unwrap()
            .wait(Deadline::after(Duration::from_secs(2)))
            .unwrap();
        assert_eq!(resumed.phase, Phase::Completed, "{:?}", resumed.error);
        scheduler.begin_shutdown();
        assert!(scheduler
            .join_when_released(Deadline::after(Duration::from_secs(2)))
            .is_ok());
    }
    if let Some(path) = std::env::var_os("YANG_NATIVE_STATUS_OUTPUT") {
        std::fs::write(path, serde_json::to_vec(&ui_cases).unwrap()).unwrap();
    }
}
fn registered(b: &yang_worker::backend::NativeBackend, c: DomainConfig) -> ContextV3 {
    let r = b.execute(&support::request(
        "configure",
        "configure_domain",
        json!({"config":c}),
        Some(b.global_context()),
    ));
    assert_eq!(r.phase, Phase::Completed);
    let ctx = b.registry().context(&c.domain).unwrap();
    let r=b.execute(&support::request("probe","probe",json!({"authorization":{"stage":"readonly","accepted":true,"supervised":false,"retain_session":false,"binding":{"mode":"real","domain":c.domain,"config_rev":1,"model_id":c.model_id,"profile_id":c.profile_id,"config_digest":"d".repeat(64)}}}),Some(ctx)));
    assert_eq!(r.phase, Phase::Completed, "{:?}", r.error);
    let proof = r.result.unwrap()["proof"]["proof_id"].clone();
    let r = b.execute(&support::request(
        "reg",
        "register_verified",
        json!({"domain":c.domain,"proof_id":proof,"config_digest":"d".repeat(64),"config_rev":1}),
        Some(b.global_context()),
    ));
    assert_eq!(r.phase, Phase::Completed, "{:?}", r.error);
    let ctx = b
        .registry()
        .bind(&c.domain, &yang_worker::new_id().unwrap())
        .unwrap();
    let r = b.execute(&support::request(
        "connect",
        "connect",
        json!({}),
        Some(ctx.clone()),
    ));
    assert_eq!(r.phase, Phase::Completed, "{:?}", r.error);
    ctx
}
#[test]
fn stalled_osa_does_not_block_other_domain() {
    let clock = Arc::new(yang_drivers::clock::SystemClock::default());
    let wire = osa::Wire::new(2, yang_drivers::osa::TransferFormat::Ascii);
    let (f, _, _) = factory(clock.clone(), wire.clone());
    let b = backend(f, clock.clone());
    let fixture = support::Fixture::new();
    let _worker = support::worker(b.clone(), &fixture);
    let oc = registered(&b, config("osa", 1));
    let gc = supervised_connect(&b, config("gain", 2));
    let scheduler = Scheduler::new(b.clone(), clock, Limits::default()).unwrap();
    wire.data.lock().unwrap().block_y = true;
    let held = scheduler
        .submit(support::request(
            "slow",
            "action",
            json!({"name":"read_trace","args":{}}),
            Some(oc),
        ))
        .unwrap();
    wire.entered();
    let fast = scheduler
        .submit(support::request(
            "fast",
            "action",
            json!({"name":"set_temperature","args":{"temperature_c":23}}),
            Some(gc),
        ))
        .unwrap();
    assert_eq!(
        fast.wait(Deadline::after(Duration::from_secs(1)))
            .unwrap()
            .phase,
        Phase::Completed
    );
    wire.release();
    assert_eq!(
        held.wait(Deadline::after(Duration::from_secs(2)))
            .unwrap()
            .phase,
        Phase::Completed
    );
    scheduler.begin_shutdown();
    scheduler
        .join_when_released(Deadline::after(Duration::from_secs(2)))
        .unwrap();
}
#[test]
fn revoke_fences_pending_actions() {
    let clock = Arc::new(yang_drivers::clock::SystemClock::default());
    let (f, g, _) = factory(clock.clone(), pm::Wire::new(1));
    let b = backend(f, clock);
    let ctx = supervised_connect(&b, config("gain", 1));
    b.registry().fence(ctx.domain.as_ref().unwrap()).unwrap();
    let before = g.data.lock().unwrap().writes.len();
    let r = b.execute(&support::request(
        "late",
        "action",
        json!({"name":"set_current","args":{"current_ma":60}}),
        Some(ctx.clone()),
    ));
    assert_eq!(r.phase, Phase::RejectedBeforeCall);
    assert_eq!(g.data.lock().unwrap().writes.len(), before);
    let fresh = b.registry().context(ctx.domain.as_ref().unwrap()).unwrap();
    b.execute(&support::request(
        "close",
        "disconnect",
        json!({}),
        Some(fresh),
    ));
}
