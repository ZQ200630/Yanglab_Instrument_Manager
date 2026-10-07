import * as panels from './panels.js';
import {renderDeviceSetup,renderAddWizard,renderSettings,instrumentTarget,signature} from './setup.js';
import {connectionState,connectionEndpoint} from './connections.js';
import {connectLocalHost} from './host-client.js';
import {createRemoteClient} from './remote-client.js';
import {createSetupActions} from './setup-actions.js';
import {connectionView,connectionReleased,ensureInstrumentControl} from './connection.js';
import {instanceView,actionFor} from './instance-view.js';
import {deviceKey,routeFor,parseRoute} from './routes.js';
import {renderInstrumentIcon} from './instrument-icon.js';
import {buildPmSettingAction} from './pm400.js';
import {appendTelemetry,osaCursorIndex} from './view-model.js';
import {renderOverview} from './overview.js';
import {baselineConfirmations} from './operations.js';
import {createSharedResults} from './shared-results.js';
import {createArchiveHistory,exportSelectedTrace,plotFraction} from './osa.js';
export {createSharedResults} from './shared-results.js';
import {pollOriginalOperation,queryOriginalOperation} from './operation-recovery.js';
export {pollOriginalOperation,queryOriginalOperation} from './operation-recovery.js';
import {startActivity,advanceActivity,finishActivity,activityBusy,renderActivity,tickActivities} from './activity.js';
export {startActivity,advanceActivity,finishActivity,renderActivity,tickActivities} from './activity.js';
const esc=panels.esc;
const driver={aq6370:'osa',voltage:'voltage',gain:'gain',pm400:'pm400',mdt693b:'mdt'};
export function createRoutedClient(ownerClient,activeHostId){return new Proxy(Object.create(null),{get(target,method){if(method==='forHost')return ownerClient;return (...args)=>ownerClient(activeHostId())[method](...args);}});}
export function confirmInstrumentAction({method,params,record,model,mode},ask){
  const label=`${String(mode).toUpperCase()} · ${record?.name||'Instrument'}`;
  if(method==='action'&&params.name==='adopt_baseline')return baselineConfirmations(params.args.side).every(message=>ask(label+'\n'+message));
  if(method==='connect'){
    if(!model)throw new Error('Trusted connection effects are unavailable. No connection was made.');
    const effects={voltage:'Opening will zero all eight channels. Closing will immediately zero them again.',gain:'Opening may trigger interlock shutdown. Closing disables current before TEC.',aq6370:'Opening reads identity and preserves front-panel measurement settings. Closing may abort this session acquisition.',pm400:'Opening reads instrument and sensor identity. Normal close preserves settings.',mdt693b:'Opening reads identity and voltages. Outputs are held and motion remains disarmed.','fiber-coupling':'Opening reads the selected controllers. Piezo outputs are held; no baseline or nominal conversion is authorized.'};
    const profile=model.profiles?.find(p=>p.id===record.profile_id),address=record.params?.port||record.params?.resource||record.members?.join(', ')||'Selected setup members';
    const serial=profile?.open_effects?.includes('DTR_RTS_reset_not_verified')?' Serial opening may toggle DTR/RTS or reset the controller; electrical behavior is unverified.':'';
    return ask(`${label}\nAddress: ${address}\n${effects[model.id]||model.connect_effects?.join(', ')}${serial}\nAuthorize these connection effects?`);
  }
  return ask(`${label}\n${method}: ${JSON.stringify(params)}\nVerify the instrument and all output effects.`);
}
export function inputSnapshot(elements){return new Map([...elements].map(element=>{
  const id=element.id||element.dataset?.draft||'param:'+element.dataset?.param;
  return [id,element.tagName==='DETAILS'?{open:element.open}:{value:element.value,...(element.type==='checkbox'?{checked:element.checked}:{})}];
}));}
export function inputValues(previousKey,currentKey,live,stored=new Map()){return new Map(previousKey===currentKey?live:stored);}
export function replaceMarkup(target,previous,next){if(previous===next)return false;target.innerHTML=next;return true;}
const focusControls='input,select,[role="tab"],button[data-ui],button[data-op],details[id]>summary';
export function focusIdentity(element){
  if(element?.matches('summary')){const id=element.closest('details[id]')?.id;return id?JSON.stringify(['summary',id]):null;}
  if(element?.dataset?.ui||element?.dataset?.op)return JSON.stringify(['action',Object.entries(element.dataset).sort(([a],[b])=>a.localeCompare(b))]);
  const id=element?.id||element?.dataset?.draft||element?.dataset?.param;return id?JSON.stringify(['field',id]):null;
}
export function snapshotBarrier(){let latest=null,minimum=null,resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no});
  const check=()=>{if(minimum!==null&&latest?.seq>=minimum)resolve(latest);};
  return {promise,reject,receive(event){latest=event;check();},expect(seq){minimum=seq;check();}};}
