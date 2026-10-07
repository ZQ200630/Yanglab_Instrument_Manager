use super::{finite, Pm400};
use crate::{transport::Deadline, DriverError, DriverResult};
use serde::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum MeasurementKind {
    Power,
    Current,
    Voltage,
    Energy,
    Frequency,
    PowerDensity,
    EnergyDensity,
    Resistance,
    Temperature,
}
impl MeasurementKind {
    pub fn scpi_suffix(self) -> &'static str {
        match self {
            Self::Power => "POWer",
            Self::Current => "CURRent:DC",
            Self::Voltage => "VOLTage:DC",
            Self::Energy => "ENERgy",
            Self::Frequency => "FREQuency",
            Self::PowerDensity => "PDENsity",
            Self::EnergyDensity => "EDENsity",
            Self::Resistance => "RESistance",
            Self::Temperature => "TEMPerature",
        }
    }
    pub fn default_unit(self) -> &'static str {
        match self {
            Self::Power => "W",
            Self::Current => "A",
            Self::Voltage => "V",
            Self::Energy => "J",
            Self::Frequency => "Hz",
            Self::PowerDensity => "W/cm^2",
            Self::EnergyDensity => "J/cm^2",
            Self::Resistance => "ohm",
            Self::Temperature => "degC",
        }
    }
    pub fn parse_configuration(text: &str) -> DriverResult<Self> {
        let text = text.to_ascii_uppercase();
        for k in [
            Self::Power,
            Self::Current,
            Self::Voltage,
            Self::Energy,
            Self::Frequency,
            Self::PowerDensity,
            Self::EnergyDensity,
            Self::Resistance,
            Self::Temperature,
        ] {
            let suffix = k.scpi_suffix();
            let short: String = suffix
                .chars()
                .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == ':')
                .collect();
            if text == suffix.to_ascii_uppercase() || text == short {
                return Ok(k);
            }
        }
        Err(DriverError::Protocol(
            "unknown measurement configuration".into(),
        ))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum PowerUnit {
    Watts,
    Dbm,
}
impl PowerUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Watts => "W",
            Self::Dbm => "DBM",
        }
    }
    pub(crate) fn parse(text: &str) -> DriverResult<Self> {
        match text {
            "W" => Ok(Self::Watts),
            "DBM" => Ok(Self::Dbm),
            _ => Err(DriverError::Protocol("unknown power unit".into())),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Measurement {
    pub kind: MeasurementKind,
    pub value: f64,
    pub unit: String,
    pub measured_at: f64,
    pub raw: String,
}
impl Pm400 {
    fn result(
        &self,
        kind: MeasurementKind,
        text: String,
        unit: String,
    ) -> DriverResult<Measurement> {
        let value = finite(&text)?;
        if value.abs() >= 9.9e37 {
            return Err(DriverError::Protocol(
                "invalid-device measurement sentinel".into(),
            ));
        }
        Ok(Measurement {
            kind,
            value,
            unit,
            measured_at: self.clock.now().as_secs_f64(),
            raw: text,
        })
    }
    fn unit(&mut self, kind: MeasurementKind, deadline: Deadline) -> DriverResult<String> {
        if kind == MeasurementKind::Power {
            Ok(
                PowerUnit::parse(&self.query("SENSe:POWer:DC:UNIT?", deadline)?)?
                    .as_str()
                    .into(),
            )
        } else {
            Ok(kind.default_unit().into())
        }
    }
    pub fn get_configuration(&mut self) -> DriverResult<MeasurementKind> {
        let k = MeasurementKind::parse_configuration(&self.query("CONFigure?", self.deadline())?)?;
        self.supported(k)?;
        Ok(k)
    }
    pub fn configure(&mut self, kind: MeasurementKind) -> DriverResult<MeasurementKind> {
        self.supported(kind)?;
        self.action(
            &format!("CONFigure:SCALar:{}", kind.scpi_suffix()),
            self.deadline(),
        )?;
        Ok(kind)
    }
    pub fn initiate(&mut self) -> DriverResult<()> {
        self.action("INITiate:IMMediate", self.deadline())
    }
    pub fn abort(&mut self) -> DriverResult<()> {
        self.action("ABORt", self.deadline())
    }
    pub fn measure(
        &mut self,
        kind: MeasurementKind,
        deadline: Deadline,
    ) -> DriverResult<Measurement> {
        self.supported(kind)?;
        self.begin_measurement()?;
        let r = (|| {
            let unit = self.unit(kind, deadline)?;
            let raw = self.query(&format!("MEASure:SCALar:{}?", kind.scpi_suffix()), deadline)?;
            self.result(kind, raw, unit)
        })();
        self.finish_measurement(r)
    }
    pub fn fetch(&mut self, kind: MeasurementKind) -> DriverResult<Measurement> {
        self.supported(kind)?;
        let d = self.deadline();
        let unit = self.unit(kind, d)?;
        let raw = self.query("FETCh?", d)?;
        self.result(kind, raw, unit)
    }
    pub fn fetch_configured(&mut self) -> DriverResult<Measurement> {
        let k = self.get_configuration()?;
        self.fetch(k)
    }
    pub fn read(&mut self, kind: MeasurementKind, deadline: Deadline) -> DriverResult<Measurement> {
        self.supported(kind)?;
        self.begin_measurement()?;
        let r = (|| {
            self.action(
                &format!("CONFigure:SCALar:{}", kind.scpi_suffix()),
                deadline,
            )?;
            let unit = self.unit(kind, deadline)?;
            let raw = self.query("READ?", deadline)?;
            self.result(kind, raw, unit)
        })();
        self.finish_measurement(r)
    }
    pub fn read_configured(&mut self, deadline: Deadline) -> DriverResult<Measurement> {
        self.begin_measurement()?;
        let r = (|| {
            let k = MeasurementKind::parse_configuration(&self.query("CONFigure?", deadline)?)?;
            self.supported(k)?;
            let unit = self.unit(k, deadline)?;
            let raw = self.query("READ?", deadline)?;
            self.result(k, raw, unit)
        })();
        self.finish_measurement(r)
    }
}
macro_rules! convenience{($($method:ident=>$kind:ident),*)=>{impl Pm400{$(pub fn $method(&mut self,deadline:Deadline)->DriverResult<Measurement>{self.measure(MeasurementKind::$kind,deadline)})*}}}
convenience!(measure_power=>Power,measure_current=>Current,measure_voltage=>Voltage,measure_energy=>Energy,measure_frequency=>Frequency,measure_power_density=>PowerDensity,measure_energy_density=>EnergyDensity,measure_resistance=>Resistance,measure_temperature=>Temperature);
