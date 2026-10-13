//! Complete typed admission, before entering a driver or loading native transports.
use crate::{pm_ops::PmAction, WorkerError};
use serde_json::Value;
use std::time::Duration;
use yang_drivers::osa::TraceId;
use yang_setups::{StageSide, Vector3Um};
#[derive(Clone, Debug)]
pub enum Action {
    Scope(ScopeAction),
    Generator(GeneratorAction),
    OsaRestage(String),
    Osa {
        acquire: bool,
        trace: TraceId,
    },
    Voltage(VoltageAction),
    Gain(GainAction),
    Pm(PmAction),
    Laser(LaserAction),
    MdtStatus,
    FiberAdopt {
        side: StageSide,
        allow_nominal: bool,
    },
    FiberMove {
        side: StageSide,
        delta: Vector3Um,
    },
}
#[derive(Clone, Debug)]
pub enum ScopeAction {
    Status,
    Display { channel: u8, enabled: bool },
    Depth(u64),
    Run,
    Stop,
    Capture { request: yang_drivers::scope::CaptureRequest, fresh: bool, stop_before: bool },
}
#[derive(Clone, Debug)]
pub enum GeneratorAction {
    Status,
    Configure { channel: u8, signal: yang_drivers::generator::SignalConfig },
    Output { channel: u8, enabled: bool },
    Phase { channel: u8, degrees: f64 },
    Load { channel: u8, load: yang_drivers::generator::OutputLoad },
    Polarity { channel: u8, polarity: yang_drivers::generator::OutputPolarity },
}
fn channel(args: &Value, maximum: u64) -> Result<u8, WorkerError> {
    args["channel"].as_u64().filter(|n|(1..=maximum).contains(n)).map(|n|n as u8).ok_or_else(||invalid("integer channel outside model range"))
}
pub fn scope_capture_bytes(model: &str, request: &yang_drivers::scope::CaptureRequest) -> Result<u64, WorkerError> {
    let width = match model { "ds7054"=>1_u64,"mho5054"=>2,_=>return Err(invalid("unreviewed scope model")) };
    let bytes = request.points.or_else(||matches!(request.source,yang_drivers::scope::CaptureSource::Screen).then_some(1000))
        .map(|points|points.checked_mul(request.channels.len() as u64).and_then(|n|n.checked_mul(width)).ok_or_else(||invalid("capture byte overflow")))
        .transpose()?.unwrap_or(request.byte_budget as u64);
    if bytes==0 || bytes>request.byte_budget as u64 { return Err(invalid("requested capture exceeds admitted byte budget")); }
    Ok(bytes)
}
#[derive(Clone, Debug)]
pub enum LaserAction {
    Status,
    Motion,
    Legacy(yang_lab_tlb::Action),
    Control(yang_lab_tlb::Control),
    Target(f64),
    Goto(f64),
}
#[derive(Clone, Debug)]
pub enum VoltageAction {
    Channel(u8, f64),
    All([f64; 8]),
    Zero,
}
#[derive(Clone, Debug)]
pub enum GainAction {
    ReadPid,
    SetPid([f64; 3]),
    Ramp { current: f64, step: f64, interval: Duration },
    Start { current: f64, soft_start: bool, step: f64, interval: Duration, timeout: Duration },
    Temperature(f64),
    Current(f64),
    EnableTec,
    DisableTec,
    Stable(Duration),
    EnableCurrent,
    DisableCurrent,
}
pub(crate) fn invalid(message: &str) -> WorkerError {
    WorkerError::new("InvalidArguments", message)
}
pub(crate) fn fields(args: &Value, allowed: &[&str], required: &[&str]) -> Result<(), WorkerError> {
    let object = args
        .as_object()
        .ok_or_else(|| invalid("typed object required"))?;
    if object.keys().any(|k| !allowed.contains(&k.as_str()))
        || required.iter().any(|k| !object.contains_key(*k))
    {
        return Err(invalid("missing or unexpected argument"));
    }
    Ok(())
}
pub(crate) fn number(args: &Value, key: &str, min: f64, max: f64) -> Result<f64, WorkerError> {
    args[key]
        .as_f64()
        .filter(|v| v.is_finite() && (min..=max).contains(v))
        .ok_or_else(|| invalid("finite number outside permitted bounds"))
}
pub(crate) fn flag(args: &Value, key: &str) -> Result<bool, WorkerError> {
    args[key]
        .as_bool()
        .ok_or_else(|| invalid("boolean required"))
}
fn side(args: &Value) -> Result<StageSide, WorkerError> {
    match args["side"].as_str() {
        Some("left") => Ok(StageSide::Left),
        Some("right") => Ok(StageSide::Right),
        _ => Err(invalid("registered stage side required")),
    }
}
pub fn parse(kind: &str, name: &str, args: &Value) -> Result<Action, WorkerError> {
    let unsupported = || {
        WorkerError::new(
            "UnsupportedAction",
            "action not exposed for this instrument",
        )
    };
    Ok(match (kind, name) {
        ("scope", "read_status" | "run" | "stop") => {
            fields(args,&[],&[])?;
            Action::Scope(match name {"read_status"=>ScopeAction::Status,"run"=>ScopeAction::Run,_=>ScopeAction::Stop})
        }
        ("scope", "set_channel_display") => {
            fields(args,&["channel","enabled"],&["channel","enabled"])?;
            Action::Scope(ScopeAction::Display{channel:channel(args,4)?,enabled:flag(args,"enabled")?})
        }
        ("scope", "set_memory_depth") => {
            fields(args,&["points"],&["points"])?;
            let points=args["points"].as_u64().filter(|n|(1..=250_000_000).contains(n)).ok_or_else(||invalid("memory depth must be an integer 1..250M"))?;
            Action::Scope(ScopeAction::Depth(points))
        }
        ("scope", "capture") => {
            use yang_drivers::scope::{CaptureRequest,CaptureSource};
            fields(args,&["channels","source","points","byte_budget","fresh","stop_before"],&["channels","source"])?;
            let channels=args["channels"].as_array().filter(|a|!a.is_empty()&&a.len()<=4).ok_or_else(||invalid("one to four channels required"))?
                .iter().map(|v|v.as_u64().filter(|n|(1..=4).contains(n)).map(|n|n as u8).ok_or_else(||invalid("channel outside 1..4"))).collect::<Result<Vec<_>,_>>()?;
            if channels.iter().collect::<std::collections::BTreeSet<_>>().len()!=channels.len() {return Err(invalid("duplicate capture channel"));}
            let source=match args["source"].as_str(){Some("screen")=>CaptureSource::Screen,Some("memory")=>CaptureSource::Memory,_=>return Err(invalid("screen or memory source required"))};
            let maximum=if source==CaptureSource::Screen {1000}else{250_000_000};
            let points=args.get("points").map(|v|v.as_u64().filter(|n|(1..=maximum).contains(n)).ok_or_else(||invalid("capture points outside source bounds"))).transpose()?;
            let byte_budget=args.get("byte_budget").map(|v|v.as_u64().filter(|n|(1..=yang_protocol::MAX_WAVEFORM_BYTES).contains(n)).map(|n|n as usize).ok_or_else(||invalid("capture byte budget outside bounds"))).transpose()?.unwrap_or(yang_protocol::MAX_WAVEFORM_BYTES as usize);
            let fresh=args.get("fresh").map(|_|flag(args,"fresh")).transpose()?.unwrap_or(false);
            let stop_before=args.get("stop_before").map(|_|flag(args,"stop_before")).transpose()?.unwrap_or(false);
            if fresh && stop_before {return Err(invalid("fresh and stop_before are mutually exclusive"));}
            Action::Scope(ScopeAction::Capture{request:CaptureRequest{channels,source,points,byte_budget},fresh,stop_before})
        }
        ("generator", "read_status") => {fields(args,&[],&[])?;Action::Generator(GeneratorAction::Status)}
        ("generator", "configure_signal") => {
            use yang_drivers::generator::{SignalConfig,Waveform};
            fields(args,&["channel","waveform","frequency_hz","amplitude_vpp","offset_v"],&["channel","waveform","frequency_hz","amplitude_vpp","offset_v"])?;
            let waveform=serde_json::from_value::<Waveform>(args["waveform"].clone()).map_err(|_|invalid("unsupported signal waveform"))?;
            let amplitude_vpp=number(args,"amplitude_vpp",0.,20.)?;let offset_v=number(args,"offset_v",-10.,10.)?;
            if amplitude_vpp+2.*offset_v.abs()>20. {return Err(invalid("combined Vpp/offset exceeds high-Z envelope"));}
            Action::Generator(GeneratorAction::Configure{channel:channel(args,2)?,signal:SignalConfig{waveform,frequency_hz:number(args,"frequency_hz",0.000001,200_000_000.)?,amplitude_vpp,offset_v}})
        }
        ("generator", "set_output") => {fields(args,&["channel","enabled"],&["channel","enabled"])?;Action::Generator(GeneratorAction::Output{channel:channel(args,2)?,enabled:flag(args,"enabled")?})}
        ("generator", "set_phase") => {fields(args,&["channel","degrees"],&["channel","degrees"])?;Action::Generator(GeneratorAction::Phase{channel:channel(args,2)?,degrees:number(args,"degrees",0.,360.)?})}
        ("generator", "set_load") => {
            use yang_drivers::generator::OutputLoad;
            fields(args,&["channel","load"],&["channel","load"])?;
            let load=&args["load"];
            let load=match load["kind"].as_str(){Some("high_z")=>{fields(load,&["kind"],&["kind"])?;OutputLoad::HighZ},Some("ohms")=>{fields(load,&["kind","ohms"],&["kind","ohms"])?;OutputLoad::Ohms{ohms:number(load,"ohms",1.,10000.)?}},_=>return Err(invalid("reviewed load required"))};
            Action::Generator(GeneratorAction::Load{channel:channel(args,2)?,load})
        }
        ("generator", "set_polarity") => {
            fields(args,&["channel","polarity"],&["channel","polarity"])?;
            let polarity=serde_json::from_value(args["polarity"].clone()).map_err(|_|invalid("normal or inverted polarity required"))?;
            Action::Generator(GeneratorAction::Polarity{channel:channel(args,2)?,polarity})
        }
        ("laser", "read_status" | "read_motion") => {
            fields(args, &[], &[])?;
            Action::Laser(if name == "read_status" {
                LaserAction::Status
            } else {
                LaserAction::Motion
            })
        }
        ("laser", "start_scan") => {
            fields(
                args,
                &["start_nm", "stop_nm", "speed_nm_s", "return_speed_nm_s", "confirm"],
                &["start_nm", "stop_nm", "speed_nm_s", "confirm"],
            )?;
            if !flag(args, "confirm")? {
                return Err(invalid("explicit confirmation required"));
            }
            Action::Laser(LaserAction::Control(yang_lab_tlb::Control::ScanStart(
                yang_lab_tlb::ScanPlan {
                    start_nm: number(args, "start_nm", 1., 5000.)?,
                    stop_nm: number(args, "stop_nm", 1., 5000.)?,
                    speed_nm_s: number(args, "speed_nm_s", 0.01, 20.)?,
                    return_speed_nm_s: args.get("return_speed_nm_s")
                        .map(|_| number(args, "return_speed_nm_s", 0.01, 20.))
                        .transpose()?,
                },
            )))
        }
        ("laser", "scan_forward" | "scan_backward") => {
            fields(args, &["target_nm", "speed_nm_s", "confirm"], &["target_nm", "speed_nm_s", "confirm"])?;
            if !flag(args, "confirm")? { return Err(invalid("explicit confirmation required")); }
            Action::Laser(LaserAction::Control(yang_lab_tlb::Control::ScanTo(yang_lab_tlb::SingleScanPlan {
                target_nm: number(args, "target_nm", 1., 5000.)?,
                speed_nm_s: number(args, "speed_nm_s", 0.01, 20.)?,
            })))
        }
        ("laser", "stop_scan") => {
            fields(args, &["confirm"], &["confirm"])?;
            if !flag(args, "confirm")? {
                return Err(invalid("explicit confirmation required"));
            }
            Action::Laser(LaserAction::Control(yang_lab_tlb::Control::ScanStop))
        }
        ("laser", "set_remote" | "set_output" | "set_tracking" | "set_wavelength" | "set_piezo" |
            "move_wavelength" | "goto_wavelength" | "set_target_wavelength" | "control_piezo" | "control_tracking" | "control_output") => {
            let field = match name {
                "set_remote" => "remote",
                "set_output" | "set_tracking" | "control_output" | "control_tracking" => "enabled",
                "set_piezo" | "control_piezo" => "percent",
                _ => "wavelength_nm",
            };
            fields(args, &[field, "confirm"], &[field, "confirm"])?;
            if !flag(args, "confirm")? {
                return Err(invalid("explicit confirmation required"));
            }
            use yang_lab_tlb::{Action as A, Control as C};
            Action::Laser(match name {
                "set_remote" => LaserAction::Legacy(A::Remote(flag(args, field)?)),
                "set_output" => LaserAction::Legacy(A::Output(flag(args, field)?)),
                "set_tracking" => LaserAction::Legacy(A::Tracking(flag(args, field)?)),
                "set_wavelength" => LaserAction::Legacy(A::Wavelength(number(args, field, 1., 5000.)?)),
                "set_piezo" => LaserAction::Legacy(A::Piezo(number(args, field, 0., 100.)?)),
                "control_output" => LaserAction::Control(C::Output(flag(args, field)?)),
                "control_tracking" => LaserAction::Control(C::Tracking(flag(args, field)?)),
                "control_piezo" => LaserAction::Control(C::Piezo(number(args, field, 0., 100.)?)),
                "move_wavelength" => LaserAction::Control(C::Wavelength(number(args, field, 1., 5000.)?)),
                "goto_wavelength" => LaserAction::Goto(number(args, field, 1., 5000.)?),
                _ => LaserAction::Target(number(args, field, 1., 5000.)?),
            })
        }
        ("osa" | "scope", "retry_staging") => {
            fields(args, &["capture_id"], &["capture_id"])?;
            let id = args["capture_id"]
                .as_str()
                .filter(|s| yang_protocol::valid_id(s))
                .ok_or_else(|| invalid("original recovery ticket required"))?;
            Action::OsaRestage(id.into())
        }
        ("osa", "read_trace" | "acquire") => {
            fields(args, &["trace"], &[])?;
            let trace = match args.get("trace") {
                None => TraceId::A,
                Some(v) => v
                    .as_str()
                    .ok_or_else(|| invalid("trace identifier required"))?
                    .parse()
                    .map_err(|_| invalid("invalid trace identifier"))?,
            };
            Action::Osa {
                acquire: name == "acquire",
                trace,
            }
        }
        ("voltage", "set_channel") => {
            fields(args, &["channel", "voltage"], &["channel", "voltage"])?;
            let channel = args["channel"]
                .as_u64()
                .filter(|v| (1..=8).contains(v))
                .ok_or_else(|| invalid("channel must be an integer 1..8"))?;
            Action::Voltage(VoltageAction::Channel(
                channel as u8,
                number(args, "voltage", 0., 14.)?,
            ))
        }
        ("voltage", "set_all") => {
            fields(args, &["values"], &["values"])?;
            let values = args["values"]
                .as_array()
                .filter(|a| a.len() == 8)
                .ok_or_else(|| invalid("exactly eight values required"))?;
            let mut typed = [0.; 8];
            for (i, v) in values.iter().enumerate() {
                typed[i] = v
                    .as_f64()
                    .filter(|v| v.is_finite() && (0. ..=14.).contains(v))
                    .ok_or_else(|| invalid("voltage outside 0..14 V"))?;
            }
            Action::Voltage(VoltageAction::All(typed))
        }
        ("voltage", "zero") => {
            fields(args, &[], &[])?;
            Action::Voltage(VoltageAction::Zero)
        }
        ("gain", "read_pid") => {
            fields(args, &[], &[])?;
            Action::Gain(GainAction::ReadPid)
        }
        ("gain", "set_pid") => {
            fields(args, &["p", "i", "d"], &["p", "i", "d"])?;
            Action::Gain(GainAction::SetPid([number(args,"p",0.,999.999)?,number(args,"i",0.,999.999)?,number(args,"d",0.,999.999)?]))
        }
        ("gain", "ramp_current") => {
            fields(args, &["current_ma", "step_ma", "interval_s"], &["current_ma", "step_ma", "interval_s"])?;
            Action::Gain(GainAction::Ramp { current:number(args,"current_ma",0.,200.)?,step:number(args,"step_ma",0.001,1.)?,interval:Duration::from_secs_f64(number(args,"interval_s",0.05,180.)?) })
        }
        ("gain", "start_current") => {
            fields(args, &["current_ma", "soft_start", "step_ma", "interval_s", "timeout_s"], &["current_ma"])?;
            let optional = |key,min,max,default| if args.get(key).is_none() { Ok(default) } else { number(args,key,min,max) };
            Action::Gain(GainAction::Start {
                current:number(args,"current_ma",0.,200.)?,
                soft_start:if args.get("soft_start").is_none() { true } else { args["soft_start"].as_bool().ok_or_else(||invalid("soft_start must be boolean"))? },
                step:optional("step_ma",0.001,1.,1.)?,
                interval:Duration::from_secs_f64(optional("interval_s",0.05,180.,0.1)?),
                timeout:Duration::from_secs_f64(optional("timeout_s",0.05,180.,30.)?),
            })
        }
        ("gain", "set_temperature") => {
            fields(args, &["temperature_c"], &["temperature_c"])?;
            Action::Gain(GainAction::Temperature(number(
                args,
                "temperature_c",
                15.,
                40.,
            )?))
        }
        ("gain", "set_current") => {
            fields(args, &["current_ma"], &["current_ma"])?;
            Action::Gain(GainAction::Current(number(args, "current_ma", 0., 200.)?))
        }
        ("gain", "wait_stable") => {
            fields(args, &["timeout_s"], &[])?;
            Action::Gain(GainAction::Stable(Duration::from_secs_f64(
                if args.get("timeout_s").is_none() {
                    30.
                } else {
                    number(args, "timeout_s", 0.05, 180.)?
                },
            )))
        }
        ("gain", "enable_tec" | "disable_tec" | "enable_current" | "disable_current") => {
            fields(args, &[], &[])?;
            Action::Gain(match name {
                "enable_tec" => GainAction::EnableTec,
                "disable_tec" => GainAction::DisableTec,
                "enable_current" => GainAction::EnableCurrent,
                _ => GainAction::DisableCurrent,
            })
        }
        ("pm400", _) => Action::Pm(crate::pm_ops::parse(name, args)?),
        ("mdt", "read_status") => {
            fields(args, &[], &[])?;
            Action::MdtStatus
        }
        ("fiber", "adopt_baseline") => {
            fields(
                args,
                &["side", "confirm", "allow_nominal"],
                &["side", "confirm"],
            )?;
            if !flag(args, "confirm")? {
                return Err(invalid("baseline adoption requires confirm=true"));
            }
            Action::FiberAdopt {
                side: side(args)?,
                allow_nominal: if args.get("allow_nominal").is_some() {
                    flag(args, "allow_nominal")?
                } else {
                    false
                },
            }
        }
        ("fiber", "move") => {
            fields(args, &["side", "dx", "dy", "dz"], &["side"])?;
            let side = side(args)?;
            let read = |key| {
                if args.get(key).is_none() {
                    Ok(0.)
                } else {
                    number(args, key, -1., 1.)
                }
            };
            let delta = Vector3Um::new(read("dx")?, read("dy")?, read("dz")?)?;
            if delta.x * f64::from(side.toward_chip_sign()) > 0.2 {
                return Err(invalid("toward-chip move exceeds 0.2 um"));
            }
            Action::FiberMove { side, delta }
        }
        ("osa" | "scope" | "generator" | "voltage" | "gain" | "mdt" | "fiber" | "laser", _) => return Err(unsupported()),
        _ => {
            return Err(WorkerError::new(
                "UnsupportedDriver",
                "driver not in catalog",
            ))
        }
    })
}
