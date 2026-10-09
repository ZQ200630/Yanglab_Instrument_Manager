use super::*;
use crate::transport::{serial_abi::SerialIo, serial_discovery::DeviceRecord, CloseReport};
use std::{collections::VecDeque, sync::atomic::AtomicUsize};

// Start with a due poll and an already admitted safety job, without depending
// on thread scheduling between the public generation fence and queue admission.
struct DueClock(AtomicUsize);
impl Clock for DueClock {
    fn now(&self) -> Duration {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 { Duration::ZERO } else { Duration::from_secs(1) }
    }
    fn wait(&self, _: Duration) { panic!("actor clock must not sleep"); }
}
#[derive(Default)]
struct Frames {
    commands: Vec<Vec<u8>>,
    pending: VecDeque<u8>,
}
struct Backend(Arc<Mutex<Frames>>);
struct Io(Arc<Mutex<Frames>>);
impl SerialBackend for Backend {
    fn enumerate(&self) -> DriverResult<Vec<DeviceRecord>> { panic!("injected actor test never enumerates"); }
    fn open(&self, _: &str) -> DriverResult<Box<dyn SerialIo>> { Ok(Box::new(Io(self.0.clone()))) }
}
impl SerialIo for Io {
    fn configure(&mut self, _: &SerialConfig) -> DriverResult<()> { Ok(()) }
    fn write(&mut self, bytes: &[u8], _: Deadline) -> DriverResult<usize> {
        let mut frames = self.0.lock().unwrap();
        assert!(frames.pending.is_empty());
        frames.commands.push(bytes.to_vec());
        let reply = match bytes {
            b"STQA000000\r\n" | b"RDQA\r\n" => "READY;Q=0\r\n",
            b"STRA000000\r\n" => "READY;D=0\r\n",
            b"RDTA\r\n" => "READY;T=22.000\r\n",
            b"RDEA\r\n" => "READY;E=22.000\r\n",
            b"RDRA\r\n" => "READY;R=0\r\n",
            b"RDCA\r\n" => "READY;C=0.000\r\n",
            _ => panic!("unexpected actor frame {bytes:?}"),
        };
        frames.pending.extend(reply.bytes());
        Ok(bytes.len())
    }
    fn read(&mut self, maximum: usize, _: Deadline) -> DriverResult<Vec<u8>> {
        let mut frames = self.0.lock().unwrap();
        let count = maximum.min(frames.pending.len());
        Ok(frames.pending.drain(..count).collect())
    }
    fn close(&mut self) -> DriverResult<CloseReport> { Ok(CloseReport { released: true, status: None }) }
    fn has_pending(&self) -> bool { false }
}

#[test]
fn admitted_current_off_precedes_an_already_due_background_snapshot() {
    let frames = Arc::new(Mutex::new(Frames::default()));
    let backend = Arc::new(Backend(frames.clone()));
    let driver = GainDriver::with_backend(
        GainConfig { port: Some("COM13".into()), ..Default::default() },
        ResourceBook::isolated(), Arc::new(DueClock(AtomicUsize::new(0))), backend.clone(),
    ).unwrap();
    let shared = driver.shared.clone();
    shared.state.lock().unwrap().state = DriverState::Ready;
    let mut io = SerialSession::from_backend_owned(
        SerialConfig::instrument("COM13"), &driver.book, backend,
    ).map_err(|failure| failure.error).unwrap();
    let (_normal_tx, normal) = mpsc::sync_channel(1);
    let (safety_tx, safety) = mpsc::sync_channel(1);
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    safety_tx.send(Job {
        op: Op::Flag(b'Q', false), generation: 0,
        deadline: Deadline::after(Duration::from_secs(2)), reply: reply_tx,
    }).unwrap();
    let actor_shared = shared.clone();
    let worker = std::thread::spawn(move || actor(&actor_shared, &mut io, normal, safety));
    assert!(matches!(reply_rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(), Reply::Flag(false)));
    shared.close_request.store(1, Ordering::Release);
    worker.join().unwrap();
    assert_eq!(frames.lock().unwrap().commands.first().unwrap(), b"STQA000000\r\n",
        "an admitted Off must not wait behind five background queries");
}
