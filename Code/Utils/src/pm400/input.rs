use super::{properties::Capability as C, LimitSelector, NumericValue, Pm400};
use crate::{DriverError, DriverResult};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterType {
    Photodiode,
    Thermal,
    Pyro,
}
impl AdapterType {
    fn token(self) -> &'static str {
        match self {
            Self::Photodiode => "PHOTodiode",
            Self::Thermal => "THERmal",
            Self::Pyro => "PYRo",
        }
    }
    fn parse(t: &str) -> DriverResult<Self> {
        match t {
            "PHOT" | "PHOTODIODE" => Ok(Self::Photodiode),
            "THER" | "THERMAL" => Ok(Self::Thermal),
            "PYR" | "PYRO" => Ok(Self::Pyro),
            _ => Err(DriverError::Protocol("invalid adapter type".into())),
        }
    }
}
pub struct Input<'a>(pub(crate) &'a mut Pm400);
impl Pm400 {
    pub fn input(&mut self) -> Input<'_> {
        Input(self)
    }
}
macro_rules! bools{($($set:ident,$get:ident,$header:literal);*$(;)?)=>{impl Input<'_>{$(pub fn $set(&mut self,v:bool)->DriverResult<bool>{self.0.capability(C::Power)?;self.0.set_boolean($header,v)}pub fn $get(&mut self)->DriverResult<bool>{self.0.capability(C::Power)?;self.0.get_boolean($header)})*}}}
bools!(set_photodiode_lowpass_enabled,get_photodiode_lowpass_enabled,"INPut:PDIOde:FILTer:LPASs:STATe";set_thermopile_accelerator_enabled,get_thermopile_accelerator_enabled,"INPut:THERmopile:ACCelerator:STATe";set_thermopile_accelerator_auto,get_thermopile_accelerator_auto,"INPut:THERmopile:ACCelerator:AUTO";);
impl Input<'_> {
    pub fn set_thermopile_tau_s(&mut self, v: impl Into<NumericValue>) -> DriverResult<f64> {
        self.0.capability(C::Tau)?;
        self.0
            .set_numeric("INPut:THERmopile:ACCelerator:TAU", v.into(), true)
    }
    pub fn get_thermopile_tau_s(&mut self, s: Option<LimitSelector>) -> DriverResult<f64> {
        self.0.capability(C::Power)?;
        self.0
            .get_numeric("INPut:THERmopile:ACCelerator:TAU", s, true)
    }
    pub fn set_adapter_type(&mut self, v: AdapterType, confirm: bool) -> DriverResult<AdapterType> {
        Pm400::confirm(confirm)?;
        self.0.set_confirm(
            "INPut:ADAPter:TYPE",
            v.token(),
            v,
            AdapterType::parse,
            |a, b| a == b,
        )
    }
    pub fn get_adapter_type(&mut self) -> DriverResult<AdapterType> {
        AdapterType::parse(&self.0.query("INPut:ADAPter:TYPE?", self.0.deadline())?)
    }
}
