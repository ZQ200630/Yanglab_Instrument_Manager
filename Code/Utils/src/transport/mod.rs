pub mod owner;
pub mod reservations;
pub mod serial;
pub mod serial_abi;
pub mod serial_discovery;
pub mod visa;
pub mod visa_abi;
pub mod visa_trace;
pub use crate::error::Deadline;
pub use reservations::{CanonicalResource, ResourceBook};
pub use serial::{SerialConfig, SerialSession};
pub use serial_discovery::{enumerate_serial, SerialDeviceInfo};
pub use visa::{CloseReport, VisaManager, VisaSession};

/// One bounded read and its authoritative transport message boundary. A short
/// read, an empty read, or a timeout must never be inferred to mean completion.
pub struct ReadFragment {
    pub bytes: Vec<u8>,
    pub end_of_message: bool,
}

pub trait ByteTransport: Send {
    fn write_all(&mut self, bytes: &[u8], deadline: Deadline) -> crate::DriverResult<()>;
    fn read_bounded(&mut self, maximum: usize, deadline: Deadline) -> crate::DriverResult<Vec<u8>>;
    fn read_fragment(
        &mut self,
        maximum: usize,
        deadline: Deadline,
    ) -> crate::DriverResult<ReadFragment> {
        self.read_bounded(maximum, deadline)
            .map(|bytes| ReadFragment {
                bytes,
                end_of_message: false,
            })
    }
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
    fn read_fragment(
        &mut self,
        maximum: usize,
        deadline: Deadline,
    ) -> crate::DriverResult<ReadFragment> {
        VisaSession::read_fragment(self, maximum, deadline)
    }
    fn close(&mut self) -> crate::DriverResult<CloseReport> {
        VisaSession::close(self)
    }
    fn has_responsibility(&self) -> bool {
        VisaSession::has_responsibility(self)
    }
}
