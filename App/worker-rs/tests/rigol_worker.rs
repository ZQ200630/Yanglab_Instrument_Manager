use serde_json::json;
use std::sync::Arc;
use yang_worker::{
    actions::parse,
    catalog::{physical_keys, Catalog, DOCUMENT},
    session::{DriverFactory, SystemFactory},
};
#[path = "support/native.rs"]
mod support;

fn config(model: &str, kind: &str, resource: &str) -> yang_protocol::DomainConfig {
    yang_protocol::DomainConfig {
        domain: yang_protocol::DomainRef {
            kind: "device".into(),
            id: "b".repeat(32),
        },
        config_rev: 1,
        driver_kind: kind.into(),
        model_id: model.into(),
        profile_id: Some("visa".into()),
        params: json!({"resource":resource}),
        expected_identity: json!({}),
        members: vec![],
    }
}
#[test]
fn rigol_actions_are_typed_and_reject_unreviewed_fields() {
    for (kind, name, args) in [
        ("scope", "read_status", json!({})),
        (
            "scope",
            "set_channel_display",
            json!({"channel":4,"enabled":true}),
        ),
        ("scope", "set_memory_depth", json!({"points":250_000_000})),
        ("scope", "run", json!({})),
        ("scope", "stop", json!({})),
        (
            "scope",
            "capture",
            json!({"channels":[1,4],"source":"memory","points":250_000_000,"byte_budget":500_000_000}),
        ),
        ("generator", "read_status", json!({})),
        (
            "generator",
            "configure_signal",
            json!({"channel":1,"waveform":"triangle","frequency_hz":1000,"amplitude_vpp":2,"offset_v":0}),
        ),
        (
            "generator",
            "set_output",
            json!({"channel":2,"enabled":false}),
        ),
        ("generator", "set_phase", json!({"channel":1,"degrees":180})),
        (
            "generator",
            "set_load",
            json!({"channel":1,"load":{"kind":"high_z"}}),
        ),
        (
            "generator",
            "set_polarity",
            json!({"channel":1,"polarity":"inverted"}),
        ),
        (
            "scope",
            "retry_staging",
            json!({"capture_id":"c".repeat(32)}),
        ),
    ] {
        assert!(parse(kind, name, &args).is_ok(), "{kind}/{name}");
    }
    for (kind, name, args) in [
        (
            "scope",
            "capture",
            json!({"channels":[1,1],"source":"screen"}),
        ),
        (
            "scope",
            "capture",
            json!({"channels":[5],"source":"screen"}),
        ),
        (
            "scope",
            "capture",
            json!({"channels":[1],"source":"screen","points":1001}),
        ),
        (
            "scope",
            "capture",
            json!({"channels":[1],"source":"memory","byte_budget":536870913}),
        ),
        ("scope", "run", json!({"raw":"*RST"})),
        (
            "generator",
            "set_output",
            json!({"channel":3,"enabled":true}),
        ),
        (
            "generator",
            "configure_signal",
            json!({"channel":1,"waveform":"triangle","frequency_hz":1,"amplitude_vpp":21,"offset_v":0}),
        ),
        ("generator", "set_phase", json!({"channel":1,"degrees":361})),
    ] {
        assert!(parse(kind, name, &args).is_err(), "{kind}/{name}");
    }
}
#[test]
fn reviewed_rigol_catalog_factories_are_lazy_and_canonicalize_visa() {
    let catalog = Catalog::load(DOCUMENT).unwrap();
    let factory = SystemFactory::new(Arc::new(yang_drivers::clock::SystemClock::default()));
    for (model, kind) in [
        ("ds7054", "scope"),
        ("mho5054", "scope"),
        ("dg4162", "generator"),
        ("dg4202", "generator"),
        ("dg922pro", "generator"),
    ] {
        let approved =
            yang_worker::catalog::admit(&config(model, kind, "TCPIP0::SCOPE.LAB::inst0::INSTR"))
                .unwrap();
        assert_eq!(approved.params["backend"], "system");
        assert!(catalog
            .model(model)
            .unwrap()
            .close_effects
            .iter()
            .any(|s| s == "preserve_output_and_settings"));
        let mut driver = factory.create(&approved).unwrap();
        assert!(!driver.has_responsibility());
        assert!(driver.close().unwrap().resources_released());
        let a = physical_keys(&approved).unwrap();
        let b = physical_keys(&config(model, kind, "TCPIP::scope.lab::INSTR")).unwrap();
        assert_eq!(a, b);
        assert!(catalog
            .validate_connection(
                model,
                "visa",
                &json!({"resource":"TCPIP0::scope.lab::5025::SOCKET"})
            )
            .is_err());
        assert!(catalog
            .validate_connection(
                model,
                "visa",
                &json!({"resource":"GPIB0::4::INSTR","backend":"@py"})
            )
            .is_err());
    }
    assert!(!factory.auxiliary_responsibility());
}
#[test]
fn fresh_capture_is_an_explicit_bounded_action_not_a_default_side_effect() {
    assert!(parse(
        "scope",
        "capture",
        &json!({"channels":[1],"source":"memory","points":10,"fresh":true})
    )
    .is_ok());
    assert!(parse(
        "scope",
        "capture",
        &json!({"channels":[1],"source":"memory","fresh":"yes"})
    )
    .is_err());
}
#[test]
fn snapshot_capture_is_explicit_typed_and_excludes_fresh_triggering() {
    assert!(parse(
        "scope",
        "capture",
        &json!({"channels":[1],"source":"memory","points":10,"stop_before":true})
    )
    .is_ok());
    for args in [
        json!({"channels":[1],"source":"memory","stop_before":"yes"}),
        json!({"channels":[1],"source":"memory","fresh":true,"stop_before":true}),
    ] {
        assert!(parse("scope", "capture", &args).is_err());
    }
}
#[test]
fn explicit_stop_and_selected_output_off_use_the_safety_lane() {
    let request = |name: &str, args| yang_protocol::RequestV3 {
        v: 3,
        id: "f".repeat(32),
        method: "action".into(),
        params: json!({"name":name,"args":args}),
        context: None,
    };
    assert!(yang_worker::safety::intent("scope", &request("stop", json!({}))).is_some());
    assert!(yang_worker::safety::intent(
        "generator",
        &request("set_output", json!({"channel":1,"enabled":false}))
    )
    .is_some());
    assert!(yang_worker::safety::intent(
        "generator",
        &request("set_output", json!({"channel":1,"enabled":true}))
    )
    .is_none());
    assert!(yang_worker::safety::intent("scope", &request("run", json!({}))).is_none());
    assert!(
        yang_worker::safety::intent(
            "scope",
            &request(
                "capture",
                json!({"channels":[1],"source":"memory","stop_before":true})
            )
        )
        .is_none(),
        "snapshot STOP belongs to the capture operation, not the public safety lane"
    );
}
mod spool {
    use super::*;
    use yang_data::{Calibration, ChannelWaveform, Samples, WaveformFrame};
    #[test]
    fn raw_waveform_spool_has_no_osa_point_limit_and_preserves_unsigned_word_tail() {
        let nonce = yang_worker::new_id().unwrap();
        let root = std::env::temp_dir()
            .join("native-waveform-spool-tests")
            .join(&nonce);
        std::fs::create_dir_all(&root).unwrap();
        let mut spool =
            yang_worker::captures::CaptureSpool::open(root.clone(), nonce.clone()).unwrap();
        let id = "d".repeat(32);
        let count = 200_002;
        let mut samples = vec![32768_u16; count];
        samples[count - 1] = 65535;
        let frame = WaveformFrame {
            id: "e".repeat(32),
            captured_at_unix_ms: 42,
            nominal_offset_s: Some(0.5),
            channels: vec![ChannelWaveform {
                channel: "CH1".into(),
                samples: Samples::U16(samples),
                calibration: Calibration {
                    x_increment_s: 1e-9,
                    x_origin_s: -1e-6,
                    x_reference: 0.,
                    y_increment_v: 0.001,
                    y_origin: -3.,
                    y_reference: 32768.,
                },
                metadata: json!({"convention":"unsigned"}),
            }],
        };
        spool.reserve_read(&id, (count * 2) as u64).unwrap();
        let descriptor = spool
            .stage_waveform_owned(&id, &frame, &json!({"model":"mho5054","identity":{}}))
            .unwrap();
        assert_eq!(descriptor.kind, "waveform");
        assert_eq!(descriptor.point_count, count as u64);
        assert_eq!(descriptor.byte_count, (count * 2) as u64);
        let channel = &descriptor.metadata["frame"]["channels"][0];
        assert_eq!(channel["dtype"], "u16_le");
        assert_eq!(channel["byte_offset"], 0);
        assert_eq!(channel["calibration"]["y_reference"], 32768.);
        assert_eq!(
            spool
                .read_chunk(&nonce, &id, (count * 2 - 4) as u64, 4)
                .unwrap(),
            vec![0, 128, 255, 255]
        );
        assert!(spool.read_chunk(&nonce, &id, 0, 16385).is_err());
        assert!(
            std::fs::write(root.join(format!("{id}.bin")), b"tamper").is_err(),
            "pinned raw bytes deny external mutation"
        );
        assert!(spool.ack(&nonce, &id, &"0".repeat(64)).is_err());
        spool.ack(&nonce, &id, &descriptor.sha256).unwrap();
        spool.ack(&nonce, &id, &descriptor.sha256).unwrap();
        drop(spool);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn exact_raw_byte_reservations_bound_concurrent_acquisition_before_io() {
        let nonce = yang_worker::new_id().unwrap();
        let root = std::env::temp_dir()
            .join("native-waveform-spool-tests")
            .join(&nonce);
        std::fs::create_dir_all(&root).unwrap();
        let mut spool = yang_worker::captures::CaptureSpool::open(root.clone(), nonce).unwrap();
        assert!(spool
            .reserve_read(&"a".repeat(32), yang_protocol::MAX_WAVEFORM_BYTES)
            .is_ok());
        assert!(
            spool
                .reserve_read(&"b".repeat(32), yang_protocol::MAX_WAVEFORM_BYTES)
                .is_err(),
            "metadata and pending bytes must fit before any read"
        );
        spool.release_read(&"a".repeat(32));
        assert!(spool.reserve_read(&"b".repeat(32), 500_000_000).is_ok());
        drop(spool);
        std::fs::remove_dir_all(root).unwrap();
    }
}
mod native {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex, time::Duration};
    use yang_drivers::{
        clock::ManualClock,
        generator::{Generator, GeneratorModel, GeneratorOptions},
        lifecycle::{DriverLifecycle, DriverState},
        scope::{Scope, ScopeModel, ScopeOptions},
        transport::{ByteTransport, CloseReport, Deadline},
        DriverError, DriverResult,
    };
    use yang_worker::{rigol_sessions::RigolSession, session::DeviceSession};
    struct Transcript {
        exchanges: VecDeque<(String, Vec<u8>)>,
        pending: VecDeque<u8>,
        writes: Arc<Mutex<Vec<String>>>,
        live: bool,
    }
    impl ByteTransport for Transcript {
        fn write_all(&mut self, bytes: &[u8], _: Deadline) -> DriverResult<()> {
            let command = String::from_utf8(bytes.to_vec()).unwrap();
            self.writes.lock().unwrap().push(command.clone());
            let (want, reply) = self
                .exchanges
                .pop_front()
                .ok_or_else(|| DriverError::Protocol(format!("unexpected {command}")))?;
            assert_eq!(command, want);
            assert!(self.pending.is_empty());
            self.pending.extend(reply);
            Ok(())
        }
        fn read_bounded(&mut self, maximum: usize, _: Deadline) -> DriverResult<Vec<u8>> {
            assert!(maximum <= 16384);
            let count = maximum.min(3).min(self.pending.len());
            if count == 0 {
                return Err(DriverError::Protocol("truncated transcript".into()));
            }
            Ok(self.pending.drain(..count).collect())
        }
        fn close(&mut self) -> DriverResult<CloseReport> {
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
    fn transport(
        entries: Vec<(&str, Vec<u8>)>,
    ) -> (Box<dyn ByteTransport>, Arc<Mutex<Vec<String>>>) {
        let writes = Arc::new(Mutex::new(vec![]));
        (
            Box::new(Transcript {
                exchanges: entries
                    .into_iter()
                    .map(|(q, a)| (format!("{q}\n"), a))
                    .collect(),
                pending: VecDeque::new(),
                writes: writes.clone(),
                live: true,
            }),
            writes,
        )
    }
    fn text(s: &str) -> Vec<u8> {
        format!("{s}\n").into_bytes()
    }
    enum ReplyDelay {
        Duration(Duration),
        UntilDeadline,
    }
    struct DelayedReply {
        inner: Box<dyn ByteTransport>,
        command: &'static [u8],
        occurrences: &'static [usize],
        seen: usize,
        pending: bool,
        delay: ReplyDelay,
    }
    impl ByteTransport for DelayedReply {
        fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<()> {
            self.inner.write_all(bytes, deadline)?;
            if bytes == self.command {
                self.seen += 1;
                self.pending = self.occurrences.contains(&self.seen);
            }
            Ok(())
        }
        fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
            let reply = self.inner.read_bounded(maximum, deadline)?;
            if self.pending && reply.ends_with(b"\n") {
                self.pending = false;
                match self.delay {
                    ReplyDelay::Duration(delay) => {
                        std::thread::sleep(delay);
                        deadline.remaining_millis()?;
                    }
                    ReplyDelay::UntilDeadline => {
                        // A complete reply may arrive at its native I/O deadline.
                        // Keep this injected boundary bounded even if a bug passes
                        // an unexpectedly long transport deadline.
                        let bound = std::time::Instant::now() + Duration::from_millis(250);
                        while deadline.remaining_millis().is_ok() {
                            assert!(std::time::Instant::now() < bound, "unbounded I/O deadline");
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                }
            }
            Ok(reply)
        }
        fn close(&mut self) -> DriverResult<CloseReport> {
            self.inner.close()
        }
        fn has_responsibility(&self) -> bool {
            self.inner.has_responsibility()
        }
    }
    fn context() -> yang_protocol::ContextV3 {
        yang_protocol::ContextV3 {
            session_id: "a".repeat(32),
            domain: Some(config("ds7054", "scope", "GPIB0::4::INSTR").domain),
            connection_id: Some("c".repeat(32)),
            epoch: 1,
        }
    }
    // Catches a successful depth action carrying only stale cached status, so
    // the Host could begin capture without confirmed application evidence.
    #[test]
    fn depth_action_returns_confirmed_actual_depth_and_restored_stop() {
        let clock = Arc::new(ManualClock::default());
        let (io, writes) = transport(vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":TRIGger:STATus?", text("STOP")),
            (":ACQuire:MDEPth?", text("10M")),
            (":RUN", vec![]),
            (":TRIGger:STATus?", text("RUN")),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", text("100K")),
            (":STOP", vec![]),
            (":TRIGger:STATus?", text("STOP")),
            (":ACQuire:MDEPth?", text("100K")),
            (":TRIGger:STATus?", text("STOP")),
            (":ACQuire:MDEPth?", text("100K")),
            (":CHANnel1:DISPlay?", text("1")),
            (":CHANnel2:DISPlay?", text("1")),
            (":CHANnel3:DISPlay?", text("0")),
            (":CHANnel4:DISPlay?", text("0")),
        ]);
        let driver = Scope::with_transport(
            ScopeModel::Ds7054,
            io,
            clock.clone(),
            ScopeOptions::default(),
        ).unwrap();
        let mut session = RigolSession::from_scope(driver, clock).unwrap();
        session.connect().unwrap();
        let outcome = session.action("set_memory_depth", &json!({"points":100_000}), &context());
        assert_eq!(outcome.phase, yang_protocol::Phase::Completed);
        let status = outcome.result.unwrap()["status"].clone();
        assert_eq!(status["memory_depth"], 100_000);
        assert_eq!(status["running"], false);
        assert_eq!(status["acquisition"], "stopped");
        assert_eq!(status["quality"], "confirmed");
        assert_eq!(status["previous"], false);
        let before = writes.lock().unwrap().len();
        session.close().unwrap();
        assert_eq!(before, writes.lock().unwrap().len());
    }

