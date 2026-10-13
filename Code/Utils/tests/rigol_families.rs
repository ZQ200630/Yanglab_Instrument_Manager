use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use yang_data::Samples;
use yang_drivers::{
    clock::{Clock, ManualClock},
    generator::{Generator, GeneratorModel, GeneratorOptions, SignalConfig, Waveform},
    scope::{
        decode_block, parse_preamble, CaptureRequest, CaptureSource, Scope, ScopeModel,
        ScopeOptions,
    },
    transport::{ByteTransport, CloseReport, Deadline},
    DriverError, DriverResult,
};

#[test]
fn default_screen_point_budget_is_rejected_before_instrument_io() {
    let (io, evidence) = transport(vec![("*IDN?", b"RIGOL,MHO5054,S1,1\n".to_vec())], 127);
    let mut scope = Scope::with_transport(
        ScopeModel::Mho5054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    let request = CaptureRequest {
        channels: vec![1, 2, 3, 4],
        source: CaptureSource::Screen,
        points: None,
        byte_budget: 4096,
    };
    assert!(matches!(
        scope.capture(&request, deadline()),
        Err(DriverError::Invalid(_))
    ));
    assert_eq!(evidence.lock().unwrap().writes, vec!["*IDN?\n"]);
    scope.close().unwrap();
}

#[derive(Default)]
struct Evidence {
    writes: Vec<String>,
    closes: usize,
    fail_closes: usize,
    reads: usize,
    cancel_after_read: Option<(usize, yang_drivers::scope::StopHandle)>,
    empty_error: Option<DriverError>,
    report_message_end: bool,
    empty_read_attempts: usize,
    io_deadlines_ms: Vec<u32>,
    read_sizes: Vec<usize>,
    expire_deadline_after_read: Option<usize>,
}
struct Transcript {
    exchanges: VecDeque<(String, Vec<u8>)>,
    pending: VecDeque<u8>,
    evidence: Arc<Mutex<Evidence>>,
    live: bool,
    fragment: usize,
}
impl ByteTransport for Transcript {
    fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<()> {
        let command = String::from_utf8(bytes.to_vec()).unwrap();
        let mut evidence = self.evidence.lock().unwrap();
        evidence.writes.push(command.clone());
        evidence.io_deadlines_ms.push(deadline.remaining_millis()?);
        drop(evidence);
        let Some((want, reply)) = self.exchanges.pop_front() else {
            return Err(DriverError::Protocol(format!(
                "unexpected command {command}"
            )));
        };
        if command != want {
            return Err(DriverError::Protocol(format!(
                "wanted {want:?}, got {command:?}"
            )));
        }
        if !self.pending.is_empty() {
            return Err(DriverError::Protocol("previous reply not drained".into()));
        }
        self.pending.extend(reply);
        Ok(())
    }
    fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
        assert!(maximum <= 16384);
        let mut evidence = self.evidence.lock().unwrap();
        evidence.io_deadlines_ms.push(deadline.remaining_millis()?);
        evidence.read_sizes.push(maximum);
        drop(evidence);
        let count = maximum.min(self.fragment).min(self.pending.len());
        if count == 0 {
            self.evidence.lock().unwrap().empty_read_attempts += 1;
            return Err(self
                .evidence
                .lock()
                .unwrap()
                .empty_error
                .clone()
                .unwrap_or_else(|| DriverError::Protocol("truncated transcript".into())));
        }
        let reply = self.pending.drain(..count).collect();
        let mut evidence = self.evidence.lock().unwrap();
        evidence.reads += 1;
        if let Some((at, stop)) = &evidence.cancel_after_read {
            if evidence.reads == *at {
                stop.request_stop();
            }
        }
        let expire_deadline = evidence.expire_deadline_after_read == Some(evidence.reads);
        drop(evidence);
        if expire_deadline {
            // Model a matching response delivered at the I/O deadline boundary.
            // This is a bounded transport double, not hardware or a timing ETA.
            for _ in 0..100 {
                if deadline.remaining_millis().is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(deadline.remaining_millis().is_err());
        }
        Ok(reply)
    }
    fn read_fragment(
        &mut self,
        maximum: usize,
        deadline: Deadline,
    ) -> DriverResult<yang_drivers::transport::ReadFragment> {
        let bytes = self.read_bounded(maximum, deadline)?;
        Ok(yang_drivers::transport::ReadFragment {
            bytes,
            end_of_message: self.evidence.lock().unwrap().report_message_end
                && self.pending.is_empty(),
        })
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        let mut evidence = self.evidence.lock().unwrap();
        evidence.closes += 1;
        if evidence.closes <= evidence.fail_closes {
            return Ok(CloseReport {
                released: false,
                status: Some(-1),
            });
        }
        self.live = false;
        Ok(CloseReport {
            released: true,
            status: Some(0),
        })
    }
    fn has_responsibility(&self) -> bool {
        self.live
    }
}
fn transport<S: AsRef<str>>(
    entries: Vec<(S, Vec<u8>)>,
    fragment: usize,
) -> (Box<dyn ByteTransport>, Arc<Mutex<Evidence>>) {
    let evidence = Arc::new(Mutex::new(Evidence::default()));
    (
        Box::new(Transcript {
            exchanges: entries
                .into_iter()
                .map(|(s, b)| (format!("{}\n", s.as_ref()), b))
                .collect(),
            pending: VecDeque::new(),
            evidence: evidence.clone(),
            live: true,
            fragment,
        }),
        evidence,
    )
}
fn deadline() -> Deadline {
    Deadline::after(Duration::from_secs(3))
}

// Catches acknowledging a stopped DS7054 depth write without applying it or
// restoring the stopped acquisition state before reporting success.
#[test]
fn stopped_ds7054_depth_is_applied_and_verified_before_restoring_stop() {
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":RUN", vec![]),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", b"100K\n".to_vec()),
            (":STOP", vec![]),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"100K\n".to_vec()),
        ],
        127,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    scope.set_memory_depth(100_000, deadline()).unwrap();
    assert_eq!(evidence.lock().unwrap().writes.len(), 10);
    scope.close().unwrap();
    assert_eq!(evidence.lock().unwrap().writes.len(), 10);
}

// Catches automatically running an already running scope or trusting an old
// depth readback instead of the depth that was actually applied.
#[test]
fn running_scope_depth_waits_for_exact_readback_without_replaying_write() {
    let clock = Arc::new(ManualClock::default());
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"AUTO\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth?", b"100K\n".to_vec()),
        ],
        127,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        clock.clone(),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    scope.set_memory_depth(100_000, deadline()).unwrap();
    assert_eq!(clock.now(), Duration::from_millis(20));
    assert_eq!(evidence.lock().unwrap().writes.len(), 6);
    scope.close().unwrap();
}

// Catches treating the per-packet I/O stall timeout as the entire depth
// application budget even while every query completes promptly.
#[test]
fn responsive_depth_application_can_outlast_one_io_stall_budget() {
    let clock = Arc::new(ManualClock::default());
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth 1000000", vec![]),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth?", b"1M\n".to_vec()),
        ],
        127,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054, io, clock.clone(),
        ScopeOptions { timeout: Duration::from_millis(60), ..Default::default() },
    ).unwrap();
    scope.connect().unwrap();
    scope.set_memory_depth(1_000_000, deadline()).unwrap();
    assert_eq!(clock.now(), Duration::from_millis(80));
    assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Ready);
    assert_eq!(evidence.lock().unwrap().writes.iter()
        .filter(|command| command.as_str() == ":ACQuire:MDEPth 1000000\n").count(), 1);
    assert!(!evidence.lock().unwrap().writes.iter()
        .any(|command| command == ":RUN\n" || command == ":STOP\n"));
    scope.close().unwrap();
}

// Catches unbounded/zero native confirmation budgets before any resource opens.
#[test]
fn depth_application_budget_is_nonzero_and_cannot_exceed_native_ceiling() {
    for depth_timeout in [Duration::ZERO, Duration::from_secs(31)] {
        let (io, evidence) = transport::<&str>(vec![], 127);
        assert!(matches!(Scope::with_transport(
            ScopeModel::Ds7054, io, Arc::new(ManualClock::default()),
            ScopeOptions { depth_timeout, ..Default::default() },
        ), Err(DriverError::Invalid(_))));
        assert!(evidence.lock().unwrap().writes.is_empty());
    }
}

// Catches hiding the requested and last actually reported values after an
// ignored write, or automatically replaying it / changing RUN on failure.
#[test]
fn ignored_running_depth_reports_bounded_context_and_releases_without_replay() {
    let clock = Arc::new(ManualClock::default());
    let (io, evidence) = transport(vec![
        ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
        (":TRIGger:STATus?", b"RUN\n".to_vec()),
        (":ACQuire:MDEPth?", b"10M\n".to_vec()),
        (":ACQuire:MDEPth 1000000", vec![]),
        (":ACQuire:MDEPth?", b"10M\n".to_vec()),
        (":ACQuire:MDEPth?", b"10M\n".to_vec()),
        (":ACQuire:MDEPth?", b"10M\n".to_vec()),
    ], 127);
    let mut scope = Scope::with_transport(ScopeModel::Ds7054, io, clock.clone(),
        ScopeOptions { depth_timeout: Duration::from_millis(40), ..Default::default() },
    ).unwrap();
    scope.connect().unwrap();
    let error = scope.set_memory_depth(1_000_000, deadline()).unwrap_err();
    assert!(matches!(error, DriverError::Timeout { operation, .. }
        if operation.contains("requested=1000000")
            && operation.contains("last_reported=10000000")
            && operation.contains("no retry")));
    assert_eq!(clock.now(), Duration::from_millis(40));
    assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Fault);
    assert_eq!(evidence.lock().unwrap().writes.len(), 7);
    assert!(scope.close().unwrap().resources_released());
    assert_eq!(evidence.lock().unwrap().writes.len(), 7);
}

