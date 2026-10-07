use super::codec;
use crate::DriverResult;
pub(super) fn validate(values: [f64; 3]) -> DriverResult<()> {
    for value in values {
        codec::format_fixed(value, 0., 999.999)?;
    }
    Ok(())
}
