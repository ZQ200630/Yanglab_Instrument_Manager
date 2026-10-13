import test from 'node:test';
import assert from 'node:assert/strict';
import * as recovery from '../web/console-ui.js';
import {renderSettings} from '../web/setup.js';

const incident='b'.repeat(32);
const receipt={recovery_authorized:true,cleanup_verified:false,physical_zero_verified:false,incident_id:incident,host_stopped:true};
const retained={connected:true,worker_cleanup:{resource_released:false,process_exit:{confirmed:true,success:false,code:19},error:'Cleanup receipt was not received'}};

test('Settings offers recovery only for a confirmed exited Worker or disconnected ownership issue',()=>{
 const cases=[
  [retained,true],
  [{connected:true,worker_cleanup:{resource_released:false,process_exit:{confirmed:false}}},false],
  [{connected:true,worker_cleanup:{resource_released:true,process_exit:{confirmed:true,success:true}}},false],
  [{connected:false,hostIssue:{code:'OwnershipUnknown',message:'Original owner is unresolved'}},true],
  [{connected:false,hostIssue:{code:'WorkerRecoveryRequired',message:'Worker exited'}},true],
  [{connected:true,hostIssue:{code:'OwnershipUnknown'}},false],
  [{connected:false,hostIssue:{code:'HostStartPending'}},false],
  [{connected:false},false]
 ];
 for(const [host,want]of cases){
  const html=renderSettings(host,{});
  assert.equal(/data-ui="recover-host"/.test(html),want,JSON.stringify(host));
 }
});

function controller({result=receipt,confirm=true,gate,timeoutMs=30000}={}){
 const calls=[],changes=[],host=structuredClone(retained);
 assert.equal(typeof recovery.createHostRecovery,'function');
 const flow=recovery.createHostRecovery({host:()=>host,confirm:message=>{calls.push(['confirm',message]);return confirm;},
  client:{recoverHost:async()=>{calls.push(['recover']);await gate;if(result instanceof Error)throw result;return structuredClone(result);}},
  disconnect:async()=>{calls.push(['disconnect']);},clearConnection:()=>{calls.push(['clear']);host.connected=false;},
  reconnect:async()=>{calls.push(['reconnect']);},onChange:state=>changes.push(structuredClone(state)),timeoutMs});
 return {flow,calls,changes,host};
}

test('recovery needs deliberate confirmation with the consequences before any native action',async()=>{
 const f=controller({confirm:false});
 assert.equal(await f.flow.run(),null);
 assert.deepEqual(f.calls.map(c=>c[0]),['confirm']);
 const message=f.calls[0][1];
 for(const disclosure of [/old Worker.*exited/i,/all clients.*disconnect/i,/unsaved.*previews.*lost/i,/instrument.*output.*unverified/i,/commands.*not.*replayed/i,/read.only.*verif/i])assert.match(message,disclosure);
});

test('a delayed recovery acknowledges immediately and ignores a second click before one Host reconnect',async()=>{
 let release;const gate=new Promise(resolve=>release=resolve),f=controller({gate});
 const pending=f.flow.run();
 assert.equal(f.flow.state.busy,true);
 assert.equal(f.changes[0].phase,'recovering');
 assert.equal(await f.flow.run(),null);
 assert.deepEqual(f.calls.map(c=>c[0]),['confirm','recover']);
 release();assert.deepEqual(await pending,receipt);
 assert.deepEqual(f.calls.map(c=>c[0]),['confirm','recover','disconnect','clear','reconnect']);
 assert.equal(f.flow.state.busy,false);
 assert.deepEqual(f.flow.state.receipt,receipt);
 assert.equal(f.flow.state.phase,'complete');
 assert.equal(f.host.worker_cleanup.error,'Cleanup receipt was not received');
});

test('failed or unverified recovery retains connection and diagnostics without reconnecting',async()=>{
 for(const result of [new Error('Old Worker is still alive'),{...receipt,recovery_authorized:false},{...receipt,incident_id:'not-an-incident'},{...receipt,host_stopped:false}]){
  const f=controller({result});
  await assert.rejects(f.flow.run());
  assert.deepEqual(f.calls.map(c=>c[0]),['confirm','recover']);
  assert.equal(f.host.connected,true);
  assert.deepEqual(f.host.worker_cleanup,retained.worker_cleanup);
  assert.equal(f.flow.state.busy,false);
  assert.equal(f.flow.state.phase,'failed');
  assert.ok(f.flow.state.error);
 }
});

test('timed out recovery stops pending feedback and never acts on a late receipt',async()=>{
 let release;const gate=new Promise(resolve=>release=resolve),f=controller({gate,timeoutMs:10});
 await assert.rejects(f.flow.run(),/unconfirmed/i);
 assert.equal(f.flow.state.busy,false);assert.equal(f.flow.state.phase,'unknown');
 release();await gate;await Promise.resolve();
 assert.deepEqual(f.calls.map(c=>c[0]),['confirm','recover']);
 assert.equal(f.host.connected,true);
 assert.equal(f.flow.state.receipt,null);
});

test('recovery cannot run when confirmed exit evidence is absent',async()=>{
 const f=controller();f.host.worker_cleanup.process_exit.confirmed=false;
 await assert.rejects(f.flow.run(),/not available/i);
 assert.deepEqual(f.calls,[]);
});

test('pending Settings recovery exposes elapsed feedback and disables only conflicting Host actions',()=>{
 const html=renderSettings({...retained,hostRecovery:{busy:true,phase:'recovering',started:100,error:null,receipt:null}},{});
 assert.match(html,/<button[^>]*data-ui="recover-host"[^>]*disabled[^>]*aria-busy="true"/);
 assert.match(html,/Recovering Host/);assert.match(html,/data-activity-elapsed="100"/);
 assert.match(html,/<button[^>]*data-ui="stop-host"[^>]*disabled/);
 assert.match(html,/<button[^>]*data-ui="disconnect-host"[^>]*disabled/);
 assert.doesNotMatch(html,/<button[^>]*data-ui="save-host"[^>]*disabled/);
 assert.doesNotMatch(html,/<button[^>]*data-ui="check-drivers"[^>]*disabled/);
 assert.match(html,/Cleanup receipt was not received/);
});

test('successful Host recovery never presents instruments as ready before verification',()=>{
 const html=renderSettings({connected:true,hostRecovery:{busy:false,phase:'complete',receipt,previousCleanup:retained.worker_cleanup}},{});
 assert.doesNotMatch(html,/Ready to use your instruments/);
 assert.match(html,/read-only verification before instrument actions/);
});

test('recovery keeps the original disconnected ownership error as historical diagnostics',async()=>{
 const original={code:'OwnershipUnknown',message:'Original Worker identity cannot be independently verified'};
 const f=controller();f.host.connected=false;f.host.worker_cleanup=null;f.host.hostIssue=original;
 await f.flow.run();f.host.hostIssue=null;
 const html=renderSettings({...f.host,hostRecovery:f.flow.state},{});
 assert.match(html,/Original Worker identity cannot be independently verified/);
});
