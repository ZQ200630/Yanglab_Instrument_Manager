import test from 'node:test';
import assert from 'node:assert/strict';
import {connectionReleased,connectionView} from '../web/connection.js';
import {observeDisconnects} from '../web/console-ui.js';
import {instanceView} from '../web/instance-view.js';
import {laser} from '../web/panels.js';
import {releasedDomainWithCachedSample} from './released-domain-fixture.mjs';
import {activityBusy,renderActivity} from '../web/activity.js';

const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),domain={kind:'device',id:d},key=h+'/device/'+d;
function fixture(){
 const context={session_id:'f'.repeat(32),domain,connection_id:null,epoch:3};
 const state=releasedDomainWithCachedSample(context);
 const host={connected:true,synced:true,bootId:b,seq:4,control:{['device:'+d]:{state:'AVAILABLE'}}};
 const store={get:()=>state,host:()=>host,canControl:()=>false};
 return {state,host,store};
}

function failedDisconnect(){
 const {host,state,store}=fixture(),context={...state.context,connection_id:'3'.repeat(32),epoch:2};
 const ticket={ticket_id:'7'.repeat(32),boot_id:b,domain,control_epoch:1,cause:'SafeStop',baseline_invalidated:false};
 Object.assign(state,{state:'RETAINED',context,responsibility:true,safety:{state:'RETAINED',phase:'failed_after_call_started',
  context:structuredClone(context),attempt_id:'5'.repeat(32),result:null,error:{type:'ReleaseUnconfirmed',message:'Release remains retained'}}});
 host.seq=5;host.control['device:'+d]={state:'RETAINED',control_epoch:1};
 const receipt={attempt_id:'8'.repeat(32),ticket:structuredClone(ticket),resource_release_confirmed:false,
  physical_zero_verified:false,result:null,error:{code:'CleanupRetained',message:'Driver close did not confirm release'}};
 host.cleanup_attempts=[[receipt]];
 const local={connectionGeneration:1,disconnectInFlight:false,disconnectAccepted:true,disconnectEvidenceBoot:b,disconnectEvidenceSeq:4,
  unknown:true,disconnecting:100,activity:{id:1,kind:'disconnect',phase:'disconnect',started:100,phaseStarted:100,timings:{}},
  disconnectAttempt:{generation:1,activityId:1,bootId:b,domain,context:{...context,epoch:1},seq:4,ticket:structuredClone(ticket),seenAttemptIds:[]}};
 return {host,state,store,local,receipt};
}

test('accepted cleanup terminal failure ends disconnect waiting without claiming release',()=>{
 const {host,state,store,local}=failedDisconnect();
 observeDisconnects({[key]:local},store);
 assert.equal(activityBusy(local.activity),false);
 assert.equal(local.activity.outcome,'unknown');
 assert.equal(local.disconnecting,null);assert.equal(local.disconnectFailed,true);assert.equal(local.unknown,true);
 assert.equal(connectionReleased(host,state),false);
 assert.equal(connectionView(host,store,key,local).label,'Retry disconnect');
 assert.match(renderActivity(local.activity),/Outcome unknown/);assert.doesNotMatch(renderActivity(local.activity),/activity-spinner|Still working/);
});

for(const [name,change]of [
 ['pre-acceptance snapshot',f=>{f.host.seq=4;}],
 ['different Host boot',f=>{f.host.bootId='9'.repeat(32);}],
 ['different returned ticket',f=>{f.receipt.ticket.ticket_id='9'.repeat(32);}],
 ['different receipt domain',f=>{f.receipt.ticket.domain={kind:'device',id:'9'.repeat(32)};}],
 ['different receipt boot',f=>{f.receipt.ticket.boot_id='9'.repeat(32);}],
 ['different control epoch',f=>{f.host.control['device:'+d].control_epoch=2;}],
 ['different Worker session',f=>{f.state.context.session_id='9'.repeat(32);}],
 ['replacement connection',f=>{f.state.context.connection_id='9'.repeat(32);}],
 ['older Worker context',f=>{f.state.context.epoch=0;}],
 ['previous attempt for the same ticket',f=>{f.local.disconnectAttempt.seenAttemptIds.push(f.receipt.attempt_id);}],
 ['invalid cleanup attempt ID',f=>{f.receipt.attempt_id='invalid';}],
 ['missing release outcome',f=>{delete f.receipt.resource_release_confirmed;}],
 ['contradictory release outcome',f=>{f.receipt.resource_release_confirmed=true;}],
 ['unfinished receipt',f=>{f.receipt.error=null;delete f.receipt.resource_release_confirmed;}],
 ['newer UI generation',f=>{f.local.connectionGeneration=2;}],
 ['newer activity',f=>{f.local.activity.id=2;}],
])test(`disconnect does not consume stale or unrelated failure evidence: ${name}`,()=>{
 const f=failedDisconnect();change(f);observeDisconnects({[key]:f.local},f.store);
 assert.equal(activityBusy(f.local.activity),true);assert.equal(f.local.disconnectFailed,undefined);
});

