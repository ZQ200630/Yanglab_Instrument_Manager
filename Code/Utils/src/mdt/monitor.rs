use super::{
    protocol,
    session::{Io, Shared},
    Axis, MdtStatus,
};
use crate::DriverResult;
pub(crate) fn poll(s: &Shared, io: &mut Io, g: u64) -> DriverResult<MdtStatus> {
    let mut status = io.status.clone().expect("MDT connected snapshot");
    let end = crate::transport::Deadline::after(s.config.io_timeout.saturating_mul(3));
    for a in [Axis::X, Axis::Y, Axis::Z] {
        let value = protocol::number(&io.query(s, &format!("{}voltage?", a.token()), g, end)?)?;
        status.axes.get_mut(&a).unwrap().actual_v = value;
    }
    if status
        .axes
        .iter()
        .any(|(a, v)| v.actual_v > s.config.application_limits_v[a.index()])
    {
        status.restricted = true;
        status.fault_evidence =
            Some("MDT monitor observed external voltage over ceiling; hold".into());
    }
    status.observed_at = s.clock.now().as_secs_f64();
    status.axis_command_known = false;
    Ok(status)
}
