import test from 'node:test';
import assert from 'node:assert/strict';
import {setImmediate as tick} from 'node:timers/promises';
import {mountConsole,renderConsole} from '../web/console-ui.js';
import {createConsoleSession} from '../web/main.js';

const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),s='d'.repeat(32);
const domain={kind:'device',id:d},key=h+'/device/'+d,route='#/host/'+h+'/device/'+d;
const model={id:'aq6370',name:'AQ6370',category:'OSA',manufacturer:'Yokogawa',profiles:[{id:'gpib-visa',access:'visa',interfaces:['GPIB'],probe_mode:'readonly',fields:{resource:{kind:'text',required:true}},open_effects:[]}],connect_effects:['read_identity']};
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
    driverStatus:async()=>{calls.push('drivers');return {newport:{sdk:{state:'ready'},devices:[]}};},
    catalog:async()=>({models:[model],categories:['OSA']}),subscribe:async fn=>{subscriber=fn;return ()=>{};},
    requestSnapshot:async()=>{calls.push('sync');publish();return {seq};},ping:async()=>({monotonic_ms:performance.now(),client_session_id:s,boot_id:b}),
    acquire:async selected=>{assert.deepEqual(selected,domain);calls.push('acquire');await options.acquireGate?.promise;
      if(options.acquireUnknown)throw Object.assign(new Error('Permission reply unavailable'),{outcomeUnknown:true});
      if(options.acquireAppliedUnknown){state.control['device:'+d]={state:'CONTROLLED',controller_session:options.foreignOwner?'2'.repeat(32):s,control_epoch:0};publish();throw Object.assign(new Error('Permission reply unavailable'),{outcomeUnknown:true});}
      if(options.busy){state.control['device:'+d]={state:'CONTROLLED',controller_session:'2'.repeat(32),control_epoch:0};publish();throw Object.assign(new Error('Already in use'),{code:'ControlOwned'});}
      lease.control_epoch=state.control['device:'+d].control_epoch;state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:lease.control_epoch};return lease;},
    prepare:async intent=>{calls.push('prepare');assert.equal(intent.lease_token,lease.token);if(options.rejectPrepare)throw new Error('Connection preparation declined');return {token:'proof'};},
    createDraft:async params=>{calls.push('draft');const draft={device_id:d,revision:1,mode:'real',config_digest:'digest',model_id:params.model_id,profile_id:params.profile_id,params:params.params,name:params.name};state.registry.drafts=[draft];return draft;},
    testConnection:async params=>{calls.push('test');await options.testGate?.promise;if(options.testFailure)throw new Error('Selected controller is unplugged');return {proof_id:'verified',release_confirmed:true};},
    saveDevice:async()=>{calls.push('save');const record={...state.registry.drafts[0],config_rev:1};state.registry.devices=[record];state.registry.drafts=[];return record;},
    cancelDraft:async()=>{calls.push('cancel');state.registry.drafts=[];return {cancelled:true};},
    execute:async (id,intent)=>{calls.push('execute');assert.ok(['connect','resume'].includes(intent.method));
      await options.executeGate?.promise;
      if(options.unknown)throw Object.assign(new Error('Connection outcome unknown'),{outcomeUnknown:true});
      if(options.connectFailure&&calls.filter(c=>c==='execute').length===1){
        Object.assign(state.domains['device:'+d],{state:'CONNECTING',context:{...state.domains['device:'+d].context,connection_id:'3'.repeat(32)}});publish();
        const released=options.connectFailure==='released',current=state.domains['device:'+d].context;
        Object.assign(state.domains['device:'+d],{state:released?'DISCONNECTED':'FAULT',device:released?null:{connected:false,state:'FAULT'},context:{...current,connection_id:released?null:'3'.repeat(32),epoch:released?current.epoch+1:current.epoch}});
        return {request_id:id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'failed_after_call_started',result:{context:state.domains['device:'+d].context,error:{message:'Selected controller was not found; '+(released?'resource release confirmed':'release unconfirmed')}}};
      }
      Object.assign(state.domains['device:'+d],{state:'READY',device:{connected:true,state:'READY',identity:'YOKOGAWA,AQ6370D,SN,FW'},context:{...state.domains['device:'+d].context,connection_id:'3'.repeat(32),epoch:1}});
      return {request_id:id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context:state.domains['device:'+d].context,result:{}}};},
    safeStop:async selected=>{assert.deepEqual(selected,domain);calls.push('stop');await options.stopGate?.promise;if(options.stopFailure)throw new Error('Close reply unavailable');
      state.control['device:'+d]={state:'RETAINED',controller_session:null,control_epoch:1};
      state.domains['device:'+d].state='CLOSING';return {accepted:true,physical_stop_confirmed:false};},
    release:async()=>{calls.push('release');state.control['device:'+d]={state:'AVAILABLE',controller_session:null,control_epoch:1};publish();return {accepted:true};},
    saveCheckPolicy:async params=>{calls.push('save-policy');state.registry.devices[0].check_policy={interval_s:params.interval_s};state.registry.registry_rev++;},
    refreshDevice:async params=>{assert.deepEqual(params,{device_id:d,config_rev:1});calls.push('refresh-device');await options.refreshGate?.promise;return {refreshed:true};},
    renew:async()=>lease,nextSequence:()=>calls.filter(c=>c==='execute').length+1};
  try{
    globalThis.document={activeElement:null,querySelector:node,getElementById:id=>node('#'+id)};
    globalThis.window={addEventListener(name,listener){listeners.set('window:'+name,[listener]);}};globalThis.location={hash:route};
    globalThis.confirm=()=>{calls.push('confirm');return options.consent!==false;};
    globalThis.setInterval=()=>1;globalThis.clearInterval=()=>{};
    const session=createConsoleSession(client,()=>ui?.render());
    ui=mountConsole(session,{event:{listen(){}}});await ui.ready;calls.length=0;
    return {session,client,state,calls,ui,publish,lease,html:()=>node('#content').innerHTML,notice:()=>node('#notice'),
      click(op){for(const listener of listeners.get('#content:click')||[])listener({target:{closest:selector=>selector==='button'?{disabled:false,dataset:{op}}:null}});},
      uiClick(name,data={}){for(const listener of listeners.get('#content:click')||[])listener({target:{closest:selector=>selector==='button'?{disabled:false,dataset:{ui:name,...data}}:null}});},
      change(dataset,value){for(const listener of listeners.get('#content:change')||[])listener({target:{dataset,value}});},
      background(){for(const listener of listeners.get('#content:click')||[])listener({target:{closest:selector=>selector==='[data-wizard-backdrop]'?{}:null}});},
      navigate(hash){location.hash=hash;for(const listener of listeners.get('window:hashchange')||[])listener();},
      value(id,value){node('#'+id).value=value;},
      restore(){ui.clearNotice?.();for(const [k,p]of prior){if(p.exists)globalThis[k]=p.value;else delete globalThis[k];}}};
  }catch(error){for(const [k,p]of prior){if(p.exists)globalThis[k]=p.value;else delete globalThis[k];}throw error;}
}