// Catches losing the last readback when the wall-clock deadline expires before
// the next polling query (ManualClock alone cannot exercise that boundary).
#[test]
fn depth_wall_clock_deadline_preserves_last_readback_context() {
    let (io, evidence) = transport(vec![
        ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
        (":TRIGger:STATus?", b"RUN\n".to_vec()),
        (":ACQuire:MDEPth?", b"10M\n".to_vec()),
        (":ACQuire:MDEPth 1000000", vec![]),
        (":ACQuire:MDEPth?", b"10M\n".to_vec()),
    ], 127);
    evidence.lock().unwrap().expire_deadline_after_read = Some(4);
    let mut scope = Scope::with_transport(ScopeModel::Ds7054, io,
        Arc::new(ManualClock::default()),
        ScopeOptions { depth_timeout: Duration::from_millis(40), ..Default::default() },
    ).unwrap();
    scope.connect().unwrap();
    let error = scope.set_memory_depth(1_000_000, deadline()).unwrap_err();
    assert!(matches!(error, DriverError::Timeout { operation, .. }
        if operation.contains("requested=1000000")
            && operation.contains("last_reported=10000000")
            && operation.contains("deadline")
            && operation.contains("no retry")));
    assert_eq!(evidence.lock().unwrap().writes.len(), 5);
    assert!(scope.close().unwrap().resources_released());
}

// Catches leaving a scope running after an uncertain write or replaying the
// write automatically after a transport timeout.
#[test]
fn stopped_scope_depth_timeout_attempts_stop_once_and_never_replays_depth() {
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":RUN", vec![]),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", vec![]),
            (":STOP", vec![]),
        ],
        127,
    );
    evidence.lock().unwrap().empty_error = Some(DriverError::Timeout {
        operation: "injected depth readback stall".into(),
        transferred: 0,
    });
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    assert!(matches!(
        scope.set_memory_depth(100_000, deadline()),
        Err(DriverError::Timeout { .. })
    ));
    assert_eq!(
        evidence.lock().unwrap().writes,
        vec![
            "*IDN?\n", ":TRIGger:STATus?\n", ":ACQuire:MDEPth?\n", ":RUN\n", ":TRIGger:STATus?\n",
            ":ACQuire:MDEPth 100000\n", ":ACQuire:MDEPth?\n", ":STOP\n"
        ]
    );
    scope.close().unwrap();
}

// Catches a permanently ignored depth command being acknowledged as completed,
// or a second application write being sent while attempting to confirm it.
#[test]
fn ignored_stopped_scope_depth_fails_bounded_and_attempts_stop_once() {
    let clock = Arc::new(ManualClock::default());
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":RUN", vec![]),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":STOP", vec![]),
        ],
        127,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        clock.clone(),
        ScopeOptions {
            timeout: Duration::from_millis(40),
            depth_timeout: Duration::from_millis(40),
            ..Default::default()
        },
    )
    .unwrap();
    scope.connect().unwrap();
    assert!(matches!(
        scope.set_memory_depth(100_000, deadline()),
        Err(DriverError::Timeout { .. })
    ));
    assert_eq!(clock.now(), Duration::from_millis(40));
    assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Fault);
    assert_eq!(evidence.lock().unwrap().writes.len(), 10);
    scope.close().unwrap();
}

// Catches using a running readback as proof of the final frozen buffer's depth.
#[test]
fn stopped_scope_depth_final_buffer_mismatch_is_fault_without_second_stop() {
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":RUN", vec![]),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", b"100K\n".to_vec()),
            (":STOP", vec![]),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
        ],
        127,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    assert!(matches!(
        scope.set_memory_depth(100_000, deadline()),
        Err(DriverError::Protocol(_))
    ));
    assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Fault);
    assert_eq!(evidence.lock().unwrap().writes.len(), 10);
    scope.close().unwrap();
}

// Catches applying a DS7000-specific acquisition workaround to an unreviewed
// MHO5054 stopped-depth failure.
#[test]
fn ignored_mho_depth_fails_without_automatic_acquisition_changes() {
    let clock = Arc::new(ManualClock::default());
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,MHO5054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
        ],
        127,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Mho5054,
        io,
        clock.clone(),
        ScopeOptions {
            timeout: Duration::from_millis(20),
            depth_timeout: Duration::from_millis(20),
            ..Default::default()
        },
    )
    .unwrap();
    scope.connect().unwrap();
    assert!(matches!(
        scope.set_memory_depth(100_000, deadline()),
        Err(DriverError::Timeout { .. })
    ));
    assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Fault);
    assert_eq!(evidence.lock().unwrap().writes.len(), 6);
    scope.close().unwrap();
}

// Catches replacing an already matching frozen buffer during every Acquire.
#[test]
fn unchanged_stopped_depth_is_confirmed_without_writes_or_refreshing_buffer() {
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"100K\n".to_vec()),
        ],
        127,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    scope.set_memory_depth(100_000, deadline()).unwrap();
    assert_eq!(evidence.lock().unwrap().writes.len(), 3);
    scope.close().unwrap();
}

// Catches suppressing owned STOP cleanup when the application deadline expires
// just as the matching depth readback arrives.
#[test]
fn stopped_depth_matching_reply_at_deadline_still_sends_and_confirms_one_stop() {
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":RUN", vec![]),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", b"100K\n".to_vec()),
            (":STOP", vec![]),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"100K\n".to_vec()),
        ],
        127,
    );
    evidence.lock().unwrap().expire_deadline_after_read = Some(5);
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions {
            timeout: Duration::from_millis(40),
            depth_timeout: Duration::from_millis(40),
            close_timeout: Duration::from_millis(40),
            ..Default::default()
        },
    ).unwrap();
    scope.connect().unwrap();
    scope.set_memory_depth(100_000, deadline()).unwrap();
    assert_eq!(evidence.lock().unwrap().writes.len(), 10);
    assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Ready);
    scope.close().unwrap();
}

// Catches losing the native/protocol failure or overstating a cleanup write as
// confirmed STOP evidence after failed depth application.
#[test]
fn failed_depth_keeps_original_error_and_reports_unconfirmed_stop_cleanup() {
    for injected in [
        DriverError::Native {
            operation: "injected read failure".into(), status: -1073807339, transferred: 17,
        },
        DriverError::Protocol("injected malformed reply".into()),
    ] {
        let (io, evidence) = transport(
            vec![
                ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
                (":TRIGger:STATus?", b"STOP\n".to_vec()),
                (":ACQuire:MDEPth?", b"10M\n".to_vec()),
                (":RUN", vec![]),
                (":TRIGger:STATus?", b"RUN\n".to_vec()),
                (":ACQuire:MDEPth 100000", vec![]),
                (":ACQuire:MDEPth?", vec![]),
                (":STOP", vec![]),
            ],
            127,
        );
        evidence.lock().unwrap().empty_error = Some(injected.clone());
        let mut scope = Scope::with_transport(
            ScopeModel::Ds7054, io, Arc::new(ManualClock::default()), ScopeOptions::default(),
        ).unwrap();
        scope.connect().unwrap();
        let error = scope.set_memory_depth(100_000, deadline()).unwrap_err();
        match (&injected, &error) {
            (DriverError::Native { status, transferred, .. },
             DriverError::Native { status: actual, transferred: bytes, .. }) => {
                assert_eq!(status, actual);
                assert_eq!(transferred, bytes);
            }
            (DriverError::Protocol(_), DriverError::Protocol(_)) => {}
            _ => panic!("original error type was lost: {error}"),
        }
        let detail = error.to_string();
        assert!(detail.contains("injected"));
        assert!(detail.contains("STOP cleanup sent, state unconfirmed"));
        assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Fault);
        assert_eq!(evidence.lock().unwrap().writes.len(), 8);
        scope.close().unwrap();
    }
}

// Catches collapsing a native depth failure into a generic protocol error when
// the sole STOP cleanup write also fails.
#[test]
fn failed_depth_and_failed_stop_keep_native_status_and_uncertain_cleanup_evidence() {
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"10M\n".to_vec()),
            (":RUN", vec![]),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", vec![]),
            // No accepted STOP exchange: the cleanup write must fail once.
        ],
        127,
    );
    evidence.lock().unwrap().empty_error = Some(DriverError::Native {
        operation: "injected depth failure".into(), status: -1073807339, transferred: 17,
    });
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054, io, Arc::new(ManualClock::default()), ScopeOptions::default(),
    ).unwrap();
    scope.connect().unwrap();
    match scope.set_memory_depth(100_000, deadline()).unwrap_err() {
        DriverError::Native { operation, status, transferred } => {
            assert_eq!(status, -1073807339);
            assert_eq!(transferred, 17);
            assert!(operation.contains("injected depth failure"));
            assert!(operation.contains("STOP cleanup failed"));
            assert!(operation.contains("state uncertain"));
        }
        error => panic!("native evidence was lost: {error}"),
    }
    assert_eq!(evidence.lock().unwrap().writes.len(), 8);
    assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Fault);
    scope.close().unwrap();
}

// The wire fixture is bounded independently from the driver's DATA window:
// it contains the requested full-depth record, but permits only 16 KiB reads.
fn ds_memory_entries(points: usize, windows: &[(usize, usize)]) -> Vec<(String, Vec<u8>)> {
    let mut entries = vec![
        ("*IDN?".into(), b"RIGOL,DS7054,S1,1\n".to_vec()),
        (":TRIGger:STATus?".into(), b"STOP\n".to_vec()),
        (":ACQuire:MDEPth?".into(), format!("{points}\n").into_bytes()),
        (":WAVeform:SOURce CHANnel1".into(), vec![]),
        (":WAVeform:MODE RAW".into(), vec![]),
        (":WAVeform:FORMat BYTE".into(), vec![]),
        (":WAVeform:STARt 1".into(), vec![]),
        (format!(":WAVeform:POINts {points}"), vec![]),
        (format!(":WAVeform:STOP {points}"), vec![]),
        (":WAVeform:PREamble?".into(), format!("0,2,{points},1,1e-6,0,0,0.1,0,128\n").into_bytes()),
    ];
    for &(start, stop) in windows {
        let count = stop - start + 1;
        let length = count.to_string();
        let mut block = format!("#{}{length}", length.len()).into_bytes();
        block.extend((start - 1..stop).map(|index| (index % 251) as u8));
        block.push(b'\n');
        entries.extend([
            (format!(":WAVeform:STARt {start}"), vec![]),
            (format!(":WAVeform:STOP {stop}"), vec![]),
            (":WAVeform:DATA?".into(), block),
        ]);
    }
    entries
}

