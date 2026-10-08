import test from 'node:test';
import assert from 'node:assert/strict';
import {setImmediate as tick} from 'node:timers/promises';
import {mountConsole,renderConsole} from '../web/console-ui.js';
import {createConsoleSession} from '../web/main.js';
import {savedTrace} from './osa-fixture.mjs';
import {createRemoteClient} from '../web/remote-client.js';
import {releasedDomainWithCachedSample} from './released-domain-fixture.mjs';

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
  const nodes=new Map(),listeners=new Map(),calls=[],intervals=[],windowEvents=new Map();
  const node=selector=>{if(!nodes.has(selector))nodes.set(selector,{innerHTML:'',textContent:'',hidden:true,
    querySelectorAll:()=>[],addEventListener:(name,fn)=>{const key=selector+':'+name;listeners.set(key,[...(listeners.get(key)||[]),fn]);}});return nodes.get(selector);};
  let seq=0,subscriber,ui;
  const context={session_id:'f'.repeat(32),domain,connection_id:null,epoch:0};
  const state={host_id:h,host_name:'Test Host',mode:'real',control:{['device:'+d]:{state:'AVAILABLE',controller_session:null,control_epoch:0}},
    registry:{registry_rev:1,devices:[{device_id:d,name:'Bench OSA',model_id:'aq6370',profile_id:'gpib-visa',params:{resource:'GPIB0::4::INSTR'},config_rev:1}],drafts:[],setups:[]},
    domains:{['device:'+d]:{state:'DISCONNECTED',device:null,context,host_sample_ms:0}}};
  if(options.freshGainDraft){
    const existingId='8'.repeat(32),existingDomain={kind:'device',id:existingId};
    state.registry.devices=[{device_id:existingId,name:'Connected Laser',model_id:'tlb6700',profile_id:'newport-usb',params:{device_key:'6700 SN22500001'},config_rev:1}];
    state.control={['device:'+existingId]:{state:'CONTROLLED',controller_session:'9'.repeat(32),control_epoch:4}};
    state.domains={['device:'+existingId]:{state:'READY',device:{connected:true,state:'READY',identity:'TLB-6722'},context:{session_id:'f'.repeat(32),domain:existingDomain,connection_id:'7'.repeat(32),epoch:6},host_sample_ms:0}};
  }
  const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
  function publish(){subscriber?.({type:'snapshot',host_id:h,boot_id:b,seq:++seq,data:state});}
  const client={preferences:async()=>({pythonPath:'VISA'}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},
    chooseDataRoot:async()=>options.chosenFolder??null,
    saveSettings:async params=>{calls.push('save-settings');assert.equal(params.expected_rev,state.registry.registry_rev);if(options.saveSettingsFailure)throw new Error('Settings save failed');state.registry.settings=params.settings;state.registry.registry_rev++;return state.registry;},
    driverStatus:async()=>{calls.push('drivers');return options.inventory||{newport:{sdk:{state:'ready'},devices:[]}};},
    catalog:async()=>({models:options.models||[model],categories:['OSA']}),subscribe:async fn=>{subscriber=fn;return ()=>{};},
    requestSnapshot:async()=>{calls.push('sync');publish();return {seq};},ping:async()=>({monotonic_ms:performance.now(),client_session_id:s,boot_id:b}),
    acquire:async selected=>{assert.deepEqual(selected,domain);calls.push('acquire');await options.acquireGate?.promise;
      if(options.acquireUnknown)throw Object.assign(new Error('Permission reply unavailable'),{outcomeUnknown:true});
      if(options.acquireAppliedUnknown){state.control['device:'+d]={state:'CONTROLLED',controller_session:options.foreignOwner?'2'.repeat(32):s,control_epoch:0};publish();throw Object.assign(new Error('Permission reply unavailable'),{outcomeUnknown:true});}
      if(options.busy){state.control['device:'+d]={state:'CONTROLLED',controller_session:'2'.repeat(32),control_epoch:0};publish();throw Object.assign(new Error('Already in use'),{code:'ControlOwned'});}
      lease.control_epoch=state.control['device:'+d].control_epoch;state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:lease.control_epoch};return lease;},
    prepare:async intent=>{calls.push('prepare');assert.equal(intent.lease_token,lease.token);await options.prepareGate?.promise;if(options.rejectPrepare)throw new Error('Connection preparation declined');return {token:'proof'};},
    createDraft:async params=>{calls.push('draft');const draft={device_id:d,revision:1,mode:'real',config_digest:'digest',model_id:params.model_id,profile_id:params.profile_id,params:params.params,name:params.name};state.registry.devices=state.registry.devices.filter(record=>record.device_id!==d);state.registry.drafts=[draft];
      if(options.freshGainDraft){state.registry.registry_rev++;state.control['device:'+d]={state:'AVAILABLE',controller_session:null,control_epoch:0};state.domains['device:'+d]={state:'DISCONNECTED',device:null,context,host_sample_ms:0};}
      return draft;},
    testConnection:async params=>{calls.push('test');if(options.supervisedGuard&&state.domains['device:'+d].device?.connected!==true)throw new Error('No independently authorized session');await options.testGate?.promise;if(options.testFailure)throw new Error('Selected controller is unplugged');return {proof_id:'verified',release_confirmed:true};},
    saveDevice:async()=>{calls.push('save');const record={...state.registry.drafts[0],config_rev:1};state.registry.devices=[...state.registry.devices.filter(saved=>saved.device_id!==record.device_id),record];state.registry.drafts=[];return record;},
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
    globalThis.window={addEventListener:(name,fn)=>windowEvents.set(name,fn)};globalThis.location={hash:options.route||route};
    globalThis.confirm=()=>{calls.push('confirm');return options.consent!==false;};
    globalThis.setInterval=(fn,ms)=>{intervals.push({fn,ms});return intervals.length;};globalThis.clearInterval=()=>{};
    const session=createConsoleSession(client,()=>ui?.render());
    if(options.remote){session.addRemote(options.remote.hostId,options.remote.client);session.setCatalog(options.remote.hostId,{models:options.models||[model],categories:['OSA']});session.apply(options.remote.snapshot,true);}
    ui=mountConsole(session,options.native||{event:{listen(){}}});await ui.ready;calls.length=0;
    return {session,client,state,calls,ui,publish,lease,intervals,html:()=>node('#content').innerHTML,notice:()=>node('#notice'),
      navigate(hash){globalThis.location.hash=hash;windowEvents.get('hashchange')?.();},
      background:()=>node('#background-work').innerHTML,
      click(op,data={}){for(const listener of listeners.get('#content:click')||[])listener({target:{closest:selector=>selector==='button'?{disabled:false,dataset:{op,...data}}:null}});},
      clickUi(ui,host){for(const listener of listeners.get('#content:click')||[])listener({target:{closest:selector=>selector==='button'?{disabled:false,dataset:{ui,host}}:null}});},
      uiClick(name,data={}){for(const listener of listeners.get('#content:click')||[])listener({target:{closest:selector=>selector==='button'?{disabled:false,dataset:{ui:name,...data}}:null}});},
      change(dataset,value){for(const listener of listeners.get('#content:change')||[])listener({target:{dataset,value}});},
      backgroundClick(){for(const listener of listeners.get('#content:click')||[])listener({target:{closest:selector=>selector==='[data-wizard-backdrop]'?{}:null}});},
      value(id,value){node('#'+id).value=value;},
      restore(){ui.clearNotice?.();for(const [k,p]of prior){if(p.exists)globalThis[k]=p.value;else delete globalThis[k];}}};
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

