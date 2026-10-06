/** Production uses only authenticated Host APIs; v2 remains a test fixture. */
export async function connectLocalHost(client,config){
  async function attach(){
    const reply=await client.connect();
    if(reply?.connected!==true||reply.mode!=='real'||reply.worker_protocol!==3){
      let cleanupError;
      try{await client.disconnect();}catch(error){cleanupError=error;}
      throw Object.assign(new Error('Upgrade and safely restart the local Host before connecting instruments.'),
        {code:'HostIncompatible',cleanupError});
    }
    return reply;
  }
  try{return await attach();}catch(error){if(error?.code!=='HostAbsent')throw error;}
  try{await client.startHost(config);}catch(error){if(error?.code!=='HostRunning')throw error;}
  return attach();
}
export function createHostClient(invoke,listen){
  if(typeof invoke!=='function'||typeof listen!=='function')throw new TypeError('Native Host bridge required');
  const namespace=globalThis.crypto.randomUUID().replaceAll('-','');let rpc=0,sequence=0;
  async function call(method,params={}){
    const request={v:1,id:`ui-${namespace}-${++rpc}`,method,params};let reply;
    try{reply=await invoke('host_call',{request});}catch(cause){throw Object.assign(new Error(cause?.message||String(cause)),{requestId:request.id,outcomeUnknown:true});}
    if(reply?.v!==1||reply.id!==request.id||typeof reply.ok!=='boolean')throw Object.assign(new Error('Host reply mismatch; outcome unknown'),{outcomeUnknown:true});
    if(!reply.ok)throw Object.assign(new Error(reply.error?.message||'Host request failed'),{code:reply.error?.code,requestId:request.id});
    return reply.result;
  }
  return Object.freeze({
    connect:()=>invoke('host_connect'),
    disconnect:()=>invoke('host_disconnect'),
    startHost:config=>invoke('host_start',{config}),
    preferences:()=>invoke('host_preferences'),
    savePreferences:config=>invoke('host_save_preferences',{config}),
    chooseDataRoot:()=>invoke('choose_data_root'),
    exportArchive:reference=>invoke('export_archive',{reference}),
    stopHost:()=>call('stop',{confirm:true}),
    catalog:()=>call('catalog'),snapshot:()=>call('snapshot'),workerStatus:()=>call('worker_status'),ping:()=>call('ping'),
    remoteStatus:()=>call('remote_status'),remoteListener:endpoint=>call('remote_listener',{endpoint}),
    beginPairing:()=>call('remote_pair_begin'),approvePeer:id=>call('remote_approve',{id}),revokePeer:id=>call('remote_revoke',{id}),
    requestSnapshot:()=>call('request_snapshot'),
    closeClient:()=>call('close_client'),
    acquire:domain=>call('acquire_control',{domain}),renew:token=>invoke('host_heartbeat',{token}),
    release:(domain,lease)=>call('release_control',{domain,token:lease.token,control_epoch:lease.control_epoch}),
    safeStop:domain=>call('safe_stop',{domain}),
    prepare:intent=>call('prepare',{intent}),execute:(requestId,intent)=>call('execute',{request_id:requestId,intent}),
    operation:requestId=>call('operation',{request_id:requestId}),
    listArchives:params=>call('list_archives',params),
    archiveManifest:params=>call('archive_manifest',params),
    archiveManifestBytes:params=>call('archive_manifest_bytes',params),
    readArchive:params=>call('read_archive',params),
    createDraft:params=>call('create_draft',params),testConnection:params=>call('test_connection',params),
    saveDevice:params=>call('save_device',params),cancelDraft:params=>call('cancel_draft',params),
    saveSettings:params=>call('save_settings',params),saveSetup:params=>call('save_setup',params),
    retireSetup:params=>call('retire_setup',params),
    saveCheckPolicy:params=>call('save_check_policy',params),
    renameDevice:params=>call('rename_device',params),retireDevice:params=>call('retire_device',params),
    nextSequence:()=>++sequence,
    readResult:params=>invoke('host_result',{params}),
    async subscribe(onEvent){const unlisten=await listen('host-event',event=>onEvent(event.payload));
      try{await invoke('host_subscribe');return unlisten;}catch(error){unlisten();throw error;}}
  });
}