test('retry of a retained ticket waits for a new immutable cleanup attempt',()=>{
 const {host,store,local,receipt}=failedDisconnect();local.disconnectAttempt.seenAttemptIds=[receipt.attempt_id];
 observeDisconnects({[key]:local},store);assert.equal(activityBusy(local.activity),true);
 host.seq++;host.cleanup_attempts[0].push({...receipt,attempt_id:'9'.repeat(32)});
 observeDisconnects({[key]:local},store);assert.equal(activityBusy(local.activity),false);assert.equal(local.unknown,true);
});

test('an older failed receipt cannot override a later new successful cleanup receipt',()=>{
 const {host,store,local,receipt}=failedDisconnect();
 host.cleanup_attempts[0].push({...receipt,attempt_id:'9'.repeat(32),resource_release_confirmed:true,error:null});
 observeDisconnects({[key]:local},store);
 assert.equal(activityBusy(local.activity),true,'a receipt alone still cannot prove release');
 assert.equal(local.disconnectFailed,undefined);
});

test('later authoritative release updates an ended uncertain wait with its actual observation time',()=>{
 const {host,state,store}=fixture(),local={connectionGeneration:1,disconnectInFlight:false,disconnectAccepted:true,
  disconnectEvidenceBoot:b,disconnectEvidenceSeq:3,unknown:true,disconnecting:null,disconnectFailed:true,
  activity:{id:1,kind:'disconnect',phase:'disconnect',started:0,phaseStarted:0,ended:1,outcome:'unknown',timings:{disconnect:1}}};
 observeDisconnects({[key]:local},store);
 assert.equal(local.unknown,false);assert.equal(local.activity.outcome,'complete');
 assert.ok(local.activity.ended>1,'a later release must not inherit the earlier timeout timestamp');
 assert.equal(local.activity.timings.disconnect,local.activity.ended);
 assert.equal(connectionReleased(host,state),true);
});

function acceptedReleasedDisconnect(){
 const f=failedDisconnect(),context={...f.state.context,connection_id:null,epoch:3};
 Object.assign(f.state,releasedDomainWithCachedSample(context));
 f.host.control['device:'+d]={state:'AVAILABLE',control_epoch:1};
 f.local.disconnecting=null;f.local.disconnectFailed=true;
 f.local.activity={id:1,kind:'disconnect',phase:'disconnect',started:0,phaseStarted:0,ended:1,outcome:'unknown',timings:{disconnect:1}};
 return f;
}

for(const [name,change]of [
 ['replacement Worker session',f=>{f.state.context.session_id='9'.repeat(32);}],
 ['newer external control epoch',f=>{f.host.control['device:'+d].control_epoch=4;}],
 ['older Worker context',f=>{f.state.context.epoch=0;}],
 ['missing control epoch evidence',f=>{delete f.host.control['device:'+d].control_epoch;delete f.local.disconnectAttempt.ticket.control_epoch;}],
])test(`accepted timeout is not completed by release from ${name}`,()=>{
 const f=acceptedReleasedDisconnect();change(f);f.state.safety.context=structuredClone(f.state.context);
 assert.equal(connectionReleased(f.host,f.state),true,'snapshot itself confirms release, but not this attempt');
 observeDisconnects({[key]:f.local},f.store);
 assert.equal(f.local.unknown,true);assert.equal(f.local.disconnectFailed,true);assert.equal(f.local.disconnectAccepted,true);
 assert.equal(f.local.activity.outcome,'unknown');assert.match(renderActivity(f.local.activity),/Outcome unknown/);
});

