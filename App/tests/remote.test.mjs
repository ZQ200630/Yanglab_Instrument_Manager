import test from 'node:test';
import assert from 'node:assert/strict';
import {createRemoteClient} from '../web/remote-client.js';
import {createConsoleSession} from '../web/main.js';
import {renderOverview} from '../web/overview.js';
import {observeDisconnects,createRoutedClient} from '../web/console-ui.js';
import {createHostClient} from '../web/host-client.js';

const a='a'.repeat(32),b='b'.repeat(32),d='d'.repeat(32),boot='c'.repeat(32);
test('routing wraps actual frozen native clients without violating Proxy invariants',async()=>{
  const calls=[];let selected=a;
  const build=id=>createHostClient(async(command,args)=>{calls.push([id,command]);return {v:1,id:args?.request?.id,ok:true,result:{},error:null};},async()=>()=>{});
  const clients=new Map([[a,build(a)],[b,build(b)]]);
  const routed=createRoutedClient(id=>clients.get(id),()=>selected);
  await routed.savePreferences({});await routed.safeStop({kind:'device',id:d});selected=b;
  await routed.operation('request');await routed.forHost(a).disconnect();
  assert.deepEqual(calls,[[a,'host_save_preferences'],[a,'host_call'],[b,'host_call'],[a,'host_disconnect']]);
});
function snapshot(id){return {type:'snapshot',host_id:id,boot_id:boot,seq:1,data:{host_id:id,mode:'real',host_name:'Desktop',registry:{devices:[{device_id:d,name:'OSA',model_id:'aq6370',params:{resource:'GPIB0::4::INSTR'}}],setups:[]},domains:{['device:'+d]:{state:'DISCONNECTED',device:null,context:{session_id:'e'.repeat(32),domain:{kind:'device',id:d},connection_id:null,epoch:0}}},control:{}}};}

test('remote actions, results, heartbeat and exports select the owning native client',async()=>{
  const calls=[];
  const client=createRemoteClient(async(command,args)=>{calls.push([command,args]);return command==='remote_call'?{v:1,id:args.request.id,ok:true,result:{ok:'received'},error:null}:null;},async()=>()=>{},b);
  await client.connect();await client.acquire({kind:'device',id:d});await client.renew('lease');await client.readResult({domain:{kind:'device',id:d},id:'result',offset:0,length:4});
  await client.exportArchive({host_id:b,id:'archive'});
  assert.deepEqual(calls.map(c=>c[0]),['remote_connect','remote_call','remote_call','remote_call','remote_export']);
  assert.ok(calls.slice(0,4).every(c=>c[1].hostId===b));
  assert.deepEqual(calls.slice(1,4).map(c=>c[1].request.method),['acquire_control','renew_control','read_result']);
  assert.equal(calls[4][1].reference.host_id,b);
});

test('a remote event never changes local identity and duplicate device IDs remain independent',()=>{
  const local={name:'local'},remote={name:'remote'},session=createConsoleSession(local);
  session.apply(snapshot(a));session.addRemote(b,remote);session.apply(snapshot(b),true);
  assert.equal(session.hostId,a);assert.equal(session.clientFor(a),local);assert.equal(session.clientFor(b),remote);
  assert.equal(session.store.all().length,2);assert.equal(session.store.host(b).remote,true);
  session.offline(b);
  assert.equal(session.store.host(a).connected,true);assert.equal(session.store.host(b).connected,false);
  const html=renderOverview(session.store.host(a),session.store);
  assert.match(html,/REMOTE/);assert.match(html,/OFFLINE/);assert.match(html,new RegExp('/host/'+b+'/device/'+d));
  assert.match(html,new RegExp('/host/'+a+'/device/'+d));
});

test('remote event subscriptions discard another Host envelope',async()=>{
  let receiver,received=[];
  const client=createRemoteClient(async()=>null,async(name,fn)=>{assert.equal(name,'remote-event');receiver=fn;return()=>{};},b);
  await client.subscribe(event=>received.push(event));
  receiver({payload:{host_id:a,event:snapshot(a)}});receiver({payload:{host_id:b,event:snapshot(b)}});
  assert.equal(received.length,1);assert.equal(received[0].host_id,b);
});

test('a blocked remote heartbeat cannot delay renewals on the local Host',async()=>{
  let finish,localRenewals=0;
  const token='1'.repeat(32),sessionId='e'.repeat(32),domain={kind:'device',id:d};
  const lease={token,boot_id:boot,session_id:sessionId,domain,control_epoch:0,expires_in_ms:10000};
  const blocked=new Promise(resolve=>finish=resolve);
  const session=createConsoleSession({renew:async()=>{localRenewals++;return lease;}});
  session.addRemote(b,{renew:()=>blocked});
  for(const id of [a,b]){const event=snapshot(id);event.data.control['device:'+d]={state:'CONTROLLED',controller_session:sessionId,control_epoch:0};session.apply(event,id===b);session.store.setLease(id+'/device/'+d,lease);}
  const first=session.heartbeat();await Promise.resolve();await Promise.resolve();
  await session.heartbeat();
  assert.equal(localRenewals,2);
  finish(lease);await first;
});

test('another Host snapshot cannot confirm an outstanding instrument disconnect',()=>{
  const session=createConsoleSession({});session.addRemote(b,{});
  for(const id of [a,b]){const e=snapshot(id);e.data.control['device:'+d]={state:'AVAILABLE'};session.apply(e,id===b);}
  const key=a+'/device/'+d,local={unknown:true,disconnectAccepted:true,disconnectEvidenceBoot:boot,disconnectEvidenceSeq:1};
  const update=snapshot(b);update.seq=5;session.apply(update,true);
  observeDisconnects({[key]:local},session.store);assert.equal(local.unknown,true);
  const own=snapshot(a);own.seq=2;own.data.control['device:'+d]={state:'AVAILABLE'};session.apply(own);
  observeDisconnects({[key]:local},session.store);assert.equal(local.unknown,false);
});
