import test from 'node:test';
import assert from 'node:assert/strict';
import {setImmediate as tick} from 'node:timers/promises';
import {mountConsole,renderConsole} from '../web/console-ui.js';
import {createConsoleSession} from '../web/main.js';

const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),s='d'.repeat(32);
const domain={kind:'device',id:d},key=h+'/device/'+d,route='#/host/'+h+'/device/'+d;
const model={id:'aq6370',profiles:[{id:'gpib-visa',open_effects:[]}],connect_effects:['read_identity']};
const deferred=()=>{let resolve;const promise=new Promise(yes=>resolve=yes);return {promise,resolve};};
async function until(predicate){const deadline=Date.now()+1500;while(!predicate()){assert.ok(Date.now()<deadline,'UI operation did not settle');await tick();}}

// Only the native transport and DOM are substituted; renderer, store, session,
// click handlers, operation admission and lifecycle presentation are production.
async function fixture(options={}){
  const globals=['document','window','location','confirm','setInterval','clearInterval'];
  const prior=new Map(globals.map(k=>[k,{exists:Object.hasOwn(globalThis,k),value:globalThis[k]}]));
  const nodes=new Map(),listeners=new Map(),calls=[];
  const node=selector=>{if(!nodes.has(selector))nodes.set(selector,{innerHTML:'',textContent:'',hidden:true,
    querySelectorAll:()=>[],addEventListener:(name,fn)=>{const key=selector+':'+name;listeners.set(key,[...(listeners.get(key)||[]),fn]);}});return nodes.get(selector);};
  let seq=0,subscriber,ui;
  const context={session_id:'f'.repeat(32),domain,connection_id:null,epoch:0};
  const state={host_id:h,host_name:'Test Host',mode:'real',control:{['device:'+d]:{state:'AVAILABLE',controller_session:null,control_epoch:0}},
    registry:{registry_rev:1,devices:[{device_id:d,name:'Bench OSA',model_id:'aq6370',profile_id:'gpib-visa',params:{resource:'GPIB0::4::INSTR'},config_rev:1}],drafts:[],setups:[]},
    domains:{['device:'+d]:{state:'DISCONNECTED',device:null,context,host_sample_ms:0}}};
  const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
  function publish(){subscriber?.({type:'snapshot',host_id:h,boot_id:b,seq:++seq,data:state});}
  const client={preferences:async()=>({pythonPath:'VISA'}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},
    catalog:async()=>({models:[model],categories:['OSA']}),subscribe:async fn=>{subscriber=fn;return ()=>{};},
    requestSnapshot:async()=>{calls.push('sync');publish();return {seq};},ping:async()=>({monotonic_ms:performance.now(),client_session_id:s,boot_id:b}),
    acquire:async selected=>{assert.deepEqual(selected,domain);calls.push('acquire');await options.acquireGate?.promise;
      if(options.acquireUnknown)throw Object.assign(new Error('Permission reply unavailable'),{outcomeUnknown:true});
      if(options.acquireAppliedUnknown){state.control['device:'+d]={state:'CONTROLLED',controller_session:options.foreignOwner?'2'.repeat(32):s,control_epoch:0};publish();throw Object.assign(new Error('Permission reply unavailable'),{outcomeUnknown:true});}
      if(options.busy){state.control['device:'+d]={state:'CONTROLLED',controller_session:'2'.repeat(32),control_epoch:0};publish();throw Object.assign(new Error('Already in use'),{code:'ControlOwned'});}
      state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:0};return lease;},
    prepare:async intent=>{calls.push('prepare');assert.equal(intent.lease_token,lease.token);if(options.rejectPrepare)throw new Error('Connection preparation declined');return {token:'proof'};},
    execute:async (id,intent)=>{calls.push('execute');assert.ok(['connect','resume'].includes(intent.method));
      if(options.unknown)throw Object.assign(new Error('Connection outcome unknown'),{outcomeUnknown:true});
      Object.assign(state.domains['device:'+d],{state:'READY',device:{connected:true,state:'READY',identity:'YOKOGAWA,AQ6370D,SN,FW'},context:{...context,connection_id:'3'.repeat(32),epoch:1}});
      return {request_id:id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context:state.domains['device:'+d].context,result:{}}};},
    safeStop:async selected=>{assert.deepEqual(selected,domain);calls.push('stop');await options.stopGate?.promise;if(options.stopFailure)throw new Error('Close reply unavailable');
      state.control['device:'+d]={state:'RETAINED',controller_session:null,control_epoch:1};
      state.domains['device:'+d].state='CLOSING';return {accepted:true,physical_stop_confirmed:false};},
    release:async()=>{calls.push('release');state.control['device:'+d]={state:'AVAILABLE',controller_session:null,control_epoch:1};publish();return {accepted:true};},
    renew:async()=>lease,nextSequence:()=>calls.filter(c=>c==='execute').length+1};
  try{
    globalThis.document={activeElement:null,querySelector:node,getElementById:id=>node('#'+id)};
    globalThis.window={addEventListener() {}};globalThis.location={hash:options.route||route};
    globalThis.confirm=()=>{calls.push('confirm');return options.consent!==false;};
    globalThis.setInterval=()=>1;globalThis.clearInterval=()=>{};
    const session=createConsoleSession(client,()=>ui?.render());
    if(options.remote){session.addRemote(options.remote.hostId,options.remote.client);session.setCatalog(options.remote.hostId,{models:[model],categories:['OSA']});session.apply(options.remote.snapshot,true);}
    ui=mountConsole(session,options.native||{event:{listen(){}}});await ui.ready;calls.length=0;
    return {session,client,state,calls,ui,publish,lease,html:()=>node('#content').innerHTML,notice:()=>node('#notice'),
      click(op){for(const listener of listeners.get('#content:click')||[])listener({target:{closest:selector=>selector==='button'?{disabled:false,dataset:{op}}:null}});},
      clickUi(ui,host){for(const listener of listeners.get('#content:click')||[])listener({target:{closest:selector=>selector==='button'?{disabled:false,dataset:{ui,host}}:null}});},
      restore(){for(const [k,p]of prior){if(p.exists)globalThis[k]=p.value;else delete globalThis[k];}}};
  }catch(error){for(const [k,p]of prior){if(p.exists)globalThis[k]=p.value;else delete globalThis[k];}throw error;}
}

