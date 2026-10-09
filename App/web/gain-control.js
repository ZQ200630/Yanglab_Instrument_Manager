import {gainEvidence,gainFields} from './control-state.js';
import {deviceKey} from './routes.js';
import {pollOriginalOperation,queryOriginalOperation} from './operation-recovery.js';
import {startActivity,advanceActivity,finishActivity} from './activity.js';

const same=(a,b)=>JSON.stringify(a)===JSON.stringify(b);
const binding=(boot,context)=>JSON.stringify([boot,context?.session_id,context?.connection_id]);
export const gainDraftIds=['gain-temp','gain-current','gain-pid-p','gain-pid-i','gain-pid-d','gain-soft-start','gain-smooth-change','gain-ramp-step','gain-ramp-interval-ms','gain-stable-timeout'];
function readyControl(role,device,nowMs){
 return Boolean(role?.confirmed&&role.context?.connection_id&&['READY','ACTIVE'].includes(role.mode)&&
  !role.normalPending&&!role.safetyPending&&!role.activeRequest&&!role.unknown&&!role.hostRestricted&&!role.stopHeld&&
  device?.connected===true&&['READY','ACTIVE'].includes(device.state)&&!device.status_error&&
  gainFields.every(name=>{const field=gainEvidence(device,name,nowMs);return field.quality==='fresh'&&field.connection_id===role.context.connection_id&&
   (name.endsWith('_enabled')?typeof field.value==='boolean':Number.isFinite(field.value));}));
}
export function gainCanResume(role,device,nowMs=performance.now()){
 const safety=role?.safety,result=safety?.result;
 return Boolean(role?.mode==='STOP_HELD'&&readyControl({...role,mode:'READY',stopHeld:false},device,nowMs)&&
  safety?.state==='STOP_HELD'&&safety.phase==='completed'&&safety.error===null&&same(safety.context,role.context)&&
  /^[a-f0-9]{32}$/.test(safety.attempt_id)&&result?.attempt_id===safety.attempt_id&&['current_off','tec_off'].includes(result.effective_intent)&&
  device.fields.current_enabled.value===false&&(result.effective_intent!=='tec_off'||device.fields.tec_enabled.value===false));
}
export function gainCanOperate(role,device,nowMs=performance.now()){return readyControl(role,device,nowMs)||gainCanResume(role,device,nowMs);}
export function gainCanStart(role,device,nowMs=performance.now()){
 return gainCanOperate(role,device,nowMs)&&device.fields.tec_enabled.value===true&&device.fields.current_enabled.value===false;
}
export function gainCanStop(role){return Boolean(role?.confirmed&&!role.hostRestricted&&role.context?.session_id&&role.context.connection_id);}
function number(get,id,min,max){const raw=get(id);if(typeof raw!=='string'||!raw.trim())throw new Error('Enter a Gain control value.');const value=Number(raw);if(!Number.isFinite(value)||value<min||value>max)throw new Error(`Gain value must be ${min}–${max}.`);return value;}
function checked(get,id){const value=get(id);if(typeof value!=='boolean')throw new Error('Gain option state is unavailable.');return value;}
function ramp(get){return {step_ma:number(get,'gain-ramp-step',.001,1),interval_s:number(get,'gain-ramp-interval-ms',50,180000)/1000};}
export function gainAction(op,get,device){
 if(op==='gain-read-pid')return {name:'read_pid',args:{}};
 if(op==='gain-set-pid')return {name:'set_pid',args:{p:number(get,'gain-pid-p',0,999.999),i:number(get,'gain-pid-i',0,999.999),d:number(get,'gain-pid-d',0,999.999)}};
 if(op==='gain-set-temp')return {name:'set_temperature',args:{temperature_c:number(get,'gain-temp',15,40)}};
 if(op==='gain-enable-current')return {name:'start_current',args:{current_ma:number(get,'gain-current',0,200),soft_start:checked(get,'gain-soft-start'),...ramp(get),timeout_s:number(get,'gain-stable-timeout',.05,180)}};
 if(op==='gain-set-current'){
  const field=gainEvidence(device,'current_enabled');if(field.quality!=='fresh'||typeof field.value!=='boolean')throw new Error('Current output state is unknown. Check confirmed status before changing current.');
  const current_ma=number(get,'gain-current',0,200);return field.value&&checked(get,'gain-smooth-change')?{name:'ramp_current',args:{current_ma,...ramp(get)}}:{name:'set_current',args:{current_ma}};
 }
 const names={'gain-enable-tec':'enable_tec','gain-disable-tec':'disable_tec','gain-disable-current':'disable_current','gain-stable':'wait_stable'};
 if(names[op])return {name:names[op],args:{}};
 throw new Error('Unsupported Gain action.');
}
export function syncGainDraft(local,domain,bootId,ageUpperMs,nowMs=performance.now()){
 const next=binding(bootId,domain?.context),changed=local.gainDraftBinding!==next;local.inputs??=new Map();local.gainDraftDirty??=new Set();
 if(changed){for(const id of gainDraftIds)local.inputs.delete(id);local.gainDraftDirty.clear();local.gainDraftBinding=next;}
 const set=(id,value)=>{if(!local.gainDraftDirty.has(id))local.inputs.set(id,typeof value==='boolean'?{value:'on',checked:value}:{value:String(value)});};
 if(changed){set('gain-soft-start',true);set('gain-smooth-change',true);set('gain-ramp-step',1);set('gain-ramp-interval-ms',100);set('gain-stable-timeout',30);}
 if(!domain?.context?.connection_id||!Number.isFinite(ageUpperMs))return changed;
 const device=domain.device,timing={roundTripMs:ageUpperMs,receivedAtMs:nowMs};
 for(const [id,name]of [['gain-temp','target_c'],['gain-current','current_ma']]){const field=gainEvidence({...device,timing},name,nowMs);if(field.quality==='fresh'&&field.connection_id===domain.context.connection_id&&Number.isFinite(field.value))set(id,field.value);}
 const pid=device?.pid;if(pid?.quality==='fresh'&&pid.connection_id===domain.context.connection_id&&Array.isArray(pid.values)&&pid.values.length===3&&pid.values.every(Number.isFinite))
  for(const [i,id]of ['gain-pid-p','gain-pid-i','gain-pid-d'].entries())set(id,pid.values[i]);
 return changed;
}
export function markGainPidRead(local,bootId,context){local.gainPidReadBinding=binding(bootId,context);}
export function gainPidReadDue({visible,role,device,local,bootId,nowMs=performance.now()}){
 return Boolean(visible&&['READY','ACTIVE'].includes(role?.mode)&&gainCanOperate(role,device,nowMs)&&!local.gainSafetyPending&&!local.gainSafetyUnknown&&
  local.gainPidReadBinding!==binding(bootId,role.context)&&!(device.pid?.connection_id===role.context.connection_id&&Array.isArray(device.pid.values)));
}
export async function submitGainAction({action,readState,run,local,onChange=()=>{},isCurrent=()=>true}){
 if(local.gainNormalFlow)throw new Error('A Gain operation is already in progress.');
 const initial=readState(),captured=structuredClone(action),priorSafetyId=local.gainSafetyAttempt?.request_id;
 if(!isCurrent()||!gainCanOperate(initial.role,initial.device))throw new Error('Gain controls are unavailable. Check status before continuing.');
 const exact=()=>{const current=readState();return isCurrent()&&current.bootId===initial.bootId&&same(current.context,initial.context)&&
  current.lease?.token===initial.lease?.token&&current.lease?.control_epoch===initial.lease?.control_epoch&&gainCanOperate(current.role,current.device);};
 local.gainNormalFlow=true;onChange();
 try{
  if(gainCanResume(initial.role,initial.device)){
   const resumed=await run('resume',{confirm:true});
   if(resumed.status!=='Terminal'||resumed.phase!=='completed')throw new Error('Gain resume is unconfirmed. Check its original status before trying again.');
   if(!exact()||!['READY','ACTIVE'].includes(readState().role.mode))throw new Error('Gain authority or context changed during resume. No output action was sent.');
  }
  if(!exact())throw new Error('Gain authority or context changed. No output action was sent.');
  return await run('action',captured);
 }catch(cause){
  if(cause.gainCanceled){
   try{await local.gainSafetyPromise;}catch{}
   const stopped=local.gainSafetyAttempt,receipt=local.gainSafetyRecord,current=readState();
   if(stopped?.request_id&&stopped.request_id!==priorSafetyId&&stopped.resolved&&!local.gainSafetyUnknown&&receipt?.status==='Terminal'&&receipt.phase==='completed'&&
    receipt.request_id===stopped.request_id&&same(receipt.domain,stopped.domain)&&current.role?.confirmed&&!current.role.hostRestricted&&
    current.bootId===stopped.boot_id&&same(current.context,receipt.result?.context)&&stopped.boot_id===initial.bootId&&
    stopped.context?.session_id===initial.context?.session_id&&stopped.context?.connection_id===initial.context?.connection_id){
    if(local.activity?.kind===captured.name){local.gainCanceledActivity=local.activity;local.activity=null;}return cause.operation;
   }
  }
  throw cause;
 }finally{local.gainNormalFlow=false;onChange();}
}
/** A separate safety request preserves the original normal operation and never acquires authority. */
export async function sendGainSafety({client,store,hostId,domain,rev,local,name,resync,onChange=()=>{},isCurrent=()=>true}){
 if(!['disable_current','disable_tec'].includes(name))throw new Error('Invalid Gain safety action.');
 if(local.gainSafetyPromise){
  if(name==='disable_tec'&&local.gainSafetyAttempt?.name==='disable_current'){
   if(local.gainTecOffQueued)return local.gainTecOffQueued;
   const pending=local.gainSafetyPromise,queued=(async()=>{try{await pending;}catch{}if(local.gainSafetyUnknown||!local.gainSafetyAttempt?.resolved)throw new Error('Current shutdown is unconfirmed. TEC Off was not sent; check the original shutdown status.');return sendGainSafety({client,store,hostId,domain,rev,local,name,resync,onChange,isCurrent});})();
   local.gainTecOffQueued=queued;try{return await queued;}finally{if(local.gainTecOffQueued===queued)delete local.gainTecOffQueued;}
  }
  return local.gainSafetyPromise;
 }
 const previous=local.gainSafetyAttempt;
 const job=(async()=>{
  const key=deviceKey(hostId,domain),scope=()=>({synced:store.host(hostId)?.connected&&store.host(hostId)?.synced,boot_id:store.host(hostId)?.bootId,context:store.get(key)?.context});
  let outcome='failed',dispatched=false,terminal=false,newAttempt=false;local.gainSafetyPending=true;local.gainSafetyActivity=startActivity(name,previous&&!previous.resolved?'sync':'prepare');onChange();
  try{
   if(previous&&!previous.resolved){
    const result=await queryOriginalOperation(client,previous,resync,scope);local.gainSafetyRecord=result.record;previous.resolved=result.resolved;local.gainSafetyUnknown=!result.resolved;outcome=result.resolved?'complete':'unknown';
    if(name==='disable_tec'&&previous.name==='disable_current'){if(!result.resolved)throw new Error('Current shutdown is unconfirmed. TEC Off was not sent; check the original shutdown status.');}else return result.record;
   }
   if(!isCurrent()||local.connecting||local.disconnectInFlight||local.disconnecting)throw new Error('Gain connection is changing. Check the connection before sending Off.');
   const host=store.host(hostId),snapshot=store.get(key),lease=store.lease(key);
   if(!host?.connected||!host.synced||!store.canControl(key)||!lease||!snapshot?.context?.connection_id)throw new Error('Gain control authority is unavailable. No output command was sent.');
   const request_id=crypto.randomUUID().replaceAll('-',''),context=structuredClone(snapshot.context);
   const intent={domain,lease_token:lease.token,control_epoch:lease.control_epoch,config_rev:rev,context,method:'action',params:{name,args:{}},sequence:client.nextSequence(),confirmation:null};
   const attempt={request_id,domain:structuredClone(domain),boot_id:host.bootId,context,resolved:false,name};local.gainSafetyAttempt=attempt;local.gainSafetyRecord=null;newAttempt=true;
   local.gainSafetyHistory=[...(local.gainSafetyHistory||[]),attempt].slice(-16);
   const proof=await client.prepare(intent);intent.confirmation=proof.token;
   if(!isCurrent()||store.host(hostId)?.bootId!==attempt.boot_id||!store.canControl(key)||!same(store.get(key)?.context,context)||store.lease(key)?.token!==lease.token||store.lease(key)?.control_epoch!==lease.control_epoch){attempt.resolved=true;throw new Error('Gain authority or context changed during preparation. No output command was sent.');}
   local.gainSafetyActivity=advanceActivity(local.gainSafetyActivity,'instrument');onChange();
   dispatched=true;let record=await client.execute(request_id,intent);record=await pollOriginalOperation(client,request_id,record);local.gainSafetyRecord=record;
   if(record.status==='Outcome Unknown'||record.phase==='timed_out_unknown'){local.gainSafetyUnknown=true;outcome='unknown';return record;}
   if(record.request_id!==request_id||!same(record.domain,domain))throw Object.assign(new Error('Gain safety operation identity mismatch.'),{outcomeUnknown:true});
   terminal=record.status==='Terminal';
   local.gainSafetyActivity=advanceActivity(local.gainSafetyActivity,'sync');onChange();await resync();
   const current=scope();attempt.resolved=record.status==='Terminal'&&Boolean(record.result?.context)&&current.synced&&current.boot_id===attempt.boot_id&&same(current.context,record.result.context)&&record.phase!=='completed_readback_failed';
   local.gainSafetyUnknown=!attempt.resolved;
   if(!attempt.resolved){outcome='unknown';return record;}
   if(record.phase!=='completed')throw new Error(record.result?.error?.message||record.result?.error||record.phase);
   outcome='complete';return record;
  }catch(cause){if(cause.outcomeUnknown||['AdmissionPending','OutcomeUnknown'].includes(cause.code)||dispatched&&(!terminal||!local.gainSafetyAttempt?.resolved)){local.gainSafetyUnknown=true;outcome='unknown';}else if(newAttempt&&!dispatched)local.gainSafetyAttempt.resolved=true;throw cause;}
  finally{local.gainSafetyPending=false;local.gainSafetyActivity=finishActivity(local.gainSafetyActivity,local.gainSafetyUnknown?'unknown':outcome);onChange();}
 })();local.gainSafetyPromise=job;
 try{return await job;}finally{if(local.gainSafetyPromise===job)delete local.gainSafetyPromise;}
}