test('slow connection stays visibly active across navigation without a second admission',async()=>{
 const gate=deferred(),f=await fixture({prepareGate:gate});try{
  f.click('connect');await until(()=>f.calls.includes('prepare'));
  assert.match(f.html(),/operation-feedback/);assert.match(f.html(),/Preparing/);
  f.navigate('#overview');assert.match(f.background(),/Bench OSA/);assert.match(f.background(),/Preparing/);
  f.navigate(route);assert.match(f.html(),/operation-feedback/);f.click('connect');await tick();
  assert.equal(f.calls.filter(c=>c==='acquire').length,1);
  gate.resolve();await until(()=>f.html().includes('>Disconnect</button>'));await tick();
  assert.doesNotMatch(f.background(),/Preparing/);
 }finally{gate.resolve();await tick();await tick();f.restore();}
});
test('elapsed feedback never rebuilds the page or navigation',async()=>{
 const gate=deferred(),f=await fixture({prepareGate:gate});try{
  f.click('connect');await until(()=>f.calls.includes('prepare'));let replacements=0;
  const node=document.querySelector('#content'),before=node.innerHTML;
  Object.defineProperty(node,'innerHTML',{get:()=>before,set:()=>replacements++});
  const timer=f.intervals.find(i=>i.ms===1000);assert.ok(timer);timer.fn();assert.equal(replacements,0);
  gate.resolve();await until(()=>f.calls.includes('execute'));
 }finally{gate.resolve();await tick();await tick();f.restore();}
});

test('late operation completion cannot finish a newer disconnect wait',async()=>{
 const gate=deferred(),f=await fixture({executeGate:gate});try{
  f.click('connect');await until(()=>f.calls.includes('execute'));
  f.clickUi('safe-stop');await until(()=>f.calls.includes('stop'));await tick();
  assert.match(f.html(),/Disconnecting/);gate.resolve();await tick();await tick();
  assert.match(f.html(),/activity-spinner/);assert.doesNotMatch(f.html(),/Release check finished/);
 }finally{gate.resolve();await tick();await tick();f.restore();}
});

