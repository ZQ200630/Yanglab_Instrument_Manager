use super::*;
use crate::host::ipc::{read_frame, write_frame};
use std::future::Future;

fn run(test: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all().build().unwrap().block_on(test);
}
fn fixture_pipe() -> (String, tokio::net::windows::named_pipe::NamedPipeServer) {
    let security = PipeSecurity::current().unwrap();
    let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
    let server = security.create_server(&endpoint, true).unwrap();
    (endpoint, server)
}
fn healthy_host() -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let (endpoint, mut server) = fixture_pipe();
    let task = tokio::spawn(async move {
        server.connect().await.unwrap();
        let mut methods = Vec::new();
        while let Some(bytes) = read_frame(&mut server).await.unwrap() {
            let request: HostRequest = serde_json::from_slice(&bytes).unwrap();
            methods.push(request.method.clone());
            write_frame(&mut server, &HostReply::from_result(request.id, Ok(json!({
                "protocol_version":1,"mode":"real","worker_protocol":3,"worker_kind":"rust",
                "worker_startup_revision":1,"host_id":"a".repeat(32),"boot_id":"b".repeat(32),
                "attach_token":"c".repeat(32)
            })))).await.unwrap();
        }
        methods
    });
    (endpoint, task)
}

#[test]
fn local_host_startup_silent_existing_owner_is_bounded_and_never_absent() {
    run(async {
        let state = GuiState::default();
        let (endpoint, mut server) = fixture_pipe();
        let task = tokio::spawn(async move {
            server.connect().await.unwrap();
            let request: HostRequest = serde_json::from_slice(&read_frame(&mut server).await.unwrap().unwrap()).unwrap();
            assert_eq!(request.method, "ping");
            std::future::pending::<()>().await;
        });
        let deadline = tokio::time::Instant::now() + Duration::from_millis(75);
        let result = tokio::time::timeout(Duration::from_millis(250), startup_preflight(&state, &endpoint, deadline)).await;
        task.abort();
        assert_eq!(result.expect("startup handshake exceeded its overall deadline").unwrap_err().code, "HostStartPending");
    });
}

#[test]
fn local_host_startup_elapsed_deadline_sends_no_handshake() {
    run(async {
        let state = GuiState::default();
        let (endpoint, mut server) = fixture_pipe();
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            server.connect().await.unwrap();
            let bytes = read_frame(&mut server).await.unwrap().unwrap();
            let request: HostRequest = serde_json::from_slice(&bytes).unwrap();
            seen_tx.send(request.method).unwrap();
            std::future::pending::<()>().await;
        });
        let deadline = tokio::time::Instant::now() - Duration::from_millis(1);
        let result = tokio::time::timeout(Duration::from_millis(150), startup_preflight(&state, &endpoint, deadline)).await;
        task.abort();
        assert_eq!(result.expect("expired startup deadline admitted a handshake").unwrap_err().code, "HostStartPending");
        assert!(seen_rx.try_recv().is_err(), "expired startup emitted a new RPC");
    });
}

#[test]
fn local_host_startup_uncertain_owner_fences_later_absent_endpoint() {
    run(async {
        let state = GuiState::default();
        let (endpoint, mut server) = fixture_pipe();
        let task = tokio::spawn(async move {
            server.connect().await.unwrap();
            let _ = read_frame(&mut server).await.unwrap();
            std::future::pending::<()>().await;
        });
        let _ = tokio::time::timeout(Duration::from_millis(200), startup_preflight(
            &state, &endpoint, tokio::time::Instant::now() + Duration::from_millis(50),
        )).await;
        task.abort();
        let _ = task.await;
        let result = startup_preflight(&state, &endpoint, tokio::time::Instant::now() + Duration::from_millis(50)).await;
        assert_eq!(result.expect_err("uncertain original owner permitted another spawn").code, "HostStartPending");
    });
}

#[test]
fn local_host_startup_healthy_existing_owner_is_reused_with_only_ping() {
    run(async {
        let state = GuiState::default();
        state.release_required.store(true, std::sync::atomic::Ordering::Release);
        let (endpoint, task) = healthy_host();
        let result = startup_preflight(&state, &endpoint, tokio::time::Instant::now() + Duration::from_secs(1)).await;
        assert_eq!(result.unwrap_err().code, "HostRunning");
        assert_eq!(task.await.unwrap(), vec!["ping"]);
        assert!(state.release_required.load(std::sync::atomic::Ordering::Acquire), "read-only reuse cleared cleanup evidence");
    });
}

#[test]
fn local_host_startup_genuine_absence_permits_only_initial_spawn_admission() {
    run(async {
        let state = GuiState::default();
        let security = PipeSecurity::current().unwrap();
        let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
        startup_preflight(&state, &endpoint, tokio::time::Instant::now() + Duration::from_secs(1)).await.unwrap();
        assert!(!state.startup_pending.load(std::sync::atomic::Ordering::Acquire));
    });
}

