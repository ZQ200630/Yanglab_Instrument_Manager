use super::{csv, integer, Pm400};
use crate::{transport::Deadline, DriverError, DriverResult};
use serde::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum StatusGroup {
    Measurement,
    Auxiliary,
    Operation,
    Questionable,
}
impl StatusGroup {
    pub(crate) fn suffix(self) -> &'static str {
        match self {
            Self::Measurement => "MEASurement",
            Self::Auxiliary => "AUXiliary",
            Self::Operation => "OPERation",
            Self::Questionable => "QUEStionable",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SystemError {
    pub code: i32,
    pub message: String,
    pub raw: String,
}
impl SystemError {
    pub fn parse(text: &str) -> DriverResult<Self> {
        let f = csv(text, 2)?;
        let code = f[0]
            .trim()
            .parse()
            .map_err(|_| DriverError::Protocol("invalid system error code".into()))?;
        Ok(Self {
            code,
            message: f[1].clone(),
            raw: text.into(),
        })
    }
}
pub struct Status<'a>(pub(crate) &'a mut Pm400);
impl Status<'_> {
    /// Event register query consumes/clears the instrument's latched events.
    pub fn read_event(&mut self, group: StatusGroup) -> DriverResult<u16> {
        self.read(group, "EVENt")
    }
    pub fn read_condition(&mut self, group: StatusGroup) -> DriverResult<u16> {
        self.read(group, "CONDition")
    }
    fn read(&mut self, group: StatusGroup, suffix: &str) -> DriverResult<u16> {
        let text = self.0.query(
            &format!("STATus:{}:{suffix}?", group.suffix()),
            self.0.deadline(),
        )?;
        Ok(integer(&text, 0, 65535)? as u16)
    }
}
impl Pm400 {
    pub fn status(&mut self) -> Status<'_> {
        Status(self)
    }
    /// Consumes/clears IEEE-488.2 standard-event register bits.
    pub fn read_standard_event_status(&mut self) -> DriverResult<u8> {
        let text = self.query("*ESR?", self.deadline())?;
        Ok(integer(&text, 0, 255)? as u8)
    }
    pub fn read_status_byte(&mut self) -> DriverResult<u8> {
        let text = self.query("*STB?", self.deadline())?;
        Ok(integer(&text, 0, 255)? as u8)
    }
    pub fn wait_operation_complete(&mut self, deadline: Deadline) -> DriverResult<()> {
        if self.query("*OPC?", deadline)? != "1" {
            return Err(DriverError::Protocol(
                "operation-complete response must be 1".into(),
            ));
        }
        Ok(())
    }
}
