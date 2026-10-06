#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peer_stop_clears_exit_latch_only_for_authenticated_matching_host_boot_and_release() {
        use std::sync::atomic::Ordering;
        let state = GuiState::default();
        let host = "a".repeat(32);
        let boot = "b".repeat(32);
        *state.stream_identity.lock().unwrap() = Some((host.clone(), boot.clone()));
        state.release_required.store(true, Ordering::Release);
        let good = json!({"type":"host_stopped","host_id":host,"boot_id":boot,"data":{"resource_released":true,"process_exit":{"confirmed":true,"success":true},"physical_zero_verified":false}});
        for bad in [
            json!({}),
            {
                let mut e = good.clone();
                e["boot_id"] = json!("c".repeat(32));
                e
            },
            {
                let mut e = good.clone();
                e["host_id"] = json!("c".repeat(32));
                e
            },
            {
                let mut e = good.clone();
                e["data"]["resource_released"] = json!(false);
                e
            },
            {
                let mut e = good.clone();
                e["data"]["process_exit"]["success"] = json!(false);
                e
            },
            {
                let mut e = good.clone();
                e["data"]["process_exit"]["confirmed"] = json!(false);
                e
            },
        ] {
            state.observe_host_event(&host, &boot, &bad);
            assert!(state.release_required.load(Ordering::Acquire));
        }
        state.observe_host_event(&host, &boot, &good);
        assert!(!state.release_required.load(Ordering::Acquire));
        state.release_required.store(true, Ordering::Release);
        *state.stream_identity.lock().unwrap() = Some((host.clone(), "c".repeat(32)));
        state.observe_host_event(&host, &boot, &good);
        assert!(
            state.release_required.load(Ordering::Acquire),
            "old stream cleared a new connection's exit latch"
        );
    }
    #[test]
    fn real_only_gui_has_named_methods_and_no_backend_selector() {
        assert!(allowed("create_draft"));
        assert!(allowed("execute"));
        for method in [
            "worker_request",
            "worker_start",
            "raw",
            "settings_save",
            "register_verified",
        ] {
            assert!(!allowed(method));
        }
        let config: StartConfig = serde_json::from_value(json!({
            "pythonPath":"D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe"
        }))
        .unwrap();
        assert!(config.validate().is_ok());
        assert!(serde_json::from_value::<StartConfig>(json!({
            "pythonPath":"VISA/python.exe", "mode":"simulate"
        }))
        .is_err());
    }
    #[test]
    fn historical_archive_methods_are_named_result_channel_calls_not_command_authority() {
        for method in [
            "list_archives",
            "read_archive",
            "archive_manifest",
            "archive_manifest_bytes",
        ] {
            assert!(allowed(method));
            assert_eq!(request_channel(method), "results");
        }
        for private in ["read_capture_chunk", "ack_capture", "export_to_path"] {
            assert!(!allowed(private));
        }
        assert_eq!(request_channel("safe_stop"), "safety");
        assert_eq!(request_channel("execute"), "control");
    }
    #[test]
    fn real_only_startup_preferences_survive_restart_and_reject_corrupt_config() {
        let dir = std::env::temp_dir().join(format!("yang-gui-prefs-{}", new_id().unwrap()));
        let path = dir.join("preferences.json");
        let missing = load_preferences(&path).unwrap();
        assert_eq!(
            serde_json::to_value(&missing).unwrap(),
            json!({"pythonPath":"D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe"})
        );
        let mut chosen = missing.clone();
        chosen.python_path = "VISA/python.exe".into();
        save_preferences(&path, &chosen).unwrap();
        assert_eq!(
            load_preferences(&path).unwrap().python_path,
            "VISA/python.exe"
        );
        assert_eq!(
            serde_json::to_value(load_preferences(&path).unwrap()).unwrap(),
            json!({"pythonPath":"VISA/python.exe"})
        );
        std::fs::write(&path, b"{broken").unwrap();
        assert!(load_preferences(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn real_only_legacy_preferences_preserve_python_path_and_original_bytes() {
        let dir = std::env::temp_dir().join(format!("yang-gui-prefs-{}", new_id().unwrap()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("preferences.json");
        let original = br#"{"version":1,"local_host":{"mode":"simulate","pythonPath":"VISA/python.exe","confirmReal":false}}"#;
        std::fs::write(&path, original).unwrap();
        let config = load_preferences(&path).unwrap();
        assert_eq!(
            serde_json::to_value(config).unwrap(),
            json!({"pythonPath":"VISA/python.exe"})
        );
        assert_eq!(
            std::fs::read(path.with_extension("pre-real-only.json")).unwrap(),
            original
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap()["version"],
            2
        );
        assert!(load_preferences(&path).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn disconnect_and_close_share_the_release_gate() {
        assert!(!client_may_detach(Ok(&json!({"released":false})), true));
        assert!(client_may_detach(Ok(&json!({"released":true})), true));
        let error = HostError::new("HostOffline", "transport lost");
        assert!(!client_may_detach(Err(&error), true));
        assert!(client_may_detach(Err(&error), false));
    }
    #[test]
    fn safety_reply_does_not_wait_for_an_unresolved_ordinary_rpc() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            use crate::host::ipc::{read_frame, write_frame};
            let security = PipeSecurity::current().unwrap();
            let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
            let mut ordinary_pipe = security.create_server(&endpoint, true).unwrap();
            let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let ordinary_server = tokio::spawn(async move {
                ordinary_pipe.connect().await.unwrap();
                let hello: HostRequest =
                    serde_json::from_slice(&read_frame(&mut ordinary_pipe).await.unwrap().unwrap())
                        .unwrap();
                write_frame(
                    &mut ordinary_pipe,
                    &HostReply::from_result(
                        hello.id,
                        Ok(json!({"protocol_version":1,"mode":"real","worker_protocol":3,"attach_token":"a".repeat(32)})),
                    ),
                )
                .await
                .unwrap();
                let request: HostRequest =
                    serde_json::from_slice(&read_frame(&mut ordinary_pipe).await.unwrap().unwrap())
                        .unwrap();
                seen_tx.send(()).unwrap();
                release_rx.await.unwrap();
                write_frame(
                    &mut ordinary_pipe,
                    &HostReply::from_result(request.id, Ok(json!({"ordinary":true}))),
                )
                .await
                .unwrap();
            });
            let control = Arc::new(LocalHostClient::connect(&endpoint).await.unwrap());
            let safety_endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
            let mut safety_pipe = security.create_server(&safety_endpoint, true).unwrap();
            let safety_server = tokio::spawn(async move {
                safety_pipe.connect().await.unwrap();
                let hello: HostRequest =
                    serde_json::from_slice(&read_frame(&mut safety_pipe).await.unwrap().unwrap())
                        .unwrap();
                write_frame(
                    &mut safety_pipe,
                    &HostReply::from_result(
                        hello.id,
                        Ok(json!({"protocol_version":1,"mode":"real","worker_protocol":3,"attach_token":"b".repeat(32)})),
                    ),
                )
                .await
                .unwrap();
                let request: HostRequest =
                    serde_json::from_slice(&read_frame(&mut safety_pipe).await.unwrap().unwrap())
                        .unwrap();
                assert_eq!(request.method, "safe_stop");
                write_frame(
                    &mut safety_pipe,
                    &HostReply::from_result(request.id, Ok(json!({"released":true}))),
                )
                .await
                .unwrap();
            });
            let safety = Arc::new(LocalHostClient::connect(&safety_endpoint).await.unwrap());
            let clients = Clients {
                control: control.clone(),
                heartbeat: control.clone(),
                results: control,
                safety,
            };
            let ordinary = clients.request_client("test_connection");
            let blocked = tokio::spawn(async move {
                ordinary
                    .call(HostRequest {
                        v: 1,
                        id: "ordinary".into(),
                        method: "test_connection".into(),
                        params: json!({}),
                    })
                    .await
            });
            seen_rx.await.unwrap();
            let reply = tokio::time::timeout(
                Duration::from_millis(300),
                clients.request_client("safe_stop").call(HostRequest {
                    v: 1,
                    id: "safety".into(),
                    method: "safe_stop".into(),
                    params: json!({}),
                }),
            )
            .await;
            release_tx.send(()).unwrap();
            blocked.await.unwrap().unwrap();
            ordinary_server.await.unwrap();
            if reply.is_ok() {
                safety_server.await.unwrap();
            } else {
                safety_server.abort();
            }
            assert!(reply.is_ok(), "safety waited for the ordinary pipe mutex");
            assert_eq!(reply.unwrap().unwrap().result["released"], true);
        });
    }
}
use crate::host::archive::ArchiveRef;
use crate::{
    host::{
        contracts::HostError,
        ipc::{HostReply, HostRequest, PipeSecurity},
        registry::new_id,
    },
    host_client::LocalHostClient,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{Emitter, Manager};
struct Clients {
    control: Arc<LocalHostClient>,
    heartbeat: Arc<LocalHostClient>,
    results: Arc<LocalHostClient>,
    safety: Arc<LocalHostClient>,
}
impl Clients {
    fn request_client(&self, method: &str) -> Arc<LocalHostClient> {
        match request_channel(method) {
            "safety" => self.safety.clone(),
            "results" => self.results.clone(),
            "heartbeat" => self.heartbeat.clone(),
            _ => self.control.clone(),
        }
    }
}
fn request_channel(method: &str) -> &'static str {
    match method {
        "safe_stop" | "release_control" | "close_client" | "stop" => "safety",
        "operation"
        | "request_snapshot"
        | "list_archives"
        | "read_archive"
        | "archive_manifest"
        | "archive_manifest_bytes" => "results",
        "ping" => "heartbeat",
        _ => "control",
    }
}
#[derive(Default)]
pub struct GuiState {
    clients: Mutex<Option<Clients>>,
    events: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    stream_identity: Mutex<Option<(String, String)>>,
    closing: std::sync::atomic::AtomicBool,
    file_dialog_open: Arc<std::sync::atomic::AtomicBool>,
    release_required: std::sync::atomic::AtomicBool,
    pub exit_authorized: std::sync::atomic::AtomicBool,
}
fn client_may_detach(report: Result<&Value, &HostError>, release_required: bool) -> bool {
    match report {
        Ok(value) => value["released"] == true,
        Err(_) => !release_required,
    }
}
impl GuiState {
    pub(crate) fn client(&self, method: &str) -> Result<Arc<LocalHostClient>, HostError> {
        self.clients.lock().unwrap().as_ref().map(|c|c.request_client(method)).ok_or_else(offline)
    }
    fn observe_host_event(&self, host: &str, boot: &str, event: &Value) {
        let identity = self.stream_identity.lock().unwrap();
        if crate::host::contracts::valid_id(host)
            && crate::host::contracts::valid_id(boot)
            && identity.as_ref().is_some_and(|(active_host, active_boot)| {
                active_host == host && active_boot == boot
            })
            && event["type"] == "host_stopped"
            && event["host_id"] == host
            && event["boot_id"] == boot
            && event["data"]["resource_released"] == true
            && event["data"]["process_exit"]["confirmed"] == true
            && event["data"]["process_exit"]["success"] == true
        {
            self.release_required
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }
    async fn close_connection(&self) -> Result<Value, HostError> {
        use std::sync::atomic::Ordering;
        let client = self
            .clients
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.safety.clone());
        let report = if let Some(client) = client {
            client
                .call(HostRequest {
                    v: 1,
                    id: new_id()?,
                    method: "close_client".into(),
                    params: json!({}),
                })
                .await
                .and_then(|r| {
                    if r.ok {
                        Ok(r.result)
                    } else {
                        Err(r.error.unwrap())
                    }
                })
        } else {
            Ok(json!({"released":true,"cleanup_attempts":[],"host_stopped":false}))
        };
        if !client_may_detach(
            report.as_ref(),
            self.release_required.load(Ordering::Acquire),
        ) {
            return Err(HostError::new("ClientCleanupRetained",format!("Keep this management window: client-owned release is not confirmed: {report:?}")));
        }
        let value=report.unwrap_or_else(|error|json!({"released":true,"cleanup_attempts":[],"no_client_release_required":true,"transport_error":error,"host_stopped":false,"physical_zero_verified":false}));
        self.release_required.store(false, Ordering::Release);
        self.detach();
        Ok(value)
    }
    pub fn close_requested(app: &tauri::AppHandle) {
        let state = app.state::<GuiState>();
        if state
            .closing
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let remote = app.state::<crate::remote_gui::RemoteGuiState>().close_all(&app).await;
            let report = match remote { Ok(()) => app.state::<GuiState>().close_connection().await, Err(error) => Err(error) };
            let state = app.state::<GuiState>();
            match report {
                Ok(report) if report["released"] == true => {
                    state
                        .exit_authorized
                        .store(true, std::sync::atomic::Ordering::Release);
                    state.detach();
                    app.exit(0);
                }
                result => {
                    app.state::<crate::remote_gui::RemoteGuiState>().cancel_close();
                    let _=app.emit("host-close-retained",json!({"report":result.as_ref().ok(),"error":result.as_ref().err(),"message":"Window retained: client-owned cleanup is not confirmed. Inspect and retry safe stop; Host remains available."}));
                    state
                        .closing
                        .store(false, std::sync::atomic::Ordering::Release);
                }
            }
        });
    }
    pub fn detach(&self) {
        self.stream_identity.lock().unwrap().take();
        if let Some(task) = self.events.lock().unwrap().take() {
            task.abort();
        }
        self.clients.lock().unwrap().take();
    }
}
fn allowed(method: &str) -> bool {
    matches!(
        method,
        "ping"
            | "remote_status"
            | "remote_listener"
            | "remote_pair_begin"
            | "remote_approve"
            | "remote_revoke"
            | "catalog"
            | "snapshot"
            | "worker_status"
            | "acquire_control"
            | "release_control"
            | "safe_stop"
            | "stop"
            | "prepare"
            | "execute"
            | "operation"
            | "create_draft"
            | "test_connection"
            | "save_device"
            | "cancel_draft"
            | "save_settings"
            | "save_setup"
            | "retire_setup"
            | "rename_device"
            | "retire_device"
            | "save_check_policy"
            | "request_snapshot"
            | "close_client"
            | "list_archives"
            | "read_archive"
            | "archive_manifest"
            | "archive_manifest_bytes"
    )
}
fn offline() -> HostError {
    HostError::new("HostOffline", "Connect to the local Host first")
}
#[tauri::command]
pub async fn choose_data_root(
    window: tauri::Window,
    state: tauri::State<'_, GuiState>,
) -> Result<Option<String>, HostError> {
    let guard = crate::native_files::SelectionGuard::begin(&state.file_dialog_open)?;
    if state.closing.load(std::sync::atomic::Ordering::Acquire) {
        return Err(HostError::new(
            "GuiClosing",
            "Client release is in progress",
        ));
    }
    let owner = window
        .hwnd()
        .map_err(|e| HostError::new("NativeWindow", e.to_string()))?
        .0 as isize;
    let (selected, _guard) =
        crate::native_files::choose_folder(owner, "Choose the local recording folder", guard)
            .await?;
    selected
        .map(|folder| {
            let path = folder
                .path()
                .to_str()
                .filter(|p| p.len() <= 1024)
                .ok_or_else(|| {
                    HostError::new("HostSettings", "Folder path is unsupported or too long")
                })?;
            Ok(path.to_owned())
        })
        .transpose()
}
#[tauri::command]
pub async fn export_archive(
    window: tauri::Window,
    state: tauri::State<'_, GuiState>,
    reference: ArchiveRef,
) -> Result<Option<Value>, HostError> {
    reference.validate()?;
    let guard = crate::native_files::SelectionGuard::begin(&state.file_dialog_open)?;
    if state.closing.load(std::sync::atomic::Ordering::Acquire) {
        return Err(HostError::new(
            "GuiClosing",
            "Client release is in progress",
        ));
    }
    let client = state
        .clients
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.results.clone())
        .ok_or_else(offline)?;
    let reply = client
        .call(HostRequest {
            v: 1,
            id: new_id()?,
            method: "ping".into(),
            params: json!({}),
        })
        .await?;
    if !reply.ok {
        return Err(reply.error.unwrap());
    }
    let host = reply.result["host_id"]
        .as_str()
        .filter(|id| crate::host::contracts::valid_id(id))
        .ok_or_else(|| HostError::new("ArchiveScope", "Connected Host identity unavailable"))?
        .to_owned();
    if host != reference.host_id {
        return Err(HostError::new(
            "ArchiveScope",
            "Archive belongs to another Host",
        ));
    }
    let owner = window
        .hwnd()
        .map_err(|e| HostError::new("NativeWindow", e.to_string()))?
        .0 as isize;
    let (selected, _guard) = crate::native_files::choose_folder(
        owner,
        "Choose a folder for the native spectrum export",
        guard,
    )
    .await?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let bundle = crate::native_files::receive_archive(&reference, &host, |method, params| {
        let method = method.to_owned();
        let client = client.clone();
        let available = !state.closing.load(std::sync::atomic::Ordering::Acquire)
            && state
                .clients
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|c| Arc::ptr_eq(&client, &c.results));
        async move {
            if !available {
                return Err(HostError::new(
                    "ArchiveCancelled",
                    "Client disconnected or is closing",
                ));
            }
            let reply = client
                .call(HostRequest {
                    v: 1,
                    id: new_id()?,
                    method,
                    params,
                })
                .await?;
            if reply.ok {
                Ok(reply.result)
            } else {
                Err(reply.error.unwrap())
            }
        }
    })
    .await?;
    if state.closing.load(std::sync::atomic::Ordering::Acquire) {
        return Err(HostError::new("ArchiveCancelled", "Client is closing"));
    }
    let destination = tauri::async_runtime::spawn_blocking(move || {
        let _guard = _guard;
        bundle.write(&selected)
    })
    .await
    .map_err(|e| HostError::new("ArchiveExport", e.to_string()))??;
    Ok(Some(
        json!({"directory":destination,"id":reference.id,"name":reference.name,"native_sha256":reference.sha256}),
    ))
}
#[tauri::command]
pub async fn host_connect(app:tauri::AppHandle,state: tauri::State<'_, GuiState>) -> Result<Value, HostError> {
    app.state::<crate::profile::Profile>().require_hardware_host()?;
    if state.clients.lock().unwrap().is_some() {
        return Err(HostError::new("HostConnected", "Already connected"));
    }
    let endpoint = PipeSecurity::current()?.endpoint();
    let control = Arc::new(LocalHostClient::connect(&endpoint).await?);
    let heartbeat = Arc::new(control.attached("heartbeat").await?);
    let results = Arc::new(control.attached("results").await?);
    let safety = Arc::new(control.attached("safety").await?);
    let mut slot = state.clients.lock().unwrap();
    if slot.is_some() {
        return Err(HostError::new(
            "HostConnected",
            "Concurrent connect rejected",
        ));
    }
    *slot = Some(Clients {
        control,
        heartbeat,
        results,
        safety,
    });
    state
        .release_required
        .store(false, std::sync::atomic::Ordering::Release);
    Ok(json!({"connected":true,"mode":"real","worker_protocol":3}))
}
#[tauri::command]
pub async fn host_call(
    state: tauri::State<'_, GuiState>,
    request: HostRequest,
) -> Result<HostReply, HostError> {
    if !allowed(&request.method) {
        return Err(HostError::new(
            "HostMethod",
            "Only named Host APIs are available",
        ));
    }
    crate::host::ipc::parse_request(
        &serde_json::to_vec(&request).map_err(|e| HostError::new("HostProtocol", e.to_string()))?,
    )?;
    if state.closing.load(std::sync::atomic::Ordering::Acquire)
        && !matches!(
            request.method.as_str(),
            "ping"
                | "catalog"
                | "snapshot"
                | "worker_status"
                | "operation"
                | "safe_stop"
                | "stop"
                | "close_client"
                | "request_snapshot"
        )
    {
        return Err(HostError::new(
            "GuiClosing",
            "Client release is in progress",
        ));
    }
    let client = state
        .clients
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.request_client(&request.method))
        .ok_or_else(offline)?;
    if request.method == "acquire_control" {
        state
            .release_required
            .store(true, std::sync::atomic::Ordering::Release);
    }
    let stop = request.method == "stop";
    let reply = client.call(request).await?;
    if stop
        && reply.ok
        && reply.result["resource_released"] == true
        && reply.result["process_exit"]["confirmed"] == true
        && reply.result["process_exit"]["success"] == true
    {
        state
            .release_required
            .store(false, std::sync::atomic::Ordering::Release);
    }
    Ok(reply)
}
#[tauri::command]
pub async fn host_heartbeat(
    state: tauri::State<'_, GuiState>,
    token: String,
) -> Result<Value, HostError> {
    let client = state
        .clients
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.heartbeat.clone())
        .ok_or_else(offline)?;
    let reply = client
        .call(HostRequest {
            v: 1,
            id: new_id()?,
            method: "renew_control".into(),
            params: json!({"token":token}),
        })
        .await?;
    if reply.ok {
        Ok(reply.result)
    } else {
        Err(reply.error.unwrap())
    }
}
#[tauri::command]
pub async fn host_result(
    state: tauri::State<'_, GuiState>,
    params: Value,
) -> Result<Value, HostError> {
    let client = state
        .clients
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.results.clone())
        .ok_or_else(offline)?;
    let reply = client
        .call(HostRequest {
            v: 1,
            id: new_id()?,
            method: "read_result".into(),
            params,
        })
        .await?;
    if reply.ok {
        Ok(reply.result)
    } else {
        Err(reply.error.unwrap())
    }
}
#[tauri::command]
pub async fn host_subscribe(
    app: tauri::AppHandle,
    state: tauri::State<'_, GuiState>,
) -> Result<(), HostError> {
    if state.events.lock().unwrap().is_some() {
        return Err(HostError::new("EventSubscription", "Already subscribed"));
    }
    let control = state
        .clients
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.control.clone())
        .ok_or_else(offline)?;
    let events = control.attached("events").await?.subscribe().await?;
    let host = events.snapshot["host_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let boot = events.snapshot["boot_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    *state.stream_identity.lock().unwrap() = Some((host.clone(), boot.clone()));
    let task = tauri::async_runtime::spawn(async move {
        let _ = app.emit("host-event", events.snapshot.clone());
        loop {
            match events.next().await {
                Ok(Some(event)) => {
                    app.state::<GuiState>()
                        .observe_host_event(&host, &boot, &event);
                    let _ = app.emit("host-event", event);
                }
                _ => {
                    let _ = app.emit("host-offline", json!({"output_state":"UNKNOWN"}));
                    break;
                }
            }
        }
    });
    *state.events.lock().unwrap() = Some(task);
    Ok(())
}
#[tauri::command]
pub async fn host_disconnect(state: tauri::State<'_, GuiState>) -> Result<Value, HostError> {
    use std::sync::atomic::Ordering;
    if state.closing.swap(true, Ordering::AcqRel) {
        return Err(HostError::new(
            "GuiClosing",
            "Client release already in progress",
        ));
    }
    let result = state.close_connection().await;
    state.closing.store(false, Ordering::Release);
    result
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StartConfig {
    python_path: String,
}
impl StartConfig {
    fn validate(&self) -> Result<(), HostError> {
        if self.python_path.trim().is_empty()
            || self.python_path.len() > 1024
            || self.python_path.contains('\0')
        {
            return Err(HostError::new(
                "HostStart",
                "Choose a valid Anaconda VISA Python executable",
            ));
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    local_host: StartConfig,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyStartConfig {
    mode: String,
    python_path: String,
    #[serde(default)]
    confirm_real: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyPreferences {
    version: u32,
    local_host: LegacyStartConfig,
}
fn load_preferences(path: &std::path::Path) -> Result<StartConfig, HostError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StartConfig {
                python_path: "D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe".into(),
            })
        }
        Err(error) => return Err(HostError::new("Preferences", error.to_string())),
    };
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(16385)
        .read_to_end(&mut bytes)
        .map_err(|e| HostError::new("Preferences", e.to_string()))?;
    if bytes.len() > 16384 {
        return Err(HostError::new(
            "Preferences",
            "Preferences exceed bounded storage",
        ));
    }
    let version = serde_json::from_slice::<Value>(&bytes)
        .map_err(|e| HostError::new("Preferences", e.to_string()))?["version"]
        .as_u64();
    if version == Some(1) {
        let legacy: LegacyPreferences = serde_json::from_slice(&bytes)
            .map_err(|e| HostError::new("Preferences", e.to_string()))?;
        if legacy.version != 1 || !matches!(legacy.local_host.mode.as_str(), "real" | "simulate") {
            return Err(HostError::new("Preferences", "Invalid legacy preferences"));
        }
        // Confirmation/mode are obsolete input only, never hardware authority.
        let _ = legacy.local_host.confirm_real;
        let config = StartConfig {
            python_path: legacy.local_host.python_path,
        };
        config.validate()?;
        crate::host::registry::backup_original(&path.with_extension("pre-real-only.json"), &bytes)?;
        save_preferences(path, &config)?;
        return Ok(config);
    }
    let prefs: Preferences =
        serde_json::from_slice(&bytes).map_err(|e| HostError::new("Preferences", e.to_string()))?;
    if prefs.version != 2 {
        return Err(HostError::new(
            "Preferences",
            "Unsupported preference version",
        ));
    }
    prefs.local_host.validate()?;
    Ok(prefs.local_host)
}
fn save_preferences(path: &std::path::Path, config: &StartConfig) -> Result<(), HostError> {
    config.validate()?;
    let bytes = serde_json::to_vec(&Preferences {
        version: 2,
        local_host: config.clone(),
    })
    .map_err(|e| HostError::new("Preferences", e.to_string()))?;
    crate::host::registry::write_atomic(path, &bytes)
}
fn preferences_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, HostError> {
    Ok(crate::profile::config_dir(app)?.join("preferences.json"))
}
#[tauri::command]
pub async fn host_preferences(app: tauri::AppHandle) -> Result<StartConfig, HostError> {
    let path = preferences_path(&app)?;
    tokio::task::spawn_blocking(move || load_preferences(&path))
        .await
        .map_err(|e| HostError::new("Preferences", e.to_string()))?
}
#[tauri::command]
pub async fn host_save_preferences(
    app: tauri::AppHandle,
    config: StartConfig,
) -> Result<(), HostError> {
    let path = preferences_path(&app)?;
    tokio::task::spawn_blocking(move || save_preferences(&path, &config))
        .await
        .map_err(|e| HostError::new("Preferences", e.to_string()))?
}
#[tauri::command]
pub async fn host_start(app: tauri::AppHandle, config: StartConfig) -> Result<Value, HostError> {
    app.state::<crate::profile::Profile>().require_hardware_host()?;
    config.validate()?;
    let endpoint = PipeSecurity::current()?.endpoint();
    match LocalHostClient::connect(&endpoint).await {
        Ok(_) => {
            return Err(HostError::new(
                "HostRunning",
                "Host is already running; connect instead",
            ))
        }
        Err(error) if error.code == "HostAbsent" => (),
        Err(error) => return Err(error),
    }
    let resource = app.path().resource_dir().ok();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = crate::worker_root::resolve_worker_root(
        resource.as_deref(),
        &source,
        crate::worker_root::source_fallback_permitted(tauri::is_dev(), cfg!(debug_assertions)),
    )
    .map_err(|e| HostError::new("HostPackage", e))?;
    let executable =
        if crate::worker_root::source_fallback_permitted(tauri::is_dev(), cfg!(debug_assertions)) {
            let current = std::env::current_exe()
                .map_err(|e| HostError::new("HostPackageMissing", e.to_string()))?;
            crate::worker_root::resolve_packaged_host(
                current.parent().ok_or_else(|| {
                    HostError::new("HostPackageMissing", "Native directory missing")
                })?,
            )?
        } else {
            crate::worker_root::resolve_packaged_host(resource.as_deref().ok_or_else(|| {
                HostError::new("HostPackageMissing", "Bundled resources missing")
            })?)?
        };
    let directory = crate::profile::config_dir(&app)?.join("host");
    use std::os::windows::process::CommandExt;
    std::process::Command::new(executable)
        .args([
            "--root",
            root.to_str()
                .ok_or_else(|| HostError::new("HostStart", "Invalid root"))?,
            "--record-dir",
            directory
                .to_str()
                .ok_or_else(|| HostError::new("HostStart", "Invalid config path"))?,
            "--python",
            &config.python_path,
            "--real",
        ])
        .creation_flags(0x08000000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| HostError::new("HostStart", e.to_string()))?;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if LocalHostClient::connect(&endpoint).await.is_ok() {
            return Ok(json!({"started":true,"mode":"real"}));
        }
    }
    Err(HostError::new(
        "HostStartPending",
        "Host readiness unknown; do not start another instance",
    ))
}