    // Catches reusing the exhausted depth deadline for the final status read:
    // the depth reply is complete, so the action must still confirm its status.
    #[test]
    fn depth_action_reads_status_with_a_new_budget_after_deadline_boundary_reply() {
        let clock = Arc::new(ManualClock::default());
        let (io, writes) = transport(vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":TRIGger:STATus?", text("RUN")),
            (":ACQuire:MDEPth?", text("10M")),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", text("100K")),
            (":TRIGger:STATus?", text("RUN")),
            (":ACQuire:MDEPth?", text("100K")),
            (":CHANnel1:DISPlay?", text("1")),
            (":CHANnel2:DISPlay?", text("1")),
            (":CHANnel3:DISPlay?", text("0")),
            (":CHANnel4:DISPlay?", text("0")),
        ]);
        let io = Box::new(DelayedReply {
            inner: io,
            command: b":ACQuire:MDEPth?\n",
            occurrences: &[2],
            seen: 0,
            pending: false,
            delay: ReplyDelay::UntilDeadline,
        });
        let driver = Scope::with_transport(
            ScopeModel::Ds7054, io, clock.clone(), ScopeOptions {
                timeout: Duration::from_millis(80),
                depth_timeout: Duration::from_millis(80),
                ..Default::default()
            },
        ).unwrap();
        let mut session = RigolSession::from_scope(driver, clock).unwrap();
        session.connect().unwrap();
        let outcome = session.action("set_memory_depth", &json!({"points":100_000}), &context());
        assert_eq!(outcome.phase, yang_protocol::Phase::Completed, "{outcome:?}");
        let status = outcome.result.unwrap()["status"].clone();
        assert_eq!(status["memory_depth"], 100_000);
        assert_eq!(status["quality"], "confirmed");
        assert_eq!(status["previous"], false);
        assert_eq!(status["channels"].as_array().unwrap().len(), 4);
        assert_eq!(writes.lock().unwrap().iter().filter(|q| *q == ":ACQuire:MDEPth 100000\n").count(), 1);
        assert!(session.close().unwrap().resources_released());
    }

    // Catches limiting the whole depth transition to one I/O stall budget even
    // though every individual reply arrives within that communication budget.
    #[test]
    fn depth_action_allows_confirmation_across_multiple_io_budgets() {
        let clock = Arc::new(yang_drivers::clock::SystemClock::default());
        let (io, writes) = transport(vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":TRIGger:STATus?", text("RUN")),
            (":ACQuire:MDEPth?", text("10M")),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", text("10M")),
            (":ACQuire:MDEPth?", text("100K")),
            (":TRIGger:STATus?", text("RUN")),
            (":ACQuire:MDEPth?", text("100K")),
            (":CHANnel1:DISPlay?", text("1")),
            (":CHANnel2:DISPlay?", text("0")),
            (":CHANnel3:DISPlay?", text("0")),
            (":CHANnel4:DISPlay?", text("0")),
        ]);
        let io = Box::new(DelayedReply {
            inner: io,
            command: b":ACQuire:MDEPth?\n",
            occurrences: &[2, 3],
            seen: 0,
            pending: false,
            delay: ReplyDelay::Duration(Duration::from_millis(50)),
        });
        let driver = Scope::with_transport(
            ScopeModel::Ds7054, io, clock.clone(), ScopeOptions {
                timeout: Duration::from_millis(80),
                ..Default::default()
            },
        ).unwrap();
        let mut session = RigolSession::from_scope(driver, clock).unwrap();
        session.connect().unwrap();
        let outcome = session.action("set_memory_depth", &json!({"points":100_000}), &context());
        assert_eq!(outcome.phase, yang_protocol::Phase::Completed, "{outcome:?}");
        assert_eq!(outcome.result.unwrap()["status"]["memory_depth"], 100_000);
        let writes = writes.lock().unwrap();
        assert_eq!(writes.iter().filter(|q| *q == ":ACQuire:MDEPth 100000\n").count(), 1);
        assert!(!writes.iter().any(|q| q == ":RUN\n" || q == ":STOP\n"));
        drop(writes);
        assert!(session.close().unwrap().resources_released());
    }

    // Catches replaying a failed depth write or requiring forced cleanup before
    // a newly connected, independent session can capture the existing buffer.
    #[test]
    fn failed_depth_closes_normally_before_independent_session_capture() {
        let clock = Arc::new(ManualClock::default());
        let (io, writes) = transport(vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":TRIGger:STATus?", text("RUN")),
            (":ACQuire:MDEPth?", text("10M")),
            (":ACQuire:MDEPth 100000", vec![]),
            (":ACQuire:MDEPth?", text("10M")),
            (":ACQuire:MDEPth?", text("10M")),
            (":ACQuire:MDEPth?", text("10M")),
        ]);
        let driver = Scope::with_transport(
            ScopeModel::Ds7054, io, clock.clone(), ScopeOptions {
                depth_timeout: Duration::from_millis(40),
                ..Default::default()
            },
        ).unwrap();
        let mut failed = RigolSession::from_scope(driver, clock.clone()).unwrap();
        failed.connect().unwrap();
        let outcome = failed.action("set_memory_depth", &json!({"points":100_000}), &context());
        assert_eq!(outcome.phase, yang_protocol::Phase::FailedAfterCallStarted);
        assert!(outcome.error.unwrap().message.contains("Timeout"));
        assert_eq!(failed.state(), DriverState::Fault);
        let before = writes.lock().unwrap().clone();
        assert!(failed.close().unwrap().resources_released());
        assert_eq!(*writes.lock().unwrap(), before, "normal close must not replay or force outputs");
        assert_eq!(before.iter().filter(|q| *q == ":ACQuire:MDEPth 100000\n").count(), 1);
        assert!(!before.iter().any(|q| q == ":RUN\n" || q == ":STOP\n"));

        let mut entries = vec![("*IDN?", text("RIGOL,DS7054,SCOPE1,01"))];
        entries.extend(capture_entries(false, 129));
        let (io, _) = transport(entries);
        let driver = Scope::with_transport(
            ScopeModel::Ds7054, io, clock.clone(), ScopeOptions::default(),
        ).unwrap();
        let mut recovered = RigolSession::from_scope(driver, clock).unwrap();
        recovered.connect().unwrap();
        let mut next_context = context();
        next_context.connection_id = Some("d".repeat(32));
        next_context.epoch = 2;
        let outcome = recovered.action(
            "capture", &json!({"channels":[1],"source":"memory","points":2,"byte_budget":2}),
            &next_context,
        );
        assert_eq!(outcome.phase, yang_protocol::Phase::Completed, "{outcome:?}");
        let frame = recovered.take_waveform().unwrap();
        assert_eq!(frame.channels[0].samples.value(0), Some(129.0));
        assert!(recovered.close().unwrap().resources_released());
    }
    fn capture_entries(fresh: bool, raw: u8) -> Vec<(&'static str, Vec<u8>)> {
        let mut entries = vec![];
        if fresh {
            entries.extend([
                (":SINGle", vec![]),
                (":TRIGger:STATus?", text("WAIT")),
                (":TRIGger:STATus?", text("STOP")),
            ]);
        }
        entries.extend([
            (":TRIGger:STATus?", text("STOP")),
            (":ACQuire:MDEPth?", text("1K")),
            (":WAVeform:SOURce CHANnel1", vec![]),
            (":WAVeform:MODE RAW", vec![]),
            (":WAVeform:FORMat BYTE", vec![]),
            (":WAVeform:STARt 1", vec![]),
            (":WAVeform:POINts 2", vec![]),
            (":WAVeform:STOP 2", vec![]),
            (":WAVeform:PREamble?", text("0,2,2,1,1e-9,0,0,0.01,10,128")),
            (":WAVeform:STARt 1", vec![]),
            (":WAVeform:STOP 2", vec![]),
            (":WAVeform:DATA?", vec![b'#', b'1', b'2', raw, 255, b'\n']),
        ]);
        entries
    }
    #[test]
    fn ds7054_readonly_probe_preserves_raw_transfer_state_with_queries_only() {
        let clock = Arc::new(ManualClock::default());
        let (io, writes) = transport(vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":TRIGger:STATus?", text("STOP")),
            (":ACQuire:MDEPth?", text("1.0000E+04")),
            (":WAVeform:SOURce?", text("MATH1")),
            (":WAVeform:MODE?", text("MAX")),
            (":WAVeform:FORMat?", text("ASC")),
            (":WAVeform:POINts?", text("1000")),
            (":WAVeform:STARt?", text("121")),
            (":WAVeform:STOP?", text("1020")),
            (
                ":WAVeform:PREamble?",
                text("2,1,1000,8,1e-9,-1e-6,0,0.01,10,128"),
            ),
            (":CHANnel1:DISPlay?", text("ON")),
            (":CHANnel2:DISPlay?", text("0")),
            (":CHANnel3:DISPlay?", text("1")),
            (":CHANnel4:DISPlay?", text("OFF")),
        ]);
        let mut session = RigolSession::from_scope(
            Scope::with_transport(
                ScopeModel::Ds7054,
                io,
                clock.clone(),
                ScopeOptions::default(),
            )
            .unwrap(),
            clock,
        )
        .unwrap();
        let report = session.probe_readonly().unwrap();
        assert_eq!(
            report.identity(),
            &json!({"manufacturer":"RIGOL","model":"DS7054","serial":"SCOPE1","firmware":"01"})
        );
        assert_eq!(
            report.observations()["connect_effects"],
            "identity_query_only"
        );
        assert_eq!(
            report.observations()["transfer_state"],
            json!({
                "model":"DS7054", "trigger_status":"STOP", "acquisition_memory_depth":"1.0000E+04",
                "waveform_source":"MATH1", "waveform_mode":"MAX", "waveform_format":"ASC",
                "waveform_points":"1000", "waveform_start":"121", "waveform_stop":"1020",
                "waveform_preamble":"2,1,1000,8,1e-9,-1e-6,0,0.01,10,128",
                "channel_1_display":"ON", "channel_2_display":"0", "channel_3_display":"1", "channel_4_display":"OFF",
            }),
            "an ASC preamble must remain raw instead of being parsed as capture BYTE data"
        );
        assert_eq!(session.state(), DriverState::Ready);
        assert!(!report.release_confirmed());
        assert!(session.close().unwrap().resources_released());
        assert_eq!(
            writes.lock().unwrap().as_slice(),
            [
                "*IDN?\n",
                ":TRIGger:STATus?\n",
                ":ACQuire:MDEPth?\n",
                ":WAVeform:SOURce?\n",
                ":WAVeform:MODE?\n",
                ":WAVeform:FORMat?\n",
                ":WAVeform:POINts?\n",
                ":WAVeform:STARt?\n",
                ":WAVeform:STOP?\n",
                ":WAVeform:PREamble?\n",
                ":CHANnel1:DISPlay?\n",
                ":CHANnel2:DISPlay?\n",
                ":CHANnel3:DISPlay?\n",
                ":CHANnel4:DISPlay?\n",
            ],
            "probe and close must not change settings, run/stop acquisition, or transfer samples"
        );
    }
    #[test]
    fn readonly_transfer_query_failure_is_not_retried_or_followed_by_actuation() {
        let clock = Arc::new(ManualClock::default());
        let (io, writes) = transport(vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":TRIGger:STATus?", vec![]),
        ]);
        let mut session = RigolSession::from_scope(
            Scope::with_transport(
                ScopeModel::Ds7054,
                io,
                clock.clone(),
                ScopeOptions::default(),
            )
            .unwrap(),
            clock,
        )
        .unwrap();
        let error = session.probe_readonly().unwrap_err();
        assert!(error.to_string().contains(":TRIGger:STATus?"), "{error}");
        assert_eq!(session.state(), DriverState::Fault);
        assert!(session.close().unwrap().resources_released());
        assert_eq!(
            writes.lock().unwrap().as_slice(),
            ["*IDN?\n", ":TRIGger:STATus?\n"]
        );
    }
    #[test]
    fn normal_scope_connect_and_other_model_probe_remain_identity_only() {
        for (model, idn, readonly) in [
            (ScopeModel::Ds7054, "RIGOL,DS7054,SCOPE1,01", false),
            (ScopeModel::Mho5054, "RIGOL,MHO5054,SCOPE2,02", true),
        ] {
            let clock = Arc::new(ManualClock::default());
            let (io, writes) = transport(vec![("*IDN?", text(idn))]);
            let mut session = RigolSession::from_scope(
                Scope::with_transport(model, io, clock.clone(), ScopeOptions::default()).unwrap(),
                clock,
            )
            .unwrap();
            let report = if readonly {
                session.probe_readonly()
            } else {
                session.connect()
            }
            .unwrap();
            assert_eq!(
                report.observations(),
                &json!({"connect_effects":"identity_query_only"})
            );
            assert!(session.close().unwrap().resources_released());
            assert_eq!(writes.lock().unwrap().as_slice(), ["*IDN?\n"]);
        }
    }
    #[test]
    fn snapshot_stops_then_confirms_and_returns_existing_buffer_without_followup_io() {
        let clock = Arc::new(ManualClock::default());
        let mut entries = vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":STOP", vec![]),
            (":TRIGger:STATus?", text("RUN")),
            (":TRIGger:STATus?", text("STOP")),
        ];
        entries.extend(capture_entries(false, 128));
        let expected_calls = entries.len();
        let (io, writes) = transport(entries);
        let mut session = RigolSession::from_scope(
            Scope::with_transport(
                ScopeModel::Ds7054,
                io,
                clock.clone(),
                ScopeOptions::default(),
            )
            .unwrap(),
            clock,
        )
        .unwrap();
        session.connect().unwrap();
        let outcome = session.action(
            "capture",
            &json!({"channels":[1],"source":"memory","points":2,"byte_budget":2,"stop_before":true}),
            &context(),
        );
        assert_eq!(
            outcome.phase,
            yang_protocol::Phase::Completed,
            "{:?}",
            outcome.error
        );
        let frame = session
            .take_waveform()
            .expect("completed raw frame must survive");
        assert_eq!(frame.channels[0].samples.value(0), Some(128.));
        assert_eq!(
            frame.channels[0].metadata["snapshot"],
            json!({
                "confirmed":true,"method":"stop_buffer","final_state":"STOP","fresh_acquisition":false
            })
        );
        assert!(frame.channels[0]
            .metadata
            .get("fresh_acquisition")
            .is_none());
        assert_eq!(writes.lock().unwrap().len(), expected_calls);
        session.close().unwrap();
        assert_eq!(writes.lock().unwrap().len(), expected_calls);
        assert!(!writes
            .lock()
            .unwrap()
            .iter()
            .any(|q| q.contains("SINGle") || q.contains(":RUN")));
    }
    #[test]
    fn snapshot_preflight_rejects_conflicts_and_byte_overflow_before_stop() {
        let clock = Arc::new(ManualClock::default());
        let (io, writes) = transport(vec![("*IDN?", text("RIGOL,DS7054,SCOPE1,01"))]);
        let mut session = RigolSession::from_scope(
            Scope::with_transport(
                ScopeModel::Ds7054,
                io,
                clock.clone(),
                ScopeOptions::default(),
            )
            .unwrap(),
            clock,
        )
        .unwrap();
        session.connect().unwrap();
        let conflict = session.action("capture", &json!({
            "channels":[1],"source":"memory","points":2,"byte_budget":2,"fresh":true,"stop_before":true
        }), &context());
        assert_eq!(conflict.phase, yang_protocol::Phase::RejectedBeforeCall);
        let overflow = session.action(
            "capture",
            &json!({
                "channels":[1],"source":"memory","points":2,"byte_budget":1,"stop_before":true
            }),
            &context(),
        );
        assert_eq!(overflow.phase, yang_protocol::Phase::FailedAfterCallStarted);
        assert!(overflow.error.unwrap().message.contains("budget"));
        assert_eq!(writes.lock().unwrap().as_slice(), ["*IDN?\n"]);
        session.close().unwrap();
    }
    #[test]
    fn snapshot_stop_confirmation_timeout_is_bounded_without_replay_or_transfer() {
        use yang_drivers::clock::Clock;
        let clock = Arc::new(ManualClock::default());
        let mut entries = vec![("*IDN?", text("RIGOL,DS7054,SCOPE1,01")), (":STOP", vec![])];
        for _ in 0..4 {
            entries.push((":TRIGger:STATus?", text("RUN")));
        }
        let expected_calls = entries.len();
        let (io, writes) = transport(entries);
        let mut session = RigolSession::from_scope(
            Scope::with_transport(
                ScopeModel::Ds7054,
                io,
                clock.clone(),
                ScopeOptions {
                    timeout: std::time::Duration::from_millis(60),
                    ..Default::default()
                },
            )
            .unwrap(),
            clock.clone(),
        )
        .unwrap();
        session.connect().unwrap();
        let outcome = session.action(
            "capture",
            &json!({
                "channels":[1],"source":"memory","points":2,"byte_budget":2,"stop_before":true
            }),
            &context(),
        );
        assert_eq!(outcome.phase, yang_protocol::Phase::FailedAfterCallStarted);
        assert!(outcome.error.unwrap().message.contains("state uncertain"));
        assert_eq!(clock.now(), std::time::Duration::from_millis(60));
        assert!(session.take_waveform().is_none());
        session.close().unwrap();
        assert_eq!(writes.lock().unwrap().len(), expected_calls);
        assert_eq!(
            writes
                .lock()
                .unwrap()
                .iter()
                .filter(|q| *q == ":STOP\n")
                .count(),
            1
        );
    }
    #[test]
    fn snapshot_failed_transfer_never_replays_stop_or_data() {
        let clock = Arc::new(ManualClock::default());
        let mut entries = vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":STOP", vec![]),
            (":TRIGger:STATus?", text("STOP")),
        ];
        entries.extend(capture_entries(false, 128));
        entries.last_mut().unwrap().1 = vec![];
        let expected_calls = entries.len();
        let (io, writes) = transport(entries);
        let mut session = RigolSession::from_scope(
            Scope::with_transport(
                ScopeModel::Ds7054,
                io,
                clock.clone(),
                ScopeOptions::default(),
            )
            .unwrap(),
            clock,
        )
        .unwrap();
        session.connect().unwrap();
        let outcome = session.action(
            "capture",
            &json!({
                "channels":[1],"source":"memory","points":2,"byte_budget":2,"stop_before":true
            }),
            &context(),
        );
        assert_eq!(outcome.phase, yang_protocol::Phase::FailedAfterCallStarted);
        assert!(session.take_waveform().is_none());
        session.close().unwrap();
        assert_eq!(writes.lock().unwrap().len(), expected_calls);
        assert_eq!(
            writes
                .lock()
                .unwrap()
                .iter()
                .filter(|q| *q == ":STOP\n")
                .count(),
            1
        );
        assert_eq!(
            writes
                .lock()
                .unwrap()
                .iter()
                .filter(|q| *q == ":WAVeform:DATA?\n")
                .count(),
            1
        );
    }
    #[test]
    fn scope_session_cached_observation_fresh_frames_and_explicit_stop_are_real_driver_calls() {
        let clock = Arc::new(ManualClock::default());
        let mut entries = vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01.00")),
            (":TRIGger:STATus?", text("STOP")),
            (":ACQuire:MDEPth?", text("1K")),
        ];
        entries.extend([
            (":CHANnel1:DISPlay?", text("1")),
            (":CHANnel2:DISPlay?", text("0")),
            (":CHANnel3:DISPlay?", text("0")),
            (":CHANnel4:DISPlay?", text("0")),
        ]);
        entries.extend(capture_entries(true, 128));
        entries.extend(capture_entries(true, 129));
        entries.push((":STOP", vec![]));
        let (io, writes) = transport(entries);
        let driver = Scope::with_transport(
            ScopeModel::Ds7054,
            io,
            clock.clone(),
            ScopeOptions::default(),
        )
        .unwrap();
        let mut session = RigolSession::from_scope(driver, clock).unwrap();
        session.connect().unwrap();
        let ctx = context();
        assert_eq!(session.identity()["serial"], "SCOPE1");
        let status = session.action("read_status", &json!({}), &ctx);
        assert_eq!(status.phase, yang_protocol::Phase::Completed);
        let status = status.result.unwrap();
        assert_eq!(status["status"]["identity"], session.identity());
        assert_eq!(
            status["status"]["identity_text"],
            "RIGOL,DS7054,SCOPE1,01.00"
        );
        assert_eq!(status["status"]["channels"][0]["displayed"], true);
        let before = writes.lock().unwrap().len();
        let observer = session.cached_observer().unwrap();
        for _ in 0..10 {
            assert_eq!(observer.observe(&ctx).status["memory_depth"], 1000);
            assert_eq!(
                observer.observe(&ctx).status["identity"],
                session.identity()
            );
        }
        assert_eq!(before, writes.lock().unwrap().len());
        let args =
            json!({"channels":[1],"source":"memory","points":2,"byte_budget":2,"fresh":true});
        let mut ids = vec![];
        for raw in [128, 129] {
            assert_eq!(
                session.action("capture", &args, &ctx).phase,
                yang_protocol::Phase::Completed
            );
            let frame = session.take_waveform().unwrap();
            assert_eq!(frame.channels[0].samples.value(0), Some(raw as f64));
            assert_eq!(
                frame.channels[0].metadata["fresh_acquisition"]["confirmed"],
                true
            );
            ids.push(frame.id);
        }
        assert_ne!(ids[0], ids[1]);
        session.stop_signal().cancel_operation();
        assert_eq!(
            session.action("stop", &json!({}), &ctx).phase,
            yang_protocol::Phase::Completed
        );
        assert_eq!(session.state(), DriverState::Ready);
        let before = writes.lock().unwrap().len();
        assert!(session.close().unwrap().resources_released());
        assert_eq!(
            before,
            writes.lock().unwrap().len(),
            "normal close must send no commands"
        );
    }
    #[test]
    fn immediate_stop_is_not_falsely_reported_as_fresh_and_no_capture_is_returned() {
        let clock = Arc::new(ManualClock::default());
        let (io, writes) = transport(vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":SINGle", vec![]),
            (":TRIGger:STATus?", text("STOP")),
        ]);
        let mut session = RigolSession::from_scope(
            Scope::with_transport(
                ScopeModel::Ds7054,
                io,
                clock.clone(),
                ScopeOptions::default(),
            )
            .unwrap(),
            clock,
        )
        .unwrap();
        session.connect().unwrap();
        let outcome = session.action(
            "capture",
            &json!({"channels":[1],"source":"memory","points":2,"byte_budget":2,"fresh":true}),
            &context(),
        );
        assert_eq!(outcome.phase, yang_protocol::Phase::FailedAfterCallStarted);
        assert!(outcome.error.unwrap().message.contains("unconfirmed"));
        assert!(session.take_waveform().is_none());
        assert_eq!(writes.lock().unwrap().len(), 3);
        session.close().unwrap();
    }
    #[test]
    fn fresh_trigger_wait_is_bounded_and_does_not_force_or_replay_acquisition() {
        use yang_drivers::clock::Clock;
        let clock = Arc::new(ManualClock::default());
        let mut entries = vec![
            ("*IDN?", text("RIGOL,DS7054,SCOPE1,01")),
            (":SINGle", vec![]),
        ];
        for _ in 0..4 {
            entries.push((":TRIGger:STATus?", text("WAIT")));
        }
        let (io, writes) = transport(entries);
        let driver = Scope::with_transport(
            ScopeModel::Ds7054,
            io,
            clock.clone(),
            ScopeOptions {
                timeout: std::time::Duration::from_millis(60),
                ..Default::default()
            },
        )
        .unwrap();
        let mut session = RigolSession::from_scope(driver, clock.clone()).unwrap();
        session.connect().unwrap();
        let outcome = session.action(
            "capture",
            &json!({"channels":[1],"source":"memory","points":2,"byte_budget":2,"fresh":true}),
            &context(),
        );
        assert_eq!(outcome.phase, yang_protocol::Phase::FailedAfterCallStarted);
        assert!(outcome.error.unwrap().message.contains("state uncertain"));
        assert!(session.take_waveform().is_none());
        assert_eq!(clock.now(), std::time::Duration::from_millis(60));
        session.close().unwrap();
        assert!(!writes
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("FORce") || c.contains(":RUN") || c.contains(":STOP")));
    }
    #[test]
    fn generator_explicit_selected_off_survives_operation_cancel_and_close_preserves_other_output()
    {
        let clock = Arc::new(ManualClock::default());
        let (io, writes) = transport(vec![
            ("*IDN?", text("RIGOL,DG922 Pro,GEN1,01")),
            (":OUTPut2:STATe OFF", vec![]),
        ]);
        let mut session = RigolSession::from_generator(
            Generator::with_transport(
                GeneratorModel::Dg922Pro,
                io,
                clock.clone(),
                GeneratorOptions::default(),
            )
            .unwrap(),
            clock,
        )
        .unwrap();
        session.connect().unwrap();
        session.stop_signal().cancel_operation();
        assert_eq!(
            session
                .action(
                    "set_output",
                    &json!({"channel":2,"enabled":false}),
                    &context()
                )
                .phase,
            yang_protocol::Phase::Completed
        );
        let receipt = session.close().unwrap();
        assert!(receipt.resources_released());
        assert_eq!(
            writes.lock().unwrap().as_slice(),
            ["*IDN?\n", ":OUTPut2:STATe OFF\n"]
        );
    }
    fn held_generator(
        failed_read: bool,
    ) -> (
        yang_worker::scheduler::Scheduler,
        Arc<yang_worker::backend::NativeBackend>,
        yang_protocol::ContextV3,
        Arc<Mutex<Vec<String>>>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use yang_worker::{
            backend::NativeBackend,
            discovery::InventoryPort,
            scheduler::{Backend, Scheduler},
            DomainRegistry, WorkerError,
        };
        struct Factory {
            created: AtomicUsize,
            failed_read: bool,
            writes: Mutex<Vec<Arc<Mutex<Vec<String>>>>>,
        }
        impl DriverFactory for Factory {
            fn create(
                &self,
                _: &yang_protocol::DomainConfig,
            ) -> Result<Box<dyn DeviceSession>, WorkerError> {
                let mut entries = vec![("*IDN?", text("RIGOL,DG4202,GEN1,01"))];
                if self.created.fetch_add(1, Ordering::SeqCst) > 0 {
                    entries.push((":OUTPut1 OFF", vec![]));
                    entries.push((
                        ":OUTPut1:LOAD?",
                        if self.failed_read {
                            vec![]
                        } else {
                            text("INF")
                        },
                    ));
                    if !self.failed_read {
                        entries.extend([
                            (":SOURce1:FUNCtion?", text("SIN")),
                            (":SOURce1:FREQuency?", text("50")),
                            (":SOURce1:VOLTage:UNIT?", text("VPP")),
                            (":SOURce1:VOLTage?", text("2")),
                            (":SOURce1:VOLTage:OFFSet?", text("0")),
                            (":SOURce1:PHASe?", text("0")),
                            (":OUTPut1:POLarity?", text("NORM")),
                            (":OUTPut1?", text("OFF")),
                            (":OUTPut2:LOAD?", text("INF")),
                            (":SOURce2:FUNCtion?", text("SIN")),
                            (":SOURce2:FREQuency?", text("1000")),
                            (":SOURce2:VOLTage:UNIT?", text("VPP")),
                            (":SOURce2:VOLTage?", text("1")),
                            (":SOURce2:VOLTage:OFFSet?", text("0")),
                            (":SOURce2:PHASe?", text("0")),
                            (":OUTPut2:POLarity?", text("NORM")),
                            (":OUTPut2?", text("ON")),
                        ]);
                    }
                }
                let (io, writes) = transport(entries);
                self.writes.lock().unwrap().push(writes);
                let clock = Arc::new(ManualClock::default());
                Ok(Box::new(
                    RigolSession::from_generator(
                        Generator::with_transport(
                            GeneratorModel::Dg4202,
                            io,
                            clock.clone(),
                            GeneratorOptions::default(),
                        )
                        .unwrap(),
                        clock,
                    )
                    .unwrap(),
                ))
            }
        }
        struct Inventory;
        impl InventoryPort for Inventory {
            fn serial(
                &self,
            ) -> Result<Vec<yang_drivers::transport::serial_discovery::SerialDeviceInfo>, WorkerError>
            {
                panic!("No enumeration authorized")
            }
            fn visa(&self) -> Result<Vec<String>, WorkerError> {
                panic!("No enumeration authorized")
            }
        }
        let factory = Arc::new(Factory {
            created: AtomicUsize::new(0),
            failed_read,
            writes: Mutex::new(vec![]),
        });
        let clock = Arc::new(ManualClock::default());
        let backend = NativeBackend::with_ports(
            DomainRegistry::new(&yang_worker::new_id().unwrap()).unwrap(),
            clock.clone(),
            factory.clone(),
            Arc::new(Inventory),
        );
        let c = config("dg4202", "generator", "GPIB0::7::INSTR");
        let request = super::support::request;
        assert_eq!(
            backend
                .execute(&request(
                    "cfg",
                    "configure_domain",
                    json!({"config":c}),
                    Some(backend.global_context())
                ))
                .phase,
            yang_protocol::Phase::Completed
        );
        let ctx = backend.registry().context(&c.domain).unwrap();
        let probe = backend.execute(&request("probe", "probe", json!({"authorization":{"stage":"readonly","accepted":true,"supervised":false,"retain_session":false,"binding":{"mode":"real","domain":c.domain,"config_rev":1,"model_id":"dg4202","profile_id":"visa","config_digest":"a".repeat(64)}}}), Some(ctx)));
        assert_eq!(
            probe.phase,
            yang_protocol::Phase::Completed,
            "{:?}",
            probe.error
        );
        let proof = probe.result.unwrap()["proof"]["proof_id"].clone();
        assert_eq!(backend.execute(&request("reg", "register_verified", json!({"domain":c.domain,"proof_id":proof,"config_digest":"a".repeat(64),"config_rev":1}), Some(backend.global_context()))).phase, yang_protocol::Phase::Completed);
        let ctx = backend
            .registry()
            .bind(&c.domain, &yang_worker::new_id().unwrap())
            .unwrap();
        assert_eq!(
            backend
                .execute(&request("connect", "connect", json!({}), Some(ctx.clone())))
                .phase,
            yang_protocol::Phase::Completed
        );
        let writes = factory.writes.lock().unwrap()[1].clone();
        let scheduler =
            Scheduler::new(backend.clone(), clock, yang_protocol::Limits::default()).unwrap();
        let off = scheduler
            .submit(request(
                "off",
                "action",
                json!({"name":"set_output","args":{"channel":1,"enabled":false}}),
                Some(ctx),
            ))
            .unwrap()
            .wait(Deadline::after(std::time::Duration::from_secs(2)))
            .unwrap();
        assert_eq!(
            off.phase,
            yang_protocol::Phase::Completed,
            "{:?}",
            off.error
        );
        let held = backend.registry().context(&c.domain).unwrap();
        (scheduler, backend, held, writes)
    }

    #[test]
    fn generator_held_readback_is_admitted_and_resume_preserves_outputs() {
        use yang_worker::scheduler::Backend;
        let (scheduler, backend, context, writes) = held_generator(false);
        let request = super::support::request;
        let deadline = || Deadline::after(std::time::Duration::from_secs(2));
        let read = scheduler
            .submit(request(
                "held-read",
                "action",
                json!({"name":"read_status","args":{}}),
                Some(context.clone()),
            ))
            .unwrap()
            .wait(deadline())
            .unwrap();
        assert_eq!(
            read.phase,
            yang_protocol::Phase::Completed,
            "{:?}",
            read.error
        );
        assert_eq!(
            read.result.as_ref().unwrap()["status"]["channels"][0]["output_enabled"],
            false
        );
        assert_eq!(
            read.result.as_ref().unwrap()["status"]["channels"][1]["output_enabled"],
            true
        );
        let status = scheduler
            .submit(request("held-status", "status", json!({}), None))
            .unwrap()
            .wait(deadline())
            .unwrap()
            .result
            .unwrap();
        let key = format!("device:{}", context.domain.as_ref().unwrap().id);
        assert_eq!(status["domains"][&key]["state"], "STOP_HELD");
        assert_eq!(backend.registry().snapshot()[0].state, DriverState::Ready);
        let before_resume = writes.lock().unwrap().len();
        let resumed = scheduler
            .submit(request(
                "resume",
                "resume",
                json!({"confirm":true}),
                Some(context),
            ))
            .unwrap()
            .wait(deadline())
            .unwrap();
        assert_eq!(
            resumed.phase,
            yang_protocol::Phase::Completed,
            "{:?}",
            resumed.error
        );
        assert_eq!(
            writes.lock().unwrap().len(),
            before_resume,
            "resume must not send instrument commands"
        );
        assert_eq!(
            writes
                .lock()
                .unwrap()
                .iter()
                .filter(|q| !q.trim_end().ends_with('?'))
                .cloned()
                .collect::<Vec<_>>(),
            [":OUTPut1 OFF\n"]
        );
        scheduler.begin_shutdown();
        scheduler.join_when_released(deadline()).unwrap();
    }

    #[test]
    fn generator_failed_held_readback_never_resumes_or_replays_output() {
        let (scheduler, _backend, context, writes) = held_generator(true);
        let request = super::support::request;
        let deadline = || Deadline::after(std::time::Duration::from_secs(2));
        let read = scheduler
            .submit(request(
                "held-read",
                "action",
                json!({"name":"read_status","args":{}}),
                Some(context.clone()),
            ))
            .unwrap()
            .wait(deadline())
            .unwrap();
        assert_eq!(
            read.phase,
            yang_protocol::Phase::FailedAfterCallStarted,
            "{:?}",
            read.error
        );
        let resumed = scheduler
            .submit(request(
                "resume",
                "resume",
                json!({"confirm":true}),
                Some(context),
            ))
            .unwrap()
            .wait(deadline())
            .unwrap();
        assert_eq!(resumed.phase, yang_protocol::Phase::RejectedBeforeCall);
        assert_eq!(resumed.error.unwrap().kind, "ResumeRestricted");
        assert_eq!(
            writes.lock().unwrap().as_slice(),
            ["*IDN?\n", ":OUTPut1 OFF\n", ":OUTPut1:LOAD?\n"]
        );
        scheduler.begin_shutdown();
        scheduler.join_when_released(deadline()).unwrap();
    }

    #[test]
    fn generator_historical_held_cache_cannot_authorize_resume() {
        let (scheduler, _backend, context, writes) = held_generator(false);
        let request = super::support::request;
        let deadline = || Deadline::after(std::time::Duration::from_secs(2));
        let key = format!("device:{}", context.domain.as_ref().unwrap().id);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let status = scheduler
                .submit(request(
                    &yang_worker::new_id().unwrap(),
                    "status",
                    json!({}),
                    None,
                ))
                .unwrap()
                .wait(deadline())
                .unwrap()
                .result
                .unwrap();
            if status["devices"][&key]["previous"] == true
                && status["domains"][&key]["observing"] == false
            {
                break;
            }
            assert!(
                std::time::Instant::now() < until,
                "historical cache was not published"
            );
            std::thread::yield_now();
        }
        let resumed = scheduler
            .submit(request(
                "resume",
                "resume",
                json!({"confirm":true}),
                Some(context),
            ))
            .unwrap()
            .wait(deadline())
            .unwrap();
        assert_eq!(resumed.phase, yang_protocol::Phase::RejectedBeforeCall);
        assert_eq!(resumed.error.unwrap().kind, "ResumeRestricted");
        assert_eq!(
            writes.lock().unwrap().as_slice(),
            ["*IDN?\n", ":OUTPut1 OFF\n"]
        );
        scheduler.begin_shutdown();
        scheduler.join_when_released(deadline()).unwrap();
    }

    #[test]
    fn backend_waveform_reserves_before_io_and_storage_retry_never_reacquires() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use yang_worker::{
            backend::NativeBackend,
            captures::{CaptureSpool, Durability},
            discovery::InventoryPort,
            scheduler::Backend,
            DomainRegistry, WorkerError,
        };
        struct Factory {
            created: AtomicUsize,
            writes: Mutex<Vec<Arc<Mutex<Vec<String>>>>>,
        }
        impl DriverFactory for Factory {
            fn create(
                &self,
                _: &yang_protocol::DomainConfig,
            ) -> Result<Box<dyn DeviceSession>, WorkerError> {
                let mut entries = vec![("*IDN?", text("RIGOL,DS7054,SCOPE1,01"))];
                if self.created.fetch_add(1, Ordering::SeqCst) > 0 {
                    entries.extend(capture_entries(false, 128));
                } else {
                    entries.extend([
                        (":TRIGger:STATus?", text("STOP")),
                        (":ACQuire:MDEPth?", text("1K")),
                        (":WAVeform:SOURce?", text("CHAN1")),
                        (":WAVeform:MODE?", text("NORM")),
                        (":WAVeform:FORMat?", text("BYTE")),
                        (":WAVeform:POINts?", text("1000")),
                        (":WAVeform:STARt?", text("1")),
                        (":WAVeform:STOP?", text("1000")),
                        (
                            ":WAVeform:PREamble?",
                            text("0,0,1000,1,1e-9,0,0,0.01,10,128"),
                        ),
                        (":CHANnel1:DISPlay?", text("1")),
                        (":CHANnel2:DISPlay?", text("0")),
                        (":CHANnel3:DISPlay?", text("0")),
                        (":CHANnel4:DISPlay?", text("0")),
                    ]);
                }
                let (io, writes) = transport(entries);
                self.writes.lock().unwrap().push(writes);
                let clock = Arc::new(ManualClock::default());
                Ok(Box::new(
                    RigolSession::from_scope(
                        Scope::with_transport(
                            ScopeModel::Ds7054,
                            io,
                            clock.clone(),
                            ScopeOptions::default(),
                        )
                        .unwrap(),
                        clock,
                    )
                    .unwrap(),
                ))
            }
        }
        struct Inventory;
        impl InventoryPort for Inventory {
            fn serial(
                &self,
            ) -> Result<Vec<yang_drivers::transport::serial_discovery::SerialDeviceInfo>, WorkerError>
            {
                panic!("No enumeration authorized")
            }
            fn visa(&self) -> Result<Vec<String>, WorkerError> {
                panic!("No enumeration authorized")
            }
        }
        struct Disk(AtomicBool);
        impl Durability for Disk {
            fn sync(&self, file: &std::fs::File) -> std::io::Result<()> {
                if self.0.load(Ordering::SeqCst) {
                    Err(std::io::Error::other("injected disk full"))
                } else {
                    file.sync_all()
                }
            }
        }
        let fixture = super::support::Fixture::new();
        let disk = Arc::new(Disk(AtomicBool::new(true)));
        let spool = Arc::new(Mutex::new(
            CaptureSpool::with_durability(
                fixture.root.clone(),
                fixture.nonce.clone(),
                disk.clone(),
            )
            .unwrap(),
        ));
        let factory = Arc::new(Factory {
            created: AtomicUsize::new(0),
            writes: Mutex::new(vec![]),
        });
        let backend = NativeBackend::with_ports(
            DomainRegistry::new(&yang_worker::new_id().unwrap()).unwrap(),
            Arc::new(ManualClock::default()),
            factory.clone(),
            Arc::new(Inventory),
        );
        backend.install_spool(spool.clone());
        let c = config("ds7054", "scope", "GPIB0::4::INSTR");
        let request = super::support::request;
        assert_eq!(
            backend
                .execute(&request(
                    "cfg",
                    "configure_domain",
                    json!({"config":c}),
                    Some(backend.global_context())
                ))
                .phase,
            yang_protocol::Phase::Completed
        );
        let ctx = backend.registry().context(&c.domain).unwrap();
        let proof=backend.execute(&request("probe","probe",json!({"authorization":{"stage":"readonly","accepted":true,"supervised":false,"retain_session":false,"binding":{"mode":"real","domain":c.domain,"config_rev":1,"model_id":"ds7054","profile_id":"visa","config_digest":"a".repeat(64)}}}),Some(ctx)));
        assert_eq!(
            proof.phase,
            yang_protocol::Phase::Completed,
            "{:?}",
            proof.error
        );
        let proof = proof.result.unwrap()["proof"]["proof_id"].clone();
        assert_eq!(backend.execute(&request("reg","register_verified",json!({"domain":c.domain,"proof_id":proof,"config_digest":"a".repeat(64),"config_rev":1}),Some(backend.global_context()))).phase,yang_protocol::Phase::Completed);
        let ctx = backend
            .registry()
            .bind(&c.domain, &yang_worker::new_id().unwrap())
            .unwrap();
        assert_eq!(
            backend
                .execute(&request("connect", "connect", json!({}), Some(ctx.clone())))
                .phase,
            yang_protocol::Phase::Completed
        );
        let ids = (0..32)
            .map(|_| yang_worker::new_id().unwrap())
            .collect::<Vec<_>>();
        for id in &ids {
            spool.lock().unwrap().reserve_read(id, 1).unwrap();
        }
        let args = json!({"name":"capture","args":{"channels":[1],"source":"memory","points":2,"byte_budget":2}});
        let denied = backend.execute(&request(
            "denied",
            "action",
            args.clone(),
            Some(ctx.clone()),
        ));
        assert_eq!(denied.error.unwrap().kind, "CaptureCapacity");
        assert_eq!(
            factory.writes.lock().unwrap()[1].lock().unwrap().len(),
            1,
            "budget rejection must happen before any instrument query"
        );
        for id in &ids {
            spool.lock().unwrap().release_read(id);
        }
        let capture = backend.execute(&request(
            "original-capture",
            "action",
            args.clone(),
            Some(ctx.clone()),
        ));
        assert_eq!(capture.error.unwrap().kind, "CaptureStagingFailed");
        let calls = factory.writes.lock().unwrap()[1].lock().unwrap().len();
        let ticket = backend.observe(&ctx).status["unstaged_capture"]["capture_id"].clone();
        assert!(ticket.as_str().is_some());
        assert_eq!(
            backend
                .execute(&request("duplicate", "action", args, Some(ctx.clone())))
                .error
                .unwrap()
                .kind,
            "CaptureUnstaged"
        );
        let closed = backend.execute(&request(
            "close",
            "disconnect",
            json!({}),
            Some(ctx.clone()),
        ));
        assert_eq!(closed.result.unwrap()["data_retained"], true);
        disk.0.store(false, Ordering::SeqCst);
        let recovered = backend.execute(&request(
            "storage-only",
            "action",
            json!({"name":"retry_staging","args":{"capture_id":ticket}}),
            Some(ctx.clone()),
        ));
        assert_eq!(
            recovered.phase,
            yang_protocol::Phase::Completed,
            "{:?}",
            recovered.error
        );
        let result = recovered.result.unwrap();
        assert_eq!(result["original_request_id"], "original-capture");
        assert_eq!(result["capture"]["kind"], "waveform");
        assert_eq!(result["capture"]["metadata"]["source"]["model"], "ds7054");
        assert_eq!(
            factory.writes.lock().unwrap()[1].lock().unwrap().len(),
            calls,
            "storage-only retry must not enter the instrument"
        );
        let descriptor = result["capture"].clone();
        let chunk=backend.execute(&request("chunk","read_capture_chunk",json!({"ownership_nonce":fixture.nonce,"capture_id":descriptor["capture_id"],"offset":0,"length":2}),Some(backend.global_context())));
        assert_eq!(chunk.result.unwrap()["data_hex"], "80ff");
        assert_eq!(backend.execute(&request("ack","ack_capture",json!({"ownership_nonce":fixture.nonce,"capture_id":descriptor["capture_id"],"sha256":descriptor["sha256"]}),Some(backend.global_context()))).phase,yang_protocol::Phase::Completed);
        backend.execute(&request("release", "disconnect", json!({}), Some(ctx)));
    }
}
