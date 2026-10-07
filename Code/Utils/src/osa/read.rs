use super::{
    decode_trace_reply, Osa, ReadTiming, TraceCapture, TraceContext, TraceContextParams, TraceId,
    MAX_TRACE_REPLY_BYTES, TRACE_CHUNK_POINTS,
};
use crate::{transport::Deadline, DriverError, DriverResult};
use std::time::{Duration, Instant, SystemTime};
impl Osa {
    pub(crate) fn active_trace(&mut self, trace: TraceId, deadline: Deadline) -> DriverResult<()> {
        if self
            .query_text(":TRACe:ACTive?", deadline, 32)?
            .to_ascii_uppercase()
            != trace.instrument_name()
        {
            return Err(DriverError::Protocol("Select the requested trace on the front panel first; software will not switch active trace".into()));
        }
        Ok(())
    }
    pub(crate) fn integer(&mut self, command: &str, deadline: Deadline) -> DriverResult<usize> {
        let text = self.query_text(command, deadline, 256)?;
        let digits = text.strip_prefix('+').unwrap_or(&text);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(DriverError::Protocol(
                "OSA metadata is not a nonnegative integer".into(),
            ));
        }
        digits
            .parse()
            .map_err(|_| DriverError::Protocol("OSA integer metadata exceeds bounds".into()))
    }
    fn number(&mut self, command: &str, deadline: Deadline) -> DriverResult<f64> {
        super::decode::ascii_number(self.query_text(command, deadline, 256)?.as_bytes())
    }
    pub(crate) fn sweep_mode(&mut self, deadline: Deadline) -> DriverResult<u8> {
        let mode = self.integer(":INITiate:SMODe?", deadline)?;
        if !(1..=3).contains(&mode) {
            return Err(DriverError::Protocol(
                "OSA sweep mode is unsupported".into(),
            ));
        }
        Ok(mode as u8)
    }
    pub(crate) fn read_context(
        &mut self,
        trace: TraceId,
        deadline: Deadline,
    ) -> DriverResult<TraceContext> {
        self.active_trace(trace, deadline)?;
        if self.integer(":UNIT:X?", deadline)? != 0 {
            return Err(DriverError::Protocol(
                "OSA frequency interpretation is not validated".into(),
            ));
        }
        let format = self.query_text(":FORMat:DATA?", deadline, 256)?.parse()?;
        let sample_count = self.integer(
            &format!(":TRACe:DATA:SNUMber? {}", trace.instrument_name()),
            deadline,
        )?;
        let byte = |value: usize| {
            u8::try_from(value)
                .map_err(|_| DriverError::Protocol("OSA metadata byte out of range".into()))
        };
        let spacing = byte(self.integer(":DISPlay:TRACe:Y1:SCALe:SPACing?", deadline)?)?;
        let level_unit = byte(self.integer(":DISPlay:TRACe:Y1:SCALe:UNIT?", deadline)?)?;
        // A trace-name argument can switch active trace on this instrument.
        let trace_attribute = byte(self.integer(":TRACe:ATTRibute?", deadline)?)?;
        let center_m = self.number(":SENSe:WAVelength:CENTer?", deadline)?;
        let span_m = self.number(":SENSe:WAVelength:SPAN?", deadline)?;
        let resolution_m = self.number(":SENSe:BWIDth:RESolution?", deadline)?;
        let sweep_mode = self.sweep_mode(deadline)?;
        TraceContext::new(TraceContextParams {
            transfer_format: format,
            sample_count,
            spacing,
            level_unit,
            x_unit: 0,
            trace_attribute,
            active_trace: trace,
            center_m,
            span_m,
            resolution_m,
            sweep_mode,
        })
    }
    pub(crate) fn read_body(
        &mut self,
        trace: TraceId,
        deadline: Deadline,
    ) -> DriverResult<TraceCapture> {
        let started = Instant::now();
        let started_utc = SystemTime::now();
        self.io_elapsed = Duration::ZERO;
        let before = self.read_context(trace, deadline)?;
        let unit = before.native_unit()?;
        let points = before.params().sample_count;
        let format = before.params().transfer_format;
        let mut x = Vec::with_capacity(points);
        let mut y = Vec::with_capacity(points);
        let mut decode = Duration::ZERO;
        for first in (1..=points).step_by(TRACE_CHUNK_POINTS) {
            let last = (first + TRACE_CHUNK_POINTS - 1).min(points);
            for (axis, destination) in [("X", &mut x), ("Y", &mut y)] {
                let reply = self.query(
                    &format!(":TRACe:{axis}? {},{first},{last}", trace.instrument_name()),
                    deadline,
                    MAX_TRACE_REPLY_BYTES,
                )?;
                let decode_started = Instant::now();
                let values = decode_trace_reply(&reply, format, last - first + 1)?;
                destination.extend(values);
                decode += decode_started.elapsed();
                self.stop.check()?;
                deadline.remaining_millis()?;
            }
        }
        let after = self.read_context(trace, deadline)?;
        self.stop.check()?;
        deadline.remaining_millis()?;
        let conversion = Instant::now();
        for value in &mut x {
            *value *= 1e9;
        }
        decode += conversion.elapsed();
        TraceCapture::new(
            x,
            y,
            unit,
            trace,
            self.identity.clone(),
            ReadTiming {
                started_utc,
                finished_utc: SystemTime::now(),
                elapsed: started.elapsed(),
                decode,
                io: self.io_elapsed,
            },
            before,
            after,
        )
    }
    pub fn read_trace(&mut self, trace: TraceId, deadline: Deadline) -> DriverResult<TraceCapture> {
        self.begin_operation(deadline)?;
        let result = self.read_body(trace, deadline);
        self.finish_operation(result)
    }
}