test('an available unowned device offers one enabled Connect without permission buttons',async()=>{
  const f=await fixture();try{
    assert.match(f.html(),/<button[^>]*data-op="connect"[^>]*>Connect<\/button>/);
    assert.doesNotMatch(f.html().match(/<button[^>]*data-op="connect"[^>]*>/)?.[0]||'',/disabled/);
    assert.equal((f.html().match(/data-op="connect"/g)||[]).length,1);
    assert.doesNotMatch(f.html(),/Take control|Control held|Exclusive control|Safe stop \/ release/);
    assert.equal(f.session.store.lease(key),null);
  }finally{f.restore();}
});

async function openWizard(f){globalThis.location.hash='#devices';f.ui.render();f.uiClick('add-new');f.change({draft:'modelId'},'aq6370');f.change({param:'resource'},'GPIB0::4::INSTR');}
test('wizard testing has progress, suppresses duplicate submissions and enables Save only on success',async()=>{
 const gate=deferred(),f=await fixture({testGate:gate});try{await openWizard(f);f.uiClick('test-draft');await until(()=>f.calls.includes('test'));
  assert.match(f.html(),/Testing connection/);assert.match(f.html(),/data-ui="save-draft"[^>]*disabled/);
  f.uiClick('test-draft');await tick();assert.equal(f.calls.filter(c=>c==='test').length,1);
  gate.resolve();await until(()=>{const tag=f.html().match(/<button[^>]*data-ui="save-draft"[^>]*>/)?.[0];return tag&&!tag.includes('disabled');});
  assert.ok(!f.calls.includes('confirm'));assert.doesNotMatch(f.html(),/safe-stop-draft|within 60 seconds/);
  f.uiClick('save-draft');await until(()=>f.calls.includes('save'));await tick();assert.equal(globalThis.location.hash,route);assert.doesNotMatch(f.html(),/class="card wizard"/);
 }finally{gate.resolve();f.restore();}
});
test('failed Test explains its reason in an error notification and cannot enable Save',async()=>{
 const f=await fixture({testFailure:true});try{await openWizard(f);f.uiClick('test-draft');await until(()=>f.notice().hidden===false);
  assert.equal(f.notice().className,'notice error');assert.match(f.notice().textContent,/Connection failed:.*unplugged/);assert.match(f.html(),/data-ui="save-draft"[^>]*disabled/);
 }finally{f.restore();}
});
test('clicking the background cancels an untouched wizard without claiming or opening anything',async()=>{
 const f=await fixture();try{await openWizard(f);f.background();await tick();assert.doesNotMatch(f.html(),/class="card wizard"/);assert.ok(!f.calls.some(c=>['draft','acquire','prepare','execute','confirm'].includes(c)));}finally{f.restore();}
});

