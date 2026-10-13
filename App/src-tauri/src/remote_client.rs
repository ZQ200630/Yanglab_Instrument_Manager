#[cfg(test)]
#[path = "remote_client_tests.rs"]
mod tests;

use crate::{host::{contracts::HostError,ipc::{HostRequest,HostReply,read_frame_limit,write_frame,parse_request},registry::new_id},remote::{ClientStream,connect_tls}};
#[cfg(test)]
use crate::host::ipc::MAX_FRAME;
use serde::{Serialize,Deserialize};
use serde_json::{Value,json};
use std::{sync::atomic::{AtomicBool,Ordering},time::Duration};
use tokio::sync::Mutex;

#[derive(Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemotePeer {
    pub host_id:String, pub name:String, pub endpoint:String, pub fingerprint:String,
    pub peer_id:String, pub credential:String,
}
impl RemotePeer {
    pub fn public(&self)->Value { json!({"host_id":self.host_id,"name":self.name,"endpoint":self.endpoint,"fingerprint":self.fingerprint}) }
}
pub(crate) struct RemoteHostClient {
    stream:Mutex<ClientStream>, pub(crate) peer:RemotePeer, attach_token:String, failed:AtomicBool,
    pub(crate) boot_id:String,pub(crate) client_session_id:String,release_token:String,
}
pub(crate) struct RemoteEvents { pub snapshot:Value, client:RemoteHostClient }
impl RemoteHostClient {
    pub async fn connect(peer:RemotePeer)->Result<Self,HostError> { Self::open(peer,json!({})).await }
    pub async fn attached(&self,channel:&str)->Result<Self,HostError> {
        let client=Self::open(self.peer.clone(),json!({"attach_token":self.attach_token,"channel":channel})).await?;
        if client.boot_id!=self.boot_id||client.client_session_id!=self.client_session_id||client.release_token!=self.release_token {return Err(HostError::new("RemoteIdentity","Attached channel changed session identity"));}Ok(client)
    }
    async fn open(peer:RemotePeer,join:Value)->Result<Self,HostError> {
        let stream = connect_tls(&peer.endpoint,&peer.fingerprint).await?;
        let mut client = Self { stream:Mutex::new(stream),peer,attach_token:String::new(),failed:AtomicBool::new(false),boot_id:String::new(),client_session_id:String::new(),release_token:String::new() };
        let reply = client.call_internal(HostRequest {v:1,id:new_id()?,method:"remote_auth".into(),params:json!({"peer_id":client.peer.peer_id,"credential":client.peer.credential,"join":join})}).await?;
        if !reply.ok { return Err(reply.error.unwrap()); }
        if reply.result["host_id"]!=client.peer.host_id || reply.result["mode"]!="real" || reply.result["worker_protocol"]!=3 || reply.result["protocol_version"]!=1 || !crate::host::contracts::native_host_compatible(&reply.result) {
            return Err(HostError::new("RemoteIdentity","Host identity/protocol changed. Re-pair explicitly."));
        }
        client.attach_token = reply.result["attach_token"].as_str().filter(|s|crate::host::contracts::valid_id(s))
            .ok_or_else(||HostError::new("RemoteProtocol","Missing private channel token"))?.into();
        for (field,target) in [("boot_id",&mut client.boot_id),("client_session_id",&mut client.client_session_id)] {
            *target=reply.result[field].as_str().filter(|s|crate::host::contracts::valid_id(s)).ok_or_else(||HostError::new("RemoteProtocol","Missing session release identity"))?.into();
        }
        client.release_token=reply.result["release_token"].as_str().filter(|s|s.len()==64&&s.bytes().all(|b|b.is_ascii_hexdigit())).ok_or_else(||HostError::new("RemoteProtocol","Missing native release capability"))?.into();
        Ok(client)
    }
    pub(crate) async fn reconcile(&self,peer:RemotePeer)->Result<Value,HostError> {
        if peer.host_id!=self.peer.host_id||peer.peer_id!=self.peer.peer_id||peer.fingerprint!=self.peer.fingerprint {return Err(HostError::new("ReleaseIdentity","Cannot reconcile through another peer identity"));}
        let probe=Self::connect(peer).await?;
        let reply=probe.call(HostRequest{v:1,id:new_id()?,method:"reconcile_client".into(),params:json!({"boot_id":self.boot_id,"client_session_id":self.client_session_id,"release_token":self.release_token})}).await;
        // This fresh authenticated observer never acquires a device or replays an operation.
        let _=probe.call(HostRequest{v:1,id:new_id()?,method:"close_client".into(),params:json!({})}).await;
        let reply=reply?;if reply.ok {Ok(reply.result)}else{Err(reply.error.unwrap())}
    }
    pub async fn call(&self,request:HostRequest)->Result<HostReply,HostError> {
        if !crate::remote::remote_method(&request.method) { return Err(HostError::new("RemoteMethod","Configure the owning Host on its local computer")); }
        self.call_internal(request).await
    }
    async fn call_internal(&self,request:HostRequest)->Result<HostReply,HostError> {
        let deadline=tokio::time::Instant::now()+Duration::from_secs(95);
        let admission_timeout=||HostError::new("AdmissionTimeout","Remote request deadline expired before transmission; request was not sent");
        parse_request(&serde_json::to_vec(&request).map_err(|_|HostError::new("RemoteProtocol","Invalid request"))?)?;
        if self.failed.load(Ordering::Acquire) { return Err(HostError::new("RemoteOffline","Connection invalid. No operation was retried.")); }
        let mut stream=tokio::time::timeout_at(deadline,self.stream.lock()).await.map_err(|_|admission_timeout())?;
        // A preceding call may have failed while this one waited for the stream.
        if self.failed.load(Ordering::Acquire) { return Err(HostError::new("RemoteOffline","Connection invalid. No operation was retried.")); }
        let mut exchange_started=false;
        let outcome=tokio::time::timeout_at(deadline,async {
            // A ready mutex/write must not defeat the absolute admission deadline.
            if tokio::time::Instant::now()>=deadline {return Err(admission_timeout());}
            exchange_started=true;
            let mut exchange=crate::host_client::PendingExchange::new(&self.failed);
            write_frame(&mut *stream,&request).await?;
            let bytes=read_frame_limit(&mut *stream,crate::host::ipc::reply_limit(&request.method)).await?
                .ok_or_else(||HostError::new("RemoteOffline","Remote Host disconnected"))?;
            let value=crate::runtime::strict_json(&bytes).map_err(|_|HostError::new("RemoteProtocol","Invalid reply"))?;
            let reply:HostReply=serde_json::from_value(value).map_err(|_|HostError::new("RemoteProtocol","Invalid reply"))?;
            if reply.v!=1 || reply.id!=request.id || reply.ok==reply.error.is_some() { return Err(HostError::new("RemoteProtocol","Reply mismatch; outcome unknown")); }
            exchange.complete();
            Ok(reply)
        }).await;
        if !exchange_started {return Err(admission_timeout());}
        let reply=outcome.unwrap_or_else(|_|Err(HostError::new("OutcomeUnknown","Remote reply timed out; do not repeat the command")));
        if reply.is_err() { self.failed.store(true,Ordering::Release); }
        reply
    }
    pub async fn subscribe(self)->Result<RemoteEvents,HostError> {
        let reply=self.call(HostRequest{v:1,id:new_id()?,method:"subscribe".into(),params:json!({})}).await?;
        if !reply.ok {return Err(reply.error.unwrap());}
        let snapshot=crate::host::events::read_event(&mut *self.stream.lock().await).await?
            .ok_or_else(||HostError::new("RemoteOffline","Remote snapshot stream closed"))?;
        if snapshot["host_id"]!=self.peer.host_id {return Err(HostError::new("RemoteIdentity","Event belongs to another Host"));}
        Ok(RemoteEvents {snapshot,client:self})
    }
}
impl RemoteEvents {
    pub async fn next(&self)->Result<Option<Value>,HostError> {
        let event=crate::host::events::read_event(&mut *self.client.stream.lock().await).await?;
        if event.as_ref().is_some_and(|e|e["host_id"]!=self.client.peer.host_id) { return Err(HostError::new("RemoteIdentity","Event belongs to another Host")); }
        Ok(event)
    }
}