// Catches coupling the instrument's DATA window to the transport fragment size,
// and silently returning fewer samples than the requested 10M-point record.
#[test]
fn ds7054_ten_million_points_use_ten_windows_with_bounded_native_reads() {
    let windows = [
        (1, 1_000_000), (1_000_001, 2_000_000), (2_000_001, 3_000_000),
        (3_000_001, 4_000_000), (4_000_001, 5_000_000), (5_000_001, 6_000_000),
        (6_000_001, 7_000_000), (7_000_001, 8_000_000), (8_000_001, 9_000_000),
        (9_000_001, 10_000_000),
    ];
    let (io, evidence) = transport(ds_memory_entries(10_000_000, &windows), usize::MAX);
    let mut options = ScopeOptions::default();
    options.chunk_points = 1_000_000;
    let mut scope = Scope::with_transport(ScopeModel::Ds7054, io, Arc::new(ManualClock::default()), options).unwrap();
    scope.connect().unwrap();
    let frame = scope.capture(&CaptureRequest {
        channels: vec![1], source: CaptureSource::Memory,
        points: Some(10_000_000), byte_budget: 10_000_000,
    }, Deadline::after(Duration::from_secs(120))).unwrap();
    let Samples::U8(samples) = &frame.channels[0].samples else { panic!("wrong sample representation") };
    assert_eq!(samples.len(), 10_000_000);
    assert!(samples.iter().enumerate().all(|(index, &value)| value == (index % 251) as u8));
    let observed = evidence.lock().unwrap();
    assert_eq!(observed.writes.iter().filter(|command| command.as_str() == ":WAVeform:DATA?\n").count(), 10);
    assert!(observed.read_sizes.iter().all(|&size| size <= 16_384));
    assert!(observed.read_sizes.contains(&16_384));
    drop(observed);
    assert!(scope.close().unwrap().resources_released());
}

// Catches enlarging the per-I/O stall timeout when the whole capture budget is longer.
#[test]
fn scope_capture_keeps_individual_io_timeout_below_whole_operation_deadline() {
    let (io, evidence) = transport(ds_memory_entries(3, &[(1, 3)]), usize::MAX);
    let mut options = ScopeOptions::default();
    options.timeout = Duration::from_millis(20);
    let mut scope = Scope::with_transport(ScopeModel::Ds7054, io, Arc::new(ManualClock::default()), options).unwrap();
    scope.connect().unwrap();
    evidence.lock().unwrap().io_deadlines_ms.clear();
    let frame = scope.capture(&CaptureRequest {
        channels: vec![1], source: CaptureSource::Memory, points: Some(3), byte_budget: 3,
    }, deadline()).unwrap();
    assert!(matches!(&frame.channels[0].samples, Samples::U8(values) if values == &[0, 1, 2]));
    let observed = evidence.lock().unwrap();
    assert!(observed.io_deadlines_ms.iter().all(|&millis| millis <= 25), "I/O inherited aggregate capture deadline: {:?}", observed.io_deadlines_ms);
    drop(observed);
    scope.close().unwrap();
}

// Catches reporting the last zero-byte VISA read as if no prior capture data arrived.
#[test]
fn memory_timeout_keeps_completed_window_and_partial_payload_evidence_without_replay() {
    let mut entries = ds_memory_entries(200_000, &[(1, 100_000), (100_001, 200_000)]);
    // Keep the declared length but supply only two 16 KiB payload fragments.
    entries.last_mut().unwrap().1.truncate(8 + 32_768);
    let (io, evidence) = transport(entries, usize::MAX);
    evidence.lock().unwrap().empty_error = Some(DriverError::Timeout { operation: "viRead".into(), transferred: 0 });
    let mut options = ScopeOptions::default();
    options.chunk_points = 100_000;
    let mut scope = Scope::with_transport(ScopeModel::Ds7054, io, Arc::new(ManualClock::default()), options).unwrap();
    scope.connect().unwrap();
    let error = scope.capture(&CaptureRequest {
        channels: vec![1], source: CaptureSource::Memory, points: Some(200_000), byte_budget: 200_000,
    }, deadline()).unwrap_err();
    assert!(matches!(&error, DriverError::Timeout { operation, transferred: 0 }
        if operation.contains("channel=CH1") && operation.contains("window=100001..200000")
            && operation.contains("completed_capture_bytes=100000") && operation.contains("received_bytes=32768")), "{error:?}");
    assert_eq!(evidence.lock().unwrap().writes.iter().filter(|command| command.as_str() == ":WAVeform:DATA?\n").count(), 2);
    assert!(scope.close().unwrap().resources_released());
}

#[test]
fn cancellation_inside_large_waveform_window_stops_before_next_fragment_or_query() {
    let (io, evidence) = transport(ds_memory_entries(200_000, &[(1, 100_000), (100_001, 200_000)]), usize::MAX);
    let mut options = ScopeOptions::default();
    options.chunk_points = 100_000;
    let mut scope = Scope::with_transport(ScopeModel::Ds7054, io, Arc::new(ManualClock::default()), options).unwrap();
    scope.connect().unwrap();
    // IDN/status/depth/preamble + block prefix/length/first payload fragment.
    evidence.lock().unwrap().cancel_after_read = Some((7, scope.stop_handle()));
    assert_eq!(scope.capture(&CaptureRequest {
        channels: vec![1], source: CaptureSource::Memory, points: Some(200_000), byte_budget: 200_000,
    }, deadline()).unwrap_err(), DriverError::Canceled);
    let observed = evidence.lock().unwrap();
    assert_eq!(observed.reads, 7);
    assert_eq!(observed.writes.iter().filter(|command| command.as_str() == ":WAVeform:DATA?\n").count(), 1);
    drop(observed);
    assert!(scope.close().unwrap().resources_released());
}

#[test]
fn large_waveform_window_rejects_early_eom_without_wait_or_replay() {
    let mut entries = ds_memory_entries(100_000, &[(1, 100_000)]);
    entries.last_mut().unwrap().1.truncate(8 + 16_384);
    let (io, evidence) = transport(entries, usize::MAX);
    evidence.lock().unwrap().report_message_end = true;
    let mut options = ScopeOptions::default();
    options.chunk_points = 100_000;
    let mut scope = Scope::with_transport(ScopeModel::Ds7054, io, Arc::new(ManualClock::default()), options).unwrap();
    scope.connect().unwrap();
    let error = scope.capture(&CaptureRequest {
        channels: vec![1], source: CaptureSource::Memory, points: Some(100_000), byte_budget: 100_000,
    }, deadline()).unwrap_err();
    assert!(matches!(error, DriverError::Protocol(message) if message.contains("ended before") && message.contains("received_bytes=16384")));
    assert_eq!(evidence.lock().unwrap().empty_read_attempts, 0);
    assert!(scope.close().unwrap().resources_released());
}

#[test]
fn large_waveform_window_exact_eom_accepts_payload_without_terminator_read() {
    let mut entries = ds_memory_entries(100_000, &[(1, 100_000)]);
    assert_eq!(entries.last_mut().unwrap().1.pop(), Some(b'\n'));
    let (io, evidence) = transport(entries, usize::MAX);
    evidence.lock().unwrap().report_message_end = true;
    let mut options = ScopeOptions::default();
    options.chunk_points = 100_000;
    let mut scope = Scope::with_transport(ScopeModel::Ds7054, io, Arc::new(ManualClock::default()), options).unwrap();
    scope.connect().unwrap();
    let frame = scope.capture(&CaptureRequest {
        channels: vec![1], source: CaptureSource::Memory, points: Some(100_000), byte_budget: 100_000,
    }, deadline()).unwrap();
    assert!(matches!(&frame.channels[0].samples, Samples::U8(samples) if samples.len() == 100_000 && samples[99_999] == 101));
    assert_eq!(evidence.lock().unwrap().empty_read_attempts, 0);
    assert!(scope.close().unwrap().resources_released());
}

#[test]
fn scope_window_admission_keeps_ds_software_ceiling_and_mho_existing_bound() {
    for (model, points) in [(ScopeModel::Ds7054, 1_000_001), (ScopeModel::Mho5054, 8_186)] {
        let (io, evidence) = transport(vec![("*IDN?", vec![])], usize::MAX);
        let mut options = ScopeOptions::default();
        options.chunk_points = points;
        assert!(matches!(Scope::with_transport(model, io, Arc::new(ManualClock::default()), options), Err(DriverError::Invalid(_))));
        assert!(evidence.lock().unwrap().writes.is_empty());
    }
}

