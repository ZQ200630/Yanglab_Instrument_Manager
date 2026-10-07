use super::{Osa, TraceCapture, TraceId};
use crate::{transport::Deadline, DriverError, DriverResult};
impl Osa {
    pub fn acquire_trace(
        &mut self,
        trace: TraceId,
        deadline: Deadline,
    ) -> DriverResult<TraceCapture> {
        self.begin_operation(deadline)?;
        let result = (|| {
            self.active_trace(trace, deadline)?;
            let mode = self.sweep_mode(deadline)?;
            if !matches!(mode, 1 | 3) {
                return Err(DriverError::Protocol(
                    "New OSA sweep requires panel SINGLE or AUTO; REPEAT uses read_trace".into(),
                ));
            }
            self.stop.check()?;
            deadline.remaining_millis()?;
            // Establish ownership before even a partial INIT write; uncertainty
            // must not be mistaken for a panel-owned sweep or replayed INIT.
            self.sweep_owned = true;
            self.session
                .as_mut()
                .ok_or(DriverError::Closed)?
                .write_all(b":INITiate:IMMediate\n", deadline)?;
            self.stop.check()?;
            if self.query_text("*OPC?", deadline, 32)? != "1" {
                return Err(DriverError::Protocol(
                    "OSA did not acknowledge sweep completion".into(),
                ));
            }
            if self.sweep_mode(deadline)? != mode {
                return Err(DriverError::Protocol(
                    "OSA mode changed after INIT; sweep ownership retained for explicit cleanup"
                        .into(),
                ));
            }
            self.stop.check()?;
            self.sweep_owned = false;
            self.read_body(trace, deadline)
        })();
        self.finish_operation(result)
    }
}
