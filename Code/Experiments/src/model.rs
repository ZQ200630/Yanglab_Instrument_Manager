use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::AtomicBool;
use yang_data::{WaveformFrame, WaveformRecord};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBinding {
    pub role: String,
    pub host_id: String,
    pub domain_id: String,
    pub channel: Option<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentDefinition {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub template_id: String,
    pub template_version: u32,
    pub roles: Vec<RoleBinding>,
    pub parameters: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoleInfo {
    pub family: String,
    pub identity: Value,
    pub config_rev: u64,
    pub connected: bool,
    pub capabilities: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoleSnapshot {
    pub binding: RoleBinding,
    pub info: RoleInfo,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Uncertain,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepRecord {
    pub stage: String,
    pub role: Option<String>,
    pub started_at_unix_ms: u64,
    pub completed_at_unix_ms: u64,
    pub outcome: String,
    pub detail: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub schema: u32,
    pub id: String,
    pub experiment_id: String,
    pub definition: ExperimentDefinition,
    pub bindings: Vec<RoleSnapshot>,
    pub status: RunStatus,
    pub stage: String,
    pub completed_units: u64,
    pub total_units: u64,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: Option<u64>,
    pub records: Vec<String>,
    pub analyses: Vec<String>,
    pub steps: Vec<StepRecord>,
    pub error: Option<ExperimentError>,
    pub cleanup_errors: Vec<ExperimentError>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExperimentError {
    pub code: String,
    pub message: String,
}
impl ExperimentError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn uncertain(&self) -> bool {
        matches!(
            self.code.as_str(),
            "Unknown" | "OutcomeUnknown" | "timed_out_unknown" | "failed_after_call_started"
        )
    }
}
impl std::fmt::Display for ExperimentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ExperimentError {}
impl From<yang_data::DataError> for ExperimentError {
    fn from(error: yang_data::DataError) -> Self {
        Self::new(
            if error == yang_data::DataError::Cancelled {
                "Cancelled"
            } else {
                "DataError"
            },
            error.to_string(),
        )
    }
}

/// The Host adapter supplies ownership, typed family admission and durable operations.
/// Implementations must enforce deadlines; this interface is not raw SCPI access.
pub trait ExperimentPort {
    fn inspect(&mut self, binding: &RoleBinding) -> Result<RoleInfo, ExperimentError>;
    fn reserve(&mut self, binding: &RoleBinding) -> Result<(), ExperimentError>;
    fn release(&mut self, binding: &RoleBinding) -> Result<(), ExperimentError>;
    fn action(
        &mut self,
        binding: &RoleBinding,
        name: &str,
        args: &Value,
    ) -> Result<Value, ExperimentError>;
    fn wait_ready(
        &mut self,
        binding: &RoleBinding,
        cancelled: &AtomicBool,
    ) -> Result<(), ExperimentError>;
    fn capture(
        &mut self,
        binding: &RoleBinding,
        params: &Value,
        cancelled: &AtomicBool,
    ) -> Result<WaveformFrame, ExperimentError>;
    /// Establish the grouped record before the first capture; no transport effect.
    fn begin_record(&mut self, _record: &WaveformRecord) -> Result<(), ExperimentError> {
        Ok(())
    }
    /// Durably retain the completed frame before acknowledging device-side staging.
    fn frame_completed(&mut self, _record: &WaveformRecord) -> Result<(), ExperimentError> {
        Ok(())
    }
    fn persist(&mut self, record: &WaveformRecord) -> Result<String, ExperimentError>;
    /// Transfer final capture ownership without copying deep-memory sample arrays.
    fn persist_owned(&mut self, record: WaveformRecord) -> Result<String, ExperimentError> {
        self.persist(&record)
    }
    fn analyze(
        &mut self,
        record: &WaveformRecord,
        params: &Value,
    ) -> Result<Vec<String>, ExperimentError>;
    fn wait_interval(&mut self, millis: u64, cancelled: &AtomicBool)
        -> Result<(), ExperimentError>;
}
