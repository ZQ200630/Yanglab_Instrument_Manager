//! One persistent owner thread for the global Newport SDK and process guard.
//! Only typed, bounded exchanges cross this boundary; raw SDK handles never do.
use std::{
    any::Any,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
        Arc, Condvar, Mutex,
    },
    time::Duration
};
use serde_json::Value;
use yang_drivers::clock::Clock;
use yang_lab_tlb::{self as tlb, Bus, Wire};

/// Explicit transport injection for finite offline checks, not an app backend.
pub struct OwnerWire {
    pub wire: Box<dyn Wire>,
    pub guard: Box<dyn Any>
}
pub trait NewportFactory: Send + Sync {
    fn create(&self) -> tlb::Result<OwnerWire>;
}
struct SystemNewport;
impl NewportFactory for SystemNewport {
    fn create(&self) -> tlb::Result<OwnerWire> {
        #[cfg(windows)] {
            let guard = tlb::sdk::OwnerGuard::acquire()?;
            Ok(OwnerWire {
                wire: Box::new(tlb::sdk::Sdk::default()),
                guard: Box::new(guard)
            })
        }
        #[cfg(not(windows))] {
            Err(error("connection", "Newport SDK requires Windows"))
        }
    }
}
struct ErasedWire(Box<dyn Wire>);
impl Wire for ErasedWire {
    fn open(&mut self) -> tlb::Result<Vec<String>> {
        self.0.open()
    }
    fn query(&mut self, key: &str, command: &str) -> tlb::Result<String> {
        self.0.query(key, command)
    }
    fn close(&mut self) -> tlb::Result<()> {
        self.0.close()
    }
}
struct Owned {
    bus: Bus<ErasedWire>,
    _guard: Box<dyn Any>
}
pub(crate) enum Command {
    Connect {
        key: String,
        limits: Option<tlb::ControlLimits>,
        full: bool
    },
    Status(String),
    Motion(String),
    FinishMove(String, f64, bool, bool),
    Legacy(String, tlb::Action),
    Control(String, tlb::Control),
    Disconnect(String),
    Discover,
    CloseAll,
}
impl Command {
    fn key(&self) -> Option<&str> {
        match self {
            Self::Connect { key, .. }
            | Self::Status(key)
            | Self::Motion(key)
            | Self::FinishMove(key, _, _, _)
            | Self::Legacy(key, _)
            | Self::Control(key, _)
            | Self::Disconnect(key) => Some(key),
            _ => None
        }
    }
}
pub(crate) enum Reply {
    Connected {
        identity: tlb::Identity,
        sample: Option<(Value, Duration)>
    },
    Sample(Value, Duration),
    MoveSample(Value, Duration, bool),
    Controllers(Vec<tlb::Identity>),
    Done,
}
pub(crate) struct Completion {
    pub result: tlb::Result<Reply>,
    pub responsibility: bool
}
#[derive(Default)]
struct Entry {
    entered: bool,
    canceled: bool,
    result: Option<Completion>
}
#[derive(Clone)]
pub(crate) struct Ticket(Arc<(Mutex<Entry>, Condvar)>);
impl Ticket {
    /// A timeout retains the same exchange. Queued work is canceled before entry;
    /// started native calls are never canceled, replayed or forcefully unloaded.
    pub(crate) fn wait(&self, timeout: Duration) -> tlb::Result<Completion> {
        let (lock, wake) = &*self.0;
        let entry = lock.lock().unwrap();
        let (mut entry, _) = wake
            .wait_timeout_while(entry, timeout, |e| e.result.is_none())
            .unwrap();
        if let Some(result) = entry.result.take() {
            return Ok(result);
        }
        if !entry.entered {
            entry.canceled = true;
        }
        Err(error(if entry.entered {
            "pending"
        } else {
            "canceled"
        }, "Native exchange remains pending; do not replay it"))
    }
}
struct Exchange {
    command: Command,
    ticket: Ticket,
    fence: Option<Arc<AtomicBool>>
}
struct Snapshot {
    released: AtomicBool,
    pending: AtomicUsize
}
pub(crate) struct Newport {
    factory: Arc<dyn NewportFactory>,
    clock: Arc<dyn Clock>,
    timeout: Duration,
    sender: Mutex<Option<SyncSender<Exchange>>>,
    snapshot: Arc<Snapshot>,
}
fn error(kind: &'static str, message: &str) -> tlb::Error {
    tlb::Error {
        kind,
        message: message.into()
    }
}
impl Newport {
    pub(crate) fn system(clock: Arc<dyn Clock>) -> Arc<Self> {
        Self::new(clock, Arc::new(SystemNewport), Duration::from_secs(15))
    }
    pub(crate) fn new(clock: Arc<dyn Clock>, factory: Arc<dyn NewportFactory>, timeout: Duration) -> Arc<Self> {
        Arc::new(Self {
            factory,
            clock,
            timeout,
            sender: Mutex::new(None),
            snapshot: Arc::new(Snapshot {
                released: AtomicBool::new(true),
                pending: AtomicUsize::new(0)
            })
        })
    }
    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }
    pub(crate) fn released(&self) -> bool {
        self.snapshot.pending.load(Ordering::Acquire) == 0 && self.snapshot.released.load(Ordering::Acquire)
    }
    pub(crate) fn submit(&self, command: Command) -> tlb::Result<Ticket> {
        self.submit_guarded(command, None)
    }
    pub(crate) fn submit_guarded(&self, command: Command, fence: Option<Arc<AtomicBool>>) -> tlb::Result<Ticket> {
        let mut sender = self.sender.lock().unwrap();
        if sender.is_none() {
            let (tx, rx) = mpsc::sync_channel::<Exchange>(32);
            let factory = self.factory.clone();
            let clock = self.clock.clone();
            let snapshot = self.snapshot.clone();
            std::thread::Builder::new().name("newport-owner".into()).spawn(move || {
                let mut owned: Option<Owned> = None;
                let mut poisoned = false;
                while let Ok(exchange) = rx.recv() {
                    let mut entry = exchange.ticket.0.0.lock().unwrap();
                    let canceled = entry.canceled || exchange.fence.as_ref().is_some_and(|flag| flag.load(Ordering::Acquire));
                    entry.entered = !canceled;
                    drop(entry);
                    let result = if canceled {
                        Err(error("canceled", "Exchange canceled before native entry"))
                    } else {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            if poisoned && !matches!(exchange.command, Command::Disconnect(_) | Command::CloseAll) {
                                return Err(error("connection", "Native owner fault remains retained"));
                            }
                            if owned.is_none() && matches!(exchange.command, Command::Connect {
                                ..
                            }
                            | Command::Discover) {
                                let OwnerWire {
                                    wire,
                                    guard
                                }
                                = factory.create()?;
                                snapshot.released.store(false, Ordering::Release);
                                owned = Some(Owned {
                                    bus: Bus::new(ErasedWire(wire)),
                                    _guard: guard
                                });
                            }
                            execute(owned.as_mut().map(|o| &mut o.bus), &exchange.command, &*clock)
                        })).unwrap_or_else(|_| {
                            poisoned = true;
                            Err(error("connection", "Native owner panic; responsibility retained"))
                        })
                    };
                    if !poisoned && owned.as_ref().is_some_and(|o| o.bus.resources_released()) {
                        // DLL and cross-process guard both drop on this same owner.
                        owned = None;
                    }
                    if result.is_ok()
                        && matches!(exchange.command, Command::CloseAll | Command::Disconnect(_))
                        && owned.as_ref().is_none_or(|o| o.bus.resources_released()) {
                        owned = None;
                        poisoned = false;
                    }
                    snapshot.released.store(owned.is_none() && !poisoned, Ordering::Release);
                    let responsibility = exchange.command.key().is_some_and(|key| {
                        owned.as_ref().is_some_and(|o| o.bus.session_responsibility(key))
                    }) || poisoned;
                    exchange.ticket.0.0.lock().unwrap().result = Some(Completion {
                        result,
                        responsibility
                    });
                    snapshot.pending.fetch_sub(1, Ordering::AcqRel);
                    exchange.ticket.0.1.notify_all();
                }
                // Channel loss never drops a live SDK/guard. One preserving attempt
                // is allowed after calls settle; failure retains this owner thread.
                if let Some(o) = &mut owned {
                    if poisoned || o.bus.close_all().is_err() {
                        loop {
                            std::thread::park();
                        }
                    }
                }
                drop(owned);
                snapshot.released.store(true, Ordering::Release);
            }).map_err(|_| error("connection", "Could not create native owner thread"))?;
            *sender = Some(tx);
        }
        let ticket = Ticket(Arc::new((Mutex::new(Entry::default()), Condvar::new())));
        self.snapshot.pending.fetch_add(1, Ordering::AcqRel);
        if sender.as_ref().unwrap().try_send(Exchange {
            command,
            ticket: ticket.clone(),
            fence
        }).is_err() {
            self.snapshot.pending.fetch_sub(1, Ordering::AcqRel);
            return Err(error("capacity", "Native owner queue unavailable"));
        }
        Ok(ticket)
    }
    pub(crate) fn call(&self, command: Command) -> tlb::Result<Reply> {
        self.submit(command)?.wait(self.timeout)?.result
    }
}
fn execute(bus: Option<&mut Bus<ErasedWire>>, command: &Command, clock: &dyn Clock) -> tlb::Result<Reply> {
    let Some(bus) = bus else {
        return if matches!(command, Command::Disconnect(_) | Command::CloseAll) {
            Ok(Reply::Done)
        } else {
            Err(error("connection", "No native SDK session"))
        };
    };
    Ok(match command {
        Command::Connect {
            key,
            limits,
            full
        }
        => {
            let identity = bus.connect(key)?;
            if let Some(limits) = limits {
                bus.set_limits(key, *limits)?;
            }
            let sample = if *full {
                let started = clock.now();
                Some((serde_json::to_value(bus.status(key)?).unwrap(), started))
            } else {
                None
            };
            Reply::Connected {
                identity,
                sample
            }
        }
        Command::Status(key) => {
            let started = clock.now();
            Reply::Sample(serde_json::to_value(bus.status(key)?).unwrap(), started)
        }
        Command::Motion(key) => {
            let started = clock.now();
            Reply::Sample(serde_json::to_value(bus.motion(key)?).unwrap(), started)
        }
        Command::FinishMove(key, target, check_setpoint, settled) => {
            let started = clock.now();
            let (sample, held) = bus.finish_move(key, *target, *check_setpoint, *settled)?;
            Reply::MoveSample(serde_json::to_value(sample).unwrap(), started, held)
        }
        Command::Legacy(key, a) => {
            bus.action(key, a.clone(), true)?;
            Reply::Done
        }
        Command::Control(key, a) => {
            bus.control(key, a.clone(), true)?;
            Reply::Done
        }
        Command::Disconnect(key) => {
            bus.disconnect(key)?;
            Reply::Done
        }
        Command::Discover => Reply::Controllers(bus.discover()?),
        Command::CloseAll => {
            bus.close_all()?;
            Reply::Done
        }
    })
}
