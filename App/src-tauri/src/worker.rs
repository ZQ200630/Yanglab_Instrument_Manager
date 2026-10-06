//! One Rust-owned Python process protects process-local VISA leases and serial ownership.
//! Never kill a child after it may have opened hardware: EOF/shutdown lets its `finally`
//! cleanup run, and a lost response is reported as an unknown outcome.

use serde::Deserialize;
use serde_json::{json, Value};
#[cfg(test)]
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::Manager;

use crate::startup_handshake::validate_startup_handshake;
use crate::worker_root::{resolve_worker_root, source_fallback_permitted};

use crate::runtime::*;
#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::request_writer::DispatchClass;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    #[test]
    fn command_admission_bounds_wait_work_and_preserves_query_and_safety_reserves() {
        let f = Fixture::new();
        f.worker.ready.store(true, Ordering::Release);
        let state = WorkerState(
            Mutex::new(Some(f.worker.clone())),
            Arc::new(AtomicBool::new(false)),
        );
        let mut admitted = Vec::new();
        for n in 0..31 {
            let frame = request(&format!("normal-{n}"), "action", Some("osa"), "set", 0);
            admitted.push((frame.clone(), f.worker.admit(&frame).ok().unwrap()));
        }
        // Replies may already be terminal, but their reserved waiting work is not retired.
        assert_eq!(f.worker.waits.lock().unwrap().len(), 31);
        let runtime = tauri::async_runtime::TokioRuntime::new().unwrap();
        for n in 0..100 {
            let rejected = runtime
                .block_on(request_worker_async(
                    &state,
                    request(&format!("overflow-{n}"), "action", Some("osa"), "set", 0),
                ))
                .unwrap();
            assert_eq!(rejected["phase"], "rejected_before_call");
        }
        let query = request("retained-query", "status", None, "", 0);
        let query_wait = f.worker.admit(&query).ok().unwrap();
        let collision = runtime
            .block_on(request_worker_async(
                &state,
                request("query-collision", "status", None, "", 0),
            ))
            .unwrap();
        assert_eq!(collision["error"]["type"], "HostAdmission");
        assert!(
            runtime
                .block_on(request_worker_async(&state, query.clone()))
                .is_err(),
            "an accepted or recent ID must not get a fabricated second terminal"
        );
        assert_eq!(
            runtime
                .block_on(request_worker_async(
                    &state,
                    request("safety-reserve", "action", Some("voltage"), "zero", 0)
                ))
                .unwrap()["ok"],
            true
        );
        let stop = f.worker.reserve_shutdown().unwrap();
        assert!(f.worker.reserve_shutdown().is_err());
        drop(stop);
        assert_eq!(f.worker.waits.lock().unwrap().len(), 32);
        f.worker
            .finish_exchange(&query, Duration::from_secs(2), query_wait)
            .unwrap();
        for (frame, wait) in admitted {
            f.worker
                .finish_exchange(&frame, Duration::from_secs(2), wait)
                .unwrap();
        }
        assert!(f.worker.waits.lock().unwrap().is_empty());
        f.worker.shutdown().unwrap();
    }
    #[test]
    fn command_boundary_one_worker_keeps_safety_query_and_close_accessible() {
        let f = Fixture::new();
        f.configure(json!({"hold":["trace"]}));
        f.worker.ready.store(true, Ordering::Release);
        let state = Arc::new(WorkerState(
            Mutex::new(Some(f.worker.clone())),
            Arc::new(AtomicBool::new(false)),
        ));
        // Tokio's documented environment override creates precisely one async worker.
        std::env::set_var("TOKIO_WORKER_THREADS", "1");
        let runtime = tauri::async_runtime::TokioRuntime::new().unwrap();
        std::env::remove_var("TOKIO_WORKER_THREADS");
        let osa_state = state.clone();
        let osa = runtime.spawn(async move {
            request_worker_async(
                &osa_state,
                request("held-osa", "action", Some("osa"), "trace", 0),
            )
            .await
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while f.configure(json!({}))["held"] != 1 {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        let (sent, received) = std::sync::mpsc::channel();
        let task_state = state.clone();
        runtime.spawn(async move {
            let zero = request_worker_async(
                &task_state,
                request("zero-command", "action", Some("voltage"), "zero", 0),
            )
            .await;
            let status = request_worker_async(
                &task_state,
                request("status-command", "status", None, "", 0),
            )
            .await;
            let close = request_worker_async(
                &task_state,
                request("close-command", "disconnect", Some("gain"), "", 0),
            )
            .await;
            sent.send((zero, status, close)).unwrap();
        });
        let progress = received.recv_timeout(Duration::from_secs(2));
        f.configure(json!({"hold_shutdown":true}));
        let stop_state = state.clone();
        let shutdown = runtime.spawn(async move { stop_worker_async(&stop_state).await });
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut shutdown_entered = false;
        while Instant::now() < deadline {
            if f.configure(json!({}))["attempts"] == 1 {
                shutdown_entered = true;
                break;
            }
            thread::yield_now();
        }
        let (query_sent, query_received) = std::sync::mpsc::channel();
        let query_state = state.clone();
        runtime.spawn(async move {
            let reply =
                request_worker_async(&query_state, request("during-close", "status", None, "", 0))
                    .await;
            let _ = query_sent.send(reply);
        });
        let during_close = query_received.recv_timeout(Duration::from_secs(2));
        f.configure(json!({"release":true}));
        runtime.block_on(osa).unwrap().unwrap();
        f.worker.writer.close();
        runtime.block_on(shutdown).unwrap().unwrap();
        let (zero, status, close) = progress.expect("command executor starved behind held OSA");
        assert_eq!(zero.unwrap()["ok"], true);
        assert_eq!(status.unwrap()["result"]["held"], 1);
        assert_eq!(close.unwrap()["ok"], true);
        assert!(
            shutdown_entered,
            "global close did not reach transport before OSA release"
        );
        assert_eq!(
            during_close.unwrap().unwrap()["result"]["held"],
            2,
            "status must remain accessible while OSA and global cleanup are held"
        );
    }
    fn request(id: &str, method: &str, role: Option<&str>, name: &str, epoch: u64) -> Value {
        let params = match role {
            Some(role) if method == "resume" => json!({"role":role,"confirm":true}),
            Some(role) => json!({"role":role,"name":name}),
            None => json!({}),
        };
        json!({"v":2,"id":id,"method":method,"params":params,"context":{"session_id":"pipe-session","connection_id":role.map(|_|"c"),"epoch":epoch}})
    }
    struct Fixture {
        root: PathBuf,
        worker: Arc<WorkerRuntime>,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "sil-v2-pipe-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(root.join("App/worker")).unwrap();
            std::fs::write(root.join("App/__init__.py"), "").unwrap();
            std::fs::write(root.join("App/worker/__init__.py"), "").unwrap();
            std::fs::write(
                root.join("App/worker/main.py"),
                include_str!("../../tests/native_worker_fixture.py"),
            )
            .unwrap();
            let worker = WorkerRuntime::spawn_legacy(
                Path::new("D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe"),
                &root,
                "real",
            )
            .unwrap();
            worker
                .exchange(
                    &request("ping", "ping", None, "", 0),
                    Duration::from_secs(2),
                )
                .unwrap();
            Self { root, worker }
        }
        fn configure(&self, params: Value) -> Value {
            let id = format!("fixture-config-{}", NEXT.fetch_add(1, Ordering::Relaxed));
            let rx = self.worker.broker.register(&id, None).unwrap();
            self.worker
                .writer
                .enqueue(
                    json!({"v":2,"id":id,"method":"configure","params":params,"context":null}),
                    DispatchClass::Normal,
                )
                .unwrap();
            rx.recv_timeout(Duration::from_secs(2)).unwrap()["result"].clone()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.worker.writer.close();
            let deadline = Instant::now() + Duration::from_secs(2);
            while self
                .worker
                .child
                .lock()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_none()
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(10));
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn runtime_fixture_requires_the_exact_real_flag() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/native_worker_fixture.py");
        for (flag, expected_code) in [("--sim", 2), ("--real", 0)] {
            let mut command = Command::new("D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe");
            command
                .arg("-B")
                .arg(&fixture)
                .args([flag, "--protocol", "2"])
                .stdin(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(expected_code), "flag: {flag}");
            if flag == "--sim" {
                assert!(String::from_utf8_lossy(&output.stderr)
                    .contains("unrecognized arguments: --sim"));
            }
        }
    }
    #[test]
    fn runtime_stop_does_not_wait_for_osa_and_reverse_replies_stay_correlated() {
        let f = Fixture::new();
        f.configure(json!({"hold":["trace"]}));
        let worker = f.worker.clone();
        let osa = thread::spawn(move || {
            worker.exchange(
                &request("osa", "action", Some("osa"), "trace", 0),
                Duration::from_secs(3),
            )
        });
        let worker = f.worker.clone();
        let second = thread::spawn(move || {
            worker.exchange(
                &request("other", "action", Some("pm400"), "trace", 0),
                Duration::from_secs(3),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while f.configure(json!({}))["held"] != 2 {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        let stop = f
            .worker
            .exchange(
                &request("stop", "action", Some("gain"), "disable_current", 0),
                Duration::from_millis(500),
            )
            .unwrap();
        assert_eq!(stop["result"]["effective_intent"], "disable_current");
        assert!(!osa.is_finished());
        f.configure(json!({"release":true}));
        assert_eq!(osa.join().unwrap().unwrap()["id"], "osa");
        assert_eq!(second.join().unwrap().unwrap()["id"], "other");
        f.worker.shutdown().unwrap();
    }
    #[test]
    fn runtime_status_and_late_reply_do_not_clear_timeout_but_authorized_resume_does() {
        let f = Fixture::new();
        f.configure(json!({"hold":["trace"]}));
        assert!(f
            .worker
            .exchange(
                &request("slow", "action", Some("osa"), "trace", 0),
                Duration::from_millis(20)
            )
            .unwrap_err()
            .contains("unconfirmed"));
        f.worker
            .exchange(
                &request("status", "status", None, "", 0),
                Duration::from_secs(1),
            )
            .unwrap();
        assert!(f
            .worker
            .exchange(
                &request("blocked", "action", Some("osa"), "set", 0),
                Duration::from_secs(1)
            )
            .is_err());
        assert!(f
            .worker
            .exchange(
                &request("premature-resume", "resume", Some("osa"), "", 0),
                Duration::from_secs(1)
            )
            .unwrap_err()
            .contains("remains pending"));
        assert_eq!(f.configure(json!({}))["held"], 1);
        f.configure(json!({"release":true}));
        assert!(f
            .worker
            .exchange(
                &request("late-blocked", "action", Some("osa"), "set", 0),
                Duration::from_secs(1)
            )
            .is_err());
        f.configure(json!({"reject_resume":true}));
        assert_eq!(
            f.worker
                .exchange(
                    &request("bad-resume", "resume", Some("osa"), "", 0),
                    Duration::from_secs(1)
                )
                .unwrap()["ok"],
            false
        );
        assert!(f.worker.broker.restricted("osa"));
        f.configure(json!({"reject_resume":false}));
        f.worker
            .exchange(
                &request("resume", "resume", Some("osa"), "", 0),
                Duration::from_secs(1),
            )
            .unwrap();
        assert!(!f.worker.broker.restricted("osa"));
        f.worker
            .exchange(
                &request("allowed", "action", Some("osa"), "set", 0),
                Duration::from_secs(1),
            )
            .unwrap();
        f.worker.shutdown().unwrap();
    }
    #[test]
    fn runtime_live_failed_cleanup_allows_a_new_attempt() {
        let f = Fixture::new();
        f.configure(json!({"behavior":"unreleased_live"}));
        assert!(f.worker.shutdown().is_err());
        let first = f.worker.shutdown_evidence()["attempts"][0].clone();
        assert_eq!(first["result"]["unreleased"], json!(["pipe_fixture"]));
        assert_eq!(f.configure(json!({}))["attempts"], 1);
        f.configure(json!({"behavior":"normal"}));
        let report = f.worker.shutdown().unwrap();
        assert_eq!(report["unreleased"], json!([]));
        let evidence = f.worker.shutdown_evidence();
        assert_eq!(evidence["attempts"][0], first);
        assert_ne!(evidence["attempts"][0]["id"], evidence["attempts"][1]["id"]);
        assert_ne!(first["result"]["attempt_id"], report["attempt_id"]);
        assert_eq!(evidence["attempts"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn runtime_eof_rejects_each_held_request_and_does_not_invent_cleanup_release() {
        let f = Fixture::new();
        f.configure(json!({"hold":["trace"]}));
        let mut calls = Vec::new();
        for (id, role) in [("held-osa", "osa"), ("held-meter", "pm400")] {
            let worker = f.worker.clone();
            calls.push(thread::spawn(move || {
                worker.exchange(
                    &request(id, "action", Some(role), "trace", 0),
                    Duration::from_secs(3),
                )
            }));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while f.configure(json!({}))["held"] != 2 {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        f.worker.writer.close();
        for call in calls {
            assert!(call.join().unwrap().unwrap_err().contains("unknown"));
        }
        for role in ["osa", "pm400", "voltage", "gain", "fiber"] {
            assert!(f.worker.broker.restricted(role));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while f.worker.poll_exit().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let report = f.worker.shutdown().unwrap();
        assert_eq!(report["process_exit"]["confirmed"], true);
        assert_eq!(report["resource_release_verified"], false);
        assert_eq!(report["unreleased"], json!(["unknown"]));
        assert_eq!(report["received_cleanup_report"], Value::Null);
    }
    #[test]
    fn runtime_late_cleanup_is_retained_without_a_waiting_caller() {
        let f = Fixture::new();
        f.configure(json!({"hold_shutdown":true}));
        assert!(f
            .worker
            .shutdown_with_limits(Duration::from_millis(20), Duration::from_millis(20))
            .is_err());
        assert!(f
            .worker
            .shutdown_with_limits(Duration::from_millis(20), Duration::from_millis(20))
            .unwrap_err()
            .contains("existing"));
        f.configure(json!({"release":true}));
        let evidence = f.worker.shutdown_evidence();
        assert_eq!(evidence["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(evidence["received_cleanup_report"]["unreleased"], json!([]));
        assert_eq!(f.configure(json!({}))["attempts"], 1);
        f.worker.writer.close();
        f.worker.shutdown().unwrap();
    }
    #[test]
    fn runtime_timed_out_shutdown_reports_later_process_exit_without_release() {
        let f = Fixture::new();
        f.configure(json!({"hold_shutdown":true}));
        assert!(f
            .worker
            .shutdown_with_limits(Duration::from_millis(20), Duration::from_millis(20))
            .is_err());
        f.worker.writer.close();
        let deadline = Instant::now() + Duration::from_secs(1);
        while f.worker.poll_exit().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let report = f.worker.shutdown().unwrap();
        assert_eq!(report["process_exit"]["confirmed"], true);
        assert_eq!(report["resource_release_verified"], false);
        assert_eq!(report["unreleased"], json!(["unknown"]));
    }
    #[test]
    fn runtime_late_resume_cannot_clear_a_newer_timeout() {
        let f = Fixture::new();
        f.configure(json!({"hold":["trace"],"hold_resume":true}));
        assert!(f
            .worker
            .exchange(
                &request("timeout1", "action", Some("osa"), "trace", 0),
                Duration::from_millis(20)
            )
            .is_err());
        f.configure(json!({"release":true}));
        let worker = f.worker.clone();
        let resume = thread::spawn(move || {
            worker.exchange(
                &request("resume-old", "resume", Some("osa"), "", 0),
                Duration::from_secs(2),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while f.configure(json!({}))["held"] != 1 {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        // A later accepted safety attempt times out while the older resume is in flight.
        let _rx = f.worker.broker.register("newer-stop", Some("osa")).unwrap();
        f.worker.broker.mark_timeout("newer-stop");
        f.configure(json!({"release":true}));
        assert!(resume
            .join()
            .unwrap()
            .unwrap_err()
            .contains("host recovery evidence changed"));
        assert!(f.worker.broker.restricted("osa"));
        f.worker.shutdown().unwrap();
    }
    #[test]
    fn runtime_released_cleanup_only_checks_exit_on_retry_and_preserves_abnormal_evidence() {
        let f = Fixture::new();
        f.configure(json!({"behavior":"shutdown_reply_live"}));
        assert!(f.worker.shutdown().unwrap_err().contains("did not exit"));
        assert_eq!(f.configure(json!({}))["attempts"], 1);
        f.worker.writer.close();
        let report = f.worker.shutdown().unwrap();
        assert_eq!(report["unreleased"], json!([]));
        let f = Fixture::new();
        f.configure(json!({"behavior":"exit_nonzero"}));
        let report = f.worker.shutdown().unwrap();
        assert_eq!(report["process_exit"]["code"], 19);
        assert_eq!(report["received_cleanup_report"]["unreleased"], json!([]));
        assert_eq!(report["resource_release_verified"], false);
    }
}

#[derive(Default)]
pub struct WorkerState(pub Mutex<Option<Arc<WorkerRuntime>>>, Arc<AtomicBool>);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartConfig {
    mode: String,
    python_path: String,
}

/// Memory is bounded even if a child emits a newline-free diagnostic flood.
impl Drop for WorkerState {
    fn drop(&mut self) {
        if let Ok(slot) = self.0.get_mut() {
            if let Some(worker) = slot.as_ref() {
                if let Err(error) = worker.shutdown() {
                    eprintln!("console worker shutdown could not be verified: {error}");
                }
            }
        }
    }
}
fn project_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let resources = app.path().resource_dir().ok();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    resolve_worker_root(
        resources.as_deref(),
        &source,
        source_fallback_permitted(tauri::is_dev(), cfg!(debug_assertions)),
    )
}
#[tauri::command]
pub async fn worker_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, WorkerState>,
    config: StartConfig,
) -> Result<Value, String> {
    let starting = reserve_start(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<WorkerState>();
        start_worker_reserved(&state, &config, &project_root(&app)?, starting)
    })
    .await
    .map_err(|e| format!("startup task failed; ownership must be inspected: {e}"))?
}
struct Starting(Arc<AtomicBool>);
impl Drop for Starting {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
fn start_worker(state: &WorkerState, config: &StartConfig, root: &Path) -> Result<Value, String> {
    let starting = reserve_start(state)?;
    start_worker_reserved(state, config, root, starting)
}
fn reserve_start(state: &WorkerState) -> Result<Starting, String> {
    let slot = state.0.lock().map_err(|_| "worker owner lock poisoned")?;
    if slot.is_some() || state.1.swap(true, Ordering::AcqRel) {
        return Err("instrument worker is already running or starting".into());
    }
    Ok(Starting(state.1.clone()))
}
fn start_worker_reserved(
    state: &WorkerState,
    config: &StartConfig,
    root: &Path,
    _starting: Starting,
) -> Result<Value, String> {
    if config.mode != "real" {
        return Err("only real hardware is supported".into());
    }
    let python = Path::new(&config.python_path)
        .canonicalize()
        .map_err(|error| format!("VISA Python path is unavailable: {error}"))?;
    if !python
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("VISA"))
    {
        return Err("select python.exe inside the Anaconda VISA environment".into());
    }
    let worker = WorkerRuntime::spawn_legacy(&python, root, &config.mode)?;
    // Ownership is published before handshake, but action authority is still disarmed.
    *state.0.lock().map_err(|_| "worker owner lock poisoned")? = Some(worker.clone());
    let identity = (|| {
        let id = format!(
            "host-ping-{}",
            NEXT_RUNTIME_ATTEMPT.fetch_add(1, Ordering::Relaxed)
        );
        let ping = worker.exchange(
            &json!({"v":2,"id":id,"method":"ping","params":{},"context":null}),
            Duration::from_secs(15),
        )?;
        if ping["ok"] != true {
            return Err(format!("Python startup failed: {}", response_error(&ping)));
        }
        let result = &ping["result"];
        validate_startup_handshake(
            &config.mode,
            result["mode"].as_str(),
            result["protocol_version"].as_u64(),
            result["connected"].as_bool(),
        )?;
        if !result["environment_name"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("VISA"))
            || result["python_executable"]
                .as_str()
                .and_then(|p| Path::new(p).canonicalize().ok())
                .as_deref()
                != Some(python.as_path())
            || result["project_root"]
                .as_str()
                .and_then(|p| Path::new(p).canonicalize().ok())
                .as_deref()
                != Some(root)
            || result["session_id"].as_str().is_none()
            || !result["roles"].as_object().is_some_and(|roles| {
                ["osa", "voltage", "gain", "pm400", "fiber"]
                    .iter()
                    .all(|role| {
                        let value = &roles[*role];
                        value["session_id"] == result["session_id"]
                            && value["connection_id"].is_null()
                            && value["epoch"].as_u64().is_some()
                    })
            })
        {
            return Err("Python interpreter, package or context identity did not match disconnected startup".into());
        }
        Ok(result.clone())
    })();
    match identity {
        Ok(identity) => {
            worker.ready.store(true, Ordering::Release);
            Ok(identity)
        }
        Err(error) => {
            // EOF is the only compatible cleanup request for an untrusted protocol.
            // Keep the owner if exit cannot be verified. Never kill or replace it.
            worker.writer.close();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if worker.poll_exit()?.is_some() {
                    let mut slot = state.0.lock().unwrap();
                    if slot
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &worker))
                    {
                        *slot = None;
                    }
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(format!(
                "{error}; startup cleanup/exit evidence: {}{}",
                worker.shutdown_evidence(),
                worker.log_suffix()
            ))
        }
    }
}
fn owned_worker(state: &WorkerState) -> Result<Arc<WorkerRuntime>, String> {
    if state.1.load(Ordering::Acquire) {
        return Err("worker startup in progress; no action authority".into());
    }
    state
        .0
        .lock()
        .map_err(|_| "worker owner lock poisoned")?
        .clone()
        .ok_or("instrument worker is not running".into())
}
fn prepare_request(
    state: &WorkerState,
    request: &Value,
) -> Result<(Arc<WorkerRuntime>, Duration), String> {
    let method = request["method"].as_str().unwrap_or("");
    if method == "shutdown" {
        return Err("use worker_stop for shutdown".into());
    }
    let worker = owned_worker(state)?;
    if !worker.ready.load(Ordering::Acquire) && !["status", "ping"].contains(&method) {
        return Err("startup identity unverified; ordinary control unavailable".into());
    }
    let timeout = match method {
        "action" => 180,
        "connect" | "disconnect" => 90,
        "status" => 5,
        _ => 30,
    };
    Ok((worker, Duration::from_secs(timeout)))
}
#[cfg(test)]
fn request_worker(state: &WorkerState, request: Value) -> Result<Value, String> {
    let (worker, timeout) = prepare_request(state, &request)?;
    worker.exchange(&request, timeout)
}
#[tauri::command]
pub async fn worker_request(
    state: tauri::State<'_, WorkerState>,
    request: Value,
) -> Result<Value, String> {
    request_worker_async(&state, request).await
}
async fn request_worker_async(state: &WorkerState, request: Value) -> Result<Value, String> {
    let (worker, timeout) = prepare_request(state, &request)?;
    let admitted = match worker.admit(&request) {
        Ok(admitted) => admitted,
        Err(AdmissionError::BeforeSend(message)) => {
            return Ok(json!({"v":2,"id":request["id"],"ok":false,
            "phase":"rejected_before_call","context":request["context"],
            "error":{"type":"HostAdmission","message":message}}))
        }
        Err(error) => return Err(error.message()),
    };
    tauri::async_runtime::spawn_blocking(move || {
        worker.finish_exchange(&request, timeout, admitted)
    })
    .await
    .map_err(|e| format!("request wait failed; outcome unknown: {e}"))?
}
#[tauri::command]
pub async fn worker_stop(state: tauri::State<'_, WorkerState>) -> Result<Value, String> {
    stop_worker_async(&state).await
}
async fn stop_worker_async(state: &WorkerState) -> Result<Value, String> {
    let worker = owned_worker(&state)?;
    let reservation = worker.reserve_shutdown()?;
    let owned = worker.clone();
    let report = tauri::async_runtime::spawn_blocking(move || stop_report(&owned, reservation))
        .await
        .map_err(|e| format!("shutdown task failed; release unconfirmed: {e}"))??;
    release_owner(state, &worker, report)
}
#[cfg(test)]
fn stop_worker(state: &WorkerState) -> Result<Value, String> {
    let worker = owned_worker(state)?;
    let reservation = worker.reserve_shutdown()?;
    let report = stop_report(&worker, reservation)?;
    release_owner(state, &worker, report)
}
fn stop_report(
    worker: &Arc<WorkerRuntime>,
    _reservation: ShutdownReservation,
) -> Result<Value, String> {
    let report = match worker.shutdown_attempt(Duration::from_secs(90), Duration::from_secs(5)) {
        Ok(report) => report,
        Err(error) => match worker.poll_exit()? {
            Some(exit) => worker.abnormal_report(&error, exit),
            None => {
                return Err(format!(
                    "{error}; shutdown evidence: {}{}",
                    worker.shutdown_evidence(),
                    worker.log_suffix()
                ))
            }
        },
    };
    Ok(report)
}
fn release_owner(
    state: &WorkerState,
    worker: &Arc<WorkerRuntime>,
    report: Value,
) -> Result<Value, String> {
    let mut slot = state.0.lock().map_err(|_| "worker owner lock poisoned")?;
    if slot
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, &worker))
    {
        *slot = None;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    const VISA_PYTHON: &str = "D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe";
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn request(id: &str, method: &str) -> Value {
        if ["ping", "status"].contains(&method) {
            json!({"v":2,"id":id,"method":method,"params":{},"context":null})
        } else {
            json!({"v":2,"id":id,"method":"action","params":{"role":"osa","name":method},
                "context":{"session_id":"pipe-session","connection_id":"c","epoch":0}})
        }
    }

    // Owns only an isolated standard-library pipe fixture.
    // Cleanup still runs on assertion failure; no real hardware process is killed.
    struct Fixture {
        root: PathBuf,
        state: WorkerState,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "sil-native-pipe-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("App/worker")).unwrap();
            fs::write(root.join("App/__init__.py"), "").unwrap();
            fs::write(root.join("App/worker/__init__.py"), "").unwrap();
            fs::write(
                root.join("App/worker/main.py"),
                include_str!("../../tests/native_worker_fixture.py"),
            )
            .unwrap();
            let worker =
                WorkerRuntime::spawn_legacy(Path::new(VISA_PYTHON), &root, "real").unwrap();
            let fixture = Self {
                root,
                state: WorkerState(Mutex::new(Some(worker)), Arc::new(AtomicBool::new(false))),
            };
            // A round trip ensures later short deadlines test transport behavior,
            // not interpreter startup scheduling.
            fixture.exchange("ready", "status").unwrap();
            fixture
        }

        fn exchange(&self, id: &str, method: &str) -> Result<Value, String> {
            self.state
                .0
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .exchange(&request(id, method), Duration::from_secs(3))
        }

        fn configure(&self, behavior: &str) {
            let worker = self.state.0.lock().unwrap().clone().unwrap();
            let id = format!("configure-{}", NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed));
            let receiver = worker.broker.register(&id, None).unwrap();
            worker.writer.enqueue(json!({"v":2,"id":id,"method":"configure","params":{"behavior":behavior},"context":null}),
                crate::request_writer::DispatchClass::Normal).unwrap();
            receiver.recv_timeout(Duration::from_secs(3)).unwrap();
        }

        fn timeout(&self) -> String {
            self.state
                .0
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .exchange(&request("timeout", "no_reply"), Duration::from_millis(30))
                .unwrap_err()
        }

        fn wait_for_exit(&self) {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if self
                    .state
                    .0
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .child
                    .lock()
                    .unwrap()
                    .try_wait()
                    .unwrap()
                    .is_some()
                {
                    return;
                }
                assert!(Instant::now() < deadline, "fixture did not exit");
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let slot = self
                .state
                .0
                .get_mut()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(worker) = slot.take() {
                worker.writer.close();
                let deadline = Instant::now() + Duration::from_secs(3);
                while worker.poll_exit().unwrap().is_none() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                assert!(
                    worker.poll_exit().unwrap().is_some(),
                    "pipe fixture must exit on EOF"
                );
            }
            if self.root.parent() == Some(std::env::temp_dir().as_path())
                && self
                    .root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("sil-native-pipe-")
            {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }

    #[test]
    fn failed_status_does_not_unlock_actions_after_a_timeout() {
        let fixture = Fixture::new();
        fixture.configure("status_error");
        assert!(fixture.timeout().contains("unknown"));
        assert_eq!(fixture.exchange("inspect", "status").unwrap()["ok"], false);
        assert!(
            fixture.exchange("mutate", "action").is_err(),
            "failed status must not reopen mutations after an unknown outcome"
        );
    }

    #[test]
    fn nonzero_exit_after_cleanup_reply_is_not_successful_shutdown() {
        let fixture = Fixture::new();
        fixture.configure("exit_nonzero");
        let result = fixture.state.0.lock().unwrap().as_mut().unwrap().shutdown();
        assert!(
            result.unwrap()["process_exit"]["success"] == false,
            "a cleanup response does not prove a successful process exit"
        );
    }

    #[test]
    fn explicit_stop_releases_confirmed_dead_owner_with_failed_cleanup_evidence() {
        let fixture = Fixture::new();
        assert!(fixture
            .exchange("crash", "exit")
            .unwrap_err()
            .contains("unknown"));
        fixture.wait_for_exit();
        let result = stop_worker(&fixture.state);
        assert!(
            result.is_ok(),
            "confirmed exit must allow explicit ownership release: {result:?}"
        );
        let report = result.unwrap();
        assert_eq!(report["steps"][0]["ok"], false);
        assert_eq!(report["voltage_zero"], Value::Null);
        assert_eq!(report["resource_release_verified"], false);
        assert_eq!(report["process_exit"]["code"], 23);
        assert_eq!(report["received_cleanup_report"], Value::Null);
        assert!(fixture.state.0.lock().unwrap().is_none());
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let restarted = start_worker(
            &fixture.state,
            &StartConfig {
                mode: "real".to_string(),
                python_path: VISA_PYTHON.to_string(),
            },
            &root,
        )
        .unwrap();
        assert_eq!(restarted["connected"], false);
        stop_worker(&fixture.state).unwrap();
        // Starting a new disconnected session does not rewrite the prior evidence.
        assert_eq!(report["steps"][0]["ok"], false);
    }

    #[test]
    fn stale_response_cannot_satisfy_a_new_request() {
        let fixture = Fixture::new();
        assert!(fixture.exchange("current", "stale").is_err());
        assert!(fixture.exchange("new-action", "action").is_err());
    }

    #[test]
    fn malformed_json_and_wrong_versions_leave_outcome_unknown() {
        for method in ["malformed", "wrong_version"] {
            let fixture = Fixture::new();
            assert!(fixture
                .exchange("broken", method)
                .unwrap_err()
                .contains("unknown"));
            assert!(fixture.exchange("mutate", "action").is_err());
        }
    }

    #[test]
    fn missing_response_envelope_does_not_count_as_a_valid_reply() {
        let fixture = Fixture::new();
        assert!(fixture.exchange("broken", "missing_ok").is_err());
        assert!(fixture.exchange("mutate", "action").is_err());
    }

    #[test]
    fn failed_shutdown_of_live_child_retains_owner() {
        let fixture = Fixture::new();
        fixture.configure("shutdown_error_live");
        assert!(stop_worker(&fixture.state).is_err());
        let mut slot = fixture.state.0.lock().unwrap();
        assert!(slot
            .as_mut()
            .unwrap()
            .child
            .lock()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none());
    }

    #[test]
    fn stopped_abnormal_child_preserves_original_report_separately() {
        let fixture = Fixture::new();
        fixture.configure("exit_nonzero");
        let report = stop_worker(&fixture.state).unwrap();
        assert_eq!(report["steps"][0]["ok"], false);
        assert_eq!(
            report["process_exit"],
            json!({"confirmed":true,"success":false,"code":19})
        );
        assert_eq!(
            report["received_cleanup_report"],
            json!({"attempt_id": report["received_cleanup_report"]["attempt_id"], "steps":[],"unreleased":[],"voltage_zero":null})
        );
        assert_eq!(report["resource_release_verified"], false);
        assert_eq!(report["unreleased"], json!(["unknown"]));
        assert!(fixture.state.0.lock().unwrap().is_none());
    }

    #[test]
    fn cleanup_reply_without_process_exit_keeps_owner_and_original_report() {
        let fixture = Fixture::new();
        fixture.configure("shutdown_reply_live");
        let error = stop_worker(&fixture.state).unwrap_err();
        assert!(error.contains("did not exit"));
        let mut slot = fixture.state.0.lock().unwrap();
        let worker = slot.as_mut().expect("live process must remain owned");
        assert!(worker.child.lock().unwrap().try_wait().unwrap().is_none());
        assert_eq!(
            worker.shutdown_evidence()["received_cleanup_report"]["unreleased"],
            json!([])
        );
    }

    #[test]
    fn later_explicit_stop_preserves_cached_cleanup_report_after_child_exits() {
        let fixture = Fixture::new();
        fixture.configure("shutdown_reply_live_marked");
        assert!(stop_worker(&fixture.state)
            .unwrap_err()
            .contains("did not exit"));
        let original = fixture
            .state
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .shutdown_evidence()["received_cleanup_report"]
            .clone();
        assert_eq!(
            original["steps"],
            json!([{"role":"pipe_fixture","action":"close","ok":true}])
        );

        fixture.configure("exit_cleanly");
        fixture.wait_for_exit();
        let status = fixture
            .state
            .0
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .child
            .lock()
            .unwrap()
            .try_wait()
            .unwrap()
            .expect("pipe fixture should have exited");
        assert!(status.success());

        let report = stop_worker(&fixture.state).unwrap();
        assert_eq!(report, original);
        assert!(fixture.state.0.lock().unwrap().is_none());
    }

    #[test]
    fn successful_status_preserves_role_restriction_after_timeout() {
        let fixture = Fixture::new();
        assert!(fixture.timeout().contains("unknown"));
        assert!(fixture.exchange("blocked", "action").is_err());
        assert_eq!(fixture.exchange("inspect", "status").unwrap()["ok"], true);
        assert!(fixture.exchange("still-blocked", "action").is_err());
    }

    #[test]
    fn stderr_tail_bounds_lines_and_newline_free_floods() {
        let store = Arc::new(Mutex::new(VecDeque::new()));
        let input = format!("{}tail", ("x".repeat(10_000) + "\n").repeat(100));
        collect_stderr(std::io::Cursor::new(input), store.clone());
        let tail = store.lock().unwrap();
        assert_eq!(tail.len(), 60);
        assert!(tail.iter().all(|line| line.len() <= 4096));
        assert_eq!(tail.back().unwrap(), "tail");
    }

    #[test]
    fn pending_start_owns_one_child_without_holding_owner_lock_or_action_authority() {
        let fixture = Fixture::new();
        stop_worker(&fixture.state).unwrap();
        let folder = fixture.root.join("App/worker");
        fs::write(folder.join("startup_hold"), "").unwrap();
        let state = Arc::new(WorkerState::default());
        let other = state.clone();
        let root = fixture.root.canonicalize().unwrap();
        let start = thread::spawn(move || {
            start_worker(
                &other,
                &StartConfig {
                    mode: "real".into(),
                    python_path: VISA_PYTHON.into(),
                },
                &root,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !folder.join("startup_entered").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            state.0.try_lock().is_ok(),
            "handshake cannot hold owner lock"
        );
        assert!(start_worker(
            &state,
            &StartConfig {
                mode: "real".into(),
                python_path: VISA_PYTHON.into()
            },
            &fixture.root
        )
        .unwrap_err()
        .contains("already running"));
        assert!(request_worker(&state, request("early-action", "action"))
            .unwrap_err()
            .contains("startup"));
        assert!(stop_worker(&state).unwrap_err().contains("startup"));
        fs::write(folder.join("startup_release"), "").unwrap();
        assert_eq!(start.join().unwrap().unwrap()["connected"], false);
        assert!(state
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .ready
            .load(Ordering::Acquire));
        stop_worker(&state).unwrap();
    }

    #[test]
    fn v1_handshake_is_rejected_and_live_failed_startup_retains_ownership() {
        let fixture = Fixture::new();
        stop_worker(&fixture.state).unwrap();
        let folder = fixture.root.join("App/worker");
        fs::write(folder.join("startup_v1"), "").unwrap();
        fs::write(folder.join("retain_eof"), "").unwrap();
        let state = WorkerState::default();
        let result = start_worker(
            &state,
            &StartConfig {
                mode: "real".into(),
                python_path: VISA_PYTHON.into(),
            },
            &fixture.root,
        );
        assert!(result
            .unwrap_err()
            .contains("startup cleanup/exit evidence"));
        let worker = state
            .0
            .lock()
            .unwrap()
            .clone()
            .expect("unconfirmed owner must remain");
        assert!(!worker.ready.load(Ordering::Acquire));
        assert!(worker.poll_exit().unwrap().is_none());
        assert!(request_worker(&state, request("early-action", "action"))
            .unwrap_err()
            .contains("unverified"));
        assert!(start_worker(
            &state,
            &StartConfig {
                mode: "real".into(),
                python_path: VISA_PYTHON.into()
            },
            &fixture.root
        )
        .is_err());
        fs::write(folder.join("release_eof"), "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while worker.poll_exit().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            stop_worker(&state).unwrap()["resource_release_verified"],
            false
        );
        assert!(state.0.lock().unwrap().is_none());
    }

    #[test]
    fn status_exposes_host_restrictions_and_live_cleanup_evidence_is_preserved() {
        let fixture = Fixture::new();
        assert!(fixture.timeout().contains("unknown"));
        let status = fixture.exchange("cached-status", "status").unwrap();
        assert_eq!(status["result"]["host_transport"]["roles"]["osa"], true);
        fixture.configure("unreleased_live");
        let first = stop_worker(&fixture.state).unwrap_err();
        assert!(first.contains("shutdown evidence"));
        let owner = fixture.state.0.lock().unwrap().clone().unwrap();
        let original = owner.shutdown_evidence()["attempts"][0].clone();
        fixture.configure("normal");
        assert_eq!(
            stop_worker(&fixture.state).unwrap()["unreleased"],
            json!([])
        );
        assert_eq!(owner.shutdown_evidence()["attempts"][0], original);
    }

    #[test]
    fn malformed_status_payload_returns_unknown_without_panicking_or_releasing_owner() {
        let fixture = Fixture::new();
        fixture.configure("status_nonobject");
        let result = fixture.exchange("bad-status", "status");
        assert!(result.unwrap_err().contains("unknown"));
        assert!(fixture.state.0.lock().unwrap().is_some());
    }

    #[test]
    fn invalid_interpreter_selection_never_acquires_worker_ownership() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let state = WorkerState::default();
        let missing = std::env::temp_dir().join("sil-no-such-python.exe");
        assert!(!missing.exists());
        let missing_result = start_worker(
            &state,
            &StartConfig {
                mode: "real".to_string(),
                python_path: missing.to_string_lossy().into_owned(),
            },
            &root,
        );
        assert!(missing_result.unwrap_err().contains("path is unavailable"));
        // The Rust test executable exists, but it is not a VISA interpreter;
        // rejecting it before spawn also prevents accidental recursive tests.
        let wrong_environment = start_worker(
            &state,
            &StartConfig {
                mode: "real".to_string(),
                python_path: std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            },
            &root,
        );
        assert!(wrong_environment.unwrap_err().contains("VISA environment"));
        assert!(state.0.lock().unwrap().is_none());
    }
}
