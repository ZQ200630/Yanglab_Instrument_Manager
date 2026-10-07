//! One independent native worker protects process-local VISA leases and serial ownership.
//! Never kill a child after it may have opened hardware: EOF/shutdown lets its lifecycle
//! cleanup run, and a lost response is reported as an unknown outcome.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MAX_LOG_LINES: usize = 60;

/// Strict recursive JSON reader rejects duplicate keys before Value loses them.
struct UniqueValue(Value);
impl<'de> serde::Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("unique-key JSON")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|value| UniqueValue(Value::Number(value)))
                    .ok_or_else(|| E::custom("Nonfinite JSON"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueValue>()? {
                    values.push(value.0)
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("Duplicate JSON key"));
                    }
                    values.insert(key, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
pub(crate) fn strict_json(bytes: &[u8]) -> Result<Value, String> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let parsed = <UniqueValue as serde::Deserialize>::deserialize(&mut decoder)
        .map_err(|error| error.to_string())?;
    decoder.end().map_err(|error| error.to_string())?;
    Ok(parsed.0)
}
fn fields(value: &Value, required: &[&str], optional: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        required.iter().all(|key| object.contains_key(*key))
            && object
                .keys()
                .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
    })
}
fn finite_v3(value: &Value) -> bool {
    let mut pending = vec![(value, 0)];
    while let Some((item, depth)) = pending.pop() {
        if depth > 32 {
            return false;
        }
        match item {
            Value::Number(number) => {
                let max = crate::host::contracts::MAX_SEQUENCE;
                if number.as_u64().is_some_and(|number| number > max)
                    || number
                        .as_i64()
                        .is_some_and(|number| number.unsigned_abs() > max)
                    || number.as_f64().is_some_and(|number| !number.is_finite())
                {
                    return false;
                }
            }
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(items) => pending.extend(items.values().map(|item| (item, depth + 1))),
            _ => {}
        }
    }
    true
}
pub(crate) fn validate_domain_config(value: &Value) -> Result<(), String> {
    use crate::host::catalog::{Catalog, DOCUMENT};
    if !fields(
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
    ) || !value["config_rev"]
        .as_u64()
        .is_some_and(|revision| revision > 0 && revision <= crate::host::contracts::MAX_SEQUENCE)
        || !value["expected_identity"].is_object()
        || !value["params"].is_object()
        || !value["members"].is_array()
    {
        return Err("Invalid domain configuration".into());
    }
    let context = json!({"session_id":"a".repeat(32),"domain":value["domain"],"connection_id":null,"epoch":0});
    if !crate::reply_broker::valid_context_v3(&context) || context["domain"].is_null() {
        return Err("Invalid configured domain".into());
    }
    if value["domain"]["kind"] == "device" {
        let catalog = Catalog::load(DOCUMENT).map_err(|error| error.to_string())?;
        let model = catalog
            .model(value["model_id"].as_str().unwrap_or(""))
            .map_err(|error| error.to_string())?;
        if value["driver_kind"] != model.driver_kind
            || !value["members"].as_array().unwrap().is_empty()
        {
            return Err("Model/driver binding mismatch".into());
        }
        catalog
            .validate_connection(
                &model.id,
                value["profile_id"].as_str().unwrap_or(""),
                &value["params"],
            )
            .map_err(|error| error.to_string())?;
    } else {
        if value["driver_kind"] != "fiber"
            || value["model_id"] != "fiber-coupling"
            || !value["profile_id"].is_null()
            || !fields(
                &value["params"],
                &[],
                &[
                    "voltage_limit_v",
                    "toward_chip_limit_um",
                    "other_limit_um",
                    "serial_timeout_s",
                ],
            )
        {
            return Err("Unknown setup binding".into());
        }
        let members = value["members"].as_array().unwrap();
        if !(1..=2).contains(&members.len()) {
            return Err("Select one or two fiber members".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut serials = std::collections::HashSet::new();
        for member in members {
            validate_domain_config(member)?;
            let serial = member["expected_identity"]["serial"]
                .as_str()
                .or_else(|| member["expected_identity"]["transport_serial"].as_str())
                .unwrap_or("");
            if member["driver_kind"] != "mdt"
                || member["domain"]["kind"] != "device"
                || !["2110148249-10", "160721175410"].contains(&serial)
                || !ids.insert(member["domain"]["id"].as_str().unwrap())
                || !serials.insert(serial)
            {
                return Err("Invalid fiber member identity".into());
            }
        }
    }
    Ok(())
}
pub fn decode_v3(bytes: &[u8]) -> Result<Value, String> {
    if bytes.is_empty() || bytes.len() > 65_536 {
        return Err("V3 request exceeds 64 KiB".into());
    }
    let frame = strict_json(bytes)?;
    if !fields(&frame, &["v", "id", "method", "params", "context"], &[])
        || frame["v"].as_u64() != Some(3)
        || !finite_v3(&frame)
        || !crate::reply_broker::valid_context_v3(&frame["context"])
        || !frame["id"].as_str().is_some_and(|id| {
            !id.is_empty() && id.chars().count() <= 64 && !id.chars().any(|char| char < ' ')
        })
    {
        return Err("Invalid v3 request".into());
    }
    let method = frame["method"].as_str().ok_or("Missing method")?;
    let params = &frame["params"];
    let valid_id = crate::host::contracts::valid_id;
    let valid_params = match method {
        "ping" | "status" | "inventory" | "disconnect" | "shutdown" => fields(params, &[], &[]),
        "activate" => {
            fields(params, &["ownership_nonce"], &[])
                && params["ownership_nonce"].as_str().is_some_and(valid_id)
        }
        "read_capture_chunk" => {
            fields(
                params,
                &["ownership_nonce", "capture_id", "offset", "length"],
                &[],
            ) && params["ownership_nonce"].as_str().is_some_and(valid_id)
                && params["capture_id"].as_str().is_some_and(valid_id)
                && params["length"]
                    .as_u64()
                    .is_some_and(|n| (1..=16_384).contains(&n))
                && params["offset"]
                    .as_u64()
                    .zip(params["length"].as_u64())
                    .is_some_and(|(offset, length)| {
                        offset
                            .checked_add(length)
                            .is_some_and(|end| end <= 200_001 * 16)
                    })
        }
        "ack_capture" => {
            fields(params, &["ownership_nonce", "capture_id", "sha256"], &[])
                && params["ownership_nonce"].as_str().is_some_and(valid_id)
                && params["capture_id"].as_str().is_some_and(valid_id)
                && params["sha256"].as_str().is_some_and(valid_capture_hash)
        }
        "configure_domain" => {
            fields(params, &["config"], &[]) && validate_domain_config(&params["config"]).is_ok()
        }
        "retire_domain" => {
            fields(params, &["domain", "config_rev"], &[])
                && crate::reply_broker::valid_context_v3(
                    &json!({"session_id":"a".repeat(32),"domain":params["domain"],"connection_id":null,"epoch":0}),
                )
                && !params["domain"].is_null()
                && params["config_rev"].as_u64().is_some_and(|revision| {
                    revision > 0 && revision <= crate::host::contracts::MAX_SEQUENCE
                })
        }
        "register_verified" => {
            fields(
                params,
                &["domain", "proof_id", "config_digest", "config_rev"],
                &[],
            ) && serde_json::from_value::<crate::host::leases::DomainRef>(params["domain"].clone())
                .is_ok_and(|domain| domain.validate().is_ok())
                && params["proof_id"].as_str().is_some_and(valid_id)
                && params["config_digest"].as_str().is_some_and(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
                && params["config_rev"]
                    .as_u64()
                    .is_some_and(|rev| rev > 0 && rev <= crate::host::contracts::MAX_SEQUENCE)
        }
        "connect" => {
            fields(params, &[], &["acknowledge_lifecycle"])
                && params
                    .get("acknowledge_lifecycle")
                    .map_or(true, Value::is_boolean)
        }
        "resume" => fields(params, &["confirm"], &[]) && params["confirm"] == true,
        "probe" | "check_online" => {
            fields(params, &["authorization"], &[])
                && fields(
                    &params["authorization"],
                    &[],
                    &[
                        "stage",
                        "binding",
                        "accepted",
                        "supervised",
                        "retain_session",
                    ],
                )
        }
        "action" => {
            fields(params, &["name", "args"], &[])
                && params["name"]
                    .as_str()
                    .is_some_and(|name| !name.is_empty() && name.chars().count() <= 64)
                && params["args"].as_object().is_some_and(|args| {
                    !args.keys().any(|key| {
                        ["role", "driver_kind", "priority", "domain", "context"]
                            .contains(&key.as_str())
                    })
                })
        }
        _ => false,
    };
    let domain_method = [
        "probe",
        "check_online",
        "connect",
        "disconnect",
        "resume",
        "action",
    ]
    .contains(&method);
    if !valid_params
        || (frame["context"].is_null() && !["ping", "status"].contains(&method))
        || (domain_method && frame["context"]["domain"].is_null())
        || (!domain_method && !frame["context"]["domain"].is_null())
    {
        return Err("Invalid v3 method parameters/context".into());
    }
    Ok(frame)
}

fn valid_capture_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_capture_stamp(value: &str) -> bool {
    let Some(value) = value.strip_suffix("+00:00") else {
        return false;
    };
    let (clock, fraction) = value
        .split_once('.')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    if clock.len() != 19
        || !clock.is_ascii()
        || fraction
            .is_some_and(|s| s.is_empty() || s.len() > 6 || !s.bytes().all(|b| b.is_ascii_digit()))
        || [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')]
            .iter()
            .any(|(i, b)| clock.as_bytes()[*i] != *b)
    {
        return false;
    }
    let values: Option<Vec<u32>> = [(0, 4), (5, 7), (8, 10), (11, 13), (14, 16), (17, 19)]
        .iter()
        .map(|(first, last)| {
            let text = &clock[*first..*last];
            if text.bytes().all(|b| b.is_ascii_digit()) {
                text.parse().ok()
            } else {
                None
            }
        })
        .collect();
    let Some(v) = values else {
        return false;
    };
    let year = v[0];
    let month = v[1];
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    year > 0 && days > 0 && (1..=days).contains(&v[2]) && v[3] < 24 && v[4] < 60 && v[5] < 60
}

pub(crate) fn validate_capture_descriptor(value: &Value) -> Result<(), String> {
    if !fields(
        value,
        &[
            "schema",
            "kind",
            "capture_id",
            "point_count",
            "byte_count",
            "sha256",
            "metadata",
        ],
        &[],
    ) || value["schema"].as_u64() != Some(1)
        || value["kind"] != "osa_trace"
        || !value["capture_id"]
            .as_str()
            .is_some_and(crate::host::contracts::valid_id)
        || !value["sha256"].as_str().is_some_and(valid_capture_hash)
        || !value["point_count"]
            .as_u64()
            .is_some_and(|n| (1..=200_001).contains(&n))
        || value["byte_count"].as_u64() != value["point_count"].as_u64().map(|n| n * 16)
        || !finite_v3(value)
    {
        return Err("Invalid native capture descriptor".into());
    }
    let metadata = &value["metadata"];
    if !fields(
        metadata,
        &[
            "native_unit",
            "trace",
            "identity",
            "read_started_at",
            "read_finished_at",
            "elapsed_s",
            "context_before",
            "context_after",
            "consistency",
        ],
        &[],
    ) || serde_json::to_vec(metadata)
        .map_err(|e| e.to_string())?
        .len()
        > 8192
        || !metadata["native_unit"]
            .as_str()
            .is_some_and(|u| matches!(u, "dBm" | "W"))
        || !metadata["trace"]
            .as_str()
            .is_some_and(|t| matches!(t, "A" | "B" | "C" | "D" | "E" | "F" | "G"))
        || !metadata["identity"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
        || !["read_started_at", "read_finished_at"]
            .iter()
            .all(|k| metadata[*k].as_str().is_some_and(valid_capture_stamp))
        || !metadata["elapsed_s"]
            .as_f64()
            .is_some_and(|n| n.is_finite() && n >= 0.)
        || metadata["consistency"] != "unproven"
        || metadata["context_before"] != metadata["context_after"]
    {
        return Err("Invalid native capture metadata".into());
    }
    let context = &metadata["context_before"];
    let unit_code = if metadata["native_unit"] == "W" { 1 } else { 0 };
    if !fields(
        context,
        &[
            "transfer_format",
            "sample_count",
            "spacing",
            "level_unit",
            "x_unit",
            "trace_attribute",
            "active_trace",
            "center_m",
            "span_m",
            "resolution_m",
            "sweep_mode",
        ],
        &[],
    ) || context["sample_count"] != value["point_count"]
        || !context["transfer_format"]
            .as_str()
            .is_some_and(|s| matches!(s, "ASCII" | "REAL,32" | "REAL,64"))
        || context["spacing"].as_u64() != Some(unit_code)
        || context["level_unit"].as_u64() != Some(unit_code)
        || context["x_unit"].as_u64() != Some(0)
        || !context["trace_attribute"].as_u64().is_some_and(|n| n <= 4)
        || context["active_trace"] != format!("TR{}", metadata["trace"].as_str().unwrap())
        || !context["sweep_mode"]
            .as_u64()
            .is_some_and(|n| (1..=3).contains(&n))
        || !["center_m", "resolution_m"].iter().all(|k| {
            context[*k]
                .as_f64()
                .is_some_and(|n| n > 0. && n.is_finite())
        })
        || !context["span_m"]
            .as_f64()
            .is_some_and(|n| n >= 0. && n.is_finite())
    {
        return Err("Unsupported or inconsistent native capture context".into());
    }
    Ok(())
}

pub(crate) fn validate_capture_chunk(
    value: &Value,
    descriptor: &Value,
    offset: u64,
    length: u64,
) -> Result<Vec<u8>, String> {
    validate_capture_descriptor(descriptor)?;
    if !(1..=16_384).contains(&length)
        || !offset
            .checked_add(length)
            .is_some_and(|end| end <= descriptor["byte_count"].as_u64().unwrap())
        || !fields(
            value,
            &["capture_id", "offset", "byte_count", "sha256", "data_hex"],
            &[],
        )
        || value["capture_id"] != descriptor["capture_id"]
        || value["sha256"] != descriptor["sha256"]
        || value["offset"].as_u64() != Some(offset)
        || value["byte_count"].as_u64() != Some(length)
    {
        return Err("Capture chunk does not match its reference and range".into());
    }
    let hex = value["data_hex"]
        .as_str()
        .ok_or("Capture chunk must contain hex bytes")?;
    if hex.len() != length as usize * 2
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Capture chunk has invalid hex byte count".into());
    }
    let digit = |b: u8| {
        if b.is_ascii_digit() {
            b - b'0'
        } else {
            b - b'a' + 10
        }
    };
    Ok(hex
        .as_bytes()
        .chunks_exact(2)
        .map(|b| digit(b[0]) * 16 + digit(b[1]))
        .collect())
}

fn create_capture_spool(nonce: &str) -> Result<PathBuf, String> {
    if !crate::host::contracts::valid_id(nonce) {
        return Err("Invalid capture ownership nonce".into());
    }
    let base = std::env::temp_dir();
    // Check every existing ancestor before creating a private random directory.
    for parent in base.ancestors() {
        let info = std::fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            info.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if !info.is_dir() || info.file_type().is_symlink() || reparse {
            return Err("Capture staging parent must be an ordinary directory".into());
        }
    }
    let private = base.join(format!(
        "yang-lab-capture-{}",
        crate::host::registry::new_id().map_err(|e| e.to_string())?
    ));
    std::fs::create_dir(&private).map_err(|e| e.to_string())?;
    let root = private.join(nonce);
    std::fs::create_dir(&root).map_err(|e| e.to_string())?;
    // Orphans are intentionally retained, not deleted before verified transfer.
    Ok(root)
}

// The sole live runtime owns correlation, bounded writer admission and lifecycle.

#[derive(Clone)]
pub struct RuntimeConfig {
    pub launch: crate::native_worker::NativeWorkerLaunch,
    pub catalog_root: PathBuf,
    pub mode: String,
    pub protocol: u64,
    pub ownership_nonce: Option<String>,
    pub record_child: Option<Arc<dyn Fn(&ChildIdentity) -> Result<(), String> + Send + Sync>>,
}
#[derive(Clone)]
pub struct ChildIdentity {
    pub pid: u32,
    pub creation_time: String,
    pub ownership_nonce: String,
    pub image_path: PathBuf,
    pub image_sha256: String,
    pub package_revision: String,
}
pub enum WorkerRequest {
    V2(Value),
    V3(Value),
}
pub struct RuntimeError {
    pub message: String,
    pub retained_runtime: Option<Arc<WorkerRuntime>>,
}
impl std::fmt::Debug for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("RuntimeError")
            .field("message", &self.message)
            .field("responsibility_retained", &self.retained_runtime.is_some())
            .finish()
    }
}
impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for RuntimeError {}
pub struct PendingReply {
    runtime: Arc<WorkerRuntime>,
    frame: Value,
    admitted: Admitted,
}
impl PendingReply {
    pub async fn wait_async(self, timeout: Duration) -> Result<Value, RuntimeError> {
        let deadline = Instant::now() + timeout;
        let retained = self.runtime.clone();
        let fail = move |message| RuntimeError {
            message,
            retained_runtime: Some(retained.clone()),
        };
        loop {
            match self.admitted.receiver.try_recv() {
                Ok(response) => {
                    return self
                        .runtime
                        .finish_response(&self.frame, self.admitted.generation, response)
                        .map_err(fail)
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(fail(self.runtime.broker.failure().unwrap_or_else(|| {
                        "Worker transport ended; outcome unknown".into()
                    })))
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                self.runtime
                    .broker
                    .mark_timeout(self.frame["id"].as_str().unwrap());
                return Err(fail(
                    "Worker reply deadline expired; responsibility remains tracked".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    pub fn wait(self, timeout: Duration) -> Result<Value, RuntimeError> {
        self.runtime
            .finish_exchange(&self.frame, timeout, self.admitted)
            .map_err(|message| RuntimeError {
                message,
                retained_runtime: Some(self.runtime.clone()),
            })
    }
}
#[derive(Clone, Debug)]
pub struct StopReport {
    pub cleanup: Value,
    pub process_exit: Option<Value>,
    pub resource_released: bool,
}
impl StopReport {
    pub fn claims_physical_zero(&self) -> bool {
        false
    }
}
fn read_reply_frame(reader: &mut impl BufRead, maximum: usize) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    let count = reader
        .take((maximum + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| format!("stdout read failed: {error}"))?;
    if count == 0 {
        return Ok(None);
    }
    if count > maximum {
        return Err("worker reply capacity exceeded; outcome unknown".into());
    }
    strict_json(&bytes).map(Some)
}

#[derive(Default)]
struct DomainRoutes {
    kinds: std::collections::HashMap<String, String>,
    pending: std::collections::HashMap<String, (String, bool, bool)>,
}
impl DomainRoutes {
    fn prepare(&mut self, request: &Value) -> Result<(), String> {
        let method = request["method"].as_str().unwrap();
        if !matches!(method, "configure_domain" | "retire_domain") {
            return Ok(());
        }
        let config = &request["params"]["config"];
        let domain = if method == "configure_domain" {
            &config["domain"]
        } else {
            &request["params"]["domain"]
        };
        let key = crate::reply_broker::domain_key(&json!({"domain":domain}))
            .ok_or("Invalid route domain")?;
        if self.pending.values().any(|(pending, _, _)| pending == &key) {
            return Err("Domain management is already pending".into());
        }
        let fresh = !self.kinds.contains_key(&key);
        if method == "configure_domain" {
            let kind = config["driver_kind"]
                .as_str()
                .ok_or("Missing driver kind")?;
            if fresh && self.kinds.len() >= 64 {
                return Err("Execution-domain capacity is 64".into());
            }
            if self.kinds.get(&key).is_some_and(|old| old != kind) {
                return Err("Domain driver kind cannot change".into());
            }
            self.kinds.entry(key.clone()).or_insert_with(|| kind.into());
        } else if fresh {
            return Err("Unknown route domain".into());
        }
        self.pending.insert(
            request["id"].as_str().unwrap().into(),
            (key, method == "configure_domain", fresh),
        );
        Ok(())
    }
    fn complete(&mut self, reply: &Value) {
        if let Some((key, configure, fresh)) =
            reply["id"].as_str().and_then(|id| self.pending.remove(id))
        {
            if (!configure && reply["ok"] == true) || (configure && fresh && reply["ok"] != true) {
                self.kinds.remove(&key);
            }
        }
    }
    fn rollback(&mut self, id: &str) {
        self.complete(&json!({"id":id,"ok":false}));
    }
}
pub struct WorkerRuntime {
    _launch: crate::native_worker::NativeWorkerLaunch,
    protocol: u64,
    capture_nonce: Option<String>,
    domain_routes: Arc<Mutex<DomainRoutes>>,
    pub(crate) child: Mutex<Child>,
    pub(crate) ready: AtomicBool,
    pub(crate) stderr_tail: Arc<Mutex<VecDeque<String>>>,
    pub(crate) writer: Arc<crate::request_writer::RequestWriter>,
    pub(crate) broker: Arc<crate::reply_broker::ReplyBroker>,
    pub(crate) admission: Mutex<()>,
    pub(crate) session: Mutex<Option<String>>,
    pub(crate) lifecycle: Arc<Mutex<RuntimeLifecycle>>,
    pub(crate) waits: Arc<Mutex<Vec<crate::request_writer::DispatchClass>>>,
}
pub(crate) struct WaitPermit {
    slots: Arc<Mutex<Vec<crate::request_writer::DispatchClass>>>,
    class: crate::request_writer::DispatchClass,
}
impl Drop for WaitPermit {
    fn drop(&mut self) {
        let mut slots = self.slots.lock().unwrap();
        let index = slots.iter().position(|c| *c == self.class).unwrap();
        slots.remove(index);
    }
}
pub(crate) struct Admitted {
    pub(crate) receiver: std::sync::mpsc::Receiver<Value>,
    pub(crate) generation: u64,
    pub(crate) _permit: WaitPermit,
}
pub(crate) enum AdmissionError {
    BeforeSend(String),
    Uncertain(String),
}
impl AdmissionError {
    pub(crate) fn message(self) -> String {
        match self {
            Self::BeforeSend(s) | Self::Uncertain(s) => s,
        }
    }
}
#[derive(Default)]
pub(crate) struct RuntimeLifecycle {
    pub(crate) checking: bool,
    pub(crate) closing: bool,
    pub(crate) active: Option<String>,
    pub(crate) released: Option<Value>,
    pub(crate) reports: Vec<Value>,
    pub(crate) exit: Option<Value>,
}
pub(crate) struct ShutdownReservation(Arc<Mutex<RuntimeLifecycle>>);
impl Drop for ShutdownReservation {
    fn drop(&mut self) {
        self.0.lock().unwrap().checking = false;
    }
}
pub(crate) static NEXT_RUNTIME_ATTEMPT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);
impl WorkerRuntime {
    pub fn protocol(&self) -> u64 {
        self.protocol
    }
    pub(crate) fn global_context(&self) -> Result<Value, String> {
        let session = self
            .session
            .lock()
            .unwrap()
            .clone()
            .ok_or("Worker session is not established")?;
        Ok(json!({"session_id":session,"domain":null,"connection_id":null,"epoch":0}))
    }
    pub(crate) fn domain_pending(&self, key: &str) -> usize {
        self.broker.domain_pending(key)
    }
    pub(crate) fn transport_failure(&self) -> Option<String> {
        self.broker.failure()
    }
    pub(crate) fn domain_context(&self, key: &str) -> Option<Value> {
        self.broker.domain_context(key)
    }
    pub(crate) fn fence_domain(&self, key: &str) -> Result<usize, String> {
        let _admission = self.admission.lock().unwrap();
        self.broker.control_changed();
        self.writer.fence_domain(key)
    }
    pub fn spawn(config: RuntimeConfig) -> Result<Arc<Self>, RuntimeError> {
        let reject = |message: String| RuntimeError {
            message,
            retained_runtime: None,
        };
        if config.mode != "real"
            || config.protocol != 3
            || config.record_child.is_none()
            || !config
                .ownership_nonce
                .as_deref()
                .is_some_and(crate::host::contracts::valid_id)
        {
            return Err(reject(
                "Native startup requires real mode, protocol 3, durable child record and nonce"
                    .into(),
            ));
        }
        if !config.catalog_root.is_absolute() || !config.catalog_root.is_dir() {
            return Err(reject("Native catalog directory is unavailable".into()));
        }
        let runtime = Self::spawn_transport(config.launch, 3, config.ownership_nonce.as_deref())
            .map_err(reject)?;
        let startup = (|| -> Result<(), String> {
            let reply = runtime.exchange(&json!({"v":3,"id":format!("startup-{}",NEXT_RUNTIME_ATTEMPT.fetch_add(1,Ordering::Relaxed)),"method":"ping","params":{},"context":null}), Duration::from_secs(15))?;
            if reply["ok"] != true {
                return Err(response_error(&reply));
            }
            let mut raw_identity = reply["result"].clone();
            raw_identity
                .as_object_mut()
                .ok_or("Native identity must be an object")?
                .remove("host_transport");
            let identity: yang_protocol::NativeIdentity =
                serde_json::from_value(raw_identity).map_err(|e| e.to_string())?;
            identity.validate_startup().map_err(|e| e.to_string())?;
            if Path::new(&identity.executable)
                .canonicalize()
                .map_err(|e| e.to_string())?
                != runtime._launch.executable()
                || identity.package_revision != runtime._launch.package().package_revision
            {
                return Err("Native worker executable or package identity mismatch".into());
            }
            let nonce = config.ownership_nonce.as_deref().unwrap();
            let child = runtime.child_identity(nonce)?;
            config.record_child.as_ref().unwrap()(&child)?;
            let activated = runtime.exchange(&json!({"v":3,"id":format!("activation-{}",NEXT_RUNTIME_ATTEMPT.fetch_add(1,Ordering::Relaxed)),"method":"activate","params":{"ownership_nonce":nonce},"context":{"session_id":identity.session_id,"domain":null,"connection_id":null,"epoch":0}}),Duration::from_secs(15))?;
            if activated["ok"] != true || activated["result"]["activated"] != true {
                return Err("Native activation unconfirmed".into());
            }
            runtime.ready.store(true, Ordering::Release);
            Ok(())
        })();
        match startup {
            Ok(()) => Ok(runtime),
            Err(message) => Err(RuntimeError {
                message,
                retained_runtime: Some(runtime),
            }),
        }
    }
    fn child_identity(&self, nonce: &str) -> Result<ChildIdentity, String> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::FILETIME;
            use windows_sys::Win32::System::Threading::{
                GetProcessTimes, QueryFullProcessImageNameW,
            };
            let child = self
                .child
                .lock()
                .map_err(|_| "Child ownership lock poisoned")?;
            let mut created = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut exited = created;
            let mut kernel = created;
            let mut user = created;
            // SAFETY: live owned process handle and four valid FILETIME output pointers.
            if unsafe {
                GetProcessTimes(
                    child.as_raw_handle() as _,
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                )
            } == 0
            {
                return Err("Could not verify worker process creation time".into());
            }
            let stamp = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
            let mut image = vec![0u16; 32768];
            let mut length = image.len() as u32;
            // SAFETY: owned live process handle and bounded writable UTF-16 buffer.
            if unsafe {
                QueryFullProcessImageNameW(
                    child.as_raw_handle() as _,
                    0,
                    image.as_mut_ptr(),
                    &mut length,
                )
            } == 0
            {
                return Err("Native process image identity unavailable".into());
            }
            use std::os::windows::ffi::OsStringExt;
            let image_path =
                PathBuf::from(std::ffi::OsString::from_wide(&image[..length as usize]))
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
            if image_path != self._launch.executable() {
                return Err("Spawned native process image differs from pinned package".into());
            }
            Ok(ChildIdentity {
                pid: child.id(),
                creation_time: format!("{stamp:016x}"),
                ownership_nonce: nonce.into(),
                image_path,
                image_sha256: self._launch.package().worker_sha256.clone(),
                package_revision: self._launch.package().package_revision.clone(),
            })
        }
        #[cfg(not(windows))]
        {
            Err("Windows process identity required".into())
        }
    }
    pub fn submit(self: &Arc<Self>, request: WorkerRequest) -> Result<PendingReply, RuntimeError> {
        let (version, frame) = match request {
            WorkerRequest::V2(frame) => (2, frame),
            WorkerRequest::V3(frame) => (3, frame),
        };
        let fail = |message| RuntimeError {
            message,
            retained_runtime: Some(self.clone()),
        };
        if version != self.protocol {
            return Err(fail("Request protocol does not match this runtime".into()));
        }
        if !self.ready.load(Ordering::Acquire)
            && !matches!(
                frame["method"].as_str(),
                Some("ping" | "status" | "shutdown")
            )
        {
            return Err(fail("Worker startup is disarmed".into()));
        }
        let admitted = self.admit(&frame).map_err(|error| fail(error.message()))?;
        Ok(PendingReply {
            runtime: self.clone(),
            frame,
            admitted,
        })
    }
    pub fn stop(self: &Arc<Self>) -> Result<StopReport, RuntimeError> {
        let cleanup = self.shutdown().map_err(|message| RuntimeError {
            message,
            retained_runtime: Some(self.clone()),
        })?;
        let exit = self.lifecycle.lock().unwrap().exit.clone();
        Ok(StopReport {
            resource_released: release_verified(&cleanup),
            cleanup,
            process_exit: exit,
        })
    }
    fn domain_kind(&self, request: &Value) -> Option<String> {
        crate::reply_broker::domain_key(&request["context"])
            .and_then(|key| self.domain_routes.lock().unwrap().kinds.get(&key).cloned())
    }
    fn target_key(&self, request: &Value) -> Option<String> {
        if self.protocol == 3 {
            crate::reply_broker::domain_key(&request["context"])
        } else {
            request["params"]["role"].as_str().map(str::to_owned)
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn_pipe_fixture(
        root: &Path,
        mode: &str,
    ) -> Result<Arc<Self>, String> {
        if mode != "real" {
            return Err("only real hardware is supported".into());
        }
        let launch =
            crate::native_worker::NativeWorkerLaunch::test_fixture(root).map_err(|e| e.message)?;
        Self::spawn_transport(launch, 2, None)
    }
    fn spawn_transport(
        launch: crate::native_worker::NativeWorkerLaunch,
        protocol: u64,
        nonce: Option<&str>,
    ) -> Result<Arc<Self>, String> {
        let mut command = launch.command();
        command
            .arg("--real")
            .arg("--protocol")
            .arg(protocol.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(nonce) = nonce {
            let spool = create_capture_spool(nonce)?;
            command
                .arg("--ownership-nonce")
                .arg(nonce)
                .arg("--capture-spool")
                .arg(spool);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("could not start native worker: {e}"))?;
        let stdin = child.stdin.take().ok_or("stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("stderr unavailable")?;
        let broker = Arc::new(crate::reply_broker::ReplyBroker::for_protocol(protocol)?);
        let writer =
            crate::request_writer::RequestWriter::for_protocol(stdin, broker.clone(), protocol)?;
        let lifecycle = Arc::new(Mutex::new(RuntimeLifecycle::default()));
        let reader_lifecycle = lifecycle.clone();
        let reader_broker = broker.clone();
        let reader_writer = Arc::downgrade(&writer);
        let domain_routes = Arc::new(Mutex::new(DomainRoutes::default()));
        let reader_routes = domain_routes.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let frame = match read_reply_frame(&mut reader, 17 * 1024 * 1024) {
                    Ok(Some(frame)) => Ok(frame),
                    Ok(None) => break,
                    Err(error) => Err(error),
                };
                match frame {
                    Ok(mut frame) => {
                        let id = frame["id"].as_str().map(str::to_owned);
                        // Preserve late cleanup evidence independently of the finite
                        // broker history and independently of the caller's deadline.
                        if let Err(error) = reader_broker.validate_delivery(&frame) {
                            reader_broker.transport_failed(&error);
                            break;
                        }
                        if protocol == 3 && frame["result"].get("all_resources_released").is_some()
                        {
                            match normalize_native_cleanup(&frame["result"]) {
                                Ok(report) => frame["result"] = report,
                                Err(error) => {
                                    reader_broker.transport_failed(&error);
                                    break;
                                }
                            }
                        }
                        {
                            let mut state = reader_lifecycle.lock().unwrap();
                            if id.is_some() && state.active.as_deref() == id.as_deref() {
                                state.reports.push(frame.clone());
                                state.active = None;
                                if frame["ok"] == true && release_verified(&frame["result"]) {
                                    state.released = Some(frame["result"].clone());
                                }
                            }
                        }
                        // Settle admission before waking the original waiter. No I/O lock is held.
                        if protocol == 3 {
                            reader_routes.lock().unwrap().complete(&frame);
                        }
                        if let (Some(writer), Some(id)) = (reader_writer.upgrade(), id.as_deref()) {
                            writer.complete(id);
                        }
                        if reader_broker.deliver(frame).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        reader_broker.transport_failed(&error);
                        break;
                    }
                }
            }
            reader_broker.transport_failed("worker stdout ended; transport outcome unknown");
            if let Some(writer) = reader_writer.upgrade() {
                writer.close();
            }
        });
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let log_store = stderr_tail.clone();
        thread::spawn(move || collect_stderr(stderr, log_store));
        Ok(Arc::new(Self {
            _launch: launch,
            protocol,
            capture_nonce: nonce.map(str::to_owned),
            domain_routes,
            child: Mutex::new(child),
            ready: AtomicBool::new(false),
            stderr_tail,
            writer,
            broker,
            admission: Mutex::new(()),
            session: Mutex::new(None),
            lifecycle,
            waits: Arc::new(Mutex::new(Vec::new())),
        }))
    }
    pub fn exchange(&self, request: &Value, timeout: Duration) -> Result<Value, String> {
        let admitted = self.admit(request).map_err(AdmissionError::message)?;
        self.finish_exchange(request, timeout, admitted)
    }
    fn capture_exchange(&self, method: &str, mut params: Value) -> Result<Value, String> {
        if self.protocol != 3 || !self.ready.load(Ordering::Acquire) {
            return Err("Owned V3 capture staging is unavailable".into());
        }
        let nonce = self
            .capture_nonce
            .as_deref()
            .ok_or("Capture ownership is unavailable")?;
        let session = self
            .session
            .lock()
            .unwrap()
            .clone()
            .ok_or("Capture session is unavailable")?;
        params["ownership_nonce"] = json!(nonce);
        let frame = json!({"v":3,"id":crate::host::registry::new_id().map_err(|e| e.to_string())?,
            "method":method,"params":params,"context":{"session_id":session,"domain":null,"connection_id":null,"epoch":0}});
        let response = self.exchange(&frame, Duration::from_secs(5))?;
        if response["ok"] != true {
            return Err(response_error(&response));
        }
        if serde_json::to_vec(&response)
            .map_err(|e| e.to_string())?
            .len()
            > 65_536
        {
            return Err("Capture reply exceeds the control envelope".into());
        }
        Ok(response["result"].clone())
    }
    pub(crate) fn read_capture_chunk(
        &self,
        descriptor: &Value,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, String> {
        validate_capture_descriptor(descriptor)?;
        if !(1..=16_384).contains(&length)
            || !offset
                .checked_add(length)
                .is_some_and(|end| end <= descriptor["byte_count"].as_u64().unwrap())
        {
            return Err("Invalid bounded capture range".into());
        }
        let chunk = self.capture_exchange(
            "read_capture_chunk",
            json!({"capture_id":descriptor["capture_id"],"offset":offset,"length":length}),
        )?;
        validate_capture_chunk(&chunk, descriptor, offset, length)
    }
    pub(crate) fn ack_capture(&self, descriptor: &Value) -> Result<(), String> {
        validate_capture_descriptor(descriptor)?;
        let result = self.capture_exchange(
            "ack_capture",
            json!({"capture_id":descriptor["capture_id"],"sha256":descriptor["sha256"]}),
        )?;
        if result != json!({"acknowledged":true}) {
            return Err("Capture acknowledgement was not confirmed".into());
        }
        Ok(())
    }
    pub(crate) fn admit(&self, request: &Value) -> Result<Admitted, AdmissionError> {
        use crate::request_writer::{classify, DispatchClass};
        use AdmissionError::{BeforeSend, Uncertain};
        if request["v"].as_u64() != Some(self.protocol) {
            return Err(BeforeSend("Protocol does not match this worker".into()));
        }
        let kind = self.domain_kind(request);
        let class = if self.protocol == 3 {
            crate::request_writer::classify_v3(request, kind.as_deref())
                .map_err(BeforeSend)?
                .0
        } else {
            classify(request).map_err(Uncertain)?
        };
        let method = request["method"].as_str().unwrap();
        let id = request["id"].as_str().unwrap();
        let target = self.target_key(request);
        let role = if [
            "action",
            "connect",
            "disconnect",
            "resume",
            "probe",
            "check_online",
        ]
        .contains(&method)
        {
            target.as_deref()
        } else {
            None
        };
        {
            let _admission = self.admission.lock().unwrap();
            if let Some(error) = self.broker.failure() {
                return Err(Uncertain(error));
            }
            // Keep duplicate/recent IDs distinct from a new before-send rejection.
            let receiver = self
                .broker
                .register_runtime(id, role, &request["context"])
                .map_err(|error| match error {
                    crate::reply_broker::RegistrationError::Full => {
                        BeforeSend("pending reply capacity exhausted; request not sent".into())
                    }
                    crate::reply_broker::RegistrationError::Other(message) => Uncertain(message),
                })?;
            let reject = |message: &str| {
                self.broker.withdraw(id);
                BeforeSend(message.into())
            };
            if class == DispatchClass::Normal && method != "resume" {
                if self.lifecycle.lock().unwrap().closing {
                    return Err(reject("session is closing"));
                }
                if role.is_some_and(|r| self.broker.restricted(r)) {
                    return Err(reject("role outcome unknown; inspect, stop, or explicitly resume after backend authorization"));
                }
            }
            if method == "resume" && self.broker.has_timed_out(role.unwrap()) {
                return Err(reject(
                    "earlier role attempt remains pending; final evidence required before resume",
                ));
            }
            let limit = match &class {
                DispatchClass::Normal => 31,
                DispatchClass::Safety(_) if self.protocol == 3 => 3,
                DispatchClass::Safety(role) => match role.as_str() {
                    "gain" => 3,
                    "voltage" => 2,
                    _ => 1,
                },
                _ => 1,
            };
            let mut slots = self.waits.lock().unwrap();
            if slots.iter().filter(|c| **c == class).count() >= limit {
                return Err(reject("host wait capacity full; request not sent"));
            }
            slots.push(class.clone());
            let permit = WaitPermit {
                slots: self.waits.clone(),
                class: class.clone(),
            };
            drop(slots);
            if matches!(class, DispatchClass::Safety(_) | DispatchClass::Shutdown) {
                self.broker.control_changed();
            }
            let generation = self.broker.generation();
            if self.protocol == 3 {
                if let Err(error) = self.domain_routes.lock().unwrap().prepare(request) {
                    self.broker.withdraw(id);
                    return Err(BeforeSend(error));
                }
            }
            let sent = if self.protocol == 3 {
                self.writer.enqueue_v3(request.clone(), kind.as_deref())
            } else {
                self.writer.enqueue(request.clone(), class)
            };
            if let Err(error) = sent {
                if self.protocol == 3 {
                    self.domain_routes.lock().unwrap().rollback(id);
                }
                self.broker.withdraw(id);
                return Err(if self.broker.failure().is_some() {
                    Uncertain(error)
                } else {
                    BeforeSend(error)
                });
            }
            Ok(Admitted {
                receiver,
                generation,
                _permit: permit,
            })
        }
    }
    pub(crate) fn finish_exchange(
        &self,
        request: &Value,
        timeout: Duration,
        admitted: Admitted,
    ) -> Result<Value, String> {
        let Admitted {
            receiver,
            generation,
            _permit,
        } = admitted;
        let method = request["method"].as_str().unwrap();
        let id = request["id"].as_str().unwrap();
        match receiver.recv_timeout(timeout) {
            Ok(response) => self.finish_response(request, generation, response),
            Err(RecvTimeoutError::Timeout) => {
                self.broker.mark_timeout(id);
                Err(format!("{method} timed out; delivery unconfirmed and device outcome unknown; attempt {id} remains tracked{}", self.log_suffix()))
            }
            Err(RecvTimeoutError::Disconnected) => Err(format!(
                "{}; outcome unknown{}",
                self.broker
                    .failure()
                    .unwrap_or_else(|| "worker transport ended; outcome unknown".into()),
                self.log_suffix()
            )),
        }
    }
    fn finish_response(
        &self,
        request: &Value,
        generation: u64,
        mut response: Value,
    ) -> Result<Value, String> {
        let method = request["method"].as_str().unwrap();
        if let Some(session) = response["result"]["session_id"]
            .as_str()
            .or_else(|| response["context"]["session_id"].as_str())
        {
            let mut current = self.session.lock().unwrap();
            if current.as_ref().is_some_and(|old| old != session) {
                self.broker
                    .transport_failed("worker session changed unexpectedly");
                return Err("worker session changed; outcome unknown".into());
            }
            *current = Some(session.into());
        }
        if method == "resume"
            && response["ok"] == true
            && response["result"]["resumed"] == true
            && response["context"] == request["context"]
        {
            if !self.broker.recover(
                self.target_key(request).as_deref().unwrap(),
                &response["context"],
                generation,
            ) {
                return Err("backend resume returned but host recovery evidence changed; outcome remains restricted".into());
            }
        }
        if ["status", "ping"].contains(&method) && response["ok"] == true {
            let Some(result) = response["result"].as_object_mut() else {
                self.broker
                    .transport_failed("invalid query payload; outcome unknown");
                return Err("invalid query payload; outcome unknown".into());
            };
            result.insert("host_transport".into(), self.transport_status());
        }
        Ok(response)
    }
    pub(crate) fn log_suffix(&self) -> String {
        let tail = self.stderr_tail.lock().unwrap();
        if tail.is_empty() {
            String::new()
        } else {
            format!(
                " (stderr tail: {})",
                tail.iter().cloned().collect::<Vec<_>>().join("\\n")
            )
        }
    }
    fn transport_status(&self) -> Value {
        if self.protocol == 3 {
            let domains: serde_json::Map<String, Value> = self
                .domain_routes
                .lock()
                .unwrap()
                .kinds
                .keys()
                .map(|key| (key.clone(), Value::Bool(self.broker.restricted(key))))
                .collect();
            return json!({"failure":self.broker.failure(),"domains":domains,
                "closing":self.lifecycle.lock().unwrap().closing});
        }
        let roles: serde_json::Map<String, Value> = ["osa", "voltage", "gain", "pm400", "fiber"]
            .into_iter()
            .map(|role| (role.into(), Value::Bool(self.broker.restricted(role))))
            .collect();
        json!({"failure": self.broker.failure(), "roles": roles,
            "closing": self.lifecycle.lock().unwrap().closing})
    }
    /// Immutable snapshots retain original attempt reports separately from exit evidence.
    pub fn shutdown_evidence(&self) -> Value {
        let state = self.lifecycle.lock().unwrap();
        json!({"active_attempt":state.active,"attempts":state.reports,"received_cleanup_report":state.released,"process_exit":state.exit})
    }
    pub fn shutdown(&self) -> Result<Value, String> {
        self.shutdown_with_limits(Duration::from_secs(90), Duration::from_secs(5))
    }
    pub(crate) fn shutdown_with_limits(
        &self,
        reply_timeout: Duration,
        exit_timeout: Duration,
    ) -> Result<Value, String> {
        let _reservation = self.reserve_shutdown()?;
        self.shutdown_attempt(reply_timeout, exit_timeout)
    }
    pub(crate) fn reserve_shutdown(&self) -> Result<ShutdownReservation, String> {
        {
            let mut state = self.lifecycle.lock().unwrap();
            if state.checking {
                return Err(format!(
                    "existing shutdown attempt: {}",
                    state.active.as_deref().unwrap_or("exit verification")
                ));
            }
            state.checking = true;
            state.closing = true;
        }
        Ok(ShutdownReservation(self.lifecycle.clone()))
    }
    pub(crate) fn shutdown_attempt(
        &self,
        reply_timeout: Duration,
        exit_timeout: Duration,
    ) -> Result<Value, String> {
        let (released, active) = {
            let state = self.lifecycle.lock().unwrap();
            (state.released.clone(), state.active.clone())
        };
        if released.is_none() {
            let response = if let Some(id) = active {
                match self.broker.terminal(&id) {
                    Some(response) => response,
                    None => {
                        if let Some(exit) = self.poll_exit()? {
                            return Ok(self.abnormal_report(
                                "process exited before cleanup terminal evidence",
                                exit,
                            ));
                        }
                        return Err(format!(
                            "existing shutdown attempt {id}; terminal cleanup unconfirmed"
                        ));
                    }
                }
            } else {
                // Once the child has exited, no new cleanup may be inferred or sent.
                if let Some(exit) = self.poll_exit()? {
                    return Ok(
                        self.abnormal_report("process exited without verified cleanup", exit)
                    );
                }
                let session = self
                    .session
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or("worker session identity unavailable")?;
                let id = format!(
                    "host-shutdown-{}",
                    NEXT_RUNTIME_ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                );
                self.lifecycle.lock().unwrap().active = Some(id.clone());
                let mut context = json!({"session_id":session,"connection_id":null,"epoch":0});
                if self.protocol == 3 {
                    context["domain"] = Value::Null;
                }
                match self.exchange(&json!({"v":self.protocol,"id":id,"method":"shutdown","params":{},"context":context}),reply_timeout) {
                    Ok(response)=>response,
                    Err(error)=>{
                        if let Some(exit)=self.poll_exit()? {return Ok(self.abnormal_report(&error,exit));}
                        // A timeout keeps the accepted attempt; a before-send rejection does not.
                        if !error.contains("remains tracked") && self.broker.failure().is_none() {self.lifecycle.lock().unwrap().active=None;}
                        return Err(error);
                    }
                }
            };
            let report = response.get("result").cloned();
            let released = response["ok"] == true && report.as_ref().is_some_and(release_verified);
            if !released {
                if let Some(exit) = self.poll_exit()? {
                    return Ok(self.abnormal_report("cleanup did not confirm release", exit));
                }
                return Err("cleanup did not confirm release; retained original report; explicit retry may create a new attempt".into());
            }
        }
        let deadline = Instant::now() + exit_timeout;
        loop {
            if let Some(exit) = self.poll_exit()? {
                if exit["success"] == true {
                    return Ok(self.lifecycle.lock().unwrap().released.clone().unwrap());
                }
                return Ok(self.abnormal_report("process exited abnormally after cleanup", exit));
            }
            if Instant::now() >= deadline {
                return Err(
                    "Native worker replied but did not exit; release report retained, exit unconfirmed"
                        .into(),
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
    pub(crate) fn poll_exit(&self) -> Result<Option<Value>, String> {
        // The child lock spans one nonblocking try_wait, never the wait loop.
        let status = self
            .child
            .lock()
            .unwrap()
            .try_wait()
            .map_err(|e| format!("could not verify process exit: {e}"))?;
        let exit = status.map(|s| json!({"confirmed":true,"success":s.success(),"code":s.code()}));
        if let Some(exit) = &exit {
            self.lifecycle.lock().unwrap().exit = Some(exit.clone());
        }
        Ok(exit)
    }
    pub(crate) fn abnormal_report(&self, message: &str, exit: Value) -> Value {
        let state = self.lifecycle.lock().unwrap();
        let original = state
            .released
            .clone()
            .or_else(|| state.reports.last().and_then(|r| r.get("result").cloned()));
        json!({"steps":[{"role":"worker","action":"shutdown","ok":false,"error":message}],"unreleased":["unknown"],"voltage_zero":null,
            "resource_release_verified":false,"process_exit":exit,"received_cleanup_report":original})
    }
}
impl Drop for WorkerRuntime {
    fn drop(&mut self) {
        self.writer.close();
    }
}
fn release_verified(report: &Value) -> bool {
    report["unreleased"].as_array().is_some_and(Vec::is_empty)
        && report["steps"].is_array()
        && report.get("voltage_zero").is_some()
        && report
            .get("resource_release_verified")
            .map_or(true, |v| v == true)
}
fn normalize_native_cleanup(native: &Value) -> Result<Value, String> {
    let released = native["all_resources_released"]
        .as_bool()
        .ok_or("Missing native release conclusion")?;
    let reports = native["cleanup_reports"]
        .as_array()
        .filter(|r| !r.is_empty() && r.len() <= 66)
        .ok_or("Invalid native cleanup reports")?;
    let mut steps = Vec::new();
    let mut unreleased = Vec::new();
    let mut zero = Value::Null;
    for report in reports {
        if !fields(
            report,
            &["attempt_id", "steps", "voltage_zero", "unreleased"],
            &[],
        ) || !report["attempt_id"]
            .as_str()
            .is_some_and(crate::host::contracts::valid_id)
        {
            return Err("Invalid immutable native cleanup attempt".into());
        }
        let native_steps = report["steps"]
            .as_array()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .ok_or("Invalid native cleanup steps")?;
        let retained = report["unreleased"]
            .as_array()
            .filter(|s| s.len() <= 64)
            .ok_or("Invalid native cleanup responsibilities")?;
        for item in retained {
            if !item
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 256)
            {
                return Err("Invalid retained native role".into());
            }
            unreleased.push(item.clone());
        }
        for step in native_steps {
            if !fields(step, &["role", "action", "error"], &[])
                || ["role", "action"].iter().any(|k| {
                    !step[*k]
                        .as_str()
                        .is_some_and(|s| !s.is_empty() && s.len() <= 128)
                })
                || !(step["error"].is_null()
                    || step["error"].as_str().is_some_and(|s| s.len() <= 4096))
            {
                return Err("Invalid native cleanup step".into());
            }
            let mut observed = step.clone();
            observed["ok"] = json!(step["error"].is_null());
            steps.push(observed);
        }
        if !report["voltage_zero"].is_null() {
            if !zero.is_null() {
                return Err("Ambiguous native voltage evidence".into());
            }
            zero = report["voltage_zero"].clone();
        }
    }
    if released && !unreleased.is_empty() {
        return Err("Native release conclusion conflicts with retained resources".into());
    }
    if !released && unreleased.is_empty() {
        unreleased.push(json!("native_work"));
    }
    Ok(
        json!({"steps":steps,"unreleased":unreleased,"voltage_zero":zero,"resource_release_verified":released,"physical_zero_verified":false,"native_cleanup":native}),
    )
}

pub(crate) fn collect_stderr(mut source: impl Read, store: Arc<Mutex<VecDeque<String>>>) {
    let mut bytes = [0; 1024];
    let mut line = Vec::new();
    let publish = |line: &mut Vec<u8>| {
        let mut tail = store.lock().unwrap();
        if tail.len() == MAX_LOG_LINES {
            tail.pop_front();
        }
        tail.push_back(String::from_utf8_lossy(line).into_owned());
        line.clear();
    };
    loop {
        match source.read(&mut bytes) {
            Ok(0) | Err(_) => {
                if !line.is_empty() {
                    publish(&mut line);
                }
                break;
            }
            Ok(count) => {
                for byte in &bytes[..count] {
                    if *byte == b'\n' {
                        publish(&mut line);
                    } else if line.len() < 4096 {
                        line.push(*byte);
                    }
                }
            }
        }
    }
}

pub(crate) fn response_error(response: &Value) -> String {
    response["error"]["message"]
        .as_str()
        .unwrap_or("unknown worker error")
        .into()
}

#[cfg(test)]
mod v3_tests {
    use super::*;
    #[test]
    fn native_capture_round_trip_uses_owned_staging_with_driver_byte_fixture() {
        // Test-only staging replaces finite VISA bytes, not the runtime/driver.
        let fixture = std::env::temp_dir().join(format!(
            "yang-capture-wire-{}",
            crate::host::registry::new_id().unwrap()
        ));
        std::fs::create_dir_all(fixture.join("App/worker")).unwrap();
        let runtime = match WorkerRuntime::spawn(RuntimeConfig {
            launch: crate::native_worker::NativeWorkerLaunch::test_osa_fixture(&fixture).unwrap(),
            catalog_root: fixture.clone(),
            mode: "real".into(),
            protocol: 3,
            ownership_nonce: Some("b".repeat(32)),
            record_child: Some(Arc::new(|_| Ok(()))),
        }) {
            Ok(runtime) => runtime,
            Err(error) => {
                if let Some(runtime) = &error.retained_runtime {
                    runtime.stop().unwrap();
                }
                std::fs::remove_dir_all(&fixture).unwrap();
                panic!("Fixture worker startup failed: {}", error.message);
            }
        };
        let result = (|| -> Result<(), String> {
            let session = runtime
                .session
                .lock()
                .unwrap()
                .clone()
                .ok_or("Missing session")?;
            let global = json!({"session_id":session,"domain":null,"connection_id":null,"epoch":0});
            let exchange = |method: &str, params: Value, context: Value| -> Result<Value, String> {
                let frame = json!({"v":3,"id":crate::host::registry::new_id().map_err(|e| e.to_string())?,
                    "method":method,"params":params,"context":context});
                let reply = runtime.exchange(&frame, Duration::from_secs(4))?;
                if reply["ok"] != true {
                    return Err(response_error(&reply));
                }
                Ok(reply)
            };
            let configuration = json!({"domain":{"kind":"device","id":"c".repeat(32)},"config_rev":1,
                "driver_kind":"osa","model_id":"aq6370","profile_id":"gpib-visa",
                "params":{"resource":"GPIB0::1::INSTR"},"expected_identity":{"model":"AQ6370E","serial":"HOST-OSA-1"},"members":[]});
            let configured = exchange(
                "configure_domain",
                json!({"config":configuration.clone()}),
                global.clone(),
            )?;
            let proof = exchange(
                "probe",
                json!({"authorization":{"stage":"readonly","accepted":true,"supervised":false,"retain_session":false,"binding":{"mode":"real","domain":configuration["domain"],"config_rev":1,"model_id":"aq6370","profile_id":"gpib-visa","config_digest":"a".repeat(64)}}}),
                configured["result"]["context"].clone(),
            )?;
            exchange(
                "register_verified",
                json!({"domain":configuration["domain"],"proof_id":proof["result"]["proof"]["proof_id"],"config_rev":1,"config_digest":"a".repeat(64)}),
                global,
            )?;
            let connected = exchange(
                "connect",
                json!({"acknowledge_lifecycle":true}),
                configured["result"]["context"].clone(),
            )?;
            let capture = exchange(
                "action",
                json!({"name":"read_trace","args":{"trace":"A"}}),
                connected["context"].clone(),
            )?;
            if serde_json::to_vec(&capture)
                .map_err(|e| e.to_string())?
                .len()
                >= 65_536
            {
                return Err("Oversized terminal".into());
            }
            let descriptor = &capture["result"]["capture"];
            let bytes = runtime.read_capture_chunk(descriptor, 0, 32)?;
            let expected: Vec<u8> = [1550f64, -30., 1551., -31.]
                .into_iter()
                .flat_map(f64::to_le_bytes)
                .collect();
            if bytes != expected
                || descriptor["sha256"]
                    != crate::host::verification::sha256_bytes(&expected)
                        .map_err(|e| e.to_string())?
            {
                return Err("Native payload changed across staging".into());
            }
            runtime.ack_capture(descriptor)?;
            if !runtime
                .read_capture_chunk(descriptor, 0, 16)
                .is_err_and(|e| e.contains("Unknown capture"))
            {
                return Err("Acknowledged bytes remain deliverable".into());
            }
            Ok(())
        })();
        // Always obtain actual worker release before removing test-owned sources.
        let stopped = runtime.stop();
        if stopped
            .as_ref()
            .is_ok_and(|report| report.resource_released)
        {
            std::fs::remove_dir_all(&fixture).unwrap();
        }
        assert!(stopped.is_ok(), "Test worker release failed");
        assert!(result.is_ok(), "{}", result.unwrap_err());
    }
    fn native_descriptor() -> Value {
        let context = json!({"transfer_format":"ASCII","sample_count":2,"spacing":0,"level_unit":0,
            "x_unit":0,"trace_attribute":0,"active_trace":"TRA","center_m":1.55e-6,
            "span_m":2e-9,"resolution_m":2e-11,"sweep_mode":1});
        json!({"schema":1,"kind":"osa_trace","capture_id":"c".repeat(32),"point_count":2,"byte_count":32,
            "sha256":"d".repeat(64),"metadata":{"native_unit":"dBm","trace":"A","identity":"YOKOGAWA,AQ6370E,SN,FW",
            "read_started_at":"2026-10-06T10:00:00+00:00","read_finished_at":"2026-10-06T10:00:01+00:00",
            "elapsed_s":1.,"consistency":"unproven","context_before":context,"context_after":context}})
    }
    #[test]
    fn capture_descriptor_rejects_mislabeled_or_oversized_native_context() {
        let descriptor = native_descriptor();
        assert!(validate_capture_descriptor(&descriptor).is_ok());
        for (key, value) in [
            ("point_count", json!(200002)),
            ("byte_count", json!(31)),
            ("capture_id", json!("../outside")),
            ("sha256", json!("D".repeat(64))),
            ("path", json!("outside")),
        ] {
            let mut bad = descriptor.clone();
            bad[key] = value;
            assert!(validate_capture_descriptor(&bad).is_err(), "{bad}");
        }
        for (key, value) in [
            ("native_unit", json!("dB/nm")),
            ("consistency", json!("atomic")),
            ("identity", json!("x".repeat(9000))),
            ("trace", json!("B")),
        ] {
            let mut bad = descriptor.clone();
            bad["metadata"][key] = value;
            assert!(validate_capture_descriptor(&bad).is_err());
        }
        let mut frequency = descriptor.clone();
        frequency["metadata"]["context_before"]["x_unit"] = json!(1);
        frequency["metadata"]["context_after"]["x_unit"] = json!(1);
        assert!(validate_capture_descriptor(&frequency).is_err());
        let mut mismatch = descriptor.clone();
        mismatch["metadata"]["context_after"]["sweep_mode"] = json!(3);
        assert!(validate_capture_descriptor(&mismatch).is_err());
    }
    #[test]
    fn capture_metadata_requires_real_utc_calendar_timestamps() {
        let descriptor = native_descriptor();
        for value in [
            "garbageT+00:00",
            "0000-01-01T00:00:00+00:00",
            "2026-02-29T10:00:00+00:00",
            "2026-13-01T00:00:00+00:00",
            "2026-10-06T24:00:00+00:00",
            "2026-10-06T10:60:00+00:00",
            "2026-10-06T10:00:00.+00:00",
            "2026-10-06T10:00:00.1234567+00:00",
            "2026-10-06T10:00:00+01:00",
        ] {
            let mut bad = descriptor.clone();
            bad["metadata"]["read_started_at"] = json!(value);
            assert!(validate_capture_descriptor(&bad).is_err(), "{value}");
        }
        let mut leap = descriptor.clone();
        leap["metadata"]["read_started_at"] = json!("2024-02-29T23:59:59.123456+00:00");
        assert!(validate_capture_descriptor(&leap).is_ok());
    }
    #[test]
    fn capture_chunk_requires_exact_reference_range_hash_and_hex_count() {
        let descriptor = native_descriptor();
        let chunk = json!({"capture_id":"c".repeat(32),"offset":0,"byte_count":16,"sha256":"d".repeat(64),"data_hex":"00000000000000000000000000000000"});
        assert_eq!(
            validate_capture_chunk(&chunk, &descriptor, 0, 16).unwrap(),
            vec![0u8; 16]
        );
        for (key, value) in [
            ("offset", json!(1)),
            ("byte_count", json!(32)),
            ("sha256", json!("e".repeat(64))),
            ("capture_id", json!("e".repeat(32))),
            ("data_hex", json!("00")),
            ("data_hex", json!("zz".repeat(16))),
        ] {
            let mut bad = chunk.clone();
            bad[key] = value;
            assert!(validate_capture_chunk(&bad, &descriptor, 0, 16).is_err());
        }
        assert!(validate_capture_chunk(&chunk, &descriptor, 0, 16385).is_err());
        assert!(validate_capture_chunk(&chunk, &descriptor, 32, 16).is_err());
    }
    #[test]
    fn private_capture_requests_require_bounded_global_owner_shapes() {
        let frame = json!({"v":3,"id":"chunk","method":"read_capture_chunk","params":{
            "ownership_nonce":"b".repeat(32),"capture_id":"c".repeat(32),"offset":0,"length":16384},
            "context":{"session_id":"a".repeat(32),"domain":null,"connection_id":null,"epoch":0}});
        assert!(decode_v3(&serde_json::to_vec(&frame).unwrap()).is_ok());
        for (key, value) in [
            ("length", json!(16385)),
            ("length", json!(true)),
            ("offset", json!(-1)),
            ("capture_id", json!("../outside")),
            ("path", json!("outside")),
        ] {
            let mut bad = frame.clone();
            bad["params"][key] = value;
            assert!(
                decode_v3(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "{bad}"
            );
        }
        let mut domain = frame.clone();
        domain["context"]["domain"] = json!({"kind":"device","id":"d".repeat(32)});
        assert!(decode_v3(&serde_json::to_vec(&domain).unwrap()).is_err());
        let ack = json!({"v":3,"id":"ack","method":"ack_capture","params":{
            "ownership_nonce":"b".repeat(32),"capture_id":"c".repeat(32),"sha256":"d".repeat(64)},
            "context":frame["context"]});
        assert!(decode_v3(&serde_json::to_vec(&ack).unwrap()).is_ok());
    }
    #[test]
    fn domain_routes_are_bounded_and_settle_even_without_a_waiter() {
        let mut routes = DomainRoutes::default();
        for index in 0..64 {
            let request = json!({"id":format!("c{index}"),"method":"configure_domain","params":{"config":{
                "domain":{"kind":"device","id":format!("{index:032x}")},"driver_kind":"osa"}}});
            routes.prepare(&request).unwrap();
            routes.complete(&json!({"id":request["id"],"ok":true}));
        }
        let extra = json!({"id":"extra","method":"configure_domain","params":{"config":{
            "domain":{"kind":"device","id":"f".repeat(32)},"driver_kind":"osa"}}});
        assert!(routes.prepare(&extra).is_err());
        let retirement = json!({"id":"retire","method":"retire_domain","params":{
            "domain":{"kind":"device","id":format!("{:032x}",0)}}});
        routes.prepare(&retirement).unwrap();
        assert_eq!(routes.kinds.len(), 64);
        routes.complete(&json!({"id":"retire","ok":false}));
        assert_eq!(routes.kinds.len(), 64);
        routes.prepare(&retirement).unwrap();
        routes.complete(&json!({"id":"retire","ok":true}));
        assert_eq!(routes.kinds.len(), 63);
        routes.prepare(&extra).unwrap();
        routes.complete(&json!({"id":"extra","ok":false}));
        assert_eq!(routes.kinds.len(), 63);
        assert!(routes.pending.is_empty());
    }
    #[test]
    fn native_worker_records_ownership_before_activation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let recorded = Arc::new(AtomicBool::new(false));
        let marker = recorded.clone();
        let config = RuntimeConfig {
            launch: crate::native_worker::NativeWorkerLaunch::test_native().unwrap(),
            catalog_root: root,
            mode: "real".into(),
            protocol: 3,
            ownership_nonce: Some("b".repeat(32)),
            record_child: Some(Arc::new(move |identity| {
                assert!(identity.pid > 0);
                assert!(!identity.creation_time.is_empty());
                assert_eq!(identity.ownership_nonce, "b".repeat(32));
                marker.store(true, Ordering::Release);
                Ok(())
            })),
        };
        let runtime = WorkerRuntime::spawn(config).unwrap();
        assert!(recorded.load(Ordering::Acquire));
        assert_eq!(runtime.protocol(), 3);
        let session = runtime.session.lock().unwrap().clone().unwrap();
        let global = json!({"session_id":session,"domain":null,"connection_id":null,"epoch":0});
        let query = runtime
            .submit(WorkerRequest::V3(
                json!({"v":3,"id":"query","method":"status","params":{},"context":global}),
            ))
            .unwrap()
            .wait(Duration::from_secs(3))
            .unwrap();
        assert_eq!(query["result"]["activated"], true);
        assert_eq!(query["result"]["connected"], false);
        for index in 1..=2 {
            let configuration = json!({"domain":{"kind":"device","id":format!("{index:032x}")},
                "config_rev":1,"driver_kind":"osa","model_id":"aq6370","profile_id":"gpib-visa",
                "params":{"resource":format!("GPIB0::{index}::INSTR")},"expected_identity":{},"members":[]});
            let response = runtime
                .submit(WorkerRequest::V3(
                    json!({"v":3,"id":format!("configure-{index}"),
                "method":"configure_domain","params":{"config":configuration},"context":global}),
                ))
                .unwrap()
                .wait(Duration::from_secs(3))
                .unwrap();
            assert_eq!(response["ok"], true, "{response}");
        }
        let status = runtime
            .submit(WorkerRequest::V3(
                json!({"v":3,"id":"configured-status","method":"status",
            "params":{},"context":global}),
            ))
            .unwrap()
            .wait(Duration::from_secs(3))
            .unwrap();
        assert_eq!(
            status["result"]["host_transport"]["domains"]
                .as_object()
                .unwrap()
                .len(),
            2
        );
        assert!(status["result"]["host_transport"].get("roles").is_none());
        let report = runtime.stop().unwrap();
        assert!(report.resource_released);
        assert!(!report.claims_physical_zero());
        assert_eq!(query["result"]["worker_kind"], "rust");
    }
    #[test]
    fn native_worker_record_failure_prevents_activation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let result = WorkerRuntime::spawn(RuntimeConfig {
            launch: crate::native_worker::NativeWorkerLaunch::test_native().unwrap(),
            catalog_root: root,
            mode: "real".into(),
            protocol: 3,
            ownership_nonce: Some("b".repeat(32)),
            record_child: Some(Arc::new(|_| Err("Startup storage failed".into()))),
        });
        let error = match result {
            Ok(_) => panic!("Failed record must not activate"),
            Err(error) => error,
        };
        let runtime = error.retained_runtime.unwrap();
        assert!(!runtime.ready.load(Ordering::Acquire));
        let reply = runtime
            .exchange(
                &json!({"v":3,"id":"inspect","method":"status","params":{},"context":null}),
                Duration::from_secs(3),
            )
            .unwrap();
        assert_eq!(reply["result"]["activated"], false);
        runtime.stop().unwrap();
    }
    #[test]
    fn reply_read_is_bounded_before_json_allocation() {
        let mut bytes = std::io::Cursor::new(vec![b'x'; 17 * 1024 * 1024 + 2]);
        assert!(read_reply_frame(&mut bytes, 17 * 1024 * 1024)
            .unwrap_err()
            .contains("capacity"));
    }
    #[test]
    fn shared_vectors_have_the_same_strict_cross_language_result() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/v3-contracts.json")).unwrap();
        for case in cases.as_array().unwrap() {
            assert_eq!(
                decode_v3(case["line"].as_str().unwrap().as_bytes()).is_ok(),
                case["valid"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
}