test('one Connect click acquires authority before opening and removes successful ID banners',async()=>{
  const f=await fixture();try{f.click('connect');await until(()=>f.html().includes('>Disconnect</button>'));
    assert.equal(f.session.store.canControl(key),true);
    assert.deepEqual(f.calls.filter(c=>['confirm','acquire','prepare','execute'].includes(c)),['acquire','prepare','execute']);
    assert.equal((f.html().match(/data-op="disconnect"/g)||[]).length,1);
    assert.doesNotMatch(f.html(),/Shared acquisition|Control held|Exclusive control/);
    assert.equal(f.notice().hidden,true);
    assert.match(f.html(),/<details[^>]*>[\s\S]*Diagnostics/);
  }finally{f.restore();}
});

test('failed Connect with confirmed release restores enabled Connect without extra Disconnect',async()=>{
  for(const alreadyOwned of [false,true]){
    const f=await fixture({connectFailure:'released'});try{
      if(alreadyOwned){f.state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:0};f.session.store.setLease(key,f.lease);f.publish();}
      f.click('connect');await until(()=>f.notice().hidden===false);
      assert.match(f.notice().textContent,/Selected controller was not found/);
      assert.match(f.html(),/<button[^>]*data-op="connect"[^>]*>Connect<\/button>/);
      assert.doesNotMatch(f.html().match(/<button[^>]*data-op="connect"[^>]*>/)?.[0]||'',/disabled/);
      assert.doesNotMatch(f.html(),/data-op="disconnect"|Connection unavailable/);
      assert.equal(f.calls.filter(c=>c==='execute').length,1,'failed Connect is never automatically replayed');
      assert.equal(f.calls.filter(c=>c==='release').length,alreadyOwned?0:1);assert.ok(!f.calls.includes('stop'));
      f.click('connect');await until(()=>f.html().includes('>Disconnect</button>'));
      assert.equal(f.calls.filter(c=>c==='execute').length,2,'explicit retry uses the released context');
    }finally{f.restore();}
  }
});

test('failed Connect with unconfirmed release retains Disconnect and cannot retry Connect',async()=>{
  const f=await fixture({connectFailure:'retained'});try{
    f.click('connect');await until(()=>f.notice().hidden===false);
    assert.match(f.html(),/data-op="disconnect"/);assert.doesNotMatch(f.html(),/data-op="connect"/);
    assert.ok(!f.calls.includes('release'));assert.ok(!f.calls.includes('stop'));
  }finally{f.restore();}
});

