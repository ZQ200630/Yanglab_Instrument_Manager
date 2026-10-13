//! Rigol DG4000 and DG900 Pro adapters. Output enable is always explicit.
pub use crate::scpi::StopHandle;
use crate::{
    clock::Clock,
    lifecycle::{CleanupReport, DriverLifecycle, DriverState, ProbeReport},
    scpi::{self, Session},
    transport::{ByteTransport, Deadline, VisaManager},
    DriverError, DriverResult,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeneratorModel {
    Dg4162,
    Dg4202,
    Dg922Pro,
}
impl GeneratorModel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dg4162 => "DG4162",
            Self::Dg4202 => "DG4202",
            Self::Dg922Pro => "DG922 Pro",
        }
    }
}
#[derive(Clone, Debug)]
pub struct GeneratorOptions {
    pub timeout: Duration,
    pub close_timeout: Duration,
}
impl Default for GeneratorOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            close_timeout: Duration::from_secs(2),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Waveform {
    Sine,
    Square,
    Triangle,
    Ramp,
    Pulse,
    Noise,
    Dc,
}
impl Waveform {
    fn command(self) -> &'static str {
        match self {
            Self::Sine => "SINusoid",
            Self::Square => "SQUare",
            Self::Triangle | Self::Ramp => "RAMP",
            Self::Pulse => "PULSe",
            Self::Noise => "NOISe",
            Self::Dc => "DC",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalConfig {
    pub waveform: Waveform,
    pub frequency_hz: f64,
    pub amplitude_vpp: f64,
    pub offset_v: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputLoad {
    HighZ,
    Ohms { ohms: f64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputPolarity {
    Normal,
    Inverted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeneratorCapabilities {
    pub channels: u8,
    pub phase: bool,
    pub load: bool,
    pub polarity: bool,
    pub waveforms: Vec<Waveform>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelSignal {
    pub channel: u8,
    pub waveform: Waveform,
    pub frequency_hz: f64,
    pub amplitude_vpp: f64,
    pub offset_v: f64,
    pub output_enabled: bool,
    pub phase_deg: f64,
    pub load: OutputLoad,
    pub polarity: OutputPolarity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeneratorStatus {
    pub model: GeneratorModel,
    pub identity: String,
    pub channels: Vec<ChannelSignal>,
    pub capabilities: GeneratorCapabilities,
}
/// Pure, transport-free validation shared by the driver and experiment admission.
/// The returned plan contains only validated write-order choices, not authority.
pub struct SignalTransition {
    shape_first: bool,
    amplitude_first: bool,
}
impl GeneratorModel {
    fn max_frequency(self, waveform: Waveform) -> f64 {
        match waveform {
            Waveform::Sine => {
                if self == Self::Dg4162 {
                    160e6
                } else {
                    200e6
                }
            }
            Waveform::Square => {
                if self == Self::Dg4162 {
                    50e6
                } else {
                    60e6
                }
            }
            Waveform::Triangle | Waveform::Ramp => {
                if self != Self::Dg4162 {
                    5e6
                } else {
                    4e6
                }
            }
            Waveform::Pulse => {
                if self != Self::Dg4162 {
                    50e6
                } else {
                    40e6
                }
            }
            Waveform::Noise | Waveform::Dc => 200e6,
        }
    }
    fn check_basic(self, signal: &SignalConfig) -> DriverResult<()> {
        if !signal.frequency_hz.is_finite()
            || !(1e-6..=self.max_frequency(signal.waveform)).contains(&signal.frequency_hz)
            || !signal.amplitude_vpp.is_finite()
            || signal.amplitude_vpp <= 0.
            || signal.amplitude_vpp > 20.
            || !signal.offset_v.is_finite()
            || signal.offset_v.abs() > 10.
        {
            return Err(DriverError::Invalid(
                "generator waveform/frequency/voltage outside model bounds".into(),
            ));
        }
        Ok(())
    }
    fn max_vpp(self, frequency: f64, waveform: Waveform, load: OutputLoad) -> f64 {
        let high = if waveform == Waveform::Noise {
            4.
        } else if self == Self::Dg922Pro {
            if frequency <= 50e6 {
                20.
            } else if frequency <= 100e6 {
                10.
            } else {
                4.
            }
        } else if frequency <= 20e6 {
            20.
        } else if frequency <= 70e6 {
            10.
        } else if frequency <= 120e6 {
            5.
        } else {
            2.
        };
        high * match load {
            OutputLoad::HighZ => 1.,
            OutputLoad::Ohms { ohms } => ohms / (ohms + 50.),
        }
    }
    fn peak_envelope(self, frequency: f64, waveform: Waveform, load: OutputLoad) -> f64 {
        if self == Self::Dg922Pro {
            self.max_vpp(frequency, waveform, load)
        } else {
            20. * match load {
                OutputLoad::HighZ => 1.,
                OutputLoad::Ohms { ohms } => ohms / (ohms + 50.),
            }
        }
    }
    fn validate_load(self, load: OutputLoad) -> DriverResult<()> {
        if matches!(load,OutputLoad::Ohms{ohms} if !ohms.is_finite() || !(1.0..=10000.0).contains(&ohms) || self!=Self::Dg922Pro&&ohms.fract()!=0.)
        {
            return Err(DriverError::Invalid(
                "generator load must be HighZ or a supported resistance (DG4000: whole ohms)"
                    .into(),
            ));
        }
        Ok(())
    }
    /// Validate requested levels for the actual model and read-back output load.
    pub fn validate_signal(self, signal: &SignalConfig, load: OutputLoad) -> DriverResult<()> {
        self.check_basic(signal)?;
        self.validate_load(load)?;
        let max = self.max_vpp(signal.frequency_hz, signal.waveform, load);
        let min = 0.002
            * match load {
                OutputLoad::HighZ => 1.,
                OutputLoad::Ohms { ohms } => ohms / (ohms + 50.),
            };
        let envelope = self.peak_envelope(signal.frequency_hz, signal.waveform, load);
        if signal.amplitude_vpp < min
            || signal.amplitude_vpp > max + 1e-12
            || signal.amplitude_vpp + 2. * signal.offset_v.abs() > envelope + 1e-12
        {
            return Err(DriverError::Invalid(format!(
                "generator amplitude exceeds {max} Vpp or Vpp + 2*abs(offset) exceeds {envelope} V at current load/frequency"
            )));
        }
        Ok(())
    }
    /// Validate both present readback and target, then determine the same safe
    /// intermediate ordering the real driver will use. Does not open or write I/O.
    pub fn validate_signal_transition(
        self,
        signal: &SignalConfig,
        current: &ChannelSignal,
    ) -> DriverResult<SignalTransition> {
        channel(current.channel)?;
        self.validate_signal(signal, current.load)?;
        self.validate_signal(
            &SignalConfig {
                waveform: current.waveform,
                frequency_hz: current.frequency_hz,
                amplitude_vpp: current.amplitude_vpp,
                offset_v: current.offset_v,
            },
            current.load,
        )?;
        let frequency = current.frequency_hz.min(signal.frequency_hz);
        let old_max = self.max_vpp(frequency, current.waveform, current.load);
        let new_max = self.max_vpp(frequency, signal.waveform, current.load);
        let shape_first = new_max > old_max;
        let max = self.peak_envelope(
            frequency,
            if shape_first {
                signal.waveform
            } else {
                current.waveform
            },
            current.load,
        );
        let amplitude_first = signal.amplitude_vpp + 2. * current.offset_v.abs() <= max;
        if !amplitude_first && current.amplitude_vpp + 2. * signal.offset_v.abs() > max {
            return Err(DriverError::Invalid(
                "voltage transition has no safe intermediate level".into(),
            ));
        }
        Ok(SignalTransition {
            shape_first,
            amplitude_first,
        })
    }
}
#[cfg(test)]
mod validation_tests {
    use super::*;
    #[test]
    fn dg4202_uses_its_own_ramp_and_pulse_bandwidth() {
        for (waveform, maximum) in [
            (Waveform::Triangle, 5e6),
            (Waveform::Ramp, 5e6),
            (Waveform::Pulse, 50e6),
        ] {
            let mut signal = SignalConfig {
                waveform,
                frequency_hz: maximum,
                amplitude_vpp: 1.,
                offset_v: 0.,
            };
            assert!(GeneratorModel::Dg4202
                .validate_signal(&signal, OutputLoad::HighZ)
                .is_ok());
            signal.frequency_hz = maximum + 1.;
            assert!(GeneratorModel::Dg4202
                .validate_signal(&signal, OutputLoad::HighZ)
                .is_err());
        }
    }
    #[test]
    fn dg4000_separates_ac_derating_from_full_peak_envelope() {
        let mut signal = SignalConfig {
            waveform: Waveform::Sine,
            frequency_hz: 150e6,
            amplitude_vpp: 1.,
            offset_v: 3.,
        };
        assert!(GeneratorModel::Dg4202
            .validate_signal(&signal, OutputLoad::Ohms { ohms: 50. })
            .is_ok());
        signal.amplitude_vpp = 1.01;
        assert!(GeneratorModel::Dg4202
            .validate_signal(&signal, OutputLoad::Ohms { ohms: 50. })
            .is_err());
        signal.amplitude_vpp = 1.;
        signal.offset_v = 4.51;
        assert!(GeneratorModel::Dg4202
            .validate_signal(&signal, OutputLoad::Ohms { ohms: 50. })
            .is_err());
    }
    #[test]
    fn dg4000_rejects_fractional_loads_in_shared_preflight() {
        let signal = SignalConfig {
            waveform: Waveform::Sine,
            frequency_hz: 50.,
            amplitude_vpp: 1.,
            offset_v: 0.,
        };
        assert!(GeneratorModel::Dg4202
            .validate_signal(&signal, OutputLoad::Ohms { ohms: 75.5 })
            .is_err());
        assert!(GeneratorModel::Dg4202
            .validate_signal(&signal, OutputLoad::Ohms { ohms: 75. })
            .is_ok());
    }
    #[test]
    fn pure_preflight_enforces_model_load_levels_and_existing_transition() {
        let mut requested = SignalConfig {
            waveform: Waveform::Triangle,
            frequency_hz: 5e6,
            amplitude_vpp: 2.,
            offset_v: 0.,
        };
        assert!(GeneratorModel::Dg4162
            .validate_signal(&requested, OutputLoad::HighZ)
            .is_err());
        assert!(GeneratorModel::Dg922Pro
            .validate_signal(&requested, OutputLoad::HighZ)
            .is_ok());
        requested.frequency_hz = 1000.;
        requested.amplitude_vpp = 0.001;
        assert!(GeneratorModel::Dg4162
            .validate_signal(&requested, OutputLoad::HighZ)
            .is_err());
        assert!(GeneratorModel::Dg4162
            .validate_signal(&requested, OutputLoad::Ohms { ohms: 50. })
            .is_ok());
        requested.amplitude_vpp = 8.;
        requested.offset_v = 2.;
        assert!(GeneratorModel::Dg4162
            .validate_signal(&requested, OutputLoad::Ohms { ohms: 50. })
            .is_err());
        requested.amplitude_vpp = 2.;
        requested.offset_v = 0.;
        let mut current = ChannelSignal {
            channel: 1,
            waveform: Waveform::Sine,
            frequency_hz: 1000.,
            amplitude_vpp: 2.,
            offset_v: 0.,
            output_enabled: false,
            phase_deg: 0.,
            load: OutputLoad::HighZ,
            polarity: OutputPolarity::Normal,
        };
        assert!(GeneratorModel::Dg4162
            .validate_signal_transition(&requested, &current)
            .is_ok());
        current.amplitude_vpp = 20.;
        current.offset_v = 1.;
        assert!(GeneratorModel::Dg4162
            .validate_signal_transition(&requested, &current)
            .is_err());
        current.amplitude_vpp = 2.;
        current.offset_v = 0.;
        current.load = OutputLoad::Ohms { ohms: f64::NAN };
        assert!(GeneratorModel::Dg4162
            .validate_signal_transition(&requested, &current)
            .is_err());
    }
}
pub struct Generator {
    model: GeneratorModel,
    session: Session,
    options: GeneratorOptions,
}
fn channel(channel: u8) -> DriverResult<()> {
    if (1..=2).contains(&channel) {
        Ok(())
    } else {
        Err(DriverError::Invalid(
            "generator channel must be 1 or 2".into(),
        ))
    }
}
impl Generator {
    pub fn new(
        model: GeneratorModel,
        manager: VisaManager,
        resource: impl Into<String>,
        clock: Arc<dyn Clock>,
        options: GeneratorOptions,
    ) -> DriverResult<Self> {
        Self::build(model, Some(manager), resource.into(), None, clock, options)
    }
    pub fn with_transport(
        model: GeneratorModel,
        transport: Box<dyn ByteTransport>,
        clock: Arc<dyn Clock>,
        options: GeneratorOptions,
    ) -> DriverResult<Self> {
        Self::build(model, None, String::new(), Some(transport), clock, options)
    }
    fn build(
        model: GeneratorModel,
        manager: Option<VisaManager>,
        resource: String,
        io: Option<Box<dyn ByteTransport>>,
        clock: Arc<dyn Clock>,
        options: GeneratorOptions,
    ) -> DriverResult<Self> {
        Ok(Self {
            model,
            session: Session::new(
                manager,
                resource,
                io,
                clock,
                options.timeout,
                options.close_timeout,
            )?,
            options,
        })
    }
    pub fn connect(&mut self) -> DriverResult<ProbeReport> {
        self.session.connect(self.model.name())
    }
    pub fn capabilities(&self) -> GeneratorCapabilities {
        GeneratorCapabilities {
            channels: 2,
            phase: true,
            load: true,
            polarity: true,
            waveforms: vec![
                Waveform::Sine,
                Waveform::Square,
                Waveform::Triangle,
                Waveform::Ramp,
                Waveform::Pulse,
                Waveform::Noise,
                Waveform::Dc,
            ],
        }
    }
    fn query(&mut self, channel: u8, suffix: &str, deadline: Deadline) -> DriverResult<String> {
        self.session
            .query_text(&format!(":SOURce{channel}:{suffix}?"), deadline)
    }
    fn load_inner(&mut self, channel: u8, deadline: Deadline) -> DriverResult<OutputLoad> {
        let text = self
            .session
            .query_text(&format!(":OUTPut{channel}:LOAD?"), deadline)?;
        if matches!(
            text.to_ascii_uppercase().as_str(),
            "INF" | "INFINITY" | "HIGHZ"
        ) {
            return Ok(OutputLoad::HighZ);
        }
        let ohms = scpi::number(&text)?;
        if ohms >= 1e30 {
            return Ok(OutputLoad::HighZ);
        }
        if (1.0..=10000.0).contains(&ohms) {
            Ok(OutputLoad::Ohms { ohms })
        } else {
            Err(DriverError::Protocol(
                "invalid generator load readback".into(),
            ))
        }
    }
    fn waveform_inner(&mut self, channel: u8, deadline: Deadline) -> DriverResult<Waveform> {
        let shape = self
            .query(channel, "FUNCtion", deadline)?
            .to_ascii_uppercase();
        match shape.as_str() {
            "SIN" | "SINUSOID" => Ok(Waveform::Sine),
            "SQU" | "SQUARE" => Ok(Waveform::Square),
            "RAMP" => {
                let symmetry =
                    scpi::number(&self.query(channel, "FUNCtion:RAMP:SYMMetry", deadline)?)?;
                if !(0.0..=100.0).contains(&symmetry) {
                    return Err(DriverError::Protocol("invalid generator symmetry".into()));
                }
                Ok(if (symmetry - 50.0).abs() < 1e-6 {
                    Waveform::Triangle
                } else {
                    Waveform::Ramp
                })
            }
            "PULS" | "PULSE" => Ok(Waveform::Pulse),
            "NOIS" | "NOISE" => Ok(Waveform::Noise),
            "DC" => Ok(Waveform::Dc),
            _ => Err(DriverError::Protocol(format!(
                "unsupported generator waveform readback {shape}"
            ))),
        }
    }
    fn amplitude(
        &mut self,
        channel: u8,
        waveform: Waveform,
        load: OutputLoad,
        deadline: Deadline,
    ) -> DriverResult<(f64, String)> {
        let unit = self
            .query(channel, "VOLTage:UNIT", deadline)?
            .to_ascii_uppercase();
        let raw = scpi::number(&self.query(channel, "VOLTage", deadline)?)?;
        let vpp = match unit.as_str() {
            "VPP" => raw,
            "VRMS" | "DBM" => {
                let rms = if unit == "DBM" {
                    let OutputLoad::Ohms { ohms } = load else {
                        return Err(DriverError::Protocol("dBm readback with HighZ".into()));
                    };
                    (0.001 * 10f64.powf(raw / 10.0) * ohms).sqrt()
                } else {
                    raw
                };
                rms*match waveform{Waveform::Sine=>2.0*2f64.sqrt(),Waveform::Square=>2.0,Waveform::Triangle|Waveform::Ramp=>2.0*3f64.sqrt(),_=>return Err(DriverError::Protocol("amplitude unit conversion unsupported for this waveform; select Vpp explicitly".into()))}
            }
            _ => {
                return Err(DriverError::Protocol(
                    "unknown generator amplitude unit".into(),
                ))
            }
        };
        if vpp.is_finite() && vpp >= 0.0 {
            Ok((vpp, unit))
        } else {
            Err(DriverError::Protocol("invalid generator amplitude".into()))
        }
    }
    fn output_command(&self, channel: u8) -> String {
        format!(
            ":OUTPut{channel}{}",
            if self.model == GeneratorModel::Dg922Pro {
                ":STATe"
            } else {
                ""
            }
        )
    }
    pub fn output_enabled(&mut self, channel: u8, deadline: Deadline) -> DriverResult<bool> {
        channel_check(channel)?;
        self.session.ready(deadline)?;
        scpi::boolean(
            &self
                .session
                .query_text(&format!("{}?", self.output_command(channel)), deadline)?,
        )
    }
    pub fn read_signal(&mut self, channel: u8, deadline: Deadline) -> DriverResult<ChannelSignal> {
        channel_check(channel)?;
        self.session.ready(deadline)?;
        let load = self.load_inner(channel, deadline)?;
        let waveform = self.waveform_inner(channel, deadline)?;
        let frequency_hz = scpi::number(&self.query(channel, "FREQuency", deadline)?)?;
        let amplitude_vpp = self.amplitude(channel, waveform, load, deadline)?.0;
        let offset_v = scpi::number(&self.query(channel, "VOLTage:OFFSet", deadline)?)?;
        let phase_deg = scpi::number(&self.query(channel, "PHASe", deadline)?)?;
        let polarity = match self
            .session
            .query_text(&format!(":OUTPut{channel}:POLarity?"), deadline)?
            .to_ascii_uppercase()
            .as_str()
        {
            "NORM" | "NORMAL" => OutputPolarity::Normal,
            "INV" | "INVERTED" => OutputPolarity::Inverted,
            _ => {
                return Err(DriverError::Protocol(
                    "invalid generator polarity readback".into(),
                ))
            }
        };
        let output_enabled = self.output_enabled(channel, deadline)?;
        Ok(ChannelSignal {
            channel,
            waveform,
            frequency_hz,
            amplitude_vpp,
            offset_v,
            phase_deg,
            load,
            polarity,
            output_enabled,
        })
    }
    pub fn read_status(&mut self, deadline: Deadline) -> DriverResult<GeneratorStatus> {
        let channels = vec![
            self.read_signal(1, deadline)?,
            self.read_signal(2, deadline)?,
        ];
        Ok(GeneratorStatus {
            model: self.model,
            identity: self.identity().into(),
            channels,
            capabilities: self.capabilities(),
        })
    }
    fn check_basic(&self, signal: &SignalConfig) -> DriverResult<()> {
        self.model.check_basic(signal)
    }
    fn check_levels(&self, signal: &SignalConfig, load: OutputLoad) -> DriverResult<()> {
        self.model.validate_signal(signal, load)
    }
    fn require_independent_continuous(
        &mut self,
        channel: u8,
        deadline: Deadline,
    ) -> DriverResult<()> {
        if self.model == GeneratorModel::Dg922Pro {
            return Ok(());
        }
        // DG4000 FUNCtion writes edit a carrier in Mod/Sweep/Burst, and DC disables
        // those modes. Coupled writes can also change the other channel. Preserve
        // these panel modes rather than silently converting them to ordinary CW.
        for command in [
            format!(":SOURce{channel}:MOD:STATe?"),
            format!(":SOURce{channel}:SWEep:STATe?"),
            format!(":SOURce{channel}:BURSt:STATe?"),
            ":COUPling:AMPL:STATe?".into(),
            ":COUPling:FREQuency:STATe?".into(),
            ":COUPling:PHASe:STATe?".into(),
        ] {
            if scpi::boolean(&self.session.query_text(&command, deadline)?)? {
                return Err(DriverError::Invalid("Basic controls require continuous, uncoupled channels. Disable modulation/sweep/burst and channel coupling on the panel, then Sync; panel state was preserved.".into()));
            }
        }
        Ok(())
    }
    pub fn configure_signal(
        &mut self,
        channel: u8,
        signal: &SignalConfig,
        deadline: Deadline,
    ) -> DriverResult<()> {
        channel_check(channel)?;
        self.check_basic(signal)?;
        self.session.ready(deadline)?;
        let load = self.load_inner(channel, deadline)?;
        self.check_levels(signal, load)?;
        // Validate all requested values before any write. Existing voltage settings
        // determine the safe order of the combined amplitude/offset transition.
        let current_waveform = self.waveform_inner(channel, deadline)?;
        let current_frequency = scpi::number(&self.query(channel, "FREQuency", deadline)?)?;
        let (current_amplitude, unit) =
            self.amplitude(channel, current_waveform, load, deadline)?;
        let current_offset = scpi::number(&self.query(channel, "VOLTage:OFFSet", deadline)?)?;
        let SignalTransition {
            shape_first,
            amplitude_first,
        } = self.model.validate_signal_transition(
            signal,
            &ChannelSignal {
                channel,
                waveform: current_waveform,
                frequency_hz: current_frequency,
                amplitude_vpp: current_amplitude,
                offset_v: current_offset,
                output_enabled: false,
                phase_deg: 0.,
                load,
                polarity: OutputPolarity::Normal,
            },
        )?;
        self.require_independent_continuous(channel, deadline)?;
        let result = (|| {
            if signal.frequency_hz < current_frequency {
                self.session.write(
                    &format!(":SOURce{channel}:FREQuency {}", signal.frequency_hz),
                    deadline,
                )?;
            }
            if unit != "VPP" {
                self.session
                    .write(&format!(":SOURce{channel}:VOLTage:UNIT VPP"), deadline)?;
            }
            if shape_first {
                self.write_shape(channel, signal.waveform, deadline)?;
            }
            if amplitude_first {
                self.session.write(
                    &format!(":SOURce{channel}:VOLTage {}", signal.amplitude_vpp),
                    deadline,
                )?;
                self.session.write(
                    &format!(":SOURce{channel}:VOLTage:OFFSet {}", signal.offset_v),
                    deadline,
                )?;
            } else {
                self.session.write(
                    &format!(":SOURce{channel}:VOLTage:OFFSet {}", signal.offset_v),
                    deadline,
                )?;
                self.session.write(
                    &format!(":SOURce{channel}:VOLTage {}", signal.amplitude_vpp),
                    deadline,
                )?;
            }
            if !shape_first {
                self.write_shape(channel, signal.waveform, deadline)?;
            }
            if signal.frequency_hz >= current_frequency {
                self.session.write(
                    &format!(":SOURce{channel}:FREQuency {}", signal.frequency_hz),
                    deadline,
                )?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.session.state = DriverState::Fault;
        }
        result
    }
    fn write_shape(
        &mut self,
        channel: u8,
        waveform: Waveform,
        deadline: Deadline,
    ) -> DriverResult<()> {
        self.session.write(
            &format!(":SOURce{channel}:FUNCtion {}", waveform.command()),
            deadline,
        )?;
        let symmetry = match waveform {
            Waveform::Triangle => Some(50),
            Waveform::Ramp => Some(100),
            _ => None,
        };
        if let Some(symmetry) = symmetry {
            self.session.write(
                &format!(":SOURce{channel}:FUNCtion:RAMP:SYMMetry {symmetry}"),
                deadline,
            )?;
        }
        Ok(())
    }
    pub fn set_output(
        &mut self,
        channel: u8,
        enabled: bool,
        deadline: Deadline,
    ) -> DriverResult<()> {
        channel_check(channel)?;
        if !enabled {
            return self.session.write_explicit_disable(
                &format!("{} OFF", self.output_command(channel)),
                deadline,
            );
        }
        self.session.ready(deadline)?;
        self.session.write(
            &format!(
                "{} {}",
                self.output_command(channel),
                if enabled { "ON" } else { "OFF" }
            ),
            deadline,
        )
    }
    pub fn set_phase(&mut self, channel: u8, degrees: f64, deadline: Deadline) -> DriverResult<()> {
        channel_check(channel)?;
        if !self.capabilities().phase || !degrees.is_finite() || !(0.0..=360.0).contains(&degrees) {
            return Err(DriverError::Invalid("unsupported generator phase".into()));
        }
        self.session.ready(deadline)?;
        self.require_independent_continuous(channel, deadline)?;
        self.session
            .write(&format!(":SOURce{channel}:PHASe {degrees}"), deadline)
    }
    pub fn set_load(
        &mut self,
        channel: u8,
        load: OutputLoad,
        deadline: Deadline,
    ) -> DriverResult<()> {
        channel_check(channel)?;
        self.model.validate_load(load)?;
        self.session.ready(deadline)?;
        let current = self.read_signal(channel, deadline)?;
        // DG4000 LOAD changes the display's termination assumption, not the
        // physical output. HighZ 20 Vpp becomes 50-ohm 10 Vpp without VOLT writes.
        if self.model == GeneratorModel::Dg922Pro {
            self.check_levels(
                &SignalConfig {
                    waveform: current.waveform,
                    frequency_hz: current.frequency_hz,
                    amplitude_vpp: current.amplitude_vpp,
                    offset_v: current.offset_v,
                },
                load,
            )?;
        }
        let value = match load {
            OutputLoad::HighZ => "INF".into(),
            OutputLoad::Ohms { ohms } => format!("{ohms}"),
        };
        self.session
            .write(&format!(":OUTPut{channel}:LOAD {value}"), deadline)
    }
    pub fn set_polarity(
        &mut self,
        channel: u8,
        polarity: OutputPolarity,
        deadline: Deadline,
    ) -> DriverResult<()> {
        channel_check(channel)?;
        if !self.capabilities().polarity {
            return Err(DriverError::Invalid(
                "generator polarity unsupported".into(),
            ));
        }
        self.session.ready(deadline)?;
        self.session.write(
            &format!(
                ":OUTPut{channel}:POLarity {}",
                if polarity == OutputPolarity::Normal {
                    "NORMal"
                } else {
                    "INVerted"
                }
            ),
            deadline,
        )
    }
    /// Explicit operator action; normal connect/read/close never changes amplitude units.
    pub fn set_amplitude_unit_vpp(&mut self, channel: u8, deadline: Deadline) -> DriverResult<()> {
        channel_check(channel)?;
        self.session.ready(deadline)?;
        self.session
            .write(&format!(":SOURce{channel}:VOLTage:UNIT VPP"), deadline)
    }
    pub fn close(&mut self) -> DriverResult<CleanupReport> {
        self.session.close("generator")
    }
    pub fn state(&self) -> DriverState {
        self.session.state
    }
    pub fn identity(&self) -> &str {
        &self.session.identity
    }
    pub fn resource_name(&self) -> &str {
        self.session.resource_name()
    }
    pub fn stop_handle(&self) -> StopHandle {
        self.session.stop.clone()
    }
    pub fn has_resource_responsibility(&self) -> bool {
        self.session.has_responsibility()
    }
    pub fn cleanup_report(&self) -> Option<&CleanupReport> {
        self.session.cleanup_report()
    }
    pub fn options(&self) -> &GeneratorOptions {
        &self.options
    }
}
fn channel_check(value: u8) -> DriverResult<()> {
    channel(value)
}
impl DriverLifecycle for Generator {
    fn close(&mut self) -> DriverResult<CleanupReport> {
        Generator::close(self)
    }
    fn has_responsibility(&self) -> bool {
        self.has_resource_responsibility()
    }
}
