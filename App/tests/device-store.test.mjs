import test from 'node:test';
import assert from 'node:assert/strict';
import {createDeviceStore} from '../web/device-store.js';
import {routeFor,parseRoute} from '../web/routes.js';
const host='a'.repeat(32),boot='b'.repeat(32),session='c'.repeat(32);
const ref=n=>({kind:'device',id:String(n).repeat(32)});
const context=(n,epoch=1)=>({session_id:session,domain:ref(n),connection_id:'d'.repeat(32),epoch});
const key=n=>`${host}/device/${ref(n).id}`;
function start(){const store=createDeviceStore(()=>1000);store.apply({type:'snapshot',host_id:host,boot_id:boot,seq:1,data:{mode:'real',domains:{
  [`device:${ref(1).id}`]:{context:context(1),gain:{lastCommand:'unchanged'}},
  [`device:${ref(2).id}`]:{context:context(2),gain:{lastCommand:'unchanged'}}}}});return store;}
test('old_instance_result_is_history_not_current',()=>{
  const store=start();
  assert.equal(store.apply({type:'domain',host_id:host,boot_id:boot,seq:2,domain:ref(1),data:{context:context(1,2),gain:{lastCommand:'disable_current'}}}).currentChanged,true);
  assert.equal(store.get(key(1)).gain.lastCommand,'disable_current');assert.equal(store.get(key(2)).gain.lastCommand,'unchanged');
  assert.equal(store.apply({type:'domain',host_id:host,boot_id:boot,seq:3,domain:ref(1),data:{context:context(1,1),gain:{lastCommand:'enable_current'}}}).currentChanged,false);
  assert.equal(store.get(key(1)).gain.lastCommand,'disable_current');
  assert.equal(store.history(key(1)).length,1);
});
test('sequence gap boot change and offline invalidate control without inventing zero',()=>{
  const store=start();
  assert.equal(store.apply({type:'domain',host_id:host,boot_id:boot,seq:5,domain:ref(1),data:{context:context(1)}}).requiresSnapshot,true);
  assert.equal(store.canControl(key(1)),false);
  assert.equal(store.get(key(1)).gain.lastCommand,'unchanged');
  store.disconnected(host);assert.equal(store.get(key(1)).communication,'UNKNOWN');
});
test('routes are instance scoped and do not invoke a device',()=>{
  const route=routeFor(host,ref(1));assert.deepEqual(parseRoute(route),{hostId:host,domain:ref(1)});
  assert.notEqual(route,routeFor(host,ref(2)));assert.equal(parseRoute('#/host/../../raw'),null);
});
test('explicit coalescing spans do not look like lost events',()=>{
  const store=start();
  assert.equal(store.apply({type:'domain',host_id:host,boot_id:boot,seq:4,first_seq:2,domain:ref(1),data:{context:context(1),reading:4}}).requiresSnapshot,false);
  assert.equal(store.get(key(1)).reading,4);
});
test('queued events older than an authoritative resync are ignored without losing control',()=>{
  const store=start();
  assert.equal(store.apply({type:'domain',host_id:host,boot_id:boot,seq:1,domain:ref(1),data:{context:context(1),reading:'stale'}}).requiresSnapshot,false);
  assert.equal(store.get(key(1)).gain.lastCommand,'unchanged');
});
test('authoritative snapshots remove retired domains and their software leases',()=>{
  const store=start();store.setLease(key(1),{token:'old',boot_id:boot,expires_in_ms:10000});
  store.apply({type:'snapshot',host_id:host,boot_id:boot,seq:2,data:{domains:{[`device:${ref(2).id}`]:{context:context(2)}}}});
  assert.equal(store.get(key(1)),null);assert.equal(store.lease(key(1)),null);
});
