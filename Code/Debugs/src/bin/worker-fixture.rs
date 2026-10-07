//! Finite pipe peer for supervisor regressions only. No instrument imports.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[path = "../../../Utils/tests/support/osa_wire.rs"]
mod osa_wire;
struct FixtureFactory;
impl yang_worker::session::DriverFactory for FixtureFactory {
    fn create(
        &self,
        config: &yang_protocol::DomainConfig,
    ) -> Result<Box<dyn yang_worker::session::DeviceSession>, yang_worker::WorkerError> {
        use std::sync::Arc;
        use yang_drivers::{
            clock::ManualClock,
            osa::{Osa, OsaOptions, TransferFormat},
        };
        if config.driver_kind != "osa" {
            return Err(yang_worker::WorkerError::new(
                "UnsupportedDriver",
                "Finite fixture contains OSA only",
            ));
        }
        let resource = config.params["resource"]
            .as_str()
            .ok_or_else(|| yang_worker::WorkerError::new("Fixture", "resource"))?;
        let serial = match resource {
            "GPIB0::1::INSTR" => "HOST-OSA-1",
            "GPIB0::2::INSTR" => "HOST-OSA-2",
            "GPIB0::4::INSTR" => "HOST-OSA-4",
            _ => {
                return Err(yang_worker::WorkerError::new(
                    "Fixture",
                    "Unreviewed finite address",
                ))
            }
        };
        let wire = osa_wire::Wire::new(2, TransferFormat::Ascii);
        {
            let mut data = wire.data.lock().unwrap();
            data.metadata
                .insert("*IDN?".into(), format!("YOKOGAWA,AQ6370E,{serial},1.0"));
            data.metadata
                .insert(":TRACe:X? TRA,1,2".into(), "1.55e-6,1.551e-6".into());
            data.metadata
                .insert(":TRACe:Y? TRA,1,2".into(), "-30,-31".into());
        }
        let osa = Osa::with_options(
            wire.manager(),
            resource.into(),
            Arc::new(ManualClock::default()),
            OsaOptions {
                timeout: Duration::from_secs(2),
                close_timeout: Duration::from_secs(2),
            },
        )?;
        Ok(Box::new(yang_worker::session::OsaSession::new(osa)))
    }
}
struct NoInventory;
impl yang_worker::discovery::InventoryPort for NoInventory {
    fn serial(
        &self,
    ) -> Result<
        Vec<yang_drivers::transport::serial_discovery::SerialDeviceInfo>,
        yang_worker::WorkerError,
    > {
        Err(yang_worker::WorkerError::new(
            "Fixture",
            "Enumeration is not in this finite transcript",
        ))
    }
    fn visa(&self) -> Result<Vec<String>, yang_worker::WorkerError> {
        Err(yang_worker::WorkerError::new(
            "Fixture",
            "Enumeration is not in this finite transcript",
        ))
    }
}
fn osa_worker(nonce: String, spool: PathBuf) -> Result<(), String> {
    use std::sync::Arc;
    let clock = Arc::new(yang_drivers::clock::SystemClock::default());
    let backend = yang_worker::backend::NativeBackend::with_ports(
        yang_worker::DomainRegistry::new(&yang_worker::new_id().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?,
        clock.clone(),
        Arc::new(FixtureFactory),
        Arc::new(NoInventory),
    );
    let scheduler = yang_worker::scheduler::Scheduler::new(
        backend.clone(),
        clock,
        yang_protocol::Limits::default(),
    )
    .map_err(|e| e.to_string())?;
    let worker = yang_worker::dispatch::Worker::new(
        backend,
        scheduler,
        yang_worker::captures::CaptureSpool::open(spool, nonce).map_err(|e| e.to_string())?,
    );
    let receipt = worker
        .run_io(std::io::stdin(), std::io::stdout())
        .map_err(|e| e.to_string())?;
    if !receipt.all_resources_released {
        return Err("Finite fixture cleanup remains retained".into());
    }
    Ok(())
}
fn marker(root: &Path, entered: &str, release: &str) {
    std::fs::write(root.join(entered), b"entered").unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !root.join(release).exists() {
        assert!(Instant::now() < deadline, "finite fixture marker timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn send(
    request: &Value,
    result: Value,
    error: Option<Value>,
    context: Option<Value>,
    version: u64,
) {
    let context = context.unwrap_or_else(|| {
        if !request["context"].is_null() {
            request["context"].clone()
        } else if version == 3 {
            json!({"session_id":"a".repeat(32),"domain":null,"connection_id":null,"epoch":0})
        } else {
            json!({"session_id":"pipe-session","connection_id":null,"epoch":0})
        }
    });
    let mut reply = json!({"v":version,"id":request["id"],"ok":error.is_none(),"phase":if error.is_none(){"completed"}else{"failed_after_call_started"},"context":context});
    if let Some(error) = error {
        reply["error"] = error;
    } else {
        reply["result"] = result;
    }
    println!("{}", reply);
    std::io::stdout().flush().unwrap();
}
fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let mut version = 3;
    let mut root = PathBuf::new();
    let mut nonce = None;
    let mut real = false;
    let mut profile = String::new();
    let mut spool = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--real") if !real => real = true,
            Some("--protocol") => {
                version = args
                    .next()
                    .and_then(|s| s.into_string().ok())
                    .and_then(|s| s.parse().ok())
                    .ok_or("protocol")?
            }
            Some("--fixture-root") => root = PathBuf::from(args.next().ok_or("fixture root")?),
            Some("--fixture-profile") => {
                profile = args
                    .next()
                    .and_then(|s| s.into_string().ok())
                    .filter(|s| ["pipe", "osa"].contains(&s.as_str()))
                    .ok_or("profile")?;
            }
            Some("--ownership-nonce") => {
                nonce = Some(
                    args.next()
                        .and_then(|s| s.into_string().ok())
                        .ok_or("nonce")?,
                )
            }
            Some("--capture-spool") => {
                spool = Some(PathBuf::from(args.next().ok_or("spool")?));
            }
            _ => return Err("unknown diagnostic fixture option".into()),
        }
    }
    if !real || ![2, 3].contains(&version) || !root.is_absolute() {
        return Err("explicit diagnostic fixture boundary required".into());
    }
    if profile == "osa" {
        if version != 3 {
            return Err("OSA transcript requires v3".into());
        }
        return osa_worker(nonce.ok_or("nonce")?, spool.ok_or("spool")?);
    }
    let mut options = json!({});
    let mut held: Vec<Value> = vec![];
    let mut held_results: HashMap<String, Value> = HashMap::new();
    let mut attempts = 0;
    let mut activated = false;
    let mut domains = serde_json::Map::new();
    for line in std::io::stdin().lock().lines() {
        let request: Value =
            serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let method = request["method"].as_str().ok_or("method")?;
        let params = &request["params"];
        let name = params["name"].as_str().unwrap_or("");
        let behavior = options["behavior"].as_str().unwrap_or("normal").to_owned();
        let reply = |value| send(&request, value, None, None, version);
        let fail = |kind, message| {
            send(
                &request,
                Value::Null,
                Some(json!({"type":kind,"message":message})),
                None,
                version,
            )
        };
        match method {
            "configure" => {
                for (key, value) in params.as_object().ok_or("configure object")? {
                    options[key] = value.clone();
                }
                if params["release"] == true {
                    for old in held.drain(..).rev() {
                        let result = held_results
                            .remove(old["id"].as_str().unwrap())
                            .unwrap_or_else(|| json!({"resumed":true,"released":true}));
                        send(&old, result, None, None, version);
                    }
                }
                reply(json!({"held":held.len(),"attempts":attempts}));
                if options["behavior"] == "exit_cleanly" {
                    return Ok(());
                }
            }
            "ping" => {
                if root.join("startup_hold").exists() {
                    marker(&root, "startup_entered", "startup_release");
                }
                if root.join("startup_v1").exists() {
                    println!(
                        "{}",
                        json!({"v":1,"id":request["id"],"ok":true,"result":{}})
                    );
                    continue;
                }
                if version == 3 {
                    reply(
                        json!({"startup_revision":1,"worker_kind":"rust","executable":std::env::current_exe().map_err(|e|e.to_string())?,"package_revision":"0.1.0","protocol_version":3,"mode":"real","session_id":"a".repeat(32),"activated":activated,"connected":false,"domains":if activated{Value::Object(domains.clone())}else{json!([])}}),
                    );
                } else {
                    let roles: serde_json::Map<String, Value> = [
                        "osa", "voltage", "gain", "pm400", "fiber",
                    ]
                    .into_iter()
                    .map(|role| {
                        (
                            role.into(),
                            json!({"session_id":"pipe-session","connection_id":null,"epoch":0}),
                        )
                    })
                    .collect();
                    reply(
                        json!({"session_id":"pipe-session","worker_kind":"rust","mode":"real","protocol_version":2,"connected":false,"roles":roles}),
                    );
                }
            }
            "activate" => {
                if activated || params["ownership_nonce"].as_str() != nonce.as_deref() {
                    fail("Ownership", "Activation rejected");
                } else {
                    activated = true;
                    reply(json!({"activated":true}));
                }
            }
            "configure_domain" if version == 3 && activated => {
                let c = &params["config"];
                let context = json!({"session_id":"a".repeat(32),"domain":c["domain"],"connection_id":null,"epoch":0});
                domains.insert(
                    format!(
                        "{}:{}",
                        c["domain"]["kind"].as_str().unwrap(),
                        c["domain"]["id"].as_str().unwrap()
                    ),
                    json!({"context":context}),
                );
                reply(json!({"context":context,"config_rev":c["config_rev"]}));
            }
            "status" if behavior == "status_error" => fail("ReadError", "status unavailable"),
            "status" if behavior == "status_nonobject" => reply(json!("invalid status")),
            "status" => {
                if version == 3 {
                    reply(
                        json!({"session_id":"a".repeat(32),"activated":activated,"connected":false,"domains":domains,"capture_staging_configured":true}),
                    );
                } else {
                    reply(json!({"session_id":"pipe-session","devices":{},"held":held.len()}));
                }
            }
            "shutdown" => {
                attempts += 1;
                if behavior == "shutdown_error_live" {
                    fail("CloseError", "still owned");
                    continue;
                }
                let report = json!({"attempt_id":request["id"],"steps":if behavior == "shutdown_reply_live_marked" {json!([{"role":"pipe_fixture","action":"close","ok":true}])}else{json!([])},"unreleased":if behavior=="unreleased_live"{json!(["pipe_fixture"])}else{json!([])},"voltage_zero":null});
                if options["hold_shutdown"] == true {
                    held_results.insert(request["id"].as_str().unwrap().into(), report);
                    held.push(request.clone());
                    continue;
                }
                reply(report);
                if [
                    "unreleased_live",
                    "shutdown_reply_live",
                    "shutdown_reply_live_marked",
                ]
                .contains(&behavior.as_str())
                {
                    continue;
                }
                if behavior == "delayed_exit" {
                    std::thread::sleep(Duration::from_secs_f64(
                        options["exit_delay"].as_f64().unwrap_or(5.2),
                    ));
                }
                if behavior == "exit_nonzero" {
                    std::process::exit(19);
                }
                return Ok(());
            }
            "action" if name == "no_reply" => (),
            "action" if name == "exit_cleanly" => return Ok(()),
            "action" if name == "exit" => std::process::exit(23),
            "action" if name == "stale" => {
                send(
                    &json!({"id":"prior-id"}),
                    json!("stale"),
                    None,
                    None,
                    version,
                );
                reply(json!("matched"));
            }
            "action" if name == "wrong_version" => println!(
                "{}",
                json!({"v":1,"id":request["id"],"ok":true,"result":{}})
            ),
            "action" if name == "missing_ok" => {
                println!("{}", json!({"v":2,"id":request["id"],"result":{}}))
            }
            "action" if name == "malformed" => println!("not JSON"),
            "resume" if options["hold_resume"] == true => held.push(request.clone()),
            "action"
                if options["hold"]
                    .as_array()
                    .is_some_and(|list| list.contains(&json!(name))) =>
            {
                held.push(request.clone())
            }
            "disconnect" | "action"
                if method == "disconnect"
                    || ["zero", "disable_current", "disable_tec"].contains(&name) =>
            {
                let mut context = request["context"].clone();
                context["epoch"] = json!(context["epoch"].as_u64().unwrap() + 1);
                send(
                    &request,
                    json!({"effective_intent":if method=="disconnect"{"disconnect"}else{name}}),
                    None,
                    Some(context),
                    version,
                );
            }
            "resume" if options["reject_resume"] == true => fail("UnsafeResume", "not authorized"),
            "resume" => reply(json!({"resumed":true})),
            _ => reply(json!({})),
        }
    }
    if root.join("retain_eof").exists() {
        marker(&root, "eof_entered", "release_eof");
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
