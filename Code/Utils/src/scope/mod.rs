//! Rigol scope adapters. Connect and normal close preserve acquisition and outputs.
//! Capture changes only waveform-transfer source/mode/format/range. Memory capture
//! requires an explicitly stopped scope; it never issues STOP or RUN implicitly.
//! Explicit snapshot acquisition sends STOP and transfers the confirmed frozen
//! buffer without triggering a new acquisition or restarting the scope.
pub use crate::scpi::StopHandle;
use crate::{
    clock::Clock,
    lifecycle::{CleanupReport, DriverLifecycle, DriverState, ProbeReport},
    scpi::{self, Session},
    transport::{ByteTransport, Deadline, VisaManager},
    DriverError, DriverResult,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use yang_data::{Calibration, ChannelWaveform, Samples, WaveformFrame};

/// Whole-capture ceiling, separate from the configured per-I/O stall timeout.
/// This is a hard software budget, not a completion-time estimate.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(120);
/// Whole depth-application ceiling, independent of the per-I/O stall timeout.
/// This is a bounded confirmation budget, not an expected hardware latency.
pub const DEPTH_APPLICATION_TIMEOUT: Duration = Duration::from_secs(30);
/// Conservative DS7054 request window; native reads remain bounded at 16 KiB.
/// This software policy does not assert a manufacturer maximum.
pub const DS7054_WINDOW_POINTS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeModel {
    Ds7054,
    Mho5054,
}
impl ScopeModel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ds7054 => "DS7054",
            Self::Mho5054 => "MHO5054",
        }
    }
    pub fn sample_bytes(self) -> usize {
        match self {
            Self::Ds7054 => 1,
            Self::Mho5054 => 2,
        }
    }
    fn memory_depths(self) -> &'static [u64] {
        match self {
            Self::Ds7054 => &[
                1_000, 10_000, 100_000, 1_000_000, 10_000_000, 25_000_000,
                50_000_000, 100_000_000, 125_000_000, 250_000_000,
            ],
            Self::Mho5054 => &[
                1_000, 10_000, 100_000, 1_000_000, 10_000_000, 25_000_000,
                50_000_000, 100_000_000, 125_000_000, 200_000_000, 250_000_000,
            ],
        }
    }
}
/// Pure model admission, shared by explicit depth changes and Host preflight.
pub fn validate_memory_depth(model: ScopeModel, points: u64) -> DriverResult<()> {
    if model.memory_depths().contains(&points) {
        Ok(())
    } else {
        Err(DriverError::Invalid("unsupported scope memory depth".into()))
    }
}
#[derive(Clone, Debug)]
pub struct ScopeOptions {
    pub timeout: Duration,
    pub depth_timeout: Duration,
    pub close_timeout: Duration,
    pub max_capture_bytes: usize,
    pub chunk_points: usize,
}
impl Default for ScopeOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            depth_timeout: DEPTH_APPLICATION_TIMEOUT,
            close_timeout: Duration::from_secs(2),
            max_capture_bytes: 512 * 1024 * 1024,
            chunk_points: 8184,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureSource {
    Screen,
    Memory,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRequest {
    pub channels: Vec<u8>,
    pub source: CaptureSource,
    pub points: Option<u64>,
    pub byte_budget: usize,
}
/// Pure admission shared by drivers and diagnostic preflight. An omitted Screen
/// point count means 1000; an omitted Memory count is checked again once known.
pub fn validate_capture_request(
    model: ScopeModel,
    request: &CaptureRequest,
    max_capture_bytes: usize,
) -> DriverResult<()> {
    if request.channels.is_empty()
        || request.channels.len() > 4
        || request.byte_budget == 0
        || request
            .channels
            .iter()
            .any(|channel| !(1..=4).contains(channel))
        || request
            .channels
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != request.channels.len()
    {
        return Err(DriverError::Invalid(
            "capture requires distinct channels 1..4 and nonzero byte budget".into(),
        ));
    }
    admit_capture_points(
        model,
        request,
        request
            .points
            .unwrap_or(if request.source == CaptureSource::Screen {
                1000
            } else {
                1
            }),
        max_capture_bytes,
    )?;
    Ok(())
}
fn admit_capture_points(
    model: ScopeModel,
    request: &CaptureRequest,
    points: u64,
    max_capture_bytes: usize,
) -> DriverResult<usize> {
    let maximum = if request.source == CaptureSource::Screen {
        1000
    } else {
        250_000_000
    };
    if points == 0 || points > maximum {
        return Err(DriverError::Invalid(format!(
            "requested points exceed {maximum} for capture source"
        )));
    }
    let points = usize::try_from(points)
        .map_err(|_| DriverError::Invalid("point count does not fit address space".into()))?;
    let total = points
        .checked_mul(model.sample_bytes())
        .and_then(|n| n.checked_mul(request.channels.len()))
        .ok_or_else(|| DriverError::Invalid("capture byte count overflow".into()))?;
    if total > request.byte_budget || total > max_capture_bytes {
        return Err(DriverError::Invalid(format!(
            "capture requires {total} bytes, beyond admitted budget"
        )));
    }
    Ok(points)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScopeCapabilities {
    pub channels: u8,
    pub memory_depths: Vec<u64>,
    pub max_points: u64,
    pub sample_bytes: usize,
    pub screen: bool,
    pub memory: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcquisitionState {
    Running,
    Stopped,
    Waiting,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScopeChannelStatus {
    pub channel: u8,
    pub displayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScopeStatus {
    pub model: ScopeModel,
    pub identity: String,
    pub acquisition: AcquisitionState,
    pub running: bool,
    pub memory_depth: u64,
    pub memory_depth_auto: bool,
    pub channels: Vec<ScopeChannelStatus>,
    pub capabilities: ScopeCapabilities,
}
fn memory_depth(text: &str) -> DriverResult<Option<u64>> {
    let text = text.trim().to_ascii_uppercase();
    if text == "AUTO" {
        return Ok(None);
    }
    let (value, multiplier) = if let Some(value) = text.strip_suffix('K') {
        (value, 1000.0)
    } else if let Some(value) = text.strip_suffix('M') {
        (value, 1_000_000.0)
    } else {
        (text.as_str(), 1.0)
    };
    let value = scpi::number(value)? * multiplier;
    if !value.is_finite() || value <= 0.0 || value.fract() != 0.0 || value > 500_000_000.0 {
        return Err(DriverError::Protocol("invalid scope memory depth".into()));
    }
    Ok(Some(value as u64))
}
#[derive(Clone, Debug)]
pub struct Preamble {
    pub format: u8,
    pub acquisition_type: u8,
    pub points: u64,
    pub count: u64,
    pub calibration: Calibration,
    pub original: String,
}
pub fn parse_preamble(text: &str, model: ScopeModel) -> DriverResult<Preamble> {
    let fields: Vec<_> = text.split(',').map(str::trim).collect();
    if fields.len() != 10 {
        return Err(DriverError::Protocol(
            "scope preamble must contain ten fields".into(),
        ));
    }
    let format = scpi::integer(fields[0])?;
    let acquisition_type = scpi::integer(fields[1])?;
    let points = scpi::integer(fields[2])?;
    let count = scpi::integer(fields[3])?;
    if format != if model == ScopeModel::Ds7054 { 0 } else { 1 }
        || acquisition_type > 2
        || points == 0
        || points > 250_000_000
        || count == 0
    {
        return Err(DriverError::Protocol(
            "scope preamble representation/count mismatch".into(),
        ));
    }
    let calibration = Calibration {
        x_increment_s: scpi::number(fields[4])?,
        x_origin_s: scpi::number(fields[5])?,
        x_reference: scpi::number(fields[6])?,
        y_increment_v: scpi::number(fields[7])?,
        y_origin: -scpi::number(fields[8])? * scpi::number(fields[7])?,
        y_reference: scpi::number(fields[9])?,
    };
    if calibration.x_increment_s <= 0.0
        || calibration.y_increment_v <= 0.0
        || !calibration.y_origin.is_finite()
    {
        return Err(DriverError::Protocol("invalid scope calibration".into()));
    }
    Ok(Preamble {
        format: format as u8,
        acquisition_type: acquisition_type as u8,
        points,
        count,
        calibration,
        original: text.into(),
    })
}
/// Strict bounded decoder, also useful for protocol validation without a device.
pub fn decode_block(block: &[u8], model: ScopeModel, points: usize) -> DriverResult<Samples> {
    let invalid = || DriverError::Protocol("malformed, oversized or truncated scope block".into());
    let expected = points
        .checked_mul(model.sample_bytes())
        .ok_or_else(invalid)?;
    if expected == 0
        || expected > scpi::TRANSFER_MAX - 14
        || block.len() > scpi::TRANSFER_MAX
        || block.first() != Some(&b'#')
        || !matches!(block.get(1), Some(b'1'..=b'9'))
    {
        return Err(invalid());
    }
    let header = 2 + usize::from(block[1] - b'0');
    let digits = block
        .get(2..header)
        .filter(|v| v.iter().all(u8::is_ascii_digit))
        .ok_or_else(invalid)?;
    let declared = std::str::from_utf8(digits)
        .map_err(|_| invalid())?
        .parse::<usize>()
        .map_err(|_| invalid())?;
    if declared != expected {
        return Err(invalid());
    }
    let end = header.checked_add(expected).ok_or_else(invalid)?;
    let body = block.get(header..end).ok_or_else(invalid)?;
    if !matches!(block.get(end..), Some(b"" | b"\n" | b"\r\n")) {
        return Err(invalid());
    }
    Ok(decode_samples(body, model))
}
fn decode_samples(body: &[u8], model: ScopeModel) -> Samples {
    match model {
        ScopeModel::Ds7054 => Samples::U8(body.to_vec()),
        ScopeModel::Mho5054 => Samples::U16(
            body.chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect(),
        ),
    }
}
pub struct Scope {
    model: ScopeModel,
    session: Session,
    options: ScopeOptions,
}
impl Scope {
    pub fn new(
        model: ScopeModel,
        manager: VisaManager,
        resource: impl Into<String>,
        clock: Arc<dyn Clock>,
        options: ScopeOptions,
    ) -> DriverResult<Self> {
        Self::build(model, Some(manager), resource.into(), None, clock, options)
    }
    /// Transport injection is for bounded native protocol tests, never a catalog backend.
    pub fn with_transport(
        model: ScopeModel,
        transport: Box<dyn ByteTransport>,
        clock: Arc<dyn Clock>,
        options: ScopeOptions,
    ) -> DriverResult<Self> {
        Self::build(model, None, String::new(), Some(transport), clock, options)
    }
    fn build(
        model: ScopeModel,
        manager: Option<VisaManager>,
        resource: String,
        io: Option<Box<dyn ByteTransport>>,
        clock: Arc<dyn Clock>,
        options: ScopeOptions,
    ) -> DriverResult<Self> {
        if options.depth_timeout.is_zero() || options.depth_timeout > DEPTH_APPLICATION_TIMEOUT {
            return Err(DriverError::Invalid("scope depth confirmation budget must be positive and at most 30 s".into()));
        }
        if options.max_capture_bytes == 0
            || options.chunk_points == 0
            || options
                .chunk_points
                .checked_mul(model.sample_bytes())
                .is_none_or(|n| n > match model {
                    ScopeModel::Ds7054 => DS7054_WINDOW_POINTS,
                    ScopeModel::Mho5054 => scpi::TRANSFER_MAX - 14,
                })
        {
            return Err(DriverError::Invalid("scope byte budget/chunk bound".into()));
        }
        Ok(Self {
            model,
            session: Session::new(
                manager,
                resource,
                io,
                clock,
                options.timeout,
                options.close_timeout,
            )?,
            options,
        })
    }
    pub fn connect(&mut self) -> DriverResult<ProbeReport> {
        self.session.connect(self.model.name())
    }
    pub fn model(&self) -> ScopeModel {
        self.model
    }
    /// DS7054 diagnostic evidence only. Preserve current transfer settings and
    /// return their raw text; the selected format need not be capture-compatible.
    pub fn inspect_transfer_state_readonly(&mut self, deadline: Deadline) -> DriverResult<serde_json::Value> {
        if self.model != ScopeModel::Ds7054 {
            return Err(DriverError::Invalid("transfer-state inspection is reviewed only for DS7054".into()));
        }
        self.session.ready(deadline)?;
        let mut evidence = serde_json::Map::new();
        evidence.insert("model".into(), serde_json::json!(self.model.name()));
        for (field, command) in [
            ("trigger_status", ":TRIGger:STATus?"),
            ("acquisition_memory_depth", ":ACQuire:MDEPth?"),
            ("waveform_source", ":WAVeform:SOURce?"),
            ("waveform_mode", ":WAVeform:MODE?"),
            ("waveform_format", ":WAVeform:FORMat?"),
            ("waveform_points", ":WAVeform:POINts?"),
            ("waveform_start", ":WAVeform:STARt?"),
            ("waveform_stop", ":WAVeform:STOP?"),
            ("waveform_preamble", ":WAVeform:PREamble?"),
            ("channel_1_display", ":CHANnel1:DISPlay?"),
            ("channel_2_display", ":CHANnel2:DISPlay?"),
            ("channel_3_display", ":CHANnel3:DISPlay?"),
            ("channel_4_display", ":CHANnel4:DISPlay?"),
        ] {
            let text = self.session.query_text(command, deadline).map_err(|error| match error {
                DriverError::Protocol(message) => DriverError::Protocol(format!(
                    "scope transfer-state query {command}: {message}"
                )),
                error => error,
            })?;
            evidence.insert(field.into(), serde_json::Value::String(text));
        }
        Ok(serde_json::Value::Object(evidence))
    }
    pub fn capabilities(&self) -> ScopeCapabilities {
        ScopeCapabilities {
            channels: 4,
            memory_depths: self.model.memory_depths().to_vec(),
            max_points: 250_000_000,
            sample_bytes: self.model.sample_bytes(),
            screen: true,
            memory: true,
        }
    }
    fn channel(&self, channel: u8) -> DriverResult<()> {
        if (1..=4).contains(&channel) {
            Ok(())
        } else {
            Err(DriverError::Invalid(
                "scope channel must be 1 through 4".into(),
            ))
        }
    }
    fn acquisition(&mut self, deadline: Deadline) -> DriverResult<AcquisitionState> {
        match self
            .session
            .query_text(":TRIGger:STATus?", deadline)?
            .to_ascii_uppercase()
            .as_str()
        {
            "STOP" => Ok(AcquisitionState::Stopped),
            "WAIT" => Ok(AcquisitionState::Waiting),
            "RUN" | "TD" | "AUTO" => Ok(AcquisitionState::Running),
            _ => Err(DriverError::Protocol("unknown scope trigger status".into())),
        }
    }
    pub fn read_status(&mut self, deadline: Deadline) -> DriverResult<ScopeStatus> {
        self.session.ready(deadline)?;
        let acquisition = self.acquisition(deadline)?;
        let depth = memory_depth(&self.session.query_text(":ACQuire:MDEPth?", deadline)?)?;
        let memory_depth = depth.unwrap_or(0);
        if memory_depth > self.capabilities().max_points {
            return Err(DriverError::Protocol(
                "scope memory depth exceeds admitted model range".into(),
            ));
        }
        let mut channels = vec![];
        for channel in 1..=4 {
            let displayed = scpi::boolean(
                &self
                    .session
                    .query_text(&format!(":CHANnel{channel}:DISPlay?"), deadline)?,
            )?;
            channels.push(ScopeChannelStatus { channel, displayed });
        }
        Ok(ScopeStatus {
            model: self.model,
            identity: self.identity().into(),
            running: acquisition != AcquisitionState::Stopped,
            acquisition,
            memory_depth,
            memory_depth_auto: depth.is_none(),
            channels,
            capabilities: self.capabilities(),
        })
    }
    pub fn set_channel_display(
        &mut self,
        channel: u8,
        enabled: bool,
        deadline: Deadline,
    ) -> DriverResult<()> {
        self.channel(channel)?;
        self.session.ready(deadline)?;
        self.session.write(
            &format!(
                ":CHANnel{channel}:DISPlay {}",
                if enabled { "ON" } else { "OFF" }
            ),
            deadline,
        )
    }
    /// Explicit depth application. A stopped DS7054 briefly enters RUN so the
    /// setting can apply, replacing its old frozen buffer, then returns to a
    /// confirmed STOP with exact depth readback. This is never connection work.
    /// Other models receive no automatic acquisition-state workaround. Failed
    /// application never replays RUN/depth and leaves the driver faulted; a
    /// temporary RUN owns at most one bounded STOP cleanup attempt.
    pub fn set_memory_depth(&mut self, points: u64, deadline: Deadline) -> DriverResult<()> {
        validate_memory_depth(self.model, points)?;
        let deadline = deadline.earlier(Deadline::after(self.options.depth_timeout));
        self.session.ready(deadline)?;
        let started = self.session.clock.now();
        let mut temporary_run = false;
        let mut stop_attempted = false;
        let mut last_reported = None;
        let mut phase = "memory depth preflight";
        let result = (|| {
            let stopped = self.acquisition(deadline)? == AcquisitionState::Stopped;
            let initial = memory_depth(&self.session.query_text(":ACQuire:MDEPth?", deadline)?)?;
            last_reported = Some(initial);
            if initial == Some(points) {
                return Ok(());
            }
            if stopped && self.model == ScopeModel::Ds7054 {
                // Set ownership before I/O because a failed write is uncertain.
                temporary_run = true;
                phase = "depth acquisition-state confirmation";
                self.session.write(":RUN", deadline)?;
                self.confirm_acquisition_state(false, started, self.options.depth_timeout, deadline)?;
            }
            phase = "memory depth application";
            self.session.write(&format!(":ACQuire:MDEPth {points}"), deadline)?;
            loop {
                self.session.ready(deadline)?;
                let actual = memory_depth(&self.session.query_text(":ACQuire:MDEPth?", deadline)?)?;
                last_reported = Some(actual);
                if actual == Some(points) {
                    break;
                }
                self.wait_for_depth_confirmation(started, self.options.depth_timeout,
                    "memory depth application")?;
            }
            if temporary_run {
                // Restoration has its own bounded budget: application readback
                // may finish at its deadline, but still owns this sole STOP.
                let stop_deadline = Deadline::after(self.options.close_timeout);
                let stop_started = self.session.clock.now();
                stop_attempted = true;
                phase = "depth STOP restoration";
                self.session.write_explicit_disable(":STOP", stop_deadline)?;
                self.confirm_acquisition_state(true, stop_started, self.options.close_timeout, stop_deadline)?;
                let actual = memory_depth(&self.session.query_text(":ACQuire:MDEPth?", stop_deadline)?)?;
                last_reported = Some(actual);
                if actual != Some(points) {
                    return Err(DriverError::Protocol(format!(
                        "stopped scope buffer depth {actual:?} differs from requested {points}; no retry"
                    )));
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Include the last received value even when the wall-clock deadline
            // expires before another query or an individual I/O stalls.
            let error = match error {
                DriverError::Timeout { operation, transferred } => {
                    let reported = match last_reported {
                        Some(Some(value)) => value.to_string(),
                        Some(None) => "AUTO".into(),
                        None => "not_read".into(),
                    };
                    DriverError::Timeout {
                        operation: format!("{phase}: {operation}; requested={points}, last_reported={reported}; state uncertain, no retry"),
                        transferred,
                    }
                }
                other => other,
            };
            self.session.state = DriverState::Fault;
            if temporary_run && !stop_attempted {
                let cleanup = self.session.write_explicit_disable(
                    ":STOP", Deadline::after(self.options.close_timeout),
                );
                if let Err(cleanup_error) = cleanup {
                    let evidence = format!(
                        "STOP cleanup failed ({cleanup_error}); state uncertain, no retry"
                    );
                    return Err(match error {
                        DriverError::Canceled => DriverError::Protocol(format!(
                            "depth application canceled; {evidence}"
                        )),
                        other => scpi::transfer_error("depth application", &evidence, other),
                    });
                }
                return Err(scpi::transfer_error(
                    "depth application",
                    "STOP cleanup sent, state unconfirmed; no retry",
                    error,
                ));
            }
            return Err(error);
        }
        Ok(())
    }
    fn confirm_acquisition_state(
        &mut self,
        stopped: bool,
        started: Duration,
        timeout: Duration,
        deadline: Deadline,
    ) -> DriverResult<()> {
        loop {
            self.session.ready(deadline)?;
            if (self.acquisition(deadline)? == AcquisitionState::Stopped) == stopped {
                return Ok(());
            }
            self.wait_for_depth_confirmation(started, timeout, "depth acquisition-state confirmation")?;
        }
    }
    fn wait_for_depth_confirmation(&self, started: Duration, timeout: Duration, operation: &str) -> DriverResult<()> {
        let elapsed = self.session.clock.now().saturating_sub(started);
        if elapsed >= timeout {
            return Err(DriverError::Timeout {
                operation: format!("{operation}; state uncertain, no retry"),
                transferred: 0,
            });
        }
        self.session.clock.wait(
            Duration::from_millis(20).min(timeout.saturating_sub(elapsed)),
        );
        Ok(())
    }
    pub fn run(&mut self, deadline: Deadline) -> DriverResult<()> {
        self.session.ready(deadline)?;
        self.session.write(":RUN", deadline)
    }
    pub fn stop(&mut self, deadline: Deadline) -> DriverResult<()> {
        self.session.write_explicit_disable(":STOP", deadline)
    }
    /// Explicit STOP-buffer snapshot. Pure admission precedes STOP, then bounded
    /// status confirmation precedes transfer. This is not a fresh trigger; the
    /// scope remains stopped and failures never replay STOP or waveform reads.
    pub fn capture_snapshot(
        &mut self,
        request: &CaptureRequest,
        deadline: Deadline,
    ) -> DriverResult<WaveformFrame> {
        let deadline = deadline.earlier(Deadline::after(CAPTURE_TIMEOUT));
        validate_capture_request(self.model, request, self.options.max_capture_bytes)?;
        let stop_deadline = deadline.earlier(Deadline::after(self.options.timeout));
        self.session.ready(stop_deadline)?;
        let started = self.session.clock.now();
        // This command is owned by the capture operation, so normal operation
        // cancellation applies. It does not enter the public safety-stop lane.
        self.session.write(":STOP", stop_deadline)?;
        loop {
            self.session.ready(stop_deadline)?;
            if self.acquisition(stop_deadline)? == AcquisitionState::Stopped {
                break;
            }
            let elapsed = self.session.clock.now().saturating_sub(started);
            if elapsed >= self.options.timeout {
                return Err(DriverError::Timeout {
                    operation: "stop confirmation; state uncertain, no retry".into(),
                    transferred: 0,
                });
            }
            self.session.clock.wait(
                Duration::from_millis(20).min(self.options.timeout.saturating_sub(elapsed)),
            );
        }
        let mut frame = self.capture_buffer(request, deadline, true)?;
        for channel in &mut frame.channels {
            channel.metadata["snapshot"] = serde_json::json!({
                "confirmed":true,"method":"stop_buffer","final_state":"STOP","fresh_acquisition":false
            });
        }
        Ok(frame)
    }
    /// Explicit fresh-acquisition action. Changes trigger mode to SINGLE and
    /// leaves the scope stopped. A non-STOP -> STOP transition is required;
    /// immediate STOP is ambiguous (ignored command or a fast acquisition), so
    /// it is never mislabeled as a confirmed fresh frame. No forced trigger,
    /// automatic RUN, reset, settings restoration or retry is performed.
    pub fn capture_fresh(
        &mut self,
        request: &CaptureRequest,
        deadline: Deadline,
    ) -> DriverResult<WaveformFrame> {
        let deadline = deadline.earlier(Deadline::after(CAPTURE_TIMEOUT));
        validate_capture_request(self.model, request, self.options.max_capture_bytes)?;
        self.session.ready(deadline)?;
        self.session.write(":SINGle", deadline)?;
        let started = self.session.clock.now();
        let mut active = false;
        loop {
            self.session.ready(deadline)?;
            let state = self.acquisition(deadline)?;
            if state == AcquisitionState::Stopped {
                if !active {
                    return Err(DriverError::Protocol("fresh acquisition unconfirmed: immediate STOP; acquisition may have completed too quickly or command may have been ignored".into()));
                }
                break;
            }
            active = true;
            if self.session.clock.now().saturating_sub(started) >= self.options.timeout {
                return Err(DriverError::Timeout {
                    operation: "single acquisition; state uncertain, no retry".into(),
                    transferred: 0,
                });
            }
            self.session.clock.wait(Duration::from_millis(20));
        }
        let mut frame = self.capture(request, deadline)?;
        for channel in &mut frame.channels {
            channel.metadata["fresh_acquisition"] = serde_json::json!({"confirmed":true,"method":"single_non_stop_to_stop","final_state":"STOP","trigger_mode":"single"});
        }
        Ok(frame)
    }
    fn admit(&self, request: &CaptureRequest, points: u64) -> DriverResult<usize> {
        admit_capture_points(self.model, request, points, self.options.max_capture_bytes)
    }
    pub fn capture(
        &mut self,
        request: &CaptureRequest,
        deadline: Deadline,
    ) -> DriverResult<WaveformFrame> {
        self.capture_buffer(request, deadline, false)
    }
    fn capture_buffer(
        &mut self,
        request: &CaptureRequest,
        deadline: Deadline,
        require_stopped: bool,
    ) -> DriverResult<WaveformFrame> {
        let deadline = deadline.earlier(Deadline::after(CAPTURE_TIMEOUT));
        validate_capture_request(self.model, request, self.options.max_capture_bytes)?;
        self.session.ready(deadline)?;
        let acquisition = self.acquisition(deadline)?;
        if (request.source == CaptureSource::Memory || require_stopped)
            && acquisition != AcquisitionState::Stopped
        {
            return Err(DriverError::Responsibility(
                "memory or snapshot capture requires explicit STOP before transfer".into(),
            ));
        }
        let available = if request.source == CaptureSource::Screen {
            Some(1000)
        } else {
            memory_depth(&self.session.query_text(":ACQuire:MDEPth?", deadline)?)?
        };
        let requested = request.points.or(available).ok_or_else(|| {
            DriverError::Invalid(
                "automatic memory depth requires an explicit bounded point request".into(),
            )
        })?;
        if available.is_some_and(|available| requested > available) {
            return Err(DriverError::Invalid(
                "requested points exceed current memory depth".into(),
            ));
        }
        let count = self.admit(request, requested)?;
        let started = self.session.clock.now();
        let captured_at_unix_ms = scpi::now_ms();
        let result = (|| {
            let mut channels = Vec::with_capacity(request.channels.len());
            let mut completed_capture_bytes = 0;
            for &channel in &request.channels {
                self.session
                    .write(&format!(":WAVeform:SOURce CHANnel{channel}"), deadline)?;
                self.session.write(
                    if request.source == CaptureSource::Screen {
                        ":WAVeform:MODE NORMal"
                    } else {
                        ":WAVeform:MODE RAW"
                    },
                    deadline,
                )?;
                self.session.write(
                    if self.model == ScopeModel::Ds7054 {
                        ":WAVeform:FORMat BYTE"
                    } else {
                        ":WAVeform:FORMat WORD"
                    },
                    deadline,
                )?;
                if self.model == ScopeModel::Ds7054 {
                    // PREamble describes the persistent transfer window, including a prior chunk.
                    self.session.write(":WAVeform:STARt 1", deadline)?;
                }
                self.session
                    .write(&format!(":WAVeform:POINts {count}"), deadline)?;
                if self.model == ScopeModel::Ds7054 {
                    self.session
                        .write(&format!(":WAVeform:STOP {count}"), deadline)?;
                }
                let preamble = parse_preamble(
                    &self.session.query_text(":WAVeform:PREamble?", deadline)?,
                    self.model,
                )?;
                let expected_transfer_type = if request.source == CaptureSource::Screen {
                    0
                } else {
                    2
                };
                let mismatch = if preamble.points != requested {
                    Some("point count")
                } else if preamble.acquisition_type != expected_transfer_type {
                    Some("transfer mode")
                } else {
                    None
                };
                if let Some(mismatch) = mismatch {
                    // Bound escaped raw evidence well below the Worker error envelope.
                    let raw: String = preamble.original.chars().take(512).collect();
                    let evidence = serde_json::json!({
                        "model":self.model.name(), "channel":channel, "source":request.source,
                        "requested_points":requested, "observed_points":preamble.points,
                        "expected_transfer_type":expected_transfer_type,
                        "observed_transfer_type":preamble.acquisition_type,
                        "preamble_truncated":raw.len() < preamble.original.len(), "preamble":raw,
                    });
                    return Err(DriverError::Protocol(format!(
                        "scope preamble {mismatch} mismatch: {evidence}"
                    )));
                }
                // All channel samples were admitted together before allocating any raw array.
                let allocation_error =
                    |_| DriverError::Invalid("insufficient memory for admitted waveform".into());
                let mut samples = match self.model {
                    ScopeModel::Ds7054 => {
                        let mut values = Vec::new();
                        values.try_reserve_exact(count).map_err(allocation_error)?;
                        Samples::U8(values)
                    }
                    ScopeModel::Mho5054 => {
                        let mut values = Vec::new();
                        values.try_reserve_exact(count).map_err(allocation_error)?;
                        Samples::U16(values)
                    }
                };
                for start in (0..count).step_by(self.options.chunk_points) {
                    self.session.stop.check()?;
                    let stop = (start + self.options.chunk_points).min(count);
                    let bytes = (|| {
                        self.session
                            .write(&format!(":WAVeform:STARt {}", start + 1), deadline)?;
                        self.session
                            .write(&format!(":WAVeform:STOP {stop}"), deadline)?;
                        self.session.query_block(
                            ":WAVeform:DATA?",
                            (stop - start) * self.model.sample_bytes(),
                            deadline,
                        )
                    })().map_err(|error| scpi::transfer_error(
                        "scope capture",
                        &format!("channel=CH{channel} window={}..{stop} completed_capture_bytes={completed_capture_bytes} total_capture_bytes={}", start + 1, count * self.model.sample_bytes() * request.channels.len()),
                        error,
                    ))?;
                    completed_capture_bytes += bytes.len();
                    match &mut samples {
                        Samples::U8(values) => values.extend(bytes),
                        Samples::U16(values) => values.extend(
                            bytes
                                .chunks_exact(2)
                                .map(|b| u16::from_le_bytes([b[0], b[1]])),
                        ),
                        _ => unreachable!("model sample representation fixed before acquisition"),
                    }
                }
                let data = ChannelWaveform {
                    channel: format!("CH{channel}"),
                    samples,
                    calibration: preamble.calibration,
                    metadata: serde_json::json!({"model":self.model,"identity":self.identity(),"source":request.source,"preamble":preamble.original,"dtype":if self.model==ScopeModel::Ds7054{"u8"}else{"u16_le"},"conversion":"(raw - yreference - yorigin_codes) * yincrement","capture_elapsed_ms":self.session.clock.now().saturating_sub(started).as_millis().min(u128::from(u64::MAX)) as u64}),
                };
                data.validate()
                    .map_err(|e| DriverError::Protocol(format!("invalid waveform: {e}")))?;
                channels.push(data);
            }
            Ok(WaveformFrame {
                id: scpi::id(),
                captured_at_unix_ms,
                nominal_offset_s: None,
                channels,
            })
        })();
        if result.is_err() {
            self.session.state = DriverState::Fault;
        }
        result
    }
    pub fn close(&mut self) -> DriverResult<CleanupReport> {
        self.session.close("scope")
    }
    pub fn state(&self) -> DriverState {
        self.session.state
    }
    pub fn identity(&self) -> &str {
        &self.session.identity
    }
    pub fn resource_name(&self) -> &str {
        self.session.resource_name()
    }
    pub fn stop_handle(&self) -> StopHandle {
        self.session.stop.clone()
    }
    pub fn has_resource_responsibility(&self) -> bool {
        self.session.has_responsibility()
    }
    pub fn cleanup_report(&self) -> Option<&CleanupReport> {
        self.session.cleanup_report()
    }
    pub fn options(&self) -> &ScopeOptions {
        &self.options
    }
}
impl DriverLifecycle for Scope {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        Scope::close(self)
    }
    fn has_responsibility(&self) -> bool {
        self.has_resource_responsibility()
    }
}
