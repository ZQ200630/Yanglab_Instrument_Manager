import test from 'node:test';
import assert from 'node:assert/strict';
import {createDeviceStore} from '../web/device-store.js';
import {actionFor} from '../web/instance-view.js';
let gain={};try{gain=await import('../web/gain-control.js');}catch(error){if(error.code!=='ERR_MODULE_NOT_FOUND')throw error;}
const domain={kind:'device',id:'c'.repeat(32)},hostId='a'.repeat(32),boot='b'.repeat(32),key=hostId+'/device/'+domain.id;
const context={session_id:'f'.repeat(32),connection_id:'e'.repeat(32),epoch:1,domain};
const fields=Object.fromEntries(Object.entries({temperature_c:25,target_c:25,tec_enabled:true,current_ma:3,current_enabled:false}).map(([name,value])=>[name,{value,quality:'fresh',connection_id:context.connection_id,revision:1,observed_age_s:0}]));
const device={state:'READY',connected:true,fields};
const role={context,confirmed:true,mode:'READY'};
const draft={'gain-current':'42','gain-soft-start':true,'gain-smooth-change':true,'gain-ramp-step':'0.5','gain-ramp-interval-ms':'100','gain-stable-timeout':'30','gain-pid-p':'0.35','gain-pid-i':'0.1','gain-pid-d':'0'};
const get=id=>draft[id];
test('Gain Enable uses one native automatic start operation with validated draft settings',()=>{
 assert.equal(typeof gain.gainAction,'function');
 assert.deepEqual(gain.gainAction('gain-enable-current',get,device),{name:'start_current',args:{current_ma:42,soft_start:true,step_ma:0.5,interval_s:0.1,timeout_s:30}});
 assert.deepEqual(actionFor('gain-enable-current',get,{device}),{name:'start_current',args:{current_ma:42,soft_start:true,step_ma:0.5,interval_s:0.1,timeout_s:30}});
});
test('an already running output changes current through the native ramp only when Smooth change is checked',()=>{
 assert.equal(typeof gain.gainAction,'function');const running={...device,fields:{...fields,current_enabled:{...fields.current_enabled,value:true}}};
 assert.deepEqual(gain.gainAction('gain-set-current',get,running),{name:'ramp_current',args:{current_ma:42,step_ma:0.5,interval_s:0.1}});
 assert.deepEqual(gain.gainAction('gain-set-current',id=>id==='gain-smooth-change'?false:get(id),running),{name:'set_current',args:{current_ma:42}});
 assert.deepEqual(gain.gainAction('gain-set-current',get,device),{name:'set_current',args:{current_ma:42}});
 assert.throws(()=>gain.gainAction('gain-set-current',get,{...device,fields:{...fields,current_enabled:{...fields.current_enabled,quality:'unknown'}}}),/output.*unknown|confirmed/i);
});
test('PID actions preserve zero coefficients and reject blank or out-of-range drafts',()=>{
 assert.equal(typeof gain.gainAction,'function');assert.deepEqual(gain.gainAction('gain-read-pid',get,device),{name:'read_pid',args:{}});
 assert.deepEqual(actionFor('gain-set-pid',get,{device}),{name:'set_pid',args:{p:0.35,i:0.1,d:0}});
 for(const [id,value]of [['gain-pid-p',''],['gain-pid-i','1000'],['gain-pid-d','-0.001'],['gain-current','201'],['gain-ramp-step','0'],['gain-ramp-interval-ms','49'],['gain-stable-timeout','181']])
  assert.throws(()=>gain.gainAction(id.startsWith('gain-pid')?'gain-set-pid':'gain-enable-current',key=>key===id?value:get(key),device));
});
test('Gain allows owned fresh ACTIVE control without weakening stale, pending, fault or authority gates',()=>{
 assert.equal(typeof gain.gainCanOperate,'function');assert.equal(gain.gainCanOperate({...role,mode:'ACTIVE'},{...device,state:'ACTIVE'}),true);
 for(const change of [{confirmed:false},{normalPending:'old'},{safetyPending:'off'},{unknown:true},{hostRestricted:true},{stopHeld:true}])assert.equal(gain.gainCanOperate({...role,...change},device),false);
 assert.equal(gain.gainCanOperate(role,{...device,state:'FAULT'}),false);assert.equal(gain.gainCanOperate(role,{...device,fields:{...fields,current_ma:{...fields.current_ma,observed_age_s:6}}}),false);
 assert.equal(gain.gainCanStart(role,device),true);assert.equal(gain.gainCanStart(role,{...device,fields:{...fields,tec_enabled:{...fields.tec_enabled,value:false}}}),false);
 assert.equal(gain.gainCanStop({...role,normalPending:'waiting',unknown:true}),true);assert.equal(gain.gainCanStop({...role,confirmed:false}),false);
});
test('clean drafts follow exact-connection readback while edited PID and target survive updates',()=>{
 assert.equal(typeof gain.syncGainDraft,'function');const local={inputs:new Map(),gainDraftDirty:new Set()},snapshot={context,state:'READY',device};
 gain.syncGainDraft(local,snapshot,boot,0,1000);assert.equal(local.inputs.get('gain-temp').value,'25');assert.equal(local.inputs.get('gain-soft-start').checked,true);
 local.inputs.set('gain-temp','26.5');local.gainDraftDirty.add('gain-temp');local.inputs.set('gain-pid-p','0.5');local.gainDraftDirty.add('gain-pid-p');
 const next={...snapshot,device:{...device,fields:{...fields,target_c:{...fields.target_c,value:27}},pid:{values:[0.35,0.1,0],quality:'fresh',connection_id:context.connection_id,revision:1,observed_age_s:0}}};
 gain.syncGainDraft(local,next,boot,0,2000);assert.equal(local.inputs.get('gain-temp'),'26.5');assert.equal(local.inputs.get('gain-pid-p'),'0.5');assert.equal(local.inputs.get('gain-pid-i').value,'0.1');
 gain.syncGainDraft(local,{...next,context:{...context,connection_id:'7'.repeat(32),epoch:2}},boot,0,3000);
 assert.equal(local.gainDraftDirty.size,0);assert.equal(local.inputs.has('gain-pid-p'),false,'old PID cannot seed a new connection');assert.equal(local.inputs.has('gain-temp'),false,'old target cannot seed a new connection');
});
test('initial PID read is bounded to one healthy owned visible connection attempt',()=>{
 assert.equal(typeof gain.gainPidReadDue,'function');const local={};
 assert.equal(gain.gainPidReadDue({visible:true,role,device,local,bootId:boot}),true);
 gain.markGainPidRead(local,boot,context);assert.equal(gain.gainPidReadDue({visible:true,role,device,local,bootId:boot}),false);
 assert.equal(gain.gainPidReadDue({visible:false,role,device,local:{},bootId:boot}),false);
 assert.equal(gain.gainPidReadDue({visible:true,role:{...role,normalPending:'waiting'},device,local:{},bootId:boot}),false);
 assert.equal(gain.gainPidReadDue({visible:true,role,device:{...device,pid:{values:[.35,.1,0],connection_id:context.connection_id}},local:{},bootId:boot}),false);
});
function safetyFixture(overrides={}){
 const store=createDeviceStore(()=>1000),lease={token:'1'.repeat(32),boot_id:boot,session_id:'d'.repeat(32),domain,control_epoch:0,expires_in_ms:10000};let seq=0,executeCount=0,prepareCount=0,queryCount=0;const local={pending:'original-start',operationAttempt:{request_id:'original-start'},unknown:false};
 const state={host_id:hostId,registry:{devices:[],drafts:[],setups:[]},control:{['device:'+domain.id]:{state:'CONTROLLED',controller_session:lease.session_id,control_epoch:0}},domains:{['device:'+domain.id]:{context,state:'READY',device}}};
 const publish=()=>store.apply({type:'snapshot',seq:++seq,host_id:hostId,boot_id:boot,data:state});publish();store.setLease(key,lease);
 const completed=id=>({request_id:id,domain,status:'Terminal',phase:'completed',result:{context:state.domains['device:'+domain.id].context,result:{ok:true}}});
 const client={nextSequence:()=>1,prepare:async intent=>{prepareCount++;assert.equal(intent.lease_token,lease.token);assert.deepEqual(intent.context,context);assert.equal(intent.params.name,'disable_current');return {token:'proof'};},execute:async(id,intent)=>{executeCount++;assert.equal(intent.confirmation,'proof');return completed(id);},operation:async id=>{queryCount++;return completed(id);},...overrides};
 return {store,lease,local,state,publish,completed,client,args:{client,store,hostId,domain,rev:1,local,name:'disable_current',resync:async()=>publish(),onChange:()=>{}},counts:()=>({executeCount,prepareCount,queryCount})};
}
test('Off submits a separate exact-lease safety operation while original automatic start remains tracked',async()=>{
 assert.equal(typeof gain.sendGainSafety,'function');const f=safetyFixture();const original=f.local.operationAttempt;const record=await gain.sendGainSafety(f.args);
 assert.equal(record.phase,'completed');assert.deepEqual(f.counts(),{executeCount:1,prepareCount:1,queryCount:0});assert.equal(f.local.pending,'original-start');assert.equal(f.local.operationAttempt,original);assert.equal(f.local.unknown,false);
});
test('an uncertain Off is queried on repeated click without replaying prepare or execute',async()=>{
 assert.equal(typeof gain.sendGainSafety,'function');const f=safetyFixture({execute:async()=>{throw Object.assign(Error('Lost shutdown reply'),{outcomeUnknown:true});}});
 await assert.rejects(gain.sendGainSafety(f.args),/Lost shutdown reply/);assert.equal(f.local.gainSafetyUnknown,true);const attempt=f.local.gainSafetyAttempt;
 await gain.sendGainSafety(f.args);assert.equal(f.local.gainSafetyAttempt,attempt);assert.deepEqual(f.counts(),{executeCount:0,prepareCount:1,queryCount:1});assert.equal(f.local.gainSafetyUnknown,false);assert.equal(f.local.pending,'original-start');
});
test('changed authority during safety preparation cannot execute with a stale confirmation',async()=>{
 assert.equal(typeof gain.sendGainSafety,'function');const f=safetyFixture();f.client.prepare=async()=>{f.state.domains['device:'+domain.id].context={...context,epoch:2};f.publish();return {token:'proof'};};
 await assert.rejects(gain.sendGainSafety(f.args),/changed|authority/i);assert.equal(f.counts().executeCount,0);
});
test('closing the owning window during safety preparation prevents execution',async()=>{
 assert.equal(typeof gain.sendGainSafety,'function');const f=safetyFixture();let current=true;f.client.prepare=async()=>{current=false;return{token:'proof'};};await assert.rejects(gain.sendGainSafety({...f.args,isCurrent:()=>current}),/changed|closed|authority/i);assert.equal(f.counts().executeCount,0);
});
test('Off cannot acquire authority or submit during reconnect, and mismatched query stays uncertain',async()=>{
 assert.equal(typeof gain.sendGainSafety,'function');const f=safetyFixture();f.store.dropLease(key);await assert.rejects(gain.sendGainSafety(f.args),/authority|control/i);assert.equal(f.counts().prepareCount,0);
 f.store.setLease(key,f.lease);f.local.connecting=true;await assert.rejects(gain.sendGainSafety(f.args),/connection|connecting/i);assert.equal(f.counts().prepareCount,0);f.local.connecting=false;
 f.client.execute=async()=>({request_id:'bad',domain,status:'Outcome Unknown',phase:'timed_out_unknown'});await gain.sendGainSafety(f.args);f.client.operation=async()=>({...f.completed('wrong'),domain:{kind:'device',id:'9'.repeat(32)}});
 await assert.rejects(gain.sendGainSafety(f.args),/identity/i);assert.equal(f.local.gainSafetyUnknown,true);
});
for(const code of ['AdmissionPending','OutcomeUnknown'])test(`a typed ${code} reply preserves original Off identity without replay`,async()=>{
 assert.equal(typeof gain.sendGainSafety,'function');const f=safetyFixture({execute:async()=>{throw Object.assign(Error(code),{code});}});f.local.gainSafetyRecord=f.completed('old');await assert.rejects(gain.sendGainSafety(f.args),new RegExp(code));assert.equal(f.local.gainSafetyUnknown,true);
 const id=f.local.gainSafetyAttempt.request_id;await gain.sendGainSafety(f.args);assert.equal(f.local.gainSafetyAttempt.request_id,id);assert.equal(f.counts().prepareCount,1);assert.equal(f.counts().queryCount,1);
});
test('a lost operation query after dispatch never authorizes resubmitting Off',async()=>{
 assert.equal(typeof gain.sendGainSafety,'function');const f=safetyFixture({execute:async id=>({request_id:id,domain,status:'Accepted',phase:'running'}),operation:async()=>{throw Error('Operation query offline');}});
 await assert.rejects(gain.sendGainSafety(f.args),/query offline/);assert.equal(f.local.gainSafetyUnknown,true);f.client.operation=async id=>f.completed(id);await gain.sendGainSafety(f.args);assert.equal(f.counts().prepareCount,1);
});
test('TEC Off requested during Current Off is a distinct queued intent with fresh context',async()=>{
 assert.equal(typeof gain.sendGainSafety,'function');const f=safetyFixture(),calls=[];let finish;const gate=new Promise(resolve=>finish=resolve);f.client.prepare=async intent=>{calls.push(['prepare',intent.params.name,intent.context.epoch]);return{token:'proof'};};
 f.client.execute=async(id,intent)=>{calls.push(['execute',intent.params.name,id]);if(intent.params.name==='disable_current'){await gate;f.state.domains['device:'+domain.id].context={...context,epoch:2};f.publish();}return f.completed(id);};
 const first=gain.sendGainSafety(f.args);await new Promise(resolve=>setImmediate(resolve));const second=gain.sendGainSafety({...f.args,name:'disable_tec'}),duplicate=gain.sendGainSafety({...f.args,name:'disable_tec'});
 assert.equal(calls.filter(c=>c[0]==='execute').length,1);finish();await Promise.all([first,second,duplicate]);assert.deepEqual(calls.filter(c=>c[0]==='prepare').map(c=>c.slice(1)),[['disable_current',1],['disable_tec',2]]);assert.deepEqual(calls.filter(c=>c[0]==='execute').map(c=>c[1]),['disable_current','disable_tec']);assert.notEqual(calls[1][2],calls[3][2]);assert.equal(f.local.pending,'original-start');
});
test('unknown Current Off explicitly blocks queued TEC Off instead of dropping or replaying it',async()=>{
 assert.equal(typeof gain.sendGainSafety,'function');const f=safetyFixture();let finish;const gate=new Promise(resolve=>finish=resolve);f.client.execute=async()=>{await gate;throw Object.assign(Error('Off unknown'),{code:'OutcomeUnknown'});};
 const first=gain.sendGainSafety(f.args),caught=first.catch(()=>{});await new Promise(resolve=>setImmediate(resolve));const stronger=gain.sendGainSafety({...f.args,name:'disable_tec'});finish();await caught;await assert.rejects(stronger,/unconfirmed|unknown/i);assert.equal(f.counts().prepareCount,1);assert.equal(f.local.gainSafetyUnknown,true);
});
const heldSafety={state:'STOP_HELD',phase:'completed',context,attempt_id:'8'.repeat(32),error:null,result:{attempt_id:'8'.repeat(32),effective_intent:'current_off'}};
test('only a confirmed healthy same-context shutdown can enable automatic metadata resume',()=>{
 assert.equal(typeof gain.gainCanResume,'function');const held={...role,mode:'STOP_HELD',stopHeld:true,safety:heldSafety};assert.equal(gain.gainCanResume(held,device),true);assert.equal(gain.gainCanOperate(held,device),true);
 for(const safety of [{...heldSafety,phase:'failed_after_call_started'},{...heldSafety,error:{message:'Off failed'}},{...heldSafety,context:{...context,epoch:2}},{...heldSafety,result:{effective_intent:'disconnect'}},{...heldSafety,result:{effective_intent:'tec_off',attempt_id:heldSafety.attempt_id}}])assert.equal(gain.gainCanResume({...held,safety},device),false);
 assert.equal(gain.gainCanResume({...held,unknown:true},device),false);assert.equal(gain.gainCanResume(held,{...device,state:'FAULT'}),false);
});
function normalFixture(){let state={role:{...role,mode:'STOP_HELD',stopHeld:true,safety:heldSafety},device,bootId:boot,context,lease:{token:'1'.repeat(32),control_epoch:0}};const local={},calls=[];
 const run=async(method,params)=>{calls.push([method,structuredClone(params)]);if(method==='resume')state={...state,role:{...role,mode:'READY'}};return {status:'Terminal',phase:'completed',result:{context}};};return{local,calls,readState:()=>state,run,replace:value=>state=value,get:()=>state};}
