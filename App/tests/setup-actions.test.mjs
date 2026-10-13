import test from 'node:test';import assert from 'node:assert/strict';
import {createSetupActions} from '../web/setup-actions.js';
import * as setupActions from '../web/setup-actions.js';
test('clearing the selected model safely resets discovery without querying any resources',async()=>{
 const d={params:{},modelId:null,profileId:null,visaScan:{state:'ready'},serialScan:{state:'ready'}};
 await setupActions.refreshDraftConnection(d,undefined,undefined,()=>{throw Error('Unexpected inventory query');},()=>{throw Error('Unexpected laser scan');},()=>{});
 assert.equal(d.visaScan,null);assert.equal(d.serialScan,null);assert.equal(d.driverCheck,null);
});
test('a late serial inventory cannot erase discovery for a newly selected VISA model',async()=>{
 let finish;const d={modelId:'gain',profileId:'serial',params:{}},nextScan={modelId:'aq6370',profileId:'gpib-visa',state:'checking'};
 const pending=setupActions.refreshDraftConnection(d,{id:'gain'},{id:'serial',access:'serial'},()=>new Promise(resolve=>finish=resolve),()=>{},()=>{});
 await Promise.resolve();d.modelId='aq6370';d.profileId='gpib-visa';d.visaScan=nextScan;
 finish({serial:[],errors:{},usb_serial:{}});await pending;
 assert.equal(d.visaScan,nextScan);assert.equal(d.params.resource,undefined);
});
test('a changed laser head on the same controller invalidates an earlier connection proof',async()=>{
 const d={modelId:'tlb6700',profileId:'newport-usb',params:{device_key:'6700 SN1012'},proof:{proof_id:'old'},controllerScan:{state:'ready',controllers:[{device_key:'6700 SN1012',serial:'1012',head_model:'6712',head_serial:'H1'}]}};
 await setupActions.refreshControllerChoices(d,{id:'tlb6700'},{id:'newport-usb',access:'newport'},async()=>({controllers:[{device_key:'6700 SN1012',serial:'1012',head_model:'6722-P',head_serial:'H2'}]}),()=>{});
 assert.equal(d.params.device_key,'6700 SN1012');assert.equal(d.proof,null);assert.equal(d.controllerScan.controllers[0].head_model,'6722-P');
});
test('normal driver-and-device refresh invalidates a proof for a replaced head',async()=>{
 const d={modelId:'tlb6700',profileId:'newport-usb',name:'Laser',params:{device_key:'6700 SN1012'},record:{revision:1},proof:{proof_id:'old-head-proof',revision:1,issued:performance.now(),signature:JSON.stringify(['tlb6700','newport-usb','Laser',{device_key:'6700 SN1012'}])},controllerScan:{state:'ready',controllers:[{device_key:'6700 SN1012',serial:'1012',head_model:'6712',head_serial:'H1'}]}};
 await setupActions.refreshDraftConnection(d,{id:'tlb6700'},{id:'newport-usb',access:'newport'},async()=>({newport:{sdk:{state:'ready'},devices:[]}}),async()=>({controllers:[{device_key:'6700 SN1012',serial:'1012',head_model:'6722-P',head_serial:'H2'}]}),()=>{});
 assert.equal(d.proof,null);assert.equal(d.controllerScan.controllers[0].head_model,'6722-P');
});
test('missing Newport prerequisites prevent draft creation, leasing and identity probes',async()=>{
 const calls=[];const client={createDraft:async()=>{calls.push('create');return {device_id:'d'.repeat(32),revision:1,config_digest:'x'};},acquire:async()=>{calls.push('acquire');return {token:'x',control_epoch:0};},testConnection:async()=>{calls.push('test');return {proof_id:'p'};},nextSequence:()=>1};
 const store={host:()=>({mode:'real',registry:{registry_rev:0}}),lease:()=>null,setLease:()=>{}};
 const actions=createSetupActions({client,store,hostId:'a'.repeat(32)},()=>true,async()=>{});
 const d={modelId:'tlb6700',profileId:'newport-usb',params:{},name:'Laser'};
 await assert.rejects(actions.test(d,{name:'TLB-6700'},{access:'newport',open_effects:[],probe_mode:'readonly'}),/driver/i);
 assert.deepEqual(calls,[]);
});
test('a late driver check cannot authorize a newly selected connection profile',async()=>{
 assert.equal(typeof setupActions.refreshDraftDrivers,'function');
 const d={modelId:'tlb6700',profileId:'newport-usb'},model={id:'tlb6700'},profile={id:'newport-usb',access:'newport'};
 let complete;const read=()=>new Promise(resolve=>{complete=resolve;});
 const pending=setupActions.refreshDraftDrivers(d,model,profile,read,()=>{});
 assert.equal(d.driverCheck.state,'checking');
 d.modelId='gain';d.profileId='cp210x-serial';
 await setupActions.refreshDraftDrivers(d,{id:'gain'},{id:'cp210x-serial',access:'serial'},async()=>({usb_serial:{cp210x:{state:'unavailable',devices:[]}}}),()=>{});
 complete({newport:{sdk:{state:'ready'},devices:[{driver_state:'ready'}]},errors:{}});await pending;
 assert.notEqual(d.driverCheck?.state,'ready');
});
test('imported configuration must be cancelled and freshly verified before save authority exists',async()=>{
 const calls=[],priorId='a'.repeat(32),newId='b'.repeat(32),hostId='c'.repeat(32);const host={mode:'real',registry:{registry_rev:8}};
 const d={modelId:'aq6370',profileId:'gpib-visa',name:'Imported OSA',params:{resource:'GPIB0::4::INSTR'},record:{device_id:priorId,revision:3,mode:'unverified'},proof:{proof_id:'obsolete'}};
 const client={
  cancelDraft:async p=>{calls.push(['cancel',p]);host.registry.registry_rev++;},
  createDraft:async p=>{calls.push(['create',p]);host.registry.registry_rev++;return {device_id:newId,revision:1,mode:'real',config_digest:'digest'};},
  acquire:async p=>{calls.push(['acquire',p]);return {token:'lease',control_epoch:0};},
  testConnection:async p=>{calls.push(['test',p]);return {proof_id:'fresh'};},nextSequence:()=>1};
 const store={host:()=>host,dropLease:k=>calls.push(['drop',k]),lease:()=>null,setLease:()=>{}};
 const actions=createSetupActions({client,store,hostId},()=>true,async()=>{});
 await actions.test(d,{name:'OSA'},{open_effects:[],probe_mode:'readonly'});
 assert.deepEqual(calls.map(c=>c[0]),['cancel','drop','create','acquire','test']);
 assert.deepEqual(calls.find(c=>c[0]==='create')[1].params,{resource:'GPIB0::4::INSTR'});
 assert.equal(calls.find(c=>c[0]==='test')[1].draft_id,newId);
 assert.equal(calls.find(c=>c[0]==='test')[1].consent.mode,'real');
 assert.equal(d.proof.proof_id,'fresh');assert.equal(d.record.device_id,newId);
});
test('saving a current proof never implicitly connects the instrument',async()=>{
  const calls=[];const client={saveDevice:async p=>{calls.push(['save',p]);return {device_id:'d'};}};
  const host={registry:{registry_rev:1}};const session={client,hostId:'h',store:{host:()=>host}};
  const actions=createSetupActions(session,()=>true,async()=>{});
  const d={modelId:'osa',profileId:'gpib',name:'OSA',params:{resource:'GPIB0::4::INSTR'},record:{device_id:'d',revision:1},proof:{proof_id:'proof',revision:1,issued:performance.now(),signature:JSON.stringify(['osa','gpib','OSA',{resource:'GPIB0::4::INSTR'}])}};
  await actions.save(d);assert.deepEqual(calls.map(c=>c[0]),['save']);
});
test('a direct manual VISA test cannot acquire while discovery is unresolved',async()=>{
 const calls=[],actions=createSetupActions({client:{createDraft:async()=>{calls.push('create');throw Error('Unexpected create');}},store:{host:()=>({mode:'real',registry:{registry_rev:0}})},hostId:'a'.repeat(32)},()=>true,async()=>{});
 const d={modelId:'aq6370',profileId:'gpib-visa',params:{resource:'GPIB0::4::INSTR'},manualResource:true,visaScan:{state:'checking'}};
 await assert.rejects(actions.test(d,{id:'aq6370'},{id:'gpib-visa',access:'visa',interfaces:['GPIB'],fields:{resource:{kind:'resource',interfaces:['GPIB']}}}),/VISA resources/);
 assert.deepEqual(calls,[]);
});