// DS7054 retains the final DATA window across channels and capture calls.
// These initial values are the real read-only report's 1816-point residual window.
struct WindowTranscript {
    start: usize,
    stop: usize,
    channel: u8,
    pending: VecDeque<u8>,
    evidence: Arc<Mutex<Evidence>>,
    live: bool,
}
impl ByteTransport for WindowTranscript {
    fn write_all(&mut self, bytes: &[u8], _: Deadline) -> DriverResult<()> {
        assert!(self.live && self.pending.is_empty());
        let command = std::str::from_utf8(bytes).unwrap().trim_end();
        let mut evidence = self.evidence.lock().unwrap();
        assert!(
            evidence.writes.len() < 128,
            "bounded capture command budget"
        );
        evidence.writes.push(format!("{command}\n"));
        drop(evidence);
        let reply = match command {
            "*IDN?" => b"RIGOL,DS7054,WINDOW-FIXTURE,1\n".to_vec(),
            ":TRIGger:STATus?" => b"STOP\n".to_vec(),
            ":ACQuire:MDEPth?" => b"1.0000E+04\n".to_vec(),
            ":CHANnel1:DISPlay?" => b"ON\n".to_vec(),
            ":CHANnel2:DISPlay?" => b"0\n".to_vec(),
            ":CHANnel3:DISPlay?" => b"1\n".to_vec(),
            ":CHANnel4:DISPlay?" => b"OFF\n".to_vec(),
            ":WAVeform:SOURce?" => format!("CHAN{}\n", self.channel).into_bytes(),
            ":WAVeform:MODE?" => b"RAW\n".to_vec(),
            ":WAVeform:FORMat?" => b"BYTE\n".to_vec(),
            ":WAVeform:POINts?" => format!("{}\n", self.stop - self.start + 1).into_bytes(),
            ":WAVeform:STARt?" => format!("{}\n", self.start).into_bytes(),
            ":WAVeform:STOP?" => format!("{}\n", self.stop).into_bytes(),
            ":WAVeform:PREamble?" => format!(
                "0,2,{},1,2.000000E-6,-1.001000E-2,0.000000,2.1871E-02,-71,128\n",
                self.stop - self.start + 1,
            )
            .into_bytes(),
            ":WAVeform:MODE RAW" | ":WAVeform:FORMat BYTE" | ":WAVeform:POINts 10000" => vec![],
            ":WAVeform:DATA?" => {
                let count = self.stop - self.start + 1;
                assert!(count == 8184 || count == 1816, "production chunk bound");
                let mut block = format!("#4{count:04}").into_bytes();
                block.extend(
                    (self.start - 1..self.stop).map(|i| (i % 251) as u8 + self.channel - 1),
                );
                block.push(b'\n');
                block
            }
            _ if command.starts_with(":WAVeform:SOURce CHANnel") => {
                self.channel = command
                    .trim_start_matches(":WAVeform:SOURce CHANnel")
                    .parse()
                    .unwrap();
                assert!(self.channel == 1 || self.channel == 4);
                vec![]
            }
            _ if command.starts_with(":WAVeform:STARt ") => {
                self.start = command
                    .trim_start_matches(":WAVeform:STARt ")
                    .parse()
                    .unwrap();
                assert!((1..=10000).contains(&self.start));
                vec![]
            }
            _ if command.starts_with(":WAVeform:STOP ") => {
                self.stop = command
                    .trim_start_matches(":WAVeform:STOP ")
                    .parse()
                    .unwrap();
                assert!((1..=10000).contains(&self.stop));
                vec![]
            }
            _ => panic!("unreviewed command {command:?}"),
        };
        self.pending.extend(reply);
        Ok(())
    }
    fn read_bounded(&mut self, maximum: usize, _: Deadline) -> DriverResult<Vec<u8>> {
        assert!(maximum <= 16384 && !self.pending.is_empty());
        Ok(self
            .pending
            .drain(..maximum.min(127).min(self.pending.len()))
            .collect())
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        assert!(self.pending.is_empty());
        self.evidence.lock().unwrap().closes += 1;
        self.live = false;
        Ok(CloseReport {
            released: true,
            status: Some(0),
        })
    }
    fn has_responsibility(&self) -> bool {
        self.live
    }
}

#[test]
fn ds7054_resets_residual_window_for_every_channel_and_capture() {
    let evidence = Arc::new(Mutex::new(Evidence::default()));
    let io = Box::new(WindowTranscript {
        start: 8185,
        stop: 10000,
        channel: 1,
        pending: VecDeque::new(),
        evidence: evidence.clone(),
        live: true,
    });
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    let initial = scope.inspect_transfer_state_readonly(deadline()).unwrap();
    assert_eq!(initial["channel_1_display"], "ON");
    assert_eq!(initial["channel_2_display"], "0");
    assert_eq!(initial["channel_3_display"], "1");
    assert_eq!(initial["channel_4_display"], "OFF");
    assert_eq!(initial["waveform_start"], "8185");
    assert_eq!(initial["waveform_stop"], "10000");
    assert_eq!(initial["waveform_points"], "1816");
    assert!(initial["waveform_preamble"]
        .as_str()
        .unwrap()
        .starts_with("0,2,1816,"));
    let request = CaptureRequest {
        channels: vec![1, 4],
        source: CaptureSource::Memory,
        points: Some(10000),
        byte_budget: 20000,
    };
    for _ in 0..2 {
        let frame = scope.capture(&request, deadline()).unwrap();
        assert_eq!(frame.channels.len(), 2);
        for (waveform, channel) in frame.channels.iter().zip([1u8, 4]) {
            assert_eq!(waveform.channel, format!("CH{channel}"));
            let expected = (channel - 1..=channel + 249).cycle().take(10000).collect();
            assert_eq!(waveform.samples, Samples::U8(expected));
            assert_eq!(waveform.calibration.x_increment_s, 2e-6);
            assert_eq!(waveform.calibration.x_origin_s, -1.001e-2);
            assert_eq!(waveform.calibration.x_reference, 0.);
            assert_eq!(waveform.calibration.y_increment_v, 2.1871e-2);
            assert_eq!(waveform.calibration.y_reference, 128.);
            assert!((waveform.calibration.y_origin - 1.552841).abs() < 1e-12);
            assert!(waveform.metadata["preamble"]
                .as_str()
                .unwrap()
                .starts_with("0,2,10000,"));
        }
        let residual = scope.inspect_transfer_state_readonly(deadline()).unwrap();
        assert_eq!(residual["waveform_start"], "8185");
        assert_eq!(residual["waveform_stop"], "10000");
        assert_eq!(residual["waveform_points"], "1816");
    }
    assert!(scope.close().unwrap().resources_released());
    let writes = &evidence.lock().unwrap().writes;
    assert_eq!(
        writes.iter().filter(|q| *q == ":WAVeform:DATA?\n").count(),
        8
    );
    assert_eq!(
        writes
            .windows(4)
            .filter(|q| {
                q[0] == ":WAVeform:STARt 1\n"
                    && q[1] == ":WAVeform:POINts 10000\n"
                    && q[2] == ":WAVeform:STOP 10000\n"
                    && q[3] == ":WAVeform:PREamble?\n"
            })
            .count(),
        4
    );
    assert!(!writes.iter().any(|q| q == ":STOP\n"
        || q.contains(":RUN")
        || q.contains("SINGle")
        || q.starts_with(":ACQuire:MDEPth ")));
}

#[test]
fn preamble_mismatch_retains_bounded_evidence_without_transferring_or_retrying() {
    for (source, preamble, mismatch, observed_points, observed_type) in [
        (
            CaptureSource::Screen,
            "0,0,2,1,1,0,0,1,0,0".to_string(),
            "point count",
            2,
            0,
        ),
        (
            CaptureSource::Memory,
            "0,0,3,1,1,0,0,1,0,0".to_string(),
            "transfer mode",
            3,
            0,
        ),
        (
            CaptureSource::Memory,
            format!("0,2,1000,1,1,0,0,1,0,{}0", " ".repeat(3000)),
            "point count",
            1000,
            2,
        ),
    ] {
        let mut entries = vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
        ];
        if source == CaptureSource::Memory {
            entries.push((":ACQuire:MDEPth?", b"1K\n".to_vec()));
        }
        entries.extend([
            (":WAVeform:SOURce CHANnel4", vec![]),
            (
                if source == CaptureSource::Screen {
                    ":WAVeform:MODE NORMal"
                } else {
                    ":WAVeform:MODE RAW"
                },
                vec![],
            ),
            (":WAVeform:FORMat BYTE", vec![]),
            (":WAVeform:STARt 1", vec![]),
            (":WAVeform:POINts 3", vec![]),
            (":WAVeform:STOP 3", vec![]),
            (":WAVeform:PREamble?", format!("{preamble}\n").into_bytes()),
        ]);
        let expected_calls = entries.len();
        let (io, evidence) = transport(entries, 127);
        let mut scope = Scope::with_transport(
            ScopeModel::Ds7054,
            io,
            Arc::new(ManualClock::default()),
            ScopeOptions::default(),
        )
        .unwrap();
        scope.connect().unwrap();
        let error = scope
            .capture(
                &CaptureRequest {
                    channels: vec![4],
                    source,
                    points: Some(3),
                    byte_budget: 3,
                },
                deadline(),
            )
            .unwrap_err();
        assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Fault);
        assert!(scope.close().unwrap().resources_released());
        let writes = &evidence.lock().unwrap().writes;
        assert_eq!(
            writes.len(),
            expected_calls,
            "a failed request must not be replayed"
        );
        assert!(!writes.iter().any(|q| q.contains(":DATA?")));
        assert_eq!(
            writes
                .iter()
                .filter(|q| *q == ":WAVeform:STARt 1\n")
                .count(),
            1
        );
        assert_eq!(
            writes.iter().filter(|q| *q == ":WAVeform:STOP 3\n").count(),
            1
        );
        let DriverError::Protocol(message) = error else {
            panic!("unexpected mismatch error: {error:?}");
        };
        assert!(
            message.starts_with(&format!("scope preamble {mismatch} mismatch: ")),
            "{message}"
        );
        assert!(
            message.len() < 4096,
            "evidence must fit the Worker error envelope"
        );
        let diagnostic: serde_json::Value =
            serde_json::from_str(message.split_once(": ").unwrap().1).unwrap();
        assert_eq!(diagnostic["model"], "DS7054");
        assert_eq!(diagnostic["channel"], 4);
        assert_eq!(
            diagnostic["source"],
            if source == CaptureSource::Screen {
                "screen"
            } else {
                "memory"
            }
        );
        assert_eq!(diagnostic["requested_points"], 3);
        assert_eq!(diagnostic["observed_points"], observed_points);
        assert_eq!(
            diagnostic["expected_transfer_type"],
            if source == CaptureSource::Screen {
                0
            } else {
                2
            }
        );
        assert_eq!(diagnostic["observed_transfer_type"], observed_type);
        let raw = diagnostic["preamble"].as_str().unwrap();
        assert!(preamble.starts_with(raw));
        assert!(!raw.is_empty() && raw.len() <= 512);
        assert_eq!(diagnostic["preamble_truncated"], raw.len() < preamble.len());
    }
}

