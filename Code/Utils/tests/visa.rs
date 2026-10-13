use std::sync::{Arc, Mutex};
use std::time::Duration;
use yang_drivers::transport::visa_abi::*;
use yang_drivers::transport::{Deadline, ResourceBook, VisaManager};
use yang_drivers::{DriverError, DriverResult};

#[derive(Default)]
struct Calls {
    open_calls: usize,
    closed: Vec<u32>,
    next: u32,
    fail_close: bool,
    fail_manager_close: bool,
    busy: bool,
    writes: Vec<Vec<u8>>,
    read_overflow: bool,
    read_status: i32,
    read_count: Option<u32>,
    reads: Vec<usize>,
}
#[derive(Default)]
struct Abi(Mutex<Calls>);
impl VisaApi for Abi {
    fn set_termination_enabled(&self, _: u32, enabled: bool) -> i32 {
        assert!(!enabled);
        0
    }
    fn open_manager(&self) -> (i32, u32) {
        (0, 1)
    }
    fn parse(&self, _: u32, name: &str) -> DriverResult<String> {
        Ok(match name {
            "osa" | "GPIB::4" | "GPIB0::4::INSTR" => "GPIB0::4::INSTR",
            _ => name,
        }
        .into())
    }
    fn find(&self, _: u32) -> (i32, u32, u32, DriverResult<String>) {
        (0, 2, 2, Ok("GPIB0::4::INSTR".into()))
    }
    fn find_next(&self, _: u32) -> DriverResult<String> {
        Ok("USB0::1::2::sensitiveSerial::0::INSTR".into())
    }
    fn open(&self, _: u32, _: &str, mode: u32, _: u32) -> (i32, u32) {
        assert_eq!(mode, VI_EXCLUSIVE_LOCK);
        let mut calls = self.0.lock().unwrap();
        calls.open_calls += 1;
        if calls.busy {
            (VI_ERROR_RSRC_BUSY, 0)
        } else {
            calls.next += 1;
            (0, 10 + calls.next)
        }
    }
    fn close(&self, handle: u32) -> i32 {
        let mut calls = self.0.lock().unwrap();
        calls.closed.push(handle);
        if (handle > 10 && calls.fail_close) || (handle == 1 && calls.fail_manager_close) {
            -1
        } else {
            0
        }
    }
    fn set_timeout(&self, _: u32, millis: u32) -> i32 {
        assert!(millis > 0 && millis < u32::MAX);
        0
    }
    fn write(&self, _: u32, bytes: &[u8]) -> (i32, u32) {
        self.0.lock().unwrap().writes.push(bytes.to_vec());
        (0, bytes.len().min(2) as u32)
    }
    fn read(&self, _: u32, bytes: &mut [u8]) -> (i32, u32) {
        let mut calls = self.0.lock().unwrap();
        calls.reads.push(bytes.len());
        if calls.read_overflow {
            return (0, bytes.len() as u32 + 1);
        }
        let count = calls.read_count.unwrap_or(2);
        let copied = (count as usize).min(bytes.len()).min(2);
        bytes[..copied].copy_from_slice(&b"OK"[..copied]);
        (calls.read_status, count)
    }
}

