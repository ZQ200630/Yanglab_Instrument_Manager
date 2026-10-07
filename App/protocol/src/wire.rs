use crate::limits::{
    MAX_CAPTURE_CHUNK, MAX_REPLY_BYTES, MAX_REQUEST_BYTES, MAX_SEQUENCE, MAX_TRACE_POINTS,
};
use serde::{
    de::{MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::{json, Map, Number, Value};
use std::{fmt, io::BufRead};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(pub String);
impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for ProtocolError {}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct DomainRef {
    pub kind: String,
    pub id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextV3 {
    pub session_id: String,
    pub domain: Option<DomainRef>,
    pub connection_id: Option<String>,
    pub epoch: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainConfig {
    pub domain: DomainRef,
    pub config_rev: u64,
    pub driver_kind: String,
    pub model_id: String,
    pub profile_id: Option<String>,
    pub params: Value,
    pub expected_identity: Value,
    pub members: Vec<DomainConfig>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestV3 {
    pub v: u8,
    pub id: String,
    pub method: String,
    pub params: Value,
    pub context: Option<ContextV3>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    RejectedBeforeCall,
    SupersededBeforeCall,
    Completed,
    CompletedReadbackFailed,
    FailedAfterCallStarted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireError {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
}
#[derive(Clone, Debug)]
pub struct OutcomeV3 {
    pub phase: Phase,
    pub context: Option<ContextV3>,
    pub result: Option<Value>,
    pub error: Option<WireError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureDescriptor {
    pub schema: u32,
    pub kind: String,
    pub capture_id: String,
    pub point_count: u64,
    pub byte_count: u64,
    pub sha256: String,
    pub metadata: Value,
}
fn error(message: impl Into<String>) -> ProtocolError {
    ProtocolError(message.into())
}
pub fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn request_id(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 64 && !value.chars().any(|c| (c as u32) < 32)
}

// Parse maps before conversion to Value so duplicates cannot disappear.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unique-key finite JSON")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Number(v.into())))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Number(v.into())))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Number::from_f64(v)
                    .map(|n| UniqueValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite JSON"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(v.into())))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(v)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueValue(v)) = seq.next_element()? {
                    values.push(v);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    values.insert(key, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}
pub fn strict_json(bytes: &[u8], maximum: usize) -> Result<Value, ProtocolError> {
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(error("JSON frame exceeds bounds"));
    }
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValue::deserialize(&mut parser)
        .map_err(|e| error(e.to_string()))?
        .0;
    parser.end().map_err(|e| error(e.to_string()))?;
    Ok(value)
}
pub fn finite_tree(value: &Value) -> bool {
    let mut pending = vec![(value, 0usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > 32 {
            return false;
        }
        match value {
            Value::Number(n) if n.is_u64() => {
                if n.as_u64().unwrap() > MAX_SEQUENCE {
                    return false;
                }
            }
            Value::Number(n) if n.is_i64() => {
                if n.as_i64().unwrap().unsigned_abs() > MAX_SEQUENCE {
                    return false;
                }
            }
            Value::Number(n) => {
                if !n.as_f64().is_some_and(f64::is_finite) {
                    return false;
                }
            }
            Value::Array(values) => pending.extend(values.iter().map(|v| (v, depth + 1))),
            Value::Object(values) => pending.extend(values.values().map(|v| (v, depth + 1))),
            _ => {}
        }
    }
    true
}
fn fields<'a>(
    value: &'a Value,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a Map<String, Value>, ProtocolError> {
    let map = value.as_object().ok_or_else(|| error("Expected object"))?;
    if required.iter().any(|k| !map.contains_key(*k))
        || map
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err(error("Unexpected or missing fields"));
    }
    Ok(map)
}
fn sequence(value: &Value, positive: bool) -> bool {
    value
        .as_u64()
        .is_some_and(|n| n <= MAX_SEQUENCE && (!positive || n > 0))
}
fn id_value(value: &Value) -> bool {
    value.as_str().is_some_and(valid_id)
}
fn hash_value(value: &Value) -> bool {
    value.as_str().is_some_and(valid_hash)
}
pub fn validate_domain(domain: &DomainRef) -> Result<(), ProtocolError> {
    if !matches!(domain.kind.as_str(), "device" | "setup") || !valid_id(&domain.id) {
        return Err(error("Invalid domain identity"));
    }
    Ok(())
}
pub fn validate_context(context: &ContextV3) -> Result<(), ProtocolError> {
    if !valid_id(&context.session_id)
        || context.epoch > MAX_SEQUENCE
        || context
            .connection_id
            .as_deref()
            .is_some_and(|id| !valid_id(id))
    {
        return Err(error("Invalid session, epoch or connection"));
    }
    if let Some(domain) = &context.domain {
        validate_domain(domain)?;
    } else if context.connection_id.is_some() || context.epoch != 0 {
        return Err(error("Invalid global context"));
    }
    Ok(())
}
fn context_from_value(value: &Value) -> Result<Option<ContextV3>, ProtocolError> {
    if value.is_null() {
        return Ok(None);
    }
    fields(
        value,
        &["session_id", "domain", "connection_id", "epoch"],
        &[],
    )?;
    if !value["domain"].is_null() {
        fields(&value["domain"], &["kind", "id"], &[])?;
    }
    let context: ContextV3 =
        serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
    validate_context(&context)?;
    Ok(Some(context))
}
pub fn validate_config(config: &DomainConfig) -> Result<(), ProtocolError> {
    validate_domain(&config.domain)?;
    if config.config_rev == 0
        || config.config_rev > MAX_SEQUENCE
        || !matches!(
            config.driver_kind.as_str(),
            "osa" | "voltage" | "gain" | "pm400" | "mdt" | "fiber"
        )
        || config.model_id.is_empty()
        || !config.params.is_object()
        || !config.expected_identity.is_object()
        || !finite_tree(&config.params)
        || !finite_tree(&config.expected_identity)
    {
        return Err(error("Invalid domain config"));
    }
    if config.domain.kind == "device" {
        if config.driver_kind == "fiber"
            || !config.members.is_empty()
            || config.profile_id.as_deref().is_none_or(str::is_empty)
        {
            return Err(error("Invalid physical device config"));
        }
    } else {
        if config.driver_kind != "fiber"
            || config.model_id != "fiber-coupling"
            || config.profile_id.is_some()
            || !(1..=2).contains(&config.members.len())
        {
            return Err(error("Invalid setup config"));
        }
        let mut ids = std::collections::HashSet::new();
        let mut serials = std::collections::HashSet::new();
        for member in &config.members {
            if member.domain.kind != "device"
                || member.driver_kind != "mdt"
                || !member.members.is_empty()
            {
                return Err(error("Invalid setup member"));
            }
            validate_config(member)?;
            let serial = member
                .expected_identity
                .get("serial")
                .or_else(|| member.expected_identity.get("transport_serial"))
                .and_then(Value::as_str)
                .ok_or_else(|| error("Missing setup serial"))?;
            if !matches!(serial, "2110148249-10" | "160721175410")
                || !ids.insert(&member.domain)
                || !serials.insert(serial)
            {
                return Err(error("Duplicate or unknown setup member"));
            }
        }
        if config.params.as_object().unwrap().keys().any(|k| {
            ![
                "voltage_limit_v",
                "toward_chip_limit_um",
                "other_limit_um",
                "serial_timeout_s",
            ]
            .contains(&k.as_str())
        }) {
            return Err(error("Unreviewed setup parameter"));
        }
    }
    Ok(())
}
fn config_from_value(value: &Value) -> Result<DomainConfig, ProtocolError> {
    fields(
        value,
        &[
            "domain",
            "config_rev",
            "driver_kind",
            "model_id",
            "profile_id",
            "params",
            "expected_identity",
            "members",
        ],
        &[],
    )?;
    fields(&value["domain"], &["kind", "id"], &[])?;
    if let Some(members) = value["members"].as_array() {
        for member in members {
            config_from_value(member)?;
        }
    }
    let config: DomainConfig =
        serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
    validate_config(&config)?;
    Ok(config)
}
const DOMAIN_METHODS: &[&str] = &[
    "probe",
    "check_online",
    "connect",
    "disconnect",
    "resume",
    "action",
];
fn validate_params(method: &str, params: &Value) -> Result<(), ProtocolError> {
    if !params.is_object() || !finite_tree(params) {
        return Err(error("Invalid parameters"));
    }
    let valid = match method {
        "ping" | "status" | "inventory" | "disconnect" | "shutdown" => {
            fields(params, &[], &[])?;
            true
        }
        "activate" => {
            fields(params, &["ownership_nonce"], &[])?;
            id_value(&params["ownership_nonce"])
        }
        "read_capture_chunk" => {
            fields(
                params,
                &["ownership_nonce", "capture_id", "offset", "length"],
                &[],
            )?;
            id_value(&params["ownership_nonce"])
                && id_value(&params["capture_id"])
                && sequence(&params["offset"], false)
                && sequence(&params["length"], true)
                && params["length"].as_u64().unwrap() <= MAX_CAPTURE_CHUNK as u64
                && params["offset"]
                    .as_u64()
                    .unwrap()
                    .checked_add(params["length"].as_u64().unwrap())
                    .is_some_and(|end| end <= (MAX_TRACE_POINTS * 16) as u64)
        }
        "ack_capture" => {
            fields(params, &["ownership_nonce", "capture_id", "sha256"], &[])?;
            id_value(&params["ownership_nonce"])
                && id_value(&params["capture_id"])
                && hash_value(&params["sha256"])
        }
        "configure_domain" => {
            fields(params, &["config"], &[])?;
            config_from_value(&params["config"])?;
            true
        }
        "retire_domain" => {
            fields(params, &["domain", "config_rev"], &[])?;
            fields(&params["domain"], &["kind", "id"], &[])?;
            let domain: DomainRef = serde_json::from_value(params["domain"].clone())
                .map_err(|e| error(e.to_string()))?;
            validate_domain(&domain)?;
            sequence(&params["config_rev"], true)
        }
        "register_verified" => {
            fields(
                params,
                &["domain", "proof_id", "config_digest", "config_rev"],
                &[],
            )?;
            fields(&params["domain"], &["kind", "id"], &[])?;
            let domain: DomainRef = serde_json::from_value(params["domain"].clone())
                .map_err(|e| error(e.to_string()))?;
            validate_domain(&domain)?;
            id_value(&params["proof_id"])
                && hash_value(&params["config_digest"])
                && sequence(&params["config_rev"], true)
        }
        "connect" => {
            fields(params, &[], &["acknowledge_lifecycle", "authorization"])?;
            if let Some(auth)=params.get("authorization") {
                fields(auth, &["stage","binding","accepted","supervised","retain_session"], &[])?;
                if auth["stage"]!="supervised" || auth["accepted"]!=true || auth["supervised"]!=true || auth["retain_session"]!=true || !auth["binding"].is_object() { return Err(error("supervised connection consent required")); }
            }
            params
                .get("acknowledge_lifecycle")
                .is_none_or(Value::is_boolean)
        }
        "resume" => {
            fields(params, &["confirm"], &[])?;
            params["confirm"] == true
        }
        "probe" | "check_online" => {
            fields(params, &["authorization"], &[])?;
            fields(
                &params["authorization"],
                &[],
                &[
                    "stage",
                    "binding",
                    "accepted",
                    "supervised",
                    "retain_session",
                ],
            )?;
            true
        }
        "action" => {
            fields(params, &["name", "args"], &[])?;
            params["name"]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.chars().count() <= 64)
                && params["args"].as_object().is_some_and(|args| {
                    !args.keys().any(|k| {
                        ["role", "driver_kind", "priority", "domain", "context"]
                            .contains(&k.as_str())
                    })
                })
        }
        _ => return Err(error("Unknown v3 method")),
    };
    if valid {
        Ok(())
    } else {
        Err(error("Invalid typed method arguments"))
    }
}
pub fn parse_request(bytes: &[u8]) -> Result<RequestV3, ProtocolError> {
    let value = strict_json(bytes, MAX_REQUEST_BYTES)?;
    fields(&value, &["v", "id", "method", "params", "context"], &[])?;
    if value["v"].as_u64() != Some(3) || !value["id"].as_str().is_some_and(request_id) {
        return Err(error("Invalid protocol version or request ID"));
    }
    let method = value["method"]
        .as_str()
        .ok_or_else(|| error("Invalid method"))?;
    validate_params(method, &value["params"])?;
    let context = context_from_value(&value["context"])?;
    if context.is_none() && !matches!(method, "ping" | "status") {
        return Err(error("Context required"));
    }
    let has_domain = context.as_ref().is_some_and(|c| c.domain.is_some());
    if DOMAIN_METHODS.contains(&method) != has_domain {
        return Err(error("Request domain does not match method scope"));
    }
    Ok(RequestV3 {
        v: 3,
        id: value["id"].as_str().unwrap().into(),
        method: method.into(),
        params: value["params"].clone(),
        context,
    })
}
pub fn encode_outcome(id: &str, outcome: &OutcomeV3) -> Result<Vec<u8>, ProtocolError> {
    if !request_id(id) {
        return Err(error("Invalid reply ID"));
    }
    if let Some(context) = &outcome.context {
        validate_context(context)?;
    }
    let success = outcome.phase == Phase::Completed;
    if success != outcome.error.is_none() || (!success && outcome.result.is_some()) {
        return Err(error("Terminal result/error disagrees with phase"));
    }
    if let Some(err) = &outcome.error {
        if err.kind.trim().is_empty()
            || err.message.trim().is_empty()
            || err
                .attempt_id
                .as_deref()
                .is_some_and(|s| s.trim().is_empty())
        {
            return Err(error("Invalid terminal error"));
        }
    }
    if outcome.result.as_ref().is_some_and(|v| !finite_tree(v)) {
        return Err(error("Invalid result values"));
    }
    let mut wire =
        json!({"v":3,"id":id,"ok":success,"phase":outcome.phase,"context":outcome.context});
    if success {
        wire["result"] = outcome.result.clone().unwrap_or(Value::Null);
    } else {
        wire["error"] = serde_json::to_value(&outcome.error).map_err(|e| error(e.to_string()))?;
    }
    let mut bytes = serde_json::to_vec(&wire).map_err(|e| error(e.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_REPLY_BYTES {
        return Err(error("Reply exceeds bounds"));
    }
    Ok(bytes)
}
pub fn read_frame(
    reader: &mut impl BufRead,
    maximum: usize,
) -> Result<Option<Vec<u8>>, ProtocolError> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|e| error(e.to_string()))?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(error("Truncated JSON frame"))
            };
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(available.len());
        if count > maximum.saturating_sub(frame.len()) {
            return Err(error("Frame exceeds bounds"));
        }
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if frame.last() == Some(&b'\n') {
            return Ok(Some(frame));
        }
    }
}
