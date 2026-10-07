use crate::WorkerError;
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use yang_drivers::clock::Clock;
use yang_protocol::{finite_tree, validate_context, ContextV3, MAX_SEQUENCE};
#[derive(Clone, Debug)]
pub struct Observation {
    pub status: Value,
    pub more: bool,
    pub sampled_at: Option<Duration>,
}
pub struct EvidenceStore {
    context: ContextV3,
    clock: Arc<dyn Clock>,
    revision: u64,
    fields: BTreeMap<String, Field>,
}
struct Field {
    value: Value,
    source: Option<String>,
    quality: &'static str,
    started: Option<Duration>,
    revision: Option<u64>,
    floor: Option<u64>,
    reason: Option<String>,
}
const READERS: [(&str, &str); 5] = [
    ("temperature_c", "read_temperature"),
    ("target_c", "read_target"),
    ("tec_enabled", "read_tec_enabled"),
    ("current_ma", "read_current"),
    ("current_enabled", "read_current_enabled"),
];
fn error(message: &str) -> WorkerError {
    WorkerError::new("EvidenceError", message)
}
impl EvidenceStore {
    pub fn new(context: ContextV3, clock: Arc<dyn Clock>) -> Result<Self, WorkerError> {
        validate_context(&context)?;
        if context.connection_id.is_none() || context.domain.is_none() {
            return Err(error("evidence needs an owned domain context"));
        }
        let fields = READERS
            .iter()
            .map(|(name, _)| {
                (
                    (*name).into(),
                    Field {
                        value: Value::Null,
                        source: None,
                        quality: "unknown",
                        started: None,
                        revision: None,
                        floor: None,
                        reason: None,
                    },
                )
            })
            .collect();
        Ok(Self {
            context,
            clock,
            revision: 0,
            fields,
        })
    }
    pub fn record(
        &mut self,
        name: &str,
        value: Value,
        started: Duration,
        revision: u64,
    ) -> Result<(), WorkerError> {
        let field = self
            .fields
            .get_mut(name)
            .ok_or_else(|| error("unknown evidence field"))?;
        let is_switch = matches!(name, "tec_enabled" | "current_enabled");
        if (is_switch && !value.is_boolean())
            || (!is_switch && !value.as_f64().is_some_and(f64::is_finite))
            || revision > MAX_SEQUENCE
            || started > self.clock.now()
        {
            return Err(error("invalid evidence value/time/revision"));
        }
        if field.floor.is_some_and(|floor| revision <= floor)
            || field.started.is_some_and(|old| started < old)
        {
            return Ok(());
        }
        self.revision = self.revision.max(revision);
        field.floor = Some(revision);
        field.revision = Some(revision);
        field.started = Some(started);
        field.value = value;
        field.source = Some(READERS.iter().find(|(n, _)| *n == name).unwrap().1.into());
        field.quality = "fresh";
        field.reason = None;
        Ok(())
    }
    pub fn invalidate(&mut self, names: &[&str], reason: &str) -> Result<(), WorkerError> {
        if names.iter().any(|name| !self.fields.contains_key(*name))
            || reason.is_empty()
            || reason.len() > 4096
            || self.revision >= MAX_SEQUENCE
        {
            return Err(error("invalid evidence invalidation"));
        }
        self.revision += 1;
        for name in names {
            let field = self.fields.get_mut(*name).unwrap();
            field.floor = Some(self.revision);
            field.quality = "unknown";
            field.reason = Some(reason.into());
        }
        Ok(())
    }
    pub fn snapshot(&self) -> BTreeMap<String, Value> {
        let now = self.clock.now();
        self.fields.iter().map(|(name, field)| {
            let age = field.started.map(|started| now.saturating_sub(started).as_secs_f64());
            let quality = if field.quality == "fresh" && age.is_some_and(|age| age > 5.0) { "stale" } else { field.quality };
            let mut value = json!({"value":field.value,"source":field.source,"quality":quality,"connection_id":self.context.connection_id,"revision":field.revision,"observed_age_s":age});
            if let Some(reason) = &field.reason { value["reason"] = json!(reason); }
            (name.clone(), value)
        }).collect()
    }
}
impl Observation {
    pub fn validate(&self, now: Duration) -> Result<(), WorkerError> {
        if !self.status.is_object()
            || !finite_tree(&self.status)
            || self.sampled_at.is_some_and(|t| t > now)
        {
            return Err(error("invalid observation"));
        }
        if let Some(value) = self.status.get("observation_error") {
            if !value.as_object().is_some_and(|e| {
                e.len() == 2
                    && ["type", "message"].iter().all(|k| {
                        e.get(*k)
                            .and_then(Value::as_str)
                            .is_some_and(|v| !v.is_empty())
                    })
            }) {
                return Err(error("malformed observation error"));
            }
        }
        Ok(())
    }
}
