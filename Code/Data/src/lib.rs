//! Transport-independent waveform evidence, bounded views, analysis and storage.
mod analysis;
mod csv;
mod export;
mod legacy;
mod npy;
mod npz;
mod reduce;
mod store;
mod waveform;
pub use analysis::*;
pub use export::*;
pub use legacy::*;
pub use reduce::*;
pub use store::*;
pub use waveform::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataError {
    Invalid(String),
    Limit(String),
    Unsupported(String),
    NotFound(String),
    Conflict(String),
    Io(String),
    Cancelled,
}
impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "cancelled at a confirmed chunk boundary"),
            Self::Invalid(s)
            | Self::Limit(s)
            | Self::Unsupported(s)
            | Self::NotFound(s)
            | Self::Conflict(s)
            | Self::Io(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for DataError {}
impl From<std::io::Error> for DataError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
impl From<serde_json::Error> for DataError {
    fn from(e: serde_json::Error) -> Self {
        Self::Invalid(e.to_string())
    }
}
impl From<zip::result::ZipError> for DataError {
    fn from(e: zip::result::ZipError) -> Self {
        Self::Invalid(e.to_string())
    }
}
pub fn new_id() -> Result<String, DataError> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| DataError::Io("system random unavailable".into()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
