import test from 'node:test';
import assert from 'node:assert/strict';
import {connectionReleased,connectionView} from '../web/connection.js';
import {observeDisconnects} from '../web/console-ui.js';
import {instanceView} from '../web/instance-view.js';
import {laser} from '../web/panels.js';
import {releasedDomainWithCachedSample} from './released-domain-fixture.mjs';

const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),domain={kind:'device',id:d},key=h+'/device/'+d;
function fixture(){
 const context={session_id:'f'.repeat(32),domain,connection_id:null,epoch:3};
 const state=releasedDomainWithCachedSample(context);
 const host={connected:true,synced:true,bootId:b,seq:4,control:{['device:'+d]:{state:'AVAILABLE'}}};
 const store={get:()=>state,host:()=>host,canControl:()=>false};
 return {state,host,store};
}

test('completed native close permits reconnect while preserving the last connected sample',()=>{
 const {host,state,store}=fixture();
 assert.equal(connectionReleased(host,state),true);
 assert.deepEqual(connectionView(host,store,key),{status:'Disconnected',label:'Connect',operation:'connect',disabled:false});
 assert.equal(state.device.connected,true,'cached readings remain immutable evidence, not live ownership');
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