#[test]
fn snapshot_freezes_and_transfers_both_model_buffers_without_triggering_or_restarting() {
    for (model, identity, format, preamble, block, expected) in [
        (
            ScopeModel::Ds7054,
            "RIGOL,DS7054,S1,1\n",
            ":WAVeform:FORMat BYTE",
            "0,2,2,1,1,0,0,1,0,0\n",
            b"#12\x01\xff\n".to_vec(),
            255.,
        ),
        (
            ScopeModel::Mho5054,
            "RIGOL,MHO5054,S1,1\n",
            ":WAVeform:FORMat WORD",
            "1,2,2,1,1,0,0,1,0,0\n",
            b"#14\x01\x00\xff\xff\n".to_vec(),
            65535.,
        ),
    ] {
        let mut entries = vec![
            ("*IDN?", identity.as_bytes().to_vec()),
            (":STOP", vec![]),
            (":TRIGger:STATus?", b"WAIT\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"1K\n".to_vec()),
            (":WAVeform:SOURce CHANnel1", vec![]),
            (":WAVeform:MODE RAW", vec![]),
            (format, vec![]),
            (":WAVeform:POINts 2", vec![]),
            (":WAVeform:PREamble?", preamble.as_bytes().to_vec()),
            (":WAVeform:STARt 1", vec![]),
            (":WAVeform:STOP 2", vec![]),
            (":WAVeform:DATA?", block),
        ];
        if model == ScopeModel::Ds7054 {
            entries.insert(9, (":WAVeform:STARt 1", vec![]));
            entries.insert(11, (":WAVeform:STOP 2", vec![]));
        }
        let expected_calls = entries.len();
        let (io, evidence) = transport(entries, 1);
        let mut scope = Scope::with_transport(
            model,
            io,
            Arc::new(ManualClock::default()),
            ScopeOptions::default(),
        )
        .unwrap();
        scope.connect().unwrap();
        let frame = scope
            .capture_snapshot(
                &CaptureRequest {
                    channels: vec![1],
                    source: CaptureSource::Memory,
                    points: Some(2),
                    byte_budget: 4,
                },
                deadline(),
            )
            .unwrap();
        assert_eq!(frame.channels[0].samples.value(1), Some(expected));
        assert_eq!(
            frame.channels[0].metadata["snapshot"]["method"],
            "stop_buffer"
        );
        assert_eq!(
            frame.channels[0].metadata["snapshot"]["fresh_acquisition"],
            false
        );
        scope.close().unwrap();
        let writes = &evidence.lock().unwrap().writes;
        assert_eq!(
            writes.len(),
            expected_calls,
            "no follow-up command may discard the finished frame"
        );
        assert_eq!(writes.iter().filter(|q| *q == ":STOP\n").count(), 1);
        assert!(!writes
            .iter()
            .any(|q| q.contains("SINGle") || q.contains(":RUN")));
    }
}

#[test]
fn snapshot_pure_preflight_rejects_invalid_channels_and_budget_before_stop() {
    let (io, evidence) = transport(vec![("*IDN?", b"RIGOL,MHO5054,S1,1\n".to_vec())], 127);
    let mut scope = Scope::with_transport(
        ScopeModel::Mho5054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    for request in [
        CaptureRequest {
            channels: vec![1, 1],
            source: CaptureSource::Memory,
            points: Some(2),
            byte_budget: 8,
        },
        CaptureRequest {
            channels: vec![1],
            source: CaptureSource::Memory,
            points: Some(2),
            byte_budget: 3,
        },
        CaptureRequest {
            channels: vec![1],
            source: CaptureSource::Screen,
            points: None,
            byte_budget: 1999,
        },
    ] {
        assert!(matches!(
            scope.capture_snapshot(&request, deadline()),
            Err(DriverError::Invalid(_))
        ));
    }
    assert_eq!(evidence.lock().unwrap().writes, ["*IDN?\n"]);
    scope.close().unwrap();
}

#[test]
fn snapshot_screen_transfer_rejects_resumed_acquisition_before_waveform_io() {
    let (io, evidence) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":STOP", vec![]),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
        ],
        127,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    assert!(matches!(
        scope.capture_snapshot(
            &CaptureRequest {
                channels: vec![1],
                source: CaptureSource::Screen,
                points: Some(2),
                byte_budget: 2,
            },
            deadline()
        ),
        Err(DriverError::Responsibility(_))
    ));
    scope.close().unwrap();
    assert_eq!(
        evidence.lock().unwrap().writes,
        [
            "*IDN?\n",
            ":STOP\n",
            ":TRIGger:STATus?\n",
            ":TRIGger:STATus?\n"
        ]
    );
}

fn existing_screen_entries(block: Vec<u8>) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
        (":TRIGger:STATus?", b"WAIT\n".to_vec()),
        (":WAVeform:SOURce CHANnel1", vec![]),
        (":WAVeform:MODE NORMal", vec![]),
        (":WAVeform:FORMat BYTE", vec![]),
        (":WAVeform:STARt 1", vec![]),
        (":WAVeform:POINts 3", vec![]),
        (":WAVeform:STOP 3", vec![]),
        (
            ":WAVeform:PREamble?",
            b"0,0,3,1,1e-6,0,0,0.1,0,128\n".to_vec(),
        ),
        (":WAVeform:STARt 1", vec![]),
        (":WAVeform:STOP 3", vec![]),
        (":WAVeform:DATA?", block),
    ]
}

// Real USB DS7054 ends its definite block with VISA EOM, without an LF byte.
// Completion is authoritative only after the exact declared payload arrives.
#[test]
fn waveform_transport_message_end_accepts_exact_payload_without_extra_read() {
    for fragment in [1, 2, 127] {
        for suffix in [b"".as_slice(), b"\n", b"\r\n"] {
            let mut block = b"#9000000003\x00\x80\xff".to_vec();
            block.extend(suffix);
            let (io, e) = transport(existing_screen_entries(block), fragment);
            e.lock().unwrap().report_message_end = true;
            let mut scope = Scope::with_transport(
                ScopeModel::Ds7054,
                io,
                Arc::new(ManualClock::default()),
                ScopeOptions::default(),
            )
            .unwrap();
            scope.connect().unwrap();
            let frame = scope
                .capture(
                    &CaptureRequest {
                        channels: vec![1],
                        source: CaptureSource::Screen,
                        points: Some(3),
                        byte_budget: 3,
                    },
                    deadline(),
                )
                .unwrap();
            assert_eq!(frame.channels[0].samples, Samples::U8(vec![0, 128, 255]));
            assert_eq!(
                e.lock().unwrap().empty_read_attempts,
                0,
                "read past a confirmed message boundary"
            );
            assert_eq!(
                e.lock()
                    .unwrap()
                    .writes
                    .iter()
                    .filter(|c| c.as_str() == ":WAVeform:DATA?\n")
                    .count(),
                1
            );
            assert!(scope.close().unwrap().resources_released());
        }
    }
}

#[test]
fn premature_waveform_message_end_rejects_without_waiting_or_replaying() {
    for block in [b"#1".as_slice(), b"#13", b"#13ab", b"#13abc\r"] {
        let (io, e) = transport(existing_screen_entries(block.to_vec()), 127);
        e.lock().unwrap().report_message_end = true;
        let mut scope = Scope::with_transport(
            ScopeModel::Ds7054,
            io,
            Arc::new(ManualClock::default()),
            ScopeOptions::default(),
        )
        .unwrap();
        scope.connect().unwrap();
        let error = scope
            .capture(
                &CaptureRequest {
                    channels: vec![1],
                    source: CaptureSource::Screen,
                    points: Some(3),
                    byte_budget: 3,
                },
                deadline(),
            )
            .unwrap_err();
        assert!(matches!(error, DriverError::Protocol(_)), "{error:?}");
        assert_eq!(
            e.lock().unwrap().empty_read_attempts,
            0,
            "waited beyond early EOM"
        );
        assert_eq!(
            e.lock()
                .unwrap()
                .writes
                .iter()
                .filter(|c| c.as_str() == ":WAVeform:DATA?\n")
                .count(),
            1
        );
        assert!(scope.close().unwrap().resources_released());
    }
}

#[test]
fn waveform_read_timeout_identifies_text_and_binary_stage_without_retry() {
    for (preamble, block, stage) in [
        (vec![], None, ":WAVeform:PREamble? text reply"),
        (
            b"0,0,3,1,1e-6,0,0,0.1,0,128\n".to_vec(),
            Some(vec![]),
            ":WAVeform:DATA? block prefix",
        ),
        (
            b"0,0,3,1,1e-6,0,0,0.1,0,128\n".to_vec(),
            Some(b"#1".to_vec()),
            ":WAVeform:DATA? block length",
        ),
        (
            b"0,0,3,1,1e-6,0,0,0.1,0,128\n".to_vec(),
            Some(b"#13".to_vec()),
            ":WAVeform:DATA? block payload",
        ),
        (
            b"0,0,3,1,1e-6,0,0,0.1,0,128\n".to_vec(),
            Some(b"#13abc".to_vec()),
            ":WAVeform:DATA? block terminator",
        ),
    ] {
        let mut entries = vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"WAIT\n".to_vec()),
            (":WAVeform:SOURce CHANnel1", vec![]),
            (":WAVeform:MODE NORMal", vec![]),
            (":WAVeform:FORMat BYTE", vec![]),
            (":WAVeform:STARt 1", vec![]),
            (":WAVeform:POINts 3", vec![]),
            (":WAVeform:STOP 3", vec![]),
            (":WAVeform:PREamble?", preamble),
        ];
        if let Some(block) = block {
            entries.extend([
                (":WAVeform:STARt 1", vec![]),
                (":WAVeform:STOP 3", vec![]),
                (":WAVeform:DATA?", block),
            ]);
        }
        let (io, e) = transport(entries, 64);
        e.lock().unwrap().empty_error = Some(DriverError::Timeout {
            operation: "viRead".into(),
            transferred: 0,
        });
        let mut scope = Scope::with_transport(
            ScopeModel::Ds7054,
            io,
            Arc::new(ManualClock::default()),
            ScopeOptions::default(),
        )
        .unwrap();
        scope.connect().unwrap();
        let error = scope
            .capture(
                &CaptureRequest {
                    channels: vec![1],
                    source: CaptureSource::Screen,
                    points: Some(3),
                    byte_budget: 3,
                },
                deadline(),
            )
            .unwrap_err();
        assert!(
            matches!(&error,DriverError::Timeout{operation,transferred:0} if operation.contains(stage)),
            "{stage}: {error:?}"
        );
        assert_eq!(
            e.lock()
                .unwrap()
                .writes
                .iter()
                .filter(|c| c.as_str() == ":WAVeform:DATA?\n")
                .count(),
            usize::from(!stage.contains("PREamble"))
        );
        assert!(scope.close().unwrap().resources_released());
    }
}

