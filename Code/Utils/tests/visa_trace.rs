use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;
use yang_drivers::transport::visa_abi::*;
use yang_drivers::transport::visa_trace::{self, Outcome, Stage};
use yang_drivers::transport::{Deadline, ResourceBook, VisaManager};
use yang_drivers::{DriverError, DriverResult};

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct Gate {
    entered: mpsc::Sender<()>,
    released: Mutex<bool>,
    wake: Condvar,
}
impl Gate {
    fn wait(&self) {
        self.entered.send(()).unwrap();
        let released = self.released.lock().unwrap();
        let (released, timed) = self
            .wake
            .wait_timeout_while(released, Duration::from_secs(5), |value| !*value)
            .unwrap();
        assert!(*released && !timed.timed_out(), "test must release injected call");
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct Api {
    block: Option<(Stage, Arc<Gate>)>,
    fail_read: bool,
}
impl Api {
    fn plain() -> Arc<Self> {
        Arc::new(Self { block: None, fail_read: false })
    }
    fn gated(stage: Stage) -> (Arc<Self>, mpsc::Receiver<()>, Release) {
        let (entered, received) = mpsc::channel();
        let gate = Arc::new(Gate { entered, released: Mutex::new(false), wake: Condvar::new() });
        (Arc::new(Self { block: Some((stage, gate.clone())), fail_read: false }), received, Release(gate))
    }
    fn enter(&self, stage: Stage) {
        if let Some((blocked, gate)) = &self.block {
            if *blocked == stage { gate.wait(); }
        }
    }
}
impl VisaApi for Api {
    fn open_manager(&self) -> (i32, u32) { self.enter(Stage::OpenDefaultRm); (0, 1) }
    fn parse(&self, _: u32, name: &str) -> DriverResult<String> { self.enter(Stage::ParseResource); Ok(name.into()) }
    fn find(&self, _: u32) -> (i32, u32, u32, DriverResult<String>) { (VI_ERROR_RSRC_NFOUND, 0, 0, Ok(String::new())) }
    fn find_next(&self, _: u32) -> DriverResult<String> { unreachable!() }
    fn open(&self, _: u32, _: &str, mode: u32, _: u32) -> (i32, u32) { assert_eq!(mode, VI_EXCLUSIVE_LOCK); self.enter(Stage::Open); (0, 2) }
    fn close(&self, _: u32) -> i32 { self.enter(Stage::Close); 0 }
    fn set_timeout(&self, _: u32, millis: u32) -> i32 { assert!(millis > 0); self.enter(Stage::SetTimeout); 0 }
    fn set_termination_enabled(&self, _: u32, enabled: bool) -> i32 { assert!(!enabled); self.enter(Stage::SetTermination); 0 }
    fn write(&self, _: u32, bytes: &[u8]) -> (i32, u32) { self.enter(Stage::Write); (0, bytes.len() as u32) }
    fn read(&self, _: u32, bytes: &mut [u8]) -> (i32, u32) {
        self.enter(Stage::Read);
        let payload = b"SECRET_SAMPLE_PAYLOAD";
        bytes[..payload.len()].copy_from_slice(payload);
        (if self.fail_read { VI_ERROR_TMO } else { 0 }, payload.len() as u32)
    }
}
fn snapshot() -> visa_trace::Snapshot {
    for _ in 0..100 {
        if let Some(value) = visa_trace::snapshot() { return value; }
        std::thread::yield_now();
    }
    panic!("diagnostic sink remained busy without native I/O in its lock");
}
fn deadline() -> Deadline { Deadline::after(Duration::from_secs(1)) }

// Catches publishing only after a blocked native call returns, or changing pending into success.
#[test]
fn manager_entry_is_visible_while_the_native_call_is_blocked() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (api, entered, release) = Api::gated(Stage::OpenDefaultRm);
    let worker = std::thread::spawn(move || {
        let _scope = visa_trace::scope("blocked-manager", "instrument:aaaa");
        VisaManager::from_api(api, ResourceBook::isolated()).unwrap().release().unwrap();
    });
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let before = snapshot().active.into_iter().find(|call| call.request_id.as_deref() == Some("blocked-manager")).unwrap();
    assert_eq!(before.stage, Stage::OpenDefaultRm);
    assert_eq!(before.domain.as_deref(), Some("instrument:aaaa"));
    assert_eq!(before.outcome, Outcome::Pending);
    assert_eq!(before.status, None);
    std::thread::sleep(Duration::from_millis(10));
    let still = snapshot().active.into_iter().find(|call| call.id == before.id).unwrap();
    assert_eq!(still.started_monotonic_ms, before.started_monotonic_ms);
    assert!(still.elapsed_ms >= before.elapsed_ms + 5);
    drop(release);
    worker.join().unwrap();
    let after = snapshot();
    assert!(!after.active.iter().any(|call| call.id == before.id));
    let done = after.recent.iter().find(|call| call.id == before.id).unwrap();
    assert_eq!(done.outcome, Outcome::Returned);
    assert_eq!(done.status, Some(0));
    assert!(done.elapsed_ms >= still.elapsed_ms);
}

// Catches wrapping a stage after the call, or omitting attributes/read/write/cleanup.
#[test]
fn each_remaining_native_stage_is_visible_before_its_injected_call_returns() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for stage in [Stage::ParseResource, Stage::SetTermination, Stage::SetTimeout, Stage::Read, Stage::Write, Stage::Close] {
        let (api, entered, release) = Api::gated(stage);
        let request = format!("blocked-{stage:?}");
        let owned_request = request.clone();
        let worker = std::thread::spawn(move || {
            let _scope = visa_trace::scope(&owned_request, "instrument:abcd");
            let manager = VisaManager::from_api(api, ResourceBook::isolated()).unwrap();
            let resource = manager.canonicalize("GPIB0::4::INSTR").unwrap();
            let mut session = manager.open(resource, deadline()).unwrap();
            session.disable_termination().unwrap();
            session.write_all(b"SECRET_COMMAND_PAYLOAD", deadline()).unwrap();
            session.read(32, deadline()).unwrap();
            assert!(session.close().unwrap().released);
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let pending = snapshot().active.into_iter().find(|call| call.request_id.as_deref() == Some(&request)).unwrap();
        assert_eq!(pending.stage, stage);
        assert_eq!(pending.outcome, Outcome::Pending);
        assert_eq!(pending.status, None);
        drop(release);
        worker.join().unwrap();
        assert!(!snapshot().active.iter().any(|call| call.id == pending.id));
    }
}

// Catches evicting an in-flight owner when unrelated completed calls wrap the history ring.
#[test]
fn pending_open_survives_history_wrap_without_releasing_its_reservation() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (api, entered, release) = Api::gated(Stage::Open);
    let book = ResourceBook::isolated();
    let manager = VisaManager::from_api(api, book.clone()).unwrap();
    let resource = manager.canonicalize("USB0::privateSerial::INSTR").unwrap();
    let observer = manager.clone();
    let owned = resource.clone();
    let worker = std::thread::spawn(move || {
        let _scope = visa_trace::scope("blocked-open", "instrument:bbbb");
        let mut session = manager.open(owned, deadline()).unwrap();
        assert!(session.close().unwrap().released);
    });
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    for _ in 0..150 { observer.canonicalize("GPIB0::4::INSTR").unwrap(); }
    let value = snapshot();
    assert!(value.recent.len() <= 64);
    let call = value.active.iter().find(|call| call.request_id.as_deref() == Some("blocked-open")).unwrap();
    assert_eq!(call.stage, Stage::Open);
    assert_eq!(call.outcome, Outcome::Pending);
    assert!(book.is_reserved(&resource));
    drop(release);
    worker.join().unwrap();
    assert!(!book.is_reserved(&resource));
}

// Catches missing transport stages or leaking command, sample, resource or serial text.
#[test]
fn release_trace_has_all_stages_without_raw_data_or_resource_names() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _scope = visa_trace::scope("all-stages", "instrument:cccc");
    let manager = VisaManager::from_api(Api::plain(), ResourceBook::isolated()).unwrap();
    let resource = manager.canonicalize("USB0::SECRET_DEVICE_SERIAL::INSTR").unwrap();
    let mut session = manager.open(resource, deadline()).unwrap();
    session.disable_termination().unwrap();
    session.write_all(b"SECRET_COMMAND_PAYLOAD", deadline()).unwrap();
    assert_eq!(session.read(32, deadline()).unwrap(), b"SECRET_SAMPLE_PAYLOAD");
    assert!(session.close().unwrap().released);
    drop(session);
    manager.release().unwrap();
    let value = snapshot();
    let calls: Vec<_> = value.recent.iter().filter(|call| call.request_id.as_deref() == Some("all-stages")).collect();
    assert_eq!(calls.iter().map(|call| call.stage).collect::<Vec<_>>(), vec![Stage::OpenDefaultRm, Stage::ParseResource, Stage::Open, Stage::SetTermination, Stage::SetTimeout, Stage::Write, Stage::SetTimeout, Stage::Read, Stage::Close, Stage::Close]);
    assert!(calls.iter().all(|call| call.outcome == Outcome::Returned));
    assert_eq!(calls[1].status, None); // parse's existing typed API exposes no successful raw status
    assert!(calls.iter().enumerate().filter(|(index, _)| *index != 1).all(|(_, call)| call.status == Some(0)));
    let encoded = serde_json::to_string(&value).unwrap();
    for secret in ["SECRET_COMMAND_PAYLOAD", "SECRET_SAMPLE_PAYLOAD", "SECRET_DEVICE_SERIAL", "USB0::"] { assert!(!encoded.contains(secret)); }
}