test('rendering a connection never claims authority or opens a device',async()=>{
  const f=await fixture({consent:false});try{f.ui.render();await tick();
    assert.deepEqual(f.calls,[]);assert.equal(f.session.store.lease(key),null);
    assert.equal(f.state.domains['device:'+d].context.connection_id,null);
  }finally{f.restore();}
});

test('explicit Connect does not depend on a browser Yes/No dialog',async()=>{
  const f=await fixture({consent:false});try{f.click('connect');await until(()=>f.calls.includes('execute'));
    assert.ok(!f.calls.includes('confirm'));assert.equal(f.session.store.canControl(key),true);
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
    assert.doesNotMatch(f.html(),/Operation result is uncertain/);
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

test('opening settings automatically reads driver metadata without acquiring an instrument',async()=>{
 const f=await fixture();try{f.navigate('#settings');await until(()=>f.calls.includes('drivers'));await until(()=>/driver-state ready/.test(f.html()));assert.doesNotMatch(f.html(),/Remote Hosts|problem code/);assert.ok(!f.calls.includes('acquire'));assert.ok(!f.calls.includes('execute'));}finally{f.restore();}
});

test('Refresh now uses the saved policy once and never saves an edited interval',async()=>{
 const gate=deferred(),f=await fixture({refreshGate:gate});try{
  f.navigate('#devices');f.value('check-interval-'+d,'60');f.uiClick('refresh-device',{device:d});
  await until(()=>f.calls.includes('refresh-device'));assert.match(f.html(),/Refreshing/);
  f.uiClick('refresh-device',{device:d});await tick();assert.equal(f.calls.filter(c=>c==='refresh-device').length,1);
  assert.ok(!f.calls.includes('save-policy'),'refresh must not persist interval edits or authorize probes');
  assert.equal(f.state.registry.registry_rev,1);assert.equal(f.calls.filter(c=>c==='sync').length,0);
  f.navigate('#overview');assert.match(f.html(),/Instrument overview/);
  gate.resolve();await until(()=>f.calls.includes('sync'));await tick();
  assert.equal(f.calls.filter(c=>c==='sync').length,1);
 }finally{gate.resolve();await tick();f.restore();}
});
test('Save interval commits the policy and synchronizes once without refreshing hardware',async()=>{
 const f=await fixture();try{f.navigate('#devices');f.value('check-interval-'+d,'60');f.uiClick('save-checks',{device:d});
  await until(()=>f.calls.includes('sync'));await tick();
  assert.equal(f.state.registry.devices[0].check_policy.interval_s,60);
  assert.equal(f.calls.filter(c=>c==='sync').length,1);assert.ok(!f.calls.includes('refresh-device'));
 }finally{f.restore();}
});

test('one instrument waiting on hardware does not mark another instrument as busy',async()=>{
 const gate=deferred(),f=await fixture({executeGate:gate});try{
  const other='e'.repeat(32),record={...f.state.registry.devices[0],device_id:other,name:'Other OSA'};
  f.state.registry.devices.push(record);f.state.control['device:'+other]={state:'AVAILABLE',controller_session:null,control_epoch:0};
  f.state.domains['device:'+other]={state:'DISCONNECTED',device:null,context:{session_id:'f'.repeat(32),domain:{kind:'device',id:other},connection_id:null,epoch:0},host_sample_ms:0};f.publish();
  f.click('connect');await until(()=>f.calls.includes('execute'));assert.match(f.html(),/aria-busy="true"/);
  f.navigate('#/host/'+h+'/device/'+other);
  assert.match(f.html(),/<button[^>]*data-op="connect"[^>]*>Connect<\/button>/);
  assert.doesNotMatch(f.html().match(/<button[^>]*data-op="connect"[^>]*>/)?.[0]||'',/disabled|aria-busy/);
  gate.resolve();await tick();
 }finally{gate.resolve();await tick();f.restore();}
});
