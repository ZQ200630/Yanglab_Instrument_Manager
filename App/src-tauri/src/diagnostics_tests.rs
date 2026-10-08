use super::*;
use crate::{
    host::{
        archive::ArchiveRef,
        ipc::{parse_request, read_frame_limit, write_frame, HostReply, MAX_FRAME},
        leases::DomainRef,
        verification::sha256_bytes,
    },
    remote_client::RemotePeer,
};
use serde_json::json;
#[test]
fn remote_archive_exact_bytes_and_release_receipts() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let dir=std::env::temp_dir().join(format!("yang-diag-tls-{}",crate::host::registry::new_id().unwrap()));std::fs::create_dir(&dir).unwrap();
        let host="a".repeat(32);let peer_id="b".repeat(32);
        let mut owner=crate::remote::RemoteStore::open(dir.join("trust.dpapi"),host.clone()).unwrap();
        let code=owner.begin_pairing().unwrap();let ticket=owner.request_pair(&peer_id,"Diagnostic",&code).unwrap();owner.approve(&ticket).unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer=RemotePeer{host_id:host.clone(),name:"Finite service".into(),endpoint:listener.local_addr().unwrap().to_string(),fingerprint:owner.fingerprint().unwrap(),peer_id:peer_id.clone(),credential:owner.take_approved(&ticket).unwrap().unwrap()};
        let payload:Vec<u8>=[1550.0f64,0.0,1551.0,1e-9].into_iter().flat_map(f64::to_le_bytes).collect();
        let context=json!({"transfer_format":"ASCII","sample_count":2,"spacing":1,"level_unit":1,"x_unit":0,"trace_attribute":0,"active_trace":"TRA","center_m":1.55e-6,"span_m":2e-9,"resolution_m":2e-11,"sweep_mode":1});
        let reference=ArchiveRef{id:"c".repeat(32),name:"osa".into(),host_id:host.clone(),domain:DomainRef{kind:"device".into(),id:"d".repeat(32)},byte_count:32,sample_count:2,sha256:sha256_bytes(&payload).unwrap(),metadata:json!({"native_unit":"W","trace":"A","identity":"YOKOGAWA,AQ6370E,TEST,FW","read_started_at":"2026-10-06T10:00:00+00:00","read_finished_at":"2026-10-06T10:00:01+00:00","elapsed_s":1.,"consistency":"unproven","context_before":context,"context_after":context})};
        let manifest=serde_json::to_vec(&json!({"schema":1,"source_kind":"real","name":"osa","archived_at_unix_ms":1,"origin":{"host_id":host,"domain":reference.domain,"device_identity":{"model":"AQ6370E"},"config_rev":1,"operation_id":reference.id},"descriptor":{"schema":1,"kind":"osa_trace","capture_id":"e".repeat(32),"point_count":2,"byte_count":32,"sha256":reference.sha256,"metadata":reference.metadata}})).unwrap();
        let acceptor=owner.acceptor().unwrap();let native=payload.clone();let original_manifest=manifest.clone();let r=reference.clone();
        let server=tokio::spawn(async move {
            let (tcp,_)=listener.accept().await.unwrap();let mut tls=acceptor.accept(tcp).await.unwrap();
            let auth=parse_request(&read_frame_limit(&mut tls,MAX_FRAME).await.unwrap().unwrap()).unwrap();assert_eq!(auth.method,"remote_auth");owner.authenticate(auth.params["peer_id"].as_str().unwrap(),auth.params["credential"].as_str().unwrap()).unwrap();
            write_frame(&mut tls,&HostReply::from_result(auth.id,Ok(json!({"protocol_version":1,"mode":"real","worker_protocol":3,"worker_kind":"rust","worker_startup_revision":1,"host_id":host,"boot_id":"f".repeat(32),"client_session_id":"1".repeat(32),"attach_token":"2".repeat(32),"release_token":owner.release_token(&peer_id,&"f".repeat(32),&"1".repeat(32)).unwrap()})))).await.unwrap();
            for method in ["worker_status","archive_manifest_bytes","read_archive","close_client"] {
                let req=parse_request(&read_frame_limit(&mut tls,MAX_FRAME).await.unwrap().unwrap()).unwrap();assert_eq!(req.method,method);
                let reply=match method {
                    "worker_status"=>json!({"running":false,"devices":[]}),
                    "archive_manifest_bytes"=>json!({"id":r.id,"name":r.name,"byte_count":manifest.len(),"sha256":sha256_bytes(&manifest).unwrap(),"data_hex":hex(&manifest)}),
                    "read_archive"=>{assert_eq!(req.params["offset"],0);assert_eq!(req.params["length"],32);json!({"id":r.id,"name":r.name,"offset":0,"length":32,"sha256":r.sha256,"data_hex":hex(&payload)})},
                    _=>json!({"released":true,"host_id":host,"client_session_id":"1".repeat(32)}),
                };
                write_frame(&mut tls,&HostReply::from_result(req.id,Ok(reply))).await.unwrap();
            }
        });
        let report=diagnose_peer(peer,Some(reference),&dir).await.unwrap();
        assert!(report.release_confirmed);assert_eq!(report.release_receipt["released"],true);assert!(report.error.is_none());
        assert_eq!(std::fs::read(dir.join("native.bin")).unwrap(),native);assert_eq!(std::fs::read(dir.join("manifest.json")).unwrap(),original_manifest);
        let public=serde_json::to_string(&report).unwrap();assert!(!public.contains("credential"));assert!(!public.contains("release_token"));
        tokio::time::timeout(std::time::Duration::from_secs(5),server).await.unwrap().unwrap();std::fs::remove_dir_all(dir).unwrap();
    });
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