async function networkFixture(){
  const id='9'.repeat(32),calls=[],receivers=new Map();let nativeConnected=false,seq=0;
  const context={session_id:'f'.repeat(32),domain,connection_id:null,epoch:0};
  const state={host_id:id,host_name:'Owner',mode:'real',registry:{devices:[{device_id:d,name:'Remote OSA',model_id:'aq6370',profile_id:'gpib-visa',params:{resource:'GPIB0::4::INSTR'},config_rev:1}],drafts:[],setups:[]},domains:{['device:'+d]:{state:'DISCONNECTED',device:null,context}},control:{['device:'+d]:{state:'AVAILABLE',control_epoch:0}}};
  const publish=()=>{const e={type:'snapshot',host_id:id,boot_id:b,seq:++seq,data:state};for(const fn of receivers.get('remote-event')||[])fn({payload:{host_id:id,event:e}});return e;};
  const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
  const native={event:{listen:async(name,fn)=>{const list=receivers.get(name)||[];list.push(fn);receivers.set(name,list);return()=>list.splice(list.indexOf(fn),1);}},core:{invoke:async(command,args)=>{
    calls.push(command==='remote_call'?args.request.method:command);
    if(command==='app_profile')return {network_only:true};
    if(command==='remote_peers')return [{host_id:id,name:'Owner',endpoint:'127.0.0.1:9443'}];
    if(command==='remote_connect'){assert.equal(nativeConnected,false);nativeConnected=true;return {connected:true,mode:'real',worker_protocol:3};}
    if(command==='remote_disconnect'){if(!nativeConnected)throw Object.assign(new Error('No native client'),{code:'RemoteOffline'});nativeConnected=false;return {released:true};}
    if(command==='remote_subscribe'){publish();return;}
    assert.equal(command,'remote_call');let result;
    switch(args.request.method){
      case 'catalog':result={models:[model],categories:['OSA']};break;
      case 'request_snapshot':result={seq:publish().seq};break;
      case 'ping':result={host_id:id,boot_id:b,client_session_id:s,monotonic_ms:performance.now()};break;
      case 'acquire_control':state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:0};result=lease;break;
      case 'prepare':result={token:'proof'};break;
      case 'execute':Object.assign(state.domains['device:'+d],{state:'READY',device:{connected:true},context:{...context,connection_id:'3'.repeat(32),epoch:1}});result={request_id:args.request.params.request_id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context:state.domains['device:'+d].context,result:{}}};break;
      default:throw new Error('Unexpected method '+args.request.method);
    }
    return {v:1,id:args.request.id,ok:true,result,error:null};
  }}};
  const f=await fixture({native,route:'#/host/'+id+'/device/'+d});return {...f,remoteId:id,remoteCalls:calls};
}

