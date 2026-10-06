use super::*;

#[test]
fn native_reconciliation_uses_a_fresh_authenticated_observer_not_the_failed_stream() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let dir=std::env::temp_dir().join(format!("yang-tls-recover-{}",new_id().unwrap()));std::fs::create_dir(&dir).unwrap();
        let host="a".repeat(32);let peer_id="b".repeat(32);let boot="c".repeat(32);let session="d".repeat(32);
        let mut owner=crate::remote::RemoteStore::open(dir.join("trust.dpapi"),host.clone()).unwrap();
        let code=owner.begin_pairing().unwrap();let ticket=owner.request_pair(&peer_id,"Client",&code).unwrap();owner.approve(&ticket).unwrap();let credential=owner.take_approved(&ticket).unwrap().unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer=RemotePeer{host_id:host.clone(),name:"Owner".into(),endpoint:listener.local_addr().unwrap().to_string(),fingerprint:owner.fingerprint().unwrap(),peer_id:peer_id.clone(),credential};
        let acceptor=owner.acceptor().unwrap();let proof=owner.release_token(&peer_id,&boot,&session).unwrap();
        let server=tokio::spawn(async move {
            for index in 0..2 {
                let (tcp,_)=listener.accept().await.unwrap();let mut tls=acceptor.accept(tcp).await.unwrap();
                let auth=crate::host::ipc::parse_request(&read_frame_limit(&mut tls,MAX_FRAME).await.unwrap().unwrap()).unwrap();assert_eq!(auth.method,"remote_auth");
                owner.authenticate(auth.params["peer_id"].as_str().unwrap(),auth.params["credential"].as_str().unwrap()).unwrap();
                let current=if index==0 {session.clone()}else{"e".repeat(32)};
                write_frame(&mut tls,&HostReply::from_result(auth.id,Ok(json!({"protocol_version":1,"mode":"real","worker_protocol":3,"host_id":host,"boot_id":boot,"client_session_id":current,"attach_token":"f".repeat(32),"release_token":owner.release_token(&peer_id,&boot,&current).unwrap()})))).await.unwrap();
                let request=crate::host::ipc::parse_request(&read_frame_limit(&mut tls,MAX_FRAME).await.unwrap().unwrap()).unwrap();
                if index==0 {assert_eq!(request.method,"acquire_control");write_frame(&mut tls,&HostReply::from_result("wrong-reply".into(),Ok(json!({})))).await.unwrap();}
                else {
                    assert_eq!(request.method,"reconcile_client");assert_eq!(request.params,json!({"boot_id":boot,"client_session_id":session,"release_token":proof}));
                    write_frame(&mut tls,&HostReply::from_result(request.id,Ok(json!({"host_id":host,"boot_id":boot,"client_session_id":session,"released":true})))).await.unwrap();
                    let close=crate::host::ipc::parse_request(&read_frame_limit(&mut tls,MAX_FRAME).await.unwrap().unwrap()).unwrap();assert_eq!(close.method,"close_client");
                    write_frame(&mut tls,&HostReply::from_result(close.id,Ok(json!({"released":true})))).await.unwrap();
                }
            }
        });
        let original=RemoteHostClient::connect(peer.clone()).await.unwrap();
        assert!(original.call(HostRequest{v:1,id:"original".into(),method:"acquire_control".into(),params:json!({"domain":{"kind":"device","id":"1".repeat(32)}})}).await.is_err());
        let report=original.reconcile(peer).await.unwrap();assert_eq!(report["released"],true);assert_eq!(report["client_session_id"],original.client_session_id);
        server.await.unwrap();std::fs::remove_file(dir.join("trust.dpapi")).unwrap();std::fs::remove_dir(dir).unwrap();
    });
}

