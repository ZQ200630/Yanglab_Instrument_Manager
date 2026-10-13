//! Administrative recovery never certifies instrument release or replays work.
use super::*;

fn pending(message: impl Into<String>) -> HostError {
    HostError::new("HostRecoveryPending", message)
}
fn validate_administrative_stop(
    report: &Value,
    host: &str,
    boot: &str,
) -> Result<String, HostError> {
    let incident = report["incident_id"]
        .as_str()
        .filter(|id| crate::host::contracts::valid_id(id));
    if !crate::host::contracts::valid_id(host)
        || !crate::host::contracts::valid_id(boot)
        || report["administrative_stop"] != true
        || report["host_id"] != host
        || report["boot_id"] != boot
        || report["process_exit"]["confirmed"] != true
        || report["resource_released"] != false
        || report["physical_zero_verified"] != false
        || incident.is_none()
    {
        return Err(pending("Administrative stop evidence is unconfirmed. Cleanup was not certified; no recovery authorization was written."));
    }
    Ok(incident.unwrap().to_owned())
}
fn recovery_receipt(mut receipt: Value, incident: Option<&str>) -> Result<Value, HostError> {
    let actual = receipt["incident_id"]
        .as_str()
        .filter(|id| crate::host::contracts::valid_id(id));
    if receipt["recovery_authorized"] != true
        || receipt["cleanup_verified"] != false
        || receipt["resource_released"] != false
        || receipt["physical_zero_verified"] != false
        || receipt["require_readonly_verification"] != true
        || actual.is_none()
        || incident.is_some_and(|expected| Some(expected) != actual)
    {
        return Err(pending("Replacement authorization is unconfirmed. Keep the previous incident; instrument state is not verified."));
    }
    // Only machine-guard acquisition can establish that the previous Host no longer owns it.
    receipt["host_stopped"] = json!(true);
    Ok(receipt)
}
pub(super) struct RecoveryGate<'a>(&'a std::sync::atomic::AtomicBool);
impl<'a> RecoveryGate<'a> {
    pub(super) fn begin(state: &'a GuiState) -> Result<Self, HostError> {
        use std::sync::atomic::Ordering;
        // Serialize admission with a connection's final installation, not its I/O.
        let _clients = state.clients.lock().unwrap();
        state
            .closing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                HostError::new(
                    "GuiClosing",
                    "Another Local Host startup, close or recovery action is in progress",
                )
            })?;
        Ok(Self(&state.closing))
    }
}
impl Drop for RecoveryGate<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}
async fn stop_exited_host(
    endpoint: &str,
    expected: Option<&(String, String)>,
) -> Result<Option<String>, HostError> {
    // A dedicated, short-lived management connection cannot poison the GUI's shared
    // safety channel if this bounded attempt ends with an uncertain transport result.
    let client = match LocalHostClient::connect(endpoint).await {
        Ok(client) => client,
        Err(error) if error.code == "HostAbsent" => return Ok(None),
        Err(error) => return Err(error),
    };
    let ping = client
        .call(HostRequest {
            v: 1,
            id: new_id()?,
            method: "ping".into(),
            params: json!({}),
        })
        .await?;
    if !ping.ok {
        return Err(ping.error.unwrap());
    }
    let host = ping.result["host_id"]
        .as_str()
        .filter(|id| crate::host::contracts::valid_id(id))
        .ok_or_else(|| pending("Original Host identity unavailable"))?;
    let boot = ping.result["boot_id"]
        .as_str()
        .filter(|id| crate::host::contracts::valid_id(id))
        .ok_or_else(|| pending("Original Host boot identity unavailable"))?;
    if expected.is_some_and(|(h, b)| h != host || b != boot) {
        return Err(pending("The connected Host changed. Refresh its status before explicitly recovering; no stop was requested."));
    }
    let reply = client
        .call(HostRequest {
            v: 1,
            id: new_id()?,
            method: "stop".into(),
            params: json!({"confirm":true,"recover_exited":true}),
        })
        .await?;
    if !reply.ok {
        let error = reply.error.unwrap();
        return Err(if error.code == "ConfirmationRequired" {
            HostError::new("HostRecoveryUpgradeRequired","The running Host predates recovery support. Close the old Host before installing the recovery update; do not terminate a live Worker.")
        } else {
            error
        });
    }
    validate_administrative_stop(&reply.result, host, boot).map(Some)
}
#[tauri::command]
pub async fn host_recover(
    app: tauri::AppHandle,
    state: tauri::State<'_, GuiState>,
    confirm: bool,
) -> Result<Value, HostError> {
    if !confirm {
        return Err(HostError::new(
            "ConfirmationRequired",
            "Explicit Host recovery confirmation required",
        ));
    }
    app.state::<crate::profile::Profile>()
        .require_hardware_host()?;
    let _gate = RecoveryGate::begin(&state)?;
    let directory = crate::profile::config_dir(&app)?.join("host");
    let identity = state.stream_identity.lock().unwrap().clone();
    let endpoint = PipeSecurity::current()?.endpoint();
    let incident=tokio::time::timeout(Duration::from_secs(15),stop_exited_host(&endpoint,identity.as_ref())).await
        .map_err(|_|pending("Host stop outcome is unconfirmed. No command was replayed; retain the previous incident and review Host status."))??;
    let receipt = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            // This retries ONLY machine-guard acquisition, never a Host or instrument command.
            match crate::host::instance::InstanceGuard::recover_exited_matching(
                &directory,
                true,
                incident.as_deref(),
            ) {
                Err(error)
                    if error.code == "HostAlreadyRunning"
                        && incident.is_some()
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(error) => return Err(error),
                Ok(receipt) => return recovery_receipt(receipt, incident.as_deref()),
            }
        }
    })
    .await
    .map_err(|error| pending(error.to_string()))??;
    // This permits administrative GUI detach, not a driver release claim. The durable
    // incident and separate acknowledgment remain available after the next Host start.
    state
        .release_required
        .store(false, std::sync::atomic::Ordering::Release);
    state.startup_pending.store(false, std::sync::atomic::Ordering::Release);
    state.detach();
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn administrative() -> Value {
        json!({"administrative_stop":true,"host_id":"a".repeat(32),"boot_id":"b".repeat(32),
            "incident_id":"c".repeat(32),"process_exit":{"confirmed":true,"success":false,"code":2},
            "resource_released":false,"physical_zero_verified":false})
    }
    fn authorization() -> Value {
        json!({"recovery_authorized":true,"incident_id":"c".repeat(32),"cleanup_verified":false,
            "resource_released":false,"physical_zero_verified":false,"require_readonly_verification":true})
    }
    #[test]
    fn host_recovery_accepts_only_matching_original_host_boot_and_confirmed_exit() {
        let host = "a".repeat(32);
        let boot = "b".repeat(32);
        assert_eq!(
            validate_administrative_stop(&administrative(), &host, &boot).unwrap(),
            "c".repeat(32)
        );
        for (key, value) in [
            ("administrative_stop", json!(false)),
            ("host_id", json!("d".repeat(32))),
            ("boot_id", json!("d".repeat(32))),
            ("incident_id", json!("invalid")),
            ("resource_released", json!(true)),
            ("physical_zero_verified", json!(true)),
            ("process_exit", json!({"confirmed":false})),
        ] {
            let mut report = administrative();
            report[key] = value;
            assert!(
                validate_administrative_stop(&report, &host, &boot).is_err(),
                "{key}"
            );
        }
    }
    #[test]
    fn host_recovery_authorization_is_not_cleanup_or_physical_zero_evidence() {
        let receipt = recovery_receipt(authorization(), Some(&"c".repeat(32))).unwrap();
        assert_eq!(receipt["host_stopped"], true);
        assert_eq!(receipt["cleanup_verified"], false);
        assert_eq!(receipt["resource_released"], false);
        assert_eq!(receipt["physical_zero_verified"], false);
        assert!(
            recovery_receipt(authorization(), None).is_ok(),
            "guarded orphan recovery"
        );
        assert!(recovery_receipt(authorization(), Some(&"d".repeat(32))).is_err());
        for (key, value) in [
            ("recovery_authorized", json!(false)),
            ("incident_id", json!("invalid")),
            ("cleanup_verified", json!(true)),
            ("resource_released", json!(true)),
            ("physical_zero_verified", json!(true)),
            ("require_readonly_verification", json!(false)),
        ] {
            let mut receipt = authorization();
            receipt[key] = value;
            assert!(recovery_receipt(receipt, None).is_err(), "{key}");
        }
    }
    #[test]
    fn host_recovery_unverified_stop_event_does_not_clear_normal_exit_latch() {
        use std::sync::atomic::Ordering;
        let state = GuiState::default();
        let host = "a".repeat(32);
        let boot = "b".repeat(32);
        *state.stream_identity.lock().unwrap() = Some((host.clone(), boot.clone()));
        state.release_required.store(true, Ordering::Release);
        state.observe_host_event(
            &host,
            &boot,
            &json!({"type":"host_stopped_unverified","host_id":host,
            "boot_id":boot,"data":administrative()}),
        );
        assert!(state.release_required.load(Ordering::Acquire));
    }
    #[test]
    fn host_recovery_gate_rejects_overlap_and_resets_on_failure_scope_exit() {
        use std::sync::atomic::Ordering;
        let state = GuiState::default();
        let first = RecoveryGate::begin(&state).unwrap();
        assert!(RecoveryGate::begin(&state).is_err());
        assert!(state.closing.load(Ordering::Acquire));
        drop(first);
        assert!(!state.closing.load(Ordering::Acquire));
        assert!(RecoveryGate::begin(&state).is_ok());
    }
    async fn finite_host(
        stop: Result<Value, HostError>,
    ) -> (String, tokio::task::JoinHandle<Vec<HostRequest>>) {
        let security = PipeSecurity::current().unwrap();
        let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
        let mut pipe = security.create_server(&endpoint, true).unwrap();
        let server = tokio::spawn(async move {
            pipe.connect().await.unwrap();
            let mut seen = Vec::new();
            while let Some(bytes) = crate::host::ipc::read_frame(&mut pipe).await.unwrap() {
                let request: HostRequest = serde_json::from_slice(&bytes).unwrap();
                let last = request.method == "stop";
                let result = if last {
                    stop.clone()
                } else {
                    assert_eq!(request.method, "ping");
                    Ok(
                        json!({"protocol_version":1,"mode":"real","worker_protocol":3,"worker_kind":"rust",
                        "worker_startup_revision":1,"host_id":"a".repeat(32),"boot_id":"b".repeat(32),"attach_token":"e".repeat(32)}),
                    )
                };
                crate::host::ipc::write_frame(
                    &mut pipe,
                    &HostReply::from_result(request.id.clone(), result),
                )
                .await
                .unwrap();
                seen.push(request);
                if last {
                    break;
                }
            }
            seen
        });
        (endpoint, server)
    }
    #[test]
    fn host_recovery_drops_connection_attachment_completed_after_recovery_detach() {
        use std::sync::atomic::Ordering;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (endpoint, server) = finite_host(Ok(administrative())).await;
                let client = Arc::new(LocalHostClient::connect(&endpoint).await.unwrap());
                let pending = Clients {
                    control: client.clone(),
                    background: client.clone(),
                    status: client.clone(),
                    heartbeat: client.clone(),
                    results: client.clone(),
                    safety: client.clone(),
                };
                let state = GuiState::default();
                let admitted_generation = state.connection_generation.load(Ordering::Acquire);
                let gate = RecoveryGate::begin(&state).unwrap();
                state.detach();
                drop(gate); // Last attachment completes after recovery has returned.
                assert!(state.install_clients(pending, admitted_generation).is_err());
                assert!(state.clients.lock().unwrap().is_none());
                drop(client);
                server.await.unwrap();
            });
    }
    #[test]
    fn host_recovery_local_rpc_sends_only_identity_checks_and_one_administrative_stop() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (endpoint, server) = finite_host(Ok(administrative())).await;
                let identity = ("a".repeat(32), "b".repeat(32));
                assert_eq!(
                    stop_exited_host(&endpoint, Some(&identity)).await.unwrap(),
                    Some("c".repeat(32))
                );
                let calls = server.await.unwrap();
                assert_eq!(
                    calls.iter().map(|r| r.method.as_str()).collect::<Vec<_>>(),
                    vec!["ping", "ping", "stop"]
                );
                assert_eq!(
                    calls[2].params,
                    json!({"confirm":true,"recover_exited":true})
                );
            });
    }
    #[test]
    fn host_recovery_boot_change_refuses_stop_before_any_administrative_effect() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (endpoint, server) = finite_host(Ok(administrative())).await;
                assert_eq!(
                    stop_exited_host(&endpoint, Some(&("a".repeat(32), "d".repeat(32))))
                        .await
                        .unwrap_err()
                        .code,
                    "HostRecoveryPending"
                );
                assert_eq!(
                    server
                        .await
                        .unwrap()
                        .iter()
                        .map(|r| r.method.as_str())
                        .collect::<Vec<_>>(),
                    vec!["ping", "ping"]
                );
            });
    }
    #[test]
    fn host_recovery_live_worker_refusal_is_preserved_without_retry() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (endpoint, server) = finite_host(Err(HostError::new(
                    "WorkerStillRunning",
                    "Original worker is alive",
                )))
                .await;
                assert_eq!(
                    stop_exited_host(&endpoint, None).await.unwrap_err().code,
                    "WorkerStillRunning"
                );
                assert_eq!(
                    server
                        .await
                        .unwrap()
                        .iter()
                        .filter(|r| r.method == "stop")
                        .count(),
                    1
                );
            });
    }
}