test('network-only App loads its owning remote catalog before OSA Connect',async()=>{
  const f=await networkFixture();try{
    f.clickUi('remote-connect',f.remoteId);await until(()=>f.session.store.host(f.remoteId)?.synced);await tick();
    f.click('connect');await until(()=>f.remoteCalls.includes('execute')||!f.notice().hidden);
    assert.ok(f.remoteCalls.includes('catalog'),'remote catalog not fetched');assert.ok(f.remoteCalls.includes('execute'),f.notice().textContent);
    assert.equal(f.session.hostId,null);assert.ok(!f.calls.includes('acquire'));
  }finally{f.restore();}
});

test('ordinary remote Connect Disconnect Connect creates a new native session',async()=>{
  const f=await networkFixture();try{
    f.clickUi('remote-connect',f.remoteId);await until(()=>f.session.store.host(f.remoteId)?.synced);await tick();
    f.clickUi('remote-disconnect',f.remoteId);await until(()=>f.session.store.host(f.remoteId)?.connected===false);await tick();
    f.clickUi('remote-connect',f.remoteId);await until(()=>f.session.store.host(f.remoteId)?.connected===true||!f.notice().hidden);
    assert.equal(f.remoteCalls.filter(c=>c==='remote_connect').length,2,f.notice().textContent);
    assert.equal(f.remoteCalls.filter(c=>c==='remote_disconnect').length,1);
  }finally{f.restore();}
});

test('remote device Connect is routed to its owning client, never the local native transport',async()=>{
  const remoteId='9'.repeat(32),remoteCalls=[];let session,subscriber,seq=1;
  const remoteContext={session_id:'f'.repeat(32),domain,connection_id:null,epoch:0};
  const remoteState={host_id:remoteId,mode:'real',host_name:'Remote desktop',control:{['device:'+d]:{state:'AVAILABLE',controller_session:null,control_epoch:0}},registry:{registry_rev:1,devices:[{device_id:d,name:'Remote OSA',model_id:'aq6370',profile_id:'gpib-visa',params:{resource:'GPIB0::4::INSTR'},config_rev:1}],drafts:[],setups:[]},domains:{['device:'+d]:{state:'DISCONNECTED',device:null,context:remoteContext}}};
  const event=()=>({type:'snapshot',host_id:remoteId,boot_id:b,seq:seq++,data:remoteState});
  const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
  const remoteClient={
    acquire:async()=>{remoteCalls.push('acquire');remoteState.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:0};return lease;},
    requestSnapshot:async()=>{const e=event();session.apply(e,true);return {seq:e.seq};},
    ping:async()=>({boot_id:b,client_session_id:s,monotonic_ms:performance.now()}),
    prepare:async()=>{remoteCalls.push('prepare');return {token:'proof'};},
    execute:async id=>{remoteCalls.push('execute');Object.assign(remoteState.domains['device:'+d],{state:'READY',device:{connected:true},context:{...remoteContext,connection_id:'3'.repeat(32),epoch:1}});return {request_id:id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context:remoteState.domains['device:'+d].context,result:{}}};},
    nextSequence:()=>1,
  };
  const f=await fixture({route:'#/host/'+remoteId+'/device/'+d,remote:{hostId:remoteId,client:remoteClient,snapshot:event()}});session=f.session;
  try {f.click('connect');await until(()=>remoteCalls.includes('execute'));await tick();
    assert.deepEqual(remoteCalls,['acquire','prepare','execute']);assert.ok(!f.calls.includes('acquire'));assert.equal(session.hostId,h);
    assert.match(f.html(),/REMOTE/);assert.match(f.html(),/>Disconnect<\/button>/);
  }finally{f.restore();}
});