// Catches signed reinterpretation, incorrect origin normalization and partial block acceptance.
#[test]
fn raw_unsigned_codes_and_normalized_axes() {
    let p = parse_preamble(
        "0,0,3,1,0.000002,-0.000004,1,0.02,5,128",
        ScopeModel::Ds7054,
    )
    .unwrap();
    assert_eq!(p.calibration.time_s(2), -0.000002);
    assert!((p.calibration.voltage_v(255.0) - 2.44).abs() < 1e-12);
    assert!(
        matches!(decode_block(b"#13\x00\x80\xff\n",ScopeModel::Ds7054,3).unwrap(),Samples::U8(v) if v==[0,128,255])
    );
    assert!(
        matches!(decode_block(b"#14\x00\x80\xff\xff\r\n",ScopeModel::Mho5054,2).unwrap(),Samples::U16(v) if v==[32768,65535])
    );
    for malformed in [
        b"#13\x01\x02".as_slice(),
        b"#03abc",
        b"#14abcdx",
        b"#13abcd\n",
    ] {
        assert!(decode_block(malformed, ScopeModel::Ds7054, 3).is_err());
    }
    assert!(parse_preamble("0,0,3,1,NaN,0,0,1,0,0", ScopeModel::Ds7054).is_err());
    assert!(parse_preamble("0,0,3,1,1,0,0,1,0,0", ScopeModel::Mho5054).is_err());
}

// Catches constructor/close side effects, zero-based ranges and unbounded/incorrect WORD reads.
#[test]
fn fragmented_scope_capture_uses_model_dtype_and_preserves_run_state() {
    for (model, id, format, preamble, block) in [
        (
            ScopeModel::Ds7054,
            "RIGOL TECHNOLOGIES,DS7054,S1,1\n",
            "BYTE",
            "0,0,3,1,0.000001,0,0,0.1,2,128\n",
            b"#13\x00\x80\xff\n".to_vec(),
        ),
        (
            ScopeModel::Mho5054,
            "RIGOL TECHNOLOGIES,MHO5054,S2,1\n",
            "WORD",
            "1,0,3,1,0.000001,0,0,0.001,4,32768\n",
            b"#16\x00\x00\x00\x80\xff\xff\n".to_vec(),
        ),
    ] {
        let format_command = format!(":WAVeform:FORMat {format}");
        let mut entries = vec![
            ("*IDN?", id.as_bytes().to_vec()),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
            (":WAVeform:SOURce CHANnel1", vec![]),
            (":WAVeform:MODE NORMal", vec![]),
            (&format_command, vec![]),
            (":WAVeform:POINts 3", vec![]),
            (":WAVeform:PREamble?", preamble.as_bytes().to_vec()),
            (":WAVeform:STARt 1", vec![]),
            (":WAVeform:STOP 3", vec![]),
            (":WAVeform:DATA?", block),
        ];
        if model == ScopeModel::Ds7054 {
            entries.insert(5, (":WAVeform:STARt 1", vec![]));
            entries.insert(7, (":WAVeform:STOP 3", vec![]));
        }
        let (io, e) = transport(entries, 1);
        let mut scope = Scope::with_transport(
            model,
            io,
            Arc::new(ManualClock::default()),
            ScopeOptions::default(),
        )
        .unwrap();
        assert!(e.lock().unwrap().writes.is_empty());
        scope.connect().unwrap();
        let frame = scope
            .capture(
                &CaptureRequest {
                    channels: vec![1],
                    source: CaptureSource::Screen,
                    points: Some(3),
                    byte_budget: 6,
                },
                deadline(),
            )
            .unwrap();
        assert_eq!(frame.channels[0].samples.len(), 3);
        assert_eq!(
            frame.channels[0].samples.value(2),
            Some(if model == ScopeModel::Ds7054 {
                255.0
            } else {
                65535.0
            })
        );
        assert!(scope.close().unwrap().resources_released());
        assert_eq!(e.lock().unwrap().closes, 1);
    }
}

// Catches admitting large requests after writing/allocating and implicit STOP during memory reads.
#[test]
fn scope_budget_and_memory_run_state_fail_before_configuration() {
    let (io, e) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"RUN\n".to_vec()),
        ],
        8,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    assert!(scope
        .capture(
            &CaptureRequest {
                channels: vec![1, 2],
                source: CaptureSource::Memory,
                points: Some(250_000_000),
                byte_budget: 1024
            },
            deadline()
        )
        .is_err());
    assert_eq!(e.lock().unwrap().writes.len(), 1);
    assert!(scope
        .capture(
            &CaptureRequest {
                channels: vec![1],
                source: CaptureSource::Memory,
                points: Some(3),
                byte_budget: 3
            },
            deadline()
        )
        .is_err());
    assert_eq!(e.lock().unwrap().writes.len(), 2);
    scope.close().unwrap();
}

fn independent_dg4000(channel: u8) -> Vec<(&'static str, Vec<u8>)> {
    let commands = if channel == 1 {
        [
            ":SOURce1:MOD:STATe?",
            ":SOURce1:SWEep:STATe?",
            ":SOURce1:BURSt:STATe?",
            ":COUPling:AMPL:STATe?",
            ":COUPling:FREQuency:STATe?",
            ":COUPling:PHASe:STATe?",
        ]
    } else {
        [
            ":SOURce2:MOD:STATe?",
            ":SOURce2:SWEep:STATe?",
            ":SOURce2:BURSt:STATe?",
            ":COUPling:AMPL:STATe?",
            ":COUPling:FREQuency:STATe?",
            ":COUPling:PHASe:STATe?",
        ]
    };
    commands
        .into_iter()
        .map(|c| (c, b"OFF\n".to_vec()))
        .collect()
}

#[test]
fn dg4202_rejects_modulated_or_coupled_configuration_before_any_setting_write() {
    for enabled in 0..6 {
        let mut script = vec![
            ("*IDN?", b"RIGOL,DG4202,S1,1\n".to_vec()),
            (":OUTPut2:LOAD?", b"INF\n".to_vec()),
            (":SOURce2:FUNCtion?", b"SIN\n".to_vec()),
            (":SOURce2:FREQuency?", b"50\n".to_vec()),
            (":SOURce2:VOLTage:UNIT?", b"VPP\n".to_vec()),
            (":SOURce2:VOLTage?", b"2\n".to_vec()),
            (":SOURce2:VOLTage:OFFSet?", b"0\n".to_vec()),
        ];
        let mut guards = independent_dg4000(2);
        guards[enabled].1 = b"ON\n".to_vec();
        script.extend(guards);
        let (io, e) = transport(script, 16);
        let mut d = Generator::with_transport(
            GeneratorModel::Dg4202,
            io,
            Arc::new(ManualClock::default()),
            GeneratorOptions::default(),
        )
        .unwrap();
        d.connect().unwrap();
        let error = d
            .configure_signal(
                2,
                &SignalConfig {
                    waveform: Waveform::Triangle,
                    frequency_hz: 50.,
                    amplitude_vpp: 2.,
                    offset_v: 0.,
                },
                deadline(),
            )
            .unwrap_err();
        assert!(matches!(error, DriverError::Invalid(_)), "{error:?}");
        assert!(e
            .lock()
            .unwrap()
            .writes
            .iter()
            .all(|command| command.trim_end().ends_with('?')));
        assert!(d.close().unwrap().resources_released());
    }
}
#[test]
fn dg4202_load_change_preserves_physical_output_and_rejects_fractional_ohms() {
    use yang_drivers::generator::OutputLoad;
    let script = vec![
        ("*IDN?", b"RIGOL,DG4202,S1,1\n".to_vec()),
        (":OUTPut1:LOAD?", b"INF\n".to_vec()),
        (":SOURce1:FUNCtion?", b"SIN\n".to_vec()),
        (":SOURce1:FREQuency?", b"50\n".to_vec()),
        (":SOURce1:VOLTage:UNIT?", b"VPP\n".to_vec()),
        (":SOURce1:VOLTage?", b"20\n".to_vec()),
        (":SOURce1:VOLTage:OFFSet?", b"0\n".to_vec()),
        (":SOURce1:PHASe?", b"0\n".to_vec()),
        (":OUTPut1:POLarity?", b"NORM\n".to_vec()),
        (":OUTPut1?", b"ON\n".to_vec()),
        (":OUTPut1:LOAD 50", vec![]),
    ];
    let (io, e) = transport(script, 16);
    let mut d = Generator::with_transport(
        GeneratorModel::Dg4202,
        io,
        Arc::new(ManualClock::default()),
        GeneratorOptions::default(),
    )
    .unwrap();
    d.connect().unwrap();
    assert!(d
        .set_load(1, OutputLoad::Ohms { ohms: 75.5 }, deadline())
        .is_err());
    assert_eq!(e.lock().unwrap().writes.len(), 1);
    d.set_load(1, OutputLoad::Ohms { ohms: 50. }, deadline())
        .unwrap();
    assert_eq!(
        e.lock()
            .unwrap()
            .writes
            .iter()
            .filter(|c| !c.trim_end().ends_with('?'))
            .cloned()
            .collect::<Vec<_>>(),
        vec![":OUTPut1:LOAD 50\n"]
    );
    assert!(d.close().unwrap().resources_released());
}

// Catches a bare RAMP mapping and implicit output changes on connect/configure/normal close.
#[test]
fn generator_triangle_mapping_and_explicit_output_dialects() {
    for (model, id, output) in [
        (GeneratorModel::Dg4162, "DG4162", ":OUTPut1"),
        (GeneratorModel::Dg4202, "DG4202", ":OUTPut1"),
        (GeneratorModel::Dg922Pro, "DG922 Pro", ":OUTPut1:STATe"),
    ] {
        let identity = format!("RIGOL TECHNOLOGIES,{id},S1,1\n");
        let output_set = format!("{output} ON");
        let output_query = format!("{output}?");
        let mut script = vec![
            ("*IDN?", identity.into_bytes()),
            (":OUTPut1:LOAD?", b"INF\n".to_vec()),
            (":SOURce1:FUNCtion?", b"SIN\n".to_vec()),
            (":SOURce1:FREQuency?", b"50\n".to_vec()),
            (":SOURce1:VOLTage:UNIT?", b"VPP\n".to_vec()),
            (":SOURce1:VOLTage?", b"2\n".to_vec()),
            (":SOURce1:VOLTage:OFFSet?", b"0\n".to_vec()),
            (":SOURce1:VOLTage 2", vec![]),
            (":SOURce1:VOLTage:OFFSet 0", vec![]),
            (":SOURce1:FUNCtion RAMP", vec![]),
            (":SOURce1:FUNCtion:RAMP:SYMMetry 50", vec![]),
            (":SOURce1:FREQuency 50", vec![]),
            (&output_set, vec![]),
            (&output_query, b"ON\n".to_vec()),
        ];
        if model != GeneratorModel::Dg922Pro {
            script.splice(7..7, independent_dg4000(1));
        }
        let (io, e) = transport(script, 3);
        let mut generator = Generator::with_transport(
            model,
            io,
            Arc::new(ManualClock::default()),
            GeneratorOptions::default(),
        )
        .unwrap();
        assert!(e.lock().unwrap().writes.is_empty());
        generator.connect().unwrap();
        generator
            .configure_signal(
                1,
                &SignalConfig {
                    waveform: Waveform::Triangle,
                    frequency_hz: 50.0,
                    amplitude_vpp: 2.0,
                    offset_v: 0.0,
                },
                deadline(),
            )
            .unwrap();
        generator.set_output(1, true, deadline()).unwrap();
        assert!(generator.output_enabled(1, deadline()).unwrap());
        assert!(generator.close().unwrap().resources_released());
    }
}

