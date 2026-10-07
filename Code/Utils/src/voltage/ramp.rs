//! Bounded DAC-code ramp planning.
use super::codec::BOARD_SCALE_V;
use crate::{DriverError, DriverResult};
pub(super) fn plan(start: [u16; 8], target: [u16; 8], step: f64) -> DriverResult<Vec<[u16; 8]>> {
    if !step.is_finite() || step <= 0. || step > 0.1 {
        return Err(DriverError::Invalid(
            "ramp step must be positive and <=0.1 V".into(),
        ));
    }
    let maximum = (step * 65535. / BOARD_SCALE_V).floor() as u32;
    if maximum == 0 {
        return Err(DriverError::Invalid(
            "ramp step is below one DAC code".into(),
        ));
    }
    let delta = (0..8)
        .map(|i| start[i].abs_diff(target[i]) as u32)
        .max()
        .unwrap();
    let count = delta.div_ceil(maximum).max(1);
    Ok((1..=count)
        .map(|n| {
            std::array::from_fn(|i| {
                let a = start[i] as i64;
                let d = target[i] as i64 - a;
                (a + d * n as i64 / count as i64) as u16
            })
        })
        .collect())
}
