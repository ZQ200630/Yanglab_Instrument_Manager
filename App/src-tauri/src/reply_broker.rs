//! Correlated terminal evidence, independent of the child process.
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Mutex,
};

pub const MAX_PENDING: usize = 41;
#[derive(Debug)]
pub(crate) enum RegistrationError {
    Full,
    Other(String),
}
struct Pending {
    sender: Option<Sender<Value>>,
    role: Option<String>,
    timed_out: bool,
    context: Option<Value>,
}
#[derive(Default)]
struct State {
    pending: HashMap<String, Pending>,
    terminal: VecDeque<(String, Option<String>, Value)>,
    restricted: HashSet<String>,
    generation: u64,
    failure: Option<String>,
    contexts: HashMap<String, Value>,
}
pub struct ReplyBroker {
    state: Mutex<State>,
    protocol: u64,
}
impl Default for ReplyBroker {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            protocol: 2,
        }
    }
}
impl ReplyBroker {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn for_protocol(protocol: u64) -> Result<Self, String> {
        if ![2, 3].contains(&protocol) {
            return Err("Unsupported worker protocol".into());
        }
        Ok(Self {
            state: Mutex::new(State::default()),
            protocol,
        })
    }
    pub fn liability_capacity(&self) -> usize {
        if self.protocol == 3 {
            225
        } else {
            MAX_PENDING
        }
    }
    pub fn protocol(&self) -> u64 {
        self.protocol
    }
    pub(crate) fn register_runtime(
        &self,
        id: &str,
        role: Option<&str>,
        context: &Value,
    ) -> Result<Receiver<Value>, RegistrationError> {
        if self.protocol == 3 {
            self.register_domain_checked(id, context)
        } else {
            self.register_checked(id, role)
        }
    }
    pub(crate) fn register_domain_checked(
        &self,
        id: &str,
        context: &Value,
    ) -> Result<Receiver<Value>, RegistrationError> {
        if self.protocol != 3 || !valid_context_v3(context) {
            return Err(RegistrationError::Other(
                "Invalid v3 request context".into(),
            ));
        }
        let domain = domain_key(context);
        self.register_internal(id, domain.as_deref(), Some(context.clone()))
    }
    #[cfg(test)]
    pub fn register_domain(&self, id: &str, context: &Value) -> Result<Receiver<Value>, String> {
        self.register_domain_checked(id, context)
            .map_err(|error| format!("{error:?}"))
    }
    #[cfg(test)]
    pub fn register(&self, id: &str, role: Option<&str>) -> Result<Receiver<Value>, String> {
        self.register_checked(id, role).map_err(|e| match e {
            RegistrationError::Full => "pending reply capacity exhausted".into(),
            RegistrationError::Other(message) => message,
        })
    }
    pub(crate) fn register_checked(
        &self,
        id: &str,
        role: Option<&str>,
    ) -> Result<Receiver<Value>, RegistrationError> {
        self.register_internal(id, role, None)
    }
    fn register_internal(
        &self,
        id: &str,
        role: Option<&str>,
        context: Option<Value>,
    ) -> Result<Receiver<Value>, RegistrationError> {
        let mut state = self.state.lock().unwrap();
        if let Some(error) = &state.failure {
            return Err(RegistrationError::Other(error.clone()));
        }
        if id.is_empty() || id.chars().count() > 64 {
            return Err(RegistrationError::Other("invalid request ID".into()));
        }
        if state.pending.contains_key(id) || state.terminal.iter().any(|(old, _, _)| old == id) {
            return Err(RegistrationError::Other("duplicate request ID".into()));
        }
        if state.pending.len() >= self.liability_capacity() {
            return Err(RegistrationError::Full);
        }
        let (sender, receiver) = mpsc::channel();
        state.pending.insert(
            id.into(),
            Pending {
                sender: Some(sender),
                role: role.map(str::to_owned),
                timed_out: false,
                context,
            },
        );
        Ok(receiver)
    }
    pub fn deliver(&self, frame: Value) -> Result<(), String> {
        if let Err(error) = self.validate_delivery(&frame) {
            self.transport_failed(&error);
            return Err(error);
        }
        let id = frame["id"].as_str().unwrap();
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = state.pending.remove(id) {
            if let Some(role) = &pending.role {
                let context = &frame["context"];
                if !context.is_null() {
                    let replace = state
                        .contexts
                        .get(role)
                        .map(|old| {
                            old["session_id"] == context["session_id"]
                                && context["epoch"].as_u64() >= old["epoch"].as_u64()
                        })
                        .unwrap_or(true);
                    if replace {
                        state.contexts.insert(role.clone(), context.clone());
                    }
                }
            }
            if let Some(sender) = pending.sender {
                let _ = sender.send(frame.clone());
            }
            state.terminal.push_back((id.into(), pending.role, frame));
            if state.terminal.len() > 256 {
                state.terminal.pop_front();
            }
            Ok(())
        } else if state.terminal.iter().any(|(old, _, _)| old == id) {
            Ok(())
        } else {
            drop(state);
            self.transport_failed("unsolicited reply; transport outcome unknown");
            Err("unsolicited reply".into())
        }
    }
    pub(crate) fn validate_delivery(&self, frame: &Value) -> Result<(), String> {
        validate_reply_version(frame, self.protocol)?;
        if self.protocol == 3 {
            let state = self.state.lock().unwrap();
            if let Some(pending) = state.pending.get(frame["id"].as_str().unwrap()) {
                if let Some(expected) = &pending.context {
                    let context = &frame["context"];
                    let matches = if expected.is_null() {
                        !context.is_null() && context["domain"].is_null()
                    } else {
                        !context.is_null()
                            && context["domain"] == expected["domain"]
                            && context["session_id"] == expected["session_id"]
                    };
                    if !matches {
                        return Err(
                            "Reply domain/session does not match its retained responsibility"
                                .into(),
                        );
                    }
                }
            }
        }
        Ok(())
    }
    pub fn mark_timeout(&self, id: &str) {
        let mut state = self.state.lock().unwrap();
        let role = if let Some(pending) = state.pending.get_mut(id) {
            pending.timed_out = true;
            pending.role.clone()
        } else {
            state
                .terminal
                .iter()
                .find(|(old, _, _)| old == id)
                .and_then(|(_, role, _)| role.clone())
        };
        state.generation += 1;
        if let Some(role) = role {
            state.restricted.insert(role);
        }
    }
    pub fn transport_failed(&self, message: &str) {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        if state.failure.is_none() {
            state.failure = Some(message.into());
        }
        for pending in state.pending.values_mut() {
            pending.sender.take();
        }
    }
    // Only an admission rejection can discard an ID with no wire responsibility.
    pub(crate) fn withdraw(&self, id: &str) {
        self.state.lock().unwrap().pending.remove(id);
    }
    pub(crate) fn restricted(&self, role: &str) -> bool {
        let state = self.state.lock().unwrap();
        state.failure.is_some() || state.restricted.contains(role)
    }
    pub(crate) fn generation(&self) -> u64 {
        self.state.lock().unwrap().generation
    }
    pub(crate) fn has_timed_out(&self, role: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .pending
            .values()
            .any(|p| p.role.as_deref() == Some(role) && p.timed_out)
    }
    pub(crate) fn control_changed(&self) {
        self.state.lock().unwrap().generation += 1;
    }
    pub(crate) fn recover(&self, role: &str, context: &Value, generation: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.failure.is_some()
            || state.generation != generation
            || state.contexts.get(role) != Some(context)
            || state
                .pending
                .values()
                .any(|p| p.role.as_deref() == Some(role) && p.timed_out)
        {
            return false;
        }
        state.restricted.remove(role);
        true
    }
    pub(crate) fn terminal(&self, id: &str) -> Option<Value> {
        self.state
            .lock()
            .unwrap()
            .terminal
            .iter()
            .find(|(old, _, _)| old == id)
            .map(|(_, _, v)| v.clone())
    }
    pub(crate) fn failure(&self) -> Option<String> {
        self.state.lock().unwrap().failure.clone()
    }
    pub(crate) fn domain_pending(&self, key: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .pending
            .values()
            .filter(|pending| pending.role.as_deref() == Some(key))
            .count()
    }
    pub(crate) fn domain_context(&self, key: &str) -> Option<Value> {
        self.state.lock().unwrap().contexts.get(key).cloned()
    }
}

