pub mod actions;
pub mod backend;
mod capture_files;
pub mod captures;
pub mod catalog;
pub mod discovery;
pub mod dispatch;
pub mod domains;
mod native_sessions;
pub mod newport;
mod laser_session;
pub mod observations;
pub mod pm_ops;
pub mod safety;
pub mod scheduler;
pub mod session;
pub mod verification;
pub use domains::{DomainRegistry, DomainSnapshot};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerError {
    pub code: String,
    pub message: String,
}
impl WorkerError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for WorkerError {}
pub fn new_id() -> Result<String, WorkerError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| WorkerError::new("Identity", "secure random unavailable"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
impl From<yang_protocol::ProtocolError> for WorkerError {
    fn from(error: yang_protocol::ProtocolError) -> Self {
        Self::new("ProtocolError", error.to_string())
    }
}
impl From<yang_drivers::DriverError> for WorkerError {
    fn from(error: yang_drivers::DriverError) -> Self {
        Self::new("DriverError", error.to_string())
    }
}
