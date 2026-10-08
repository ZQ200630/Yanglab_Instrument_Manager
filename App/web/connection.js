import {deviceKey} from './routes.js';
const same=(a,b)=>JSON.stringify(a)===JSON.stringify(b);

function completedCachedRelease(device){
  const safety=device.safety,result=safety?.result,cleanup=result?.cleanup;
  const validId=value=>typeof value==='string'&&/^[a-f0-9]{32}$/.test(value);
  // Last readings can remain connected:true after the transport was closed.
  // Accept only a completed close in this exact released scheduler context.
  return Boolean(device.responsibility===false&&device.pending===0&&
    ['active_request_id','pending_request_id','safety_request_id','readback_request_id'].every(key=>device[key]===null)&&
    safety?.state==='DISCONNECTED'&&safety.phase==='completed'&&safety.error===null&&
    same(safety.context,device.context)&&validId(safety.attempt_id)&&result?.attempt_id===safety.attempt_id&&
    result.connected===false&&result.effective_intent==='disconnect'&&validId(cleanup?.attempt_id)&&
    Array.isArray(cleanup.unreleased)&&cleanup.unreleased.length===0&&
    Array.isArray(cleanup.steps)&&cleanup.steps.length>0&&cleanup.steps.every(step=>step?.error===null));
}

export function connectionReleased(host,device){
  const control=host?.control?.[device?.domain?.kind+':'+device?.domain?.id];
  return Boolean(host?.connected&&host.synced&&control?.state==='AVAILABLE'&&
    device?.state==='DISCONNECTED'&&device.context?.connection_id===null&&
    (!device.device||completedCachedRelease(device)));
}

/** Presentation only. Cached readings and an isolated receipt cannot prove release. */
export function connectionView(host,store,key,local={}){
  const device=store.get(key),control=host?.control?.[device?.domain?.kind+':'+device?.domain?.id];
  const owned=store.canControl(key),available=host?.connected&&host.synced;
  const view=(status,label,operation,disabled=false)=>({status,label,operation,disabled});
  if(!available||!device?.context)return view('Host offline','Connect','connect',true);
  if(local.connecting)return view('Connecting','Connecting…','connect',true);
  const pendingOwner=local.disconnectOwner===control?.controller_session&&local.disconnectBoot===host.bootId&&local.disconnectEpoch===control?.control_epoch;
  if(control?.state==='CONTROLLED'&&!owned&&!pendingOwner)return view('In use · Read only','Disconnect','disconnect',true);
  if(local.disconnectInFlight)return view('Disconnecting','Disconnecting…','disconnect',true);
  if(connectionReleased(host,device)&&!local.unknown)return view('Disconnected','Connect','connect');
  if(local.disconnecting&&performance.now()-local.disconnecting<10000&&
      !['FAULT','STOP_HELD'].includes(device.state))return view('Disconnecting','Disconnecting…','disconnect',true);
  if(control?.state==='RETAINED'||local.unknown||local.disconnectFailed)
    return view('Release unconfirmed','Retry disconnect','disconnect');
  if(device.device||device.context.connection_id)return view(device.device?.connected?'Connected':'Connection unavailable','Disconnect','disconnect',!owned);
  if(owned&&device.state==='DISCONNECTED')return view('Disconnected','Connect','connect');
  return view('Unavailable','Connect','connect',true);
}

/** One-click connection still uses the existing Host lease and context fences. */
export async function ensureInstrumentControl(session,domain,resync,isCurrent=()=>true){
  const {client,store}=session,hostId=session.hostId,key=deviceKey(hostId,domain);
  const before=store.get(key),host=store.host(hostId),boot=host?.bootId;
  if(!host?.connected||!host.synced||!before?.context)throw new Error('Host unavailable. Connection was not started.');
  if(store.lease(key)&&!store.canControl(key)){
    await resync();
    if(!isCurrent()||store.host(hostId)?.bootId!==boot||!same(store.get(key)?.context,before.context))
      throw new Error('Connection context changed. No instrument was opened.');
  }
  if(store.canControl(key))return {lease:store.lease(key),acquired:false};
  if(!connectionReleased(store.host(hostId),store.get(key)))throw new Error('Instrument is in use or awaiting release. This window remains read only.');
  const lease=await client.acquire(domain);
  try{
    if(!isCurrent()||store.host(hostId)?.bootId!==boot||!same(store.get(key)?.context,before.context))
      throw new Error('Connection context changed. No instrument was opened.');
    store.setLease(key,lease);
    await resync();
    if(!isCurrent()||store.host(hostId)?.bootId!==boot||!store.canControl(key)||!same(store.get(key)?.context,before.context))
      throw new Error('Connection authority changed. No instrument was opened.');
    return {lease,acquired:true};
  }catch(error){
    if(store.lease(key)?.token===lease.token)store.dropLease(key);
    // Release this exact token only; never safe-stop a possible new controller.
    try{await client.release(domain,lease);}catch(cleanup){error.cleanupError=cleanup;}
    throw error;
  }
}
