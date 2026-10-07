use super::{
    reservations::Reservation,
    visa_abi::{
        self, VisaApi, VI_ERROR_RSRC_BUSY, VI_ERROR_RSRC_NFOUND, VI_ERROR_TMO, VI_EXCLUSIVE_LOCK,
    },
    CanonicalResource, Deadline, ResourceBook,
};
use crate::{DriverError, DriverResult};
use std::sync::{Arc, Mutex, OnceLock};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseReport {
    pub released: bool,
    pub status: Option<i32>,
}
#[derive(Clone)]
pub struct VisaManager {
    inner: Arc<ManagerInner>,
}
struct ManagerInner {
    api: Arc<dyn VisaApi>,
    handle: u32,
    book: ResourceBook,
    calls: Mutex<()>,
}
pub struct VisaSession {
    inner: Arc<SessionInner>,
}
pub struct OpenFailure {
    pub error: DriverError,
    pub session: Option<VisaSession>,
}
pub struct ManagerReleaseFailure {
    pub manager: VisaManager,
    pub error: DriverError,
}
impl std::fmt::Debug for ManagerReleaseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}
struct SessionInner {
    manager: Arc<ManagerInner>,
    state: Mutex<SessionState>,
}
struct SessionState {
    handle: Option<u32>,
    reservation: Reservation,
    faulted: bool,
}
enum Retained {
    Session(Arc<SessionInner>),
    Handle(Arc<ManagerInner>, u32),
    Manager(Arc<dyn VisaApi>, u32),
}
fn retained() -> &'static Mutex<Vec<Retained>> {
    static RETAINED: OnceLock<Mutex<Vec<Retained>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(Vec::new()))
}
fn retain(value: Retained) {
    retained()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(value);
}
fn status(operation: &str, code: i32, transferred: usize) -> DriverResult<()> {
    if code >= 0 {
        Ok(())
    } else if code == VI_ERROR_TMO {
        Err(DriverError::Timeout {
            operation: operation.into(),
            transferred,
        })
    } else if code == VI_ERROR_RSRC_BUSY {
        Err(DriverError::Busy(operation.into()))
    } else {
        Err(DriverError::Native {
            operation: operation.into(),
            status: code,
            transferred,
        })
    }
}
fn valid_name(name: &str) -> DriverResult<()> {
    if name.is_empty()
        || name.len() >= 256
        || !name.is_ascii()
        || name.bytes().any(|b| b < 32 || b == 127)
    {
        Err(DriverError::Invalid(
            "VISA resource name must be bounded printable ASCII".into(),
        ))
    } else {
        Ok(())
    }
}
impl VisaManager {
    pub fn release(self) -> Result<CloseReport, ManagerReleaseFailure> {
        let mut inner = match Arc::try_unwrap(self.inner) {
            Ok(inner) => inner,
            Err(inner) => {
                return Err(ManagerReleaseFailure {
                    manager: VisaManager { inner },
                    error: DriverError::Responsibility(
                        "VISA manager still has dependent leases".into(),
                    ),
                })
            }
        };
        let code = inner.api.close(inner.handle);
        if code < 0 {
            return Err(ManagerReleaseFailure {
                manager: VisaManager {
                    inner: Arc::new(inner),
                },
                error: DriverError::Native {
                    operation: "viClose resource manager".into(),
                    status: code,
                    transferred: 0,
                },
            });
        }
        inner.handle = 0;
        Ok(CloseReport {
            released: true,
            status: Some(code),
        })
    }
    pub fn from_api(api: Arc<dyn VisaApi>, book: ResourceBook) -> DriverResult<Self> {
        let (code, handle) = api.open_manager();
        if let Err(error) = status("viOpenDefaultRM", code, 0) {
            if handle != 0 {
                retain(Retained::Manager(api, handle));
            }
            return Err(error);
        }
        if handle == 0 {
            return Err(DriverError::Protocol("null VISA manager".into()));
        }
        Ok(Self {
            inner: Arc::new(ManagerInner {
                api,
                handle,
                book,
                calls: Mutex::new(()),
            }),
        })
    }
    pub fn from_dependency(
        api: DriverResult<Arc<dyn VisaApi>>,
        book: ResourceBook,
    ) -> DriverResult<Self> {
        Self::from_api(api?, book)
    }
    /// Explicit read-only resource-manager stage; never opens instrument sessions.
    pub fn load_system(book: ResourceBook) -> DriverResult<Self> {
        Self::from_dependency(visa_abi::load_system(), book)
    }
    pub fn enumerate(&self) -> DriverResult<Vec<String>> {
        let _call = self
            .inner
            .calls
            .lock()
            .map_err(|_| DriverError::Responsibility("manager call poisoned".into()))?;
        let api = &self.inner.api;
        let (code, list, count, first) = api.find(self.inner.handle);
        let result = (|| {
            if code == VI_ERROR_RSRC_NFOUND && list == 0 {
                return Ok(Vec::new());
            }
            status("viFindRsrc", code, 0)?;
            if list == 0 || count == 0 || count > 4096 {
                return Err(DriverError::Protocol(
                    "invalid VISA inventory count/handle".into(),
                ));
            }
            let first = first?;
            valid_name(&first)?;
            let mut names = vec![first];
            for _ in 1..count {
                let next = api.find_next(list)?;
                valid_name(&next)?;
                names.push(next);
            }
            Ok(names)
        })();
        if list != 0 {
            let close = api.close(list);
            if close < 0 {
                retain(Retained::Handle(self.inner.clone(), list));
                return Err(DriverError::Responsibility(format!(
                    "VISA find-list close unconfirmed: {close}"
                )));
            }
        }
        result
    }
    pub fn canonicalize(&self, name: &str) -> DriverResult<CanonicalResource> {
        let name = name.trim();
        valid_name(name)?;
        let _call = self
            .inner
            .calls
            .lock()
            .map_err(|_| DriverError::Responsibility("manager call poisoned".into()))?;
        let resolved = self.inner.api.parse(self.inner.handle, name)?;
        valid_name(&resolved)?;
        if !resolved.contains("::") {
            return Err(DriverError::Protocol(
                "VISA alias did not resolve to a resource".into(),
            ));
        }
        Ok(CanonicalResource(resolved))
    }
    pub fn open(
        &self,
        resource: CanonicalResource,
        deadline: Deadline,
    ) -> DriverResult<VisaSession> {
        self.open_owned(resource, deadline)
            .map_err(|failure| failure.error)
    }
    /// Return any live handle even when the vendor reported an opening error.
    /// Ignoring it still retains the native session; lifecycle drivers keep it
    /// locally so explicit close can settle the exact failed-open attempt.
    pub fn open_owned(
        &self,
        resource: CanonicalResource,
        deadline: Deadline,
    ) -> Result<VisaSession, OpenFailure> {
        let no_session = |error| OpenFailure {
            error,
            session: None,
        };
        let timeout = deadline.remaining_millis().map_err(no_session)?;
        let mut reservation = self.inner.book.reserve(&resource).map_err(no_session)?;
        // No global registry lock is held during native I/O.
        let (code, handle) = self.inner.api.open(
            self.inner.handle,
            resource.as_str(),
            VI_EXCLUSIVE_LOCK,
            timeout,
        );
        if handle == 0 {
            reservation.release();
            status("viOpen", code, 0).map_err(no_session)?;
            return Err(no_session(DriverError::Protocol(
                "null VISA session".into(),
            )));
        }
        let session = VisaSession {
            inner: Arc::new(SessionInner {
                manager: self.inner.clone(),
                state: Mutex::new(SessionState {
                    handle: Some(handle),
                    reservation,
                    faulted: code < 0,
                }),
            }),
        };
        if let Err(error) = status("viOpen", code, 0) {
            return Err(OpenFailure {
                error,
                session: Some(session),
            });
        }
        if let Err(error) = deadline.remaining_millis() {
            return Err(OpenFailure {
                error,
                session: Some(session),
            });
        }
        Ok(session)
    }
}
impl VisaSession {
    pub fn disable_termination(&mut self) -> DriverResult<()> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| DriverError::Responsibility("session poisoned".into()))?;
        let handle = usable(&state)?;
        let result = status(
            "viSetAttribute(TERMCHAR_EN)",
            self.inner
                .manager
                .api
                .set_termination_enabled(handle, false),
            0,
        );
        if result.is_err() {
            state.faulted = true;
        }
        result
    }
    pub fn read_reply(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
        if maximum == 0 || maximum > 65536 {
            return Err(DriverError::Invalid(
                "VISA reply bound outside 1..65536".into(),
            ));
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| DriverError::Responsibility("session poisoned".into()))?;
        let handle = usable(&state)?;
        let api = &self.inner.manager.api;
        let result = (|| {
            let mut result = Vec::new();
            loop {
                status(
                    "viSetAttribute(TMO)",
                    api.set_timeout(handle, deadline.remaining_millis()?),
                    result.len(),
                )?;
                let mut fragment = vec![0; 4096.min(maximum + 1 - result.len())];
                let (code, count) = api.read(handle, &mut fragment);
                if count as usize > fragment.len() {
                    return Err(DriverError::Protocol("VISA fragment count overflow".into()));
                }
                result.extend_from_slice(&fragment[..count as usize]);
                status("viRead", code, result.len())?;
                deadline.remaining_millis()?;
                if result.len() > maximum {
                    return Err(DriverError::Protocol(
                        "VISA reply exceeded its bound".into(),
                    ));
                }
                if code == 0 || code == visa_abi::VI_SUCCESS_TERM_CHAR {
                    return Ok(result);
                }
                if code != visa_abi::VI_SUCCESS_MAX_CNT || count == 0 {
                    return Err(DriverError::Protocol(
                        "VISA reply completion/progress unknown".into(),
                    ));
                }
            }
        })();
        if result.is_err() {
            state.faulted = true;
        }
        result
    }
    pub fn close(&mut self) -> DriverResult<CloseReport> {
        close_session(&self.inner)
    }
    pub fn has_responsibility(&self) -> bool {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle
            .is_some()
    }
    pub fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<()> {
        self.write_internal(bytes, deadline, false)
    }
    /// Reserved for a typed OSA lifecycle's already-owned sweep abort. Never
    /// clears the normal-I/O fault fence or allows public raw recovery writes.
    pub(crate) fn abort_owned_sweep(&mut self, deadline: Deadline) -> DriverResult<()> {
        self.write_internal(b":ABORt\n", deadline, true)
    }
    fn write_internal(
        &mut self,
        bytes: &[u8],
        deadline: Deadline,
        cleanup: bool,
    ) -> DriverResult<()> {
        if bytes.is_empty() || bytes.len() > 65536 {
            return Err(DriverError::Invalid(
                "VISA write length outside 1..65536".into(),
            ));
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| DriverError::Responsibility("session call poisoned".into()))?;
        let handle = if cleanup {
            state.handle.ok_or(DriverError::Closed)?
        } else {
            usable(&state)?
        };
        let mut sent = 0;
        let result = (|| {
            while sent < bytes.len() {
                status(
                    "viSetAttribute(TMO)",
                    self.inner
                        .manager
                        .api
                        .set_timeout(handle, deadline.remaining_millis()?),
                    sent,
                )?;
                let (code, count) = self.inner.manager.api.write(handle, &bytes[sent..]);
                if count as usize > bytes.len() - sent {
                    return Err(DriverError::Protocol("VISA write count overflow".into()));
                }
                sent += count as usize;
                status("viWrite", code, sent)?;
                deadline.remaining_millis()?;
                if count == 0 {
                    return Err(DriverError::Protocol("VISA write made no progress".into()));
                }
            }
            Ok(())
        })();
        if result.is_err() {
            state.faulted = true;
        }
        result
    }
    pub fn read(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
        if maximum == 0 || maximum > 65536 {
            return Err(DriverError::Invalid(
                "VISA read length outside 1..65536".into(),
            ));
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| DriverError::Responsibility("session call poisoned".into()))?;
        let handle = usable(&state)?;
        let result = (|| {
            status(
                "viSetAttribute(TMO)",
                self.inner
                    .manager
                    .api
                    .set_timeout(handle, deadline.remaining_millis()?),
                0,
            )?;
            let mut bytes = vec![0u8; maximum];
            let (code, count) = self.inner.manager.api.read(handle, &mut bytes);
            if count as usize > maximum {
                return Err(DriverError::Protocol("VISA read count overflow".into()));
            }
            status("viRead", code, count as usize)?;
            bytes.truncate(count as usize);
            Ok(bytes)
        })();
        if result.is_err() {
            state.faulted = true;
        }
        result
    }
}
fn usable(state: &SessionState) -> DriverResult<u32> {
    if state.faulted {
        return Err(DriverError::Responsibility(
            "VISA I/O fault; close explicitly before reopening".into(),
        ));
    }
    state.handle.ok_or(DriverError::Closed)
}
fn close_session(inner: &SessionInner) -> DriverResult<CloseReport> {
    let mut state = inner
        .state
        .lock()
        .map_err(|_| DriverError::Responsibility("session call poisoned".into()))?;
    let Some(handle) = state.handle else {
        return Ok(CloseReport {
            released: true,
            status: None,
        });
    };
    state.faulted = true;
    let code = inner.manager.api.close(handle);
    if code >= 0 {
        state.handle = None;
        state.reservation.release();
    }
    Ok(CloseReport {
        released: code >= 0,
        status: Some(code),
    })
}
impl Drop for VisaSession {
    fn drop(&mut self) {
        if self.has_responsibility() {
            retain(Retained::Session(self.inner.clone()));
        }
    }
}
impl Drop for ManagerInner {
    fn drop(&mut self) {
        if self.handle != 0 && self.api.close(self.handle) < 0 {
            retain(Retained::Manager(self.api.clone(), self.handle));
        }
    }
}
/// Explicit retries, never called by a new connection or a destructor.
pub fn retained_count() -> usize {
    retained().lock().unwrap_or_else(|e| e.into_inner()).len()
}
pub fn retry_retained() -> usize {
    let pending = std::mem::take(&mut *retained().lock().unwrap_or_else(|e| e.into_inner()));
    for item in pending {
        let released = match &item {
            Retained::Session(inner) => close_session(inner).is_ok_and(|r| r.released),
            Retained::Handle(manager, handle) => manager.api.close(*handle) >= 0,
            Retained::Manager(api, handle) => api.close(*handle) >= 0,
        };
        if !released {
            retain(item);
        }
    }
    retained().lock().unwrap_or_else(|e| e.into_inner()).len()
}