export function currentResult(intent,reply,store,key){return Boolean(store.lease(key)?.control_epoch===intent.control_epoch&&JSON.stringify(store.get(key)?.context)===JSON.stringify(reply.context));}
export function updatedHostSettings(previous,hostName,pythonPath,dataRoot){return {...previous,host_name:hostName,python_path:pythonPath,...(dataRoot?{data_root:dataRoot}:{})};}
export function observeDisconnects(locals,store){for(const [key,local]of Object.entries(locals)){
  const device=store.get(key),owner=store.host(device?.hostId);
  if(!local.disconnectInFlight&&local.disconnectAccepted&&owner?.bootId===local.disconnectEvidenceBoot&&owner?.seq>local.disconnectEvidenceSeq&&connectionReleased(owner,device)){
    local.disconnecting=null;local.disconnectFailed=false;local.unknown=false;local.disconnectAccepted=false;delete local.operationAttempt;
    if(local.activity?.kind==='disconnect')local.activity=finishActivity(local.activity);
  }
}}
export function renderConsole(page,host,store,local={},catalog={models:[]},preferences=null){
  if(page==='settings'||page==='host')return renderSettings(host,preferences,local.remoteSettings,local.connections);
  if(page==='devices')return renderDeviceSetup(host||{registry:{devices:[],drafts:[],setups:[]}},catalog);
  const route=parseRoute(page.startsWith('#')?page:'#'+page);
  if(route){if(route.hostId!==(host?.host_id||host?.hostId))host=store.host(route.hostId);
    const record=route.domain.kind==='setup'?host?.registry?.setups.find(r=>r.setup_id===route.domain.id):host?.registry?.devices.find(r=>r.device_id===route.domain.id);
    if(!record)return '<h1>Instrument unavailable</h1><p>Its Host or configuration is not available.</p>';
    const key=deviceKey(route.hostId,route.domain),value=store.get(key),kind=route.domain.kind==='setup'?'fiber':driver[record.model_id];
    const own=store.canControl(key),age=store.ageUpperMs(route.hostId,value?.host_sample_ms),view=instanceView(kind,value,own,age,{...local[key],mode:host.mode});
    view.hideConnectionAction=true;
    const iconModel=route.domain.kind==='setup'?'fiber-coupling':record.model_id,connection=connectionView(host,store,key,local[key]);
    const header=`<div class="instance-bar"><div class="instrument-identity">${renderInstrumentIcon(iconModel)}<strong>${esc(record.name)}</strong></div><span class="badge">${host.remote?'REMOTE':'LOCAL'}</span><div class="instance-connection"><span class="badge">${esc(connection.status)}</span><button class="btn ${connection.operation==='connect'?'primary':'warn'}" data-op="${connection.operation}" data-role="${kind}"${connection.disabled?' disabled':''}>${esc(connection.label)}</button></div></div>`;
    const recovery=view.unknown&&view.operationAttempt?`<div class="alert">Operation result is uncertain. Check its status or disconnect.<button class="btn" data-ui="query-original" ${view.pending||view.recovering?'disabled':''}>${view.recovering?'Checking…':'Check status'}</button></div>`:'';
    const history=store.history(key).filter(e=>e.type==='operation').slice(-16).map(e=>({operation_id:e.data?.operation_id,phase:e.data?.phase,context:e.data?.result?.context}));
    const feedback=[view.exportActivity,view.recoveryActivity,view.historyActivity,view.resultActivity,view.activity].filter(Boolean);
    const activity=feedback.find(activityBusy)||feedback.sort((a,b)=>(b.ended??b.started)-(a.ended??a.started))[0];
    const evidence='<details id="instrument-diagnostics"><summary>Diagnostics</summary><pre>'+esc(JSON.stringify({identity:record.expected_identity,context:value?.context,control:host.control?.[route.domain.kind+':'+route.domain.id],safety:value?.safety,operation:view.operationAttempt,outcome:view.lastOperation,latest_result:view.resultOperationId,result_error:view.sharedResultError,timings:{operation:view.activity?.timings,display:view.resultActivity?.timings,history:view.historyActivity?.timings,export:view.exportActivity?.timings},history},null,2))+'</pre></details>';
    const resultNote=view.sharedResultError?'<p class="alert">New spectrum unavailable. Any previous capture is labeled separately. See Diagnostics for details.</p>':'';
    if(kind==='mdt')return header+recovery+renderActivity(activity)+'<h1>MDT693B controller</h1><p>Configure a Fiber setup to use laboratory coordinates.</p><a class="btn" href="#devices">Device setup</a>'+evidence;
    return header+recovery+renderActivity(activity)+resultNote+(panels[kind]?.(view)||'<p>Driver required</p>')+evidence;
  }
  return renderOverview(host,store);
}
export function mountConsole(session,native){
  const localClient=session.client,store=session.store,content=document.querySelector('#content'),locals={},catalog={models:[],categories:[]};
  let wizard=null,connected=false,busyHost=false,renderedKey=null,lastMarkup=null,unlisten=null,preferences=null,appProfile={network_only:false},remoteSettings={peers:[],owner:null},hostActivity=null;
  const connections=connectionState();
  const resyncing=new Map(),snapshotWaiters=new Map(),identities=new Map(),remoteUnlisteners=new Map();
  const page=()=>location.hash||'#overview',activeHostId=()=>parseRoute(page())?.hostId||session.hostId;
  const host=()=>store.host(activeHostId()),localHost=()=>store.host(session.hostId);
  const route=()=>parseRoute(page()),key=()=>route()?deviceKey(route().hostId,route().domain):null;
  const ownerClient=id=>{const selected=session.clientFor(id);if(!selected)throw new Error('Connect the owning Host in Settings');return selected;};
  const client=createRoutedClient(ownerClient,activeHostId);
  const shared=createSharedResults(store,client,k=>{const l=locals[k],result=shared.current(k,{copyTrace:false});if(l&&l.displayResultId!==result.resultOperationId){l.cursor=null;l.displayResultId=result.resultOperationId;}render();});
  const archiveHistory=createArchiveHistory(client,()=>render());
  const archiveScope=()=>{const r=route(),h=host();return h?.connected&&h.synced&&r?.domain.kind==='device'
    &&h.registry.devices.some(d=>d.device_id===r.domain.id&&d.model_id==='aq6370')?{hostId:r.hostId,domain:r.domain}:null;};
  const displayedTrace=()=>{const state=archiveHistory.state(archiveScope(),{copyTrace:false}),live=shared.current(key(),{copyTrace:false});return state.historical?state.trace:live.trace||live.previousTrace;};
  const local=()=>{const k=key();if(!k)throw new Error('Select an instrument');return locals[k]||(locals[k]={});};
  function notify(message){const box=document.querySelector('#notice');box.textContent=message;box.hidden=false;}
  function error(cause){notify(cause?.message||String(cause));}
  function restoreInputs(saved){for(const element of content.querySelectorAll('input,select,details[id]')){const id=element.id||element.dataset.draft||'param:'+element.dataset.param;if(saved.has(id)){const item=saved.get(id);if(item&&typeof item==='object'){if('value' in item)element.value=item.value;if('checked' in item)element.checked=item.checked;if('open' in item)element.open=item.open;}else element.value=item;}}}
  function render(){const h=host(),p=page(),nextKey=key()||p;const live=inputSnapshot(content.querySelectorAll('input,select,details[id]'));
    observeDisconnects(locals,store);
    shared.refresh(key());archiveHistory.select(archiveScope());const viewLocals={...locals};if(key())viewLocals[key()]={...locals[key()],...shared.current(key(),{copyTrace:false}),...archiveHistory.state(archiveScope(),{copyTrace:false}),recordingRoot:h?.archive?.active_root,archiveAvailable:h?.archive?.available};
    viewLocals.remoteSettings=remoteSettings;
    viewLocals.connections=connections;
    const samePage=renderedKey===nextKey,saved=inputValues(renderedKey,nextKey,live,locals[nextKey]?.inputs||new Map());
    const active=focusIdentity(document.activeElement);const selection=[document.activeElement?.selectionStart,document.activeElement?.selectionEnd];const editing=Boolean(document.activeElement?.closest('#content')&&document.activeElement?.matches(focusControls));
    const navigation=document.querySelector('#navigation'),links='<a href="#overview">Overview</a><a href="#devices">Device setup</a><a href="#settings">Settings</a>';
    replaceMarkup(navigation,navigation.innerHTML,links);
    const background=document.querySelector('#background-work');let backgroundMarkup=activityBusy(hostActivity)?renderActivity(hostActivity):'';
    for(const [k,l]of Object.entries(locals)){const work=[l.exportActivity,l.recoveryActivity,l.activity].find(activityBusy);if(k===key()||!work)continue;
      const [hostId,kind,id]=k.split('/');if(!id)continue;const owner=store.host(hostId),record=kind==='setup'?owner?.registry.setups.find(s=>s.setup_id===id):owner?.registry.devices.find(d=>d.device_id===id);
      backgroundMarkup+=`<div class="background-task"><a href="${routeFor(hostId,{kind,id})}">${esc(record?.name||'Instrument')}</a>${renderActivity(work)}</div>`;
    }
    if(background){replaceMarkup(background,background.innerHTML,backgroundMarkup);background.hidden=!backgroundMarkup;}
    document.querySelector('#sidebar-mode').textContent=appProfile.network_only?'Network-only profile':'Local Host';document.querySelector('#sidebar-subtitle').textContent=busyHost?'Connecting…':connected?'Connected':appProfile.network_only?'No local hardware worker':'Disconnected';
    document.querySelector('#session-pill').textContent=(busyHost?'CONNECTING':store.hosts().some(h=>h.connected)?'ONLINE':'OFFLINE');
    document.querySelector('#current-page-title').textContent=p==='#devices'?'Device setup':['#host','#settings'].includes(p)?'Settings':route()?'Instrument':'Overview';
    let markup=renderConsole(p==='#devices'?'devices':['#host','#settings'].includes(p)?'settings':p,h,store,viewLocals,catalog,preferences);
    if(wizard)markup+=renderAddWizard(wizard,catalog);
    const scroll=[content.scrollTop,content.scrollLeft,window.scrollX,window.scrollY];
    const replaced=replaceMarkup(content,samePage?lastMarkup:null,markup);lastMarkup=markup;
    if(replaced){restoreInputs(saved);if(editing&&samePage&&active){const element=[...content.querySelectorAll(focusControls)].find(e=>!e.disabled&&focusIdentity(e)===active);element?.focus({preventScroll:true});if(selection[0]!==null&&selection[0]!==undefined&&element?.setSelectionRange)element.setSelectionRange(...selection);}if(samePage){content.scrollTop=scroll[0];content.scrollLeft=scroll[1];if(Number.isFinite(scroll[2])&&Number.isFinite(scroll[3]))window.scrollTo?.(scroll[2],scroll[3]);}}renderedKey=nextKey;
    if(route()&&h){const domain=store.get(key()),r=route().domain,k=r.kind==='setup'?'fiber':driver[h?.registry.devices.find(d=>d.device_id===r.id)?.model_id];
      const state=instanceView(k,domain,store.canControl(key()),store.ageUpperMs(activeHostId(),domain?.host_sample_ms),{...local(),...shared.current(key(),{copyTrace:false}),mode:h.mode});
      if(k==='gain')local().gainHistory=appendTelemetry(local().gainHistory||[],state.status.devices[k],k);
      if(k==='voltage')local().voltageHistory=appendTelemetry(local().voltageHistory||[],state.status.devices[k],k);
    }
  }
  async function calibrate(id=activeHostId()){const boot=store.host(id)?.bootId,start=performance.now(),ping=await ownerClient(id).ping(),end=performance.now();
    if(boot!==store.host(id)?.bootId)return;
    store.setClock(id,ping.monotonic_ms,start,end);
    const identity=ping.boot_id===boot&&/^[0-9a-f]{32}$/.test(ping.client_session_id)?{hostId:id,bootId:boot,sessionId:ping.client_session_id}:null;
    identities.set(id,identity);if(id===session.hostId)session.clientIdentity=identity;
  }
  async function syncHost(id=activeHostId()){if(resyncing.has(id))return resyncing.get(id);const work=(async()=>{
    const waiter=snapshotBarrier();snapshotWaiters.set(id,waiter);
    const timer=setTimeout(()=>waiter.reject(new Error('Snapshot unavailable. Controls remain disarmed.')),10000);
    try {const receipt=await ownerClient(id).requestSnapshot();waiter.expect(receipt.seq);const current=store.host(id||session.hostId);if(current?.synced&&current.seq>=receipt.seq)waiter.receive({seq:current.seq});await waiter.promise;await calibrate(id||session.hostId);} finally{clearTimeout(timer);snapshotWaiters.delete(id);}
  })().finally(()=>{resyncing.delete(id);});resyncing.set(id,work);return work;}
  const resync=()=>syncHost(activeHostId());
  async function connect(automatic=false){if(busyHost||connected)return;busyHost=true;hostActivity=startActivity('host','authority');render();try{if(automatic){if(!preferences)throw new Error('Review and save local Host settings before starting.');await connectLocalHost(localClient,preferences);}else await localClient.connect();Object.assign(catalog,await localClient.catalog());
      unlisten=await localClient.subscribe(event=>{const applied=session.apply(event);if(event.type==='snapshot')(snapshotWaiters.get(event.host_id)||snapshotWaiters.get(null))?.receive(event);if(applied.requiresSnapshot)syncHost(event.host_id).catch(error);});await syncHost(session.hostId);
      connected=true;
    }catch(cause){unlisten?.();unlisten=null;try{await localClient.disconnect();}catch(cleanup){error(cleanup);}connected=false;session.offline();throw cause;
    }finally{hostActivity=finishActivity(hostActivity,connected?'complete':'failed');busyHost=false;render();}}
  async function stopDomain(domain){const id=activeHostId(),client=ownerClient(id),host=()=>store.host(id),resync=()=>syncHost(id),k=deviceKey(id,domain),l=locals[k]||(locals[k]={});
    if(l.disconnectInFlight)throw new Error('Disconnect is already in progress.');
    const lease=store.lease(k);if(lease){l.disconnectOwner=lease.session_id;l.disconnectEpoch=lease.control_epoch;l.disconnectBoot=lease.boot_id;}
    l.disconnectEvidenceBoot=host()?.bootId;l.disconnectEvidenceSeq=host()?.seq;l.disconnectAccepted=false;l.disconnectInFlight=true;
    l.connectionGeneration=(l.connectionGeneration||0)+1;l.disconnecting=performance.now();l.activity=startActivity('disconnect','disconnect');l.unknown=true;store.dropLease(k);render();
    try{const report=await client.safeStop(domain);
      if(report?.accepted!==true)throw new Error('Disconnect acceptance is unconfirmed.');
      // A coalesced snapshot requested before stop acceptance cannot prove release.
      if(resyncing.has(id))await resyncing.get(id);
      if(host()?.bootId!==l.disconnectEvidenceBoot)throw new Error('Host changed during disconnect. Release is unconfirmed.');
      l.disconnectEvidenceSeq=host()?.seq;l.disconnectAccepted=true;await resync();return report;}
    catch(cause){l.disconnecting=null;l.disconnectFailed=true;l.activity=finishActivity(l.activity,'unknown');throw cause;}
    finally{l.disconnectInFlight=false;render();}}
  async function run(domain,rev,method,params){
    const id=activeHostId(),client=ownerClient(id),host=()=>store.host(id),resync=()=>syncHost(id),k=deviceKey(id,domain),l=locals[k]||(locals[k]={});
    if(l.pending||l.connecting)throw new Error('An operation is already in progress.');
    if(method==='connect'){const view=connectionView(host(),store,k,l);if(view.operation!=='connect'||view.disabled)throw new Error('Connection is unavailable. Check status or disconnect first.');}
    if(l.unknown&&method!=='connect')throw new Error('Check the operation status or disconnect before continuing.');
    const registry=host().registry,record=domain.kind==='setup'?registry.setups.find(s=>s.setup_id===domain.id):[...registry.devices,...registry.drafts].find(d=>d.device_id===domain.id);
    const ownCatalog=id===session.hostId?catalog:session.catalogFor(id);
    const model=domain.kind==='setup'?{id:'fiber-coupling'}:ownCatalog?.models.find(m=>m.id===record?.model_id);
    if(!confirmInstrumentAction({method,params,record,model,mode:host().mode},confirm))return;
    let acquired=false,activityId=null;
    const phase=name=>{if(l.activity?.id===activityId){l.activity=advanceActivity(l.activity,name);render();}};
    if(method==='connect'){
      const generation=l.connectionGeneration||0;l.connecting=true;l.activity=startActivity('connect','authority');activityId=l.activity.id;render();
      const identity=identities.get(id);
      if(identity?.hostId===id&&identity.bootId===host()?.bootId){l.disconnectOwner=identity.sessionId;l.disconnectBoot=identity.bootId;l.disconnectEpoch=host()?.control?.[domain.kind+':'+domain.id]?.control_epoch;}
      try{({acquired}=await ensureInstrumentControl({client,store,hostId:id},domain,resync,()=>generation===(l.connectionGeneration||0)));l.unknown=false;}
      catch(cause){if(cause.outcomeUnknown||cause.cleanupError)l.unknown=true;if(l.activity?.id===activityId)l.activity=finishActivity(l.activity,l.unknown?'unknown':'failed');throw cause;}
      finally{l.connecting=false;render();}
    }
    const snapshot=store.get(k),lease=store.lease(k);
    if(!store.canControl(k)||!lease||!snapshot?.context){if(l.activity?.id===activityId)l.activity=finishActivity(l.activity,'failed');render();throw new Error('Instrument is in use or connection authority is unavailable.');}
    const intent={domain,lease_token:lease.token,control_epoch:lease.control_epoch,config_rev:rev,context:snapshot.context,method,params,sequence:client.nextSequence(),confirmation:null};
    const requestId=crypto.randomUUID().replaceAll('-','');l.pending=requestId;
    l.activity=method==='connect'?advanceActivity(l.activity,'prepare'):startActivity(params.name||method,'prepare');activityId=l.activity.id;if(!activityBusy(l.exportActivity))delete l.exportActivity;render();
    l.operationAttempt={request_id:requestId,domain,boot_id:lease.boot_id,context:intent.context};
    let outcome='failed';try{const proof=await client.prepare(intent);intent.confirmation=proof.token;
      phase('instrument');
      let record=await client.execute(requestId,intent);record=await pollOriginalOperation(client,requestId,record);
      l.lastOperation=record;
      if(record.status==='Outcome Unknown'||record.phase==='timed_out_unknown'){l.unknown=true;outcome='unknown';return record;}
      if(record.phase!=='completed')throw new Error(record.result?.error?.message||record.result?.error||record.phase);
      phase('sync');await resync();outcome='complete';
      if(!currentResult(intent,record.result,store,k))return record;
      if(key()===k)await shared.refresh(k);return record;
    }catch(cause){if(cause.outcomeUnknown||cause.code==='AdmissionPending'){l.unknown=true;l.unknownRequestId=requestId;}
      else if(acquired&&!store.get(k)?.context?.connection_id){try{await client.release(domain,lease);store.dropLease(k);await resync();}catch(cleanup){l.unknown=true;cause.cleanupError=cleanup;}}
      throw cause;
    }finally{if(l.activity?.id===activityId)l.activity=finishActivity(l.activity,l.unknown?'unknown':outcome);l.pending=null;render();}
  }
  const setup=createSetupActions(session,confirm,resync,run);
  let refreshingRemote=false;
  async function refreshRemote(){if(refreshingRemote||typeof native.core?.invoke!=='function')return;refreshingRemote=true;
    try{const peers=await native.core.invoke('remote_peers');remoteSettings.peers=peers.map(p=>({...p,connected:store.host(p.host_id)?.connected===true}));
      remoteSettings.owner=connected?await localClient.remoteStatus():null;render();
    }finally{refreshingRemote=false;}}
  async function pollPairRequest(){const original=connections.request?.request_id;if(!original||connections.request.phase!=='Waiting'||connections.polling||connections.busy)return;connections.polling=true;
    try{const result=await native.core.invoke('remote_pair_status',{requestId:original});if(result.request_id!==original)throw new Error('Pairing request status mismatch.');connections.request=result;render();
      if(result.phase==='Completed'&&connections.connectedRequest!==original){connections.connectedRequest=original;connections.add=false;
        try{await connectPeer(result.peer.host_id);notify('Computer paired and connected.');}catch(cause){notify('Computer paired. Offline — use Connect to retry. '+(cause?.message||String(cause)));}await refreshRemote();}
    }finally{connections.polling=false;render();}}
  async function cancelPairRequest(){if(connections.busy||connections.request?.phase!=='Waiting')return;connections.busy=true;const id=connections.request.request_id;
    try{connections.request=await native.core.invoke('remote_pair_cancel',{requestId:id});if(connections.request.phase==='Completed')connections.connectedRequest=id;await refreshRemote();}finally{connections.busy=false;render();}}
  async function connectPeer(id){if(store.host(id)?.connected)return;
    const previous=session.clientFor(id);if(previous){await previous.disconnect();remoteUnlisteners.get(id)?.();remoteUnlisteners.delete(id);session.removeRemote(id);}
    const remote=createRemoteClient(native.core.invoke,native.event.listen,id);
    await remote.connect();session.addRemote(id,remote);
    try{session.setCatalog(id,await remote.catalog());const stop=await remote.subscribe(event=>{const applied=session.apply(event,true);if(event.type==='snapshot')snapshotWaiters.get(id)?.receive(event);if(applied.requiresSnapshot)syncHost(id).catch(error);});remoteUnlisteners.set(id,stop);await syncHost(id);}
    catch(cause){remoteUnlisteners.get(id)?.();remoteUnlisteners.delete(id);try{await remote.disconnect();session.removeRemote(id);}catch(cleanup){error(cleanup);}session.offline(id);throw cause;}
    await refreshRemote();
  }
  function currentRecord(){const r=route();return r.domain.kind==='setup'?host().registry.setups.find(s=>s.setup_id===r.domain.id):host().registry.devices.find(d=>d.device_id===r.domain.id);}
  const get=id=>document.getElementById(id)?.value??'';
  async function uiAction(button){const name=button.dataset.ui;
    if(name==='connections-tab'){connections.tab=button.dataset.tab==='other'?'other':'this';render();return;}
    if(name==='remote-add'){if(connections.request?.phase==='Waiting'||connections.busy)return;connections.add=true;connections.request=null;render();return;}
    if(name==='remote-add-cancel'){connections.add=false;render();return;}
    if(name==='remote-refresh')return refreshRemote();
    if(name==='remote-connect')return connectPeer(button.dataset.host);
    if(name==='remote-disconnect'||name==='remote-forget'){
      const id=button.dataset.host;if(!confirm('Disconnect this remote App session? Its owned devices will use their normal safe cleanup. Other observers keep watching.'))return;
      if(session.clientFor(id)){await ownerClient(id).disconnect();remoteUnlisteners.get(id)?.();remoteUnlisteners.delete(id);session.removeRemote(id);session.offline(id);}
      if(name==='remote-forget')await native.core.invoke('remote_forget',{hostId:id});return refreshRemote();
    }
    if(name==='remote-request'){
      if(connections.busy||connections.request?.phase==='Waiting')return;const endpoint=connectionEndpoint(connections.draft.endpoint),nickname=connections.draft.name.trim()||null;connections.busy=true;render();
      try{connections.request=await native.core.invoke('remote_pair_request',{endpoint,nickname});}
      finally{connections.busy=false;render();}return;
    }
    if(name==='remote-cancel')return cancelPairRequest();
    if(name==='remote-enable'){if(appProfile.network_only)throw new Error('Local access is unavailable in a network-only profile.');await localClient.remoteListener(connectionEndpoint(get('remote-listener')));return refreshRemote();}
    if(name==='remote-disable'||name==='remote-revoke'){
      if(!confirm('Disconnect affected remote controllers? Their device cleanup will run: Voltage zero, Gain current off before TEC, piezo hold.'))return;
      if(name==='remote-disable')await localClient.remoteListener(null);else await localClient.revokePeer(button.dataset.peer);return refreshRemote();
    }
    if(name==='remote-approve'||name==='remote-reject'){if(connections.busy)return;connections.busy=true;render();try{if(name==='remote-approve')await localClient.approvePeer(button.dataset.peer);else await localClient.rejectPeer(button.dataset.peer);await refreshRemote();}finally{connections.busy=false;render();}return;}
    if(name==='osa-history-refresh'||name==='osa-history-more')return archiveHistory.list(name==='osa-history-more');
    if(name==='osa-history-load'){local().cursor=null;delete local().exportDirectory;return archiveHistory.load(button.dataset.archive,button.dataset.name);}
    if(name==='osa-current'){local().cursor=null;archiveHistory.showCurrent();return;}
    if(busyHost)throw new Error('Local Host connection is in progress.');
    if(name==='connect-host')return connect(true);
    if(name==='disconnect-host'){await client.disconnect();unlisten?.();unlisten=null;connected=false;session.offline();return;}
    if(name==='start-host'){await client.startHost({pythonPath:get('host-python')});return connect();}
    if(name==='stop-host'){const affected=host().registry.devices.map(d=>d.name).join(', ');if(!confirm(`Stop this Host and all its devices (${affected||'none'})? All controllers lose authority. Voltage: zero; Gain: current off before TEC off; piezo: hold. Other GUIs will disconnect.`))return;const report=await client.stopHost();notify(JSON.stringify(report));if(!report.resource_released||report.process_exit?.confirmed!==true||report.process_exit?.success!==true)throw new Error('Host stop is retained. Keep this management window open.');connected=false;await client.disconnect();session.offline();return;}
    if(name==='save-host'){
      const chosen={pythonPath:get('host-python')};
      await client.savePreferences(chosen);preferences=chosen;
      if(connected){await client.saveSettings({settings:updatedHostSettings(host().registry.settings,get('host-name'),chosen.pythonPath,get('host-data-root')),expected_rev:host().registry.registry_rev});await resync();}
      notify('Settings saved locally. Startup changes apply after a safe Host restart.');return;
    }
    if(name==='choose-data-root'){
      if(!connected)throw new Error('Local Host is unavailable.');
      const selected=await client.chooseDataRoot();if(selected===null)return;
      const target=document.getElementById('host-data-root');
      const l=locals['#settings']||={};l.inputs??=new Map();l.inputs.set('host-data-root',selected);
      if(target)target.value=selected;
      return;
    }
    if(name==='save-checks'){
      const d=host().registry.devices.find(d=>d.device_id===button.dataset.device),id=d.device_id;
      const enumeration=document.getElementById('check-enumeration-'+id).checked,readonly=document.getElementById('check-readonly-'+id).checked;
      if(enumeration&&!confirm('Authorize Host startup and periodic resource enumeration for this configuration and mode? This does not prove the instrument is online.'))return;
      if(readonly&&!confirm('Separately authorize Host startup and periodic read-only identity probes? Temporary sessions will close, never grant output control, and run only while unowned.'))return;
      await client.saveCheckPolicy({device_id:id,config_rev:d.config_rev,expected_rev:host().registry.registry_rev,interval_s:Number(get('check-interval-'+id)),enumeration,readonly});return resync();
    }
    if(name==='add-new'){wizard={category:catalog.categories[0],params:{},name:''};render();return;}
    if(name==='open-draft'){const d=host().registry.drafts.find(d=>d.device_id===button.dataset.device);wizard={category:catalog.models.find(m=>m.id===d.model_id)?.category||catalog.categories[0],modelId:d.model_id,profileId:d.profile_id,name:d.name,params:{...d.params},record:d};wizard.recordSignature=signature(wizard);render();return;}
    if(name==='safe-stop-draft')return stopDomain({kind:'device',id:wizard.record.device_id});
    if(name==='cancel-wizard'||name==='cancel-draft'){const d=name==='cancel-wizard'?wizard:{record:host().registry.drafts.find(d=>d.device_id===button.dataset.device)};if(d.record&&!confirm('Cancel this saved draft? An owned session must first be safely released.'))return;await setup.cancel(d);wizard=null;render();return;}
    if(name==='test-draft'||name==='prepare-draft'||name==='save-draft'){if(!wizard)throw new Error('Open a draft first');const model=catalog.models.find(m=>m.id===wizard.modelId),profile=model?.profiles.find(p=>p.id===wizard.profileId);wizard.busy=true;render();try{
      if(name==='test-draft')await setup.test(wizard,model,profile);else if(name==='prepare-draft')await setup.prepare(wizard,model);else{await setup.save(wizard);wizard=null;}
    }finally{if(wizard)wizard.busy=false;render();}return;}
    if(name==='rename'||name==='retire'){const d=host().registry.devices.find(d=>d.device_id===button.dataset.device),params={device_id:d.device_id,config_rev:d.config_rev,expected_rev:host().registry.registry_rev};
      if(name==='rename'){const value=prompt('Instrument name',d.name);if(!value)return;params.name=value;await client.renameDevice(params);}else{if(!confirm(`Remove ${d.name}? Safe stop and confirmed release are required first.`))return;await client.retireDevice(params);}return resync();}
    if(name==='add-setup'){const choices=host().registry.devices.filter(d=>d.model_id==='mdt693b'&&['2110148249-10','160721175410'].includes(d.expected_identity.serial)&&!host().registry.setups.some(s=>s.members.includes(d.device_id)));
      if(!choices.length)throw new Error('Register a controller with one of the two known serial identities first.');const members=choices.filter(d=>confirm(`Include ${d.name} (${d.expected_identity.serial}) in this Fiber setup?`)).map(d=>d.device_id);if(!members.length)return;const name=prompt('Setup name','Fiber coupling');if(name)await client.saveSetup({name,members,expected_rev:host().registry.registry_rev});return resync();}
    if(name==='retire-setup'){const s=host().registry.setups.find(s=>s.setup_id===button.dataset.setup);if(!confirm(`Remove ${s.name}? Safe stop and confirmed hold/release are required first. No piezo zero or rollback is performed.`))return;const report=await client.retireSetup({setup_id:s.setup_id,config_rev:s.config_rev,expected_rev:host().registry.registry_rev});if(report.restart_required)notify('Setup removed. Safely restart Host before reassigning its controllers. Piezo outputs were held, not zeroed.');return resync();}
    if(name==='safe-stop')return stopDomain(route().domain);
    if(name==='query-original'){
      const k=key(),l=local(),id=activeHostId(),attempt=l.operationAttempt,generation=l.connectionGeneration||0,activityId=l.activity?.id;
      if(l.recovering)return;l.recovering=true;l.recoveryActivity=startActivity('status','sync');render();let outcome='failed';
      const current=()=>generation===(l.connectionGeneration||0)&&attempt?.request_id===l.operationAttempt?.request_id&&activityId===l.activity?.id;
      try{const result=await queryOriginalOperation(ownerClient(id),attempt,()=>syncHost(id),()=>{const h=store.host(id);return {boot_id:h?.bootId,context:store.get(k)?.context,synced:h?.connected&&h?.synced};});
        if(!current()){l.recoveryActivity=null;return;}
        l.lastOperation=result.record;l.unknown=!result.resolved;outcome=result.resolved?'complete':'unknown';
        if(result.resolved&&l.activity?.outcome==='unknown')l.activity={...l.activity,outcome:result.record.phase==='completed'?'complete':'failed'};
        if(!result.resolved)notify('Operation status remains uncertain. Disconnect or check again.');
        if(result.resolved&&key()===k)await shared.refresh(k);
      }finally{if(!current())l.recoveryActivity=null;else l.recoveryActivity=finishActivity(l.recoveryActivity,outcome);l.recovering=false;render();}return;
    }
  }
  content.addEventListener('click',event=>{const button=event.target.closest('button');if(!button||button.disabled)return;
    (async()=>{if(button.dataset.ui)return uiAction(button);
      if(button.dataset.pmTab){local().pmTab=button.dataset.pmTab;render();return;}
      const op=button.dataset.op;if(!op)return;if(op==='disconnect'){
        if(connectionView(host(),store,key(),local()).disabled)throw new Error('This instrument is in use by another window.');
        return stopDomain(route().domain);}
      const record=currentRecord();if(op==='connect')return run(route().domain,record.config_rev,'connect',{acknowledge_lifecycle:true});
      if(op==='resume')return run(route().domain,record.config_rev,'resume',{confirm:true});
      if(op==='osa-export'){const trace=displayedTrace(),scope=archiveScope(),l=local();if(!trace||l.exporting)return;if(!scope)throw new Error('Reconnect the owning Host to export this capture.');l.exporting=true;l.exportActivity=startActivity('export','export');render();let outcome='failed';
        try{const receipt=await exportSelectedTrace(client,trace,scope);outcome=receipt?'complete':'cancelled';if(receipt){l.exportDirectory=receipt.directory;notify('Capture exported: '+receipt.directory);}}
        finally{l.exportActivity=finishActivity(l.exportActivity,outcome);l.exporting=false;render();}return;}
      let action;
      if(op==='pm-measure')action={name:'measure_kind',args:{kind:get('pm-kind')}};
      else if(op==='pm-read'||op==='pm-write'){const setting=store.get(key()).device.catalog.settings.find(s=>s.key===button.dataset.setting);
        const payload=buildPmSettingAction(setting,op==='pm-read'?'read':'write',{raw:get(`pm-value-${setting.key}`),group:get(`pm-group-${setting.key}`),selector:get(`pm-selector-${setting.key}`),confirmed:!setting.sensitive||confirm(`Change sensitive setting ${setting.key}?`)});
        delete payload.role;const name=payload.name==='read'?'read_setting':'write_setting';delete payload.name;action={name,args:payload};}
      else if(op==='pm-command'){if(!confirm(`Run PM400 maintenance: ${button.dataset.command}? This may change settings or calibration.`))return;action={name:'run_maintenance',args:{command:button.dataset.command,confirm:true}};}
      else action=actionFor(op,get,button.dataset);
      if(op==='osa-read'||op==='osa-acquire'){local().cursor=null;delete local().exportDirectory;archiveHistory.showCurrent();}
      return run(route().domain,record.config_rev,'action',action);
    })().catch(error);
  });
  content.addEventListener('change',event=>{const element=event.target;if(!wizard)return;
    if(element.dataset.draft){const name=element.dataset.draft;wizard[name]=element.value;
      if(['category','modelId','profileId'].includes(name))renderedKey=null;
      if(name==='category'){wizard.modelId=null;wizard.profileId=null;wizard.params={};}
      if(name==='modelId'){const model=catalog.models.find(m=>m.id===wizard.modelId);wizard.profileId=model?.profiles[0]?.id;wizard.name=model?.name||'';wizard.params={};}
      if(name==='profileId')wizard.params={};
    }else if(element.dataset.param){const f=catalog.models.find(m=>m.id===wizard.modelId).profiles.find(p=>p.id===wizard.profileId).fields[element.dataset.param];wizard.params[element.dataset.param]=['number','integer'].includes(f.kind)?Number(element.value):element.value;}
    wizard.proof=null;render();
  });
  content.addEventListener('input',event=>{const field={'remote-endpoint':'endpoint','remote-name':'name','remote-listener':'listener'}[event.target.id];if(field){connections.draft[field]=event.target.value;return;}if(!key()||!event.target.id)return;const l=local();l.inputs??=new Map();l.inputs.set(event.target.id,event.target.value);});
  content.addEventListener('keydown',event=>{if(event.key==='Escape'){if(connections.request?.phase==='Waiting')cancelPairRequest().catch(error);else if(connections.add){connections.add=false;render();}return;}
    const tab=event.target.closest('[role="tab"]');if(!tab||!['ArrowLeft','ArrowRight','Home','End','Enter',' '].includes(event.key))return;event.preventDefault();connections.tab=event.key==='Home'?'this':event.key==='End'?'other':['Enter',' '].includes(event.key)?tab.dataset.tab:connections.tab==='this'?'other':'this';render();document.getElementById('connections-tab-'+connections.tab)?.focus();});
  content.addEventListener('click',event=>{const plot=event.target.closest('[data-osa-plot]');if(plot&&key()){const box=plot.getBoundingClientRect();local().cursor=osaCursorIndex(displayedTrace(),plotFraction((event.clientX-box.left)/box.width));render();}});
  window.addEventListener('hashchange',()=>{wizard=null;session.navigate(page());render();});
  native.event.listen('host-offline',()=>{connected=false;session.offline();render();});
  native.event.listen('remote-offline',event=>{session.offline(event.payload.host_id);refreshRemote().catch(error);render();});
  native.event.listen('host-close-retained',event=>{notify(event.payload.message+' '+JSON.stringify(event.payload.report||event.payload.error));});
  const heartbeat=setInterval(()=>session.heartbeat().catch(error),2000),clock=setInterval(()=>{for(const h of store.hosts())if(h.connected)calibrate(h.host_id).catch(()=>{});},30000),remoteTimer=setInterval(async()=>{try{await pollPairRequest();if(page()==='#settings')await refreshRemote();}catch(cause){error(cause);}},1500);
  const feedbackTimer=setInterval(()=>{tickActivities(content);const background=document.querySelector('#background-work');if(background)tickActivities(background);},1000);
  window.addEventListener('beforeunload',()=>{for(const l of Object.values(locals))l.connectionGeneration=(l.connectionGeneration||0)+1;clearInterval(heartbeat);clearInterval(clock);clearInterval(remoteTimer);clearInterval(feedbackTimer);for(const {client}of session.clients())client.disconnect();});
  render();const ready=(async()=>{if(typeof native.core?.invoke==='function')appProfile=await native.core.invoke('app_profile');remoteSettings.networkOnly=appProfile.network_only;preferences=await localClient.preferences();renderedKey=null;if(!appProfile.network_only)await connect(true);await refreshRemote();})().catch(cause=>{error(new Error('App startup unavailable: '+(cause?.message||String(cause))+'. Review Settings.'));render();});
  return {render,connect,resync,ready};
}