test('an available unowned device offers one enabled Connect without permission buttons',async()=>{
  const f=await fixture();try{
    assert.match(f.html(),/<button[^>]*data-op="connect"[^>]*>Connect<\/button>/);
    assert.doesNotMatch(f.html().match(/<button[^>]*data-op="connect"[^>]*>/)?.[0]||'',/disabled/);
    assert.equal((f.html().match(/data-op="connect"/g)||[]).length,1);
    assert.doesNotMatch(f.html(),/Take control|Control held|Exclusive control|Safe stop \/ release/);
    assert.equal(f.session.store.lease(key),null);
  }finally{f.restore();}
});

test('one Connect click acquires authority before opening and removes successful ID banners',async()=>{
  const f=await fixture();try{f.click('connect');await until(()=>f.html().includes('>Disconnect</button>'));
    assert.equal(f.session.store.canControl(key),true);
    assert.deepEqual(f.calls.filter(c=>['confirm','acquire','prepare','execute'].includes(c)),['confirm','acquire','prepare','execute']);
    assert.equal((f.html().match(/data-op="disconnect"/g)||[]).length,1);
    assert.doesNotMatch(f.html(),/Shared acquisition|Control held|Exclusive control/);
    assert.equal(f.notice().hidden,true);
    assert.match(f.html(),/<details[^>]*>[\s\S]*Diagnostics/);
  }finally{f.restore();}
});

test('declining connection consent never claims authority or opens a device',async()=>{
  const f=await fixture({consent:false});try{f.click('connect');await tick();
    assert.deepEqual(f.calls,['confirm']);assert.equal(f.session.store.lease(key),null);
    assert.equal(f.state.domains['device:'+d].context.connection_id,null);
  }finally{f.restore();}
});

test('a competing controller leaves this window read only without stopping or opening it',async()=>{
  const f=await fixture({busy:true});try{f.click('connect');await until(()=>f.calls.includes('acquire'));
    await until(()=>f.html().includes('Read only'));
    assert.equal(f.session.store.canControl(key),false);
    assert.ok(!f.calls.some(c=>['execute','prepare','stop'].includes(c)));
    assert.match(f.html(),/In use/);
  }finally{f.restore();}
});

test('duplicate Connect clicks cannot race through two admissions',async()=>{
  const gate=deferred(),f=await fixture({acquireGate:gate});try{f.click('connect');f.click('connect');
    await until(()=>f.calls.includes('acquire'));gate.resolve();await until(()=>f.calls.includes('execute'));await tick();
    assert.equal(f.calls.filter(c=>c==='acquire').length,1);assert.equal(f.calls.filter(c=>c==='execute').length,1);
  }finally{gate.resolve();f.restore();}
});

test('an unknown connection result retains management instead of replaying Connect',async()=>{
  const f=await fixture({unknown:true});try{f.click('connect');await until(()=>f.calls.includes('execute'));await tick();
    assert.equal(f.calls.filter(c=>c==='execute').length,1);
    assert.match(f.html(),/Check status/);assert.match(f.html(),/data-op="disconnect"/);
    assert.doesNotMatch(f.html(),/<button[^>]*data-op="connect"[^>]*>Connect<\/button>/);
  }finally{f.restore();}
});

test('Disconnect waits for authoritative release instead of presenting an acceptance as disconnected',async()=>{
  const f=await fixture();try{f.click('connect');await until(()=>f.html().includes('>Disconnect</button>'));
    f.click('disconnect');await until(()=>f.calls.includes('stop'));await tick();
    assert.equal(f.session.store.lease(key),null);assert.match(f.html(),/Disconnecting/);
    assert.doesNotMatch(f.html(),/data-op="connect"/);
    Object.assign(f.state.domains['device:'+d],{state:'DISCONNECTED',device:null,context:{...f.state.domains['device:'+d].context,connection_id:null,epoch:2}});
    f.state.control['device:'+d]={state:'AVAILABLE',controller_session:null,control_epoch:1};f.publish();
    assert.match(f.html(),/>Connect<\/button>/);assert.doesNotMatch(f.html(),/Disconnecting/);
    assert.equal(f.calls.filter(c=>c==='stop').length,1);
  }finally{f.restore();}
});

