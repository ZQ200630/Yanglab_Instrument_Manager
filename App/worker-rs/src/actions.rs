//! Complete typed admission, before entering a driver or loading native transports.
use crate::{pm_ops::PmAction, WorkerError};
use serde_json::Value;
use std::time::Duration;
use yang_drivers::osa::TraceId;
use yang_setups::{StageSide, Vector3Um};
#[derive(Clone, Debug)]
pub enum Action {
    OsaRestage(String),
    Osa {
        acquire: bool,
        trace: TraceId,
    },
    Voltage(VoltageAction),
    Gain(GainAction),
    Pm(PmAction),
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
pub enum VoltageAction {
    Channel(u8, f64),
    All([f64; 8]),
    Zero,
}
#[derive(Clone, Debug)]
pub enum GainAction {
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
        ("osa", "retry_staging") => {
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
        ("osa" | "voltage" | "gain" | "mdt" | "fiber", _) => return Err(unsupported()),
        _ => {
            return Err(WorkerError::new(
                "UnsupportedDriver",
                "driver not in catalog",
            ))
        }
    })
}
