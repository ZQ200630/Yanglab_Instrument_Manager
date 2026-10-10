import test from 'node:test';import assert from 'node:assert/strict';
import * as actions from '../web/setup-actions.js';
import {renderAddWizard,requiredDriver,driverCheckReady,signature,canSave} from '../web/setup.js';
const model={id:'gain',name:'Gain Chip Driver',category:'Custom',manufacturer:'Yang Lab',profiles:[{id:'cp210x-serial',access:'serial',interfaces:['Serial'],probe_mode:'supervised',open_effects:['DTR_RTS_reset_not_verified'],fields:{port:{kind:'serial',required:true}}}]},profile=model.profiles[0],catalog={models:[model],categories:['Custom']};
const port=(resource,vid,pid,serial='')=>({resource,vid,pid,serial,instance_id:`USB\\VID_${vid.toString(16)}&PID_${pid.toString(16)}\\${serial||resource}`,description:'USB adapter '+resource});
const inventory=serial=>({serial,usb_serial:Object.fromEntries(['ch340','cp210x'].map(family=>[family,{state:'ready',devices:serial.filter(p=>family==='ch340'?p.vid===0x1a86:p.vid===0x10c4).map(p=>({instance_id:p.instance_id,driver_state:'ready'}))}]))});
const draft=()=>({modelId:'gain',profileId:profile.id,params:{},name:'Gain'});
test('serial selection offers both compatible families, retains all ports and never guesses among multiple candidates',async()=>{
 const d=draft();await actions.refreshDraftConnection(d,model,profile,async()=>inventory([port('COM10',0x1a86,0x7523),port('COM8',0x0403,0x6001),port('COM7',0x10c4,0xea60),port('COM4',0x1a86,0x7523)]),()=>{throw Error('No laser scan');},()=>{});
 assert.deepEqual(d.serialScan?.ports.map(p=>p.resource),['COM4','COM7','COM8','COM10']);
 assert.deepEqual(d.serialScan?.candidates.map(p=>p.resource),['COM4','COM7','COM10']);assert.equal(d.params.port,undefined);
 const html=renderAddWizard(d,catalog);assert.match(html,/<select[^>]*data-param="port"/);assert.match(html,/CH340/);assert.match(html,/CP210/);assert.match(html,/Show all serial ports/);assert.doesNotMatch(html,/<input[^>]*data-param="port"/);
 d.showAllPorts=true;assert.match(renderAddWizard(d,catalog),/COM8/);
});
test('one ready CH340 candidate is proposed for Gain and its actual driver family is checked',async()=>{
 const d=draft();let reads=0;await actions.refreshDraftConnection(d,model,profile,async()=>{reads++;return inventory([port('COM4',0x1a86,0x7523,'GAIN-A')]);},()=>{},()=>{});
 assert.equal(reads,1);assert.equal(d.params.port,'COM4');assert.equal(requiredDriver(d.modelId,profile,d),'ch340');assert.equal(d.driverCheck.driver,'ch340');assert.equal(driverCheckReady(d,profile),true);
});
test('unplugged or replaced selected adapter clears the selection and proof without choosing a replacement',async()=>{
 const d=draft();await actions.refreshDraftConnection(d,model,profile,async()=>inventory([port('COM4',0x10c4,0xea60,'A')]),()=>{},()=>{});
 d.proof={proof_id:'old'};await actions.refreshDraftConnection(d,model,profile,async()=>inventory([port('COM4',0x10c4,0xea60,'B')]),()=>{},()=>{});
 assert.equal(d.params.port,undefined);assert.equal(d.proof,null);
});
test('selected ready adapter is not blocked by another missing driver in its family',async()=>{
 const d=draft(),reply=inventory([port('COM4',0x10c4,0xea60,'A')]);
 reply.usb_serial.cp210x.state='missing';reply.usb_serial.cp210x.devices.push({instance_id:'USB\\VID_10C4&PID_EA60\\OTHER',driver_state:'missing'});
 await actions.refreshDraftConnection(d,model,profile,async()=>reply,()=>{},()=>{});
 assert.equal(driverCheckReady(d,profile),true);assert.doesNotMatch(renderAddWizard(d,catalog),/data-ui="install-draft-driver"/);
});
test('unmatched or ambiguous instance IDs never infer a ready or missing driver',async()=>{
 for(const devices of [[],[{instance_id:'OTHER',driver_state:'ready'}],[{instance_id:'same',driver_state:'ready'},{instance_id:'same',driver_state:'missing'}]]){
  const d=draft(),p={...port('COM4',0x10c4,0xea60,'A'),instance_id:'same'},reply=inventory([p]);reply.usb_serial.cp210x.devices=devices;
  await actions.refreshDraftConnection(d,model,profile,async()=>reply,()=>{},()=>{});
  assert.equal(d.driverCheck.state,'unavailable');assert.equal(driverCheckReady(d,profile),false);assert.doesNotMatch(renderAddWizard(d,catalog),/data-ui="install-draft-driver"/);
 }
});
test('manual port proof is invalidated by removal or instance replacement even without a USB serial',async()=>{
 for(const removed of [false,true]){
  const d=draft(),a={...port('COM4',0x10c4,0xea60),instance_id:'USB\\ADAPTER-A'};
  await actions.refreshDraftConnection(d,model,profile,async()=>inventory([a]),()=>{},()=>{});d.manualPort=true;
  d.record={revision:1};d.proof={proof_id:'old',revision:1,signature:signature(d),issued:performance.now()};assert.equal(canSave(d),true);
  await actions.refreshDraftConnection(d,model,profile,async()=>inventory(removed?[]:[{...a,instance_id:'USB\\ADAPTER-B'}]),()=>{},()=>{});
  assert.equal(d.params.port,'COM4');assert.equal(canSave(d),false);
 }
});
test('Gain offers the detected missing CH340 driver before Windows assigns a COM port',async()=>{
 const d=draft(),reply=inventory([]);reply.usb_serial.ch340={state:'missing',devices:[{instance_id:'USB\\VID_1A86&PID_7523\\A',driver_state:'missing'}]};reply.usb_serial.cp210x={state:'not_detected',devices:[]};
 await actions.refreshDraftConnection(d,model,profile,async()=>reply,()=>{},()=>{});
 assert.equal(d.driverCheck.driver,'ch340');assert.equal(d.driverCheck.state,'missing');assert.match(renderAddWizard(d,catalog),/data-ui="install-draft-driver"/);
});
test('Windows instance matching ignores casing but still requires one complete identifier',async()=>{
 const d=draft(),p=port('COM4',0x10c4,0xea60,'A'),reply=inventory([p]);reply.usb_serial.cp210x.devices[0].instance_id=p.instance_id.toLowerCase();
 await actions.refreshDraftConnection(d,model,profile,async()=>reply,()=>{},()=>{});assert.equal(driverCheckReady(d,profile),true);
});
test('installing the missing adapter refreshes its newly assigned port and exact readiness',async()=>{
 const d=draft(),missing=inventory([]);missing.usb_serial.ch340={state:'missing',devices:[{instance_id:'USB\\VID_1A86&PID_7523\\A',driver_state:'missing'}]};missing.usb_serial.cp210x={state:'not_detected',devices:[]};
 await actions.refreshDraftConnection(d,model,profile,async()=>missing,()=>{},()=>{});let installed=false;
 const client={driverStatus:async()=>installed?inventory([port('COM4',0x1a86,0x7523,'A')]):missing,installDriver:async driver=>{assert.equal(driver,'ch340');installed=true;return{state:'completed',driver,restart_required:false};}};
 await actions.installMissingDriver(d,model,profile,client,()=>{});assert.equal(d.params.port,'COM4');assert.equal(driverCheckReady(d,profile),true);
});
test('late serial scan cannot populate a different model and inventory failure stays explicit',async()=>{
 assert.equal(typeof actions.refreshSerialChoices,'function');const d=draft();let finish;
 const pending=actions.refreshSerialChoices(d,model,profile,()=>new Promise(r=>finish=r),()=>{});d.modelId='voltage';finish(inventory([port('COM4',0x1a86,0x7523)]));await pending;assert.equal(d.params.port,undefined);
 const e=draft();await actions.refreshSerialChoices(e,model,profile,async()=>({serial:[],errors:{serial:'Device list unavailable'}}),()=>{});assert.equal(e.serialScan.state,'failed');assert.match(renderAddWizard(e,catalog),/Device list unavailable/);
});
test('verification errors expose the useful reason without dumping internal session JSON',()=>{
 const raw=JSON.stringify({context:{session_id:'internal-session'},error:{type:'ManualVerificationRequired',message:'No independently authorized session'}});
 assert.match(actions.draftFailureMessage(new Error(raw)),/No independently authorized session/);assert.doesNotMatch(actions.draftFailureMessage(new Error(raw)),/session_id|internal-session|\{/);
 assert.equal(actions.draftFailureMessage(new Error(JSON.stringify({error:{message:'USB cable unplugged'}}))),'USB cable unplugged');
});
