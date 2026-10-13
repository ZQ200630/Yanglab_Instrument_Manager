import test from 'node:test';
import assert from 'node:assert/strict';
import {connectLocalHost} from '../web/host-client.js';
import {createHostRecovery,renderHostRecovery} from '../web/host-recovery.js';
import {renderSettings} from '../web/setup.js';

const healthy={connected:true,mode:'real',worker_protocol:3};
const absent=()=>Object.assign(new Error('No Host'),{code:'HostAbsent'});
async function boundedResult(work){
 let timer;try{return await Promise.race([work,new Promise((_,reject)=>timer=setTimeout(()=>reject(new Error('test watchdog: startup wait did not settle')),180))]);}finally{clearTimeout(timer);}
}
test('healthy existing Host is attached without starting or disconnecting it',async()=>{
 const calls=[];
 const client={connect:async()=>{calls.push('attach');return healthy;},startHost:async()=>calls.push('start'),disconnect:async()=>calls.push('detach')};
 assert.deepEqual(await connectLocalHost(client,{},()=>{},{timeoutMs:25}),healthy);
 assert.deepEqual(calls,['attach']);
});
test('silent existing Host has a bounded wait and never starts another Host',async()=>{
 const calls=[];
 const client={connect:()=>{calls.push('attach');return new Promise(()=>{});},startHost:async()=>calls.push('start')};
 await assert.rejects(boundedResult(connectLocalHost(client,{},()=>{},{timeoutMs:20})),e=>e.code==='HostStartPending'&&e.outcomeUnknown===true);
 assert.deepEqual(calls,['attach']);
});
test('uncertain HostAbsent or failed cleanup never grants permission to start',async()=>{
 for(const extra of [{outcomeUnknown:true},{cleanupError:new Error('release pending')}]){
  let starts=0;const cause=Object.assign(absent(),extra);
  await assert.rejects(connectLocalHost({connect:async()=>{throw cause;},startHost:async()=>starts++}),e=>e===cause);
  assert.equal(starts,0);
 }
});
test('attach-only retry cannot spawn even if the old endpoint disappeared',async()=>{
 let starts=0;
 await assert.rejects(connectLocalHost({connect:async()=>{throw absent();},startHost:async()=>starts++},{},()=>{},{attachOnly:true}),e=>e.code==='HostAbsent');
 assert.equal(starts,0);
});
test('late startup completion never initiates another attach after timeout',async()=>{
 let finish,attaches=0,starts=0;
 const pending=new Promise(resolve=>finish=resolve);
 const client={connect:async()=>{attaches++;throw absent();},startHost:async()=>{starts++;return pending;}};
 await assert.rejects(boundedResult(connectLocalHost(client,{},()=>{},{timeoutMs:20})),e=>e.code==='HostStartPending');
 finish();await pending;await new Promise(resolve=>setTimeout(resolve,5));
 assert.equal(attaches,1);assert.equal(starts,1);
});
test('incompatible Host detach is bounded and preserves its unconfirmed cleanup',async()=>{
 let starts=0;
 const client={connect:async()=>({...healthy,worker_protocol:2}),disconnect:()=>new Promise(()=>{}),startHost:async()=>starts++};
 await assert.rejects(boundedResult(connectLocalHost(client,{},()=>{},{timeoutMs:20})),e=>e.code==='HostIncompatible'&&e.cleanupError?.outcomeUnknown===true);
 assert.equal(starts,0);
});
test('recovery cannot clear session identity or reconnect when GUI detach fails',async()=>{
 const calls=[],host={connected:true,worker_cleanup:{resource_released:false,process_exit:{confirmed:true,success:false}}};
 const receipt={recovery_authorized:true,host_stopped:true,incident_id:'a'.repeat(32)};
 const flow=createHostRecovery({host:()=>host,confirm:()=>true,client:{recoverHost:async()=>receipt},
  disconnect:async()=>{calls.push('detach');throw new Error('detach still pending');},clearConnection:()=>calls.push('clear'),reconnect:async()=>calls.push('reconnect')});
 await assert.rejects(flow.run(),/detach still pending/);
 assert.deepEqual(calls,['detach']);assert.equal(host.connected,true);assert.deepEqual(flow.state.receipt,receipt);
});
test('live unconfirmed cleanup is visible but never enables exited-Worker recovery',()=>{
 const host={connected:true,worker_cleanup:{active_attempt:'a'.repeat(32),resource_released:false,process_exit:{confirmed:false}}};
 const html=renderSettings(host,{});
 assert.doesNotMatch(html,/Ready to use your instruments/);
 assert.match(renderHostRecovery(host),/unconfirmed|unverified/i);
 assert.doesNotMatch(html,/data-ui="recover-host"/);
});
test('normal running Worker evidence is not a failed shutdown attempt',()=>{
 const host={connected:true,worker_cleanup:{active_attempt:null,attempts:[],received_cleanup_report:null,process_exit:null,resource_released:false}};
 const html=renderSettings(host,{});
 assert.match(html,/Ready to use your instruments/);
 assert.equal(renderHostRecovery(host),'');
 assert.doesNotMatch(html,/Connection release is unconfirmed|Worker shutdown is unverified/);
});
