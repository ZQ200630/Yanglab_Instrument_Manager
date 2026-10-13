//! Hardware-free scheduler admission tests with bounded native-call delays.
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use yang_drivers::{
    clock::SystemClock,
    lifecycle::{CleanupReport, CleanupStep},
    transport::Deadline,
};
use yang_protocol::{ContextV3, DomainConfig, DomainRef, Limits, OutcomeV3, Phase, RequestV3};
use yang_worker::{
    observations::Observation,
    scheduler::{Backend, Scheduler},
    DomainRegistry,
};

struct DelayedBackend {
    registry: DomainRegistry,
    held_ids: HashSet<String>,
    gate: (Mutex<(bool, HashSet<String>)>, Condvar),
}
impl DelayedBackend {
    fn new(domains: usize, held_ids: &[&str]) -> Arc<Self> {
        let registry = DomainRegistry::new(&"a".repeat(32)).unwrap();
        for number in 1..=domains {
            registry.configure(config(number)).unwrap();
        }
        Arc::new(Self {
            registry,
            held_ids: held_ids.iter().map(|id| (*id).into()).collect(),
            gate: (Mutex::new((false, HashSet::new())), Condvar::new()),
        })
    }
    fn request(&self, number: usize, id: &str, method: &str) -> RequestV3 {
        request(
            id,
            method,
            match method {
                "probe" | "check_online" => json!({"authorization":{}}),
                "action" => json!({"name":"held","args":{}}),
                _ => json!({}),
            },
            Some(self.registry.context(&config(number).domain).unwrap()),
        )
    }
    fn global(&self, id: &str, method: &str, params: Value) -> RequestV3 {
        request(
            id,
            method,
            params,
            Some(ContextV3 {
                session_id: self.registry.session_id().into(),
                domain: None,
                connection_id: None,
                epoch: 0,
            }),
        )
    }
    fn started(&self, id: &str) {
        let state = self.gate.0.lock().unwrap();
        let (state, _) = self
            .gate
            .1
            .wait_timeout_while(state, Duration::from_secs(2), |s| !s.1.contains(id))
            .unwrap();
        assert!(state.1.contains(id), "native call {id} did not start");
    }
    fn release(&self) {
        self.gate.0.lock().unwrap().0 = true;
        self.gate.1.notify_all();
    }
}
impl Backend for DelayedBackend {
    fn registry(&self) -> DomainRegistry {
        self.registry.clone()
    }
    fn execute(&self, request: &RequestV3) -> OutcomeV3 {
        let mut state = self.gate.0.lock().unwrap();
        state.1.insert(request.id.clone());
        self.gate.1.notify_all();
        if self.held_ids.contains(&request.id) {
            let (settled, _) = self
                .gate
                .1
                .wait_timeout_while(state, Duration::from_secs(5), |s| !s.0)
                .unwrap();
            assert!(settled.0, "bounded injected native call was not released");
        } else {
            drop(state);
        }
        let result = match request.method.as_str() {
            "configure_domain" => {
                let config = serde_json::from_value(request.params["config"].clone()).unwrap();
                let context = self.registry.configure(config).unwrap();
                json!({"context":context,"configured":true})
            }
            "connect" => json!({"connected":true,"status":{"state":"READY"}}),
            "disconnect" => {
                let cleanup = CleanupReport::new(
                    "f".repeat(32),
                    vec![CleanupStep {
                        role: "osa".into(),
                        action: "close".into(),
                        error: None,
                    }],
                    None,
                    vec![],
                )
                .unwrap();
                json!({"connected":false,"cleanup":cleanup})
            }
            "probe" | "check_online" => json!({"online":true,"release_confirmed":true}),
            "shutdown" => json!({"shutdown":true}),
            _ => json!({"status":{"state":"READY"}}),
        };
        OutcomeV3 {
            phase: Phase::Completed,
            context: request.context.clone(),
            result: Some(result),
            error: None,
        }
    }
    fn observe(&self, _: &ContextV3) -> Observation {
        Observation {
            status: json!({"state":"READY"}),
            more: false,
            sampled_at: None,
        }
    }
}
struct Release(Arc<DelayedBackend>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
fn config(number: usize) -> DomainConfig {
    DomainConfig {
        domain: DomainRef {
            kind: "device".into(),
            id: format!("{number:032x}"),
        },
        config_rev: 1,
        driver_kind: "osa".into(),
        model_id: "aq6370".into(),
        profile_id: Some("gpib-visa".into()),
        params: json!({"resource":format!("GPIB0::{number}::INSTR")}),
        expected_identity: json!({}),
        members: vec![],
    }
}
fn request(id: &str, method: &str, params: Value, context: Option<ContextV3>) -> RequestV3 {
    RequestV3 {
        v: 3,
        id: id.into(),
        method: method.into(),
        params,
        context,
    }
}
fn deadline() -> Deadline {
    Deadline::after(Duration::from_secs(2))
}
fn scheduler(backend: Arc<DelayedBackend>) -> Scheduler {
    Scheduler::new(backend, Arc::new(SystemClock::default()), Limits::default()).unwrap()
}
fn status(scheduler: &Scheduler, id: &str) -> Value {
    scheduler
        .submit(request(id, "status", json!({}), None))
        .unwrap()
        .wait(deadline())
        .unwrap()
        .result
        .unwrap()
}

#[test]
fn delayed_probe_leaves_management_and_other_probe_available() {
    let backend = DelayedBackend::new(2, &["delayed"]);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let delayed = scheduler
        .submit(backend.request(1, "delayed", "probe"))
        .unwrap();
    backend.started("delayed");
    let configured = scheduler
        .submit(backend.global(
            "configure-other",
            "configure_domain",
            json!({"config":config(3)}),
        ))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(
        configured.phase,
        Phase::Completed,
        "held probe occupied global management: {:?}",
        configured.error
    );
    let other = scheduler
        .submit(backend.request(2, "other-probe", "check_online"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(other.phase, Phase::Completed);
    let duplicate = scheduler
        .submit(backend.request(1, "duplicate-active-probe", "check_online"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(duplicate.phase, Phase::RejectedBeforeCall);
    assert_eq!(duplicate.error.unwrap().kind, "ResourceBusy");
    let overlapping = scheduler
        .submit(backend.request(1, "overlapping-connect", "connect"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(overlapping.phase, Phase::RejectedBeforeCall);
    assert!(backend
        .registry
        .context(&config(1).domain)
        .unwrap()
        .connection_id
        .is_none());
    assert_eq!(scheduler.snapshot().pending_normal, 0);
    backend.release();
    delayed.wait(deadline()).unwrap();
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}

#[test]
fn delayed_probes_leave_ordinary_connect_and_safety_available() {
    let backend = DelayedBackend::new(5, &["p1", "p2", "p3", "p4"]);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let mut pending = vec![];
    for (number, id) in [(1, "p1"), (2, "p2"), (3, "p3"), (4, "p4")] {
        let probe = scheduler
            .submit(backend.request(number, id, "probe"))
            .unwrap();
        assert!(
            probe.wait(Deadline::after(Duration::ZERO)).is_err(),
            "parallel probe was rejected"
        );
        backend.started(id);
        pending.push(probe);
    }
    let connected = scheduler
        .submit(backend.request(5, "unrelated-connect", "connect"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(connected.phase, Phase::Completed);
    let stopped = scheduler
        .submit(backend.request(5, "unrelated-disconnect", "disconnect"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(stopped.phase, Phase::Completed);
    assert_eq!(scheduler.snapshot().active, 4);
    assert_eq!(scheduler.snapshot().pending_normal, 0);
    backend.release();
    for probe in pending {
        assert_eq!(probe.wait(deadline()).unwrap().phase, Phase::Completed);
    }
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}

#[test]
fn probe_shutdown_is_admitted_but_retains_the_running_owner() {
    let backend = DelayedBackend::new(1, &["delayed"]);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let original = backend.request(1, "delayed", "probe");
    let context = original.context.clone();
    let probe = scheduler.submit(original).unwrap();
    backend.started("delayed");
    assert!(probe.wait(Deadline::after(Duration::ZERO)).is_err());
    let shutdown = scheduler
        .submit(backend.global("shutdown", "shutdown", json!({})))
        .unwrap();
    assert!(
        shutdown.wait(Deadline::after(Duration::ZERO)).is_err(),
        "shutdown was rejected instead of admitted"
    );
    let snapshot = scheduler.snapshot();
    assert!(snapshot.closing);
    assert_eq!(snapshot.active, 1);
    assert_eq!(snapshot.responsibility_slots, 2);
    assert!(scheduler
        .join_when_released(Deadline::after(Duration::ZERO))
        .is_err());
    let pending = status(&scheduler, "closing-status");
    assert_eq!(
        pending["domains"]["device:00000000000000000000000000000001"]["active_request_id"],
        "delayed"
    );
    backend.release();
    assert_eq!(probe.wait(deadline()).unwrap().context, context);
    assert_eq!(shutdown.wait(deadline()).unwrap().phase, Phase::Completed);
    scheduler.join_when_released(deadline()).unwrap();
}

#[test]
fn queued_probe_is_visible_bounded_and_fenced_by_safety() {
    let backend = DelayedBackend::new(1, &["action"]);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    scheduler
        .submit(backend.request(1, "connect", "connect"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    let action = scheduler
        .submit(backend.request(1, "action", "action"))
        .unwrap();
    backend.started("action");
    let probe = scheduler
        .submit(backend.request(1, "queued", "check_online"))
        .unwrap();
    let pending = status(&scheduler, "queued-status");
    assert_eq!(
        pending["domains"]["device:00000000000000000000000000000001"]["pending_request_id"],
        "queued"
    );
    let duplicate = scheduler
        .submit(backend.request(1, "duplicate-probe", "probe"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(duplicate.phase, Phase::RejectedBeforeCall);
    assert_eq!(duplicate.error.unwrap().kind, "ResourceBusy");
    assert_eq!(scheduler.snapshot().responsibility_slots, 2);
    let disconnect = scheduler
        .submit(backend.request(1, "disconnect", "disconnect"))
        .unwrap();
    assert_eq!(
        probe.wait(deadline()).unwrap().phase,
        Phase::SupersededBeforeCall
    );
    let pending = status(&scheduler, "fenced-status");
    assert!(
        pending["domains"]["device:00000000000000000000000000000001"]["pending_request_id"]
            .is_null()
    );
    assert_eq!(scheduler.snapshot().responsibility_slots, 2);
    backend.release();
    action.wait(deadline()).unwrap();
    assert_eq!(disconnect.wait(deadline()).unwrap().phase, Phase::Completed);
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}

#[test]
fn completed_probe_does_not_publish_a_connected_domain() {
    let backend = DelayedBackend::new(1, &[]);
    let scheduler = scheduler(backend.clone());
    let reply = scheduler
        .submit(backend.request(1, "probe", "probe"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(reply.phase, Phase::Completed);
    let snapshot = scheduler.snapshot();
    assert_eq!(snapshot.responsibility_slots, 0);
    assert_eq!(snapshot.pending_normal, 0);
    assert!(snapshot.domains[0].context.connection_id.is_none());
    assert!(!snapshot.domains[0].responsibility);
    let observed = status(&scheduler, "completed-status");
    assert_eq!(
        observed["domains"]["device:00000000000000000000000000000001"]["state"],
        "DISCONNECTED"
    );
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}

#[test]
fn shutdown_drains_a_queued_probe_without_releasing_an_active_action() {
    let backend = DelayedBackend::new(1, &["action"]);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    scheduler
        .submit(backend.request(1, "connect", "connect"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    let action = scheduler
        .submit(backend.request(1, "action", "action"))
        .unwrap();
    backend.started("action");
    let probe = scheduler
        .submit(backend.request(1, "queued", "probe"))
        .unwrap();
    let shutdown = scheduler
        .submit(backend.global("shutdown", "shutdown", json!({})))
        .unwrap();
    assert_eq!(
        probe.wait(deadline()).unwrap().phase,
        Phase::SupersededBeforeCall
    );
    assert!(action.wait(Deadline::after(Duration::ZERO)).is_err());
    assert!(shutdown.wait(Deadline::after(Duration::ZERO)).is_err());
    let observed = status(&scheduler, "drained-status");
    let domain = &observed["domains"]["device:00000000000000000000000000000001"];
    assert_eq!(domain["active_request_id"], "action");
    assert!(domain["pending_request_id"].is_null());
    backend.release();
    action.wait(deadline()).unwrap();
    assert_eq!(shutdown.wait(deadline()).unwrap().phase, Phase::Completed);
    scheduler.join_when_released(deadline()).unwrap();
}

#[test]
fn parallel_probes_obey_the_shared_responsibility_ceiling() {
    let backend = DelayedBackend::new(3, &["p1", "p2"]);
    let scheduler = Scheduler::new(
        backend.clone(),
        Arc::new(SystemClock::default()),
        Limits {
            max_responsibility_slots: 2,
            ..Limits::default()
        },
    )
    .unwrap();
    let _release = Release(backend.clone());
    let first = scheduler.submit(backend.request(1, "p1", "probe")).unwrap();
    backend.started("p1");
    let second = scheduler
        .submit(backend.request(2, "p2", "check_online"))
        .unwrap();
    backend.started("p2");
    let rejected = scheduler
        .submit(backend.request(3, "overflow", "probe"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(rejected.phase, Phase::RejectedBeforeCall);
    assert_eq!(rejected.error.unwrap().kind, "Capacity");
    assert_eq!(scheduler.snapshot().responsibility_slots, 2);
    assert_eq!(scheduler.snapshot().pending_normal, 0);
    backend.release();
    first.wait(deadline()).unwrap();
    second.wait(deadline()).unwrap();
    assert_eq!(scheduler.snapshot().responsibility_slots, 0);
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
