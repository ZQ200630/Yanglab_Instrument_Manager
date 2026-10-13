//! Offline regression: a bounded transport double is the only native-call boundary.
#[path = "support/native.rs"]
mod support;

use serde_json::{json, Value};
use std::{
    sync::{mpsc, Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use yang_drivers::{
    clock::SystemClock,
    transport::{
        serial_abi::{SerialBackend, SerialIo},
        serial_discovery::{DeviceRecord, SerialDeviceInfo},
        visa_abi::{VisaApi, VI_ERROR_RSRC_NFOUND},
        Deadline,
    },
    DriverResult,
};
use yang_protocol::{Limits, Phase};
use yang_worker::{
    backend::NativeBackend,
    discovery::InventoryPort,
    scheduler::{Backend, Scheduler},
    session::SystemFactory,
    DomainRegistry, WorkerError,
};

struct NoSerial;
impl SerialBackend for NoSerial {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> { panic!("no enumeration") }
    fn open(&self, _: &str) -> DriverResult<Box<dyn SerialIo>> { panic!("no serial open") }
}
struct NoInventory;
impl InventoryPort for NoInventory {
    fn serial(&self) -> Result<Vec<SerialDeviceInfo>, WorkerError> { panic!("no enumeration") }
    fn visa(&self) -> Result<Vec<String>, WorkerError> { panic!("no enumeration") }
}
#[derive(Default)]
struct BlockedManager {
    gate: Mutex<(bool, bool)>,
    wake: Condvar,
}
impl BlockedManager {
    fn wait_entered(&self) -> bool {
        let gate = self.gate.lock().unwrap();
        self.wake.wait_timeout_while(gate, Duration::from_secs(2), |g| !g.0).unwrap().0.0
    }
    fn release(&self) {
        self.gate.lock().unwrap().1 = true;
        self.wake.notify_all();
    }
}
// Even an assertion panic releases the fake call; its own wait is also bounded.
struct ReleaseOnDrop(Arc<BlockedManager>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) { self.0.release(); }
}
impl VisaApi for BlockedManager {
    fn open_manager(&self) -> (i32, u32) {
        let mut gate = self.gate.lock().unwrap();
        gate.0 = true;
        self.wake.notify_all();
        let _ = self.wake.wait_timeout_while(gate, Duration::from_secs(5), |g| !g.1).unwrap();
        (VI_ERROR_RSRC_NFOUND, 0)
    }
    fn parse(&self, _: u32, _: &str) -> DriverResult<String> { panic!("manager open fails first") }
    fn find(&self, _: u32) -> (i32, u32, u32, DriverResult<String>) { panic!("no enumeration") }
    fn find_next(&self, _: u32) -> DriverResult<String> { panic!("no enumeration") }
    fn open(&self, _: u32, _: &str, _: u32, _: u32) -> (i32, u32) { panic!("no device open") }
    fn close(&self, _: u32) -> i32 { panic!("failed open owns no handle") }
    fn set_timeout(&self, _: u32, _: u32) -> i32 { panic!("no device configuration") }
    fn set_termination_enabled(&self, _: u32, _: bool) -> i32 { panic!("no device configuration") }
    fn write(&self, _: u32, _: &[u8]) -> (i32, u32) { panic!("no commands") }
    fn read(&self, _: u32, _: &mut [u8]) -> (i32, u32) { panic!("no reads") }
}

fn status(scheduler: &Scheduler, id: &str) -> Value {
    scheduler.submit(support::request(id, "status", json!({}), None)).unwrap()
        .wait(Deadline::after(Duration::from_secs(2))).unwrap().result.unwrap()
}

// Removing the cache snapshot or execution scope must fail this behavior test.
#[test]
fn cached_status_correlates_a_blocked_native_call_without_waiting_for_it() {
    let clock = Arc::new(SystemClock::default());
    let api = Arc::new(BlockedManager::default());
    let release = ReleaseOnDrop(api.clone());
    let factory = Arc::new(SystemFactory::with_backends(clock.clone(), Arc::new(NoSerial), api.clone()));
    let backend = NativeBackend::with_ports(
        DomainRegistry::new(&"a".repeat(32)).unwrap(), clock.clone(), factory, Arc::new(NoInventory),
    );
    let config = support::configured(&backend);
    let context = backend.registry().context(&config.domain).unwrap();
    let scheduler = Arc::new(Scheduler::new(backend.clone(), clock, Limits::default()).unwrap());
    let probe = scheduler.submit(support::request("blocked-manager-probe", "probe", json!({
        "authorization":{"stage":"readonly","accepted":true,"supervised":false,"retain_session":false,
        "binding":{"mode":"real","domain":config.domain,"config_rev":1,"model_id":"aq6370",
        "profile_id":"gpib-visa","config_digest":"a".repeat(64)}}
    }), Some(context))).unwrap();
    let entered = api.wait_entered();
    let (send, receive) = mpsc::channel();
    let reader = scheduler.clone();
    let read_thread = std::thread::spawn(move || {
        let started = Instant::now();
        let value = status(&reader, "while-native-pending");
        let _ = send.send((value, started.elapsed()));
    });
    let while_blocked = receive.recv_timeout(Duration::from_millis(500));
    // Gather cleanup before assertions so red runs cannot strand a worker.
    drop(release);
    let outcome = probe.wait(Deadline::after(Duration::from_secs(2))).unwrap();
    read_thread.join().unwrap();
    let after = status(&scheduler, "after-native-returned");
    scheduler.begin_shutdown();
    scheduler.join_when_released(Deadline::after(Duration::from_secs(2))).unwrap();

    assert!(entered, "the probe must reach the injected native boundary");
    let (pending, elapsed) = while_blocked.expect("cached status waited for the blocked native call");
    assert!(elapsed < Duration::from_millis(500));
    let active = pending["native_io"]["active"].as_array().expect("cached native diagnostics missing");
    let call = active.iter().find(|call| call["stage"] == "viOpenDefaultRM").expect("pending native stage missing");
    assert_eq!(call["request_id"], "blocked-manager-probe");
    assert_eq!(call["domain"], format!("device:{}", config.domain.id));
    assert_eq!(call["outcome"], "pending");
    assert!(call["status"].is_null());
    assert!(call["elapsed_ms"].is_u64());
    assert_eq!(outcome.phase, Phase::FailedAfterCallStarted);
    let recent = after["native_io"]["recent"].as_array().expect("completed native diagnostics missing");
    let finished = recent.iter().find(|item| item["id"] == call["id"]).expect("completed call missing");
    assert_eq!(finished["request_id"], "blocked-manager-probe");
    assert_eq!(finished["domain"], call["domain"]);
    assert_eq!(finished["outcome"], "error");
    assert_eq!(finished["status"], VI_ERROR_RSRC_NFOUND);
    assert!(!after["native_io"]["active"].as_array().unwrap().iter().any(|item| item["id"] == call["id"]));
    for trace in [&pending["native_io"], &after["native_io"]] {
        let encoded = serde_json::to_string(trace).unwrap();
        assert!(!encoded.contains("GPIB0") && !encoded.contains("*IDN?"));
        for entry in trace["active"].as_array().unwrap().iter().chain(trace["recent"].as_array().unwrap()) {
            assert_eq!(entry.as_object().unwrap().len(), 8, "trace must contain only bounded metadata");
        }
    }
}
