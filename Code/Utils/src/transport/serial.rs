use super::{
    reservations::Reservation,
    serial_abi::{NativeSerial, SerialBackend, SerialIo},
    serial_discovery::{records_to_devices, SerialDeviceInfo},
    ByteTransport, CanonicalResource, CloseReport, Deadline, ResourceBook,
};
use crate::{DriverError, DriverResult};
use std::sync::{Arc, Mutex, OnceLock};
#[derive(Clone, Debug)]
pub struct SerialConfig {
    pub port: String,
    pub baudrate: u32,
    pub dtr: bool,
    pub rts: bool,
}
impl SerialConfig {
    pub fn instrument(port: impl Into<String>) -> Self {
        Self {
            port: port.into(),
            baudrate: 115200,
            dtr: false,
            rts: false,
        }
    }
}
pub fn canonical_com(name: &str) -> DriverResult<CanonicalResource> {
    let name = name.trim();
    let name = name.strip_prefix("\\\\.\\").unwrap_or(name);
    let upper = name.to_ascii_uppercase();
    let suffix = upper
        .strip_prefix("COM")
        .ok_or_else(|| DriverError::Invalid("expected COM port".into()))?;
    if suffix.is_empty() || suffix.len() > 5 || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DriverError::Invalid("invalid COM number".into()));
    }
    let number: u16 = suffix
        .parse()
        .map_err(|_| DriverError::Invalid("COM number outside 1..65535".into()))?;
    if number == 0 {
        return Err(DriverError::Invalid("COM0 is not a port".into()));
    }
    Ok(CanonicalResource(format!("serial://COM{number}")))
}
pub fn enumerate_with(backend: &dyn SerialBackend) -> DriverResult<Vec<SerialDeviceInfo>> {
    records_to_devices(backend.enumerate()?)
}
pub struct SerialSession {
    inner: Arc<SerialInner>,
}
pub struct SerialOpenFailure {
    pub error: DriverError,
    pub session: Option<SerialSession>,
}
impl From<DriverError> for SerialOpenFailure {
    fn from(error: DriverError) -> Self {
        Self {
            error,
            session: None,
        }
    }
}
struct SerialInner {
    state: Mutex<SerialState>,
}
struct SerialState {
    io: Box<dyn SerialIo>,
    reservation: Reservation,
    released: bool,
    faulted: bool,
}
fn retained() -> &'static Mutex<Vec<Arc<SerialInner>>> {
    static RETAINED: OnceLock<Mutex<Vec<Arc<SerialInner>>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(Vec::new()))
}
impl SerialSession {
    /// An incomplete request/reply exchange may still receive a late reply.
    /// Only explicit close is safe; do not reinterpret it as another command's ACK.
    pub(crate) fn fence_protocol(&mut self) -> DriverResult<()> {
        self.inner
            .state
            .lock()
            .map_err(|_| DriverError::Responsibility("serial state poisoned".into()))?
            .faulted = true;
        Ok(())
    }
    pub fn open(config: SerialConfig, book: &ResourceBook) -> DriverResult<Self> {
        Self::from_backend(config, book, Arc::new(NativeSerial))
    }
    pub fn from_backend(
        config: SerialConfig,
        book: &ResourceBook,
        backend: Arc<dyn SerialBackend>,
    ) -> DriverResult<Self> {
        Self::from_backend_owned(config, book, backend).map_err(|failure| failure.error)
    }
    pub fn from_backend_owned(
        config: SerialConfig,
        book: &ResourceBook,
        backend: Arc<dyn SerialBackend>,
    ) -> Result<Self, SerialOpenFailure> {
        let resource = canonical_com(&config.port)?;
        if config.baudrate == 0 || config.baudrate > 4_000_000 {
            return Err(DriverError::Invalid("invalid serial baudrate".into()).into());
        }
        let mut reservation = book.reserve(&resource)?;
        let path = format!(
            "\\\\.\\{}",
            resource.as_str().strip_prefix("serial://").unwrap()
        );
        let io = match backend.open(&path) {
            Ok(io) => io,
            Err(error) => {
                reservation.release();
                return Err(error.into());
            }
        };
        let session = Self {
            inner: Arc::new(SerialInner {
                state: Mutex::new(SerialState {
                    io,
                    reservation,
                    released: false,
                    faulted: false,
                }),
            }),
        };
        let inner = session.inner.clone();
        let configured = match inner.state.lock() {
            Ok(mut state) => state.io.configure(&config),
            Err(_) => Err(DriverError::Responsibility("serial state poisoned".into())),
        };
        if let Err(error) = configured {
            return Err(SerialOpenFailure {
                error,
                session: Some(session),
            });
        }
        Ok(session)
    }
}
impl ByteTransport for SerialSession {
    fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<()> {
        if bytes.is_empty() || bytes.len() > 65536 {
            return Err(DriverError::Invalid(
                "serial write length outside 1..65536".into(),
            ));
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| DriverError::Responsibility("serial state poisoned".into()))?;
        usable(&state)?;
        let mut sent = 0;
        let result = (|| {
            while sent < bytes.len() {
                deadline.remaining_millis()?;
                let count = state.io.write(&bytes[sent..], deadline)?;
                if count == 0 || count > bytes.len() - sent {
                    return Err(DriverError::Protocol("serial invalid write count".into()));
                }
                sent += count;
            }
            Ok(())
        })();
        if result.is_err() {
            state.faulted = true;
        }
        result
    }
    fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
        if maximum == 0 || maximum > 65536 {
            return Err(DriverError::Invalid(
                "serial read length outside 1..65536".into(),
            ));
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| DriverError::Responsibility("serial state poisoned".into()))?;
        usable(&state)?;
        let result = (|| {
            deadline.remaining_millis()?;
            let bytes = state.io.read(maximum, deadline)?;
            if bytes.len() > maximum {
                return Err(DriverError::Protocol("serial read count overflow".into()));
            }
            Ok(bytes)
        })();
        // An ordinary finite no-data timeout permits the next telemetry poll.
        // A still-pending OS operation forbids another call until release settles.
        if result.is_err()
            && (state.io.has_pending() || !matches!(result, Err(DriverError::Timeout { .. })))
        {
            state.faulted = true;
        }
        result
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        close_inner(&self.inner)
    }
    fn has_responsibility(&self) -> bool {
        !self
            .inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .released
    }
}
fn usable(state: &SerialState) -> DriverResult<()> {
    if state.released {
        Err(DriverError::Closed)
    } else if state.faulted || state.io.has_pending() {
        Err(DriverError::Responsibility(
            "serial fault/pending call; close explicitly".into(),
        ))
    } else {
        Ok(())
    }
}
fn close_inner(inner: &SerialInner) -> DriverResult<CloseReport> {
    let mut state = inner
        .state
        .lock()
        .map_err(|_| DriverError::Responsibility("serial state poisoned".into()))?;
    if state.released {
        return Ok(CloseReport {
            released: true,
            status: None,
        });
    }
    state.faulted = true;
    let report = state.io.close()?;
    if report.released && state.io.has_pending() {
        return Err(DriverError::Responsibility(
            "serial reported release while completion is pending".into(),
        ));
    }
    if report.released {
        state.reservation.release();
        state.released = true;
    }
    Ok(report)
}
impl Drop for SerialSession {
    fn drop(&mut self) {
        if self.has_responsibility() {
            retained()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(self.inner.clone());
        }
    }
}
pub fn retry_retained() -> usize {
    let pending = std::mem::take(&mut *retained().lock().unwrap_or_else(|e| e.into_inner()));
    for inner in pending {
        if !close_inner(&inner).is_ok_and(|r| r.released) {
            retained()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(inner);
        }
    }
    retained().lock().unwrap_or_else(|e| e.into_inner()).len()
}