test('accepted ticket reconciles release in the same Worker lineage after fence epoch advances and connection clears',()=>{
 const f=acceptedReleasedDisconnect();observeDisconnects({[key]:f.local},f.store);
 assert.equal(f.local.unknown,false);assert.equal(f.local.disconnectFailed,false);assert.equal(f.local.disconnectAccepted,false);
 assert.equal(f.local.activity.outcome,'complete');assert.equal(connectionView(f.host,f.store,key,f.local).label,'Connect');
});

test('completed native close permits reconnect while preserving the last connected sample',()=>{
 const {host,state,store}=fixture();
 assert.equal(connectionReleased(host,state),true);
 assert.deepEqual(connectionView(host,store,key),{status:'Disconnected',label:'Connect',operation:'connect',disabled:false});
 assert.equal(state.device.connected,true,'cached readings remain immutable evidence, not live ownership');
});
test('confirmed cached close matches exact context fields regardless of wire object order',()=>{
 const {host,state}=fixture(),context=state.safety.context;state.safety.context={epoch:context.epoch,connection_id:context.connection_id,domain:{id:context.domain.id,kind:context.domain.kind},session_id:context.session_id};assert.equal(connectionReleased(host,state),true);
 state.safety.context.epoch++;assert.equal(connectionReleased(host,state),false);
});

test('post-acceptance completed native close clears UI disconnect waiting despite cached readings',()=>{
 const {host,state,store}=fixture(),local={disconnectInFlight:false,disconnectAccepted:true,
  disconnectEvidenceBoot:b,disconnectEvidenceSeq:4,unknown:true,disconnecting:100,disconnectFailed:true};
 observeDisconnects({[key]:local},store);assert.equal(local.unknown,true,'old snapshot is insufficient');
 host.seq++;observeDisconnects({[key]:local},store);
 assert.equal(local.unknown,false);assert.equal(local.disconnecting,null);assert.equal(local.disconnectFailed,false);
 assert.equal(state.device.connected,true);
});

test('disconnected projection never initializes live scan controls from an old cached sample',()=>{
 const {state}=fixture(),cached=structuredClone(state.device);
 const view=instanceView('laser',state,false,20);
 assert.equal(view.status.devices.laser,null);
 assert.match(laser(view),/not connected/);assert.doesNotMatch(laser(view),/id="laser-scan-stop"/);
 assert.deepEqual(state.device,cached,'the native last sample remains preserved');
});

test('cached readings cannot imply release with pending work, retained ownership or an invalid close receipt',()=>{
 const unsafe=[
  ['state','READY'],['responsibility',true],['pending',1],['active_request_id','work'],
  ['pending_request_id','work'],['safety_request_id','work'],['readback_request_id','work'],
  ['context.connection_id','live'],['safety',null],['safety.phase','pending'],['safety.error','failure'],
  ['safety.state','STOP_HELD'],['safety.context.epoch',2],['safety.attempt_id','invalid'],
  ['safety.result.attempt_id','7'.repeat(32)],['safety.result.connected',true],
  ['safety.result.effective_intent','stop'],['safety.result.cleanup.attempt_id','invalid'],
  ['safety.result.cleanup.unreleased',['laser']],['safety.result.cleanup.steps',[]],
  ['safety.result.cleanup.steps',[{role:'laser',action:'preserving_close',error:'close failed'}]],
 ];
 for(const [path,value]of unsafe){const {state,host}=fixture(),parts=path.split('.'),last=parts.pop();
  let target=state;for(const part of parts)target=target[part];target[last]=value;
  assert.equal(connectionReleased(host,state),false,path);
 }
 for(const field of ['responsibility','pending','active_request_id','pending_request_id','safety_request_id','readback_request_id']){
  const {host,state}=fixture();delete state[field];assert.equal(connectionReleased(host,state),false,'missing '+field);
 }
 for(const control of ['CONTROLLED','RETAINED','REVOKING']){const {host,state}=fixture();host.control['device:'+d].state=control;assert.equal(connectionReleased(host,state),false,control);}
 for(const field of ['connected','synced']){const {host,state}=fixture();host[field]=false;assert.equal(connectionReleased(host,state),false,field);}
});
