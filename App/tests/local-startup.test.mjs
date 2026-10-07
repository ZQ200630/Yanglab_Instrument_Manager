import test from 'node:test';import assert from 'node:assert/strict';
import * as startup from '../web/main.js';
const config={};
function boundary({absent=false,startError}={}){const calls=[];let attachments=0;return {calls,
 client:{async connect(){calls.push('attach');if(absent&&attachments++===0)throw {code:'HostAbsent'};return {connected:true,mode:'real',worker_protocol:3};},
 async startHost(value){calls.push(['start',value]);if(startError)throw {code:startError};}}};}
test('normal GUI startup attaches an existing local Host without starting or operating devices',async()=>{
 assert.equal(typeof startup.connectLocalHost,'function');const f=boundary();await startup.connectLocalHost(f.client,config);
 assert.deepEqual(f.calls,['attach']);
});
test('missing Host starts the fixed native package then attaches',async()=>{
 assert.equal(typeof startup.connectLocalHost,'function');const f=boundary({absent:true});await startup.connectLocalHost(f.client,config);
 assert.deepEqual(f.calls,['attach',['start',config],'attach']);
});
test('obsolete saved interpreter input cannot select the native Host worker',async()=>{
 const f=boundary({absent:true});await startup.connectLocalHost(f.client,{pythonPath:'untrusted.exe'});
 assert.deepEqual(f.calls,['attach',['start',{}],'attach']);
});
test('a concurrent GUI winning startup is attached without spawning again',async()=>{
 assert.equal(typeof startup.connectLocalHost,'function');const f=boundary({absent:true,startError:'HostRunning'});await startup.connectLocalHost(f.client,config);
 assert.deepEqual(f.calls,['attach',['start',config],'attach']);
});
test('retained/unknown/invalid Host does not trigger automatic startup or another attempt',async()=>{
 assert.equal(typeof startup.connectLocalHost,'function');
 for(const code of ['UntrustedClient','HostProtocol','OutcomeUnknown','LocalIpc','WorkerRetained']){
  let starts=0;await assert.rejects(startup.connectLocalHost({connect:async()=>{throw {code};},startHost:async()=>{starts++;}},config));assert.equal(starts,0);
 }
 const f=boundary({absent:true,startError:'HostStartPending'});await assert.rejects(startup.connectLocalHost(f.client,config));assert.equal(f.calls.length,2);
});
