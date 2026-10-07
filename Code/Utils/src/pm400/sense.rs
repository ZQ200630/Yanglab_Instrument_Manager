use super::{finite, properties::Capability as C, LimitSelector, NumericValue, Pm400, PowerUnit};
use crate::DriverResult;
pub struct Sense<'a>(pub(crate) &'a mut Pm400);
impl Pm400 {
    pub fn sense(&mut self) -> Sense<'_> {
        Sense(self)
    }
}
macro_rules! numeric{($($set:ident,$get:ident,$header:literal,$cap:ident,$default:literal);*$(;)?)=>{impl Sense<'_>{$(pub fn $set(&mut self,value:impl Into<NumericValue>)->DriverResult<f64>{self.0.capability(C::$cap)?;self.0.set_numeric($header,value.into(),$default)}pub fn $get(&mut self,selector:Option<LimitSelector>)->DriverResult<f64>{self.0.capability(C::$cap)?;self.0.get_numeric($header,selector,$default)})*}}}
numeric!(
set_loss_db,get_loss_db,"SENSe:CORRection:LOSS:INPut:MAGNitude",None,true;
set_beam_diameter_mm,get_beam_diameter_mm,"SENSe:CORRection:BEAMdiameter",None,true;
set_current_range_a,get_current_range_a,"SENSe:CURRent:DC:RANGe:UPPer",Power,false;
set_current_reference_a,get_current_reference_a,"SENSe:CURRent:DC:REFerence",Power,true;
set_energy_range_j,get_energy_range_j,"SENSe:ENERgy:RANGe:UPPer",Energy,false;
set_energy_reference_j,get_energy_reference_j,"SENSe:ENERgy:REFerence",Energy,true;
set_power_range_w,get_power_range_w,"SENSe:POWer:DC:RANGe:UPPer",Power,false;
set_power_reference_w,get_power_reference_w,"SENSe:POWer:DC:REFerence",Power,true;
set_voltage_range_v,get_voltage_range_v,"SENSe:VOLTage:DC:RANGe:UPPer",Power,false;
set_voltage_reference_v,get_voltage_reference_v,"SENSe:VOLTage:DC:REFerence",Power,true;
set_peak_threshold_percent,get_peak_threshold_percent,"SENSe:PEAKdetector:THReshold",Energy,true;
);
macro_rules! bools{($($set:ident,$get:ident,$header:literal,$cap:ident);*$(;)?)=>{impl Sense<'_>{$(pub fn $set(&mut self,value:bool)->DriverResult<bool>{self.0.capability(C::$cap)?;self.0.set_boolean($header,value)}pub fn $get(&mut self)->DriverResult<bool>{self.0.capability(C::$cap)?;self.0.get_boolean($header)})*}}}
bools!(
set_current_auto_range,get_current_auto_range,"SENSe:CURRent:DC:RANGe:AUTO",Power;
set_current_delta_enabled,get_current_delta_enabled,"SENSe:CURRent:DC:REFerence:STATe",Power;
set_energy_delta_enabled,get_energy_delta_enabled,"SENSe:ENERgy:REFerence:STATe",Energy;
set_power_auto_range,get_power_auto_range,"SENSe:POWer:DC:RANGe:AUTO",Power;
set_power_delta_enabled,get_power_delta_enabled,"SENSe:POWer:DC:REFerence:STATe",Power;
set_voltage_auto_range,get_voltage_auto_range,"SENSe:VOLTage:DC:RANGe:AUTO",Power;
set_voltage_delta_enabled,get_voltage_delta_enabled,"SENSe:VOLTage:DC:REFerence:STATe",Power;
);
macro_rules! responses{($($set:ident,$get:ident,$header:literal,$setcap:ident,$getcap:ident);*$(;)?)=>{impl Sense<'_>{$(pub fn $set(&mut self,value:impl Into<NumericValue>,confirm:bool)->DriverResult<f64>{Pm400::confirm(confirm)?;self.0.capability(C::$setcap)?;self.0.set_numeric($header,value.into(),true)}pub fn $get(&mut self,selector:Option<LimitSelector>)->DriverResult<f64>{self.0.capability(C::$getcap)?;self.0.get_numeric($header,selector,true)})*}}}
responses!(set_photodiode_response_a_per_w,get_photodiode_response_a_per_w,"SENSe:CORRection:POWer:PDIOde:RESPonse",ResponsePower,Power;set_thermopile_response_v_per_w,get_thermopile_response_v_per_w,"SENSe:CORRection:POWer:THERmopile:RESPonse",ResponsePower,Power;set_pyro_response_v_per_j,get_pyro_response_v_per_j,"SENSe:CORRection:ENERgy:PYRO:RESPonse",ResponseEnergy,Energy;);
impl Sense<'_> {
    pub fn set_average_count(&mut self, v: u32) -> DriverResult<u32> {
        self.0.set_integer("SENSe:AVERage:COUNt", v, 1, u32::MAX)
    }
    pub fn get_average_count(&mut self) -> DriverResult<u32> {
        self.0.get_integer("SENSe:AVERage:COUNt", 1, u32::MAX)
    }
    pub fn set_wavelength_nm(&mut self, v: impl Into<NumericValue>) -> DriverResult<f64> {
        self.0.capability(C::Wavelength)?;
        self.0
            .set_numeric("SENSe:CORRection:WAVelength", v.into(), false)
    }
    pub fn get_wavelength_nm(&mut self, s: Option<LimitSelector>) -> DriverResult<f64> {
        self.0.get_numeric("SENSe:CORRection:WAVelength", s, false)
    }
    pub fn start_zero_collection(&mut self, confirm: bool) -> DriverResult<()> {
        Pm400::confirm(confirm)?;
        self.0.capability(C::Optical)?;
        self.0
            .action("SENSe:CORRection:COLLect:ZERO:INITiate", self.0.deadline())
    }
    pub fn abort_zero_collection(&mut self) -> DriverResult<()> {
        self.0.capability(C::Optical)?;
        self.0
            .action("SENSe:CORRection:COLLect:ZERO:ABORt", self.0.deadline())
    }
    pub fn get_zero_state(&mut self) -> DriverResult<bool> {
        self.0.get_boolean("SENSe:CORRection:COLLect:ZERO:STATe")
    }
    pub fn get_zero_magnitude(&mut self) -> DriverResult<f64> {
        self.0
            .get_numeric("SENSe:CORRection:COLLect:ZERO:MAGNitude", None, false)
    }
    pub fn get_frequency_upper_hz(&mut self) -> DriverResult<f64> {
        finite(
            &self
                .0
                .query("SENSe:FREQuency:RANGe:UPPer?", self.0.deadline())?,
        )
    }
    pub fn get_frequency_lower_hz(&mut self) -> DriverResult<f64> {
        finite(
            &self
                .0
                .query("SENSe:FREQuency:RANGe:LOWer?", self.0.deadline())?,
        )
    }
    pub fn set_power_unit(&mut self, v: PowerUnit) -> DriverResult<PowerUnit> {
        self.0.capability(C::Power)?;
        self.0.set_confirm(
            "SENSe:POWer:DC:UNIT",
            v.as_str(),
            v,
            PowerUnit::parse,
            |a, b| a == b,
        )
    }
    pub fn get_power_unit(&mut self) -> DriverResult<PowerUnit> {
        self.0.capability(C::Power)?;
        PowerUnit::parse(&self.0.query("SENSe:POWer:DC:UNIT?", self.0.deadline())?)
    }
}