#[test]
fn local_host_startup_readiness_absence_and_silent_handshake_share_one_deadline() {
    run(async {
        let security = PipeSecurity::current().unwrap();
        let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
        let delayed_endpoint = endpoint.clone();
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            // Initial readiness is absent; the next poll reaches a silent owner.
            tokio::time::sleep(Duration::from_millis(30)).await;
            let mut server = security.create_server(&delayed_endpoint, true).unwrap();
            server.connect().await.unwrap();
            let request: HostRequest = serde_json::from_slice(&read_frame(&mut server).await.unwrap().unwrap()).unwrap();
            seen_tx.send(request.method).unwrap();
            std::future::pending::<()>().await;
        });
        let (_send, receive) = std::sync::mpsc::sync_channel(1);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(180);
        let result = tokio::time::timeout(Duration::from_millis(350), startup_readiness(
            &endpoint, deadline, receive, || Ok(None),
        )).await;
        task.abort();
        assert_eq!(result.expect("readiness renewed the startup deadline").unwrap_err().code, "HostStartPending");
        assert_eq!(seen_rx.try_recv().unwrap(), "ping");
        assert!(seen_rx.try_recv().is_err(), "uncertain handshake was replayed");
    });
}

#[test]
fn local_host_startup_and_recovery_share_one_admission_gate() {
    let state = GuiState::default();
    let start = recovery_ui::RecoveryGate::begin(&state).unwrap();
    assert_eq!(recovery_ui::RecoveryGate::begin(&state).err().unwrap().code, "GuiClosing");
    drop(start);
    assert!(recovery_ui::RecoveryGate::begin(&state).is_ok());
}

fn clients(client: &Arc<LocalHostClient>) -> Clients {
    Clients {
        control: client.clone(), background: client.clone(), status: client.clone(),
        heartbeat: client.clone(), results: client.clone(), safety: client.clone(),
    }
}

#[test]
fn local_host_startup_fence_survives_detach_and_stale_connection_installation() {
    run(async {
        use std::sync::atomic::Ordering;
        let state = GuiState::default();
        let (endpoint, task) = healthy_host();
        let client = Arc::new(LocalHostClient::connect(&endpoint).await.unwrap());
        state.startup_pending.store(true, Ordering::Release);
        let old_generation = state.connection_generation.load(Ordering::Acquire);
        state.detach();
        assert!(state.startup_pending.load(Ordering::Acquire));
        assert!(state.install_clients(clients(&client), old_generation).is_err());
        assert!(state.startup_pending.load(Ordering::Acquire));
        let generation = state.connection_generation.load(Ordering::Acquire);
        state.install_clients(clients(&client), generation).unwrap();
        assert!(!state.startup_pending.load(Ordering::Acquire));
        state.detach();
        drop(client);
        assert_eq!(task.await.unwrap(), vec!["ping"]);
    });
}

#[test]
fn local_host_startup_failed_close_retains_cleanup_and_startup_evidence() {
    run(async {
        use std::sync::atomic::Ordering;
        let state = GuiState::default();
        let (endpoint, task) = healthy_host();
        let client = Arc::new(LocalHostClient::connect(&endpoint).await.unwrap());
        state.install_clients(clients(&client), 0).unwrap();
        state.release_required.store(true, Ordering::Release);
        state.startup_pending.store(true, Ordering::Release);
        // The fixture's reply omits release evidence, so an explicit close must retain ownership.
        assert_eq!(state.close_connection().await.unwrap_err().code, "ClientCleanupRetained");
        assert!(state.release_required.load(Ordering::Acquire));
        assert!(state.startup_pending.load(Ordering::Acquire));
        assert!(state.clients.lock().unwrap().is_some());
        state.detach();
        drop(client);
        assert_eq!(task.await.unwrap(), vec!["ping", "close_client"]);
    });
}

#[test]
fn local_host_startup_observed_pending_host_does_not_claim_gui_owned_resources() {
    run(async {
        use std::sync::atomic::Ordering;
        let state = GuiState::default();
        state.startup_pending.store(true, Ordering::Release);
        let report = state.close_connection().await.unwrap();
        assert_eq!(report["released"], true, "GUI had no client-owned resource to retain");
        assert_eq!(report["host_stopped"], false);
        assert!(state.startup_pending.load(Ordering::Acquire), "GUI detach authorized a second Host spawn");
    });
}

#[test]
fn local_host_subscription_late_install_does_not_replace_a_new_session() {
    run(async {
        use std::sync::atomic::{AtomicBool, Ordering};
        let state = GuiState::default();
        let (old_endpoint, old_server) = healthy_host();
        let old = Arc::new(LocalHostClient::connect(&old_endpoint).await.unwrap());
        state.install_clients(clients(&old), 0).unwrap();
        state.detach();
        let generation = state.connection_generation.load(Ordering::Acquire);
        let (new_endpoint, new_server) = healthy_host();
        let current = Arc::new(LocalHostClient::connect(&new_endpoint).await.unwrap());
        state.install_clients(clients(&current), generation).unwrap();
        let identity = ("d".repeat(32), "e".repeat(32));
        state.install_subscription(&current, generation, identity.clone(), || {
            tauri::async_runtime::spawn(async {})
        }).unwrap();
        let spawned = AtomicBool::new(false);
        let result = state.install_subscription(&old, 0, ("a".repeat(32), "b".repeat(32)), || {
            spawned.store(true, Ordering::Release);
            tauri::async_runtime::spawn(async {})
        });
        assert_eq!(result.unwrap_err().code, "HostConnectionChanged");
        assert!(!spawned.load(Ordering::Acquire), "late stream task was started");
        assert_eq!(*state.stream_identity.lock().unwrap(), Some(identity));
        state.detach();
        drop(old);
        drop(current);
        old_server.await.unwrap();
        new_server.await.unwrap();
    });
}

