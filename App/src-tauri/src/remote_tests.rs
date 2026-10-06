use super::*;
use crate::remote_pairing::PairTranscript;

fn v2(owner:&RemoteStore)->PairTranscript {PairTranscript{version:2,request_id:new_id().unwrap(),nonce:new_id().unwrap(),ticket:new_id().unwrap(),peer_id:new_id().unwrap(),owner_id:owner.host_id().into(),fingerprint:owner.fingerprint().unwrap()}}

#[test]
fn approval_never_rotates_known_peer() {
    let mut owner=store();let now=Instant::now();let t=v2(&owner);let ticket=t.ticket.clone();
    owner.request_v2(t.clone(),"PC","127.0.0.1".parse().unwrap(),7,&[1;32],now).unwrap();
    assert!(owner.take_approved_v2(&ticket,7,now).unwrap().is_none());
    owner.approve_v2(&ticket,7,now).unwrap();let approved=owner.take_approved_v2(&ticket,7,now).unwrap().unwrap();
    assert!(owner.take_approved_v2(&ticket,7,now).is_err());
    owner.authenticate(&t.peer_id,&approved.credential).unwrap();
    let mut retry=t.clone();retry.ticket=new_id().unwrap();retry.nonce=new_id().unwrap();retry.request_id=new_id().unwrap();
    assert!(owner.request_v2(retry,"PC","127.0.0.1".parse().unwrap(),7,&[2;32],now).is_err());
    let reopened=RemoteStore::open(owner.path.clone(),owner.host_id().into()).unwrap();reopened.authenticate(&t.peer_id,&approved.credential).unwrap();
    std::fs::remove_file(owner.path).unwrap();
}
#[test]
fn approval_write_failure_preserves_trust() {
    let mut owner=store();let now=Instant::now();let t=v2(&owner);let original=owner.path.clone();
    owner.request_v2(t.clone(),"PC","127.0.0.1".parse().unwrap(),7,&[1;32],now).unwrap();owner.approve_v2(&t.ticket,7,now).unwrap();
    owner.path=original.join("trust.dpapi"); // Existing file cannot be a directory.
    assert!(owner.take_approved_v2(&t.ticket,7,now).is_err());assert!(owner.trust.peers.is_empty());
    owner.path=original;assert!(owner.take_approved_v2(&t.ticket,7,now).unwrap().is_some());std::fs::remove_file(owner.path).unwrap();
}
#[test]
fn approval_cancel_generation_race() {
    let mut owner=store();let now=Instant::now();let t=v2(&owner);
    owner.request_v2(t.clone(),"PC","127.0.0.1".parse().unwrap(),7,&[1;32],now).unwrap();
    assert!(owner.approve_v2(&t.ticket,8,now).is_err());owner.approve_v2(&t.ticket,7,now).unwrap();owner.cancel_v2(&t.ticket,7,now).unwrap();
    assert!(owner.take_approved_v2(&t.ticket,7,now).is_err());assert!(owner.trust.peers.is_empty());
    let t=v2(&owner);owner.request_v2(t.clone(),"PC","127.0.0.1".parse().unwrap(),7,&[1;32],now).unwrap();
    owner.approve_v2(&t.ticket,7,now).unwrap();let a=owner.take_approved_v2(&t.ticket,7,now).unwrap().unwrap();
    assert!(owner.cancel_v2(&t.ticket,7,now).is_err());owner.authenticate(&t.peer_id,&a.credential).unwrap(); // cancellation cannot promise owner trust removal
    std::fs::remove_file(owner.path).unwrap();
}

#[test]
fn release_proof_is_bound_to_peer_boot_session_and_persisted_verified_stop() {
    let mut trust=store();let peer="b".repeat(32);let boot="c".repeat(32);let session="d".repeat(32);
    let token=trust.release_token(&peer,&boot,&session).unwrap();
    trust.verify_release(&peer,&boot,&session,&token).unwrap();
    for (p,b,s) in [("e".repeat(32),boot.clone(),session.clone()),(peer.clone(),"e".repeat(32),session.clone()),(peer.clone(),boot.clone(),"e".repeat(32))] {assert!(trust.verify_release(&p,&b,&s,&token).is_err());}
    assert!(trust.released_boot(&boot).is_none());
    assert!(trust.record_release(&boot,&json!({"resource_released":false,"process_exit":{"confirmed":true,"success":true}})).is_err());
    assert!(trust.record_release(&boot,&json!({"resource_released":true,"process_exit":{"confirmed":false,"success":true}})).is_err());
    trust.record_release(&boot,&json!({"resource_released":true,"process_exit":{"confirmed":true,"success":true}})).unwrap();
    let reopened=RemoteStore::open(trust.path.clone(),trust.host_id().to_owned()).unwrap();
    reopened.verify_release(&peer,&boot,&session,&token).unwrap();assert_eq!(reopened.released_boot(&boot).unwrap()["resource_released"],true);
    std::fs::remove_file(trust.path).unwrap();
}

