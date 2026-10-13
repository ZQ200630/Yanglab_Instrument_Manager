//! Private offline finite byte transcript. This is not a selectable backend.
//! Only worker-fixture's explicit `rigol` profile imports this module.
use std::{collections::VecDeque, sync::Arc};
use yang_drivers::{
    clock::ManualClock,
    generator::{Generator, GeneratorModel, GeneratorOptions},
    scope::{Scope, ScopeModel, ScopeOptions},
    transport::{ByteTransport, CloseReport, Deadline},
    DriverError, DriverResult,
};
use yang_worker::{rigol_sessions::RigolSession, session::DeviceSession, WorkerError};

pub fn session(
    config: &yang_protocol::DomainConfig,
) -> Result<Box<dyn DeviceSession>, WorkerError> {
    let scope = match (
        config.driver_kind.as_str(),
        config.model_id.as_str(),
        config.params["resource"].as_str(),
    ) {
        ("scope", "ds7054", Some("GPIB0::7::INSTR")) => true,
        ("generator", "dg4162", Some("GPIB0::8::INSTR")) => false,
        _ => {
            return Err(WorkerError::new(
                "Fixture",
                "Unreviewed finite Rigol model/address",
            ))
        }
    };
    let clock = Arc::new(ManualClock::default());
    let wire = Box::new(Wire::new(scope));
    Ok(Box::new(if scope {
        RigolSession::from_scope(
            Scope::with_transport(
                ScopeModel::Ds7054,
                wire,
                clock.clone(),
                ScopeOptions::default(),
            )?,
            clock,
        )?
    } else {
        RigolSession::from_generator(
            Generator::with_transport(
                GeneratorModel::Dg4162,
                wire,
                clock.clone(),
                GeneratorOptions::default(),
            )?,
            clock,
        )?
    }))
}

