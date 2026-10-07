use crate::WorkerError;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use yang_drivers::lifecycle::{CleanupReport, DriverState};
use yang_protocol::{valid_id, validate_config, ContextV3, DomainConfig, DomainRef, MAX_SEQUENCE};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomainSnapshot {
    pub context: ContextV3,
    pub config_rev: u64,
    pub state: DriverState,
    pub responsibility: bool,
    pub pending: usize,
}
#[derive(Clone)]
pub struct DomainRegistry {
    session_id: String,
    inner: Arc<Mutex<RegistryState>>,
}
#[derive(Default)]
struct RegistryState {
    domains: HashMap<DomainRef, DomainEntry>,
    retired: HashSet<DomainRef>,
}
struct DomainEntry {
    config: DomainConfig,
    snapshot: DomainSnapshot,
}
fn error(message: &str) -> WorkerError {
    WorkerError::new("DomainState", message)
}
fn bump(context: &mut ContextV3) -> Result<(), WorkerError> {
    if context.epoch >= MAX_SEQUENCE {
        return Err(error("domain epoch exhausted"));
    }
    context.epoch += 1;
    Ok(())
}
impl DomainRegistry {
    pub fn new(session_id: &str) -> Result<Self, WorkerError> {
        if !valid_id(session_id) {
            return Err(error("invalid worker session"));
        }
        Ok(Self {
            session_id: session_id.into(),
            inner: Arc::new(Mutex::new(RegistryState::default())),
        })
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn configure(&self, config: DomainConfig) -> Result<ContextV3, WorkerError> {
        validate_config(&config)?;
        let mut registry = self.inner.lock().map_err(|_| error("registry poisoned"))?;
        if registry.retired.contains(&config.domain) {
            return Err(error("retired domain identity cannot be reused"));
        }
        if let Some(previous) = registry.domains.get_mut(&config.domain) {
            if previous.snapshot.responsibility || previous.snapshot.pending != 0 {
                return Err(error("domain responsibility retained"));
            }
            if config.config_rev <= previous.config.config_rev {
                return Err(error("configuration revision must increase"));
            }
            let mut comparable = config.clone();
            comparable.config_rev = previous.config.config_rev;
            if serde_json::to_value(&comparable).unwrap()
                != serde_json::to_value(&previous.config).unwrap()
            {
                return Err(error("physical rebinding requires a new domain identity"));
            }
            previous.snapshot.config_rev = config.config_rev;
            previous.config = config;
            return Ok(previous.snapshot.context.clone());
        }
        if registry.domains.len() >= 64 {
            return Err(error("domain capacity is 64"));
        }
        let context = ContextV3 {
            session_id: self.session_id.clone(),
            domain: Some(config.domain.clone()),
            connection_id: None,
            epoch: 0,
        };
        let snapshot = DomainSnapshot {
            context: context.clone(),
            config_rev: config.config_rev,
            state: DriverState::Disconnected,
            responsibility: false,
            pending: 0,
        };
        registry
            .domains
            .insert(config.domain.clone(), DomainEntry { config, snapshot });
        Ok(context)
    }
    pub fn retire(&self, domain: &DomainRef, rev: u64) -> Result<(), WorkerError> {
        let mut registry = self.inner.lock().map_err(|_| error("registry poisoned"))?;
        let entry = registry
            .domains
            .get(domain)
            .ok_or_else(|| error("unknown domain"))?;
        if entry.snapshot.config_rev != rev
            || entry.snapshot.responsibility
            || entry.snapshot.pending != 0
        {
            return Err(error(
                "retire requires current revision, quiescence and confirmed release",
            ));
        }
        registry.retired.insert(domain.clone());
        registry.domains.remove(domain);
        Ok(())
    }
    pub fn snapshot(&self) -> Vec<DomainSnapshot> {
        let mut snapshots: Vec<_> = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .domains
            .values()
            .map(|e| e.snapshot.clone())
            .collect();
        snapshots.sort_by(|a, b| {
            a.context
                .domain
                .as_ref()
                .unwrap()
                .id
                .cmp(&b.context.domain.as_ref().unwrap().id)
        });
        snapshots
    }
    pub fn config(&self, domain: &DomainRef) -> Result<DomainConfig, WorkerError> {
        self.inner
            .lock()
            .map_err(|_| error("registry poisoned"))?
            .domains
            .get(domain)
            .map(|e| e.config.clone())
            .ok_or_else(|| error("unknown domain"))
    }
    pub fn context(&self, domain: &DomainRef) -> Result<ContextV3, WorkerError> {
        self.inner
            .lock()
            .map_err(|_| error("registry poisoned"))?
            .domains
            .get(domain)
            .map(|e| e.snapshot.context.clone())
            .ok_or_else(|| error("unknown domain"))
    }
    pub fn bind(&self, domain: &DomainRef, connection: &str) -> Result<ContextV3, WorkerError> {
        if !valid_id(connection) {
            return Err(error("invalid connection identity"));
        }
        let mut registry = self.inner.lock().map_err(|_| error("registry poisoned"))?;
        let entry = registry
            .domains
            .get_mut(domain)
            .ok_or_else(|| error("unknown domain"))?;
        if entry.snapshot.responsibility {
            return Err(error("old connection responsibility retained"));
        }
        bump(&mut entry.snapshot.context)?;
        entry.snapshot.context.connection_id = Some(connection.into());
        entry.snapshot.state = DriverState::Connecting;
        entry.snapshot.responsibility = true;
        Ok(entry.snapshot.context.clone())
    }
    pub fn fence(&self, domain: &DomainRef) -> Result<ContextV3, WorkerError> {
        let mut registry = self.inner.lock().map_err(|_| error("registry poisoned"))?;
        let entry = registry
            .domains
            .get_mut(domain)
            .ok_or_else(|| error("unknown domain"))?;
        bump(&mut entry.snapshot.context)?;
        entry.snapshot.state = DriverState::Closing;
        Ok(entry.snapshot.context.clone())
    }
    pub(crate) fn pending_add(&self, domain: &DomainRef) -> Result<(), WorkerError> {
        let mut registry = self.inner.lock().map_err(|_| error("registry poisoned"))?;
        let entry = registry
            .domains
            .get_mut(domain)
            .ok_or_else(|| error("unknown domain"))?;
        entry.snapshot.pending = entry
            .snapshot
            .pending
            .checked_add(1)
            .ok_or_else(|| error("pending count overflow"))?;
        Ok(())
    }
    pub(crate) fn pending_finish(&self, domain: &DomainRef) {
        let mut registry = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = registry.domains.get_mut(domain) {
            entry.snapshot.pending = entry.snapshot.pending.saturating_sub(1);
        }
    }
    pub fn matches(&self, context: &ContextV3) -> bool {
        context
            .domain
            .as_ref()
            .and_then(|d| self.context(d).ok())
            .as_ref()
            == Some(context)
    }
    pub fn publish(&self, context: &ContextV3, state: DriverState) -> bool {
        let mut registry = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = context
            .domain
            .as_ref()
            .and_then(|d| registry.domains.get_mut(d))
        else {
            return false;
        };
        if entry.snapshot.context != *context {
            return false;
        }
        entry.snapshot.state = state;
        true
    }
    pub fn release(&self, context: &ContextV3, report: &CleanupReport) -> Result<(), WorkerError> {
        let mut registry = self.inner.lock().map_err(|_| error("registry poisoned"))?;
        let entry = context
            .domain
            .as_ref()
            .and_then(|d| registry.domains.get_mut(d))
            .ok_or_else(|| error("unknown domain"))?;
        if entry.snapshot.context != *context
            || !report.resources_released()
            || entry.snapshot.pending != 0
        {
            return Err(error(
                "release requires current context, completed cleanup and quiescence",
            ));
        }
        bump(&mut entry.snapshot.context)?;
        entry.snapshot.context.connection_id = None;
        entry.snapshot.responsibility = false;
        entry.snapshot.state = DriverState::Disconnected;
        Ok(())
    }
}
