#[cfg(test)]
#[path = "remote_client_tests.rs"]
mod tests;

use crate::{host::{contracts::HostError,ipc::{HostRequest,HostReply,read_frame_limit,write_frame,parse_request,MAX_FRAME},registry::new_id},remote::{ClientStream,connect_tls}};
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
        parse_request(&serde_json::to_vec(&request).map_err(|_|HostError::new("RemoteProtocol","Invalid request"))?)?;
        if self.failed.load(Ordering::Acquire) { return Err(HostError::new("RemoteOffline","Connection invalid. No operation was retried.")); }
        let mut stream=self.stream.lock().await;
        // A preceding call may have failed while this one waited for the stream.
        if self.failed.load(Ordering::Acquire) { return Err(HostError::new("RemoteOffline","Connection invalid. No operation was retried.")); }
        let reply=tokio::time::timeout(Duration::from_secs(95),async {
            write_frame(&mut *stream,&request).await?;
            let bytes=read_frame_limit(&mut *stream,if request.method=="read_result" {600*1024}else{MAX_FRAME}).await?
                .ok_or_else(||HostError::new("RemoteOffline","Remote Host disconnected"))?;
            let value=crate::runtime::strict_json(&bytes).map_err(|_|HostError::new("RemoteProtocol","Invalid reply"))?;
            let reply:HostReply=serde_json::from_value(value).map_err(|_|HostError::new("RemoteProtocol","Invalid reply"))?;
            if reply.v!=1 || reply.id!=request.id || reply.ok==reply.error.is_some() { return Err(HostError::new("RemoteProtocol","Reply mismatch; outcome unknown")); }
            Ok(reply)
        }).await.unwrap_or_else(|_|Err(HostError::new("OutcomeUnknown","Remote reply timed out; do not repeat the command")));
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
