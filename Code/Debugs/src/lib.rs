//! Explicitly staged diagnostics. This crate is never an App backend.
pub mod args;
pub mod enumerate;
pub mod remote;
pub mod laser_scan;
pub use args::{parse_args, DiagnosticAuthorization, DiagnosticPlan, Stage};
use serde::Serialize;
use serde_json::{json, Value};
use sil_instrument_console::host::instance::InstanceGuard;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path},
    sync::{Arc, Mutex, OnceLock},
};
use yang_worker::session::{DeviceSession, DriverFactory, SystemFactory};
pub type DiagnosticError = String;
#[derive(Serialize)]
pub struct DiagnosticReport {
    schema: u32,
    source_kind: &'static str,
    stage: Stage,
    output_changes_authorized: bool,
    binding: Value,
    results: Vec<Value>,
    error: Option<String>,
    cleanup: Vec<Value>,
    resources_released: bool,
}
impl DiagnosticReport {
    pub fn resources_released(&self) -> bool {
        self.resources_released
    }
    pub fn success(&self) -> bool {
        self.error.is_none() && self.resources_released
    }
}
pub trait DiagnosticEnvironment {
    fn acquire_owner(&self) -> Result<Box<dyn Send>, String>;
    fn enumerate(&self) -> Result<Value, String>;
}
struct SystemEnvironment;
impl DiagnosticEnvironment for SystemEnvironment {
    fn acquire_owner(&self) -> Result<Box<dyn Send>, String> {
        InstanceGuard::acquire_diagnostic()
            .map(|g| Box::new(g) as Box<dyn Send>)
            .map_err(|e| e.to_string())
    }
    fn enumerate(&self) -> Result<Value, String> {
        enumerate::inventory()
    }
}
struct Pending {
    _owner: Box<dyn Send>,
    factory: Arc<dyn DriverFactory>,
    session: Option<Box<dyn DeviceSession>>,
}
fn pending() -> &'static Mutex<Vec<Pending>> {
    static P: OnceLock<Mutex<Vec<Pending>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(vec![]))
}
/// Never force-exits a retained driver. Each retry makes a new cleanup attempt.
pub fn retry_pending() -> usize {
    let mut all = pending().lock().unwrap();
    all.retain_mut(|p| {
        let released = p.session.as_mut().map_or(true, |s| {
            s.close().is_ok_and(|r| r.resources_released()) && !s.has_responsibility()
        });
        if released {
            p.session = None;
        }
        !(released
            && p.factory
                .finish_shutdown()
                .is_ok_and(|r| r.resources_released()))
    });
    all.len()
}
pub fn execute(
    plan: DiagnosticPlan,
    authorization: DiagnosticAuthorization,
) -> Result<DiagnosticReport, DiagnosticError> {
    execute_with(
        plan,
        authorization,
        Arc::new(SystemFactory::new(Arc::new(
            yang_drivers::clock::SystemClock::default(),
        ))),
        &SystemEnvironment,
    )
}
pub fn execute_with(
    plan: DiagnosticPlan,
    authorization: DiagnosticAuthorization,
    factory: Arc<dyn DriverFactory>,
    env: &dyn DiagnosticEnvironment,
) -> Result<DiagnosticReport, DiagnosticError> {
    authorization.check(&plan)?;
    // The process-wide Host guard precedes output and instrument session creation.
    // Remote observers do not own this machine's instrument resources.
    let mut owner = if plan.command == "remote" {
        None
    } else {
        Some(env.acquire_owner()?)
    };
    let output = plan
        .output
        .as_ref()
        .ok_or("Explicit Result/<name> output is required")?;
    validate_output(output)?;
    fs::create_dir(output).map_err(|e| format!("Cannot reserve new diagnostic directory: {e}"))?;
    let mut report = DiagnosticReport {
        schema: 1,
        source_kind: "real",
        stage: plan.stage,
        output_changes_authorized: plan.stage == Stage::Action,
        binding: plan.public_binding(),
        results: vec![],
        error: None,
        cleanup: vec![],
        resources_released: true,
    };
    let mut session = None;
    let result: Result<(), String> = (|| {
        if plan.command == "enumerate" {
            report.results.push(env.enumerate()?);
            return Ok(());
        }
        if plan.command == "remote" {
            let result = remote::run(&plan, output)?;
            report.resources_released = result.release_confirmed;
            report.cleanup.push(result.release_receipt);
            report.results.push(result.observation);
            if let Some(error) = result.error {
                return Err(error);
            }
            return Ok(());
        }
        let config = plan.config.as_ref().ok_or("Missing binding")?;
        session = Some(factory.create(config).map_err(|e| e.to_string())?);
        let s = session.as_mut().unwrap();
        // This probe never runs normal Gain/Voltage startup or shutdown writes.
        let proof = s.probe_readonly().map_err(|e| e.to_string())?;
        report.results.push(json!({"probe":proof}));
        identity_matches(&config.expected_identity, proof.identity())?;
        if plan.stage == Stage::Readonly {
            return Ok(());
        }
        if !proof.release_confirmed() && matches!(config.driver_kind.as_str(), "gain" | "voltage") {
            return Err("Readonly probe did not confirm release; startup refused".into());
        }
        let close = s.close().map_err(|e| e.to_string())?;
        if !close.resources_released() || s.has_responsibility() {
            return Err("Probe release remains pending".into());
        }
        report.cleanup.push(json!(close));
        *s = factory.create(config).map_err(|e| e.to_string())?;
        let connected = s.connect().map_err(|e| e.to_string())?;
        identity_matches(&config.expected_identity, connected.identity())?;
        let context = yang_protocol::ContextV3 {
            session_id: yang_worker::new_id().map_err(|e| e.to_string())?,
            domain: Some(config.domain.clone()),
            connection_id: Some(yang_worker::new_id().map_err(|e| e.to_string())?),
            epoch: 1,
        };
        for (index, action) in plan.actions.iter().enumerate() {
            let outcome = s.action(&action.name, &action.args, &context);
            report.results.push(json!({"phase":outcome.phase,"context":outcome.context,"result":outcome.result,"error":outcome.error}));
            if outcome.phase != yang_protocol::Phase::Completed {
                return Err(format!(
                    "Diagnostic action {index} failed; no automatic replay"
                ));
            }
            if let Some(capture) = s.take_capture() {
                save_capture(output, index, &capture)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        report.error = Some(error);
    }
    if let Some(s) = session.as_mut() {
        match s.close() {
            Ok(r) => {
                report.resources_released &= r.resources_released() && !s.has_responsibility();
                report.cleanup.push(json!(r));
            }
            Err(e) => {
                report.resources_released = false;
                report
                    .cleanup
                    .push(json!({"error":e.to_string(),"resources_released":false}));
            }
        }
    }
    if plan.command != "remote" {
        match factory.finish_shutdown() {
            Ok(r) => {
                report.resources_released &= r.resources_released();
                report.cleanup.push(json!(r));
            }
            Err(e) => {
                report.resources_released = false;
                report
                    .cleanup
                    .push(json!({"error":e.to_string(),"resources_released":false}));
            }
        }
        if !report.resources_released {
            pending().lock().unwrap().push(Pending {
                _owner: owner.take().unwrap(),
                factory,
                session,
            });
        }
    }
    write_new(
        &output.join("report.json"),
        &serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )?;
    Ok(report)
}
fn identity_matches(expected: &Value, actual: &Value) -> Result<(), String> {
    if expected
        .as_object()
        .ok_or("Expected identity must be an object")?
        .iter()
        .any(|(k, v)| actual.get(k) != Some(v))
    {
        Err("Instrument identity mismatch; no action authorized".into())
    } else {
        Ok(())
    }
}
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())
}
pub(crate) fn validate_output(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            != Some("Result")
        || !short_name(path.file_name().and_then(|s| s.to_str()).unwrap_or(""))
    {
        return Err("Use an absolute, new Result/<short-name> directory".into());
    }
    let mut ancestor = path.parent();
    while let Some(p) = ancestor {
        let m = fs::symlink_metadata(p).map_err(|e| e.to_string())?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err("Output ancestors must be ordinary directories".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return Err("Output reparse points are not allowed".into());
            }
        }
        ancestor = p.parent();
    }
    Ok(())
}
fn short_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        && ![
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(&s.to_ascii_uppercase().as_str())
}
fn save_capture(
    output: &Path,
    index: usize,
    c: &yang_drivers::osa::TraceCapture,
) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(c.native_values().len() * 16);
    for (&x, &y) in c.wavelength_nm().iter().zip(c.native_values()) {
        bytes.extend(x.to_le_bytes());
        bytes.extend(y.to_le_bytes());
    }
    write_new(&output.join(format!("trace-{index}.bin")), &bytes)?;
    let hash: String = ring::digest::digest(&ring::digest::SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let descriptor = json!({"schema":1,"encoding":"f64le_xy","sample_count":c.native_values().len(),"native_unit":c.native_unit(),"trace":c.trace(),"identity":c.identity(),"context_before":c.context_before(),"context_after":c.context_after(),"consistency":c.consistency(),"sha256":hash});
    write_new(
        &output.join(format!("trace-{index}.json")),
        &serde_json::to_vec_pretty(&descriptor).map_err(|e| e.to_string())?,
    )
}
