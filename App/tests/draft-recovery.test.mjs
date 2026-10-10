import test from 'node:test';
import assert from 'node:assert/strict';
import {createSetupActions,draftFailureMessage} from '../web/setup-actions.js';
import {releasedDomainWithCachedSample} from './released-domain-fixture.mjs';

const hostId='a'.repeat(32),bootId='b'.repeat(32),id='c'.repeat(32),sessionId='d'.repeat(32),domain={kind:'device',id};
const key=hostId+'/device/'+id,profile={id:'cp210x-serial',access:'serial',probe_mode:'supervised'},model={id:'gain',profiles:[profile]};
const fault='Gain temperature deviation exceeds 3 degC';
const wire=value=>JSON.parse(JSON.stringify(value,(_key,item)=>item&&typeof item==='object'&&!Array.isArray(item)?Object.fromEntries(Object.entries(item).sort(([a],[b])=>a.localeCompare(b))):item));
function fixture({healthy=false,pending=false,owned=true,releaseMode='confirmed',wireOrder=false}={}){
 const calls=[],context={session_id:'f'.repeat(32),domain,connection_id:'3'.repeat(32),epoch:1};
 const d={modelId:'gain',profileId:profile.id,name:'Gain',params:{port:'COM4'},record:{device_id:id,revision:1,mode:'real'},driverCheck:{modelId:'gain',profileId:profile.id,driver:'cp210x',state:'ready',issued:performance.now()},manualPort:true};
 d.recordSignature=JSON.stringify(['gain',profile.id,'Gain',{port:'COM4'}]);
 const lease={token:'1'.repeat(32),boot_id:bootId,session_id:sessionId,domain,control_epoch:0,expires_in_ms:10000};
 let held=lease,clock=0;
 const host={connected:true,synced:true,bootId,seq:4,mode:'real',registry:{registry_rev:1},control:{['device:'+id]:{state:'CONTROLLED',controller_session:owned?sessionId:'2'.repeat(32),control_epoch:0}},cleanup_attempts:[]};
 let snapshot={domain,context,state:'READY',responsibility:true,pending:pending?1:0,active_request_id:pending?'pending':null,pending_request_id:null,safety_request_id:null,readback_request_id:null,device:{state:healthy?'READY':'FAULT',connected:healthy,fault:healthy?null:fault}};
 const ticket={ticket_id:'4'.repeat(32),boot_id:bootId,domain,control_epoch:1,cause:'Released',baseline_invalidated:false};
 function complete(mode=releaseMode){
  const released={...context,connection_id:null,epoch:2};
  const result={ok:true,phase:'completed',context:released,result:{connected:false,effective_intent:'disconnect',cleanup:{attempt_id:'6'.repeat(32),unreleased:[],steps:[{role:'gain',action:'current_off_then_tec_off',error:null}]}}};
  const receipt={attempt_id:'7'.repeat(32),ticket:structuredClone(ticket),error:null,resource_release_confirmed:true,result};
  if(mode==='failed'){receipt.error={message:'Gain close failed'};receipt.resource_release_confirmed=false;receipt.result=null;}
  if(mode==='wrong-ticket')receipt.ticket.ticket_id='8'.repeat(32);
  if(mode==='stale-context')result.context={...released,epoch:1};
  if(mode==='wrong-boot')receipt.ticket.boot_id='9'.repeat(32);
  host.cleanup_attempts=[[receipt]];
  if(mode!=='failed'){snapshot=releasedDomainWithCachedSample(released,{state:'FAULT',connected:false,fault});host.control['device:'+id]={state:'AVAILABLE',controller_session:null,control_epoch:1};}
 }
 const store={host:()=>wireOrder?wire(host):host,get:()=>wireOrder?wire(snapshot):snapshot,lease:()=>held,setLease:(_key,next)=>{held=next;},dropLease:()=>{held=null;},canControl:()=>owned&&host.control['device:'+id].state==='CONTROLLED'&&Boolean(held)};
 const client={cancelDraft:async()=>{calls.push('cancel');throw Error('Configuration replacement attempted before release');},release:async(selected,basis)=>{assert.deepEqual(selected,domain);assert.equal(basis.token,lease.token);calls.push('release');host.control['device:'+id]={state:'RETAINED',controller_session:null,control_epoch:1};if(releaseMode==='unknown')throw Object.assign(new Error('Release reply unavailable'),{outcomeUnknown:true});return {accepted:true,ticket:wireOrder?wire(ticket):ticket};},acquire:async()=>{calls.push('acquire');host.control['device:'+id]={state:'CONTROLLED',controller_session:sessionId,control_epoch:1};return {...lease,token:'a'.repeat(32),control_epoch:1};}};
 const actions=createSetupActions({client,store,hostId},null,async()=>{calls.push('sync');host.seq++;if(calls.includes('release')&&releaseMode!=='pending'&&releaseMode!=='unknown')complete();},async(_domain,_rev,method)=>{calls.push(method);snapshot={...snapshot,context:{...snapshot.context,connection_id:'a'.repeat(32),epoch:3},responsibility:true,state:'READY',device:{state:'READY',connected:true}};return {status:'Terminal',phase:'completed'};});
 return {actions,d,calls,host,store,client,complete,options:{now:()=>clock,wait:async ms=>{clock+=ms;},releaseTimeoutMs:1}};
}
test('retained Gain FAULT behind scheduler READY releases once before one new connection',async()=>{
 const f=fixture();await f.actions.prepare(f.d,model,f.options);
 assert.deepEqual(f.calls.filter(c=>c!=='sync'),['release','acquire','connect']);
 assert.equal(f.store.get(key).device.connected,true);
});
test('canonical Host JSON ticket and cleanup receipt confirm release before one new Gain connection',async()=>{
 const f=fixture({wireOrder:true});await f.actions.prepare(f.d,model,f.options);assert.deepEqual(f.calls.filter(c=>c!=='sync'),['release','acquire','connect']);assert.equal(f.d.releaseAttempt,null);
});
test('healthy supervised connection does not release or reconnect',async()=>{
 const f=fixture({healthy:true});await f.actions.prepare(f.d,model,f.options);assert.deepEqual(f.calls,[]);
});
for(const mode of ['failed','pending','wrong-ticket','stale-context','wrong-boot','unknown'])test(`Gain recovery ${mode} cannot reconnect or replay release`,async()=>{
 const f=fixture({releaseMode:mode});await assert.rejects(f.actions.prepare(f.d,model,f.options),/release|Release|close|Close/);
 await assert.rejects(f.actions.prepare(f.d,model,f.options),/release|Release|close|Close/);
 assert.equal(f.calls.filter(c=>c==='release').length,1);assert.ok(!f.calls.includes('connect'));assert.ok(!f.calls.includes('acquire'));
});
test('timeout retry only queries the original release ticket before continuing',async()=>{
 const f=fixture({releaseMode:'pending'});await assert.rejects(f.actions.prepare(f.d,model,f.options),/release|Release/);
 f.complete('confirmed');await f.actions.prepare(f.d,model,f.options);
 assert.equal(f.calls.filter(c=>c==='release').length,1);assert.equal(f.calls.filter(c=>c==='connect').length,1);
});
test('editing a timed-out Gain draft queries its original release without cancelling or replacing configuration',async()=>{
 const f=fixture({releaseMode:'pending'});await assert.rejects(f.actions.prepare(f.d,model,f.options),/release|Release/);
 f.d.params.port='COM5';f.d.name='Edited Gain';await assert.rejects(f.actions.prepare(f.d,model,f.options),/release|Release/);
 assert.equal(f.calls.filter(c=>c==='release').length,1);assert.ok(!f.calls.includes('cancel'));assert.ok(!f.calls.includes('acquire'));assert.ok(!f.calls.includes('connect'));
});
test('a returned nonterminal connection outcome remains unknown without a second connection',async()=>{
 const f=fixture();f.complete();f.store.dropLease(key);
 const actions=createSetupActions({client:f.client,store:f.store,hostId},null,async()=>{},async()=>{f.calls.push('connect');return {status:'Outcome Unknown',phase:'timed_out_unknown'};});
 await assert.rejects(actions.prepare(f.d,model,f.options),/unconfirmed/);await assert.rejects(actions.prepare(f.d,model,f.options),/unknown/);
 assert.equal(f.calls.filter(c=>c==='connect').length,1);assert.ok(!f.calls.includes('release'));
});
test('Cancel while the original release is pending finishes release without reopening Gain',async()=>{
 const f=fixture({releaseMode:'pending'});let cancelled=false;
 await f.actions.prepare(f.d,model,{...f.options,cancelled:()=>cancelled,wait:async ms=>{cancelled=true;f.complete('confirmed');await f.options.wait(ms);}});
 assert.equal(f.calls.filter(c=>c==='release').length,1);assert.ok(!f.calls.includes('acquire'));assert.ok(!f.calls.includes('connect'));assert.equal(f.store.get(key).responsibility,false);
});
test('Cancel while the new authority acquisition resolves prevents a new connection',async()=>{
 const f=fixture();f.complete();f.store.dropLease(key);let cancelled=false;const acquire=f.client.acquire;
 f.client.acquire=async()=>{const lease=await acquire();cancelled=true;return lease;};
 await f.actions.prepare(f.d,model,{...f.options,cancelled:()=>cancelled});
 assert.equal(f.calls.filter(c=>c==='acquire').length,1);assert.ok(!f.calls.includes('connect'));assert.equal(f.store.get(key).context.connection_id,null);
});
for(const options of [{pending:true},{owned:false}])test(`Gain recovery preserves ${options.pending?'pending work':'foreign ownership'}`,async()=>{
 const f=fixture(options);await assert.rejects(f.actions.prepare(f.d,model,f.options),/progress|authority|control|Control/);assert.ok(!f.calls.includes('release'));assert.ok(!f.calls.includes('connect'));
});
test('unknown connection outcome blocks automatic fault recovery',async()=>{
 const f=fixture();await assert.rejects(f.actions.prepare(f.d,model,{...f.options,outcomeUnknown:true}),/unknown|Unknown/);assert.ok(!f.calls.includes('release'));
});
test('verification failure exposes retained cached Gain fault without internal JSON',()=>{
 const f=fixture(),raw=JSON.stringify({context:{session_id:'secret'},error:{type:'ManualVerificationRequired',message:'No independently authorized session'}});
 const message=draftFailureMessage(new Error(raw),f.store.get(key));assert.match(message,/Gain temperature deviation exceeds 3 degC/);assert.match(message,/release.*unconfirmed/i);assert.doesNotMatch(message,/secret|session_id|\{/);
});
test('a release failure already explaining the Gain fault does not repeat its reason',()=>{
 const f=fixture(),message=draftFailureMessage(new Error(fault+'. Connection release is unconfirmed.'),f.store.get(key));
 assert.equal(message.split(fault).length-1,1);assert.match(message,/release is unconfirmed/);
});
