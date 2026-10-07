use super::{NativeUnit, TraceContext, TraceId};
use crate::{DriverError, DriverResult};
use std::time::{Duration, SystemTime};
#[derive(Clone, Debug)]
pub struct ReadTiming {
    pub started_utc: SystemTime,
    pub finished_utc: SystemTime,
    pub elapsed: Duration,
    pub decode: Duration,
}
#[derive(Clone, Debug)]
pub struct TraceCapture {
    wavelength_nm: Vec<f64>,
    native_values: Vec<f64>,
    native_unit: NativeUnit,
    trace: TraceId,
    identity: String,
    timing: ReadTiming,
    context_before: TraceContext,
    context_after: TraceContext,
}
#[derive(Clone, Debug)]
pub struct Spectrum {
    wavelength_nm: Vec<f64>,
    power_dbm: Vec<f64>,
    trace: TraceId,
    acquired_at: SystemTime,
    identity: String,
}
fn samples(x: &[f64], y: &[f64]) -> DriverResult<()> {
    if x.is_empty()
        || x.len() > super::MAX_TRACE_POINTS
        || x.len() != y.len()
        || x.iter().any(|v| !v.is_finite() || *v <= 0.)
        || y.iter().any(|v| !v.is_finite())
        || x.windows(2).any(|pair| pair[1] <= pair[0])
    {
        return Err(DriverError::Protocol("OSA samples require equal bounded finite arrays and a positive increasing wavelength axis".into()));
    }
    Ok(())
}
impl TraceCapture {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        wavelength_nm: Vec<f64>,
        native_values: Vec<f64>,
        native_unit: NativeUnit,
        trace: TraceId,
        identity: String,
        timing: ReadTiming,
        context_before: TraceContext,
        context_after: TraceContext,
    ) -> DriverResult<Self> {
        samples(&wavelength_nm, &native_values)?;
        if context_before != context_after
            || context_before.params().sample_count != native_values.len()
            || native_unit != context_before.native_unit()?
            || context_before.params().active_trace != trace
            || (native_unit == NativeUnit::Watt && native_values.iter().any(|v| *v < 0.))
            || identity.trim().is_empty()
            || identity.len() > 1024
            || timing
                .started_utc
                .duration_since(SystemTime::UNIX_EPOCH)
                .is_err()
            || timing
                .finished_utc
                .duration_since(SystemTime::UNIX_EPOCH)
                .is_err()
        {
            return Err(DriverError::Protocol(
                "OSA capture context/unit/identity/timestamps disagree".into(),
            ));
        }
        Ok(Self {
            wavelength_nm,
            native_values,
            native_unit,
            trace,
            identity,
            timing,
            context_before,
            context_after,
        })
    }
    pub fn wavelength_nm(&self) -> &[f64] {
        &self.wavelength_nm
    }
    pub fn native_values(&self) -> &[f64] {
        &self.native_values
    }
    pub fn native_unit(&self) -> NativeUnit {
        self.native_unit
    }
    pub fn trace(&self) -> TraceId {
        self.trace
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn timing(&self) -> &ReadTiming {
        &self.timing
    }
    pub fn context_before(&self) -> &TraceContext {
        &self.context_before
    }
    pub fn context_after(&self) -> &TraceContext {
        &self.context_after
    }
    pub fn power_dbm(&self) -> Option<Vec<f64>> {
        if self.native_unit == NativeUnit::Dbm {
            return Some(self.native_values.clone());
        }
        if self.native_values.iter().any(|v| *v <= 0.) {
            return None;
        }
        let values: Vec<_> = self
            .native_values
            .iter()
            .map(|v| 10. * (v.log10() + 3.))
            .collect();
        values.iter().all(|v| v.is_finite()).then_some(values)
    }
    pub fn consistency(&self) -> &str {
        "unproven"
    }
    pub fn spectrum(&self) -> DriverResult<Spectrum> {
        Spectrum::new(self.wavelength_nm.clone(),self.power_dbm().ok_or_else(||DriverError::Protocol("Native W samples containing zero have no finite dBm compatibility representation".into()))?,self.trace,self.timing.finished_utc,self.identity.clone())
    }
}
impl Spectrum {
    pub fn new(
        wavelength_nm: Vec<f64>,
        power_dbm: Vec<f64>,
        trace: TraceId,
        acquired_at: SystemTime,
        identity: String,
    ) -> DriverResult<Self> {
        samples(&wavelength_nm, &power_dbm)?;
        if identity.trim().is_empty() || identity.len() > 1024 {
            return Err(DriverError::Protocol(
                "Spectrum source identity missing".into(),
            ));
        }
        Ok(Self {
            wavelength_nm,
            power_dbm,
            trace,
            acquired_at,
            identity,
        })
    }
    pub fn create(
        wavelength_nm: Vec<f64>,
        power_dbm: Vec<f64>,
        trace: TraceId,
        identity: String,
    ) -> DriverResult<Self> {
        Self::new(wavelength_nm, power_dbm, trace, SystemTime::now(), identity)
    }
    pub fn wavelength_nm(&self) -> &[f64] {
        &self.wavelength_nm
    }
    pub fn power_dbm(&self) -> &[f64] {
        &self.power_dbm
    }
    pub fn trace(&self) -> TraceId {
        self.trace
    }
    pub fn acquired_at(&self) -> SystemTime {
        self.acquired_at
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
}