#[test]
fn authenticated_admission_serializes_session_insertion_with_revocation() {
    use std::sync::{Mutex,atomic::{AtomicU64,Ordering},mpsc};
    let mut trust=store();let code=trust.begin_pairing().unwrap();let peer="b".repeat(32);
    let ticket=trust.request_pair(&peer,"Client",&code).unwrap();trust.approve(&ticket).unwrap();let credential=trust.take_approved(&ticket).unwrap().unwrap();
    let remote=Arc::new(Mutex::new(trust));let clients=Arc::new(Mutex::new(crate::host::sessions::ClientSessions::new("c".repeat(32))));let generation=Arc::new(AtomicU64::new(0));
    let (entered_tx,entered_rx)=mpsc::channel();let (continue_tx,continue_rx)=mpsc::channel();
    let (r,c,g,p,secret)=(remote.clone(),clients.clone(),generation.clone(),peer.clone(),credential.clone());
    let admitting=std::thread::spawn(move||authenticated_admission(&r,&g,0,&p,&secret,||{entered_tx.send(()).unwrap();continue_rx.recv().unwrap();c.lock().unwrap().join_peer(&json!({}),&p)}));
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(remote.try_lock().is_err(),"authentication lock was released before insertion");
    let (r,c,g,p)=(remote.clone(),clients.clone(),generation.clone(),peer.clone());
    let revoking=std::thread::spawn(move||{let mut trust=r.lock().unwrap();trust.revoke(&p).unwrap();g.fetch_add(1,Ordering::AcqRel);c.lock().unwrap().revoke_peer(&p);});
    continue_tx.send(()).unwrap();let admitted=admitting.join().unwrap().unwrap();revoking.join().unwrap();
    assert!(!clients.lock().unwrap().active(admitted.session.id()));
    assert!(authenticated_admission(&remote,&generation,0,&peer,&credential,||clients.lock().unwrap().join_peer(&json!({}),&peer)).is_err());
    std::fs::remove_file(&remote.lock().unwrap().path).unwrap();
}

#[test]
fn remote_commands_cannot_reconfigure_or_stop_the_owning_host() {
    for command in ["ping", "subscribe", "snapshot", "prepare", "execute", "read_result", "read_archive", "renew_control", "safe_stop", "close_client"] {
        assert!(remote_method(command), "{command}");
    }
    for command in ["stop", "save_device", "test_connection", "save_settings", "remote_listener", "remote_approve", "worker_request", "raw"] {
        assert!(!remote_method(command), "{command}");
    }
}

#[test]
fn real_tls_pin_mismatch_rejects_before_application_frame() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let owner = store();
        let acceptor = owner.acceptor().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (tcp,_) = listener.accept().await.unwrap();
            match acceptor.accept(tcp).await {
                Ok(mut tls) => crate::host::ipc::read_frame(&mut tls).await.ok().flatten().is_none(),
                Err(_) => true,
            }
        });
        assert_eq!(connect_tls(&address, &"0".repeat(64)).await.unwrap_err().code, "RemoteTls");
        assert!(server.await.unwrap());
        std::fs::remove_file(&owner.path).unwrap();
    });
}

fn store() -> RemoteStore {
    let dir = std::env::temp_dir().join(format!("yang-remote-{}", new_id().unwrap()));
    std::fs::create_dir(&dir).unwrap();
    RemoteStore::open(dir.join("trust.dpapi"), "a".repeat(32)).unwrap()
}

#[test]
fn endpoint_rejects_public_lan_wildcard_and_zero_port() {
    for bad in ["0.0.0.0:443", "192.168.1.4:443", "8.8.8.8:443", "127.0.0.1:0", "localhost:443", "100.128.0.1:443"] {
        assert!(endpoint(bad).is_err(), "{bad}");
    }
    for good in ["127.0.0.1:8767", "100.65.2.3:8767", "[::1]:8767", "[fd7a:115c:a1e0::2]:8767"] { assert!(endpoint(good).is_ok()); }
}

#[test]
fn pairing_requires_approval_and_credentials_survive_encrypted_restart() {
    let mut owner = store();
    assert!(owner.status()["listener"].is_null());
    let code = owner.begin_pairing().unwrap();
    let peer = "b".repeat(32);
    let ticket = owner.request_pair(&peer, "Other desktop", &code).unwrap();
    assert!(owner.take_approved(&ticket).unwrap().is_none());
    owner.approve(&ticket).unwrap();
    let credential = owner.take_approved(&ticket).unwrap().unwrap();
    assert!(owner.authenticate(&peer, &credential).is_ok());
    assert!(owner.authenticate(&peer, &"0".repeat(64)).is_err());
    assert!(owner.authenticate(&"c".repeat(32), &credential).is_err());
    let saved = std::fs::read(&owner.path).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains(&credential));
    let mut reopened = RemoteStore::open(owner.path.clone(), "a".repeat(32)).unwrap();
    assert_eq!(reopened.fingerprint().unwrap(), owner.fingerprint().unwrap());
    assert!(reopened.authenticate(&peer, &credential).is_ok());
    reopened.revoke(&peer).unwrap();
    assert!(reopened.authenticate(&peer, &credential).is_err());
    std::fs::remove_file(&owner.path).unwrap();
}

#[test]
fn pairing_expiry_and_failed_attempts_do_not_create_trust() {
    let mut owner = store();
    let code = owner.begin_pairing().unwrap();
    for _ in 0..5 { assert!(owner.request_pair(&"b".repeat(32), "Peer", "xxxxxx").is_err()); }
    assert!(owner.request_pair(&"b".repeat(32), "Peer", &code).is_err());
    let code = owner.begin_pairing().unwrap();
    owner.pairing.as_mut().unwrap().deadline = std::time::Instant::now();
    assert!(owner.request_pair(&"b".repeat(32), "Peer", &code).is_err());
    std::fs::remove_file(&owner.path).unwrap();
}