test('a lease already obtained by supervised setup is synchronized rather than acquired twice',async()=>{
  const f=await fixture();try{
    f.state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:0};f.session.store.setLease(key,f.lease);
    f.click('connect');await until(()=>f.calls.includes('execute'));await tick();
    assert.equal(f.calls.filter(c=>c==='acquire').length,0);assert.equal(f.calls.filter(c=>c==='execute').length,1);
  }finally{f.restore();}
});

test('an uncertain authority reply cannot offer another Connect from an old available snapshot',async()=>{
  const f=await fixture({acquireUnknown:true});try{f.click('connect');await until(()=>f.calls.includes('acquire'));await tick();
    assert.match(f.html(),/Retry disconnect/);assert.doesNotMatch(f.html(),/data-op="connect"/);
    f.click('connect');await tick();assert.equal(f.calls.filter(c=>c==='acquire').length,1);
  }finally{f.restore();}
});

test('context changed during acquisition releases only the returned token and never opens',async()=>{
  const gate=deferred(),f=await fixture({acquireGate:gate});try{f.click('connect');await until(()=>f.calls.includes('acquire'));
    f.state.domains['device:'+d].context={...f.state.domains['device:'+d].context,epoch:1};f.publish();gate.resolve();
    await until(()=>f.calls.includes('release'));await tick();
    assert.ok(!f.calls.some(c=>['execute','prepare','stop'].includes(c)));assert.equal(f.session.store.lease(key),null);
  }finally{gate.resolve();f.restore();}
});

test('pre-admission failure releases a newly acquired empty session',async()=>{
  const f=await fixture({rejectPrepare:true});try{f.click('connect');await until(()=>f.calls.includes('release'));await tick();
    assert.ok(!f.calls.includes('execute'));assert.equal(f.session.store.lease(key),null);assert.match(f.html(),/>Connect<\/button>/);
  }finally{f.restore();}
});

test('a failed Disconnect keeps a recovery action and never claims release',async()=>{
  const f=await fixture({stopFailure:true});try{f.click('connect');await until(()=>f.html().includes('>Disconnect</button>'));
    f.click('disconnect');await until(()=>f.calls.includes('stop'));await tick();
    assert.match(f.html(),/Retry disconnect/);assert.doesNotMatch(f.html(),/data-op="connect"/);
    assert.equal(f.state.domains['device:'+d].device.connected,true);
  }finally{f.restore();}
});

test('unknown acquire cleanup cannot reuse the pre-stop available snapshot or admit another Connect',async()=>{
  const gate=deferred(),f=await fixture({acquireUnknown:true,stopGate:gate});try{
    f.click('connect');await until(()=>f.html().includes('Retry disconnect'));
    f.click('disconnect');await until(()=>f.calls.includes('stop'));await tick();
    assert.match(f.html(),/Disconnecting/);assert.doesNotMatch(f.html(),/data-op="connect"/);
    f.click('connect');await tick();assert.equal(f.calls.filter(c=>c==='acquire').length,1);
    gate.resolve();await until(()=>f.calls.filter(c=>c==='sync').length>0);
  }finally{gate.resolve();await tick();f.restore();}
});

test('a lost acquire reply permits cleanup of only this authenticated session, never replay',async()=>{
  const f=await fixture({acquireAppliedUnknown:true});try{
    f.click('connect');await until(()=>f.calls.includes('acquire'));await tick();
    assert.match(f.html(),/>Retry disconnect<\/button>/);
    assert.doesNotMatch(f.html().match(/<button[^>]*data-op="disconnect"[^>]*>/)?.[0]||'',/disabled/);
    f.click('disconnect');await until(()=>f.calls.includes('stop'));
    assert.ok(!f.calls.some(c=>['prepare','execute'].includes(c)));
  }finally{await tick();f.restore();}
});