test('original status recovery removes the unknown activity without executing again',async()=>{
 const f=await fixture({unknown:true});try{
  f.click('connect');await until(()=>!f.notice().hidden);assert.match(f.html(),/Outcome unknown/);
  const before=f.calls.filter(c=>c==='execute').length;
  f.client.operation=async requestId=>{f.calls.push('query');return {request_id:requestId,domain,status:'Terminal',phase:'rejected_before_call',result:{context:f.state.domains['device:'+d].context}};};
  f.clickUi('query-original');await until(()=>f.calls.includes('query'));await tick();
  assert.doesNotMatch(f.html(),/Outcome unknown|Operation result is uncertain/);assert.equal(f.calls.filter(c=>c==='execute').length,before);
 }finally{f.restore();}
});
test('a delayed status recovery cannot clear a newer failed disconnect',async()=>{
 const gate=deferred(),f=await fixture({unknown:true,stopFailure:true});try{
  f.click('connect');await until(()=>!f.notice().hidden);
  f.client.operation=async requestId=>{f.calls.push('query');await gate.promise;return {request_id:requestId,domain,status:'Terminal',phase:'completed',result:{context:f.state.domains['device:'+d].context}};};
  f.clickUi('query-original');await until(()=>f.calls.includes('query'));
  f.clickUi('safe-stop');await until(()=>f.calls.includes('stop'));await tick();gate.resolve();await tick();await tick();
  assert.match(f.html(),/Release unconfirmed/);assert.match(f.html(),/Outcome unknown/);assert.doesNotMatch(f.html(),/Release check finished/);
 }finally{gate.resolve();await tick();f.restore();}
});
test('status recovery stays on the original owning Host while navigation remains usable',async()=>{
 const gate=deferred(),f=await fixture({unknown:true});try{
  f.click('connect');await until(()=>!f.notice().hidden);
  f.client.operation=async requestId=>{f.calls.push('query');await gate.promise;return {request_id:requestId,domain,status:'Terminal',phase:'completed',result:{context:f.state.domains['device:'+d].context}};};
  f.clickUi('query-original');await until(()=>f.calls.includes('query'));assert.match(f.html(),/Checking status/);
  f.navigate('#/host/'+'9'.repeat(32)+'/device/'+d);gate.resolve();await tick();await tick();
  f.navigate(route);assert.doesNotMatch(f.html(),/Outcome unknown|Operation result is uncertain/);
 }finally{gate.resolve();await tick();f.restore();}
});
test('an immutable capture export keeps feedback while another operation runs',async()=>{
 const capture=await savedTrace(),gate=deferred(),f=await fixture();try{
  f.click('connect');await until(()=>f.html().includes('>Disconnect</button>'));
  for(const method of ['archiveManifestBytes','readArchive'])f.client[method]=capture.client[method];f.client.exportArchive=async()=>{f.calls.push('export');return gate.promise;};
  f.state.operations=[{operation_id:capture.reference.id,domain,status:'Terminal',phase:'completed',command:{method:'action',params:{name:'read_trace',args:{trace:'A'}}},result:{context:f.state.domains['device:'+d].context,result:{archive_ref:capture.reference}}}];
  f.publish();f.ui.render();await until(()=>f.html().includes('data-osa-plot'));
  f.click('osa-export');await until(()=>f.calls.includes('export'));assert.match(f.html(),/Choose folder/);
  f.click('resume');await until(()=>f.calls.filter(c=>c==='execute').length===2);await tick();assert.match(f.html(),/Choose folder/);
  gate.resolve(null);await tick();assert.match(f.html(),/Export cancelled/);
 }finally{gate.resolve(null);await tick();f.restore();}
});

async function captureRecoveryFixture(){
  const remoteId='9'.repeat(32),otherId='e'.repeat(32),remoteCalls=[],capture=await savedTrace({id:'2'.repeat(32)}),gate=deferred();let session,remoteSeq=1;
  const context={session_id:'f'.repeat(32),domain,connection_id:'3'.repeat(32),epoch:1};
  const remoteState={host_id:remoteId,host_name:'Remote OSA',mode:'real',registry:{registry_rev:1,devices:[{device_id:d,name:'Other Host OSA',model_id:'aq6370',profile_id:'gpib-visa',config_rev:1,params:{resource:'GPIB0::4::INSTR'}}],drafts:[],setups:[]},control:{['device:'+d]:{state:'AVAILABLE',control_epoch:0}},domains:{['device:'+d]:{state:'READY',device:{connected:true},context}}};
  const remoteEvent=()=>({type:'snapshot',host_id:remoteId,boot_id:b,seq:remoteSeq++,data:remoteState});
  const remoteClient={requestSnapshot:async()=>{remoteCalls.push('sync-remote');const e=remoteEvent();session.apply(e,true);return {seq:e.seq};},ping:async()=>({boot_id:b,client_session_id:s,monotonic_ms:performance.now()}),archiveManifestBytes:async()=>{remoteCalls.push('manifest-remote');throw Error('Foreign archive read');}};
  const f=await fixture({remote:{hostId:remoteId,client:remoteClient,snapshot:remoteEvent()}});session=f.session;
  try{
    f.click('connect');await until(()=>f.html().includes('>Disconnect</button>'));
    f.state.registry.devices.push({...f.state.registry.devices[0],device_id:otherId,name:'Other local OSA'});
    f.state.domains['device:'+otherId]={state:'READY',device:{connected:true},context:{...context,domain:{kind:'device',id:otherId}}};f.state.control['device:'+otherId]={state:'AVAILABLE',control_epoch:0};f.publish();f.calls.length=0;
    f.client.recoverCapture=async params=>{f.calls.push('recover-local');assert.deepEqual(params,{domain,capture_id:'d'.repeat(32),name:'osa'});await gate.promise;return {archive_ref:capture.reference};};
    f.client.archiveManifestBytes=p=>{f.calls.push('manifest-local');return capture.client.archiveManifestBytes(p);};
    f.client.readArchive=p=>{f.calls.push('chunk-local');return capture.client.readArchive(p);};
    f.value('osa-name','osa');
    return {...f,capture,remoteCalls,remoteRoute:'#/host/'+remoteId+'/device/'+d,otherRoute:'#/host/'+h+'/device/'+otherId,
      start(){f.click('osa-retry-save',{capture:'d'.repeat(32)});},complete(){gate.resolve();},
      loseBoot(){session.apply({type:'snapshot',host_id:h,boot_id:'8'.repeat(32),seq:1000,data:f.state});},
      restore(){gate.resolve();f.restore();}};
  }catch(cause){gate.resolve();f.restore();throw cause;}
}