#[test]
fn queued_request_is_not_transmitted_after_previous_reply_fails() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let dir=std::env::temp_dir().join(format!("yang-tls-queue-{}",new_id().unwrap()));std::fs::create_dir(&dir).unwrap();
        let mut owner=crate::remote::RemoteStore::open(dir.join("trust.dpapi"),"a".repeat(32)).unwrap();
        let code=owner.begin_pairing().unwrap();let ticket=owner.request_pair(&"b".repeat(32),"Client",&code).unwrap();owner.approve(&ticket).unwrap();
        let credential=owner.take_approved(&ticket).unwrap().unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer=RemotePeer{host_id:"a".repeat(32),name:"Owner".into(),endpoint:listener.local_addr().unwrap().to_string(),fingerprint:owner.fingerprint().unwrap(),peer_id:"b".repeat(32),credential};
        let acceptor=owner.acceptor().unwrap();let (seen_tx,seen_rx)=tokio::sync::oneshot::channel();let (fail_tx,fail_rx)=tokio::sync::oneshot::channel();
        let server=tokio::spawn(async move {
            let (tcp,_)=listener.accept().await.unwrap();let mut tls=acceptor.accept(tcp).await.unwrap();
            let auth=crate::host::ipc::parse_request(&read_frame_limit(&mut tls,MAX_FRAME).await.unwrap().unwrap()).unwrap();
            owner.authenticate(auth.params["peer_id"].as_str().unwrap(),auth.params["credential"].as_str().unwrap()).unwrap();
            write_frame(&mut tls,&HostReply::from_result(auth.id,Ok(json!({"protocol_version":1,"mode":"real","worker_protocol":3,"host_id":"a".repeat(32),"boot_id":"c".repeat(32),"client_session_id":"e".repeat(32),"attach_token":"d".repeat(32),"release_token":"f".repeat(64)})))).await.unwrap();
            read_frame_limit(&mut tls,MAX_FRAME).await.unwrap().unwrap();seen_tx.send(()).unwrap();fail_rx.await.unwrap();
            write_frame(&mut tls,&HostReply::from_result("wrong-reply".into(),Ok(json!({})))).await.unwrap();
            let next=tokio::time::timeout(Duration::from_millis(150),read_frame_limit(&mut tls,MAX_FRAME)).await;
            assert!(!matches!(next,Ok(Ok(Some(_)))),"a second application frame crossed the failed-stream latch");
        });
        let client=std::sync::Arc::new(RemoteHostClient::connect(peer).await.unwrap());
        let first_client=client.clone();let first=tokio::spawn(async move{first_client.call(HostRequest{v:1,id:"first".into(),method:"catalog".into(),params:json!({})}).await});
        seen_rx.await.unwrap();
        let second=client.call(HostRequest{v:1,id:"second".into(),method:"snapshot".into(),params:json!({})});tokio::pin!(second);
        tokio::select! { result=&mut second=>panic!("queued call unexpectedly settled: {result:?}"), _=tokio::time::sleep(Duration::from_millis(1))=>{} }
        fail_tx.send(()).unwrap();assert!(first.await.unwrap().is_err());
        assert!(second.await.is_err());server.await.unwrap();std::fs::remove_file(dir.join("trust.dpapi")).unwrap();std::fs::remove_dir(dir).unwrap();
    });
}

#[test]
fn tls_client_authenticates_and_rejects_mismatched_host_without_exposing_credential() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let dir = std::env::temp_dir().join(format!("yang-tls-client-{}",new_id().unwrap()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("trust.dpapi");
        let mut owner = crate::remote::RemoteStore::open(path.clone(), "a".repeat(32)).unwrap();
        let code = owner.begin_pairing().unwrap();
        let ticket = owner.request_pair(&"b".repeat(32),"Client",&code).unwrap();
        owner.approve(&ticket).unwrap();
        let credential = owner.take_approved(&ticket).unwrap().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let trust = RemotePeer { host_id:"a".repeat(32),name:"Owner".into(),endpoint:listener.local_addr().unwrap().to_string(),fingerprint:owner.fingerprint().unwrap(),peer_id:"b".repeat(32),credential };
        let acceptor = owner.acceptor().unwrap();
        let server = tokio::spawn(async move {
            let (tcp,_) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            let auth = crate::host::ipc::parse_request(&crate::host::ipc::read_frame(&mut tls).await.unwrap().unwrap()).unwrap();
            assert_eq!(auth.method,"remote_auth");
            owner.authenticate(auth.params["peer_id"].as_str().unwrap(),auth.params["credential"].as_str().unwrap()).unwrap();
            crate::host::ipc::write_frame(&mut tls,&HostReply::from_result(auth.id,Ok(json!({"protocol_version":1,"mode":"real","worker_protocol":3,"host_id":"c".repeat(32),"attach_token":"d".repeat(32)})))).await.unwrap();
        });
        let result = RemoteHostClient::connect(trust).await;
        assert!(matches!(result,Err(ref e) if e.code=="RemoteIdentity"));
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    });
}
