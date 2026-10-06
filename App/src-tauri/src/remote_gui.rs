//! Native peer clients. Web receives public identities and measurements, never trust secrets.
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denied_acquisition_does_not_trap_an_observer_but_unknown_ownership_does() {
        let mut liability=ReleaseState::default();liability.begin();assert!(liability.required());
        let denied=HostReply::from_result("x".into(),Err(HostError::new("ControlOwned","Busy")));liability.finish(Ok(&denied));assert!(!liability.required());
        liability.begin();liability.finish(Err(&HostError::new("OutcomeUnknown","Lost reply")));assert!(liability.required());
        let event=json!({"type":"host_stopped","host_id":"a".repeat(32),"boot_id":"b".repeat(32),"data":{"resource_released":true,"process_exit":{"confirmed":true,"success":true}}});
        liability.observe("a".repeat(32).as_str(),"c".repeat(32).as_str(),&event);assert!(liability.required());
        liability.observe("a".repeat(32).as_str(),"b".repeat(32).as_str(),&event);assert!(!liability.required());
    }
    #[test]
    fn remote_recovery_accepts_only_the_original_bound_cleanup_evidence() {
        let host="a".repeat(32);let boot="b".repeat(32);let session="c".repeat(32);
        let report=json!({"host_id":host,"boot_id":boot,"client_session_id":session,"released":true});
        assert!(reconciled(&report,&host,&boot,&session));
        for key in ["host_id","boot_id","client_session_id"] {let mut wrong=report.clone();wrong[key]=json!("d".repeat(32));assert!(!reconciled(&wrong,&host,&boot,&session));}
        let mut retained=report.clone();retained["released"]=json!(false);assert!(!reconciled(&retained,&host,&boot,&session));
    }
}
use crate::{host::{contracts::HostError,ipc::{HostRequest,HostReply,read_frame,write_frame},registry::new_id,archive::ArchiveRef},remote_client::{RemoteHostClient,RemotePeer},remote::{load_secret,save_secret,connect_tls},gui::GuiState};
use serde_json::{Value,json};
use std::{collections::{BTreeMap,BTreeSet},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},time::Duration,path::PathBuf};
use tauri::{Emitter,Manager};