test('OSA retry-save synchronizes its owner and hydrates a still-selected matching scope',async()=>{
  const f=await captureRecoveryFixture();try{
    f.start();await until(()=>f.calls.includes('recover-local'));f.complete();await until(()=>!f.notice().hidden);
    assert.equal(f.calls.filter(c=>c==='sync').length,1);assert.deepEqual(f.remoteCalls,[]);assert.match(f.notice().textContent,/Original spectrum saved/);
    assert.ok(f.calls.includes('manifest-local'));assert.match(f.html(),/Historical capture/);assert.match(f.html(),/data-osa-plot/);
  }finally{await tick();f.restore();}
});
for(const destination of ['remote','other-device','overview'])test('OSA retry-save retains truthful owner completion after navigation to '+destination,async()=>{
  const f=await captureRecoveryFixture();try{
    f.start();await until(()=>f.calls.includes('recover-local'));f.navigate(destination==='remote'?f.remoteRoute:destination==='other-device'?f.otherRoute:'#overview');f.complete();await until(()=>!f.notice().hidden);
    assert.deepEqual(f.remoteCalls,[],'recovery must not synchronize or fetch bytes from the newly selected Host');assert.equal(f.calls.filter(c=>c==='sync').length,1);
    assert.ok(!f.calls.some(c=>['manifest-local','chunk-local'].includes(c)),'another selected archive scope must not hydrate the recovered capture');
    assert.match(f.notice().textContent,/Original spectrum saved/);assert.doesNotMatch(f.notice().textContent,/another Host|unavailable|busy|cancelled/);
    f.navigate(route);assert.match(f.html(),/Operation finished/);assert.doesNotMatch(f.html(),/Operation failed|Saving original spectrum/);assert.match(f.html(),/&quot;phase&quot;: &quot;completed&quot;/);
  }finally{await tick();f.restore();}
});
for(const changeAt of ['recover-reply','owner-sync'])test('OSA retry-save cannot publish or hydrate old recovery after boot loss at '+changeAt,async()=>{
  const f=await captureRecoveryFixture();try{
    if(changeAt==='owner-sync')f.client.requestSnapshot=async()=>{f.calls.push('sync');f.loseBoot();return {seq:1000};};
    f.start();await until(()=>f.calls.includes('recover-local'));if(changeAt==='recover-reply')f.loseBoot();f.complete();await until(()=>!f.notice().hidden);
    assert.equal(f.calls.filter(c=>c==='sync').length,changeAt==='owner-sync'?1:0);assert.deepEqual(f.remoteCalls,[]);assert.ok(!f.calls.some(c=>['manifest-local','chunk-local'].includes(c)));
    assert.match(f.notice().textContent,/Host changed/i);assert.doesNotMatch(f.notice().textContent,/Original spectrum saved/);assert.match(f.html(),/Outcome unknown/);assert.doesNotMatch(f.html(),/&quot;archive_ref&quot;/);
  }finally{await tick();f.restore();}
});
for(const change of ['navigation','boot'])test('OSA retry-save fences '+change+' while original archive hydration drains',async()=>{
  const f=await captureRecoveryFixture(),gate=deferred(),manifest=f.client.archiveManifestBytes;
  try{
    f.client.archiveManifestBytes=async p=>{const reply=await manifest(p);await gate.promise;return reply;};
    f.start();await until(()=>f.calls.includes('recover-local'));f.complete();await until(()=>f.calls.includes('manifest-local'));
    if(change==='navigation')f.navigate(f.remoteRoute);else f.loseBoot();gate.resolve();await until(()=>!f.notice().hidden);
    assert.deepEqual(f.remoteCalls,[]);assert.equal(f.calls.filter(c=>c==='sync').length,1);assert.ok(!f.calls.includes('chunk-local'));
    if(change==='navigation'){assert.match(f.notice().textContent,/Original spectrum saved/);f.navigate(route);assert.match(f.html(),/Operation finished/);assert.doesNotMatch(f.html(),/data-osa-plot/);}
    else{assert.match(f.notice().textContent,/Host changed/i);assert.match(f.html(),/Outcome unknown/);assert.doesNotMatch(f.html(),/data-osa-plot|&quot;archive_ref&quot;/);}
  }finally{gate.resolve();await tick();f.restore();}
});

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

