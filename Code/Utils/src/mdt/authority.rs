use super::{
    session::{Io, Mdt693b, Op, Shared},
    Axis, MdtStatus,
};
use crate::{transport::Deadline, DriverError, DriverResult};
use std::sync::atomic::Ordering;
pub(crate) const TOLERANCE: f64 = 1e-6;
/// A connection/generation-bound operator attestation, never saved motion authority.
#[derive(Debug)]
pub struct BaselineAttestation {
    pub(crate) connection: u64,
    pub(crate) generation: u64,
    pub(crate) evidence: String,
}
#[derive(Clone)]
pub(crate) struct Authority {
    pub commands: [f64; 3],
    pub actual: [f64; 3],
    pub master: f64,
    pub enabled: bool,
    pub generation: u64,
    pub evidence: String,
}
pub(crate) fn actual(s: &MdtStatus) -> [f64; 3] {
    [
        s.axes[&Axis::X].actual_v,
        s.axes[&Axis::Y].actual_v,
        s.axes[&Axis::Z].actual_v,
    ]
}
impl Mdt693b {
    pub fn baseline_attestation(&self, confirm: bool) -> DriverResult<BaselineAttestation> {
        super::settings::confirmation(confirm)?;
        if self.status().is_none() || self.shared.closing.load(Ordering::Acquire) {
            return Err(DriverError::Closed);
        }
        Ok(BaselineAttestation{connection:self.shared.connection.load(Ordering::Acquire),generation:self.shared.generation.load(Ordering::Acquire),evidence:"Operator attests no external/manual voltage contribution; software cannot verify this assumption".into()})
    }
    pub fn adopt_current_axis_baseline(
        &self,
        attestation: BaselineAttestation,
    ) -> DriverResult<MdtStatus> {
        if attestation.connection != self.shared.connection.load(Ordering::Acquire)
            || attestation.generation != self.shared.generation.load(Ordering::Acquire)
        {
            return Err(DriverError::Invalid(
                "stale MDT baseline attestation".into(),
            ));
        }
        self.request(Op::Adopt(attestation))
    }
}
impl Io {
    pub(super) fn invalidate(&mut self, evidence: &str) {
        self.authority = None;
        if let Some(st) = self.status.as_mut() {
            st.axis_command_known = false;
            st.baseline_evidence = None;
            st.fault_evidence = Some(evidence.into());
        }
    }
    pub(super) fn observe(&mut self, s: &Shared, mut fresh: MdtStatus, recover: bool) -> MdtStatus {
        if recover {
            self.authority = None;
        }
        if let Some(a) = &self.authority {
            if a.generation != s.generation.load(Ordering::Acquire)
                || s.stop.load(Ordering::Acquire)
                || actual(&fresh)
                    .iter()
                    .zip(a.actual)
                    .any(|(x, y)| (*x - y).abs() > TOLERANCE)
                || fresh.master_scan_enabled != a.enabled
                || (fresh.master_scan_voltage_v - a.master).abs() > TOLERANCE
                || self.status.as_ref().is_some_and(|old| {
                    old.serial_number != fresh.serial_number
                        || old.product != fresh.product
                        || old.firmware != fresh.firmware
                })
            {
                self.invalidate("MDT external/manual change or authority loss; stop and hold");
                fresh.fault_evidence =
                    Some("MDT external/manual change or authority loss; stop and hold".into());
            }
        }
        if !recover {
            if let Some(old) = &self.status {
                if old.restricted {
                    fresh.restricted = true;
                    if fresh.fault_evidence.is_none() {
                        fresh.fault_evidence = old.fault_evidence.clone();
                    }
                }
                if fresh.selected_channel.is_none() {
                    fresh.selected_channel = old.selected_channel;
                }
            }
        }
        fresh.axis_command_known = self.authority.is_some();
        fresh.baseline_evidence = self.authority.as_ref().map(|a| a.evidence.clone());
        fresh
    }
    pub(super) fn adopt(
        &mut self,
        s: &Shared,
        g: u64,
        end: Deadline,
        a: BaselineAttestation,
    ) -> DriverResult<MdtStatus> {
        if a.connection != s.connection.load(Ordering::Acquire) || a.generation != g {
            return Err(DriverError::Invalid("stale MDT attestation".into()));
        }
        let st = self.snapshot(s, g, end)?;
        if st.master_scan_enabled
            || st.master_scan_voltage_v.abs() > TOLERANCE
            || st.restricted
            || actual(&st)
                .iter()
                .enumerate()
                .any(|(i, v)| *v < 0. || *v > s.config.application_limits_v[i])
        {
            return Err(DriverError::Invalid("baseline adoption requires Master Scan disabled/zero and all actual axes in safe intervals".into()));
        }
        let values = actual(&st);
        self.authority = Some(Authority {
            commands: values,
            actual: values,
            master: 0.,
            enabled: false,
            generation: g,
            evidence: a.evidence,
        });
        let mut st = st;
        st.axis_command_known = true;
        st.baseline_evidence = self.authority.as_ref().map(|a| a.evidence.clone());
        super::session::publish(s, self, st.clone());
        Ok(st)
    }
}