test('normal operator action resumes confirmed held metadata then submits captured settings once',async()=>{
 assert.equal(typeof gain.submitGainAction,'function');const f=normalFixture(),action={name:'set_current',args:{current_ma:42}};await gain.submitGainAction({...f,action});assert.deepEqual(f.calls,[['resume',{confirm:true}],['action',{name:'set_current',args:{current_ma:42}}]]);assert.equal(f.local.gainNormalFlow,false);
});
test('failed or unknown resume and changed context cannot submit the original output action',async()=>{
 assert.equal(typeof gain.submitGainAction,'function');for(const result of [{status:'Outcome Unknown',phase:'timed_out_unknown'},{status:'Terminal',phase:'failed_after_call_started',result:{context}}]){const f=normalFixture();f.run=async(method,params)=>{f.calls.push([method,params]);return result;};await assert.rejects(gain.submitGainAction({...f,action:{name:'enable_tec',args:{}}}),/resume|confirm/i);assert.equal(f.calls.length,1);}
 const f=normalFixture();f.run=async(method,params)=>{f.calls.push([method,params]);f.replace({...f.get(),context:{...context,epoch:2},role:{...role,mode:'READY'}});return{status:'Terminal',phase:'completed',result:{context}};};await assert.rejects(gain.submitGainAction({...f,action:{name:'enable_tec',args:{}}}),/changed|context/i);assert.equal(f.calls.length,1);
});
test('confirmed operator Off suppresses only the explicitly canceled current action feedback',async()=>{
 assert.equal(typeof gain.submitGainAction,'function');const f=normalFixture();f.replace({...f.get(),role});f.local.activity={kind:'start_current',outcome:'failed',ended:10};const canceled={phase:'failed_after_call_started'},request_id='8'.repeat(32);
 f.run=async()=>{f.local.gainSafetyAttempt={request_id,domain,boot_id:boot,context,resolved:true};f.local.gainSafetyRecord={request_id,domain,status:'Terminal',phase:'completed',result:{context}};throw Object.assign(Error('Canceled'),{gainCanceled:true,operation:canceled});};assert.equal(await gain.submitGainAction({...f,action:{name:'start_current',args:{current_ma:42}}}),canceled);assert.equal(f.local.activity,null);
 f.local.gainSafetyUnknown=true;await assert.rejects(gain.submitGainAction({...f,action:{name:'start_current',args:{current_ma:42}}}),/Canceled/);
});
test('a prior completed Off cannot suppress a later unrelated Canceled failure',async()=>{
 assert.equal(typeof gain.submitGainAction,'function');const f=normalFixture();f.replace({...f.get(),role});const request_id='8'.repeat(32);f.local.gainSafetyAttempt={request_id,domain,boot_id:boot,context,resolved:true};f.local.gainSafetyRecord={request_id,domain,status:'Terminal',phase:'completed',result:{context}};
 f.run=async()=>{throw Object.assign(Error('Canceled'),{gainCanceled:true,operation:{phase:'failed_after_call_started'}});};await assert.rejects(gain.submitGainAction({...f,action:{name:'start_current',args:{current_ma:42}}}),/Canceled/);
});
test('a new Off receipt must match its exact attempt and current scope to suppress cancellation',async()=>{
 assert.equal(typeof gain.submitGainAction,'function');for(const changed of [{request_id:'9'.repeat(32)},{domain:{kind:'device',id:'9'.repeat(32)}},{result:{context:{...context,epoch:2}}}]){
  const f=normalFixture();f.replace({...f.get(),role});const request_id='8'.repeat(32);f.run=async()=>{f.local.gainSafetyAttempt={request_id,domain,boot_id:boot,context,resolved:true};f.local.gainSafetyRecord={request_id,domain,status:'Terminal',phase:'completed',result:{context},...changed};throw Object.assign(Error('Canceled'),{gainCanceled:true,operation:{phase:'failed_after_call_started'}});};await assert.rejects(gain.submitGainAction({...f,action:{name:'start_current',args:{current_ma:42}}}),/Canceled/);
 }
});