async function remoteLaserFixture(){
  const remoteId='9'.repeat(32),calls=[];let session,seq=1;
  const laser={...model,id:'tlb6700'},context={session_id:'f'.repeat(32),domain,connection_id:'3'.repeat(32),epoch:1};
  const device={connected:true,sample_age_s:.1,wavelength_range_nm:[1045,1085],operating_range_nm:[1059,1062],max_scan_speed_nm_s:10,operating_max_speed_nm_s:1,laser:{operation_complete:true,output_enabled:false,remote:false}};
  const state={host_id:remoteId,host_name:'Remote Laser',mode:'real',control:{['device:'+d]:{state:'CONTROLLED',controller_session:s,control_epoch:0}},registry:{registry_rev:1,devices:[{device_id:d,name:'Laser',model_id:'tlb6700',profile_id:'gpib-visa',params:{device_key:'6700 SN1012'},config_rev:1}],setups:[],drafts:[]},domains:{['device:'+d]:{state:'READY',device,context,host_sample_ms:0}}};
  const event=()=>({type:'snapshot',host_id:remoteId,boot_id:b,seq:seq++,data:state});
  // Real remote/Host adapters; only the native TLS bridge is a bounded double.
  const client=createRemoteClient(async(command,{hostId,request})=>{
    assert.equal(command,'remote_call');assert.equal(hostId,remoteId);calls.push(request.method);let result;
    switch(request.method){
      case 'ping':result={boot_id:b,client_session_id:s,monotonic_ms:performance.now()};break;
      case 'request_snapshot':{const e=event();session.apply(e,true);result={seq:e.seq};break;}
      case 'safe_stop':state.control['device:'+d]={state:'AVAILABLE',control_epoch:1};Object.assign(state.domains['device:'+d],{state:'DISCONNECTED',device:null,context:{...context,connection_id:null,epoch:2}});result={accepted:true};break;
      case 'save_laser_limits':throw Object.assign(new Error('Configure the owning Host on its local computer'),{code:'RemoteMethod'});
      default:throw new Error('Unexpected remote method '+request.method);
    }
    return {v:1,id:request.id,ok:true,result,error:null};
  },async()=>()=>{},remoteId);
  const f=await fixture({models:[model,laser],route:'#/host/'+remoteId+'/device/'+d,remote:{hostId:remoteId,client,snapshot:event()}});session=f.session;
  session.store.setLease(remoteId+'/device/'+d,{token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000});
  await f.ui.resync();calls.length=0;f.ui.render();
  return {...f,remoteId,remoteCalls:calls,remoteState:state};
}

test('remote Laser limits are read only with local Host guidance while reads and controls remain enabled',async()=>{
  const f=await remoteLaserFixture();try{
    assert.equal(f.session.store.host(f.remoteId).remote,true);
    for(const id of ['laser-limit-min','laser-limit-max','laser-limit-speed'])assert.match(f.html().match(new RegExp('<input[^>]*id="'+id+'"[^>]*>'))?.[0]||'',/disabled/);
    assert.match(f.html(),/id="laser-limit-min"[^>]*value="1059"/);assert.match(f.html(),/id="laser-limit-max"[^>]*value="1062"/);
    assert.match(f.html(),/data-ui="save-laser-limits"[^>]*disabled/);assert.match(f.html(),/owning Host.*local computer/i);
    assert.match(f.html(),/data-op="laser-read"/);assert.match(f.html(),/data-op="laser-output-on"/);
    assert.doesNotMatch(f.html().match(/<button[^>]*data-op="laser-read"[^>]*>/)?.[0]||'',/disabled/);
    assert.doesNotMatch(f.html().match(/<button[^>]*data-op="laser-output-on"[^>]*>/)?.[0]||'',/disabled/);
    assert.deepEqual(f.remoteCalls,[]);
  }finally{f.restore();}
});

test('remote Laser limits handler rejects direct or stale-render invocation before any side effects',async()=>{
  const f=await remoteLaserFixture();try{
    f.value('laser-limit-min','1050');f.value('laser-limit-max','1080');f.value('laser-limit-speed','1');
    // Deliberately dispatch an enabled button, bypassing the rendered disabled state.
    f.uiClick('save-laser-limits');await until(()=>!f.notice().hidden);
    assert.deepEqual(f.remoteCalls,[],'no stop, save, lease acquisition or reconnect may be sent');
    assert.deepEqual(f.calls,[],'no local fallback is permitted');
    assert.equal(f.remoteState.domains['device:'+d].device.connected,true);
    assert.equal(f.remoteState.registry.registry_rev,1);assert.equal(f.session.store.canControl(f.remoteId+'/device/'+d),true);
    assert.match(f.notice().textContent,/owning Host.*local computer/i);
  }finally{await tick();f.restore();}
});