pub(crate) fn valid_context(value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 3
        && value["session_id"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
        && (value["connection_id"].is_null() && object.contains_key("connection_id")
            || value["connection_id"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty()))
        && value["epoch"]
            .as_u64()
            .is_some_and(|n| n <= 9_007_199_254_740_991)
}
pub(crate) fn validate_reply(frame: &Value) -> Result<(), String> {
    validate_reply_version(frame, 2)
}
pub(crate) fn domain_key(context: &Value) -> Option<String> {
    let domain = &context["domain"];
    if domain.is_null() {
        return None;
    }
    Some(format!(
        "{}:{}",
        domain["kind"].as_str()?,
        domain["id"].as_str()?
    ))
}
pub(crate) fn valid_context_v3(value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    let Some(object) = value.as_object() else {
        return false;
    };
    let valid = crate::host::contracts::valid_id;
    if object.len() != 4
        || !["session_id", "domain", "connection_id", "epoch"]
            .iter()
            .all(|key| object.contains_key(*key))
        || !value["session_id"].as_str().is_some_and(valid)
        || (!value["connection_id"].is_null()
            && !value["connection_id"].as_str().is_some_and(valid))
        || !value["epoch"]
            .as_u64()
            .is_some_and(|epoch| epoch <= crate::host::contracts::MAX_SEQUENCE)
    {
        return false;
    }
    if value["domain"].is_null() {
        return value["connection_id"].is_null() && value["epoch"] == 0;
    }
    let Some(domain) = value["domain"].as_object() else {
        return false;
    };
    domain.len() == 2
        && matches!(value["domain"]["kind"].as_str(), Some("device" | "setup"))
        && value["domain"]["id"].as_str().is_some_and(valid)
}
pub(crate) fn validate_reply_version(frame: &Value, version: u64) -> Result<(), String> {
    let valid = (|| {
        let object = frame.as_object()?;
        if object.len() != 6
            || frame["v"].as_u64() != Some(version)
            || !object.contains_key("context")
            || !(if version == 3 {
                valid_context_v3(&frame["context"])
            } else {
                valid_context(&frame["context"])
            })
        {
            return None;
        }
        let id = frame["id"].as_str()?;
        if id.is_empty() || id.chars().count() > 64 {
            return None;
        }
        let phase = frame["phase"].as_str()?;
        match frame["ok"].as_bool()? {
            true if phase == "completed"
                && object.contains_key("result")
                && !object.contains_key("error") =>
            {
                Some(())
            }
            false
                if [
                    "rejected_before_call",
                    "superseded_before_call",
                    "completed_readback_failed",
                    "failed_after_call_started",
                ]
                .contains(&phase)
                    && !object.contains_key("result") =>
            {
                let error = frame["error"].as_object()?;
                if !error
                    .keys()
                    .all(|k| ["type", "message", "attempt_id"].contains(&k.as_str()))
                    || !["type", "message"].iter().all(|k| {
                        error
                            .get(*k)
                            .and_then(Value::as_str)
                            .is_some_and(|s| !s.trim().is_empty())
                    })
                    || error.get("attempt_id").is_some_and(|v| !v.is_string())
                {
                    return None;
                }
                Some(())
            }
            _ => None,
        }
    })();
    valid.ok_or_else(|| "invalid v2 reply envelope; transport outcome unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn v3_wrong_domain_reply_cannot_clear_restriction() {
        let broker = ReplyBroker::for_protocol(3).unwrap();
        let context = json!({"session_id":"a".repeat(32),"domain":{"kind":"device","id":"1".repeat(32)},
            "connection_id":"c".repeat(32),"epoch":1});
        let _rx = broker.register_domain("attempt", &context).unwrap();
        broker.mark_timeout("attempt");
        let wrong = json!({"v":3,"id":"attempt","ok":true,"phase":"completed",
            "context":{"session_id":"a".repeat(32),"domain":{"kind":"device","id":"2".repeat(32)},
            "connection_id":"c".repeat(32),"epoch":1},"result":{"resumed":true}});
        assert!(broker.deliver(wrong).is_err());
        assert!(broker.restricted(&format!("device:{}", "1".repeat(32))));
        assert_eq!(broker.liability_capacity(), 225);
    }
    fn reply(id: &str) -> Value {
        json!({"v":2,"id":id,"ok":true,"phase":"completed","context":null,"result":{}})
    }
    #[test]
    fn reverse_replies_reach_their_own_waiters() {
        let broker = ReplyBroker::new();
        let a = broker.register("a", Some("osa")).unwrap();
        let b = broker.register("b", Some("voltage")).unwrap();
        for id in ["b", "a"] {
            broker.deliver(reply(id)).unwrap();
        }
        assert_eq!(a.try_recv().unwrap()["id"], "a");
        assert_eq!(b.try_recv().unwrap()["id"], "b");
    }
    #[test]
    fn duplicate_ids_and_terminal_ring_are_bounded() {
        let broker = ReplyBroker::new();
        let _a = broker.register("a", None).unwrap();
        assert!(broker.register("a", None).is_err());
        broker.deliver(reply("a")).unwrap();
        assert!(broker.register("a", None).is_err());
        broker.deliver(reply("a")).unwrap();
        for n in 0..256 {
            let id = format!("id-{n}");
            let _rx = broker.register(&id, None).unwrap();
            broker.deliver(reply(&id)).unwrap();
        }
        assert!(broker.register("a", None).is_ok());
    }
    #[test]
    fn timeout_keeps_its_slot_and_late_reply_is_correlated() {
        let broker = ReplyBroker::new();
        let a = broker.register("a", Some("osa")).unwrap();
        broker.mark_timeout("a");
        for n in 0..40 {
            broker.register(&format!("p-{n}"), None).unwrap();
        }
        assert!(broker.register("overflow", None).is_err());
        broker.deliver(reply("a")).unwrap();
        assert_eq!(a.try_recv().unwrap()["id"], "a");
        assert!(broker.register("room", None).is_ok());
    }
    #[test]
    fn unsolicited_and_invalid_frames_fail_transport() {
        for frame in [
            reply("stranger"),
            json!({"v":1,"id":"a","ok":true}),
            json!({"v":2,"id":"a","ok":true,"phase":"completed","context":null,"result":{},"extra":1}),
        ] {
            let broker = ReplyBroker::new();
            let a = broker.register("a", Some("osa")).unwrap();
            assert!(broker.deliver(frame).is_err());
            assert!(a.recv().is_err());
            assert!(broker.register("new", None).is_err());
        }
    }
}