#[test]
fn manager_release_waits_for_all_dependents() {
    let api = Arc::new(Abi::default());
    let manager = VisaManager::from_api(api.clone(), ResourceBook::isolated()).unwrap();
    let borrowed = manager.clone();
    let failure = manager.release().unwrap_err();
    assert!(!api.0.lock().unwrap().closed.contains(&1));
    drop(borrowed);
    assert!(failure.manager.release().unwrap().released);
    assert_eq!(
        api.0
            .lock()
            .unwrap()
            .closed
            .iter()
            .filter(|h| **h == 1)
            .count(),
        1
    );
}
#[test]
fn manager_release_failure_remains_retryable() {
    let api = Arc::new(Abi::default());
    api.0.lock().unwrap().fail_manager_close = true;
    let manager = VisaManager::from_api(api.clone(), ResourceBook::isolated()).unwrap();
    let failure = manager.release().unwrap_err();
    let calls = api.0.lock().unwrap().closed.clone();
    assert_eq!(calls, vec![1]);
    api.0.lock().unwrap().fail_manager_close = false;
    assert!(failure.manager.release().unwrap().released);
    let calls = api.0.lock().unwrap().closed.clone();
    assert_eq!(calls, vec![1, 1]);
}
fn deadline() -> Deadline {
    Deadline::after(Duration::from_secs(1))
}
fn manager() -> (Arc<Abi>, ResourceBook, VisaManager) {
    let api = Arc::new(Abi::default());
    let book = ResourceBook::isolated();
    let manager = VisaManager::from_api(api.clone(), book.clone()).unwrap();
    (api, book, manager)
}
#[test]
fn alias_is_one_resource() {
    let (api, book, manager) = manager();
    let resource = manager.canonicalize("osa").unwrap();
    assert_eq!(resource, manager.canonicalize("GPIB::4").unwrap());
    let mut session = manager.open(resource.clone(), deadline()).unwrap();
    assert!(book.is_reserved(&resource));
    assert!(matches!(
        manager.open(resource.clone(), deadline()),
        Err(DriverError::Busy(_))
    ));
    assert_eq!(api.0.lock().unwrap().open_calls, 1);
    assert!(session.close().unwrap().released);
    assert!(!book.is_reserved(&resource));
}
#[test]
fn different_resources_remain_independent() {
    let (_, book, manager) = manager();
    let clone = manager.clone();
    let first = manager.canonicalize("osa").unwrap();
    let second = manager.canonicalize("GPIB0::5::INSTR").unwrap();
    let mut a = manager.open(first.clone(), deadline()).unwrap();
    let mut b = clone.open(second.clone(), deadline()).unwrap();
    a.close().unwrap();
    assert!(book.is_reserved(&second));
    assert!(b.has_responsibility());
    b.close().unwrap();
}
#[test]
fn enumeration_opens_no_session() {
    let (api, _, manager) = manager();
    assert_eq!(manager.enumerate().unwrap().len(), 2);
    let calls = api.0.lock().unwrap();
    assert_eq!(calls.open_calls, 0);
    assert_eq!(calls.closed, vec![2]);
}
#[test]
fn close_failure_keeps_manager_alive() {
    let (api, book, manager) = manager();
    let resource = manager.canonicalize("osa").unwrap();
    let mut session = manager.open(resource.clone(), deadline()).unwrap();
    api.0.lock().unwrap().fail_close = true;
    drop(manager);
    assert!(!session.close().unwrap().released);
    assert!(session.has_responsibility());
    assert!(book.is_reserved(&resource));
    assert!(!api.0.lock().unwrap().closed.contains(&1));
    api.0.lock().unwrap().fail_close = false;
    assert!(session.close().unwrap().released);
    drop(session);
    assert!(api.0.lock().unwrap().closed.contains(&1));
}
#[test]
fn external_busy_is_not_stolen() {
    let (api, book, manager) = manager();
    let resource = manager.canonicalize("osa").unwrap();
    api.0.lock().unwrap().busy = true;
    assert!(matches!(
        manager.open(resource.clone(), deadline()),
        Err(DriverError::Busy(_))
    ));
    assert!(!book.is_reserved(&resource));
    assert_eq!(api.0.lock().unwrap().open_calls, 1);
    assert!(api.0.lock().unwrap().closed.is_empty());
}
#[test]
fn missing_visa_is_dependency_unavailable() {
    let dependency: DriverResult<Arc<dyn VisaApi>> = Err(DriverError::DependencyUnavailable(
        "VISA x64 is not installed".into(),
    ));
    assert!(matches!(
        VisaManager::from_dependency(dependency, ResourceBook::isolated()),
        Err(DriverError::DependencyUnavailable(_))
    ));
}
#[test]
fn partial_writes_are_completed_and_counts_are_bounded() {
    let (api, _, manager) = manager();
    let resource = manager.canonicalize("osa").unwrap();
    let mut session = manager.open(resource, deadline()).unwrap();
    session.write_all(b"*IDN?\n", deadline()).unwrap();
    assert_eq!(
        api.0.lock().unwrap().writes,
        vec![b"*IDN?\n".to_vec(), b"DN?\n".to_vec(), b"?\n".to_vec()]
    );
    assert_eq!(session.read(16, deadline()).unwrap(), b"OK");
    api.0.lock().unwrap().read_overflow = true;
    assert!(matches!(
        session.read(16, deadline()),
        Err(DriverError::Protocol(_))
    ));
    session.close().unwrap();
}

// Catches dropping authoritative native message-end status or inventing it for MAX_CNT.
#[test]
fn read_fragment_preserves_only_native_message_end_and_vec_compatibility() {
    for (status, end_of_message) in [
        (0, true),
        (VI_SUCCESS_TERM_CHAR, true),
        (VI_SUCCESS_MAX_CNT, false),
        (1, false),
    ] {
        let (api, _, manager) = manager();
        let resource = manager.canonicalize("osa").unwrap();
        let mut session = manager.open(resource, deadline()).unwrap();
        api.0.lock().unwrap().read_status = status;
        let fragment = session.read_fragment(16, deadline()).unwrap();
        assert_eq!(fragment.bytes, b"OK");
        assert_eq!(
            fragment.end_of_message, end_of_message,
            "status {status:#x}"
        );
        assert_eq!(session.read(16, deadline()).unwrap(), b"OK");
        let calls = api.0.lock().unwrap();
        assert_eq!(calls.reads, vec![16, 16]);
        assert!(calls.writes.is_empty());
        drop(calls);
        assert!(session.close().unwrap().released);
    }
}

// Catches converting partial native failures into fragments or allowing I/O after the fault.
#[test]
fn read_fragment_preserves_partial_error_counts_and_fault_fence() {
    for (status, count) in [(VI_ERROR_TMO, 1), (0xBFFF0000u32 as i32, 2)] {
        let (api, _, manager) = manager();
        let resource = manager.canonicalize("osa").unwrap();
        let mut session = manager.open(resource, deadline()).unwrap();
        {
            let mut calls = api.0.lock().unwrap();
            calls.read_status = status;
            calls.read_count = Some(count);
        }
        let error = session.read_fragment(16, deadline()).err().unwrap();
        match error {
            DriverError::Timeout {
                operation,
                transferred,
            } => {
                assert_eq!(status, VI_ERROR_TMO);
                assert_eq!(operation, "viRead");
                assert_eq!(transferred, count as usize);
            }
            DriverError::Native {
                operation,
                status: actual,
                transferred,
            } => {
                assert_eq!(actual, status);
                assert_eq!(operation, "viRead");
                assert_eq!(transferred, count as usize);
            }
            other => panic!("unexpected native read error: {other:?}"),
        }
        assert!(matches!(
            session.read_fragment(16, deadline()),
            Err(DriverError::Responsibility(_))
        ));
        let calls = api.0.lock().unwrap();
        assert_eq!(calls.reads, vec![16]);
        assert!(calls.writes.is_empty());
        drop(calls);
        assert!(session.close().unwrap().released);
    }
}