test('Laser limits save and reconnect stay on the owning local Host across navigation',async()=>{
  const ownerId=h,ownerCalls=[],gate=deferred();let session,seq=10;
  const laser={...model,id:'tlb6700'},context={session_id:'f'.repeat(32),domain,connection_id:'3'.repeat(32),epoch:1};
  const device={connected:true,sample_age_s:.1,wavelength_range_nm:[1045,1085],max_scan_speed_nm_s:10,laser:{operation_complete:true}};
  const state={host_id:ownerId,host_name:'Local Laser',mode:'real',control:{['device:'+d]:{state:'CONTROLLED',controller_session:s,control_epoch:0}},registry:{registry_rev:1,devices:[{device_id:d,name:'Laser',model_id:'tlb6700',profile_id:'gpib-visa',params:{device_key:'6700 SN1012'},config_rev:1}],setups:[],drafts:[]},domains:{['device:'+d]:{state:'READY',device,context,host_sample_ms:0}}};
  const event=()=>({type:'snapshot',host_id:ownerId,boot_id:b,seq:seq++,data:state});
  const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
  const client={
    safeStop:async()=>{ownerCalls.push('stop');await gate.promise;state.control['device:'+d]={state:'AVAILABLE',control_epoch:1};Object.assign(state.domains['device:'+d],{state:'DISCONNECTED',device:null,context:{...context,connection_id:null,epoch:2}});return {accepted:true};},
    requestSnapshot:async()=>{const e=event();session.apply(e);return {seq:e.seq};},ping:async()=>({boot_id:b,client_session_id:s,monotonic_ms:performance.now()}),
    saveLaserLimits:async params=>{ownerCalls.push('save-limits');assert.equal(params.device_id,d);assert.equal(params.config_rev,1);assert.deepEqual(params.limits,{min_nm:1050,max_nm:1080,max_speed_nm_s:1});state.registry.registry_rev=2;state.registry.devices[0].config_rev=2;},
    acquire:async()=>{ownerCalls.push('acquire');state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:1};return {...lease,control_epoch:1};},prepare:async()=>({token:'proof'}),nextSequence:()=>1,
    execute:async(id,intent)=>{ownerCalls.push('reconnect');assert.equal(intent.method,'connect');assert.equal(intent.config_rev,2);Object.assign(state.domains['device:'+d],{state:'READY',device,context:{...context,epoch:3}});return {request_id:id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context:state.domains['device:'+d].context,result:{}}};},
  };
  const f=await fixture({models:[model,laser]});session=f.session;Object.assign(f.state,state);Object.assign(f.client,client);session.apply(event());await f.ui.resync();
  try{
    session.store.setLease(ownerId+'/device/'+d,lease);f.ui.render();
    const otherId='9'.repeat(32),otherCalls=[];session.addRemote(otherId,createRemoteClient(async(command,{request})=>{otherCalls.push(request.method);throw new Error('Unexpected call to the newly selected remote Host');},async()=>()=>{},otherId));
    session.apply({type:'snapshot',host_id:otherId,boot_id:b,seq:1,data:{...state,host_id:otherId}},true);
    for(const id of ['laser-limit-min','laser-limit-max','laser-limit-speed']){const field=f.html().match(new RegExp('<input[^>]*id="'+id+'"[^>]*>'))?.[0];assert.ok(field);assert.doesNotMatch(field,/disabled/);}
    assert.doesNotMatch(f.html().match(/<button[^>]*data-ui="save-laser-limits"[^>]*>/)?.[0]||'',/disabled/);
    f.value('laser-limit-min','1050');f.value('laser-limit-max','1080');f.value('laser-limit-speed','1');f.uiClick('save-laser-limits');await until(()=>ownerCalls.includes('stop'));
    f.navigate('#/host/'+otherId+'/device/'+d);gate.resolve();await until(()=>ownerCalls.includes('reconnect')||!f.notice().hidden);
    assert.ok(ownerCalls.includes('save-limits'),f.notice().textContent);assert.ok(ownerCalls.includes('reconnect'),f.notice().textContent);
    assert.deepEqual(otherCalls,[]);assert.ok(!f.calls.some(c=>['acquire','execute'].includes(c)));assert.equal(globalThis.location.hash,'#/host/'+otherId+'/device/'+d);
  }finally{gate.resolve();await tick();f.restore();}
});

