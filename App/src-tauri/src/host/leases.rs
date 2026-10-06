use super::{
    contracts::{valid_id, HostError, MAX_SEQUENCE},
    registry::new_id,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

pub const HEARTBEAT: Duration = Duration::from_secs(2);
pub const LEASE_TTL: Duration = Duration::from_secs(10);
static POWER_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) fn power_generation() -> u64 {
    POWER_GENERATION.load(std::sync::atomic::Ordering::Acquire)
}
unsafe extern "system" fn power_callback(
    _context: *const std::ffi::c_void,
    event: u32,
    _setting: *const std::ffi::c_void,
) -> u32 {
    if is_power_boundary(event) {
        let _ = POWER_GENERATION.fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |value| Some(value.saturating_add(1)),
        );
    }
    0
}
fn is_power_boundary(event: u32) -> bool {
    matches!(event, 4 | 7 | 18)
}
pub(crate) struct PowerObserver {
    handle: isize,
    parameters: isize,
}
impl PowerObserver {
    pub(crate) fn register() -> Result<Self, HostError> {
        use windows_sys::Win32::System::Power::{
            PowerRegisterSuspendResumeNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
        };
        let parameters = Box::into_raw(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(power_callback),
            Context: std::ptr::null_mut(),
        }));
        let mut handle = std::ptr::null_mut();
        let status = unsafe {
            PowerRegisterSuspendResumeNotification(
                2,
                parameters as *mut std::ffi::c_void,
                &mut handle,
            )
        };
        if status != 0 {
            unsafe { drop(Box::from_raw(parameters)) };
            return Err(HostError::new(
                "PowerNotification",
                format!("Cannot register system power notification: {status}"),
            ));
        }
        Ok(Self {
            handle: handle as isize,
            parameters: parameters as isize,
        })
    }
}
impl Drop for PowerObserver {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Power::{
            PowerUnregisterSuspendResumeNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
        };
        let status = unsafe { PowerUnregisterSuspendResumeNotification(self.handle) };
        // Callback uses only static storage, never this object or the parameter
        // pointer. Preserve registration storage if OS unregistration failed.
        if status == 0 {
            unsafe {
                drop(Box::from_raw(
                    self.parameters as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
                ))
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainRef {
    pub kind: String,
    pub id: String,
}
impl DomainRef {
    pub fn validate(&self) -> Result<(), HostError> {
        if valid_id(&self.id) && matches!(self.kind.as_str(), "device" | "setup") {
            Ok(())
        } else {
            Err(HostError::new(
                "DomainIdentity",
                "Invalid domain kind or identity",
            ))
        }
    }
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }
}
#[derive(Clone, Debug)]
pub struct Session {
    id: String,
    boot_id: String,
}
impl Session {
    pub(crate) fn local(id: String, boot_id: String) -> Result<Self, HostError> {
        if !valid_id(&id) || !valid_id(&boot_id) {
            return Err(HostError::new(
                "SessionIdentity",
                "Invalid local session identity",
            ));
        }
        Ok(Self { id, boot_id })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn boot_id(&self) -> &str {
        &self.boot_id
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Lease {
    pub token: String,
    pub boot_id: String,
    pub session_id: String,
    pub domain: DomainRef,
    pub control_epoch: u64,
    pub expires_in_ms: u64,
    #[serde(skip)]
    issued_at: Instant,
    #[serde(skip)]
    deadline: Instant,
}
impl Lease {
    pub fn admit_at(&self, now: Instant) -> Result<(), HostError> {
        if now < self.issued_at || now >= self.deadline {
            Err(HostError::new(
                "ControlExpired",
                "Lease is expired or its monotonic clock changed",
            ))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub enum RevokeCause {
    Released,
    Expired,
    ClientClosed,
    SafeStop,
    SystemResume,
    ClockDiscontinuity,
    HostStop,
}
#[derive(Clone, Debug, Serialize)]
pub struct CleanupTicket {
    pub ticket_id: String,
    pub boot_id: String,
    pub domain: DomainRef,
    pub control_epoch: u64,
    pub cause: RevokeCause,
    pub baseline_invalidated: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct Admission {
    pub domain: DomainRef,
    pub control_epoch: u64,
    pub session_id: String,
}
pub(crate) struct ReleaseEvidence {
    ticket_id: String,
    boot_id: String,
    domain: DomainRef,
    epoch: u64,
}
impl ReleaseEvidence {
    pub(crate) fn confirmed(ticket: &CleanupTicket) -> Self {
        Self {
            ticket_id: ticket.ticket_id.clone(),
            boot_id: ticket.boot_id.clone(),
            domain: ticket.domain.clone(),
            epoch: ticket.control_epoch,
        }
    }
}
#[derive(Default)]
struct DomainState {
    epoch: u64,
    owner: Option<Lease>,
    cleanup: Option<CleanupTicket>,
    cleanup_session: Option<String>,
    history: VecDeque<(String, bool)>,
    cleanup_count: usize,
}
pub struct LeaseBook {
    boot_id: String,
    domains: BTreeMap<DomainRef, DomainState>,
    last_clock: Option<Instant>,
}
impl LeaseBook {
    pub fn new(boot_id: &str) -> Result<Self, HostError> {
        if !valid_id(boot_id) {
            return Err(HostError::new("HostIdentity", "Invalid Host boot identity"));
        }
        Ok(Self {
            boot_id: boot_id.into(),
            domains: BTreeMap::new(),
            last_clock: None,
        })
    }
    pub fn register(&mut self, domain: &DomainRef) -> Result<(), HostError> {
        domain.validate()?;
        if !self.domains.contains_key(domain) && self.domains.len() >= 64 {
            return Err(HostError::new(
                "DomainCapacity",
                "Host execution-domain capacity is 64",
            ));
        }
        self.domains.entry(domain.clone()).or_default();
        Ok(())
    }
    pub(crate) fn forget_released(
        &mut self,
        domain: &DomainRef,
        verified: bool,
    ) -> Result<(), HostError> {
        if !verified
            || self
                .domains
                .get(domain)
                .is_some_and(|state| state.cleanup.is_some())
        {
            return Err(HostError::new(
                "ReleaseRequired",
                "Verified worker retirement and no pending cleanup are required",
            ));
        }
        self.domains.remove(domain);
        Ok(())
    }
    pub fn epoch(&self, domain: &DomainRef) -> Result<u64, HostError> {
        Ok(self
            .domains
            .get(domain)
            .ok_or_else(|| HostError::new("DomainUnknown", "Domain is not configured"))?
            .epoch)
    }
    pub fn cleanup_count(&self, domain: &DomainRef) -> usize {
        self.domains
            .get(domain)
            .map_or(0, |state| state.cleanup_count)
    }
    fn session(&self, session: &Session) -> Result<(), HostError> {
        if session.boot_id == self.boot_id {
            Ok(())
        } else {
            Err(HostError::new(
                "ControlDenied",
                "Session belongs to another Host boot",
            ))
        }
    }
    pub fn acquire(
        &mut self,
        session: &Session,
        domain: &DomainRef,
        now: Instant,
    ) -> Result<Lease, HostError> {
        self.session(session)?;
        self.tick(now)?;
        let state = self
            .domains
            .get_mut(domain)
            .ok_or_else(|| HostError::new("DomainUnknown", "Domain is not configured"))?;
        if state.cleanup.is_some() {
            return Err(HostError::new(
                "ControlRetained",
                "Previous responsibility has not confirmed release",
            ));
        }
        if state.owner.is_some() {
            return Err(HostError::new(
                "ControlOwned",
                "This domain already has a controller; no forced takeover",
            ));
        }
        if state.epoch >= MAX_SEQUENCE {
            return Err(HostError::new("ControlEpoch", "Control epoch exhausted"));
        }
        let lease = Lease {
            token: new_id()?,
            boot_id: self.boot_id.clone(),
            session_id: session.id.clone(),
            domain: domain.clone(),
            control_epoch: state.epoch,
            expires_in_ms: 10_000,
            issued_at: now,
            deadline: now + LEASE_TTL,
        };
        state.owner = Some(lease.clone());
        Ok(lease)
    }
    pub fn renew(
        &mut self,
        token: &str,
        session: &Session,
        now: Instant,
    ) -> Result<Lease, HostError> {
        self.session(session)?;
        self.tick(now)?;
        let state = self
            .domains
            .values_mut()
            .find(|state| {
                state
                    .owner
                    .as_ref()
                    .is_some_and(|lease| lease.token == token && lease.session_id == session.id)
            })
            .ok_or_else(|| {
                HostError::new(
                    "ControlDenied",
                    "Control token or client session does not match",
                )
            })?;
        let lease = state.owner.as_mut().unwrap();
        lease.admit_at(now)?;
        lease.deadline = now + LEASE_TTL;
        Ok(lease.clone())
    }
    pub(crate) fn admit(
        &mut self,
        token: &str,
        session: &Session,
        domain: &DomainRef,
        epoch: u64,
        now: Instant,
    ) -> Result<Admission, HostError> {
        self.session(session)?;
        self.tick(now)?;
        let state = self
            .domains
            .get(domain)
            .ok_or_else(|| HostError::new("DomainUnknown", "Domain is not configured"))?;
        let lease = state
            .owner
            .as_ref()
            .ok_or_else(|| HostError::new("ControlDenied", "No current lease"))?;
        if state.cleanup.is_some()
            || lease.token != token
            || lease.session_id != session.id
            || epoch != state.epoch
        {
            return Err(HostError::new(
                "ControlDenied",
                "Control token, session or epoch changed",
            ));
        }
        lease.admit_at(now)?;
        Ok(Admission {
            domain: domain.clone(),
            control_epoch: epoch,
            session_id: session.id.clone(),
        })
    }
    pub fn revoke(
        &mut self,
        domain: &DomainRef,
        cause: RevokeCause,
    ) -> Result<CleanupTicket, HostError> {
        let state = self
            .domains
            .get_mut(domain)
            .ok_or_else(|| HostError::new("DomainUnknown", "Domain is not configured"))?;
        if let Some(ticket) = &state.cleanup {
            return Ok(ticket.clone());
        }
        // Fence before any cleanup work is sent. No driver/I/O call under this lock.
        state.cleanup_session = state.owner.take().map(|l| l.session_id);
        state.epoch = state.epoch.saturating_add(1).min(MAX_SEQUENCE);
        let ticket = CleanupTicket {
            ticket_id: new_id()?,
            boot_id: self.boot_id.clone(),
            domain: domain.clone(),
            control_epoch: state.epoch,
            baseline_invalidated: domain.kind == "setup",
            cause,
        };
        state.cleanup = Some(ticket.clone());
        state.cleanup_count += 1;
        Ok(ticket)
    }
    pub fn close_session(&mut self, session: &Session) -> Result<Vec<CleanupTicket>, HostError> {
        self.session(session)?;
        let domains = self
            .domains
            .iter()
            .filter(|(_, state)| {
                state
                    .owner
                    .as_ref()
                    .is_some_and(|lease| lease.session_id == session.id)
                    || state.cleanup.is_some()
                        && state.cleanup_session.as_deref() == Some(session.id())
            })
            .map(|(domain, _)| domain.clone())
            .collect::<Vec<_>>();
        domains
            .iter()
            .map(|domain| self.revoke(domain, RevokeCause::ClientClosed))
            .collect()
    }
    pub(crate) fn session_release_confirmed(&self,session:&Session)->Result<bool,HostError> {
        self.session(session)?;
        Ok(!self.domains.values().any(|state|state.owner.as_ref().is_some_and(|l|l.session_id==session.id())||state.cleanup_session.as_deref()==Some(session.id())))
    }
    pub fn system_resume(&mut self) -> Result<Vec<CleanupTicket>, HostError> {
        self.revoke_all(RevokeCause::SystemResume)
    }
    pub fn revoke_all(&mut self, cause: RevokeCause) -> Result<Vec<CleanupTicket>, HostError> {
        let domains = self
            .domains
            .iter()
            .filter(|(_, state)| state.owner.is_some())
            .map(|(domain, _)| domain.clone())
            .collect::<Vec<_>>();
        domains
            .iter()
            .map(|domain| self.revoke(domain, cause.clone()))
            .collect()
    }
    pub fn tick(&mut self, now: Instant) -> Result<Vec<CleanupTicket>, HostError> {
        if self.last_clock.is_some_and(|old| now < old) {
            self.last_clock = Some(now);
            return self.revoke_all(RevokeCause::ClockDiscontinuity);
        }
        self.last_clock = Some(now);
        let expired = self
            .domains
            .iter()
            .filter(|(_, state)| {
                state
                    .owner
                    .as_ref()
                    .is_some_and(|lease| lease.admit_at(now).is_err())
            })
            .map(|(domain, _)| domain.clone())
            .collect::<Vec<_>>();
        expired
            .iter()
            .map(|domain| self.revoke(domain, RevokeCause::Expired))
            .collect()
    }
    pub fn pending_cleanups(&self) -> Vec<CleanupTicket> {
        self.domains
            .values()
            .filter_map(|state| state.cleanup.clone())
            .collect()
    }
    pub(crate) fn complete_cleanup(&mut self, evidence: ReleaseEvidence) -> Result<(), HostError> {
        let state = self
            .domains
            .get_mut(&evidence.domain)
            .ok_or_else(|| HostError::new("DomainUnknown", "Domain no longer exists"))?;
        let ticket = state
            .cleanup
            .as_ref()
            .ok_or_else(|| HostError::new("CleanupIdentity", "No pending cleanup"))?;
        if ticket.ticket_id != evidence.ticket_id
            || ticket.boot_id != evidence.boot_id
            || ticket.control_epoch != evidence.epoch
        {
            return Err(HostError::new(
                "CleanupIdentity",
                "Late cleanup cannot release new responsibility",
            ));
        }
        state.history.push_back((ticket.ticket_id.clone(), true));
        if state.history.len() > 16 {
            state.history.pop_front();
        }
        state.cleanup = None;
        state.cleanup_session = None;
        Ok(())
    }
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::Value::Object(self.domains.iter().map(|(domain,state)|(domain.key(),serde_json::json!({
            "control_epoch":state.epoch,"controller_session":state.owner.as_ref().map(|lease|&lease.session_id),
            "state":if state.cleanup.is_some(){"RETAINED"}else if state.owner.is_some(){"CONTROLLED"}else{"AVAILABLE"},
            "cleanup":state.cleanup}))).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readonly_session_reconciliation_requires_completed_owned_cleanup() {
        let boot="b".repeat(32);let mut book=LeaseBook::new(&boot).unwrap();let d=domain(1);book.register(&d).unwrap();
        let s=Session::local("a".repeat(32),boot.clone()).unwrap();let other=Session::local("c".repeat(32),boot.clone()).unwrap();
        assert!(book.session_release_confirmed(&s).unwrap());book.acquire(&s,&d,Instant::now()).unwrap();
        assert!(!book.session_release_confirmed(&s).unwrap());assert!(book.session_release_confirmed(&other).unwrap());
        let ticket=book.close_session(&s).unwrap().remove(0);assert!(!book.session_release_confirmed(&s).unwrap());
        book.complete_cleanup(ReleaseEvidence::confirmed(&ticket)).unwrap();assert!(book.session_release_confirmed(&s).unwrap());
        assert!(book.session_release_confirmed(&Session::local(s.id().into(),"d".repeat(32)).unwrap()).is_err());
    }
    fn domain(number: u64) -> DomainRef {
        DomainRef {
            kind: "device".into(),
            id: format!("{number:032x}"),
        }
    }
    #[test]
    fn cancelled_domain_cannot_be_reacquired_and_unconfirmed_retirement_keeps_it() {
        let mut book = LeaseBook::new(&"b".repeat(32)).unwrap();
        let d = domain(1);
        book.register(&d).unwrap();
        let s = Session::local("a".repeat(32), "b".repeat(32)).unwrap();
        let lease = book.acquire(&s, &d, Instant::now()).unwrap();
        assert!(book.forget_released(&d, false).is_err());
        assert_eq!(book.epoch(&d).unwrap(), lease.control_epoch);
        book.forget_released(&d, true).unwrap();
        assert!(book.acquire(&s, &d, Instant::now()).is_err());
    }
    #[test]
    fn two_clients_have_one_writer() {
        let boot = "b".repeat(32);
        let mut book = LeaseBook::new(&boot).unwrap();
        let ref_ = domain(1);
        book.register(&ref_).unwrap();
        let a = Session::local("a".repeat(32), boot.clone()).unwrap();
        let b = Session::local("c".repeat(32), boot).unwrap();
        let now = Instant::now();
        let lease = book.acquire(&a, &ref_, now).unwrap();
        assert_eq!(
            book.acquire(&b, &ref_, now).unwrap_err().code,
            "ControlOwned"
        );
        assert_eq!(
            book.renew(&lease.token, &b, now).unwrap_err().code,
            "ControlDenied"
        );
        assert!(book
            .admit(&lease.token, &a, &ref_, lease.control_epoch, now)
            .is_ok());
    }
    #[test]
    fn expired_lease_fences_before_dispatch() {
        let boot = "b".repeat(32);
        let mut book = LeaseBook::new(&boot).unwrap();
        let one = domain(1);
        let two = domain(2);
        book.register(&one).unwrap();
        book.register(&two).unwrap();
        let session = Session::local("a".repeat(32), boot).unwrap();
        let now = Instant::now();
        let lease = book.acquire(&session, &one, now).unwrap();
        assert!(lease.admit_at(now + Duration::from_secs(10)).is_err());
        assert!(book
            .admit(
                &lease.token,
                &session,
                &one,
                lease.control_epoch,
                now + Duration::from_secs(10)
            )
            .is_err());
        assert!(book.epoch(&one).unwrap() > lease.control_epoch);
        assert_eq!(book.cleanup_count(&two), 0);
        assert_eq!(
            book.acquire(&session, &one, now + Duration::from_secs(11))
                .unwrap_err()
                .code,
            "ControlRetained"
        );
    }
    #[test]
    fn sleep_resume_revokes_without_restoring_baseline() {
        let boot = "b".repeat(32);
        let mut book = LeaseBook::new(&boot).unwrap();
        let fiber = DomainRef {
            kind: "setup".into(),
            id: "f".repeat(32),
        };
        book.register(&fiber).unwrap();
        let session = Session::local("a".repeat(32), boot).unwrap();
        let now = Instant::now();
        let lease = book.acquire(&session, &fiber, now).unwrap();
        let tickets = book.system_resume().unwrap();
        assert_eq!(tickets.len(), 1);
        assert!(tickets[0].baseline_invalidated);
        assert!(book
            .admit(&lease.token, &session, &fiber, lease.control_epoch, now)
            .is_err());
        assert!(book.acquire(&session, &fiber, now).is_err());
    }
    #[test]
    fn windows_power_observer_registers_without_a_gui_window() {
        let observer = PowerObserver::register().unwrap();
        assert_ne!(observer.handle, 0);
        for event in [4, 7, 18] {
            assert!(is_power_boundary(event));
        }
        for event in [0, 1, 2, 3, 9] {
            assert!(!is_power_boundary(event));
        }
    }
}