struct Wire {
    scope: bool,
    pending: VecDeque<u8>,
    live: bool,
    writes: usize,
    single_poll: u8,
    running: bool,
    snapshot_ready: bool,
    completed_frames: u8,
    emitted_frames: u8,
    waveform_step: usize,
    waveform_screen: bool,
    existing_buffer: bool,
    generator_configured: bool,
    output: [bool; 2],
}
impl Wire {
    fn new(scope: bool) -> Self {
        Self {
            scope,
            pending: VecDeque::new(),
            live: true,
            writes: 0,
            single_poll: 0,
            running: false,
            snapshot_ready: false,
            completed_frames: 0,
            emitted_frames: 0,
            waveform_step: 0,
            waveform_screen: false,
            existing_buffer: false,
            generator_configured: false,
            output: [false, true],
        }
    }
    fn fail(message: &str) -> DriverError {
        DriverError::Protocol(format!("finite Rigol transcript: {message}"))
    }
    fn scope_reply(&mut self, command: &str) -> DriverResult<Vec<u8>> {
        if self.waveform_step == 0 {
            let reply = match command {
                ":WAVeform:SOURce?" => Some("CHAN1"),
                ":WAVeform:MODE?" => Some(if self.waveform_screen { "NORM" } else { "RAW" }),
                ":WAVeform:FORMat?" => Some("BYTE"),
                ":WAVeform:POINts?" | ":WAVeform:STOP?" => Some("1000"),
                ":WAVeform:STARt?" => Some("1"),
                ":WAVeform:PREamble?" => Some(if self.waveform_screen {
                    "0,0,1000,1,1e-9,0,0,0.01,10,128"
                } else {
                    "0,2,1000,1,1e-9,0,0,0.01,10,128"
                }),
                _ => None,
            };
            if let Some(reply) = reply {
                return Ok(format!("{reply}\n").into_bytes());
            }
        }
        let waveform = [
            ":WAVeform:SOURce CHANnel1",
            ":WAVeform:MODE RAW",
            ":WAVeform:FORMat BYTE",
            ":WAVeform:STARt 1",
            ":WAVeform:POINts 1000",
            ":WAVeform:STOP 1000",
            ":WAVeform:PREamble?",
            ":WAVeform:STARt 1",
            ":WAVeform:STOP 1000",
            ":WAVeform:DATA?",
        ];
        if command.starts_with(":WAVeform:") {
            if self.waveform_step == 0 {
                self.existing_buffer = self.completed_frames == self.emitted_frames;
            }
            let screen_mode = self.waveform_step == 1 && command == ":WAVeform:MODE NORMal";
            if (waveform.get(self.waveform_step) != Some(&command) && !screen_mode)
                || (self.waveform_step == 1 && self.existing_buffer && !screen_mode && !self.snapshot_ready)
            {
                return Err(Self::fail("out-of-order waveform transfer or unfrozen memory"));
            }
            if self.waveform_step == 1 {
                self.waveform_screen = screen_mode;
            }
            self.waveform_step += 1;
            if command == ":WAVeform:PREamble?" {
                return Ok(if self.waveform_screen {
                    b"0,0,1000,1,1e-9,0,0,0.01,10,128\n".to_vec()
                } else {
                    b"0,2,1000,1,1e-9,0,0,0.01,10,128\n".to_vec()
                });
            }
            if command == ":WAVeform:DATA?" {
                let mut reply = b"#41000".to_vec();
                // STOP freezes the current bounded fixture buffer. It does not
                // invent a fresh acquisition or change its sample contents.
                let sample = 127 + self.completed_frames.max(1);
                reply.extend(std::iter::repeat_n(sample, 999));
                reply.extend([255, b'\n']);
                if !self.existing_buffer { self.emitted_frames += 1; }
                self.waveform_step = 0;
                return Ok(reply);
            }
            return Ok(vec![]);
        }
        Ok(match command {
            "*IDN?" => b"RIGOL,DS7054,HOST-SCOPE-7,1.0\n".to_vec(),
            ":ACQuire:MDEPth?" => b"1K\n".to_vec(),
            ":CHANnel1:DISPlay?" => b"1\n".to_vec(),
            ":CHANnel2:DISPlay?" | ":CHANnel3:DISPlay?" | ":CHANnel4:DISPlay?" => b"0\n".to_vec(),
            ":ACQuire:MDEPth 1000" | ":CHANnel1:DISPlay ON" => vec![],
            ":STOP" => {
                self.single_poll = 0;
                self.running = false;
                self.snapshot_ready = true;
                vec![]
            }
            ":RUN" => {
                self.single_poll = 0;
                self.running = true;
                self.snapshot_ready = false;
                vec![]
            }
            ":SINGle" if self.single_poll == 0 && self.completed_frames < 4 => {
                self.single_poll = 1;
                self.running = true;
                self.snapshot_ready = false;
                vec![]
            }
            ":TRIGger:STATus?" => match self.single_poll {
                1 => {
                    self.single_poll = 2;
                    b"WAIT\n".to_vec()
                }
                2 => {
                    self.single_poll = 0;
                    self.completed_frames += 1;
                    self.running = false;
                    b"STOP\n".to_vec()
                }
                _ if self.running => b"RUN\n".to_vec(),
                _ => b"STOP\n".to_vec(),
            },
            _ => return Err(Self::fail("unreviewed scope command")),
        })
    }
    fn generator_reply(&mut self, command: &str) -> DriverResult<Vec<u8>> {
        if matches!(command,":COUPling:AMPL:STATe?"|":COUPling:FREQuency:STATe?"|":COUPling:PHASe:STATe?") {return Ok(b"OFF\n".to_vec());}
        if command == "*IDN?" {
            return Ok(b"RIGOL,DG4162,HOST-GENERATOR-8,1.0\n".to_vec());
        }
        for channel in 1..=2 {
            let source = format!(":SOURce{channel}:");
            let output = format!(":OUTPut{channel}");
            let text = if ["MOD:STATe?","SWEep:STATe?","BURSt:STATe?"].iter().any(|suffix|command==format!("{source}{suffix}")) {"OFF"} else if command == format!("{output}:LOAD?") {
                "INF"
            } else if command == format!("{output}:POLarity?") {
                "NORM"
            } else if command == format!("{output}?") {
                if self.output[channel - 1] {
                    "1"
                } else {
                    "0"
                }
            } else if command == format!("{source}FUNCtion?") {
                if self.generator_configured && channel == 1 {
                    "RAMP"
                } else {
                    "SIN"
                }
            } else if command == format!("{source}FUNCtion:RAMP:SYMMetry?") {
                "50"
            } else if command == format!("{source}FREQuency?") {
                "1000"
            } else if command == format!("{source}VOLTage:UNIT?") {
                "VPP"
            } else if command == format!("{source}VOLTage?") {
                if self.generator_configured && channel == 1 {
                    "2"
                } else {
                    "1"
                }
            } else if command == format!("{source}VOLTage:OFFSet?")
                || command == format!("{source}PHASe?")
            {
                "0"
            } else if command == format!("{output} OFF") {
                self.output[channel - 1] = false;
                return Ok(vec![]);
            } else if channel == 1 && command == format!("{output} ON") {
                self.output[0] = true;
                return Ok(vec![]);
            } else {
                continue;
            };
            return Ok(format!("{text}\n").into_bytes());
        }
        match command {
            ":SOURce1:VOLTage 2"
            | ":SOURce1:VOLTage:OFFSet 0"
            | ":SOURce1:FREQuency 1000"
            | ":SOURce1:FUNCtion:RAMP:SYMMetry 50" => Ok(vec![]),
            ":SOURce1:FUNCtion RAMP" => {
                self.generator_configured = true;
                Ok(vec![])
            }
            _ => Err(Self::fail("unreviewed generator command")),
        }
    }
}
impl ByteTransport for Wire {
    fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<()> {
        deadline.remaining_millis()?;
        if !self.live || !self.pending.is_empty() || self.writes >= 256 || bytes.len() > 128 {
            return Err(Self::fail("write budget/lifecycle boundary"));
        }
        let command = std::str::from_utf8(bytes)
            .map_err(|_| Self::fail("nonASCII command"))?
            .strip_suffix('\n')
            .ok_or_else(|| Self::fail("missing termination"))?;
        self.writes += 1;
        self.pending = if self.scope {
            self.scope_reply(command)?
        } else {
            self.generator_reply(command)?
        }
        .into();
        Ok(())
    }
    fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
        deadline.remaining_millis()?;
        if !self.live || maximum == 0 || maximum > 16384 || self.pending.is_empty() {
            return Err(Self::fail("read bound/truncated reply"));
        }
        Ok(self
            .pending
            .drain(..maximum.min(127).min(self.pending.len()))
            .collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_wire_rejects_reset_unreviewed_values_and_fifth_frame() {
        for command in [
            "*RST\n",
            "*CLS\n",
            ":ACQuire:MDEPth 2\n",
            ":WAVeform:DATA?\n",
        ] {
            assert!(Wire::new(true)
                .write_all(
                    command.as_bytes(),
                    Deadline::after(std::time::Duration::from_secs(1))
                )
                .is_err());
        }
        let mut wire = Wire::new(true);
        wire.completed_frames = 4;
        assert!(wire
            .write_all(
                b":SINGle\n",
                Deadline::after(std::time::Duration::from_secs(1))
            )
            .is_err());
        let mut wire = Wire::new(false);
        assert!(wire
            .write_all(
                b":SOURce1:VOLTage 3\n",
                Deadline::after(std::time::Duration::from_secs(1))
            )
            .is_err());
        assert!(wire.close().unwrap().released);
        assert!(wire
            .write_all(
                b"*IDN?\n",
                Deadline::after(std::time::Duration::from_secs(1))
            )
            .is_err());
    }
    #[test]
    fn snapshot_fixture_retains_existing_buffer_across_explicit_run_stop_cycles() {
        let config = yang_protocol::DomainConfig {
            domain: yang_protocol::DomainRef { kind: "device".into(), id: "a".repeat(32) },
            config_rev: 1, driver_kind: "scope".into(), model_id: "ds7054".into(),
            profile_id: Some("visa".into()), params: serde_json::json!({"resource":"GPIB0::7::INSTR"}),
            expected_identity: serde_json::json!({}), members: vec![],
        };
        let context = yang_protocol::ContextV3 {
            session_id: "b".repeat(32), domain: Some(config.domain.clone()),
            connection_id: Some("c".repeat(32)), epoch: 1,
        };
        let mut scope = session(&config).unwrap();
        scope.connect().unwrap();
        for _ in 0..2 {
            let result = scope.action("capture", &serde_json::json!({
                "channels":[1],"source":"memory","points":1000,"byte_budget":1000,"fresh":false,"stop_before":true
            }), &context);
            assert_eq!(result.phase, yang_protocol::Phase::Completed, "{:?}", result.error);
            let frame = scope.take_waveform().unwrap();
            assert_eq!(frame.channels[0].samples.value(0), Some(128.));
            assert_eq!(frame.channels[0].metadata["snapshot"]["fresh_acquisition"], false);
            assert_eq!(scope.action("run", &serde_json::json!({}), &context).phase, yang_protocol::Phase::Completed);
            let status = scope.action("read_status", &serde_json::json!({}), &context);
            assert_eq!(status.result.unwrap()["status"]["running"], true);
        }
        scope.close().unwrap();
    }
}