for(const changeAt of ['post-stop-sync','waiting-release-sync','post-save-sync'])test('Laser limits abort without a later write or reconnect after boot changes at '+changeAt,async()=>{
  const ownerId=h,calls=[];let session,seq=10,boot=b,stopped=false,saved=false,syncs=0;
  const laser={...model,id:'tlb6700'},context={session_id:'f'.repeat(32),domain,connection_id:'3'.repeat(32),epoch:1},device={connected:true,wavelength_range_nm:[1045,1085],max_scan_speed_nm_s:10};
  const state={host_id:ownerId,host_name:'Local Laser',mode:'real',control:{['device:'+d]:{state:'CONTROLLED',controller_session:s,control_epoch:0}},registry:{registry_rev:1,devices:[{device_id:d,name:'Laser',model_id:'tlb6700',profile_id:'gpib-visa',params:{device_key:'6700 SN1012'},config_rev:1}],setups:[],drafts:[]},domains:{['device:'+d]:{state:'READY',device,context,host_sample_ms:0}}};
  const event=()=>({type:'snapshot',host_id:ownerId,boot_id:boot,seq:seq++,data:state}),lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
  const released=()=>{state.control['device:'+d]={state:'AVAILABLE',control_epoch:1};Object.assign(state.domains['device:'+d],{state:'DISCONNECTED',device:null,context:{...context,connection_id:null,epoch:2}});};
  const client={
    safeStop:async()=>{calls.push('stop');stopped=true;if(changeAt==='waiting-release-sync'){state.control['device:'+d]={state:'RETAINED',control_epoch:1};state.domains['device:'+d].state='CLOSING';}else released();return {accepted:true};},
    requestSnapshot:async()=>{if(stopped){syncs++;if(changeAt==='post-stop-sync'||changeAt==='waiting-release-sync'&&syncs===2||changeAt==='post-save-sync'&&saved){if(boot===b)calls.push('boot-change');boot='8'.repeat(32);released();}}const e=event();session.apply(e);return {seq:e.seq};},
    ping:async()=>({boot_id:boot,client_session_id:s,monotonic_ms:performance.now()}),
    saveLaserLimits:async()=>{calls.push('save-limits');saved=true;state.registry.registry_rev=2;state.registry.devices[0].config_rev=2;},
    acquire:async()=>{calls.push('acquire');state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:1};return {...lease,boot_id:boot,control_epoch:1};},prepare:async()=>({token:'proof'}),nextSequence:()=>1,
    execute:async id=>{calls.push('reconnect');Object.assign(state.domains['device:'+d],{state:'READY',device,context:{...context,epoch:3}});return {request_id:id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context:state.domains['device:'+d].context,result:{}}};},
  };
  const f=await fixture({models:[model,laser]});session=f.session;Object.assign(f.state,state);Object.assign(f.client,client);session.apply(event());await f.ui.resync();
  try{
    session.store.setLease(ownerId+'/device/'+d,lease);f.ui.render();f.value('laser-limit-min','1050');f.value('laser-limit-max','1080');f.value('laser-limit-speed','1');f.uiClick('save-laser-limits');
    await until(()=>calls.includes('reconnect')||!f.notice().hidden);
    assert.ok(calls.includes('boot-change'));assert.deepEqual(calls.slice(calls.indexOf('boot-change')+1),[],'no save, acquire or reconnect may continue after the original boot is lost');
    assert.equal(calls.filter(c=>c==='save-limits').length,changeAt==='post-save-sync'?1:0);assert.match(f.notice().textContent,/Host changed/i);assert.match(f.html(),/Retry disconnect|Outcome unknown/);
  }finally{await tick();f.restore();}
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

