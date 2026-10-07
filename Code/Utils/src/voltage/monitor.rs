use super::{decode_telemetry, VoltageStatus, BOARD_SCALE_V};
use std::time::Duration;
/// Fixed-size payload: CRLF within ADC bytes is not an end-of-frame marker.
/// No CRC/measurement ID exists; these are host-observed frames, not identity.
#[derive(Default)]
pub struct TelemetryDecoder {
    buffer: Vec<(u8, u64)>,
    aligned: bool,
}
impl TelemetryDecoder {
    pub fn feed(&mut self, bytes: &[u8], at: Duration) -> Vec<VoltageStatus> {
        self.feed_scoped(bytes, at, 0)
            .into_iter()
            .map(|(status, _)| status)
            .collect()
    }
    pub(super) fn feed_scoped(
        &mut self,
        bytes: &[u8],
        at: Duration,
        generation: u64,
    ) -> Vec<(VoltageStatus, u64)> {
        let mut frames = vec![];
        // Inspect every possible initial byte offset only after all 34 offsets can
        // supply two complete candidates. Ambiguous marker patterns remain unknown.
        // ADC voltage plausibility uses the reviewed 28 V board scale, not the 14 V
        // commanded ceiling; external voltages up to the board range stay observable.
        for byte in bytes {
            self.buffer.push((*byte, generation));
            if !self.aligned && self.buffer.len() >= 102 {
                let offsets = (0..34)
                    .filter(|start| {
                        candidate(&self.buffer[*start..*start + 34], at).is_some()
                            && candidate(&self.buffer[*start + 34..*start + 68], at).is_some()
                    })
                    .collect::<Vec<_>>();
                if offsets.len() == 1 {
                    self.buffer.drain(..offsets[0]);
                    self.aligned = true;
                } else {
                    self.buffer.remove(0);
                }
            }
            while self.aligned && self.buffer.len() >= 34 {
                if let Some(status) = candidate(&self.buffer[..34], at) {
                    frames.push((status, self.buffer[0].1));
                    self.buffer.drain(..34);
                } else {
                    self.aligned = false;
                    break;
                }
            }
        }
        frames
    }
}
fn candidate(bytes: &[(u8, u64)], at: Duration) -> Option<VoltageStatus> {
    let frame: [u8; 34] = std::array::from_fn(|i| bytes[i].0);
    let status = decode_telemetry(&frame, at).ok()?;
    status
        .voltage_v
        .iter()
        .all(|v| *v <= BOARD_SCALE_V)
        .then_some(status)
}
