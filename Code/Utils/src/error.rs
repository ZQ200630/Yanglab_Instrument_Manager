use std::{
    fmt,
    time::{Duration, Instant},
};
pub type DriverResult<T> = Result<T, DriverError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverError {
    Invalid(String),
    Busy(String),
    Canceled,
    DependencyUnavailable(String),
    Closed,
    Timeout {
        operation: String,
        transferred: usize,
    },
    Native {
        operation: String,
        status: i32,
        transferred: usize,
    },
    Protocol(String),
    Responsibility(String),
}
impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DriverError {}

#[derive(Clone, Copy, Debug)]
pub struct Deadline(Instant);
impl Deadline {
    pub fn earlier(self, other: Self) -> Self {
        Self(self.0.min(other.0))
    }
    pub fn after(duration: Duration) -> Self {
        Self(
            Instant::now()
                .checked_add(duration)
                .unwrap_or_else(Instant::now),
        )
    }
    pub fn remaining_millis(self) -> DriverResult<u32> {
        let remaining = self
            .0
            .checked_duration_since(Instant::now())
            .ok_or_else(|| DriverError::Timeout {
                operation: "deadline".into(),
                transferred: 0,
            })?;
        Ok(remaining
            .as_millis()
            .saturating_add(1)
            .min((u32::MAX - 1) as u128) as u32)
    }
}