// Catches retaining the previous triangular symmetry when selecting a sawtooth ramp.
#[test]
fn dg4202_ramp_after_triangle_selects_rising_sawtooth_and_reads_back_as_ramp() {
    let mut script = vec![
        ("*IDN?", b"RIGOL,DG4202,S1,1\n".to_vec()),
        (":OUTPut1:LOAD?", b"INF\n".to_vec()),
        (":SOURce1:FUNCtion?", b"RAMP\n".to_vec()),
        (":SOURce1:FUNCtion:RAMP:SYMMetry?", b"50\n".to_vec()),
        (":SOURce1:FREQuency?", b"50\n".to_vec()),
        (":SOURce1:VOLTage:UNIT?", b"VPP\n".to_vec()),
        (":SOURce1:VOLTage?", b"2\n".to_vec()),
        (":SOURce1:VOLTage:OFFSet?", b"0\n".to_vec()),
    ];
    script.extend(independent_dg4000(1));
    script.extend([
        (":SOURce1:VOLTage 2", vec![]),
        (":SOURce1:VOLTage:OFFSet 0", vec![]),
        (":SOURce1:FUNCtion RAMP", vec![]),
        (":SOURce1:FUNCtion:RAMP:SYMMetry 100", vec![]),
        (":SOURce1:FREQuency 50", vec![]),
        (":OUTPut1:LOAD?", b"INF\n".to_vec()),
        (":SOURce1:FUNCtion?", b"RAMP\n".to_vec()),
        (":SOURce1:FUNCtion:RAMP:SYMMetry?", b"100\n".to_vec()),
        (":SOURce1:FREQuency?", b"50\n".to_vec()),
        (":SOURce1:VOLTage:UNIT?", b"VPP\n".to_vec()),
        (":SOURce1:VOLTage?", b"2\n".to_vec()),
        (":SOURce1:VOLTage:OFFSet?", b"0\n".to_vec()),
        (":SOURce1:PHASe?", b"0\n".to_vec()),
        (":OUTPut1:POLarity?", b"NORM\n".to_vec()),
        (":OUTPut1?", b"ON\n".to_vec()),
    ]);
    let (io, evidence) = transport(script, 3);
    let mut generator = Generator::with_transport(
        GeneratorModel::Dg4202,
        io,
        Arc::new(ManualClock::default()),
        GeneratorOptions::default(),
    )
    .unwrap();
    generator.connect().unwrap();
    generator
        .configure_signal(
            1,
            &SignalConfig {
                waveform: Waveform::Ramp,
                frequency_hz: 50.,
                amplitude_vpp: 2.,
                offset_v: 0.,
            },
            deadline(),
        )
        .unwrap();
    let readback = generator.read_signal(1, deadline()).unwrap();
    assert_eq!(readback.waveform, Waveform::Ramp);
    assert!(readback.output_enabled);
    assert!(generator.close().unwrap().resources_released());
    assert!(evidence
        .lock()
        .unwrap()
        .writes
        .iter()
        .filter(|command| command.starts_with(":OUTPut"))
        .all(|command| command.trim_end().ends_with('?')));
}

// Catches combined voltage and waveform frequency limits being checked after writes.
#[test]
fn generator_rejects_unsafe_combined_levels_and_waveform_frequency() {
    let (io, e) = transport(
        vec![
            ("*IDN?", b"RIGOL,DG4162,S1,1\n".to_vec()),
            (":OUTPut1:LOAD?", b"50\n".to_vec()),
        ],
        16,
    );
    let mut generator = Generator::with_transport(
        GeneratorModel::Dg4162,
        io,
        Arc::new(ManualClock::default()),
        GeneratorOptions::default(),
    )
    .unwrap();
    generator.connect().unwrap();
    assert!(generator
        .configure_signal(
            1,
            &SignalConfig {
                waveform: Waveform::Triangle,
                frequency_hz: 5e6,
                amplitude_vpp: 2.0,
                offset_v: 0.0
            },
            deadline()
        )
        .is_err());
    assert_eq!(e.lock().unwrap().writes.len(), 1);
    assert!(generator
        .configure_signal(
            1,
            &SignalConfig {
                waveform: Waveform::Sine,
                frequency_hz: 50.0,
                amplitude_vpp: 8.0,
                offset_v: -2.0
            },
            deadline()
        )
        .is_err());
    assert_eq!(e.lock().unwrap().writes.len(), 2);
    generator.close().unwrap();
}

// Catches trusting optional transfer length instead of the exact request or returning partial frames.
#[test]
fn scope_rejects_truncated_and_wrong_declared_transfers() {
    for block in [b"#13\x80\x81".to_vec(), b"#1999999999".to_vec()] {
        let (io, _) = transport(
            vec![
                ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
                (":TRIGger:STATus?", b"STOP\n".to_vec()),
                (":WAVeform:SOURce CHANnel1", vec![]),
                (":WAVeform:MODE NORMal", vec![]),
                (":WAVeform:FORMat BYTE", vec![]),
                (":WAVeform:STARt 1", vec![]),
                (":WAVeform:POINts 3", vec![]),
                (":WAVeform:STOP 3", vec![]),
                (":WAVeform:PREamble?", b"0,0,3,1,1,0,0,1,0,0\n".to_vec()),
                (":WAVeform:STARt 1", vec![]),
                (":WAVeform:STOP 3", vec![]),
                (":WAVeform:DATA?", block),
            ],
            2,
        );
        let mut scope = Scope::with_transport(
            ScopeModel::Ds7054,
            io,
            Arc::new(ManualClock::default()),
            ScopeOptions::default(),
        )
        .unwrap();
        scope.connect().unwrap();
        assert!(scope
            .capture(
                &CaptureRequest {
                    channels: vec![1],
                    source: CaptureSource::Screen,
                    points: Some(3),
                    byte_budget: 3
                },
                deadline()
            )
            .is_err());
        assert_eq!(scope.state(), yang_drivers::lifecycle::DriverState::Fault);
        assert!(scope.close().unwrap().resources_released());
    }
}

// Catches ignoring the final partial chunk or silently capping deep memory to screen length.
#[test]
fn stopped_memory_capture_reads_exact_one_based_chunks() {
    let (io, e) = transport(
        vec![
            ("*IDN?", b"RIGOL,MHO5054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":ACQuire:MDEPth?", b"1.000E+3\n".to_vec()),
            (":WAVeform:SOURce CHANnel2", vec![]),
            (":WAVeform:MODE RAW", vec![]),
            (":WAVeform:FORMat WORD", vec![]),
            (":WAVeform:POINts 5", vec![]),
            (":WAVeform:PREamble?", b"1,2,5,1,1,0,0,1,0,0\n".to_vec()),
            (":WAVeform:STARt 1", vec![]),
            (":WAVeform:STOP 2", vec![]),
            (":WAVeform:DATA?", b"#14\x01\x00\x00\x80\n".to_vec()),
            (":WAVeform:STARt 3", vec![]),
            (":WAVeform:STOP 4", vec![]),
            (":WAVeform:DATA?", b"#14\x03\x00\x04\x00\n".to_vec()),
            (":WAVeform:STARt 5", vec![]),
            (":WAVeform:STOP 5", vec![]),
            (":WAVeform:DATA?", b"#12\xff\xff\n".to_vec()),
        ],
        1,
    );
    let mut options = ScopeOptions::default();
    options.chunk_points = 2;
    let mut scope = Scope::with_transport(
        ScopeModel::Mho5054,
        io,
        Arc::new(ManualClock::default()),
        options,
    )
    .unwrap();
    scope.connect().unwrap();
    let frame = scope
        .capture(
            &CaptureRequest {
                channels: vec![2],
                source: CaptureSource::Memory,
                points: Some(5),
                byte_budget: 10,
            },
            deadline(),
        )
        .unwrap();
    assert!(matches!(&frame.channels[0].samples,Samples::U16(v) if v==&[1,32768,3,4,65535]));
    assert_eq!(frame.channels[0].channel, "CH2");
    scope.close().unwrap();
    assert_eq!(e.lock().unwrap().writes.len(), 17);
}

// Catches absent per-model frequency derating, even when combined levels fit low-frequency limits.
#[test]
fn dg4000_derating_rejects_eight_vpp_at_thirty_mhz_into_fifty_ohms() {
    let (io, e) = transport(
        vec![
            ("*IDN?", b"RIGOL,DG4202,S1,1\n".to_vec()),
            (":OUTPut1:LOAD?", b"50\n".to_vec()),
        ],
        32,
    );
    let mut generator = Generator::with_transport(
        GeneratorModel::Dg4202,
        io,
        Arc::new(ManualClock::default()),
        GeneratorOptions::default(),
    )
    .unwrap();
    generator.connect().unwrap();
    assert!(generator
        .configure_signal(
            1,
            &SignalConfig {
                waveform: Waveform::Sine,
                frequency_hz: 30e6,
                amplitude_vpp: 8.0,
                offset_v: 0.0
            },
            deadline()
        )
        .is_err());
    assert_eq!(e.lock().unwrap().writes.len(), 2);
    generator.close().unwrap();
}

// Catches automatic restart or continuing reads after a cancellation boundary.
#[test]
fn cancellation_between_chunk_reads_returns_no_frame_and_holds_scope() {
    let (io, e) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"STOP\n".to_vec()),
            (":WAVeform:SOURce CHANnel1", vec![]),
            (":WAVeform:MODE NORMal", vec![]),
            (":WAVeform:FORMat BYTE", vec![]),
            (":WAVeform:STARt 1", vec![]),
            (":WAVeform:POINts 3", vec![]),
            (":WAVeform:STOP 3", vec![]),
            (":WAVeform:PREamble?", b"0,0,3,1,1,0,0,1,0,0\n".to_vec()),
            (":WAVeform:STARt 1", vec![]),
            (":WAVeform:STOP 2", vec![]),
            (":WAVeform:DATA?", b"#12\x80\x81\n".to_vec()),
        ],
        1024,
    );
    let mut options = ScopeOptions::default();
    options.chunk_points = 2;
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        options,
    )
    .unwrap();
    scope.connect().unwrap();
    // IDN/status/preamble each one read; block prefix/length/body/terminator four reads.
    e.lock().unwrap().cancel_after_read = Some((7, scope.stop_handle()));
    assert_eq!(
        scope
            .capture(
                &CaptureRequest {
                    channels: vec![1],
                    source: CaptureSource::Screen,
                    points: Some(3),
                    byte_budget: 3
                },
                deadline()
            )
            .unwrap_err(),
        DriverError::Canceled
    );
    assert_eq!(e.lock().unwrap().writes.len(), 12);
    assert!(scope.close().unwrap().resources_released());
}