test('uncertain acquisition never grants cleanup authority over a foreign controller',async()=>{
  const f=await fixture({acquireAppliedUnknown:true,foreignOwner:true});try{
    f.click('connect');await until(()=>f.calls.includes('acquire'));await tick();
    assert.match(f.html(),/In use · Read only/);
    assert.match(f.html().match(/<button[^>]*data-op="disconnect"[^>]*>/)?.[0]||'',/disabled/);
    f.click('disconnect');await tick();assert.ok(!f.calls.includes('stop'));
  }finally{f.restore();}
});

test('an owned stopped instrument can explicitly resume without reconnecting',async()=>{
  const f=await fixture();try{
    f.state.registry.devices[0].model_id='voltage';f.state.registry.devices[0].name='Voltage';
    f.state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:0};
    Object.assign(f.state.domains['device:'+d],{state:'STOP_HELD',device:{connected:true,channels:[]},
      context:{...f.state.domains['device:'+d].context,connection_id:'3'.repeat(32),epoch:1},
      safety:{state:'STOP_HELD',attempts:[{attempt_id:'original-safe-call',ok:true}],outcomes:{voltage_zero:'unmeasured'}}});
    f.session.store.setLease(key,f.lease);f.publish();
    assert.match(f.html(),/data-op="resume"/);assert.match(f.html(),/original-safe-call/);
    f.click('resume');await until(()=>f.calls.includes('execute'));
    assert.equal(f.calls.filter(c=>c==='acquire').length,0);assert.equal(f.calls.filter(c=>c==='stop').length,0);
    f.session.store.dropLease(key);f.publish();assert.doesNotMatch(f.html(),/data-op="resume"/);
  }finally{f.restore();}
});

test('a failed cleanup cannot claim release from later unrelated available snapshots',async()=>{
  const f=await fixture({acquireUnknown:true,stopFailure:true});try{
    f.click('connect');await until(()=>f.html().includes('Retry disconnect'));
    f.click('disconnect');await until(()=>f.calls.includes('stop'));await tick();f.publish();
    assert.match(f.html(),/Retry disconnect/);assert.doesNotMatch(f.html(),/data-op="connect"/);
  }finally{f.restore();}
});

test('lost-token cleanup remains fenced to the original control epoch',async()=>{
  const f=await fixture({acquireAppliedUnknown:true});try{
    f.click('connect');await until(()=>f.html().includes('Retry disconnect'));
    f.state.control['device:'+d].control_epoch=1;f.publish();
    assert.match(f.html(),/In use · Read only/);f.click('disconnect');await tick();assert.ok(!f.calls.includes('stop'));
  }finally{f.restore();}
});

test('the error notice has its underlying shared-result evidence in collapsed Diagnostics',async()=>{
  const f=await fixture();try{
    const html=renderConsole(route,f.session.store.host(h),f.session.store,{[key]:{sharedResultError:'Trace hash mismatch'}});
    assert.match(html,/See Diagnostics/);assert.match(html,/<details id="instrument-diagnostics">/);assert.match(html,/Trace hash mismatch/);
  }finally{f.restore();}
});

test('a concurrent pre-stop snapshot cannot serve as post-acceptance release evidence',async()=>{
  const stopGate=deferred(),pingGate=deferred(),f=await fixture({acquireUnknown:true,stopGate});try{
    f.click('connect');await until(()=>f.html().includes('Retry disconnect'));
    f.click('disconnect');await until(()=>f.calls.includes('stop'));
    f.client.ping=async()=>{f.calls.push('ping-blocked');await pingGate.promise;return {monotonic_ms:performance.now(),client_session_id:s,boot_id:b};};
    const oldSync=f.ui.resync();await until(()=>f.calls.includes('ping-blocked'));
    stopGate.resolve();await tick();assert.equal(f.state.control['device:'+d].state,'RETAINED');
    pingGate.resolve();await oldSync;await tick();
    assert.doesNotMatch(f.html(),/data-op="connect"/);
    await until(()=>f.calls.filter(c=>c==='sync').length===2);
    assert.equal(f.session.store.host(h).control['device:'+d].state,'RETAINED');
  }finally{stopGate.resolve();pingGate.resolve();await tick();f.restore();}
});
