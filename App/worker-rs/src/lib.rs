pub mod domains;
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
