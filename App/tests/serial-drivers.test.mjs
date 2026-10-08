import test from 'node:test';
import assert from 'node:assert/strict';
import {renderAddWizard,renderDriverStatus,driverCheckReady} from '../web/setup.js';
import {refreshDraftDrivers,refreshDraftConnection} from '../web/setup-actions.js';
import {createHostClient} from '../web/host-client.js';
import * as actions from '../web/setup-actions.js';

const models=['voltage','gain'].map((id,i)=>({id,name:id,category:'Custom',profiles:[{id:i?'cp210x-serial':'ch340-serial',access:'serial',interfaces:['Serial'],probe_mode:'supervised',open_effects:[],fields:{port:{kind:'serial'}}}]}));
const catalog={categories:['Custom'],models};
const draft=m=>({modelId:m.id,profileId:m.profiles[0].id,params:{}});
for(const [i,m] of models.entries()) {
 const driver=i?'cp210x':'ch340',label=i?'CP210':'CH340',profile=m.profiles[0];
 test(`${driver} missing driver offers installation only for the selected device`,async()=>{
  const d=draft(m);await refreshDraftDrivers(d,m,profile,async()=>({usb_serial:{[driver]:{state:'missing',devices:[{driver_state:'missing'}]}}}),()=>{});
  assert.equal(d.driverCheck?.state,'missing');assert.equal(d.driverCheck?.driver,driver);
  const html=renderAddWizard(d,catalog);assert.match(html,new RegExp(label));assert.match(html,/data-ui="install-draft-driver"/);
  assert.match(html,/<button[^>]*data-ui="test-draft"[^>]*disabled/);assert.match(html,/<button[^>]*data-ui="prepare-draft"[^>]*disabled/);
  assert.equal(driverCheckReady(d,profile),false);
 });
 test(`${driver} absent device and failed check never offer an installer`,async()=>{
  for(const state of ['not_detected','unavailable','ready']) {
   const d=draft(m);await refreshDraftDrivers(d,m,profile,async()=>({usb_serial:{[driver]:{state,devices:[]}}}),()=>{});
   assert.equal(d.driverCheck?.state,state);assert.doesNotMatch(renderAddWizard(d,catalog),/data-ui="install-draft-driver"/);
   assert.equal(driverCheckReady(d,profile),state==='ready');
  }
 });
 test(`${driver} checking prerequisites never scans a laser`,async()=>{
  const d=draft(m);await refreshDraftConnection(d,m,profile,async()=>({usb_serial:{[driver]:{state:'ready',devices:[]}}}),()=>{throw new Error('Must not open Newport');},()=>{});
  assert.equal(d.driverCheck?.state,'ready');assert.equal(d.controllerScan,null);
 });
}
test('late serial driver reply cannot populate a different model',async()=>{
 const d=draft(models[0]);let finish;const pending=refreshDraftDrivers(d,models[0],models[0].profiles[0],()=>new Promise(r=>finish=r),()=>{});
 d.modelId=models[1].id;d.profileId=models[1].profiles[0].id;
 await refreshDraftDrivers(d,models[1],models[1].profiles[0],async()=>({usb_serial:{cp210x:{state:'ready'}}}),()=>{});
 assert.equal(typeof finish,'function');finish({usb_serial:{ch340:{state:'missing'}}});await pending;assert.equal(d.driverCheck?.driver,'cp210x');assert.equal(d.driverCheck?.state,'ready');
});
test('Settings offers installation beside only the missing driver',()=>{
 const html=renderDriverStatus({newport:{sdk:{state:'missing'},devices:[]},usb_serial:{ch340:{state:'missing'},cp210x:{state:'ready'}}});
 assert.match(html,/CH340/);assert.match(html,/CP210/);assert.match(html,/data-ui="install-driver" data-driver="ch340"/);assert.doesNotMatch(html,/data-ui="install-driver" data-driver="cp210x"/);
});
test('native install request is bound to a supported driver identifier',async()=>{
 const requests=[];const c=createHostClient(async(_,p)=>{requests.push(p.request);return {v:1,id:p.request.id,ok:true,result:{state:'running'}};},async()=>()=>{});
 await c.installDriver('ch340');await c.installDriver('cp210x');assert.deepEqual(requests.map(r=>r.params),[{driver:'ch340'},{driver:'cp210x'}]);
});
test('a stale missing-driver form rechecks readiness and never installs an unplugged or ready device',async()=>{
 assert.equal(typeof actions.installMissingDriver,'function');
 for(const state of ['ready','not_detected','unavailable']){
  const m=models[0],d=draft(m);d.driverCheck={driver:'ch340',state:'missing',modelId:m.id,profileId:d.profileId,issued:-100000};
  let installations=0;const client={driverStatus:async()=>({usb_serial:{ch340:{state}}}),installDriver:async()=>{installations++;throw new Error('Unexpected installation');}};
  const pending=actions.installMissingDriver(d,m,m.profiles[0],client,()=>{});
  if(state==='ready')await pending;else await assert.rejects(pending,/detected|unconfirmed|fault/i);
  assert.equal(installations,0);assert.equal(d.installing,false);
 }
});
test('installation sends the selected package, polls its job, then checks actual readiness',async()=>{
 assert.equal(typeof actions.installMissingDriver,'function');
 const m=models[1],d=draft(m),reads=['missing','ready'];let installed;
 d.driverCheck={driver:'cp210x',state:'missing',modelId:m.id,profileId:d.profileId};
 const client={driverStatus:async()=>({usb_serial:{cp210x:{state:reads.shift()}}}),installDriver:async id=>{installed=id;return {state:'running',driver:id};},driverInstallStatus:async()=>({state:'completed',driver:'cp210x',restart_required:false})};
 await actions.installMissingDriver(d,m,m.profiles[0],client,()=>{},{wait:async()=>{}});
 assert.equal(installed,'cp210x');assert.equal(d.driverCheck.state,'ready');assert.equal(reads.length,0);assert.equal(d.installing,false);
});
test('cancelled, reboot-required, wrong-package and successful-but-unready installs cannot claim readiness',async()=>{
 assert.equal(typeof actions.installMissingDriver,'function');
 for(const job of [{state:'failed',driver:'ch340',message:'Cancelled'},{state:'completed',driver:'ch340',restart_required:true},{state:'completed',driver:'cp210x'},{state:'completed',driver:'ch340'},{state:'no_change',driver:'ch340'}]){
  const m=models[0],d=draft(m);d.driverCheck={driver:'ch340',state:'missing',modelId:m.id,profileId:d.profileId};
  const client={driverStatus:async()=>({usb_serial:{ch340:{state:'missing'}}}),installDriver:async()=>job};
  await assert.rejects(actions.installMissingDriver(d,m,m.profiles[0],client,()=>{}));
  assert.equal(d.driverCheck.state,'missing');assert.equal(d.installing,false);
 }
});
