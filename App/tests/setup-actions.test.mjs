import test from 'node:test';import assert from 'node:assert/strict';
import {createSetupActions} from '../web/setup-actions.js';
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
