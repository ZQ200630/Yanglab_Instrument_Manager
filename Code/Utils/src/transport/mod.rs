pub mod reservations;
pub mod serial;
pub mod serial_abi;
pub mod serial_discovery;
pub mod visa;
pub mod visa_abi;
pub use crate::error::Deadline;
pub use reservations::{CanonicalResource, ResourceBook};
pub use serial::{SerialConfig, SerialSession};
pub use serial_discovery::{enumerate_serial, SerialDeviceInfo};
pub use visa::{CloseReport, VisaManager, VisaSession};

pub trait ByteTransport: Send {
    fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> crate::DriverResult<()>;
    fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> crate::DriverResult<Vec<u8>>;
    fn close(&mut self) -> crate::DriverResult<CloseReport>;
    fn has_responsibility(&self) -> bool;
}
impl ByteTransport for VisaSession {
    fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> crate::DriverResult<()> {
        VisaSession::write_all(self, bytes, deadline)
    }
    fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> crate::DriverResult<Vec<u8>> {
        self.read(maximum, deadline)
    }
    fn close(&mut self) -> crate::DriverResult<CloseReport> {
        VisaSession::close(self)
    }
    fn has_responsibility(&self) -> bool {
        VisaSession::has_responsibility(self)
    }
}