// Catches claiming release from a successful lifecycle call rather than native close evidence.
#[test]
fn cleanup_receipts_keep_failed_attempt_immutable_and_retry_owner() {
    let (io, e) = transport(vec![("*IDN?", b"RIGOL,DG4162,S1,1\n".to_vec())], 1024);
    let mut generator = Generator::with_transport(
        GeneratorModel::Dg4162,
        io,
        Arc::new(ManualClock::default()),
        GeneratorOptions::default(),
    )
    .unwrap();
    generator.connect().unwrap();
    e.lock().unwrap().fail_closes = 1;
    let first = generator.close().unwrap();
    assert!(!first.resources_released());
    assert!(generator.has_resource_responsibility());
    assert!(first.voltage_zero().is_none());
    let second = generator.close().unwrap();
    assert!(second.resources_released());
    assert!(!generator.has_resource_responsibility());
    assert!(!first.resources_released());
    assert_ne!(first.attempt_id(), second.attempt_id());
    assert_eq!(e.lock().unwrap().writes.len(), 1);
}

// Catches identity guessed from the resource or an unrelated Rigol model.
#[test]
fn identity_mismatch_fences_every_control_until_release() {
    let (io, e) = transport(vec![("*IDN?", b"RIGOL,DG4202,S1,1\n".to_vec())], 1024);
    let mut generator = Generator::with_transport(
        GeneratorModel::Dg4162,
        io,
        Arc::new(ManualClock::default()),
        GeneratorOptions::default(),
    )
    .unwrap();
    assert!(generator.connect().is_err());
    assert!(generator.set_output(1, true, deadline()).is_err());
    assert_eq!(e.lock().unwrap().writes.len(), 1);
    assert!(generator.close().unwrap().resources_released());
}

// Catches applying a larger amplitude before lowering the old frequency's restrictive range.
#[test]
fn generator_lowers_frequency_before_raising_voltage_and_normalizes_explicit_config_unit() {
    let mut script = vec![
        ("*IDN?", b"RIGOL,DG4202,S1,1\n".to_vec()),
        (":OUTPut1:LOAD?", b"INF\n".to_vec()),
        (":SOURce1:FUNCtion?", b"SIN\n".to_vec()),
        (":SOURce1:FREQuency?", b"100000000\n".to_vec()),
        (":SOURce1:VOLTage:UNIT?", b"VRMS\n".to_vec()),
        (":SOURce1:VOLTage?", b"0.7071067811865475\n".to_vec()),
        (":SOURce1:VOLTage:OFFSet?", b"0\n".to_vec()),
        (":SOURce1:FREQuency 50", vec![]),
        (":SOURce1:VOLTage:UNIT VPP", vec![]),
        (":SOURce1:VOLTage 18", vec![]),
        (":SOURce1:VOLTage:OFFSet 0", vec![]),
        (":SOURce1:FUNCtion RAMP", vec![]),
        (":SOURce1:FUNCtion:RAMP:SYMMetry 50", vec![]),
    ];
    script.splice(7..7, independent_dg4000(1));
    let (io, e) = transport(script, 1024);
    let mut generator = Generator::with_transport(
        GeneratorModel::Dg4202,
        io,
        Arc::new(ManualClock::default()),
        GeneratorOptions::default(),
    )
    .unwrap();
    generator.connect().unwrap();
    generator
        .configure_signal(
            1,
            &SignalConfig {
                waveform: Waveform::Triangle,
                frequency_hz: 50.0,
                amplitude_vpp: 18.0,
                offset_v: 0.0,
            },
            deadline(),
        )
        .unwrap();
    assert_eq!(e.lock().unwrap().writes.len(), 19);
    generator.close().unwrap();
}

// Catches treating unit-bearing DS7000 memory depth readback as invalid metadata.
#[test]
fn scope_read_status_parses_model_memory_units_without_writes() {
    let (io, e) = transport(
        vec![
            ("*IDN?", b"RIGOL,DS7054,S1,1\n".to_vec()),
            (":TRIGger:STATus?", b"WAIT\n".to_vec()),
            (":ACQuire:MDEPth?", b"125M\n".to_vec()),
            (":CHANnel1:DISPlay?", b"ON\n".to_vec()),
            (":CHANnel2:DISPlay?", b"0\n".to_vec()),
            (":CHANnel3:DISPlay?", b"1\n".to_vec()),
            (":CHANnel4:DISPlay?", b"OFF\n".to_vec()),
        ],
        2,
    );
    let mut scope = Scope::with_transport(
        ScopeModel::Ds7054,
        io,
        Arc::new(ManualClock::default()),
        ScopeOptions::default(),
    )
    .unwrap();
    scope.connect().unwrap();
    let status = scope.read_status(deadline()).unwrap();
    assert_eq!(status.memory_depth, 125_000_000);
    assert!(status.running);
    assert_eq!(
        status
            .channels
            .iter()
            .map(|c| c.displayed)
            .collect::<Vec<_>>(),
        [true, false, true, false]
    );
    assert!(status.capabilities.memory_depths.contains(&125_000_000));
    scope.close().unwrap();
    assert!(e
        .lock()
        .unwrap()
        .writes
        .iter()
        .all(|s| s.trim_end().ends_with('?')));
}

// Catches preventing a declared run cleanup from disabling its output after cancellation.
#[test]
fn explicitly_disabling_generator_output_remains_possible_after_stop_request() {
    let (io, e) = transport(
        vec![
            ("*IDN?", b"RIGOL,DG922 Pro,S1,1\n".to_vec()),
            (":OUTPut2:STATe OFF", vec![]),
        ],
        1024,
    );
    let mut generator = Generator::with_transport(
        GeneratorModel::Dg922Pro,
        io,
        Arc::new(ManualClock::default()),
        GeneratorOptions::default(),
    )
    .unwrap();
    generator.connect().unwrap();
    generator.stop_handle().request_stop();
    assert_eq!(
        generator.set_output(2, true, deadline()).unwrap_err(),
        DriverError::Canceled
    );
    generator.set_output(2, false, deadline()).unwrap();
    assert_eq!(e.lock().unwrap().writes.len(), 2);
    generator.close().unwrap();
}

// Catches loss of actual symmetry/load/polarity and hidden writes during generator status refresh.
#[test]
fn generator_status_reports_both_channels_and_optional_controls_are_explicit() {
    let mut entries = vec![("*IDN?", b"RIGOL,DG922 Pro,S1,1\n".to_vec())];
    entries.extend(vec![
        (":OUTPut1:LOAD?", b"9.9E+37\n".to_vec()),
        (":SOURce1:FUNCtion?", b"RAMP\n".to_vec()),
        (":SOURce1:FUNCtion:RAMP:SYMMetry?", b"50\n".to_vec()),
        (":SOURce1:FREQuency?", b"50\n".to_vec()),
        (":SOURce1:VOLTage:UNIT?", b"VPP\n".to_vec()),
        (":SOURce1:VOLTage?", b"2\n".to_vec()),
        (":SOURce1:VOLTage:OFFSet?", b"0.1\n".to_vec()),
        (":SOURce1:PHASe?", b"45\n".to_vec()),
        (":OUTPut1:POLarity?", b"NORM\n".to_vec()),
        (":OUTPut1:STATe?", b"ON\n".to_vec()),
        (":OUTPut2:LOAD?", b"50\n".to_vec()),
        (":SOURce2:FUNCtion?", b"SIN\n".to_vec()),
        (":SOURce2:FREQuency?", b"1000\n".to_vec()),
        (":SOURce2:VOLTage:UNIT?", b"VRMS\n".to_vec()),
        (":SOURce2:VOLTage?", b"1\n".to_vec()),
        (":SOURce2:VOLTage:OFFSet?", b"-0.1\n".to_vec()),
        (":SOURce2:PHASe?", b"0\n".to_vec()),
        (":OUTPut2:POLarity?", b"INV\n".to_vec()),
        (":OUTPut2:STATe?", b"0\n".to_vec()),
        (":SOURce1:PHASe 90", vec![]),
        (":OUTPut1:POLarity INVerted", vec![]),
    ]);
    let (io, e) = transport(entries, 2);
    let mut generator = Generator::with_transport(
        GeneratorModel::Dg922Pro,
        io,
        Arc::new(ManualClock::default()),
        GeneratorOptions::default(),
    )
    .unwrap();
    generator.connect().unwrap();
    let status = generator.read_status(deadline()).unwrap();
    assert_eq!(status.channels[0].waveform, Waveform::Triangle);
    assert_eq!(status.channels[0].phase_deg, 45.0);
    assert!(status.channels[0].output_enabled);
    assert!((status.channels[1].amplitude_vpp - 2.8284271247461903).abs() < 1e-12);
    assert!(!status.channels[1].output_enabled);
    assert_eq!(
        status.channels[1].polarity,
        yang_drivers::generator::OutputPolarity::Inverted
    );
    assert!(e
        .lock()
        .unwrap()
        .writes
        .iter()
        .all(|s| s.trim_end().ends_with('?')));
    assert!(generator.set_phase(1, f64::NAN, deadline()).is_err());
    generator.set_phase(1, 90.0, deadline()).unwrap();
    generator
        .set_polarity(
            1,
            yang_drivers::generator::OutputPolarity::Inverted,
            deadline(),
        )
        .unwrap();
    generator.close().unwrap();
}
