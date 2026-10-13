//! Bounded SCPI session shared by Rigol family adapters. No arbitrary user SCPI API.
use crate::{
    clock::Clock,
    lifecycle::{
        CleanupReport, CleanupStep, DriverState, OwnedTransport, ProbeReport,
        ResponsibilityRegistry,
    },
    transport::{ByteTransport, Deadline, ReadFragment, VisaManager},
    DriverError, DriverResult,
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const TRANSFER_MAX: usize = 16384;
// A bounded SCPI response can span many native transport fragments. This is
// software admission policy, not a claim about an instrument's maximum window.
pub const BLOCK_PAYLOAD_MAX: usize = 1_000_000;
pub(crate) fn transfer_error(command: &str, stage: &str, error: DriverError) -> DriverError {
    match error {
        DriverError::Timeout {
            operation,
            transferred,
        } => DriverError::Timeout {
            operation: format!("{command} {stage}: {operation}"),
            transferred,
        },
        DriverError::Native {
            operation,
            status,
            transferred,
        } => DriverError::Native {
            operation: format!("{command} {stage}: {operation}"),
            status,
            transferred,
        },
        DriverError::Protocol(message) => {
            DriverError::Protocol(format!("{command} {stage}: {message}"))
        }
        other => other,
    }
}
#[derive(Clone, Default)]
pub struct StopHandle(Arc<AtomicBool>, Arc<AtomicBool>);
impl StopHandle {
    pub fn request_stop(&self) {
        self.0.store(true, Ordering::Release)
    }
    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::Acquire) || self.1.load(Ordering::Acquire)
    }
    pub fn cancel_operation(&self) {
        self.1.store(true, Ordering::Release);
    }
    /// Clears only the operation fence; close/shutdown intent is independent.
    pub fn clear_operation_cancel(&self) {
        self.1.store(false, Ordering::Release);
    }
    pub(crate) fn check(&self) -> DriverResult<()> {
        if self.is_stopped() {
            Err(DriverError::Canceled)
        } else {
            Ok(())
        }
    }
}
pub(crate) fn id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{:016x}{:016x}",
        now_ms(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
pub(crate) fn number(text: &str) -> DriverResult<f64> {
    let v = text
        .trim()
        .parse::<f64>()
        .map_err(|_| DriverError::Protocol("invalid SCPI number".into()))?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(DriverError::Protocol("nonfinite SCPI number".into()))
    }
}
pub(crate) fn integer(text: &str) -> DriverResult<u64> {
    let v = number(text)?;
    if v >= 0.0 && v.fract() == 0.0 && v <= 500_000_000.0 {
        Ok(v as u64)
    } else {
        Err(DriverError::Protocol(
            "SCPI point count outside model bounds".into(),
        ))
    }
}
pub(crate) fn boolean(text: &str) -> DriverResult<bool> {
    match text.trim().to_ascii_uppercase().as_str() {
        "1" | "ON" => Ok(true),
        "0" | "OFF" => Ok(false),
        _ => Err(DriverError::Protocol("invalid SCPI boolean".into())),
    }
}
struct Closing {
    rx: Receiver<(OwnedTransport, Option<DriverError>)>,
    handle: JoinHandle<()>,
}
pub(crate) struct Session {
    manager: Option<VisaManager>,
    resource: String,
    io: Option<OwnedTransport>,
    closing: Option<Closing>,
    pub state: DriverState,
    pub identity: String,
    pub stop: StopHandle,
    pub clock: Arc<dyn Clock>,
    timeout: Duration,
    close_timeout: Duration,
    last_cleanup: Option<CleanupReport>,
    identity_verified: bool,
}
impl Session {
    pub fn new(
        manager: Option<VisaManager>,
        resource: String,
        io: Option<Box<dyn ByteTransport>>,
        clock: Arc<dyn Clock>,
        timeout: Duration,
        close_timeout: Duration,
    ) -> DriverResult<Self> {
        if resource.len() > 255
            || resource.bytes().any(|b| b < 32 || b == 127)
            || timeout.is_zero()
            || timeout > Duration::from_secs(120)
            || close_timeout.is_zero()
            || close_timeout > Duration::from_secs(30)
        {
            return Err(DriverError::Invalid(
                "invalid resource or communication timeout".into(),
            ));
        }
        Ok(Self {
            manager,
            resource,
            io: io.map(|io| ResponsibilityRegistry::default().adopt(io)),
            closing: None,
            state: DriverState::Disconnected,
            identity: String::new(),
            stop: StopHandle::default(),
            clock,
            timeout,
            close_timeout,
            last_cleanup: None,
            identity_verified: false,
        })
    }
    pub fn connect(&mut self, model: &str) -> DriverResult<ProbeReport> {
        if self.state != DriverState::Disconnected {
            return Err(DriverError::Responsibility(
                "connection requires disconnected driver".into(),
            ));
        }
        self.state = DriverState::Connecting;
        self.stop = StopHandle::default();
        let deadline = Deadline::after(self.timeout);
        let result = (|| {
            if self.io.is_none() {
                let manager = self.manager.as_ref().ok_or(DriverError::Closed)?;
                let resource = manager.canonicalize(&self.resource)?;
                match manager.open_owned(resource, deadline) {
                    Ok(mut io) => {
                        let result = io.disable_termination();
                        self.io = Some(ResponsibilityRegistry::default().adopt(Box::new(io)));
                        result?
                    }
                    Err(failure) => {
                        self.io = failure
                            .session
                            .map(|s| ResponsibilityRegistry::default().adopt(Box::new(s)));
                        return Err(failure.error);
                    }
                }
            }
            self.identity = self.query_text("*IDN?", deadline)?;
            let fields: Vec<_> = self.identity.split(',').map(str::trim).collect();
            let normalize = |s: &str| s.to_ascii_uppercase().replace([' ', '-'], "");
            if fields.len() != 4
                || fields.iter().any(|p| p.is_empty())
                || !fields[0].to_ascii_uppercase().contains("RIGOL")
                || normalize(fields[1]) != normalize(model)
            {
                return Err(DriverError::Protocol(format!(
                    "identity does not match {model}"
                )));
            }
            Ok(ProbeReport::new(
                serde_json::json!({"manufacturer":fields[0],"model":fields[1],"serial":fields[2],"firmware":fields[3]}),
                serde_json::json!({"connect_effects":"identity_query_only"}),
                false,
            ))
        })();
        self.state = if result.is_ok() {
            self.identity_verified = true;
            DriverState::Ready
        } else {
            DriverState::Fault
        };
        result
    }
    pub fn ready(&self, deadline: Deadline) -> DriverResult<()> {
        deadline.remaining_millis()?;
        self.stop.check()?;
        if self.state == DriverState::Ready {
            Ok(())
        } else {
            Err(DriverError::Responsibility("driver must be ready".into()))
        }
    }
    pub fn write(&mut self, command: &str, deadline: Deadline) -> DriverResult<()> {
        self.stop.check()?;
        deadline.remaining_millis()?;
        let io_deadline = deadline.earlier(Deadline::after(self.timeout));
        let result = self
            .io
            .as_mut()
            .ok_or(DriverError::Closed)?
            .write_all(format!("{command}\n").as_bytes(), io_deadline);
        if result.is_err() {
            self.state = DriverState::Fault;
        }
        result?;
        self.stop.check()?;
        Ok(())
    }
    fn read_exact_fragment(
        &mut self,
        count: usize,
        deadline: Deadline,
    ) -> DriverResult<ReadFragment> {
        if count > BLOCK_PAYLOAD_MAX {
            return Err(DriverError::Invalid(
                "SCPI read exceeds transfer bound".into(),
            ));
        }
        let mut out = Vec::with_capacity(count);
        let mut end_of_message = false;
        while out.len() < count {
            self.stop.check()?;
            let progress = || format!("received_bytes={} expected_bytes={count}", out.len());
            deadline.remaining_millis().map_err(|error| {
                transfer_error("SCPI read", &progress(), error)
            })?;
            let io_deadline = deadline.earlier(Deadline::after(self.timeout));
            let fragment_max = (count - out.len()).min(TRANSFER_MAX);
            let result = self
                .io
                .as_mut()
                .ok_or(DriverError::Closed)?
                .read_fragment(fragment_max, io_deadline);
            let part = match result {
                Ok(part) => part,
                Err(e) => {
                    self.state = DriverState::Fault;
                    return Err(transfer_error("SCPI read", &progress(), e));
                }
            };
            if part.bytes.is_empty() || part.bytes.len() > fragment_max {
                self.state = DriverState::Fault;
                return Err(DriverError::Protocol(
                    format!("empty or oversized SCPI transfer: {}", progress()),
                ));
            }
            end_of_message = part.end_of_message;
            out.extend(part.bytes);
            if end_of_message && out.len() < count {
                self.state = DriverState::Fault;
                return Err(DriverError::Protocol(
                    format!("SCPI message ended before its declared byte count: received_bytes={} expected_bytes={count}", out.len()),
                ));
            }
        }
        self.stop.check()?;
        Ok(ReadFragment {
            bytes: out,
            end_of_message,
        })
    }
    /// Reserved for an explicit generator output-disable action. Cancellation
    /// fences new work but must not fence the owning run's declared cleanup.
    pub fn write_explicit_disable(
        &mut self,
        command: &str,
        deadline: Deadline,
    ) -> DriverResult<()> {
        deadline.remaining_millis()?;
        if !self.identity_verified || !matches!(self.state, DriverState::Ready | DriverState::Fault)
        {
            return Err(DriverError::Responsibility(
                "output disable requires an identity-verified owned session".into(),
            ));
        }
        let io_deadline = deadline.earlier(Deadline::after(self.timeout));
        let result = self
            .io
            .as_mut()
            .ok_or(DriverError::Closed)?
            .write_all(format!("{command}\n").as_bytes(), io_deadline);
        if result.is_err() {
            self.state = DriverState::Fault;
        }
        result
    }
    pub fn query_text(&mut self, command: &str, deadline: Deadline) -> DriverResult<String> {
        self.write(command, deadline)?;
        let mut out = Vec::new();
        loop {
            self.stop.check()?;
            deadline.remaining_millis()?;
            let io_deadline = deadline.earlier(Deadline::after(self.timeout));
            let part = self
                .io
                .as_mut()
                .ok_or(DriverError::Closed)?
                .read_bounded((4096 - out.len()).min(1024), io_deadline);
            let part = match part {
                Ok(v) => v,
                Err(e) => {
                    self.state = DriverState::Fault;
                    return Err(transfer_error(command, "text reply", e));
                }
            };
            if part.is_empty() || part.len() > 4096 - out.len() {
                self.state = DriverState::Fault;
                return Err(DriverError::Protocol(
                    "empty or oversized SCPI text reply".into(),
                ));
            }
            out.extend(part);
            if out.ends_with(b"\n") {
                break;
            }
            if out.len() == 4096 {
                self.state = DriverState::Fault;
                return Err(DriverError::Protocol("unterminated SCPI text reply".into()));
            }
        }
        if !out.is_ascii() {
            return Err(DriverError::Protocol("non-ASCII SCPI reply".into()));
        }
        let text = std::str::from_utf8(&out).unwrap().trim();
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(DriverError::Protocol("malformed SCPI text reply".into()));
        }
        self.stop.check()?;
        Ok(text.into())
    }
    pub fn query_block(
        &mut self,
        command: &str,
        expected: usize,
        deadline: Deadline,
    ) -> DriverResult<Vec<u8>> {
        if expected == 0 || expected > BLOCK_PAYLOAD_MAX {
            return Err(DriverError::Invalid(
                "block payload exceeds chunk limit".into(),
            ));
        }
        self.write(command, deadline)?;
        let prefix = self
            .read_exact_fragment(2, deadline)
            .map_err(|e| transfer_error(command, "block prefix", e))?;
        if prefix.end_of_message
            || prefix.bytes[0] != b'#'
            || !(b'1'..=b'9').contains(&prefix.bytes[1])
        {
            self.state = DriverState::Fault;
            return Err(DriverError::Protocol(
                "invalid definite block prefix".into(),
            ));
        }
        let digits = self
            .read_exact_fragment((prefix.bytes[1] - b'0') as usize, deadline)
            .map_err(|e| transfer_error(command, "block length", e))?;
        if digits.end_of_message || !digits.bytes.iter().all(u8::is_ascii_digit) {
            self.state = DriverState::Fault;
            return Err(DriverError::Protocol("invalid block byte count".into()));
        }
        let declared = std::str::from_utf8(&digits.bytes)
            .unwrap()
            .parse::<usize>()
            .map_err(|_| DriverError::Protocol("block length overflow".into()))?;
        if declared != expected {
            self.state = DriverState::Fault;
            return Err(DriverError::Protocol(format!(
                "block declared {declared} bytes, expected {expected}"
            )));
        }
        let body = self
            .read_exact_fragment(expected, deadline)
            .map_err(|e| transfer_error(command, "block payload", e))?;
        // USBTMC/VISA may end the exact definite block at EOM with no LF byte.
        // Only transport-confirmed completion permits omitting the terminator;
        // a short read or timeout is never treated as successful completion.
        if body.end_of_message {
            return Ok(body.bytes);
        }
        let ending = self
            .read_exact_fragment(1, deadline)
            .map_err(|e| transfer_error(command, "block terminator", e))?;
        if ending.bytes == b"\r" {
            if ending.end_of_message
                || self
                    .read_exact_fragment(1, deadline)
                    .map_err(|e| transfer_error(command, "block CRLF terminator", e))?
                    .bytes
                    != b"\n"
            {
                self.state = DriverState::Fault;
                return Err(DriverError::Protocol("invalid block terminator".into()));
            }
        } else if ending.bytes != b"\n" {
            self.state = DriverState::Fault;
            return Err(DriverError::Protocol("invalid block terminator".into()));
        }
        Ok(body.bytes)
    }
    pub fn resource_name(&self) -> &str {
        &self.resource
    }
    pub fn has_responsibility(&self) -> bool {
        self.closing.is_some()
            || self
                .io
                .as_ref()
                .is_some_and(ByteTransport::has_responsibility)
    }
    pub fn cleanup_report(&self) -> Option<&CleanupReport> {
        self.last_cleanup.as_ref()
    }
    pub fn close(&mut self, role: &str) -> DriverResult<CleanupReport> {
        self.stop.request_stop();
        self.state = DriverState::Closing;
        let mut error = None;
        if self.closing.is_none() {
            if let Some(mut io) = self.io.take() {
                let (tx, rx) = mpsc::channel();
                let handle = std::thread::spawn(move || {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| io.close()));
                    let error = match result {
                        Ok(Ok(r)) if r.released && !io.has_responsibility() => None,
                        Ok(Ok(_)) => Some(DriverError::Responsibility(
                            "transport release unconfirmed".into(),
                        )),
                        Ok(Err(e)) => Some(e),
                        Err(_) => Some(DriverError::Responsibility(
                            "transport close panicked".into(),
                        )),
                    };
                    let _ = tx.send((io, error));
                });
                self.closing = Some(Closing { rx, handle });
            }
        }
        if let Some(job) = &self.closing {
            match job.rx.recv_timeout(self.close_timeout) {
                Ok((io, e)) => {
                    self.io = io.has_responsibility().then_some(io);
                    error = e;
                    let job = self.closing.take().unwrap();
                    if job.handle.join().is_err() {
                        error = Some(DriverError::Responsibility(
                            "cleanup worker panicked".into(),
                        ));
                    }
                }
                Err(_) => {
                    error = Some(DriverError::Timeout {
                        operation: "SCPI cleanup still retained".into(),
                        transferred: 0,
                    })
                }
            }
        }
        let pending = self.has_responsibility();
        let report = CleanupReport::new(
            id(),
            vec![CleanupStep {
                role: role.into(),
                action: "release_transport_preserve_outputs".into(),
                error: error.map(|e| e.to_string()),
            }],
            None,
            if pending { vec![role.into()] } else { vec![] },
        )?;
        self.state = if pending {
            DriverState::Fault
        } else {
            self.identity_verified = false;
            DriverState::Disconnected
        };
        self.last_cleanup = Some(report.clone());
        Ok(report)
    }
}
