use crate::{
    transport::{ByteTransport, CloseReport, Deadline},
    DriverError, DriverResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DriverState {
    Disconnected,
    Connecting,
    Ready,
    Active,
    Closing,
    Fault,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupStep {
    pub role: String,
    pub action: String,
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CleanupReport {
    attempt_id: String,
    steps: Vec<CleanupStep>,
    voltage_zero: Option<Value>,
    unreleased: Vec<String>,
}
impl CleanupReport {
    pub fn new(
        attempt_id: String,
        steps: Vec<CleanupStep>,
        voltage_zero: Option<Value>,
        unreleased: Vec<String>,
    ) -> DriverResult<Self> {
        if attempt_id.len() != 32
            || !attempt_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || steps.is_empty()
            || steps.len() > 256
            || unreleased.len() > 64
        {
            return Err(DriverError::Invalid("invalid cleanup attempt/size".into()));
        }
        if steps.iter().any(|s| {
            s.role.is_empty()
                || s.role.len() > 128
                || s.action.is_empty()
                || s.action.len() > 128
                || s.error.as_ref().is_some_and(|e| e.len() > 4096)
        }) || unreleased.iter().any(|s| s.is_empty() || s.len() > 256)
        {
            return Err(DriverError::Invalid(
                "invalid cleanup step/responsibility".into(),
            ));
        }
        if voltage_zero
            .as_ref()
            .is_some_and(|v| serde_json::to_vec(v).map_or(true, |b| b.len() > 8192))
        {
            return Err(DriverError::Invalid("oversized zero evidence".into()));
        }
        Ok(Self {
            attempt_id,
            steps,
            voltage_zero,
            unreleased,
        })
    }
    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }
    pub fn steps(&self) -> &[CleanupStep] {
        &self.steps
    }
    pub fn voltage_zero(&self) -> Option<&Value> {
        self.voltage_zero.as_ref()
    }
    pub fn unreleased(&self) -> &[String] {
        &self.unreleased
    }
    pub fn resources_released(&self) -> bool {
        self.unreleased.is_empty() && !self.steps.is_empty()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeReport {
    identity: Value,
    observations: Value,
    release_confirmed: bool,
}
impl ProbeReport {
    pub fn new(identity: Value, observations: Value, release_confirmed: bool) -> Self {
        Self {
            identity,
            observations,
            release_confirmed,
        }
    }
    pub fn identity(&self) -> &Value {
        &self.identity
    }
    pub fn observations(&self) -> &Value {
        &self.observations
    }
    pub fn release_confirmed(&self) -> bool {
        self.release_confirmed
    }
}
pub trait DriverLifecycle {
    fn close(&mut self) -> DriverResult<CleanupReport>;
    fn has_responsibility(&self) -> bool;
}

#[derive(Clone)]
pub struct ResponsibilityRegistry(Arc<Mutex<Vec<Arc<RetainedSlot>>>>);
struct RetainedSlot {
    io: Mutex<Box<dyn ByteTransport>>,
    released: AtomicBool,
}
impl Default for ResponsibilityRegistry {
    fn default() -> Self {
        static REGISTRY: OnceLock<ResponsibilityRegistry> = OnceLock::new();
        REGISTRY
            .get_or_init(|| Self(Arc::new(Mutex::new(Vec::new()))))
            .clone()
    }
}
pub struct OwnedTransport {
    io: Option<Box<dyn ByteTransport>>,
    registry: ResponsibilityRegistry,
}
impl ResponsibilityRegistry {
    pub fn adopt(&self, io: Box<dyn ByteTransport>) -> OwnedTransport {
        OwnedTransport {
            io: Some(io),
            registry: self.clone(),
        }
    }
    pub fn pending(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|slot| !slot.released.load(Ordering::Acquire))
            .count()
    }
    pub fn retry(&self, deadline: Deadline) -> DriverResult<usize> {
        // Keep slots in the registry during close; an in-flight call remains visible.
        let slots = self
            .0
            .lock()
            .map_err(|_| DriverError::Responsibility("responsibility registry poisoned".into()))?
            .clone();
        for slot in slots {
            deadline.remaining_millis()?;
            if slot.released.load(Ordering::Acquire) {
                continue;
            }
            let mut io = slot
                .io
                .lock()
                .map_err(|_| DriverError::Responsibility("retained transport poisoned".into()))?;
            if slot.released.load(Ordering::Acquire) {
                continue;
            }
            if io.close().is_ok_and(|r| r.released) && !io.has_responsibility() {
                slot.released.store(true, Ordering::Release);
            }
        }
        self.0
            .lock()
            .map_err(|_| DriverError::Responsibility("responsibility registry poisoned".into()))?
            .retain(|slot| !slot.released.load(Ordering::Acquire));
        Ok(self.pending())
    }
}
impl ByteTransport for OwnedTransport {
    fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<()> {
        self.io.as_mut().unwrap().write_all(bytes, deadline)
    }
    fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>> {
        self.io.as_mut().unwrap().read_bounded(maximum, deadline)
    }
    fn close(&mut self) -> DriverResult<CloseReport> {
        self.io.as_mut().unwrap().close()
    }
    fn has_responsibility(&self) -> bool {
        self.io.as_ref().unwrap().has_responsibility()
    }
}
impl Drop for OwnedTransport {
    fn drop(&mut self) {
        if let Some(io) = self.io.take() {
            let responsible =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| io.has_responsibility()))
                    .unwrap_or(true);
            if responsible {
                let slot = Arc::new(RetainedSlot {
                    io: Mutex::new(io),
                    released: AtomicBool::new(false),
                });
                self.registry
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(slot);
            }
        }
    }
}