#[test]
fn local_host_subscription_stale_events_and_offline_cannot_reach_a_new_session() {
    run(async {
        use std::sync::atomic::Ordering;
        let state = GuiState::default();
        let (old_endpoint, old_server) = healthy_host();
        let old = Arc::new(LocalHostClient::connect(&old_endpoint).await.unwrap());
        state.install_clients(clients(&old), 0).unwrap();
        state.detach();
        let generation = state.connection_generation.load(Ordering::Acquire);
        let (new_endpoint, new_server) = healthy_host();
        let current = Arc::new(LocalHostClient::connect(&new_endpoint).await.unwrap());
        state.install_clients(clients(&current), generation).unwrap();
        state.install_subscription(&current, generation, ("d".repeat(32), "e".repeat(32)), || {
            tauri::async_runtime::spawn(async {})
        }).unwrap();
        let mut emitted = Vec::new();
        assert!(!state.with_subscription(&old, 0, || emitted.push("old event")));
        assert!(!state.with_subscription(&old, generation, || emitted.push("old offline")));
        assert!(state.with_subscription(&current, generation, || emitted.push("current event")));
        assert_eq!(emitted, vec!["current event"]);
        state.detach();
        drop(old);
        drop(current);
        old_server.await.unwrap();
        new_server.await.unwrap();
    });
}

fn staged_attachment_host() -> (
    String,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    Vec<tokio::task::JoinHandle<()>>,
) {
    let security = PipeSecurity::current().unwrap();
    let endpoint = format!("{}-test-{}", security.endpoint(), new_id().unwrap());
    let mut first = security.create_server(&endpoint, true).unwrap();
    let server_endpoint = endpoint.clone();
    let (first_seen_tx, first_seen_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (second_seen_tx, second_seen_rx) = tokio::sync::mpsc::unbounded_channel();
    let first_task = tokio::spawn(async move {
        first.connect().await.unwrap();
        let request: HostRequest = serde_json::from_slice(&read_frame(&mut first).await.unwrap().unwrap()).unwrap();
        let mut second = security.create_server(&server_endpoint, false).unwrap();
        first_seen_tx.send(()).unwrap();
        let _ = release_rx.await;
        write_frame(&mut first, &HostReply::from_result(request.id, Ok(json!({
            "protocol_version":1,"mode":"real","worker_protocol":3,"worker_kind":"rust",
            "worker_startup_revision":1,"host_id":"a".repeat(32),"boot_id":"b".repeat(32),
            "attach_token":"c".repeat(32)
        })))).await.unwrap();
        second.connect().await.unwrap();
        let request: HostRequest = serde_json::from_slice(&read_frame(&mut second).await.unwrap().unwrap()).unwrap();
        second_seen_tx.send(request.params["channel"].as_str().unwrap().to_owned()).unwrap();
        std::future::pending::<()>().await;
    });
    (endpoint, first_seen_rx, release_tx, second_seen_rx, vec![first_task])
}

#[test]
fn local_host_attachment_late_control_reply_cannot_start_another_channel() {
    run(async {
        let state = Arc::new(GuiState::default());
        let (endpoint, first_seen, release, mut second_seen, servers) = staged_attachment_host();
        let task_state = state.clone();
        let connect = tokio::spawn(async move {
            connect_clients(&task_state, &endpoint, 0, tokio::time::Instant::now() + Duration::from_secs(1)).await
        });
        first_seen.await.unwrap();
        state.detach();
        release.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_millis(250), connect).await;
        for server in servers { server.abort(); }
        assert_eq!(result.expect("late control reply continued opening channels").unwrap().err().unwrap().code, "HostConnectionChanged");
        assert!(second_seen.try_recv().is_err(), "late control reply sent another attachment ping");
    });
}

#[test]
fn local_host_attachment_channels_share_one_native_deadline() {
    run(async {
        let state = GuiState::default();
        let (endpoint, _first_seen, release, mut second_seen, servers) = staged_attachment_host();
        release.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_millis(250), connect_clients(
            &state, &endpoint, 0, tokio::time::Instant::now() + Duration::from_millis(75),
        )).await;
        for server in servers { server.abort(); }
        assert_eq!(result.expect("attachment handshake renewed the native deadline").err().unwrap().code, "HostStartPending");
        assert!(state.startup_pending.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(second_seen.try_recv().unwrap(), "heartbeat");
        assert!(second_seen.try_recv().is_err());
    });
}
