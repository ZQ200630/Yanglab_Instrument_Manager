//! One bounded priority writer owns stdin; admission outlives queued bytes.
use crate::reply_broker::ReplyBroker;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchClass {
    Normal,
    Query,
    Safety(String),
    Shutdown,
}
struct Accepted {
    class: DispatchClass,
    rank: u8,
}
#[derive(Default)]
struct Queue {
    frames: VecDeque<Value>,
    active: HashMap<String, Accepted>,
    closed: bool,
}
pub struct RequestWriter {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    broker: Arc<ReplyBroker>,
}
impl RequestWriter {
    pub fn new<W: Write + Send + 'static>(mut sink: W, broker: Arc<ReplyBroker>) -> Arc<Self> {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let writer = Arc::new(Self {
            queue: queue.clone(),
            broker: broker.clone(),
        });
        std::thread::spawn(move || loop {
            let frame = {
                let (lock, wake) = &*queue;
                let mut state = lock.lock().unwrap();
                while state.frames.is_empty() && !state.closed {
                    state = wake.wait(state).unwrap();
                }
                if state.closed || broker.failure().is_some() {
                    return;
                }
                let index = state
                    .frames
                    .iter()
                    .position(|f| {
                        state
                            .active
                            .get(f["id"].as_str().unwrap())
                            .is_some_and(|a| a.class == DispatchClass::Shutdown)
                    })
                    .or_else(|| {
                        state.frames.iter().position(|f| {
                            state
                                .active
                                .get(f["id"].as_str().unwrap())
                                .is_some_and(|a| matches!(a.class, DispatchClass::Safety(_)))
                        })
                    })
                    .unwrap_or(0);
                state.frames.remove(index).unwrap()
            };
            let mut bytes = serde_json::to_vec(&frame).expect("JSON Value encodes");
            bytes.push(b'\n');
            if let Err(error) = sink.write_all(&bytes).and_then(|_| sink.flush()) {
                broker.transport_failed(&format!(
                    "stdin write failed: {error}; delivery unconfirmed"
                ));
                queue.0.lock().unwrap().closed = true;
                return;
            }
        });
        writer
    }
    pub fn for_protocol<W: Write + Send + 'static>(
        sink: W,
        broker: Arc<ReplyBroker>,
        protocol: u64,
    ) -> Result<Arc<Self>, String> {
        if ![2, 3].contains(&protocol) || broker.protocol() != protocol {
            return Err("Writer/broker protocol mismatch".into());
        }
        Ok(Self::new(sink, broker))
    }
    pub fn normal_capacity(&self) -> usize {
        31
    }
    pub fn active_count(&self) -> usize {
        self.queue.0.lock().unwrap().active.len()
    }
    pub fn enqueue_v3(&self, frame: Value, kind: Option<&str>) -> Result<(), String> {
        if self.broker.protocol() != 3 {
            return Err("V3 writer required".into());
        }
        let (class, rank) = classify_v3(&frame, kind)?;
        self.enqueue_ranked(frame, class, rank)
    }
    pub fn enqueue(&self, frame: Value, class: DispatchClass) -> Result<(), String> {
        if self.broker.protocol() != 2 || frame["v"] != 2 {
            return Err("V2 writer required".into());
        }
        let rank = if matches!(class, DispatchClass::Safety(_)) {
            safety_rank(&frame).ok_or("invalid safety intent")?
        } else {
            0
        };
        self.enqueue_ranked(frame, class, rank)
    }
    fn enqueue_ranked(&self, frame: Value, class: DispatchClass, rank: u8) -> Result<(), String> {
        let id = frame["id"].as_str().ok_or("request ID missing")?.to_owned();
        if let DispatchClass::Safety(role) = &class {
            let expected = if self.broker.protocol() == 3 {
                crate::reply_broker::domain_key(&frame["context"])
            } else {
                frame["params"]["role"].as_str().map(str::to_owned)
            };
            if expected.as_deref() != Some(role) {
                return Err("safety role mismatch".into());
            }
        }
        let mut state = self.queue.0.lock().unwrap();
        if state.closed || self.broker.failure().is_some() {
            return Err("writer unavailable; delivery unconfirmed".into());
        }
        if state.active.contains_key(&id) {
            return Err("duplicate active ID".into());
        }
        if self.broker.protocol() == 3 {
            let groups = state
                .active
                .values()
                .filter_map(|accepted| match &accepted.class {
                    DispatchClass::Safety(key) => Some(key),
                    _ => None,
                })
                .collect::<std::collections::HashSet<_>>();
            if let DispatchClass::Safety(key) = &class {
                if !groups.contains(key) && groups.len() >= 64 {
                    return Err("Safety domain capacity exhausted".into());
                }
            }
        }
        match &class {
            DispatchClass::Normal
                if state
                    .active
                    .values()
                    .filter(|a| a.class == DispatchClass::Normal)
                    .count()
                    >= 31 =>
            {
                return Err("normal admission full".into())
            }
            DispatchClass::Query | DispatchClass::Shutdown => {
                if let Some((old, _)) = state.active.iter().find(|(_, a)| a.class == class) {
                    return Err(format!("existing attempt: {old}"));
                }
            }
            DispatchClass::Safety(_) => {
                if let Some((old, _)) = state
                    .active
                    .iter()
                    .find(|(_, a)| a.class == class && a.rank >= rank)
                {
                    return Err(format!("existing attempt: {old}"));
                }
            }
            _ => {}
        }
        // Only unsent weaker controls can be superseded locally. A popped frame
        // belongs to the single writer even while write_all is blocked.
        let superseded: Vec<Value> = if matches!(class, DispatchClass::Safety(_)) {
            state
                .frames
                .iter()
                .filter(|f| {
                    state
                        .active
                        .get(f["id"].as_str().unwrap())
                        .is_some_and(|a| a.class == class)
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        for old in &superseded {
            let old_id = old["id"].as_str().unwrap();
            state.frames.retain(|f| f["id"] != old["id"]);
            state.active.remove(old_id);
            self.broker.deliver(serde_json::json!({"v":self.broker.protocol(),"id":old_id,"ok":false,"phase":"superseded_before_call","context":old["context"],
                "error":{"type":"Superseded","message":"unsent intent replaced by stronger attempt; no effect executed","attempt_id":id}}))?;
        }
        state.active.insert(id, Accepted { class, rank });
        state.frames.push_back(frame);
        self.queue.1.notify_one();
        Ok(())
    }
    pub fn complete(&self, id: &str) {
        self.queue.0.lock().unwrap().active.remove(id);
    }
    pub(crate) fn fence_domain(&self, key: &str) -> Result<usize, String> {
        if self.broker.protocol() != 3 {
            return Err("Domain fencing requires v3".into());
        }
        let cancelled = {
            let mut state = self.queue.0.lock().unwrap();
            let frames = state
                .frames
                .iter()
                .filter(|frame| {
                    crate::reply_broker::domain_key(&frame["context"]).as_deref() == Some(key)
                        && state
                            .active
                            .get(frame["id"].as_str().unwrap())
                            .is_some_and(|accepted| accepted.class == DispatchClass::Normal)
                })
                .cloned()
                .collect::<Vec<_>>();
            for frame in &frames {
                state.frames.retain(|item| item["id"] != frame["id"]);
                state.active.remove(frame["id"].as_str().unwrap());
            }
            frames
        };
        // No pipe write or driver wait while the queue is locked. Popped bytes
        // remain tracked: fencing is not a claim of physical interruption.
        for frame in &cancelled {
            self.broker.deliver(serde_json::json!({"v":3,"id":frame["id"],"ok":false,"phase":"superseded_before_call",
                "context":frame["context"],"error":{"type":"ControlRevoked","message":"Control was revoked before this frame was sent"}}))?;
        }
        Ok(cancelled.len())
    }
    pub fn close(&self) {
        self.queue.0.lock().unwrap().closed = true;
        self.queue.1.notify_all();
    }
}
pub(crate) fn classify_v3(
    frame: &Value,
    kind: Option<&str>,
) -> Result<(DispatchClass, u8), String> {
    crate::runtime::decode_v3(&serde_json::to_vec(frame).map_err(|error| error.to_string())?)?;
    let method = frame["method"].as_str().unwrap();
    if [
        "probe",
        "check_online",
        "connect",
        "disconnect",
        "resume",
        "action",
    ]
    .contains(&method)
        && !kind
            .is_some_and(|kind| ["osa", "voltage", "gain", "pm400", "mdt", "fiber", "laser"].contains(&kind))
    {
        return Err("Configured domain kind is required".into());
    }
    let rank = match (method, kind, frame["params"]["name"].as_str().unwrap_or("")) {
        ("disconnect", Some("gain"), _) => 3,
        ("disconnect", Some("voltage"), _) => 2,
        ("disconnect", _, _) => 1,
        ("action", Some("gain"), "disable_current") | ("action", Some("voltage"), "zero") => 1,
        ("action", Some("gain"), "disable_tec") => 2,
        _ => 0,
    };
    let class = if rank > 0 {
        DispatchClass::Safety(
            crate::reply_broker::domain_key(&frame["context"]).ok_or("Domain required")?,
        )
    } else {
        match method {
            "ping" | "status" | "inventory" => DispatchClass::Query,
            "shutdown" => DispatchClass::Shutdown,
            _ => DispatchClass::Normal,
        }
    };
    Ok((class, rank))
}
impl Drop for RequestWriter {
    fn drop(&mut self) {
        self.close();
    }
}

fn safety_rank(frame: &Value) -> Option<u8> {
    let role = frame["params"]["role"].as_str()?;
    if !["osa", "voltage", "gain", "pm400", "fiber"].contains(&role) {
        return None;
    }
    match (
        frame["method"].as_str()?,
        role,
        frame["params"]["name"].as_str().unwrap_or(""),
    ) {
        ("disconnect", "gain", _) => Some(3),
        ("disconnect", "voltage", _) => Some(2),
        ("disconnect", _, _) => Some(1),
        ("action", "gain", "disable_current") | ("action", "voltage", "zero") => Some(1),
        ("action", "gain", "disable_tec") => Some(2),
        _ => None,
    }
}

pub(crate) fn classify(frame: &Value) -> Result<DispatchClass, String> {
    let object = frame.as_object().ok_or("invalid request")?;
    if object.len() != 5
        || !["v", "id", "method", "params", "context"]
            .iter()
            .all(|k| object.contains_key(*k))
        || frame["v"].as_u64() != Some(2)
        || !crate::reply_broker::valid_context(&frame["context"])
        || !frame["params"].is_object()
        || !frame["id"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.chars().count() <= 64)
    {
        return Err("invalid v2 request".into());
    }
    let method = frame["method"].as_str().ok_or("missing method")?;
    let role_method = ["action", "connect", "disconnect", "resume"].contains(&method);
    if frame["context"].is_null() && !["ping", "status"].contains(&method) {
        return Err("context required".into());
    }
    if role_method {
        if !frame["params"]["role"]
            .as_str()
            .is_some_and(|r| ["osa", "gain", "voltage", "pm400", "fiber"].contains(&r))
        {
            return Err("invalid role".into());
        }
    } else if !frame["context"].is_null()
        && (!frame["context"]["connection_id"].is_null() || frame["context"]["epoch"] != 0)
    {
        return Err("invalid global context".into());
    }
    if method == "resume"
        && (frame["params"].as_object().unwrap().len() != 2 || frame["params"]["confirm"] != true)
    {
        return Err("resume requires role and confirm=true".into());
    }
    if safety_rank(frame).is_some() {
        return Ok(DispatchClass::Safety(
            frame["params"]["role"].as_str().unwrap().into(),
        ));
    }
    match method {
        "ping" | "status" => Ok(DispatchClass::Query),
        "shutdown" => Ok(DispatchClass::Shutdown),
        "connect" | "inventory" | "settings_get" | "settings_save" | "resume" => {
            Ok(DispatchClass::Normal)
        }
        "action"
            if frame["params"]["name"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty()) =>
        {
            Ok(DispatchClass::Normal)
        }
        _ => Err("unknown method".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{mpsc, Condvar, Mutex};
    use std::time::Duration;
    #[test]
    fn ordinary_saturation_preserves_192_safety_slots() {
        let broker = Arc::new(ReplyBroker::for_protocol(3).unwrap());
        let (sent, lines) = mpsc::channel();
        let (entered, _entered_rx) = mpsc::channel();
        let gate = Arc::new((Mutex::new(true), Condvar::new()));
        let writer = RequestWriter::for_protocol(
            Sink {
                lines: sent,
                entered,
                gate,
            },
            broker.clone(),
            3,
        )
        .unwrap();
        for index in 0..31 {
            let frame = json!({"v":3,"id":format!("normal-{index}"),"method":"action",
                "params":{"name":"set_current","args":{"current_ma":1}},
                "context":{"session_id":"a".repeat(32),"domain":{"kind":"device","id":"1".repeat(32)},
                    "connection_id":"c".repeat(32),"epoch":0}});
            writer.enqueue_v3(frame, Some("gain")).unwrap();
            lines.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        for index in 1..=64 {
            for (step, name) in ["disable_current", "disable_tec", "disconnect"]
                .into_iter()
                .enumerate()
            {
                let method = if name == "disconnect" {
                    "disconnect"
                } else {
                    "action"
                };
                let params = if method == "disconnect" {
                    json!({})
                } else {
                    json!({"name":name,"args":{}})
                };
                writer.enqueue_v3(json!({"v":3,"id":format!("safe-{index}-{step}"),"method":method,"params":params,
                    "context":{"session_id":"a".repeat(32),"domain":{"kind":"device","id":format!("{index:032x}")},
                        "connection_id":"c".repeat(32),"epoch":0}}),Some("gain")).unwrap();
                lines.recv_timeout(Duration::from_secs(2)).unwrap();
            }
        }
        assert_eq!(writer.normal_capacity(), 31);
        assert_eq!(writer.active_count(), 223);
        writer.close();
    }
    struct Sink {
        lines: mpsc::Sender<Value>,
        entered: mpsc::Sender<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }
    #[test]
    fn domain_fence_cancels_only_unsent_normal_work() {
        let (tx, lines) = mpsc::channel();
        let (enter, entered) = mpsc::channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let broker = Arc::new(ReplyBroker::for_protocol(3).unwrap());
        let writer = RequestWriter::for_protocol(
            Sink {
                lines: tx,
                entered: enter,
                gate: gate.clone(),
            },
            broker.clone(),
            3,
        )
        .unwrap();
        let context = |id| json!({"session_id":"a".repeat(32),"domain":{"kind":"device","id":format!("{id:032x}")},"connection_id":null,"epoch":0});
        let one = context(1);
        let two = context(2);
        let send = |id: &str, ctx: Value| {
            let rx = broker.register_domain(id, &ctx).unwrap();
            writer
                .enqueue_v3(
                    json!({"v":3,"id":id,"method":"connect","params":{},"context":ctx}),
                    Some("osa"),
                )
                .unwrap();
            rx
        };
        let _active = send("already-writing", one.clone());
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        let cancelled = send("not-sent", one.clone());
        let _other = send("other-domain", two);
        let result = writer
            .fence_domain(&format!("device:{}", format!("{:032x}", 1)))
            .unwrap();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let terminal = cancelled.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(result, 1);
        assert_eq!(terminal["phase"], "superseded_before_call");
        let first = lines.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = lines.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(first["id"], "already-writing");
        assert_eq!(second["id"], "other-domain");
        writer.close();
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let _ = self.entered.send(());
            let (lock, wake) = &*self.gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = wake.wait(open).unwrap();
            }
            self.lines
                .send(serde_json::from_slice(bytes).unwrap())
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn frame(id: &str, role: &str, name: &str) -> Value {
        json!({"v":2,"id":id,"method":"action","params":{"role":role,"name":name},"context":{"session_id":"s","connection_id":"c","epoch":0}})
    }
    struct Fixture {
        writer: Arc<RequestWriter>,
        broker: Arc<ReplyBroker>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        lines: mpsc::Receiver<Value>,
        entered: mpsc::Receiver<()>,
    }
    impl Fixture {
        fn new(open: bool) -> Self {
            let (tx, lines) = mpsc::channel();
            let (enter, entered) = mpsc::channel();
            let gate = Arc::new((Mutex::new(open), Condvar::new()));
            let broker = Arc::new(ReplyBroker::new());
            let writer = RequestWriter::new(
                Sink {
                    lines: tx,
                    entered: enter,
                    gate: gate.clone(),
                },
                broker.clone(),
            );
            Self {
                writer,
                broker,
                gate,
                lines,
                entered,
            }
        }
        fn send(&self, id: &str, class: DispatchClass, name: &str) -> mpsc::Receiver<Value> {
            let rx = self.broker.register(id, Some("gain")).unwrap();
            self.writer.enqueue(frame(id, "gain", name), class).unwrap();
            rx
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
            self.writer.close();
        }
    }
    #[test]
    fn outstanding_normal_limit_reserves_query_and_safety_after_writes() {
        let f = Fixture::new(true);
        for n in 0..31 {
            f.send(&format!("n{n}"), DispatchClass::Normal, "set");
            f.lines.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        assert!(f
            .writer
            .enqueue(frame("overflow", "gain", "set"), DispatchClass::Normal)
            .is_err());
        f.send("status", DispatchClass::Query, "status");
        assert!(f
            .writer
            .enqueue(frame("ping", "gain", "ping"), DispatchClass::Query)
            .unwrap_err()
            .contains("status"));
        f.send(
            "stop",
            DispatchClass::Safety("gain".into()),
            "disable_current",
        );
        f.writer.complete("n0");
        f.send("room", DispatchClass::Normal, "set");
    }
    #[test]
    fn blocked_writer_never_claims_delivery_and_upgrades_queued_intents_once() {
        let f = Fixture::new(false);
        f.send("blocked", DispatchClass::Normal, "set");
        f.entered.recv_timeout(Duration::from_secs(1)).unwrap();
        let old = f.send(
            "current",
            DispatchClass::Safety("gain".into()),
            "disable_current",
        );
        let error = f
            .writer
            .enqueue(
                frame("duplicate", "gain", "disable_current"),
                DispatchClass::Safety("gain".into()),
            )
            .unwrap_err();
        assert!(error.contains("current"));
        f.send("tec", DispatchClass::Safety("gain".into()), "disable_tec");
        let reply = old.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(reply["phase"], "superseded_before_call");
        assert_eq!(reply["error"]["attempt_id"], "tec");
        assert!(f.lines.try_recv().is_err());
        *f.gate.0.lock().unwrap() = true;
        f.gate.1.notify_all();
        let mut ids = Vec::new();
        while let Ok(line) = f.lines.recv_timeout(Duration::from_millis(100)) {
            ids.push(line["id"].as_str().unwrap().to_string());
        }
        assert!(ids.contains(&"tec".into()));
        assert!(!ids.contains(&"duplicate".into()));
        assert!(!ids.contains(&"current".into()));
    }
    #[test]
    fn sent_weaker_intent_retains_waiter_while_upgrade_is_written() {
        let f = Fixture::new(true);
        let old = f.send(
            "current",
            DispatchClass::Safety("gain".into()),
            "disable_current",
        );
        assert_eq!(
            f.lines.recv_timeout(Duration::from_secs(1)).unwrap()["id"],
            "current"
        );
        f.send("tec", DispatchClass::Safety("gain".into()), "disable_tec");
        assert_eq!(
            f.lines.recv_timeout(Duration::from_secs(1)).unwrap()["id"],
            "tec"
        );
        assert!(old.try_recv().is_err());
        assert!(f
            .writer
            .enqueue(
                frame("lower", "gain", "disable_current"),
                DispatchClass::Safety("gain".into())
            )
            .is_err());
    }
    #[test]
    fn transport_failure_stops_queued_bytes_after_the_current_write() {
        let f = Fixture::new(false);
        f.send("blocked", DispatchClass::Normal, "set");
        f.entered.recv_timeout(Duration::from_secs(1)).unwrap();
        f.send("queued", DispatchClass::Normal, "set");
        f.broker.transport_failed("malformed stdout");
        *f.gate.0.lock().unwrap() = true;
        f.gate.1.notify_all();
        assert_eq!(
            f.lines.recv_timeout(Duration::from_secs(1)).unwrap()["id"],
            "blocked"
        );
        assert!(f.lines.recv_timeout(Duration::from_millis(100)).is_err());
    }
    #[test]
    fn finite_control_chains_and_shutdown_fit_beside_all_32_normal_slots() {
        let f = Fixture::new(true);
        for n in 0..31 {
            f.send(&format!("normal{n}"), DispatchClass::Normal, "set");
            f.lines.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        f.send("query", DispatchClass::Query, "status");
        f.lines.recv_timeout(Duration::from_secs(1)).unwrap();
        for (id, role, name, disconnect) in [
            ("g1", "gain", "disable_current", false),
            ("g2", "gain", "disable_tec", false),
            ("g3", "gain", "", true),
            ("v1", "voltage", "zero", false),
            ("v2", "voltage", "", true),
            ("osa", "osa", "", true),
            ("pm", "pm400", "", true),
            ("fiber", "fiber", "", true),
        ] {
            let mut value = frame(id, role, name);
            if disconnect {
                value["method"] = "disconnect".into();
            }
            f.broker.register(id, Some(role)).unwrap();
            f.writer
                .enqueue(value, DispatchClass::Safety(role.into()))
                .unwrap();
            assert_eq!(
                f.lines.recv_timeout(Duration::from_secs(1)).unwrap()["id"],
                id
            );
        }
        f.broker.register("shutdown", None).unwrap();
        f.writer.enqueue(serde_json::json!({"v":2,"id":"shutdown","method":"shutdown","params":{},"context":null}),DispatchClass::Shutdown).unwrap();
        assert_eq!(
            f.lines.recv_timeout(Duration::from_secs(1)).unwrap()["id"],
            "shutdown"
        );
        assert!(f.broker.register("42nd", None).is_err());
        assert!(f
            .writer
            .enqueue(
                frame("g4", "gain", "disable_tec"),
                DispatchClass::Safety("gain".into())
            )
            .is_err());
    }
    #[test]
    fn classifier_does_not_trust_client_priority_or_fixture_commands() {
        let mut value = frame("id", "gain", "enable_current");
        assert_eq!(classify(&value).unwrap(), DispatchClass::Normal);
        value["priority"] = "safety".into();
        assert!(classify(&value).is_err());
        value.as_object_mut().unwrap().remove("priority");
        value["method"] = "configure".into();
        assert!(classify(&value).is_err());
        value["method"] = "action".into();
        value["params"]["name"] = "disable_current".into();
        assert_eq!(
            classify(&value).unwrap(),
            DispatchClass::Safety("gain".into())
        );
        value["params"]["role"] = "osa".into();
        assert_eq!(classify(&value).unwrap(), DispatchClass::Normal);
        value["context"]["epoch"] = json!(-1);
        assert!(classify(&value).is_err());
    }
}
