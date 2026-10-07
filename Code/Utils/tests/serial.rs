use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use yang_drivers::transport::serial::{canonical_com, enumerate_with};
use yang_drivers::transport::serial_abi::{SerialBackend, SerialIo};
use yang_drivers::transport::serial_discovery::DeviceRecord;
use yang_drivers::transport::{
    ByteTransport, CloseReport, Deadline, ResourceBook, SerialConfig, SerialSession,
};
use yang_drivers::{DriverError, DriverResult};
#[derive(Default)]
struct Calls {
    configuration_failed: bool,
    opens: usize,
    configs: Vec<u32>,
    writes: Vec<Vec<u8>>,
    reads: VecDeque<Vec<u8>>,
    pending: bool,
    cancel_requests: usize,
}
#[derive(Default)]
struct Backend(Arc<Mutex<Calls>>);
struct Io(Arc<Mutex<Calls>>);
impl SerialBackend for Backend {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> {
        Ok(vec![
            DeviceRecord {
                port: "COM12".into(),
                instance_id: "USB\\VID_10C4&PID_EA60\\SERIAL-B".into(),
                parents: vec![],
                description: "CP210x".into(),
            },
            DeviceRecord {
                port: "COM3".into(),
                instance_id: "USB\\VID_0403&PID_6001&MI_00\\6&generated".into(),
                parents: vec!["USB\\VID_0403&PID_6001\\2110148249-10".into()],
                description: "MDT".into(),
            },
        ])
    }
    fn open(&self, port: &str) -> DriverResult<Box<dyn SerialIo>> {
        assert_eq!(port, "\\\\.\\COM12");
        self.0.lock().unwrap().opens += 1;
        Ok(Box::new(Io(self.0.clone())))
    }
}
impl SerialIo for Io {
    fn configure(&mut self, config: &SerialConfig) -> DriverResult<()> {
        self.0.lock().unwrap().configs.push(config.baudrate);
        if self.0.lock().unwrap().configuration_failed {
            return Err(DriverError::Protocol(
                "finite failed DCB configuration".into(),
            ));
        }
        Ok(())
    }
    fn write(&mut self, bytes: &[u8], _: Deadline) -> DriverResult<usize> {
        self.0.lock().unwrap().writes.push(bytes.to_vec());
        Ok(bytes.len().min(2))
    }
    fn read(&mut self, maximum: usize, _: Deadline) -> DriverResult<Vec<u8>> {
        let mut calls = self.0.lock().unwrap();
        if calls.pending {
            return Err(DriverError::Timeout {
                operation: "read".into(),
                transferred: 0,
            });
        }
        let mut data = calls.reads.pop_front().unwrap();
        if data.len() > maximum {
            let suffix = data.split_off(maximum);
            calls.reads.push_front(suffix);
        }
        Ok(data)
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        let mut calls = self.0.lock().unwrap();
        calls.cancel_requests += 1;
        Ok(CloseReport {
            released: !calls.pending,
            status: None,
        })
    }
    fn has_pending(&self) -> bool {
        self.0.lock().unwrap().pending
    }
}
fn deadline() -> Deadline {
    Deadline::after(Duration::from_secs(1))
}
#[test]
fn failed_configuration_returns_its_live_serial_owner() {
    let backend = Arc::new(Backend::default());
    {
        let mut calls = backend.0.lock().unwrap();
        calls.configuration_failed = true;
        calls.pending = true;
    }
    let book = ResourceBook::isolated();
    let resource = canonical_com("COM12").unwrap();
    let Err(mut failure) = SerialSession::from_backend_owned(
        SerialConfig::instrument("COM12"),
        &book,
        backend.clone(),
    ) else {
        panic!("configuration must fail")
    };
    let session = failure
        .session
        .as_mut()
        .expect("failed configuration must return its owned native session");
    assert!(session.has_responsibility());
    assert!(!session.close().unwrap().released);
    assert!(book.is_reserved(&resource));
    backend.0.lock().unwrap().pending = false;
    assert!(session.close().unwrap().released);
    assert!(!book.is_reserved(&resource));
}
#[test]
fn com_alias_is_one_resource() {
    assert_eq!(
        canonical_com("COM12").unwrap(),
        canonical_com("\\\\.\\com012").unwrap()
    );
    for invalid in ["COM0", "COM-1", "COM12\\x", "C:/anything", "COM65536"] {
        assert!(canonical_com(invalid).is_err());
    }
    let book = ResourceBook::isolated();
    let backend = Arc::new(Backend::default());
    let mut session =
        SerialSession::from_backend(SerialConfig::instrument("COM12"), &book, backend.clone())
            .unwrap();
    assert!(matches!(
        SerialSession::from_backend(
            SerialConfig::instrument("\\\\.\\COM12"),
            &book,
            backend.clone()
        ),
        Err(DriverError::Busy(_))
    ));
    assert_eq!(backend.0.lock().unwrap().opens, 1);
    session.close().unwrap();
}
#[test]
fn usb_serial_not_port_order() {
    let devices = enumerate_with(&Backend::default()).unwrap();
    let left = devices
        .iter()
        .find(|d| d.serial == "2110148249-10")
        .unwrap();
    assert_eq!(left.resource, "COM3");
    assert_eq!(left.vid, Some(0x0403));
    assert_eq!(left.pid, Some(0x6001));
    let gain = devices.iter().find(|d| d.serial == "SERIAL-B").unwrap();
    assert_eq!(gain.vid, Some(0x10C4));
}
#[test]
fn partial_io_preserves_frames() {
    let backend = Arc::new(Backend::default());
    backend.0.lock().unwrap().reads =
        VecDeque::from([b"REA".to_vec(), b"DY;T=21.456\nNEXT".to_vec()]);
    let mut session = SerialSession::from_backend(
        SerialConfig::instrument("COM12"),
        &ResourceBook::isolated(),
        backend.clone(),
    )
    .unwrap();
    session.write_all(b"RDTA\n", deadline()).unwrap();
    assert_eq!(
        backend.0.lock().unwrap().writes,
        vec![b"RDTA\n".to_vec(), b"TA\n".to_vec(), b"\n".to_vec()]
    );
    let mut frame = session.read_bounded(3, deadline()).unwrap();
    frame.extend(session.read_bounded(13, deadline()).unwrap());
    assert_eq!(frame, b"READY;T=21.456\nN");
    assert_eq!(session.read_bounded(3, deadline()).unwrap(), b"EXT");
    session.close().unwrap();
}
#[test]
fn enumeration_opens_no_port() {
    let backend = Backend::default();
    assert_eq!(enumerate_with(&backend).unwrap().len(), 2);
    assert_eq!(backend.0.lock().unwrap().opens, 0);
}
#[test]
fn cancel_request_does_not_confirm_release() {
    let backend = Arc::new(Backend::default());
    let book = ResourceBook::isolated();
    let resource = canonical_com("COM12").unwrap();
    let mut session =
        SerialSession::from_backend(SerialConfig::instrument("COM12"), &book, backend.clone())
            .unwrap();
    backend.0.lock().unwrap().pending = true;
    assert!(session.read_bounded(16, deadline()).is_err());
    assert!(!session.close().unwrap().released);
    assert!(book.is_reserved(&resource));
    assert!(session.has_responsibility());
    drop(session);
    assert!(
        Arc::strong_count(&backend.0) > 1,
        "pending I/O owner/buffers must survive wrapper drop"
    );
    assert!(book.is_reserved(&resource));
    backend.0.lock().unwrap().pending = false;
    yang_drivers::transport::serial::retry_retained();
    assert!(!book.is_reserved(&resource));
}
#[test]
fn reviewed_protocols_use_115200_without_changing_framing() {
    for command in [
        b"RDTA\n".as_slice(),
        b"xvoltage?\r".as_slice(),
        b"\xAA\x55\0\0".as_slice(),
    ] {
        let backend = Arc::new(Backend::default());
        let mut session = SerialSession::from_backend(
            SerialConfig::instrument("COM12"),
            &ResourceBook::isolated(),
            backend.clone(),
        )
        .unwrap();
        session.write_all(command, deadline()).unwrap();
        assert_eq!(backend.0.lock().unwrap().configs, vec![115200]);
        session.close().unwrap();
    }
}
