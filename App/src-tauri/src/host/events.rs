#[cfg(test)]
mod tests {
    use super::super::leases::{DomainRef, LeaseBook, Session};
    use super::*;
    use serde_json::json;
    #[test]
    fn osa_read_and_failure_replace_sweep_for_observer_after_ring_rollover() {
        for failed in [false, true] {
            let hub = EventHub::new("a".repeat(32), "b".repeat(32), json!({})).unwrap();
            let d = DomainRef {
                kind: "device".into(),
                id: "1".repeat(32),
            };
            for n in 0..22 {
                let data = match n {
                    0 => {
                        json!({"operation_id":"2".repeat(32),"domain":d,"status":"Terminal","phase":"completed","command":{"method":"action","params":{"name":"acquire","args":{"trace":"A"}}},"result":{"context":{"epoch":1},"result":{"archive_ref":{"id":"2".repeat(32)}}}})
                    }
                    1 => {
                        json!({"operation_id":"3".repeat(32),"domain":d,"status":"Terminal","phase":if failed {"completed_readback_failed"} else {"completed"},"command":{"method":"action","params":{"name":"read_trace","args":{"trace":"A"}}},"result":{"context":{"epoch":1},"result":if failed {json!(null)} else {json!({"archive_ref":{"id":"3".repeat(32)}})},"error":if failed {json!({"message":"primary read failed"})} else {json!(null)}}})
                    }
                    _ => {
                        json!({"operation_id":format!("{n:032x}"),"domain":d,"status":"Accepted","phase":"accepted"})
                    }
                };
                hub.publish(HostEvent {
                    kind: "operation".into(),
                    host_id: "a".repeat(32),
                    boot_id: "b".repeat(32),
                    seq: 0,
                    first_seq: 0,
                    sent_monotonic_ms: 0,
                    domain: Some(d.clone()),
                    data,
                })
                .unwrap();
            }
            let observer = Session::local("4".repeat(32), "b".repeat(32)).unwrap();
            let snapshot = hub.subscribe(&observer).unwrap().snapshot.data.clone();
            assert_eq!(snapshot["operations"].as_array().unwrap().len(), 16);
            assert_eq!(snapshot["results"].as_array().unwrap().len(), 1);
            assert_eq!(snapshot["results"][0]["operation_id"], "3".repeat(32));
            if failed {
                assert_eq!(snapshot["results"][0]["phase"], "completed_readback_failed");
                assert_eq!(
                    snapshot["results"][0]["result"]["error"]["message"],
                    "primary read failed"
                );
                assert!(snapshot["results"][0]["result"]["result"]["archive_ref"].is_null());
            } else {
                assert_eq!(
                    snapshot["results"][0]["result"]["result"]["archive_ref"]["id"],
                    "3".repeat(32)
                );
            }
        }
    }
    #[test]
    fn snapshot_keeps_shared_result_after_recent_operation_ring_turns_over() {
        let hub = EventHub::new("a".repeat(32), "b".repeat(32), json!({})).unwrap();
        let d = DomainRef {
            kind: "device".into(),
            id: "1".repeat(32),
        };
        for n in 0..20 {
            let data = if n == 0 {
                json!({"operation_id":"e".repeat(32),"domain":d,"status":"Terminal","phase":"completed","command":{"method":"action","params":{"name":"acquire","args":{"trace":"A"}}},"result":{"context":{"epoch":1},"result":{"result_ref":{"id":"f".repeat(32)}}}})
            } else {
                json!({"operation_id":format!("{n:032x}"),"domain":d,"status":"Accepted","phase":"accepted"})
            };
            hub.publish(HostEvent {
                kind: "operation".into(),
                host_id: "a".repeat(32),
                boot_id: "b".repeat(32),
                seq: 0,
                first_seq: 0,
                sent_monotonic_ms: 0,
                domain: Some(d.clone()),
                data,
            })
            .unwrap();
        }
        assert_eq!(hub.current()["results"][0]["operation_id"], "e".repeat(32));
    }
    #[test]
    fn targeted_resnapshot_preserves_other_clients_and_boundary() {
        let hub = EventHub::new("a".repeat(32), "b".repeat(32), json!({"reading":0})).unwrap();
        let a = Session::local("c".repeat(32), "b".repeat(32)).unwrap();
        let b = Session::local("d".repeat(32), "b".repeat(32)).unwrap();
        let one = hub.subscribe(&a).unwrap();
        let two = hub.subscribe(&b).unwrap();
        hub.update(json!({"reading":1})).unwrap();
        hub.resnapshot(&a).unwrap();
        let snapshot = one.try_next().unwrap().unwrap();
        assert_eq!(snapshot.kind, "snapshot");
        assert_eq!(snapshot.seq, 1);
        assert_eq!(two.try_next().unwrap().unwrap().kind, "state");
        hub.update(json!({"reading":2})).unwrap();
        assert_eq!(one.try_next().unwrap().unwrap().seq, 2);
    }
    #[test]
    fn snapshot_then_events_has_no_gap() {
        let hub = EventHub::new("a".repeat(32), "b".repeat(32), json!({"value":0})).unwrap();
        let s = Session::local("c".repeat(32), "b".repeat(32)).unwrap();
        let stream = hub.subscribe(&s).unwrap();
        hub.update(json!({"value":1})).unwrap();
        assert_eq!(stream.snapshot.seq, 0);
        assert_eq!(stream.try_next().unwrap().unwrap().seq, 1);
    }
    #[test]
    fn large_events_roundtrip_in_bounded_control_frames() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let event = HostEvent {
                kind: "state".into(),
                host_id: "a".repeat(32),
                boot_id: "b".repeat(32),
                seq: 1,
                first_seq: 1,
                sent_monotonic_ms: 1,
                domain: None,
                data: json!({"text":"x".repeat(200000)}),
            };
            let mut encoded = Vec::new();
            write_event(&mut encoded, &event).await.unwrap();
            let mut cursor = std::io::Cursor::new(encoded);
            let decoded = read_event(&mut cursor).await.unwrap().unwrap();
            assert_eq!(decoded["data"]["text"].as_str().unwrap().len(), 200000);
        });
    }
    #[test]
    fn telemetry_coalescing_is_per_subscriber_and_snapshot_keeps_latest_domain() {
        let hub = EventHub::new("a".repeat(32), "b".repeat(32), json!({"domains":{}})).unwrap();
        let slow = hub
            .subscribe(&Session::local("c".repeat(32), "b".repeat(32)).unwrap())
            .unwrap();
        let fast = hub
            .subscribe(&Session::local("d".repeat(32), "b".repeat(32)).unwrap())
            .unwrap();
        for n in 1..=2 {
            hub.publish(HostEvent {
                kind: "domain".into(),
                host_id: "a".repeat(32),
                boot_id: "b".repeat(32),
                seq: 0,
                first_seq: 0,
                sent_monotonic_ms: 0,
                domain: Some(DomainRef {
                    kind: "device".into(),
                    id: "1".repeat(32),
                }),
                data: json!({"reading":n}),
            })
            .unwrap();
            assert_eq!(fast.try_next().unwrap().unwrap().first_seq, n);
        }
        let merged = slow.try_next().unwrap().unwrap();
        assert_eq!((merged.first_seq, merged.seq), (1, 2));
        let new = hub
            .subscribe(&Session::local("e".repeat(32), "b".repeat(32)).unwrap())
            .unwrap();
        assert_eq!(
            new.snapshot.data["domains"][format!("device:{}", "1".repeat(32))]["reading"],
            2
        );
    }
    #[test]
    fn slow_client_releases_only_its_domains() {
        let boot = "b".repeat(32);
        let hub = EventHub::new("a".repeat(32), boot.clone(), json!({})).unwrap();
        let a = Session::local("c".repeat(32), boot.clone()).unwrap();
        let b = Session::local("d".repeat(32), boot.clone()).unwrap();
        let _slow = hub.subscribe(&a).unwrap();
        let fast = hub.subscribe(&b).unwrap();
        let mut leases = LeaseBook::new(&boot).unwrap();
        let one = DomainRef {
            kind: "device".into(),
            id: "1".repeat(32),
        };
        let two = DomainRef {
            kind: "device".into(),
            id: "2".repeat(32),
        };
        leases.register(&one).unwrap();
        leases.register(&two).unwrap();
        leases.acquire(&a, &one, std::time::Instant::now()).unwrap();
        leases.acquire(&b, &two, std::time::Instant::now()).unwrap();
        for n in 0..257 {
            hub.update(json!({"revision":n})).unwrap();
            let _ = fast.try_next().unwrap();
        }
        let failed = hub.take_closed_sessions();
        assert_eq!(failed.len(), 1);
        for s in failed {
            leases.close_session(&s).unwrap();
        }
        assert_eq!(leases.pending_cleanups().len(), 1);
        assert_eq!(leases.pending_cleanups()[0].domain, one);
    }
}
// Atomic snapshot/subscription boundary; bounded per-session event liability.
use super::{
    contracts::{valid_id, HostError, MAX_SEQUENCE},
    leases::{DomainRef, Session},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, Weak},
    time::Instant,
};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub host_id: String,
    pub boot_id: String,
    pub seq: u64,
    pub first_seq: u64,
    pub sent_monotonic_ms: u64,
    pub domain: Option<DomainRef>,
    pub data: Value,
}
struct Queue {
    values: VecDeque<HostEvent>,
    bytes: usize,
    closed: bool,
    session: Session,
}
struct State {
    seq: u64,
    snapshot: Value,
    subscribers: BTreeMap<String, Arc<Mutex<Queue>>>,
    closed: BTreeMap<String, Session>,
}
struct Inner {
    host: String,
    boot: String,
    started: Instant,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct EventHub {
    inner: Arc<Inner>,
}
pub struct SnapshotStream {
    pub snapshot: HostEvent,
    queue: Arc<Mutex<Queue>>,
    inner: Weak<Inner>,
    session: String,
}
impl EventHub {
    pub fn new(host: String, boot: String, snapshot: Value) -> Result<Self, HostError> {
        if !valid_id(&host) || !valid_id(&boot) {
            return Err(HostError::new("EventIdentity", "Invalid Host identity"));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                host,
                boot,
                started: Instant::now(),
                state: Mutex::new(State {
                    seq: 0,
                    snapshot,
                    subscribers: BTreeMap::new(),
                    closed: BTreeMap::new(),
                }),
            }),
        })
    }
    pub fn monotonic_ms(&self) -> u64 {
        self.inner
            .started
            .elapsed()
            .as_millis()
            .min(MAX_SEQUENCE as u128) as u64
    }
    pub fn current(&self) -> Value {
        let state = self.inner.state.lock().unwrap();
        let mut snapshot = state.snapshot.clone();
        snapshot["seq"] = serde_json::json!(state.seq);
        snapshot
    }
    pub fn subscribe(&self, session: &Session) -> Result<SnapshotStream, HostError> {
        if session.boot_id() != self.inner.boot {
            return Err(HostError::new("EventIdentity", "Session boot mismatch"));
        }
        let mut state = self.inner.state.lock().unwrap();
        if state.subscribers.contains_key(session.id()) || state.subscribers.len() >= 16 {
            return Err(HostError::new(
                "EventCapacity",
                "Duplicate or excessive subscriptions",
            ));
        }
        let snapshot = HostEvent {
            kind: "snapshot".into(),
            host_id: self.inner.host.clone(),
            boot_id: self.inner.boot.clone(),
            seq: state.seq,
            first_seq: state.seq,
            sent_monotonic_ms: self.monotonic_ms(),
            domain: None,
            data: state.snapshot.clone(),
        };
        if serde_json::to_vec(&snapshot).map_err(event_error)?.len() > 1024 * 1024 {
            return Err(HostError::new("EventCapacity", "Snapshot exceeds 1 MiB"));
        }
        let queue = Arc::new(Mutex::new(Queue {
            values: VecDeque::new(),
            bytes: 0,
            closed: false,
            session: session.clone(),
        }));
        state.subscribers.insert(session.id().into(), queue.clone());
        Ok(SnapshotStream {
            snapshot,
            queue,
            inner: Arc::downgrade(&self.inner),
            session: session.id().into(),
        })
    }
    pub fn update(&self, mut snapshot: Value) -> Result<(), HostError> {
        let mut state = self.inner.state.lock().unwrap();
        if let Some(operations) = state.snapshot.get("operations") {
            snapshot["operations"] = operations.clone();
        }
        if let Some(results) = state.snapshot.get("results") {
            snapshot["results"] = results.clone();
        }
        if serde_json::to_vec(&snapshot).map_err(event_error)?.len() > 1024 * 1024 - 2048 {
            return Err(HostError::new(
                "EventCapacity",
                "Snapshot exceeds bounded storage",
            ));
        }
        state.snapshot = snapshot.clone();
        self.publish_locked(
            &mut state,
            HostEvent {
                kind: "state".into(),
                host_id: self.inner.host.clone(),
                boot_id: self.inner.boot.clone(),
                seq: 0,
                first_seq: 0,
                sent_monotonic_ms: self.monotonic_ms(),
                domain: None,
                data: snapshot,
            },
        )
    }
    pub fn resnapshot(&self, session: &Session) -> Result<u64, HostError> {
        let state = self.inner.state.lock().unwrap();
        let queue = state.subscribers.get(session.id()).ok_or_else(|| {
            HostError::new(
                "EventSubscription",
                "Subscribe before requesting a snapshot",
            )
        })?;
        let mut queue = queue.lock().unwrap();
        if queue.closed {
            return Err(HostError::new(
                "EventSubscription",
                "Subscription is closed",
            ));
        }
        let snapshot = HostEvent {
            kind: "snapshot".into(),
            host_id: self.inner.host.clone(),
            boot_id: self.inner.boot.clone(),
            seq: state.seq,
            first_seq: state.seq,
            sent_monotonic_ms: self.monotonic_ms(),
            domain: None,
            data: state.snapshot.clone(),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(event_error)?.len();
        if bytes > 1024 * 1024 {
            return Err(HostError::new(
                "EventCapacity",
                "Snapshot exceeds bounded storage",
            ));
        }
        queue.values.clear();
        queue.bytes = bytes;
        queue.values.push_back(snapshot);
        Ok(state.seq)
    }
    pub fn publish(&self, event: HostEvent) -> Result<(), HostError> {
        let mut state = self.inner.state.lock().unwrap();
        self.publish_locked(&mut state, event)
    }
    fn publish_locked(&self, state: &mut State, mut event: HostEvent) -> Result<(), HostError> {
        if event.host_id != self.inner.host
            || event.boot_id != self.inner.boot
            || state.seq >= MAX_SEQUENCE
        {
            return Err(HostError::new(
                "EventIdentity",
                "Invalid event identity or exhausted sequence",
            ));
        }
        event.seq = state.seq + 1;
        event.first_seq = event.seq;
        event.sent_monotonic_ms = self.monotonic_ms();
        let bytes = serde_json::to_vec(&event).map_err(event_error)?.len();
        if bytes > 1024 * 1024 {
            return Err(HostError::new("EventCapacity", "Event exceeds 1 MiB"));
        };
        state.seq = event.seq;
        if event.kind == "operation" {
            if let Some(slot) = shared_slot(&event.data) {
                if !state.snapshot["results"].is_array() {
                    state.snapshot["results"] = serde_json::json!([]);
                }
                let results = state.snapshot["results"].as_array_mut().unwrap();
                results.retain(|r| shared_slot(r).as_ref() != Some(&slot));
                let r = &event.data;
                results.push(serde_json::json!({"operation_id":r["operation_id"],"domain":r["domain"],"status":r["status"],"phase":r["phase"],"command":r["command"],"result":{"context":r["result"]["context"],"result":r["result"]["result"],"error":r["result"]["error"]}}));
                while results.len() > 128 {
                    results.remove(0);
                }
            }
            if !state.snapshot["operations"].is_array() {
                state.snapshot["operations"] = serde_json::json!([]);
            }
            let records = state.snapshot["operations"].as_array_mut().unwrap();
            records.push(event.data.clone());
            while records.len() > 16 {
                records.remove(0);
            }
        }
        if let Some(domain) = &event.domain {
            if event.kind == "domain" {
                if !state.snapshot["domains"].is_object() {
                    state.snapshot["domains"] = serde_json::json!({});
                }
                state.snapshot["domains"][format!("{}:{}", domain.kind, domain.id)] =
                    event.data.clone();
            }
        }
        for (subscriber, queue) in &state.subscribers {
            let mut event = event.clone();
            let mut queue = queue.lock().unwrap();
            if queue.closed {
                continue;
            }
            if event.kind == "domain" && event.domain.is_some() {
                if let Some(last) = queue.values.back() {
                    if last.kind == "domain"
                        && last.domain == event.domain
                        && last.seq + 1 == event.seq
                    {
                        let last = queue.values.pop_back().unwrap();
                        queue.bytes = queue
                            .bytes
                            .saturating_sub(serde_json::to_vec(&last).map_err(event_error)?.len());
                        event.first_seq = last.first_seq;
                    }
                }
            }
            let bytes = serde_json::to_vec(&event).map_err(event_error)?.len();
            if queue.values.len() >= 256 || queue.bytes + bytes > 1024 * 1024 {
                queue.closed = true;
                queue.values.clear();
                queue.bytes = 0;
                state
                    .closed
                    .insert(subscriber.clone(), queue.session.clone());
            } else {
                queue.bytes += bytes;
                queue.values.push_back(event.clone());
            }
        }
        Ok(())
    }
    pub fn take_closed_sessions(&self) -> Vec<Session> {
        std::mem::take(&mut self.inner.state.lock().unwrap().closed)
            .into_values()
            .collect()
    }
}
impl SnapshotStream {
    pub fn try_next(&self) -> Result<Option<HostEvent>, HostError> {
        let mut queue = self.queue.lock().unwrap();
        if queue.closed {
            return Err(HostError::new(
                "SlowConsumer",
                "Event liability exceeded; controller session must be revoked",
            ));
        }
        let next = queue.values.pop_front();
        if let Some(ref event) = next {
            queue.bytes = queue
                .bytes
                .saturating_sub(serde_json::to_vec(event).map_err(event_error)?.len());
        }
        Ok(next)
    }
}
impl Drop for SnapshotStream {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .state
                .lock()
                .unwrap()
                .subscribers
                .remove(&self.session);
        }
    }
}
fn event_error(error: impl std::fmt::Display) -> HostError {
    HostError::new("HostEvent", error.to_string())
}
fn shared_slot(record: &Value) -> Option<String> {
    if record["status"] != "Terminal" || record["command"]["method"] != "action" {
        return None;
    }
    let domain: DomainRef = serde_json::from_value(record["domain"].clone()).ok()?;
    let params = &record["command"]["params"];
    let name = params["name"].as_str()?;
    if record["phase"] != "completed" && !matches!(name, "acquire" | "read_trace") {
        return None;
    }
    let slot = if matches!(name, "acquire" | "read_trace") {
        "osa_trace"
    } else {
        name
    };
    let qualifier = match name {
        "acquire" | "read_trace" | "measure_kind" => "",
        "read_setting" => params["args"]["setting"].as_str()?,
        "run_maintenance" => params["args"]["command"].as_str()?,
        _ => return None,
    };
    Some(format!("{}:{slot}:{qualifier}", domain.key()))
}
pub async fn write_event<W: tokio::io::AsyncWrite + Unpin>(
    pipe: &mut W,
    event: &HostEvent,
) -> Result<(), HostError> {
    let bytes = serde_json::to_vec(event).map_err(event_error)?;
    if bytes.len() > 1024 * 1024 {
        return Err(event_error("Event exceeds 1 MiB"));
    }
    let checksum = super::verification::sha256_bytes(&bytes)?;
    for (index, chunk) in bytes.chunks(28000).enumerate() {
        super::ipc::write_frame(
            pipe,
            &serde_json::json!({"event_chunk":true,"seq":event.seq,"offset":index*28000,
            "size":bytes.len(),"checksum":checksum,"data_hex":super::results::hex_bytes(chunk)}),
        )
        .await?;
    }
    Ok(())
}
pub async fn read_event<R: tokio::io::AsyncRead + Unpin>(
    pipe: &mut R,
) -> Result<Option<Value>, HostError> {
    let mut bytes = Vec::new();
    let mut expected = None;
    loop {
        let Some(frame) = super::ipc::read_frame(pipe).await? else {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err(event_error("Truncated event"));
        };
        let value = crate::runtime::strict_json(&frame).map_err(event_error)?;
        let size = value["size"]
            .as_u64()
            .filter(|n| *n <= 1024 * 1024 && *n > 0)
            .ok_or_else(|| event_error("Invalid event size"))?;
        let checksum = value["checksum"]
            .as_str()
            .ok_or_else(|| event_error("Event checksum required"))?
            .to_string();
        let seq = value["seq"]
            .as_u64()
            .ok_or_else(|| event_error("Event sequence required"))?;
        let binding = (seq, size, checksum);
        if value["event_chunk"] != true
            || value["offset"].as_u64() != Some(bytes.len() as u64)
            || expected.as_ref().is_some_and(|e| e != &binding)
        {
            return Err(event_error("Event chunk mismatch"));
        }
        expected = Some(binding.clone());
        let chunk = super::results::unhex(
            value["data_hex"]
                .as_str()
                .ok_or_else(|| event_error("Invalid event chunk"))?,
        )?;
        if chunk.is_empty() || chunk.len() > 28000 || bytes.len() + chunk.len() > size as usize {
            return Err(event_error("Event chunk capacity exceeded"));
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() == size as usize {
            if super::verification::sha256_bytes(&bytes)? != binding.2 {
                return Err(event_error("Event checksum mismatch"));
            }
            return crate::runtime::strict_json(&bytes)
                .map(Some)
                .map_err(event_error);
        }
    }
}