async function openWizard(f){globalThis.location.hash='#devices';f.ui.render();f.uiClick('add-new');f.change({draft:'modelId'},'aq6370');f.change({param:'resource'},'GPIB0::4::INSTR');}
const gainModel={id:'gain',name:'Gain Chip Driver',category:'Custom',manufacturer:'Yang Lab',profiles:[{id:'cp210x-serial',access:'serial',interfaces:['Serial'],probe_mode:'supervised',open_effects:['DTR_RTS_reset_not_verified'],fields:{port:{kind:'serial',required:true}}}]};
async function openGain(f){f.navigate('#devices');f.uiClick('add-new');f.change({draft:'modelId'},'gain');await until(()=>f.calls.includes('drivers'));await tick();f.change({param:'port'},'COM4');await tick();}
test('a newly registered Gain draft connects through the mounted wizard while a Laser stays connected',async()=>{
 const f=await fixture({models:[gainModel],freshGainDraft:true,route:'#devices',supervisedGuard:true,inventory:{serial:[{resource:'COM4',vid:0x10c4,pid:0xea60,serial:'GAIN-A',instance_id:'USB\\VID_10C4&PID_EA60\\GAIN-A',description:'CP2102'}],usb_serial:{cp210x:{state:'ready',devices:[{instance_id:'USB\\VID_10C4&PID_EA60\\GAIN-A',driver_state:'ready'}]}}}});try{
  const existingId='8'.repeat(32),existing='device:'+existingId,before=structuredClone({domain:f.state.domains[existing],control:f.state.control[existing],record:f.state.registry.devices.find(record=>record.device_id===existingId)});
  assert.equal(f.session.store.get(key),null);
  await openGain(f);f.uiClick('test-draft');await until(()=>f.calls.includes('test')||!f.notice().hidden);await tick();
  assert.ok(f.calls.includes('test'),f.notice().textContent);
  assert.deepEqual(f.calls.filter(c=>['draft','acquire','prepare','execute','test'].includes(c)),['draft','acquire','prepare','execute','test']);
  assert.equal(f.session.store.canControl(key),true);assert.equal(f.state.domains['device:'+d].device.connected,true);
  assert.equal(f.notice().hidden,true);assert.doesNotMatch(f.html().match(/<button[^>]*data-ui="save-draft"[^>]*>/)?.[0]||'',/disabled/);
  assert.deepEqual({domain:f.state.domains[existing],control:f.state.control[existing],record:f.state.registry.devices.find(record=>record.device_id===existingId)},before);
  f.uiClick('save-draft');await until(()=>f.calls.includes('save'));await tick();
  assert.equal(globalThis.location.hash,route);assert.equal(f.state.registry.devices.find(record=>record.device_id===d)?.model_id,'gain');
  assert.deepEqual({domain:f.state.domains[existing],control:f.state.control[existing],record:f.state.registry.devices.find(record=>record.device_id===existingId)},before);
 }finally{f.restore();}
});
test('Gain has one understandable confirmed connection and identity verification flow',async()=>{
 const f=await fixture({models:[gainModel],supervisedGuard:true,inventory:{serial:[{resource:'COM4',vid:0x10c4,pid:0xea60,serial:'GAIN-A',instance_id:'USB\\VID_10C4&PID_EA60\\GAIN-A',description:'CP2102'}],usb_serial:{cp210x:{state:'ready',devices:[{instance_id:'USB\\VID_10C4&PID_EA60\\GAIN-A',driver_state:'ready'}]}}}});try{
  await openGain(f);assert.match(f.html(),/Connect &amp; verify/);assert.doesNotMatch(f.html(),/Prepare supervised session|interlock_shutdown_possible|DTR_RTS_reset_not_verified/);
  f.uiClick('test-draft');await until(()=>f.calls.includes('test')||!f.notice().hidden);await tick();
  assert.ok(f.calls.includes('test'),f.notice().textContent);assert.deepEqual(f.calls.filter(c=>['confirm','acquire','execute','test'].includes(c)),['acquire','execute','test']);
  assert.equal(f.notice().hidden,true);assert.doesNotMatch(f.html().match(/<button[^>]*data-ui="save-draft"[^>]*>/)?.[0]||'',/disabled/);
 }finally{f.restore();}
});
test('ordinary Gain connection requires no separate session or extra confirmation dialog',async()=>{
 const f=await fixture({models:[gainModel],consent:false,inventory:{serial:[{resource:'COM4',vid:0x10c4,pid:0xea60,serial:'GAIN-A',instance_id:'USB\\VID_10C4&PID_EA60\\GAIN-A',description:'CP2102'}],usb_serial:{cp210x:{state:'ready',devices:[{instance_id:'USB\\VID_10C4&PID_EA60\\GAIN-A',driver_state:'ready'}]}}}});try{
  await openGain(f);f.uiClick('test-draft');await until(()=>f.calls.includes('test')||!f.notice().hidden);assert.ok(!f.calls.includes('confirm'));assert.ok(f.calls.includes('execute'));assert.ok(f.calls.includes('test'));
 }finally{f.restore();}
});
test('uncertain Gain connection cannot continue to identity verification or Save',async()=>{
 const f=await fixture({models:[gainModel],unknown:true,inventory:{serial:[{resource:'COM4',vid:0x10c4,pid:0xea60,serial:'GAIN-A',instance_id:'USB\\VID_10C4&PID_EA60\\GAIN-A',description:'CP2102'}],usb_serial:{cp210x:{state:'ready',devices:[{instance_id:'USB\\VID_10C4&PID_EA60\\GAIN-A',driver_state:'ready'}]}}}});try{
  await openGain(f);f.uiClick('test-draft');await until(()=>f.notice().hidden===false);assert.ok(!f.calls.includes('test'));assert.match(f.html(),/data-ui="save-draft"[^>]*disabled/);
 }finally{f.restore();}
});
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
 const f=await fixture();try{await openWizard(f);f.backgroundClick();await tick();assert.doesNotMatch(f.html(),/class="card wizard"/);assert.ok(!f.calls.some(c=>['draft','acquire','prepare','execute','confirm'].includes(c)));}finally{f.restore();}
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

test('native completed disconnect with cached readings returns to Connect and admits a fresh connection',async()=>{
  const f=await fixture();try{
    f.click('connect');await until(()=>f.html().includes('>Disconnect</button>'));
    const sample=structuredClone(f.state.domains['device:'+d].device);
    f.click('disconnect');await until(()=>f.calls.includes('stop'));await tick();
    f.state.domains['device:'+d]=releasedDomainWithCachedSample({...f.state.domains['device:'+d].context,connection_id:null,epoch:2},sample);
    f.state.control['device:'+d]={state:'AVAILABLE',controller_session:null,control_epoch:1};f.publish();
    assert.match(f.html(),/>Connect<\/button>/);assert.doesNotMatch(f.html(),/Disconnecting|Release unconfirmed/);
    assert.equal(f.state.domains['device:'+d].device.connected,true);
    f.click('connect');await until(()=>f.calls.filter(c=>c==='execute').length===2);await tick();
    assert.equal(f.calls.filter(c=>c==='stop').length,1,'release is not replayed to erase cached readings');
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
test('choosing a recording folder persists it once without a hidden Save or Host restart',async()=>{
 const f=await fixture({chosenFolder:'D:/Recordings'});try{
  f.state.registry.settings={host_name:'Bench',data_root:'D:/Previous'};f.publish();f.navigate('#settings');f.uiClick('choose-data-root');await until(()=>f.calls.includes('save-settings'));await tick();
  assert.deepEqual(f.state.registry.settings,{host_name:'Bench',data_root:'D:/Recordings'});assert.equal(f.calls.filter(c=>c==='save-settings').length,1);assert.ok(!f.calls.includes('execute')&&!f.calls.includes('stop'));
 }finally{f.restore();}
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
