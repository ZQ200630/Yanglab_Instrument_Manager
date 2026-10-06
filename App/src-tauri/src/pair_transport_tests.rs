use super::*;
use crate::host::ipc::{parse_request, read_frame, write_frame, HostReply};
use std::{path::PathBuf, sync::Arc};
struct Owner {
    trust: Mutex<RemoteStore>,
    generation: AtomicU64,
    stopped: AtomicBool,
    path: PathBuf,
}
impl Owner {
    fn new() -> Arc<Self> {
        let path = std::env::temp_dir()
            .join(format!("pair-tls-{}", new_id().unwrap()))
            .join("trust.dpapi");
        Arc::new(Self {
            trust: Mutex::new(RemoteStore::open(path.clone(), new_id().unwrap()).unwrap()),
            generation: AtomicU64::new(7),
            stopped: AtomicBool::new(false),
            path,
        })
    }
    fn public(&self) -> Value {
        self.trust.lock().unwrap().status()
    }
    fn approve(&self) {
        let id = self.public()["pending"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.trust
            .lock()
            .unwrap()
            .approve_v2(&id, 7, Instant::now())
            .unwrap();
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(self.path.parent().unwrap());
    }
}
fn identity() -> PairIdentity {
    PairIdentity {
        id: new_id().unwrap(),
        name: "Requester".into(),
    }
}
async fn listen(owner: Arc<Owner>) -> (String, tokio::task::JoinHandle<()>) {
    let acceptor = owner.trust.lock().unwrap().acceptor().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let task = tokio::spawn(async move {
        let (tcp, source) = listener.accept().await.unwrap();
        let mut stream = acceptor.accept(tcp).await.unwrap();
        let first = parse_request(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        let _ = serve_pair_v2(
            stream,
            first,
            PairOwner {
                trust: &owner.trust,
                generation: &owner.generation,
                stopped: &owner.stopped,
                name: "Owner".into(),
            },
            source.ip(),
            7,
        )
        .await;
    });
    (endpoint, task)
}
#[tokio::test]
async fn same_channel_comparison_and_approval() {
    let owner = Owner::new();
    let (endpoint, task) = listen(owner.clone()).await;
    let pair = open_pair(&endpoint, identity(), None, &[]).await.unwrap();
    assert_eq!(
        pair.public()["comparison"],
        owner.public()["pending"][0]["comparison"]
    );
    assert!(owner.public()["peers"].as_array().unwrap().is_empty());
    owner.approve();
    let (_tx, rx) = watch::channel(false);
    let peer = pair.finish(rx).await.unwrap();
    task.await.unwrap();
    owner
        .trust
        .lock()
        .unwrap()
        .authenticate(&peer.peer_id, &peer.credential)
        .unwrap();
    let encrypted = std::fs::read(&owner.path).unwrap();
    assert!(!String::from_utf8_lossy(&encrypted).contains(&peer.credential));
    let reopened = RemoteStore::open(owner.path.clone(), peer.host_id.clone()).unwrap();
    reopened
        .authenticate(&peer.peer_id, &peer.credential)
        .unwrap();
    assert!(!peer.public().to_string().contains(&peer.credential));
}
#[tokio::test]
async fn terminated_relay_has_different_comparison() {
    let owner = Owner::new();
    let (endpoint, owner_task) = listen(owner.clone()).await;
    let relay = Owner::new();
    let acceptor = relay.trust.lock().unwrap().acceptor().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_endpoint = listener.local_addr().unwrap().to_string();
    let pin = owner.trust.lock().unwrap().fingerprint().unwrap();
    let forwarding = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut downstream = acceptor.accept(tcp).await.unwrap();
        let mut upstream = crate::remote::connect_tls(&endpoint, &pin).await.unwrap();
        let first = read_frame(&mut downstream).await.unwrap().unwrap();
        let req = parse_request(&first).unwrap();
        write_frame(&mut upstream, &req).await.unwrap();
        let ack = read_frame(&mut upstream).await.unwrap().unwrap();
        let reply: HostReply = serde_json::from_slice(&ack).unwrap();
        write_frame(&mut downstream, &reply).await.unwrap();
        let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
    });
    let pair = open_pair(&relay_endpoint, identity(), None, &[])
        .await
        .unwrap();
    assert_ne!(
        pair.public()["comparison"],
        owner.public()["pending"][0]["comparison"]
    );
    let (tx, rx) = watch::channel(true);
    assert_eq!(
        pair.finish(rx).await.err().unwrap().code,
        "PairingCancelled"
    );
    drop(tx);
    forwarding.await.unwrap();
    owner_task.await.unwrap();
    assert!(owner.public()["peers"].as_array().unwrap().is_empty());
}
#[tokio::test]
async fn pending_pair_has_no_dispatcher_access() {
    let owner = Owner::new();
    let (endpoint, task) = listen(owner.clone()).await;
    let mut pair = open_pair(&endpoint, identity(), None, &[]).await.unwrap();
    write_frame(
        &mut pair.stream,
        &HostRequest {
            v: 1,
            id: new_id().unwrap(),
            method: "catalog".into(),
            params: json!({}),
        },
    )
    .await
    .unwrap();
    let (_tx, rx) = watch::channel(false);
    assert_eq!(pair.finish(rx).await.err().unwrap().code, "PairingProtocol");
    task.await.unwrap();
    assert!(owner.public()["peers"].as_array().unwrap().is_empty());
    assert!(owner.public()["pending"].as_array().unwrap().is_empty());
    for method in ["remote_pair_v2", "remote_reject", "remote_approve"] {
        assert!(!crate::remote::remote_method(method));
    }
}
#[tokio::test]
async fn bootstrap_cancel_expiry_and_listener_change() {
    for mode in 0..3 {
        let owner = Owner::new();
        let (endpoint, task) = listen(owner.clone()).await;
        let mut pair = open_pair(&endpoint, identity(), None, &[]).await.unwrap();
        let (tx, rx) = watch::channel(mode == 0);
        if mode == 1 {
            pair.deadline = tokio::time::Instant::now();
        }
        if mode == 2 {
            owner.generation.store(8, Ordering::Release);
        }
        let e = pair.finish(rx).await.err().unwrap();
        assert_eq!(
            e.code,
            if mode == 0 {
                "PairingCancelled"
            } else {
                "PairingExpired"
            }
        );
        drop(tx);
        task.await.unwrap();
        assert!(owner.public()["peers"].as_array().unwrap().is_empty());
        assert!(owner.public()["pending"].as_array().unwrap().is_empty());
    }
}
async fn incorrect_server(phase: &str) -> String {
    let owner = Owner::new();
    let acceptor = owner.trust.lock().unwrap().acceptor().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let phase = phase.to_owned();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut stream = acceptor.accept(tcp).await.unwrap();
        let first = parse_request(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        let result = json!({"version":2,"phase":phase,"request_id":first.id,"nonce":first.params["nonce"],"ticket":new_id().unwrap(),"peer_id":first.params["peer_id"],"owner_id":owner.trust.lock().unwrap().host_id(),"owner_name":"Owner","expires_in_ms":120000});
        let mut reply = HostReply::from_result(first.id, Ok(result));
        if phase == "Waiting" {
            reply.result["nonce"] = json!("0".repeat(32));
        }
        write_frame(&mut stream, &reply).await.unwrap();
    });
    endpoint
}
#[tokio::test]
async fn bootstrap_protocol_phase_mismatch() {
    for phase in ["Completed", "Waiting"] {
        let endpoint = incorrect_server(phase).await;
        assert_eq!(
            open_pair(&endpoint, identity(), None, &[])
                .await
                .err()
                .unwrap()
                .code,
            "PairingProtocol"
        );
    }
}
#[tokio::test]
async fn bootstrap_terminal_binding_and_nonce_replay_are_rejected() {
    for field in [
        "phase",
        "request_id",
        "nonce",
        "ticket",
        "peer_id",
        "owner_id",
    ] {
        let owner = Owner::new();
        let acceptor = owner.trust.lock().unwrap().acceptor().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let field = field.to_owned();
        let task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(tcp).await.unwrap();
            assert_eq!(
                stream.get_ref().1.handshake_kind(),
                Some(rustls::HandshakeKind::Full)
            );
            let first =
                crate::host::ipc::parse_request(&read_frame(&mut stream).await.unwrap().unwrap())
                    .unwrap();
            let ticket = new_id().unwrap();
            let owner_id = owner.trust.lock().unwrap().host_id().to_owned();
            let ack = json!({"version":2,"phase":"Waiting","request_id":first.id,"nonce":first.params["nonce"],"ticket":ticket,"peer_id":first.params["peer_id"],"owner_id":owner_id,"owner_name":"Owner","expires_in_ms":120000});
            write_frame(
                &mut stream,
                &HostReply::from_result(first.id.clone(), Ok(ack)),
            )
            .await
            .unwrap();
            let mut terminal = json!({"version":2,"phase":"Completed","request_id":first.id,"nonce":first.params["nonce"],"ticket":ticket,"peer_id":first.params["peer_id"],"owner_id":owner_id,"credential":"a".repeat(64)});
            terminal[&field] = json!(if field == "phase" {
                "Waiting".into()
            } else {
                new_id().unwrap()
            });
            write_frame(&mut stream, &HostReply::from_result(first.id, Ok(terminal)))
                .await
                .unwrap();
        });
        let pair = open_pair(&endpoint, identity(), None, &[]).await.unwrap();
        let (_tx, rx) = watch::channel(false);
        assert_eq!(pair.finish(rx).await.err().unwrap().code, "PairingProtocol");
        task.await.unwrap();
    }
}
#[tokio::test]
async fn saved_pin_and_alias_cannot_downgrade() {
    let owner = Owner::new();
    for alias in [false, true] {
        let (endpoint, task) = listen(owner.clone()).await;
        let known = RemotePeer {
            host_id: owner.trust.lock().unwrap().host_id().into(),
            name: "Saved".into(),
            endpoint: if alias {
                "127.0.0.2:9443".into()
            } else {
                endpoint.clone()
            },
            fingerprint: "0".repeat(64),
            peer_id: new_id().unwrap(),
            credential: "a".repeat(64),
        };
        assert!(open_pair(&endpoint, identity(), None, &[known])
            .await
            .is_err()); // never returns new trust
        if !alias {
            task.abort();
        } else {
            task.await.unwrap();
        }
        assert!(owner.public()["peers"].as_array().unwrap().is_empty());
    }
}
#[tokio::test]
async fn four_bootstraps_leave_authenticated_capacity() {
    let slots = Arc::new(tokio::sync::Semaphore::new(4));
    let total = Arc::new(tokio::sync::Semaphore::new(crate::host::ipc::MAX_CHANNELS));
    let mut held = vec![];
    let mut pairs = vec![];
    let mut servers = vec![];
    let mut owners = vec![];
    for _ in 0..4 {
        held.push((
            total.clone().try_acquire_owned().unwrap(),
            bootstrap_permit(&slots).unwrap(),
        ));
        let owner = Owner::new();
        let (endpoint, task) = listen(owner.clone()).await;
        pairs.push(open_pair(&endpoint, identity(), None, &[]).await.unwrap());
        servers.push(task);
        owners.push(owner);
    }
    assert!(bootstrap_permit(&slots).is_err());
    assert_eq!(total.available_permits(), 76);
    let authenticated = total.clone().try_acquire_owned().unwrap();
    drop(authenticated);
    drop(pairs);
    for task in servers {
        task.await.unwrap();
    }
    drop(held);
    assert_eq!(slots.available_permits(), 4);
    assert_eq!(total.available_permits(), 80);
}
#[tokio::test]
async fn old_host_reports_update_required() {
    let owner = Owner::new();
    let acceptor = owner.trust.lock().unwrap().acceptor().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut tls = acceptor.accept(tcp).await.unwrap();
        let _ = read_frame(&mut tls).await;
    });
    assert_eq!(
        open_pair(&endpoint, identity(), None, &[])
            .await
            .err()
            .unwrap()
            .code,
        "PairingUpdateRequired"
    );
    task.await.unwrap();
}
#[tokio::test]
async fn bootstrap_partial_extra_frame_is_rejected_before_approval() {
    use tokio::io::AsyncWriteExt;
    let owner = Owner::new();
    let (endpoint, task) = listen(owner.clone()).await;
    let mut pair = open_pair(&endpoint, identity(), None, &[]).await.unwrap();
    pair.stream.write_all(&[0]).await.unwrap();
    pair.stream.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(125)).await;
    assert!(
        owner.public()["pending"].as_array().unwrap().is_empty(),
        "Extra input cannot be discarded by a cancelled frame reader"
    );
    let (_tx, rx) = watch::channel(false);
    assert_eq!(pair.finish(rx).await.err().unwrap().code, "PairingProtocol");
    task.await.unwrap();
}
