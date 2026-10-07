use serde_json::json;
use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use yang_drivers::{
    clock::SystemClock,
    lifecycle::{CleanupReport, CleanupStep, DriverState},
    transport::Deadline,
};
use yang_protocol::{ContextV3, DomainConfig, DomainRef, Limits, OutcomeV3, Phase, RequestV3};
use yang_worker::{
    observations::Observation,
    scheduler::{Backend, Scheduler},
    DomainRegistry,
};
struct TestBackend {
    registry: DomainRegistry,
    gate: (Mutex<(bool, usize)>, Condvar),
    readback_error: bool,
    stop_events: Mutex<Vec<ContextV3>>,
    stop_gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    close_events: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}
impl TestBackend {
    fn new(count: usize, readback_error: bool) -> Arc<Self> {
        let registry = DomainRegistry::new(&"a".repeat(32)).unwrap();
        for number in 1..=count {
            let domain = DomainRef {
                kind: "device".into(),
                id: format!("{number:032x}"),
            };
            registry
                .configure(DomainConfig {
                    domain: domain.clone(),
                    config_rev: 1,
                    driver_kind: "osa".into(),
                    model_id: "aq6370".into(),
                    profile_id: Some("gpib-visa".into()),
                    params: json!({"resource":format!("GPIB0::{number}::INSTR")}),
                    expected_identity: json!({}),
                    members: vec![],
                })
                .unwrap();
            let context = registry.bind(&domain, &format!("{number:032x}")).unwrap();
            registry.publish(&context, DriverState::Ready);
        }
        Arc::new(Self {
            registry,
            gate: (Mutex::new((false, 0)), Condvar::new()),
            readback_error,
            stop_events: Mutex::new(vec![]),
            stop_gate: Mutex::new(None),
            close_events: Mutex::new(None),
        })
    }
    fn request(&self, number: usize, id: &str, method: &str) -> RequestV3 {
        let domain = DomainRef {
            kind: "device".into(),
            id: format!("{number:032x}"),
        };
        RequestV3 {
            v: 3,
            id: id.into(),
            method: method.into(),
            params: if method == "action" {
                json!({"name":"held","args":{}})
            } else {
                json!({})
            },
            context: Some(self.registry.context(&domain).unwrap()),
        }
    }
    fn release(&self) {
        self.gate.0.lock().unwrap().0 = true;
        self.gate.1.notify_all();
    }
    fn started(&self, count: usize) {
        let state = self.gate.0.lock().unwrap();
        let (state, _) = self
            .gate
            .1
            .wait_timeout_while(state, Duration::from_secs(2), |s| s.1 < count)
            .unwrap();
        assert!(state.1 >= count, "backend did not enter expected calls");
    }
}
impl Backend for TestBackend {
    fn registry(&self) -> DomainRegistry {
        self.registry.clone()
    }
    fn request_stop(&self, context: &ContextV3) {
        self.stop_events.lock().unwrap().push(context.clone());
        if let Some(gate) = self.stop_gate.lock().unwrap().take() {
            gate.recv_timeout(Duration::from_secs(2)).unwrap();
        }
    }
    fn execute(&self, request: &RequestV3) -> OutcomeV3 {
        let result = if request.method == "disconnect" {
            if let Some(events) = self.close_events.lock().unwrap().take() {
                events.send(()).unwrap();
            }
            let report = CleanupReport::new(
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
            json!({"connected":false,"cleanup":report})
        } else {
            let mut state = self.gate.0.lock().unwrap();
            state.1 += 1;
            self.gate.1.notify_all();
            let _ = self
                .gate
                .1
                .wait_timeout_while(state, Duration::from_secs(2), |s| !s.0)
                .unwrap();
            json!({"status":{"state":"READY"},"post_readback":self.readback_error})
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
            status: if self.readback_error {
                json!({"observation_error":{"type":"ReadFailed","message":"offline injected error"}})
            } else {
                json!({"state":"READY"})
            },
            more: false,
            sampled_at: None,
        }
    }
}
struct Release(Arc<TestBackend>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
fn deadline() -> Deadline {
    Deadline::after(Duration::from_secs(2))
}
fn scheduler(backend: Arc<TestBackend>) -> Scheduler {
    Scheduler::new(backend, Arc::new(SystemClock::default()), Limits::default()).unwrap()
}
#[test]
fn bounded_admission() {
    let limits = Limits::default();
    assert_eq!(
        (
            limits.max_pending_normal,
            limits.max_reply_slots,
            limits.max_responsibility_slots,
            limits.max_domains
        ),
        (31, 226, 225, 64)
    );
    let backend = TestBackend::new(32, false);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let mut replies = Vec::new();
    for number in 1..=31 {
        replies.push(
            scheduler
                .submit(backend.request(number, &format!("work-{number}"), "action"))
                .unwrap(),
        );
    }
    let full = scheduler
        .submit(backend.request(32, "overflow", "action"))
        .unwrap()
        .wait(deadline())
        .unwrap();
    assert_eq!(full.phase, Phase::RejectedBeforeCall);
    assert!(scheduler.snapshot().responsibility_slots <= 225);
    backend.release();
    for reply in replies {
        assert_eq!(reply.wait(deadline()).unwrap().phase, Phase::Completed);
    }
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
#[test]
fn safety_fences_queued_normal_work() {
    let backend = TestBackend::new(5, false);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let mut active = Vec::new();
    for number in 1..=4 {
        active.push(
            scheduler
                .submit(backend.request(number, &format!("active-{number}"), "action"))
                .unwrap(),
        );
    }
    backend.started(4);
    let queued = scheduler
        .submit(backend.request(5, "queued", "action"))
        .unwrap();
    let stop = scheduler
        .submit(backend.request(5, "stop", "disconnect"))
        .unwrap();
    assert_eq!(
        queued.wait(deadline()).unwrap().phase,
        Phase::SupersededBeforeCall
    );
    assert_eq!(stop.wait(deadline()).unwrap().phase, Phase::Completed);
    backend.release();
    for reply in active {
        reply.wait(deadline()).unwrap();
    }
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
#[test]
fn late_completion_keeps_original_context() {
    let backend = TestBackend::new(1, false);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let request = backend.request(1, "held", "action");
    let context = request.context.clone();
    let reply = scheduler.submit(request).unwrap();
    backend.started(1);
    let stop = scheduler
        .submit(backend.request(1, "stop", "disconnect"))
        .unwrap();
    assert!(reply.wait(Deadline::after(Duration::ZERO)).is_err());
    assert!(scheduler
        .join_when_released(Deadline::after(Duration::ZERO))
        .is_err());
    backend.release();
    assert_eq!(reply.wait(deadline()).unwrap().context, context);
    stop.wait(deadline()).unwrap();
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
#[test]
fn stalled_domain_does_not_block_status() {
    let backend = TestBackend::new(1, false);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let reply = scheduler
        .submit(backend.request(1, "held", "action"))
        .unwrap();
    backend.started(1);
    let status = scheduler
        .submit(RequestV3 {
            v: 3,
            id: "status".into(),
            method: "status".into(),
            params: json!({}),
            context: None,
        })
        .unwrap();
    assert_eq!(status.wait(deadline()).unwrap().phase, Phase::Completed);
    assert!(scheduler.snapshot().active >= 1);
    backend.release();
    reply.wait(deadline()).unwrap();
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
#[test]
fn readback_failure_has_distinct_phase() {
    let backend = TestBackend::new(1, true);
    backend.release();
    let scheduler = scheduler(backend.clone());
    let reply = scheduler
        .submit(backend.request(1, "write", "action"))
        .unwrap();
    assert_eq!(
        reply.wait(deadline()).unwrap().phase,
        Phase::CompletedReadbackFailed
    );
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
#[test]
fn observer_callbacks_cannot_lose_reply() {
    let backend = TestBackend::new(1, false);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let reply = scheduler
        .submit(backend.request(1, "held", "action"))
        .unwrap();
    backend.started(1);
    reply.on_complete(|_| panic!("injected consumer panic"));
    let (tx, rx) = std::sync::mpsc::channel();
    reply.on_complete(move |outcome| {
        tx.send(outcome.phase).unwrap();
    });
    backend.release();
    assert_eq!(reply.wait(deadline()).unwrap().phase, Phase::Completed);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Phase::Completed
    );
    assert_eq!(scheduler.snapshot().callback_errors, 1);
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
#[test]
fn unknown_domain_does_not_leak_reply_slots() {
    let backend = TestBackend::new(1, false);
    let scheduler = scheduler(backend.clone());
    let mut request = backend.request(1, "unknown", "action");
    request
        .context
        .as_mut()
        .unwrap()
        .domain
        .as_mut()
        .unwrap()
        .id = "9".repeat(32);
    assert!(scheduler.submit(request).is_err());
    assert_eq!(scheduler.snapshot().reply_slots, 0);
    backend.release();
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
#[test]
fn safety_fences_scoped_management_queue() {
    let backend = TestBackend::new(1, false);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let action = scheduler
        .submit(backend.request(1, "held", "action"))
        .unwrap();
    backend.started(1);
    let mut probe_request = backend.request(1, "check", "check_online");
    probe_request.params = json!({"authorization":{}});
    let probe = scheduler.submit(probe_request).unwrap();
    let stop = scheduler
        .submit(backend.request(1, "stop", "disconnect"))
        .unwrap();
    assert_eq!(
        probe
            .wait(Deadline::after(Duration::from_millis(50)))
            .unwrap()
            .phase,
        Phase::SupersededBeforeCall
    );
    backend.release();
    action.wait(deadline()).unwrap();
    assert_eq!(stop.wait(deadline()).unwrap().phase, Phase::Completed);
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
#[test]
fn stop_interlock_is_notified_while_native_call_is_retained() {
    let backend = TestBackend::new(1, false);
    let scheduler = scheduler(backend.clone());
    let _release = Release(backend.clone());
    let action = scheduler
        .submit(backend.request(1, "held", "action"))
        .unwrap();
    backend.started(1);
    let stop = scheduler
        .submit(backend.request(1, "stop", "disconnect"))
        .unwrap();
    assert_eq!(backend.stop_events.lock().unwrap().len(), 1);
    assert!(action.wait(Deadline::after(Duration::ZERO)).is_err());
    backend.release();
    action.wait(deadline()).unwrap();
    stop.wait(deadline()).unwrap();
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}

#[test]
fn canceled_obligation_settles_before_safety_can_release() {
    let backend = TestBackend::new(5, false);
    let scheduler = Arc::new(scheduler(backend.clone()));
    let _release = Release(backend.clone());
    let active: Vec<_> = (1..=4)
        .map(|number| {
            scheduler
                .submit(backend.request(number, &format!("held-{number}"), "action"))
                .unwrap()
        })
        .collect();
    backend.started(4);
    let queued = scheduler
        .submit(backend.request(5, "queued", "action"))
        .unwrap();
    let (close_tx, close_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *backend.close_events.lock().unwrap() = Some(close_tx);
    *backend.stop_gate.lock().unwrap() = Some(resume_rx);
    let (owner, source) = (scheduler.clone(), backend.clone());
    let submitter = std::thread::spawn(move || {
        owner
            .submit(source.request(5, "stop", "disconnect"))
            .unwrap()
    });
    close_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    // The notification callback is paused outside the scheduler lock. At this
    // boundary only the safety attempt may remain; queued work never began I/O.
    let pending = backend
        .registry
        .snapshot()
        .into_iter()
        .find(|s| s.context.domain.as_ref().unwrap().id == format!("{:032x}", 5))
        .unwrap()
        .pending;
    resume_tx.send(()).unwrap();
    let stop = submitter.join().unwrap();
    assert!(
        pending <= 1,
        "canceled ordinary obligation remained: {pending}"
    );
    assert_eq!(
        queued.wait(deadline()).unwrap().phase,
        Phase::SupersededBeforeCall
    );
    assert_eq!(stop.wait(deadline()).unwrap().phase, Phase::Completed);
    backend.release();
    for reply in active {
        reply.wait(deadline()).unwrap();
    }
    scheduler.begin_shutdown();
    scheduler.join_when_released(deadline()).unwrap();
}