// Catches turning a native error into success or allowing later cleanup to rewrite its evidence.
#[test]
fn failed_read_diagnostics_do_not_release_or_rewrite_cleanup_ownership() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _scope = visa_trace::scope("failed-read", "instrument:dddd");
    let api = Arc::new(Api { block: None, fail_read: true });
    let book = ResourceBook::isolated();
    let manager = VisaManager::from_api(api, book.clone()).unwrap();
    let resource = manager.canonicalize("GPIB0::4::INSTR").unwrap();
    let mut session = manager.open(resource.clone(), deadline()).unwrap();
    assert!(matches!(session.read(32, deadline()), Err(DriverError::Timeout { transferred: 21, .. })));
    let before = snapshot().recent.into_iter().find(|call| call.request_id.as_deref() == Some("failed-read") && call.stage == Stage::Read).unwrap();
    assert_eq!(before.status, Some(VI_ERROR_TMO));
    assert_eq!(before.outcome, Outcome::Error);
    assert!(session.has_responsibility());
    assert!(book.is_reserved(&resource));
    assert!(session.close().unwrap().released);
    let after = snapshot().recent.into_iter().find(|call| call.id == before.id).unwrap();
    assert_eq!(before, after);
    assert!(!book.is_reserved(&resource));
}

// Catches accepting oversized/control-bearing identifiers or leaking a nested scope into later calls.
#[test]
fn scope_bounds_identifiers_and_restores_the_previous_caller() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _outer = visa_trace::scope("outer-scope", "instrument:eeee");
    let manager = {
        let _invalid = visa_trace::scope(&"x".repeat(97), "private\nvalue");
        VisaManager::from_api(Api::plain(), ResourceBook::isolated()).unwrap()
    };
    manager.release().unwrap();
    let value = snapshot();
    let last: Vec<_> = value.recent.iter().rev().take(2).collect();
    assert_eq!(last[0].request_id.as_deref(), Some("outer-scope"));
    assert_eq!(last[0].domain.as_deref(), Some("instrument:eeee"));
    assert_eq!(last[1].request_id, None);
    assert_eq!(last[1].domain, None);
    let boundary = "r".repeat(96);
    {
        let _boundary = visa_trace::scope(&boundary, "instrument:eeee");
        VisaManager::from_api(Api::plain(), ResourceBook::isolated()).unwrap().release().unwrap();
    }
    assert_eq!(snapshot().recent.last().unwrap().request_id.as_deref(), Some(boundary.as_str()));
}

// Catches unbounded active diagnostics or evicting an already-recorded hung call on saturation.
#[test]
fn active_saturation_is_bounded_and_reports_missing_diagnostics() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let prior = snapshot().dropped;
    let (api, entered, release) = Api::gated(Stage::OpenDefaultRm);
    let mut workers = Vec::new();
    for index in 0..65 {
        let api = api.clone();
        workers.push(std::thread::spawn(move || {
            let _scope = visa_trace::scope(&format!("saturated-{index}"), "instrument:ffff");
            VisaManager::from_api(api, ResourceBook::isolated()).unwrap().release().unwrap();
        }));
    }
    for _ in 0..65 { entered.recv_timeout(Duration::from_secs(2)).unwrap(); }
    let value = snapshot();
    assert_eq!(value.active.len(), 64);
    assert!(value.active.iter().all(|call| call.outcome == Outcome::Pending));
    assert!(value.dropped > prior);
    drop(release);
    for worker in workers { worker.join().unwrap(); }
    let done = snapshot();
    assert!(done.active.is_empty());
    assert!(done.recent.len() <= 64);
}
