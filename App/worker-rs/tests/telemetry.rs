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
use std::{sync::Arc, time::Duration};
use yang_drivers::{
    clock::{Clock, ManualClock},
    lifecycle::DriverState,
    transport::{
        serial_abi::{SerialBackend, SerialIo},
        serial_discovery::DeviceRecord,
        visa_abi::VisaApi,
        Deadline,
    },
    DriverResult,
};
use yang_protocol::{ContextV3, DomainConfig, DomainRef, Limits, Phase};
use yang_worker::{
    scheduler::{Backend, Scheduler},
    session::{DriverFactory, SystemFactory},
};
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