#[cfg(test)]
mod exchange_boundary_tests {
    use super::*;
    use std::{future::Future, task::Poll};
    use tokio::time::{advance, pause, resume};
    type ServerStream = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;

    fn run(test: impl Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(test);
    }
    fn request(id: &str) -> HostRequest {
        HostRequest { v: 1, id: id.into(), method: "snapshot".into(), params: json!({}) }
    }
    async fn fixture() -> (RemoteHostClient, ServerStream) {
        use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec!["yang-lab".into()]).unwrap();
        let fingerprint = crate::host::verification::sha256_bytes(cert.der().as_ref()).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13]).unwrap().with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()))).unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let accept = async {
            let (tcp, _) = listener.accept().await.unwrap();
            acceptor.accept(tcp).await.unwrap()
        };
        let (stream, server) = tokio::join!(connect_tls(&endpoint, &fingerprint), accept);
        let peer = RemotePeer { host_id:"a".repeat(32), name:"Fixture".into(), endpoint, fingerprint, peer_id:"b".repeat(32), credential:"c".repeat(64) };
        (RemoteHostClient { stream: Mutex::new(stream.unwrap()), peer, attach_token:String::new(), failed:false.into(), boot_id:String::new(), client_session_id:String::new(), release_token:String::new() }, server)
    }
    async fn poll_once<F: Future>(future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
        let mut future = future;
        std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
    }
    async fn observe_request<F: Future>(mut call: std::pin::Pin<&mut F>, server: &mut ServerStream) {
        tokio::select! {
            _ = &mut call => panic!("call completed before the fixture replied"),
            bytes = crate::host::ipc::read_frame(server) => {
                let request: HostRequest = serde_json::from_slice(&bytes.unwrap().unwrap()).unwrap();
                assert_eq!(request.id, "pending");
            }
        }
    }
    async fn confirm_usable(client: &RemoteHostClient, server: &mut ServerStream) {
        let reply = async {
            let bytes = crate::host::ipc::read_frame(server).await.unwrap().unwrap();
            let request: HostRequest = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request.id, "fresh", "an expired or cancelled queued call was transmitted");
            write_frame(server, &HostReply::from_result(request.id, Ok(json!({"fresh":true})))).await.unwrap();
        };
        let (result, ()) = tokio::join!(client.call(request("fresh")), reply);
        assert_eq!(result.unwrap().result["fresh"], true);
        assert!(!client.failed.load(Ordering::Acquire));
    }

    // Missing admission bounds would leave this pending beyond the whole-call deadline.
    #[test]
    fn held_stream_admission_expires_without_transmitting_or_poisoning() {
        run(async {
            let (client, mut server) = fixture().await;
            let guard = client.stream.lock().await;
            pause();
            let mut call = Box::pin(client.call(request("expired")));
            assert!(poll_once(call.as_mut()).await.is_pending());
            advance(Duration::from_secs(96)).await;
            assert!(matches!(poll_once(call.as_mut()).await, Poll::Ready(Err(error)) if error.code == "AdmissionTimeout"));
            drop(call);
            assert!(!client.failed.load(Ordering::Acquire));
            drop(guard);
            resume();
            confirm_usable(&client, &mut server).await;
        });
    }

    // A ready mutex must not win timeout_at's first poll after the deadline.
    #[test]
    fn expired_stream_admission_does_not_send_when_the_mutex_becomes_ready() {
        run(async {
            let (client, mut server) = fixture().await;
            let guard = client.stream.lock().await;
            pause();
            let mut call = Box::pin(client.call(request("expired")));
            assert!(poll_once(call.as_mut()).await.is_pending());
            advance(Duration::from_secs(96)).await;
            drop(guard);
            assert!(matches!(poll_once(call.as_mut()).await, Poll::Ready(Err(error)) if error.code == "AdmissionTimeout"));
            drop(call);
            assert!(!client.failed.load(Ordering::Acquire));
            resume();
            confirm_usable(&client, &mut server).await;
        });
    }

    // Resetting the exchange timer after admission would incorrectly grant another 95 seconds.
    #[test]
    fn stream_queue_and_exchange_share_one_deadline() {
        run(async {
            let (client, mut server) = fixture().await;
            let guard = client.stream.lock().await;
            pause();
            let mut call = Box::pin(client.call(request("pending")));
            assert!(poll_once(call.as_mut()).await.is_pending());
            advance(Duration::from_secs(60)).await;
            drop(guard);
            resume();
            observe_request(call.as_mut(), &mut server).await;
            pause();
            advance(Duration::from_secs(36)).await;
            assert!(matches!(poll_once(call.as_mut()).await, Poll::Ready(Err(error)) if error.code == "OutcomeUnknown"));
            assert!(client.failed.load(Ordering::Acquire));
        });
    }

    // An expired partial reply must fence the stream before any later request can write.
    #[test]
    fn post_send_stream_timeout_poison_prevents_later_transmission() {
        run(async {
            let (client, mut server) = fixture().await;
            let mut call = Box::pin(client.call(request("pending")));
            observe_request(call.as_mut(), &mut server).await;
            tokio::io::AsyncWriteExt::write_all(&mut server, &[20, 0]).await.unwrap();
            tokio::io::AsyncWriteExt::flush(&mut server).await.unwrap();
            pause();
            advance(Duration::from_secs(96)).await;
            assert!(matches!(poll_once(call.as_mut()).await, Poll::Ready(Err(error)) if error.code == "OutcomeUnknown"));
            drop(call);
            assert!(client.failed.load(Ordering::Acquire));
            assert!(matches!(client.call(request("later")).await, Err(error) if error.code == "RemoteOffline"));
            drop(client);
            resume();
            assert!(!matches!(crate::host::ipc::read_frame(&mut server).await, Ok(Some(_))));
        });
    }

    // Dropping a transmitted call must not bypass the failure latch.
    #[test]
    fn dropping_transmitted_stream_call_poison_prevents_later_transmission() {
        run(async {
            let (client, mut server) = fixture().await;
            let mut call = Box::pin(client.call(request("pending")));
            observe_request(call.as_mut(), &mut server).await;
            let mut queued = Box::pin(client.call(request("later")));
            assert!(poll_once(queued.as_mut()).await.is_pending());
            drop(call);
            assert!(client.failed.load(Ordering::Acquire));
            assert!(matches!(poll_once(queued.as_mut()).await, Poll::Ready(Err(error)) if error.code == "RemoteOffline"));
            drop(queued);
            drop(client);
            assert!(!matches!(crate::host::ipc::read_frame(&mut server).await, Ok(Some(_))));
        });
    }

    // Cancelling admission has no uncertain exchange and must leave the channel usable.
    #[test]
    fn dropping_queued_stream_call_does_not_poison_or_transmit() {
        run(async {
            let (client, mut server) = fixture().await;
            let guard = client.stream.lock().await;
            let mut call = Box::pin(client.call(request("cancelled")));
            assert!(poll_once(call.as_mut()).await.is_pending());
            drop(call);
            assert!(!client.failed.load(Ordering::Acquire));
            drop(guard);
            confirm_usable(&client, &mut server).await;
        });
    }
}
