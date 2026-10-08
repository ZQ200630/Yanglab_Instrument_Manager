use serde::{Deserialize, Serialize};

// Native-only Host implementation contract; independent of live Worker startup.
pub(crate) const NATIVE_WORKER_KIND: &str = "rust";
pub(crate) const NATIVE_WORKER_STARTUP_REVISION: u64 = 1;
pub(crate) fn native_host_compatible(reply: &serde_json::Value) -> bool {
    reply["worker_kind"] == NATIVE_WORKER_KIND
        && reply["worker_startup_revision"] == NATIVE_WORKER_STARTUP_REVISION
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostError {
    pub code: String,
    pub message: String,
}

impl HostError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HostError {}

pub const MAX_SEQUENCE: u64 = 9_007_199_254_740_991;

pub fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckBinding {
    pub mode: String,
    pub identity: serde_json::Value,
    pub config_rev: u64,
    pub profile_id: String,
    pub probe_version: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckPolicy {
    pub interval_s: u32,
    pub enumeration: Option<CheckBinding>,
    pub readonly: Option<CheckBinding>,
}
impl Default for CheckPolicy {
    fn default() -> Self {
        Self {
            interval_s: 30,
            enumeration: None,
            readonly: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostSettings {
    pub host_name: String,
    #[serde(default)]
    pub data_root: Option<String>,
}
impl Default for HostSettings {
    fn default() -> Self {
        Self {
            host_name: "Local computer".into(),
            data_root: None,
        }
    }
}
impl<'de> Deserialize<'de> for HostSettings {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            host_name: String,
            #[serde(default)]
            data_root: Option<String>,
            #[serde(default, rename = "python_path")]
            _obsolete_interpreter: Option<serde::de::IgnoredAny>,
        }
        let input = Input::deserialize(decoder)?;
        Ok(Self {
            host_name: input.host_name,
            data_root: input.data_root,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRecord {
    pub device_id: String,
    pub name: String,
    pub category: String,
    pub model_id: String,
    pub driver_id: String,
    pub profile_id: String,
    pub params: serde_json::Value,
    pub expected_identity: serde_json::Value,
    pub identity_strength: String,
    #[serde(default = "unverified_mode")]
    pub verified_mode: String,
    pub config_rev: u64,
    pub check_policy: CheckPolicy,
}
fn unverified_mode() -> String {
    "unverified".into()
}
pub fn verification_mode_matches(verified: &str, running: &str) -> Result<(), HostError> {
    if verified == "real" && running == "real" {
        Ok(())
    } else {
        Err(HostError::new("VerificationModeMismatch", "Reverify this device against real hardware; legacy or unstamped evidence grants no authority"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupRecord {
    pub setup_id: String,
    pub name: String,
    pub kind: String,
    pub members: Vec<String>,
    pub config_rev: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftRecord {
    pub device_id: String,
    pub name: String,
    pub model_id: Option<String>,
    pub profile_id: Option<String>,
    pub params: serde_json::Value,
    pub config_digest: String,
    pub revision: u64,
    pub mode: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tombstone {
    pub record: DeviceRecord,
    pub retired_rev: u64,
    pub release_attempt_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupTombstone {
    pub record: SetupRecord,
    pub retired_rev: u64,
    pub release_attempt_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrySnapshot {
    pub version: u64,
    pub host_id: String,
    pub registry_rev: u64,
    pub settings: HostSettings,
    pub devices: Vec<DeviceRecord>,
    pub setups: Vec<SetupRecord>,
    pub drafts: Vec<DraftRecord>,
    pub tombstones: Vec<Tombstone>,
    #[serde(default)]
    pub setup_tombstones: Vec<SetupTombstone>,
}

#[cfg(test)]
mod mode_tests {
    use super::*;
    #[test]
    fn real_only_saved_verification_never_grants_legacy_authority() {
        assert!(verification_mode_matches("simulate", "simulate").is_err());
        assert_eq!(
            verification_mode_matches("simulate", "real")
                .unwrap_err()
                .code,
            "VerificationModeMismatch"
        );
        assert!(verification_mode_matches("unverified", "real").is_err());
        assert!(verification_mode_matches("real", "simulate").is_err());
        assert!(verification_mode_matches("real", "real").is_ok());
    }
}
