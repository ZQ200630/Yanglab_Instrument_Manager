use crate::DiagnosticError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Enumerate,
    Readonly,
    Action,
}
impl Stage {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "enumerate" => Ok(Self::Enumerate),
            "readonly" => Ok(Self::Readonly),
            "action" => Ok(Self::Action),
            _ => Err("Stage must be enumerate, readonly or action".into()),
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticAction {
    pub name: String,
    pub args: Value,
}
pub struct DiagnosticPlan {
    pub(crate) command: String,
    pub(crate) stage: Stage,
    pub(crate) output: Option<PathBuf>,
    pub(crate) config: Option<yang_protocol::DomainConfig>,
    pub(crate) actions: Vec<DiagnosticAction>,
    pub(crate) remote: Value,
    execute: bool,
    confirm: Option<Stage>,
    acknowledge: bool,
}
impl DiagnosticPlan {
    pub fn execute_requested(&self) -> bool {
        self.execute
    }
    pub fn preview(&self) -> Value {
        json!({"command":self.command,"stage":self.stage,"binding":self.public_binding(),"actions":self.actions,"output":self.output,"output_changes":self.stage==Stage::Action,"normal_connect_effects":match self.command.as_str(){"gain"=>"disable current before TEC on startup/shutdown", "voltage"=>"zero all eight outputs on startup/shutdown", _=>"front-panel state preserved; explicitly selected actions only"}})
    }
    pub(crate) fn public_binding(&self) -> Value {
        self.config
            .as_ref()
            .map_or_else(|| self.remote.clone(), |c| json!(c))
    }
}
pub struct DiagnosticAuthorization {
    stage: Stage,
    binding: Value,
    actions: Value,
    confirmed: bool,
    acknowledge: bool,
}
impl DiagnosticAuthorization {
    pub fn from_plan(p: &DiagnosticPlan) -> Result<Self, String> {
        if !p.execute || p.confirm != Some(p.stage) {
            return Err("Explicit --execute --confirm-stage <stage> required".into());
        }
        Ok(Self {
            stage: p.stage,
            binding: p.public_binding(),
            actions: json!(p.actions),
            confirmed: true,
            acknowledge: p.acknowledge,
        })
    }
    pub(crate) fn check(&self, p: &DiagnosticPlan) -> Result<(), String> {
        if !self.confirmed
            || self.stage != p.stage
            || self.binding != p.public_binding()
            || self.actions != json!(p.actions)
            || (p.stage == Stage::Action
                && matches!(p.command.as_str(), "gain" | "voltage")
                && !self.acknowledge)
        {
            Err("Authorization does not cover this stage/binding/startup effects".into())
        } else {
            Ok(())
        }
    }
}
pub fn parse_args(args: &[String]) -> Result<DiagnosticPlan, DiagnosticError> {
    if args.is_empty() || args == ["--help"] {
        return Ok(DiagnosticPlan {
            command: "help".into(),
            stage: Stage::Enumerate,
            output: None,
            config: None,
            actions: vec![],
            remote: json!({}),
            execute: false,
            confirm: None,
            acknowledge: false,
        });
    }
    let command = args[0].as_str();
    if ![
        "enumerate",
        "osa",
        "voltage",
        "gain",
        "pm400",
        "mdt",
        "fiber",
        "remote",
    ]
    .contains(&command)
    {
        return Err("Unknown diagnostic command".into());
    }
    let mut fields = BTreeMap::new();
    let mut execute = false;
    let mut acknowledge = false;
    let mut i = 1;
    while i < args.len() {
        let key = args[i].as_str();
        if key == "--execute" {
            if execute {
                return Err("Duplicate execute flag".into());
            }
            execute = true;
            i += 1;
            continue;
        }
        if key == "--acknowledge-lifecycle" {
            if acknowledge {
                return Err("Duplicate acknowledgement".into());
            }
            acknowledge = true;
            i += 1;
            continue;
        }
        if ![
            "--stage",
            "--out",
            "--binding",
            "--actions",
            "--confirm-stage",
            "--peers",
            "--host",
            "--archive",
        ]
        .contains(&key)
            || i + 1 >= args.len()
            || args[i + 1].len() > 65536
            || fields.insert(key, args[i + 1].as_str()).is_some()
        {
            return Err("Unknown, duplicate or missing diagnostic argument".into());
        }
        i += 2;
    }
    let stage = Stage::parse(fields.get("--stage").ok_or("Explicit --stage required")?)?;
    let output = PathBuf::from(fields.get("--out").ok_or("Explicit --out required")?);
    if !output.is_absolute()
        || output
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            != Some("Result")
    {
        return Err("Use absolute Result/<name> output".into());
    }
    let confirm = fields
        .get("--confirm-stage")
        .map(|s| Stage::parse(s))
        .transpose()?;
    if execute != confirm.is_some() || confirm.is_some_and(|c| c != stage) {
        return Err("Confirm exactly this explicit stage".into());
    }
    let config = if command == "enumerate" || command == "remote" {
        None
    } else {
        let v = yang_protocol::strict_json(
            fields
                .get("--binding")
                .ok_or("Explicit typed --binding JSON required")?
                .as_bytes(),
            65536,
        )
        .map_err(|e| e.to_string())?;
        let config: yang_protocol::DomainConfig =
            serde_json::from_value(v).map_err(|e| e.to_string())?;
        if config.driver_kind != command {
            return Err("Command and typed binding differ".into());
        }
        Some(yang_worker::catalog::admit(&config).map_err(|e| e.to_string())?)
    };
    let actions = if let Some(value) = fields.get("--actions") {
        let actions: Vec<DiagnosticAction> = serde_json::from_value(
            yang_protocol::strict_json(value.as_bytes(), 65536).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if stage != Stage::Action || actions.is_empty() || actions.len() > 16 {
            return Err("1-16 actions only in separately confirmed action stage".into());
        }
        for a in &actions {
            yang_worker::actions::parse(command, &a.name, &a.args).map_err(|e| e.to_string())?;
        }
        actions
    } else {
        vec![]
    };
    if stage == Stage::Action
        && (config.is_none()
            || actions.is_empty()
            || (matches!(command, "gain" | "voltage") && !acknowledge))
    {
        return Err(
            "Action requires typed actions and lifecycle acknowledgement for Gain/Voltage".into(),
        );
    }
    if command == "enumerate" && (stage != Stage::Enumerate || fields.contains_key("--binding"))
        || (command != "enumerate" && stage == Stage::Enumerate)
        || (acknowledge && (stage != Stage::Action || !matches!(command, "gain" | "voltage")))
    {
        return Err("Arguments do not apply to this diagnostic stage".into());
    }
    let remote = if command == "remote" {
        let peers = fields
            .get("--peers")
            .ok_or("Existing --peers DPAPI file required")?;
        let host = fields.get("--host").ok_or("Explicit --host id required")?;
        if !PathBuf::from(peers).is_absolute()
            || !yang_protocol::valid_id(host)
            || fields.contains_key("--binding")
        {
            return Err("Invalid saved remote peer selection".into());
        }
        if let Some(a) = fields.get("--archive") {
            if !PathBuf::from(a).is_absolute() {
                return Err("Archive reference path must be absolute".into());
            }
        }
        json!({"peers":peers,"host_id":host,"archive":fields.get("--archive")})
    } else {
        if ["--peers", "--host", "--archive"]
            .iter()
            .any(|k| fields.contains_key(k))
        {
            return Err("Remote arguments require remote command".into());
        }
        json!({})
    };
    Ok(DiagnosticPlan {
        command: command.into(),
        stage,
        output: Some(output),
        config,
        actions,
        remote,
        execute,
        confirm,
        acknowledge,
    })
}