#[derive(Default)]
struct ReleaseState { liable:bool,pending:usize,host_released:bool }
impl ReleaseState {
    fn begin(&mut self){self.pending+=1;}
    fn finish(&mut self,result:Result<&HostReply,&HostError>){self.pending=self.pending.saturating_sub(1);if result.is_err()||result.is_ok_and(|r|r.ok){self.liable=true;}}
    fn required(&self)->bool{!self.host_released&&(self.liable||self.pending>0)}
    fn observe(&mut self,host:&str,boot:&str,event:&Value){
        if event["type"]=="host_stopped"&&event["host_id"]==host&&event["boot_id"]==boot&&event["data"]["resource_released"]==true&&event["data"]["process_exit"]["confirmed"]==true&&event["data"]["process_exit"]["success"]==true {self.host_released=true;}
    }
}
fn reconciled(report:&Value,host:&str,boot:&str,session:&str)->bool {
    report["released"]==true&&report["host_id"]==host&&report["boot_id"]==boot&&report["client_session_id"]==session
}
struct Clients { control:Arc<RemoteHostClient>,heartbeat:Arc<RemoteHostClient>,results:Arc<RemoteHostClient>,safety:Arc<RemoteHostClient>,release:Mutex<ReleaseState> }
impl Clients {
    fn for_method(&self, method:&str)->Arc<RemoteHostClient> {
        match method {
            "safe_stop"|"release_control"|"close_client"=>self.safety.clone(),
            "read_result"|"operation"|"request_snapshot"|"read_archive"|"list_archives"|"archive_manifest"|"archive_manifest_bytes"=>self.results.clone(),
            "ping"|"renew_control"=>self.heartbeat.clone(),
            _=>self.control.clone(),
        }
    }
}
#[derive(Default)]
pub struct RemoteGuiState {
    clients:Mutex<BTreeMap<String,Arc<Clients>>>, events:Mutex<BTreeMap<String,tauri::async_runtime::JoinHandle<()>>>,
    connecting:Mutex<BTreeSet<String>>, closing:AtomicBool, file_dialog_open:Arc<AtomicBool>,
}
impl RemoteGuiState {
    fn get(&self,id:&str)->Result<Arc<Clients>,HostError> {
        self.clients.lock().unwrap().get(id).cloned().ok_or_else(||HostError::new("RemoteOffline","Connect the owning remote Host in Settings"))
    }
    async fn disconnect(&self,app:&tauri::AppHandle,id:&str)->Result<Value,HostError> {
        let clients=self.clients.lock().unwrap().get(id).cloned();
        let Some(clients)=clients else{return Ok(json!({"released":true,"no_client_release_required":true}));};
        let reply=clients.safety.call(HostRequest{v:1,id:new_id()?,method:"close_client".into(),params:json!({})}).await;
        let result=match reply {
            Ok(reply) if reply.ok && reply.result["released"]==true => reply.result,
            Err(error) if !clients.release.lock().unwrap().required() => json!({"released":true,"no_client_release_required":true,"transport_error":error}),
            _=>{
                let peer=peers(app)?.into_iter().find(|p|p.host_id==id).ok_or_else(||HostError::new("RemoteCleanupRetained","Re-pair the same owning Host before reconciliation"))?;
                let proof=clients.control.reconcile(peer).await.map_err(|error|HostError::new("RemoteCleanupRetained",format!("Old session release is unconfirmed: {}. Check the owning Host, then retry Disconnect; do not replay commands.",error.message)))?;
                if !reconciled(&proof,id,&clients.control.boot_id,&clients.control.client_session_id) {return Err(HostError::new("RemoteCleanupRetained","Old session cleanup remains unconfirmed. Inspect the owning Host and retry Disconnect."));}proof
            },
        };
        if let Some(task)=self.events.lock().unwrap().remove(id) {task.abort();}
        self.clients.lock().unwrap().remove(id);
        Ok(result)
    }
    pub(crate) async fn close_all(&self,app:&tauri::AppHandle)->Result<(),HostError> {
        self.closing.store(true,Ordering::Release);
        let ids:Vec<_>=self.clients.lock().unwrap().keys().cloned().collect();
        for id in ids { if let Err(error)=self.disconnect(app,&id).await {self.cancel_close();return Err(error);} }
        Ok(())
    }
    pub(crate) fn cancel_close(&self){self.closing.store(false,Ordering::Release);}
}
fn peer_path(app:&tauri::AppHandle)->Result<PathBuf,HostError> {
    Ok(crate::profile::config_dir(app)?.join("remote-peers.dpapi"))
}
fn peers(app:&tauri::AppHandle)->Result<Vec<RemotePeer>,HostError> {
    let path=peer_path(app)?; if !path.exists() {return Ok(vec![]);}
    let peers:Vec<RemotePeer>=load_secret(&path)?;
    if peers.len()>16 {return Err(HostError::new("RemoteTrust","Too many peers"));}
    Ok(peers)
}
#[tauri::command]
pub async fn remote_peers(app:tauri::AppHandle)->Result<Value,HostError> {
    Ok(json!(peers(&app)?.iter().map(RemotePeer::public).collect::<Vec<_>>()))
}
#[tauri::command]
pub async fn remote_pair(app:tauri::AppHandle,state:tauri::State<'_,GuiState>,endpoint:String,fingerprint:String,code:String,name:String)->Result<Value,HostError> {
    if code.len()!=6 || !code.bytes().all(|b|b.is_ascii_digit()) || name.is_empty() || name.len()>128 {return Err(HostError::new("PairingRejected","Enter a six-digit pairing code and a short Host name"));}
    let identity=if app.state::<crate::profile::Profile>().network_only {
        crate::profile::app_profile(app.clone()).await?
    }else{
        let local=state.client("ping")?.call(HostRequest{v:1,id:new_id()?,method:"ping".into(),params:json!({})}).await?;
        if !local.ok {return Err(local.error.unwrap());}local.result
    };
    let peer_id=identity["host_id"].as_str().filter(|s|crate::host::contracts::valid_id(s)).ok_or_else(||HostError::new("PairingRejected","Local Host identity unavailable"))?.to_owned();
    let mut tls=connect_tls(&endpoint,&fingerprint).await?;
    let id=new_id()?;
    write_frame(&mut tls,&HostRequest{v:1,id:id.clone(),method:"remote_pair".into(),params:json!({"peer_id":peer_id,"name":identity["host_name"].as_str().or(identity["name"].as_str()).unwrap_or("Remote desktop"),"code":code})}).await?;
    let bytes=tokio::time::timeout(Duration::from_secs(125),read_frame(&mut tls)).await.map_err(|_|HostError::new("PairingExpired","Owner approval timed out"))??.ok_or_else(||HostError::new("PairingRejected","Owner closed pairing"))?;
    let reply:HostReply=serde_json::from_value(crate::runtime::strict_json(&bytes).map_err(|_|HostError::new("RemoteProtocol","Invalid pairing reply"))?).map_err(|_|HostError::new("RemoteProtocol","Invalid pairing reply"))?;
    if reply.v!=1 || reply.id!=id {return Err(HostError::new("RemoteProtocol","Pairing reply mismatch"));}
    if !reply.ok {return Err(reply.error.unwrap_or_else(||HostError::new("PairingRejected","Pairing rejected")));}
    let host_id=reply.result["host_id"].as_str().filter(|s|crate::host::contracts::valid_id(s)&&*s!=peer_id).ok_or_else(||HostError::new("RemoteIdentity","Invalid owning Host identity"))?.to_owned();
    let credential=reply.result["credential"].as_str().filter(|s|s.len()==64&&s.bytes().all(|b|b.is_ascii_hexdigit())).ok_or_else(||HostError::new("RemoteProtocol","Invalid native pairing credential"))?.to_owned();
    let peer=RemotePeer{host_id,name,endpoint,fingerprint,peer_id,credential};
    let mut saved=peers(&app)?; saved.retain(|p|p.host_id!=peer.host_id);
    if saved.len()>=16 {return Err(HostError::new("RemoteTrust","At most sixteen remote Hosts"));}
    let public=peer.public();saved.push(peer);save_secret(&peer_path(&app)?,&saved)?;Ok(public)
}
#[tauri::command]
pub async fn remote_connect(app:tauri::AppHandle,state:tauri::State<'_,RemoteGuiState>,host_id:String)->Result<Value,HostError> {
    if state.closing.load(Ordering::Acquire) {return Err(HostError::new("GuiClosing","Client release is in progress"));}
    if state.clients.lock().unwrap().contains_key(&host_id) {return Err(HostError::new("RemoteConnected","Already connected"));}
    if !state.connecting.lock().unwrap().insert(host_id.clone()) {return Err(HostError::new("RemoteConnected","Connection already in progress"));}
    let result=async {
        let peer=peers(&app)?.into_iter().find(|p|p.host_id==host_id).ok_or_else(||HostError::new("PeerRejected","Pair this Host first"))?;
        let control=Arc::new(RemoteHostClient::connect(peer.clone()).await?);
        let heartbeat=Arc::new(control.attached("heartbeat").await?);
        let results=Arc::new(control.attached("results").await?);
        let safety=Arc::new(control.attached("safety").await?);
        if state.closing.load(Ordering::Acquire) {return Err(HostError::new("GuiClosing","Client release is in progress"));}
        state.clients.lock().unwrap().insert(host_id.clone(),Arc::new(Clients{control,heartbeat,results,safety,release:Mutex::new(ReleaseState::default())}));
        Ok(json!({"connected":true,"host_id":peer.host_id,"mode":"real","worker_protocol":3}))
    }.await;
    state.connecting.lock().unwrap().remove(&host_id);result
}
#[tauri::command]
pub async fn remote_call(state:tauri::State<'_,RemoteGuiState>,host_id:String,request:HostRequest)->Result<HostReply,HostError> {
    if state.closing.load(Ordering::Acquire) && !matches!(request.method.as_str(),"close_client"|"safe_stop"|"release_control"|"operation"|"ping") {return Err(HostError::new("GuiClosing","Client release is in progress"));}
    let clients=state.get(&host_id)?;
    let acquire=request.method=="acquire_control";if acquire {clients.release.lock().unwrap().begin();}
    let result=clients.for_method(&request.method).call(request).await;
    if acquire {clients.release.lock().unwrap().finish(result.as_ref());}result
}
#[tauri::command]
pub async fn remote_subscribe(app:tauri::AppHandle,state:tauri::State<'_,RemoteGuiState>,host_id:String)->Result<(),HostError> {
    if state.events.lock().unwrap().contains_key(&host_id) {return Err(HostError::new("EventSubscription","Already subscribed"));}
    let clients=state.get(&host_id)?;
    let events=clients.control.attached("events").await?.subscribe().await?;
    let id=host_id.clone();
    let task=tauri::async_runtime::spawn(async move {
        let _=app.emit("remote-event",json!({"host_id":id,"event":events.snapshot}));
        loop {match events.next().await {
            Ok(Some(event))=>{clients.release.lock().unwrap().observe(&id,&clients.control.boot_id,&event);let _=app.emit("remote-event",json!({"host_id":id,"event":event}));},
            _=>{let _=app.emit("remote-offline",json!({"host_id":id,"output_state":"UNKNOWN"}));break;},
        }}
    });
    state.events.lock().unwrap().insert(host_id,task);Ok(())
}
#[tauri::command]
pub async fn remote_disconnect(app:tauri::AppHandle,state:tauri::State<'_,RemoteGuiState>,host_id:String)->Result<Value,HostError> {state.disconnect(&app,&host_id).await}
#[tauri::command]
pub async fn remote_forget(app:tauri::AppHandle,state:tauri::State<'_,RemoteGuiState>,host_id:String)->Result<(),HostError> {
    if state.clients.lock().unwrap().contains_key(&host_id) {state.disconnect(&app,&host_id).await?;}
    let mut saved=peers(&app)?;saved.retain(|p|p.host_id!=host_id);save_secret(&peer_path(&app)?,&saved)
}
#[tauri::command]
pub async fn remote_export(window:tauri::Window,state:tauri::State<'_,RemoteGuiState>,reference:ArchiveRef)->Result<Option<Value>,HostError> {
    reference.validate()?;
    let clients=state.get(&reference.host_id)?;
    let guard=crate::native_files::SelectionGuard::begin(&state.file_dialog_open)?;
    let owner=window.hwnd().map_err(|_|HostError::new("NativeWindow","Export window unavailable"))?.0 as isize;
    let (folder,guard)=crate::native_files::choose_folder(owner,"Choose a folder for the native spectrum export",guard).await?;
    let Some(folder)=folder else {return Ok(None);};
    let bundle=crate::native_files::receive_archive(&reference,&reference.host_id,|method,params|{
        let method=method.to_owned();let client=clients.results.clone();
        let available=!state.closing.load(Ordering::Acquire)&&state.clients.lock().unwrap().get(&reference.host_id).is_some_and(|c|Arc::ptr_eq(c,&clients));
        async move {
            if !available {return Err(HostError::new("ArchiveCancelled","Remote client disconnected"));}
            let reply=client.call(HostRequest{v:1,id:new_id()?,method,params}).await?;
            if reply.ok {Ok(reply.result)}else{Err(reply.error.unwrap())}
        }
    }).await?;
    if state.closing.load(Ordering::Acquire) {return Err(HostError::new("ArchiveCancelled","Client is closing"));}
    let directory=tauri::async_runtime::spawn_blocking(move||{let _guard=guard;bundle.write(&folder)}).await.map_err(|_|HostError::new("ArchiveExport","Export task failed"))??;
    Ok(Some(json!({"directory":directory,"id":reference.id,"name":reference.name,"native_sha256":reference.sha256})))
}
