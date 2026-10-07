use crate::{valid_id, ProtocolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeIdentity {
    pub startup_revision: u32,
    pub worker_kind: String,
    pub executable: String,
    pub package_revision: String,
    pub protocol_version: u32,
    pub mode: String,
    pub session_id: String,
    pub activated: bool,
    pub connected: bool,
    pub domains: Vec<Value>,
}
impl NativeIdentity {
    pub fn validate_startup(&self) -> Result<(), ProtocolError> {
        if self.startup_revision != 1
            || self.worker_kind != "rust"
            || self.protocol_version != 3
            || self.mode != "real"
            || !valid_id(&self.session_id)
            || self.activated
            || self.connected
            || !self.domains.is_empty()
            || self.executable.trim().is_empty()
            || self.executable.contains('\0')
            || self.executable.len() > 32768
            || self.package_revision.trim().is_empty()
            || self.package_revision.len() > 128
        {
            return Err(ProtocolError(
                "Invalid or armed native startup identity".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    pub schema: u32,
    pub source_revision: String,
    pub package_revision: String,
    pub protocol_version: u32,
    pub startup_revision: u32,
    pub worker_sha256: String,
}
