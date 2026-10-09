import {formatTarget,editTarget,formatDigits,editDigits,syncTarget,laserMotion} from './wavelength-editor.js';
import {createNotice} from './notice.js';
import * as panels from './panels.js';
import {renderDeviceSetup,renderAddWizard,renderSettings,renderHostIssue,setupConnectionEffects,instrumentTarget,signature,setupChoices,driverCheckReady,controllerChoiceReady,driverState,driverLabel} from './setup.js';
import {connectionState,connectionEndpoint} from './connections.js';
import {createRemoteClient} from './remote-client.js';
import {connectLocalHost} from './host-client.js';
import {serialChoiceReady} from './serial-ports.js';
import {createSetupActions,refreshDraftConnection,installMissingDriver,installDriverPackage,draftFailureMessage} from './setup-actions.js';
import {connectionView,connectionReleased,ensureInstrumentControl} from './connection.js';
import {instanceView,actionFor} from './instance-view.js';
import {deviceKey,routeFor,parseRoute} from './routes.js';
import {renderInstrumentIcon} from './instrument-icon.js';
import {buildPmSettingAction} from './pm400.js';
import {appendTelemetry,osaCursorIndex} from './view-model.js';
import {renderOverview} from './overview.js';
import {createSharedResults} from './shared-results.js';
import {createArchiveHistory,exportSelectedTrace,plotFraction,validateReference} from './osa.js';
import {replaceMarkup,createRenderScheduler} from './render.js';
import {laserRefreshDue,operatingLimits,updateSingleScanButtons} from './laser-control.js';
import {createScanShortcuts,captureScanShortcut} from './scan-shortcuts.js';
import {gainDraftIds,syncGainDraft,gainPidReadDue,markGainPidRead,sendGainSafety,submitGainAction} from './gain-control.js';
import {appendGainHistory} from './gain-history.js';
import {sameJsonValue} from './json-value.js';
export {replaceMarkup,createRenderScheduler} from './render.js';
export {createSharedResults} from './shared-results.js';
import {pollOriginalOperation,queryOriginalOperation} from './operation-recovery.js';
export {pollOriginalOperation,queryOriginalOperation} from './operation-recovery.js';
import {startActivity,advanceActivity,finishActivity,activityBusy,renderActivity,tickActivities} from './activity.js';
export {startActivity,advanceActivity,finishActivity,renderActivity,tickActivities} from './activity.js';
const esc=panels.esc;
const driver={aq6370:'osa',voltage:'voltage',gain:'gain',pm400:'pm400',mdt693b:'mdt',tlb6700:'laser'};
export function createRoutedClient(ownerClient,activeHostId){return new Proxy(Object.create(null),{get(target,method){if(method==='forHost')return ownerClient;return (...args)=>ownerClient(activeHostId())[method](...args);}});}
export function connectionEffects(record,model){
 if(!model)throw new Error('Trusted connection effects are unavailable.');
 const effects={tlb6700:'',voltage:'Opening will zero all eight channels. Closing immediately zeros them again.',gain:'Opening turns current off before TEC off. Closing uses the same order.',aq6370:'',pm400:'',mdt693b:'Outputs are held and motion remains disarmed.','fiber-coupling':'Piezo outputs are held; baseline adoption is separate.'};
 const profile=model.profiles?.find(p=>p.id===record?.profile_id),serial=profile?.open_effects?.includes('DTR_RTS_reset_not_verified')?' Serial opening may toggle DTR/RTS or reset the controller.':'';
 const effect=effects[model.id]??model.connect_effects?.join(', ');
 return effect||serial?`${record?.params?.device_key||record?.params?.port||record?.params?.resource||record?.name||'Selected setup'}: ${effect||''}${serial}`:'';
}
export function baselineAuthorized(consent,binding){return Boolean(binding&&consent?.binding===binding&&consent.baseline===true&&consent.nominal===true);}
export function baselineBinding(host,domain,lease,stage){return host?.connected&&host.synced&&lease&&domain?.context?.connection_id&&stage?.available&&!stage.restricted&&!stage.fault&&!domain.active_request_id&&!domain.safety_request_id&&['READY','ACTIVE'].includes(domain.state)?JSON.stringify([host.bootId,domain.context,lease.control_epoch,stage.serial,stage.observed_voltage_v]):null;}
export function inputSnapshot(elements){return new Map([...elements].filter(element=>!element.dataset?.managed).map(element=>{
  const id=element.id||element.dataset?.draft||'param:'+element.dataset?.param;
  return [id,element.tagName==='DETAILS'?{open:element.open}:{value:element.value,...(element.type==='checkbox'?{checked:element.checked}:{})}];
}));}
export function inputValues(previousKey,currentKey,live,stored=new Map()){return new Map(previousKey===currentKey?live:stored);}
const focusControls='input,select,[role="tab"],button[data-ui],button[data-op],details[id]>summary';
export function focusIdentity(element){
  if(element?.matches('summary')){const id=element.closest('details[id]')?.id;return id?JSON.stringify(['summary',id]):null;}
  if(element?.dataset?.ui||element?.dataset?.op)return JSON.stringify(['action',Object.entries(element.dataset).sort(([a],[b])=>a.localeCompare(b))]);
  const id=element?.id||element?.dataset?.draft||element?.dataset?.param;return id?JSON.stringify(['field',id]):null;
}
export function snapshotBarrier(){let latest=null,minimum=null,resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no});
  const check=()=>{if(minimum!==null&&latest?.seq>=minimum)resolve(latest);};
  return {promise,reject,receive(event){latest=event;check();},expect(seq){minimum=seq;check();}};}
export function currentResult(intent,reply,store,key){return Boolean(store.lease(key)?.control_epoch===intent.control_epoch&&sameJsonValue(store.get(key)?.context,reply.context));}
export function updatedHostSettings(previous,hostName,dataRoot){return {host_name:hostName,data_root:dataRoot||previous?.data_root||null};}
export function observeDisconnects(locals,store){for(const [key,local]of Object.entries(locals)){
  const device=store.get(key),owner=store.host(device?.hostId);
  if(!local.disconnectInFlight&&local.disconnectAccepted&&owner?.bootId===local.disconnectEvidenceBoot&&owner?.seq>local.disconnectEvidenceSeq&&connectionReleased(owner,device)){
    local.disconnecting=null;local.disconnectFailed=false;local.unknown=false;local.disconnectAccepted=false;delete local.operationAttempt;
    if(local.activity?.kind==='disconnect')local.activity=finishActivity(local.activity);
  }
}}
export function renderConsole(page,host,store,local={},catalog={models:[]},preferences=null){
  if(page==='settings'||page==='host')return renderHostIssue(host?.hostIssue)+renderSettings(host,preferences,local.remoteSettings,local.connections);
  if(page==='devices')return renderHostIssue(host?.hostIssue)+renderDeviceSetup(host||{registry:{devices:[],drafts:[],setups:[]}},catalog,local['#devices']);
  const route=parseRoute(page.startsWith('#')?page:'#'+page);
  if(route){if(route.hostId!==(host?.host_id||host?.hostId))host=store.host(route.hostId);
    const record=route.domain.kind==='setup'?host?.registry?.setups.find(r=>r.setup_id===route.domain.id):host?.registry?.devices.find(r=>r.device_id===route.domain.id);
    if(!record)return '<h1>Instrument unavailable</h1><p>Its Host or configuration is not available.</p>';
    const key=deviceKey(route.hostId,route.domain),value=store.get(key),kind=route.domain.kind==='setup'?'fiber':driver[record.model_id];
    const own=store.canControl(key),age=kind==='laser'&&(!host.connected||!host.synced)?null:store.ageUpperMs(route.hostId,value?.host_sample_ms),view=instanceView(kind,value,own,age,{...local[key],mode:host.mode});
    view.hideConnectionAction=true;
    if(kind==='laser'){view.laserRefreshInterval=record.check_policy?.interval_s||30;view.remote=host.remote===true;}
    if(kind==='fiber'){view.baselineConsent={};for(const side of ['left','right']){const stage=value?.device?.[side],binding=baselineBinding(host,value,own?store.lease(key):null,stage),consent=local[key]?.baselineConsent?.[side];view.baselineConsent[side]=consent?.binding===binding?consent:{};}}

    const iconModel=route.domain.kind==='setup'?'fiber-coupling':record.model_id,connection=connectionView(host,store,key,local[key]);
    if(kind==='gain'&&connection.status==='Release unconfirmed'&&host.control?.['device:'+route.domain.id]?.state!=='RETAINED'&&local[key]?.unknown&&!local[key]?.disconnectFailed&&!local[key]?.disconnecting&&!local[key]?.disconnectInFlight){connection.status='Operation unconfirmed';connection.label='Disconnect';}
    const header=`<div class="instance-bar"><div class="instrument-identity">${renderInstrumentIcon(iconModel)}<strong>${esc(record.name)}</strong></div><span class="badge">${host.remote?'REMOTE':'LOCAL'}</span><div class="instance-connection"><span class="badge">${esc(connection.status)}</span><button class="btn ${connection.operation==='connect'?'primary':'warn'}" data-op="${connection.operation}" data-role="${kind}"${connection.disabled?' disabled':''}>${esc(connection.label)}</button></div></div>`;
    const model=route.domain.kind==='setup'?{id:'fiber-coupling'}:catalog.models?.find(m=>m.id===record.model_id);
    const effect=model?connectionEffects(record,model):'';
    const lifecycle=effect&&kind!=='gain'?`<p class="warning">${esc(effect)}</p>`:'';
    const recovery=(view.unknown&&view.operationAttempt&&!local[key]?.disconnecting&&!local[key]?.disconnectInFlight?`<div class="alert">Operation result is uncertain. Check its status or disconnect.<button class="btn" data-ui="query-original" ${view.pending||view.recovering?'disabled':''}>${view.recovering?'Checking…':'Check status'}</button></div>`:'')+
      (view.gainSafetyUnknown?`<div class="alert">Gain output shutdown is unconfirmed. Check its original operation status.<button class="btn" data-ui="query-gain-safety" ${view.gainSafetyPending?'disabled':''}>${view.gainSafetyPending?'Checking…':'Check shutdown status'}</button></div>`:'');
    view.gainRecoveryNotice=kind==='gain'&&Boolean(recovery);
    const history=store.history(key).filter(e=>e.type==='operation').slice(-16).map(e=>({operation_id:e.data?.operation_id,phase:e.data?.phase,context:e.data?.result?.context}));
    const feedback=[view.gainSafetyActivity,view.exportActivity,view.recoveryActivity,view.historyActivity,view.resultActivity,view.activity].filter(Boolean);
    const activity=feedback.find(activityBusy)||feedback.sort((a,b)=>(b.ended??b.started)-(a.ended??a.started))[0];
    const evidence='<details id="instrument-diagnostics"><summary>Diagnostics</summary><pre>'+esc(JSON.stringify({identity:record.expected_identity,connection_effects:effect,context:value?.context,control:host.control?.[route.domain.kind+':'+route.domain.id],safety:value?.safety,operation:view.operationAttempt,outcome:view.lastOperation,gain_safety:view.gainSafetyAttempt,gain_safety_outcome:view.gainSafetyRecord,latest_result:view.resultOperationId,result_error:view.sharedResultError,timings:{native_capture:view.measuredCaptureTimings,ui_wait:view.activity?.timings,display:view.resultActivity?.timings,history:view.historyActivity?.timings,export:view.exportActivity?.timings},history},null,2))+'</pre></details>';
    const resultNote=view.sharedResultError?'<p class="alert">New spectrum unavailable. Any previous capture is labeled separately. See Diagnostics for details.</p>':'';
    if(kind==='mdt')return header+lifecycle+recovery+renderActivity(activity)+'<h1>MDT693B controller</h1><p>Configure a Fiber setup to use laboratory coordinates.</p><a class="btn" href="#devices">Device setup</a>'+evidence;
    return header+lifecycle+recovery+renderActivity(activity)+resultNote+(panels[kind]?.(view)||'<p>Driver required</p>')+evidence;
  }
  return renderHostIssue(host?.hostIssue)+renderOverview(host,store);
}
export function mountConsole(session,native){
  const localClient=session.client,store=session.store,content=document.querySelector('#content'),locals={},catalog={models:[],categories:[]};
  let wizard=null,connected=false,busyHost=false,hostIssue=null,renderedKey=null,lastMarkup=null,unlisten=null,preferences=null,appProfile={network_only:false},remoteSettings={peers:[],owner:null},hostActivity=null;
  let driverInventory=null,deviceEditor={},driverRefresh=null,closing=false;const pendingActions=new Map(),driverInstallation={installing:false};
  const connections=connectionState();
  const scanShortcuts=createScanShortcuts();let shortcutCaptureError=null;
  const resyncing=new Map(),snapshotWaiters=new Map(),identities=new Map(),remoteUnlisteners=new Map();
  // Two recent verified displays, not device authority or a replay cache.
  // Borrow immutable sample arrays; a changed Host boot/context invalidates a pin.
  const previousDisplays=new Map();
  const page=()=>location.hash||'#overview',activeHostId=()=>parseRoute(page())?.hostId||session.hostId;
  const host=()=>store.host(activeHostId()),localHost=()=>store.host(session.hostId);
  const route=()=>parseRoute(page()),key=()=>route()?deviceKey(route().hostId,route().domain):null;
  const ownerClient=id=>{const selected=session.clientFor(id);if(!selected)throw new Error('Connect the owning Host in Settings');return selected;};
  const client=createRoutedClient(ownerClient,activeHostId);
  const shared=createSharedResults(store,client,k=>{const l=locals[k],result=shared.current(k,{copyTrace:false});if(l&&l.displayResultId!==result.resultOperationId){l.cursor=null;l.displayResultId=result.resultOperationId;}render();});
  const archiveHistory=createArchiveHistory(client,()=>render());
  const archiveScope=()=>{const r=route(),h=host();return h?.connected&&h.synced&&r?.domain.kind==='device'
    &&h.registry.devices.some(d=>d.device_id===r.domain.id&&d.model_id==='aq6370')?{hostId:r.hostId,domain:r.domain}:null;};
  function displayIdentity(k){const d=store.get(k),h=store.host(d?.hostId);return h?.connected&&h.synced&&d?.context?JSON.stringify([h.bootId,d.context]):null;}
  function retainedDisplay(k){const saved=previousDisplays.get(k);if(!saved)return {};
    if(!saved.identity||saved.identity!==displayIdentity(k)){previousDisplays.delete(k);return {};}
    return {previousTrace:saved.trace,previousOperationId:saved.operationId};}
  const displayedTrace=()=>{const state=archiveHistory.state(archiveScope(),{copyTrace:false}),live=shared.current(key(),{copyTrace:false});return state.historical?state.trace:live.trace||live.previousTrace||retainedDisplay(key()).previousTrace;};
  const local=()=>{const k=key();if(!k)throw new Error('Select an instrument');return locals[k]||(locals[k]={});};
  const notice=createNotice(document.querySelector('#notice'));
  const notify=(message,isError=false)=>notice.show(message,isError),clearNotice=()=>notice.hide();
  function error(cause){notify(cause?.message||String(cause),true);}
  function restoreInputs(saved){for(const element of content.querySelectorAll('input,select,details[id]')){if(element.dataset.managed)continue;const id=element.id||element.dataset.draft||'param:'+element.dataset.param;if(saved.has(id)){const item=saved.get(id);if(item&&typeof item==='object'){if('value' in item)element.value=item.value;if('checked' in item)element.checked=item.checked;if('open' in item)element.open=item.open;}else element.value=item;}}}
  function syncGainDomains(){
    for(const domain of store.all()){
      const owner=store.host(domain.hostId),record=[...(owner?.registry?.devices||[]),...(owner?.registry?.drafts||[])].find(d=>d.device_id===domain.domain.id);
      if(domain.domain.kind!=='device'||record?.model_id!=='gain')continue;
      const k=deviceKey(domain.hostId,domain.domain),l=locals[k]||(locals[k]={}),nowMs=performance.now(),ageUpperMs=owner?.connected&&owner.synced?store.ageUpperMs(domain.hostId,domain.host_sample_ms):null;
      syncGainDraft(l,domain,owner?.bootId,ageUpperMs,nowMs);
      const observation=JSON.stringify([owner?.bootId,owner?.connected,owner?.synced,domain.communication,domain.context,domain.receivedAt]);
      if(l.gainObservation!==observation){l.gainHistory=appendGainHistory(l.gainHistory||[],{domain,bootId:owner?.bootId,ageUpperMs,nowMs});l.gainObservation=observation;}
    }
  }
  const renderer=createRenderScheduler(paint);
  function render(){renderer.flush();}
  function paint(){const h=host(),p=page(),nextKey=key()||p;const live=inputSnapshot(content.querySelectorAll('input,select,details[id]'));
    observeDisconnects(locals,store);
    syncGainDomains();
    for(const [k,l]of Object.entries(locals))if(l.baselineConsent){const domain=store.get(k),owner=store.host(domain?.hostId);for(const side of ['left','right'])if(l.baselineConsent[side]?.binding!==baselineBinding(owner,domain,store.canControl(k)?store.lease(k):null,domain?.device?.[side]))delete l.baselineConsent[side];}
    for(const [k,l]of Object.entries(locals))syncTarget(l,laserMotion(store.get(k)?.device));
    const old=shared.current(renderedKey,{copyTrace:false}),oldTrace=old.trace||old.previousTrace;
    if(oldTrace){previousDisplays.delete(renderedKey);previousDisplays.set(renderedKey,{trace:oldTrace,
      operationId:old.resultOperationId||old.previousOperationId,identity:displayIdentity(renderedKey)});
      while(previousDisplays.size>2)previousDisplays.delete(previousDisplays.keys().next().value);}
    shared.refresh(key());archiveHistory.select(archiveScope());const viewLocals={...locals};if(key()){
      const result=shared.current(key(),{copyTrace:false}),previous=!result.trace&&!result.previousTrace?retainedDisplay(key()):{};
      viewLocals[key()]={...locals[key()],...previous,...result,...archiveHistory.state(archiveScope(),{copyTrace:false}),recordingRoot:h?.archive?.active_root,archiveAvailable:h?.archive?.available,scanShortcutPreferences:scanShortcuts.preferences,scanShortcutError:shortcutCaptureError||scanShortcuts.error};}
    viewLocals.remoteSettings=remoteSettings;
    viewLocals.connections=connections;
    const samePage=renderedKey===nextKey,saved=inputValues(renderedKey,nextKey,live,locals[nextKey]?.inputs||new Map());
    if(locals[nextKey]?.gainDraftBinding)for(const id of gainDraftIds)if(!locals[nextKey].gainDraftDirty.has(id)){const value=locals[nextKey].inputs.get(id);if(value!==undefined)saved.set(id,value);else saved.delete(id);}
    const active=focusIdentity(document.activeElement);const selection=[document.activeElement?.selectionStart,document.activeElement?.selectionEnd];const editing=Boolean(document.activeElement?.closest('#content')&&document.activeElement?.matches(focusControls));
    const navigation=document.querySelector('#navigation'),links='<a href="#overview">Overview</a><a href="#devices">Device setup</a><a href="#settings">Settings</a>';
    replaceMarkup(navigation,navigation.innerHTML,links);
    const background=document.querySelector('#background-work');let backgroundMarkup=activityBusy(hostActivity)?renderActivity(hostActivity):'';
    for(const [k,l]of Object.entries(locals)){const work=[l.gainSafetyActivity,l.exportActivity,l.recoveryActivity,l.activity].find(activityBusy);if(k===key()||!work)continue;
      const [hostId,kind,id]=k.split('/');if(!id)continue;const owner=store.host(hostId),record=kind==='setup'?owner?.registry.setups.find(s=>s.setup_id===id):owner?.registry.devices.find(d=>d.device_id===id);
      backgroundMarkup+=`<div class="background-task"><a href="${routeFor(hostId,{kind,id})}">${esc(record?.name||'Instrument')}</a>${renderActivity(work)}</div>`;
    }
    if(background){replaceMarkup(background,background.innerHTML,backgroundMarkup);background.hidden=!backgroundMarkup;}
    document.querySelector('#sidebar-mode').textContent=appProfile.network_only?'Network-only profile':'Local Host';document.querySelector('#sidebar-subtitle').textContent=busyHost?'Connecting…':connected?'Connected':hostIssue?.code==='HostIncompatible'?'Previous Host running':appProfile.network_only?'No local hardware worker':'Disconnected';
    document.querySelector('#session-pill').textContent=(busyHost?'CONNECTING':store.hosts().some(h=>h.connected)?'ONLINE':hostIssue?.code==='HostIncompatible'?'HOST UPDATE REQUIRED':'OFFLINE');
    document.querySelector('#current-page-title').textContent=p==='#devices'?'Device setup':['#host','#settings'].includes(p)?'Settings':route()?'Instrument':'Overview';
    let markup=renderConsole(p==='#devices'?'devices':['#host','#settings'].includes(p)?'settings':p,{...h,driverInventory,driverInstallation,hostIssue,hostBusy:busyHost},store,{...viewLocals,'#devices':deviceEditor},catalog,preferences);
    if(wizard)markup+=renderAddWizard({...wizard,installing:wizard.installing||driverInstallation.installing},catalog);
    const scroll=[content.scrollTop,content.scrollLeft,window.scrollX,window.scrollY];
    markup=markup.replace(/<button\b([^>]*)>([\s\S]*?)<\/button>/g,(whole,attrs,label)=>{const data={};for(const match of attrs.matchAll(/data-(ui|op|device|setup|side|channel|setting|command|driver)="([^"]*)"/g))data[match[1]]=match[2];const text=pendingActions.get(actionKey(data));return text?`<button ${attrs} ${/\bdisabled\b/.test(attrs)?'':'disabled'} aria-busy="true">${esc(text)}</button>`:whole;});
    const replaced=replaceMarkup(content,samePage?lastMarkup:null,markup);lastMarkup=markup;
    if(replaced){restoreInputs(saved);if(editing&&samePage&&active){const element=[...content.querySelectorAll(focusControls)].find(e=>!e.disabled&&focusIdentity(e)===active);element?.focus({preventScroll:true});if(selection[0]!==null&&selection[0]!==undefined&&element?.setSelectionRange)element.setSelectionRange(...selection);}if(samePage){content.scrollTop=scroll[0];content.scrollLeft=scroll[1];if(Number.isFinite(scroll[2])&&Number.isFinite(scroll[3]))window.scrollTo?.(scroll[2],scroll[3]);}}renderedKey=nextKey;
    if(route()&&h){const domain=store.get(key()),r=route().domain,k=r.kind==='setup'?'fiber':driver[h?.registry.devices.find(d=>d.device_id===r.id)?.model_id];
      const state=instanceView(k,domain,store.canControl(key()),store.ageUpperMs(activeHostId(),domain?.host_sample_ms),{...local(),...shared.current(key(),{copyTrace:false}),mode:h.mode});
      if(k==='laser')updateSingleScanButtons(content,state.status.devices.laser);
      if(k==='gain'&&gainPidReadDue({visible:document.visibilityState!=='hidden',role:state.roles.gain,device:state.status.devices.gain,local:local(),bootId:h.bootId,nowMs:state.nowMs})){
        const expectedKey=key(),expectedBoot=h.bootId,expectedContext=structuredClone(domain.context),l=local();
        markGainPidRead(l,expectedBoot,expectedContext);
        queueMicrotask(()=>{if(closing||key()!==expectedKey||store.host(h.host_id)?.bootId!==expectedBoot||!sameJsonValue(store.get(expectedKey)?.context,expectedContext))return;
          const current=instanceView('gain',store.get(expectedKey),store.canControl(expectedKey),store.ageUpperMs(h.host_id,store.get(expectedKey)?.host_sample_ms),l);
          if(current.roles.gain.normalPending||current.roles.gain.safetyPending||current.roles.gain.unknown)return;
          run(r,h.registry.devices.find(d=>d.device_id===r.id)?.config_rev,'action',{name:'read_pid',args:{}},h.host_id,true).catch(error);
        });
      }
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
  function loadDriverStatus(){
    if(!connected)return Promise.resolve();
    if(driverRefresh)return driverRefresh;
    if(!driverInstallation.installing)driverInstallation.error=null;
    const boot=localHost()?.bootId;driverInventory={...driverInventory,checking:true};render();
    driverRefresh=(async()=>{try{const inventory=await localClient.driverStatus();if(connected&&localHost()?.bootId===boot)driverInventory=inventory;}
      catch(cause){if(connected&&localHost()?.bootId===boot)driverInventory={errors:{newport:cause.message||String(cause)}};throw cause;}
      finally{driverRefresh=null;if(driverInventory?.checking)driverInventory.checking=false;render();}})();
    return driverRefresh;
  }
  async function connect(automatic=false){if(busyHost||connected)return;const pendingStart=hostIssue?.code==='HostStartPending';busyHost=true;hostIssue=null;hostActivity=startActivity('host','host-check');render();try{if(automatic){if(!preferences)throw new Error('Review and save local Host settings before starting.');await connectLocalHost(localClient,preferences,phase=>{hostActivity=advanceActivity(hostActivity,phase);render();});}else await localClient.connect();hostActivity=advanceActivity(hostActivity,'host-load');render();Object.assign(catalog,await localClient.catalog());
      unlisten=await localClient.subscribe(event=>{const applied=session.apply(event);if(applied.currentChanged)syncGainDomains();if(event.type==='snapshot')(snapshotWaiters.get(event.host_id)||snapshotWaiters.get(null))?.receive(event);if(applied.requiresSnapshot)syncHost(event.host_id).catch(error);});hostActivity=advanceActivity(hostActivity,'sync');render();await syncHost(session.hostId);
      connected=true;if(page()==='#settings')loadDriverStatus().catch(error);
    }catch(cause){hostIssue={code:pendingStart?'HostStartPending':cause?.code,message:cause?.message||String(cause)};unlisten?.();unlisten=null;try{await localClient.disconnect();}catch(cleanup){error(cleanup);}connected=false;session.offline();throw cause;
    }finally{hostActivity=finishActivity(hostActivity,connected?'complete':'failed');busyHost=false;render();}}
  async function stopDomain(domain,id=activeHostId()){const client=ownerClient(id),host=()=>store.host(id),resync=()=>syncHost(id),k=deviceKey(id,domain),l=locals[k]||(locals[k]={});
    if(l.disconnectInFlight)throw new Error('Disconnect is already in progress.');
    const lease=store.lease(k);if(lease){l.disconnectOwner=lease.session_id;l.disconnectEpoch=lease.control_epoch;l.disconnectBoot=lease.boot_id;}
    l.disconnectEvidenceBoot=host()?.bootId;l.disconnectEvidenceSeq=host()?.seq;l.disconnectAccepted=false;l.disconnectInFlight=true;
    l.connectionGeneration=(l.connectionGeneration||0)+1;l.disconnecting=performance.now();l.activity=startActivity('disconnect','disconnect');l.unknown=true;store.dropLease(k);render();
    try{const report=await client.safeStop(domain);
      if(report?.accepted!==true)throw new Error('Disconnect acceptance is unconfirmed.');
      // A coalesced snapshot requested before stop acceptance cannot prove release.
      if(resyncing.has(id))await resyncing.get(id);
      if(host()?.bootId!==l.disconnectEvidenceBoot)throw new Error('Host changed during disconnect. Release is unconfirmed.');
      l.disconnectEvidenceSeq=host()?.seq;l.disconnectAccepted=true;await resync();
      if(host()?.bootId!==l.disconnectEvidenceBoot)throw new Error('Host changed during disconnect. Release is unconfirmed.');
      return report;}
    catch(cause){l.disconnecting=null;l.disconnectFailed=true;l.activity=finishActivity(l.activity,'unknown');throw cause;}
    finally{l.disconnectInFlight=false;render();}}
  async function run(domain,rev,method,params,id=activeHostId(),quiet=false){
    const client=ownerClient(id),host=()=>store.host(id),resync=()=>syncHost(id),k=deviceKey(id,domain),l=locals[k]||(locals[k]={});
    if(l.pending||l.connecting||l.gainSafetyPending)throw new Error('An operation is already in progress.');
    if(l.gainSafetyUnknown&&method!=='connect')throw new Error('Check the original Gain shutdown status before continuing.');
    if(method==='connect'){const view=connectionView(host(),store,k,l);if(view.operation!=='connect'||view.disabled)throw new Error('Connection is unavailable. Check status or disconnect first.');}
    if(l.unknown&&method!=='connect')throw new Error('Check the operation status or disconnect before continuing.');
    const registry=host().registry,record=domain.kind==='setup'?registry.setups.find(s=>s.setup_id===domain.id):[...registry.devices,...registry.drafts].find(d=>d.device_id===domain.id);
    const ownCatalog=id===session.hostId?catalog:session.catalogFor(id);
    const model=domain.kind==='setup'?{id:'fiber-coupling'}:ownCatalog?.models.find(m=>m.id===record?.model_id);
    if(method==='connect'&&!model)throw new Error('Trusted connection effects are unavailable. No connection was made.');
    let acquired=false,activityId=null;
    const phase=name=>{if(l.activity?.id===activityId){l.activity=advanceActivity(l.activity,name);render();}};
    if(method==='connect'){
      delete l.targetValue;delete l.observedTarget;l.targetDirty=false;l.targetSending=false;l.laserScanning=false;
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
    const requestId=crypto.randomUUID().replaceAll('-','');l.pending=requestId;l.pendingName=params.name||method;
    let releaseBoundary;const boundary=new Promise(resolve=>releaseBoundary=resolve);l.runBoundary=boundary;
    if(!quiet){l.activity=method==='connect'?advanceActivity(l.activity,'prepare'):startActivity(params.name||method,'prepare');activityId=l.activity.id;if(!activityBusy(l.exportActivity))delete l.exportActivity;}render();
    l.operationAttempt={request_id:requestId,domain,boot_id:lease.boot_id,context:intent.context};
    let outcome='failed',dispatchStarted=false,terminalKnown=false,syncStarted=false;try{const proof=await client.prepare(intent);intent.confirmation=proof.token;
      phase('instrument');
      dispatchStarted=true;let record=await client.execute(requestId,intent);record=await pollOriginalOperation(client,requestId,record);
      l.lastOperation=record;
      if(record.status==='Outcome Unknown'||record.phase==='timed_out_unknown'){l.unknown=true;outcome='unknown';return record;}
      if(model?.id==='gain'&&(record.status!=='Terminal'||record.request_id!==requestId||!sameJsonValue(record.domain,domain)))throw Object.assign(new Error('Gain operation identity mismatch. Check the original status.'),{outcomeUnknown:true});
      terminalKnown=true;
      if(record.phase!=='completed'){if(method==='connect')await resync();const failure=new Error(record.result?.error?.message||record.result?.error||record.phase);
        if(model?.id==='gain'&&['start_current','ramp_current'].includes(params.name)&&(record.result?.error?.type==='Canceled'||record.result?.error?.type==='DriverError'&&record.result.error.message==='Canceled'))Object.assign(failure,{gainCanceled:true,operation:record});throw failure;}
      if(method==='connect'||method==='action'&&params.name==='read_status')delete l.laserRefreshFailed;
      syncStarted=true;phase('sync');await resync();syncStarted=false;outcome='complete';
      if(!currentResult(intent,record.result,store,k))return record;
      if(key()===k)await shared.refresh(k);return record;
    }catch(cause){if(cause.outcomeUnknown||['AdmissionPending','OutcomeUnknown'].includes(cause.code)||model?.id==='gain'&&dispatchStarted&&(!terminalKnown||syncStarted)){l.unknown=true;l.unknownRequestId=requestId;}
      else if(acquired&&!store.get(k)?.context?.connection_id){try{await client.release(domain,lease);store.dropLease(k);await resync();}catch(cleanup){l.unknown=true;cause.cleanupError=cleanup;}}
      throw cause;
    }finally{if(l.activity?.id===activityId)l.activity=finishActivity(l.activity,l.unknown?'unknown':outcome);l.pending=null;delete l.pendingName;if(l.runBoundary===boundary)delete l.runBoundary;releaseBoundary();render();}
  }
  const setup=createSetupActions(session,null,()=>syncHost(session.hostId),(domain,rev,method,params)=>run(domain,rev,method,params,session.hostId));
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
    try{session.setCatalog(id,await remote.catalog());const stop=await remote.subscribe(event=>{const applied=session.apply(event,true);if(applied.currentChanged)syncGainDomains();if(event.type==='snapshot')snapshotWaiters.get(id)?.receive(event);if(applied.requiresSnapshot)syncHost(id).catch(error);});remoteUnlisteners.set(id,stop);await syncHost(id);}
    catch(cause){remoteUnlisteners.get(id)?.();remoteUnlisteners.delete(id);try{await remote.disconnect();session.removeRemote(id);}catch(cleanup){error(cleanup);}session.offline(id);throw cause;}
    await refreshRemote();
  }
  async function runLaserAction(domain,rev,action){
    const id=activeHostId(),owner=()=>store.host(id),k=deviceKey(id,domain),l=locals[k]||(locals[k]={}),
      boot=owner()?.bootId,context=JSON.stringify(store.get(k)?.context),token=store.lease(k)?.token;
    const stop=action.name==='stop_scan',output=action.name==='control_output',move=['goto_wavelength','start_scan','scan_forward','scan_backward'].includes(action.name);
    if(stop){l.moveGeneration=(l.moveGeneration||0)+1;l.queuedStop=true;}
    const generation=l.moveGeneration||0;
    if(output)l.outputSending=true;if(action.name==='goto_wavelength')l.targetSending=true;
    l.queuedLaser=(l.queuedLaser||0)+1;
    const previous=l.laserActionTail;
    const task=(async()=>{
      await previous?.catch(()=>{});
      await l.runBoundary;
      // Await only the exact prior exchange. Never send after authority/context
      // loss, an unknown result, or a disconnect while this intent was queued.
      if(closing||!owner()?.connected||!owner()?.synced||owner()?.bootId!==boot||!store.canControl(k)||l.unknown||
        store.lease(k)?.token!==token||JSON.stringify(store.get(k)?.context)!==context)
        throw new Error('Connection authority changed while waiting. No queued command was sent.');
      if(move&&generation!==(l.moveGeneration||0))return {phase:'canceled_before_call'};
      const record=await run(domain,rev,'action',action,id);
      if(action.name==='goto_wavelength'&&record.phase==='completed'&&l.targetValue===action.args.wavelength_nm)l.targetDirty=false;
      // The Rust scheduler owns motion observations and completion, including
      // tracking-off after arrival. No GUI setter replay or arrival polling.
      return record;
    })();
    l.laserActionTail=task;render();
    try{return await task;}finally{
      l.queuedLaser--;if(stop)l.queuedStop=false;if(output)l.outputSending=false;if(action.name==='goto_wavelength')l.targetSending=false;
      if(l.laserActionTail===task)delete l.laserActionTail;render();
    }
  }
  async function checkWizardDrivers(){if(!wizard)return;const draft=wizard,model=catalog.models.find(m=>m.id===draft.modelId),profile=model?.profiles.find(p=>p.id===draft.profileId);
    return refreshDraftConnection(draft,model,profile,()=>localClient.driverStatus(),()=>localClient.scanLasers(),()=>{if(wizard===draft)render();});
  }
  async function releaseDraft(d){if(!d.record)return;const domain={kind:'device',id:d.record.device_id},k=deviceKey(session.hostId,domain);
    if(store.lease(k)){await stopDomain(domain,session.hostId);const deadline=performance.now()+10000;while(!connectionReleased(localHost(),store.get(k))){if(performance.now()>deadline)throw new Error('Connection release is unconfirmed. Retry Cancel.');await new Promise(r=>setTimeout(r,150));await syncHost(session.hostId);}}
  }
  async function installWizardDriver(){const draft=wizard;if(!draft||driverInstallation.installing)return;
    if(!connected)throw new Error('Local Host is unavailable.');
    const model=catalog.models.find(m=>m.id===draft.modelId),profile=model?.profiles.find(p=>p.id===draft.profileId);
    const boot=localHost()?.bootId,isCurrent=()=>!closing&&connected&&localHost()?.bootId===boot&&wizard===draft&&draft.modelId===model?.id&&draft.profileId===profile?.id;
    Object.assign(driverInstallation,{installing:true,driver:draft.driverCheck?.driver,error:null});render();
    try{
      await installMissingDriver(draft,model,profile,localClient,()=>{if(wizard===draft)render();},{isCurrent});
      if(isCurrent()&&draft.driverCheck?.state==='ready')await checkWizardDrivers();
    }catch(cause){if(!isCurrent()){draft.driverCheck=null;draft.controllerScan=null;}throw cause;}
    finally{driverInstallation.installing=false;render();}
  }
  async function installSettingsDriver(id){
    if(driverInstallation.installing)return;
    if(!connected||driverState(driverInventory,id)!=='missing')throw new Error('Refresh drivers before installing.');
    const boot=localHost()?.bootId,isCurrent=()=>!closing&&connected&&localHost()?.bootId===boot;Object.assign(driverInstallation,{installing:true,driver:id,error:null});render();
    try{
      await installDriverPackage(id,localClient,async()=>{
        const inventory=await localClient.driverStatus();
        if(!connected||localHost()?.bootId!==boot)throw new Error('Local Host changed. Refresh drivers before continuing.');
        driverInventory=inventory;render();return {state:driverState(inventory,id)};
      },{isCurrent});
      notify(driverLabel(id)+' driver ready.');
    }catch(cause){driverInstallation.error=cause.message||String(cause);throw cause;}
    finally{driverInstallation.installing=false;render();}
  }
  function currentRecord(){const r=route();return r.domain.kind==='setup'?host().registry.setups.find(s=>s.setup_id===r.domain.id):host().registry.devices.find(d=>d.device_id===r.domain.id);}
  const get=id=>{const element=document.getElementById(id);return element?.type==='checkbox'?element.checked:element?.value??'';};
  const actionKey=data=>JSON.stringify([key()||page(),...['ui','op','device','setup','side','channel','setting','command','driver'].map(k=>data[k]||'')]);
  function progressLabel(data){return ({'connect':'Connecting…','disconnect':'Disconnecting…','osa-read':'Reading trace…','osa-acquire':'Acquiring…','osa-export':'Exporting…','test-draft':'Testing connection…','save-draft':'Saving…','prepare-draft':'Connecting…','cancel-wizard':'Closing…','cancel-draft':'Closing…','refresh-device':'Refreshing…','scan-controllers':'Scanning…','check-draft-drivers':'Checking…','check-drivers':'Checking…','save-checks':'Saving…','save-rename':'Saving…','save-host':'Saving…','retire':'Removing…','retire-setup':'Removing…','save-setup':'Saving…','stop-host':'Stopping Host…','connect-host':'Connecting…','start-host':'Starting Host…','install-draft-driver':'Installing…','install-driver':'Installing…','query-original':'Checking status…','laser-read':'Refreshing…','laser-goto':'Starting wavelength move…','laser-scan-start':'Starting scan…','laser-scan-stop':'Stopping scan…','laser-wavelength':'Tuning…','laser-piezo':'Adjusting…','save-laser-limits':'Saving & reconnecting…','pm-measure':'Reading…'}[data.ui||data.op]||'Working…');}
  async function uiAction(button){const name=button.dataset.ui,actionHost=activeHostId(),client=createRoutedClient(ownerClient,()=>actionHost),host=()=>store.host(actionHost),resync=()=>syncHost(actionHost);
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
    if(name==='save-laser-limits'){
      if(store.host(actionHost)?.remote)throw new Error('Edit and save Laser limits on the owning Host on its local computer.');
      const r=route(),k=key(),l=local(),record=currentRecord(),domain=store.get(k);
      if(record.model_id!=='tlb6700'||!store.canControl(k)||l.pending||l.limitsSaving)throw new Error('Laser controls are unavailable.');
      const limits=operatingLimits(get,domain?.device),originalBoot=host()?.bootId,originalClient=ownerClient(actionHost),originalDomain=structuredClone(r.domain);
      const checkOwner=()=>{const owner=store.host(actionHost);
        if(!closing&&owner?.connected&&owner.synced&&owner.bootId===originalBoot&&session.clientFor(actionHost)===originalClient)return;
        l.unknown=true;l.disconnectAccepted=false;l.disconnectFailed=true;l.disconnecting=null;
        if(l.activity?.kind==='disconnect')l.activity=l.activity.ended===undefined?finishActivity(l.activity,'unknown'):{...l.activity,outcome:'unknown'};
        throw Object.assign(new Error('Host changed during limits update. Save and reconnect were stopped. Review the owning Host before continuing.'),{outcomeUnknown:true});
      };
      l.limitsSaving=true;render();
      try{
        await stopDomain(originalDomain,actionHost);checkOwner();
        const deadline=performance.now()+15000;
        while(!connectionReleased(host(),store.get(k))){
          if(performance.now()>deadline)throw new Error('Disconnect is still pending. Limits were not saved.');
          await new Promise(yes=>setTimeout(yes,150));checkOwner();await resync();checkOwner();
        }
        checkOwner();
        const latest=host().registry.devices.find(d=>d.device_id===record.device_id);
        await originalClient.saveLaserLimits({device_id:latest.device_id,config_rev:latest.config_rev,expected_rev:host().registry.registry_rev,limits});checkOwner();
        await resync();checkOwner();
        for(const id of ['laser-scan-start','laser-scan-stop','laser-scan-speed','laser-scan-return-speed','laser-wavelength'])l.inputs?.delete(id);
        delete l.targetValue;delete l.observedTarget;renderedKey=null;
        const saved=host().registry.devices.find(d=>d.device_id===record.device_id);
        checkOwner();await run(originalDomain,saved.config_rev,'connect',{acknowledge_lifecycle:true},actionHost);checkOwner();
      }finally{l.limitsSaving=false;render();}
      return;
    }
    if(name==='check-drivers')return loadDriverStatus();
    if(name==='install-driver')return installSettingsDriver(button.dataset.driver);
    if(name==='check-draft-drivers'||name==='scan-controllers')return checkWizardDrivers();
    if(name==='scan-serial')return checkWizardDrivers();
    if(name==='serial-show-all'){if(wizard){wizard.showAllPorts=!wizard.showAllPorts;render();}return;}
    if(name==='serial-manual'){if(wizard){wizard.manualPort=!wizard.manualPort;wizard.proof=null;render();}return;}
    if(name==='install-draft-driver')return installWizardDriver();
    if(name==='osa-history-refresh'||name==='osa-history-more')return archiveHistory.list(name==='osa-history-more');
    if(name==='osa-history-load'){local().cursor=null;delete local().exportDirectory;return archiveHistory.load(button.dataset.archive,button.dataset.name);}
    if(name==='osa-current'){local().cursor=null;archiveHistory.showCurrent();return;}
    if(busyHost)throw new Error('Local Host connection is in progress.');
    if(name==='connect-host'||name==='start-host')return connect(hostIssue?.code!=='HostStartPending');
    if(name==='disconnect-host'){await client.disconnect();unlisten?.();unlisten=null;connected=false;session.offline();return;}
    if(name==='stop-host'){const affected=host().registry.devices.map(d=>d.name).join(', ');if(!confirm(`Stop this Host and all its devices (${affected||'none'})? All controllers lose authority. Voltage: zero; Gain: current off before TEC off; piezo: hold. Other GUIs will disconnect.`))return;const report=await client.stopHost();notify(JSON.stringify(report));if(!report.resource_released||report.process_exit?.confirmed!==true||report.process_exit?.success!==true)throw new Error('Host stop is retained. Keep this management window open.');connected=false;await client.disconnect();session.offline();return;}
    if(name==='save-host'){
      const chosen={};
      await client.savePreferences(chosen);preferences=chosen;
      if(connected){await client.saveSettings({settings:updatedHostSettings(host().registry.settings,get('host-name'),get('host-data-root')),expected_rev:host().registry.registry_rev});await resync();}
      notify('Settings saved locally. Startup changes apply after a safe Host restart.');return;
    }
    if(name==='choose-data-root'){
      if(!connected)throw new Error('Local Host is unavailable.');
      const selected=await client.chooseDataRoot();if(selected===null)return;
      const current=host();await client.saveSettings({settings:updatedHostSettings(current.registry.settings,current.registry.settings?.host_name||current.host_name||'Local computer',selected),expected_rev:current.registry.registry_rev});
      const target=document.getElementById('host-data-root');
      const l=locals['#settings']||={};l.inputs??=new Map();l.inputs.set('host-data-root',selected);
      if(target)target.value=selected;
      await resync();notify('Measurement data folder saved. It applies after the next safe Host restart.');return;
    }
    if(name==='refresh-device'){
      const d=host().registry.devices.find(d=>d.device_id===button.dataset.device);
      await client.refreshDevice({device_id:d.device_id,config_rev:d.config_rev});return resync();
    }
    if(name==='save-checks'){
      const d=host().registry.devices.find(d=>d.device_id===button.dataset.device),id=d.device_id;
      const profile=catalog.models.find(m=>m.id===d.model_id)?.profiles.find(p=>p.id===d.profile_id),enumeration=true,readonly=Boolean(profile?.automatic_probe&&profile.probe_mode==='readonly'&&!profile.open_effects.length);
      await client.saveCheckPolicy({device_id:id,config_rev:d.config_rev,expected_rev:host().registry.registry_rev,interval_s:Number(get('check-interval-'+id)),enumeration,readonly});return resync();
    }
    if(name==='add-new'){wizard={category:catalog.categories[0],params:{},name:''};render();return;}
    if(name==='open-draft'){const d=host().registry.drafts.find(d=>d.device_id===button.dataset.device);wizard={category:catalog.models.find(m=>m.id===d.model_id)?.category||catalog.categories[0],modelId:d.model_id,profileId:d.profile_id,name:d.name,params:{...d.params},record:d};wizard.recordSignature=signature(wizard);render();return checkWizardDrivers();}
    if(name==='safe-stop-draft')return stopDomain({kind:'device',id:wizard.record.device_id});
    if(name==='cancel-wizard'||name==='cancel-draft'){const d=name==='cancel-wizard'?wizard:{record:host().registry.drafts.find(d=>d.device_id===button.dataset.device)};if(!d||d.installing)return;if(d.busy){d.cancelRequested=true;return;}d.busy=true;render();try{await releaseDraft(d);await setup.cancel(d);d.driverCheck=null;d.controllerScan=null;wizard=null;}catch(cause){d.releaseError=cause.message;throw cause;}finally{d.busy=false;render();}return;}
    if(name==='test-draft'||name==='save-draft'){if(!wizard)throw new Error('Open a draft first');if(wizard.busy||wizard.installing)return;clearNotice();const model=catalog.models.find(m=>m.id===wizard.modelId),profile=model?.profiles.find(p=>p.id===wizard.profileId);wizard.busy=true;wizard.connectionError=null;render();try{
      if(name==='test-draft'){
        if(!driverCheckReady(wizard,profile)||!serialChoiceReady(wizard,profile)||(model?.id==='tlb6700'&&!controllerChoiceReady(wizard)))await checkWizardDrivers();
        wizard.proof=null;
        if(profile?.probe_mode==='supervised'){
          const domain=wizard.record?{kind:'device',id:wizard.record.device_id}:null,k=domain?deviceKey(session.hostId,domain):null,current=k?locals[k]:null;
          await setup.prepare(wizard,model,{outcomeUnknown:Boolean(current?.unknown),pending:Boolean(current?.pending||current?.connecting||current?.disconnectInFlight),
            cancelled:()=>Boolean(wizard?.cancelRequested),onProgress:message=>{wizard.message=message;render();}});
        }
        if(wizard.cancelRequested)return;
        wizard.message='Verifying instrument identity…';render();await setup.test(wizard,model,profile);wizard.message='Connection verified. Select Add & Save to register this device.';
      }else{const saved=await setup.save(wizard);wizard=null;location.hash=routeFor(session.hostId,{kind:'device',id:saved.device_id});}
    }catch(cause){if(name==='test-draft'){wizard.proof=null;wizard.message=null;const domain=wizard.record?{kind:'device',id:wizard.record.device_id}:null;
      wizard.connectionError='Connection failed: '+draftFailureMessage(cause,domain?store.get(deviceKey(session.hostId,domain)):null);throw new Error(wizard.connectionError);}throw cause;}finally{if(wizard){wizard.busy=false;if(wizard.cancelRequested){wizard.cancelRequested=false;await uiAction({dataset:{ui:'cancel-wizard'}});}}render();}return;}
    if(name==='rename'){deviceEditor={rename:button.dataset.device};renderedKey=null;render();return;}
    if(name==='cancel-editor'){deviceEditor={};renderedKey=null;render();return;}
    if(name==='save-rename'||name==='retire'){const d=host().registry.devices.find(d=>d.device_id===button.dataset.device),params={device_id:d.device_id,config_rev:d.config_rev,expected_rev:host().registry.registry_rev};
      if(name==='save-rename'){params.name=get('rename-name').trim();if(!params.name)throw new Error('Enter an instrument name.');await client.renameDevice(params);deviceEditor={};}else await client.retireDevice(params);return resync();}
    if(name==='add-setup'){if(!setupChoices(host().registry).length)throw new Error('Register a controller with one of the two known serial identities first.');deviceEditor={addSetup:true};renderedKey=null;render();return;}
    if(name==='save-setup'){const members=setupChoices(host().registry).filter(d=>document.getElementById('setup-member-'+d.device_id)?.checked).map(d=>d.device_id),name=get('setup-name').trim();if(!members.length||!name)throw new Error('Enter a name and select a controller.');await client.saveSetup({name,members,expected_rev:host().registry.registry_rev});deviceEditor={};return resync();}
    if(name==='retire-setup'){const s=host().registry.setups.find(s=>s.setup_id===button.dataset.setup);const report=await client.retireSetup({setup_id:s.setup_id,config_rev:s.config_rev,expected_rev:host().registry.registry_rev});if(report.restart_required)notify('Setup removed. Restart Host before reassigning its controllers.');return resync();}
    if(name==='safe-stop')return stopDomain(route().domain);
    if(name==='query-gain-safety')return gainOff(local().gainSafetyAttempt?.name);
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
  function gainOff(name){const r=route(),record=currentRecord();return sendGainSafety({client:ownerClient(r.hostId),store,hostId:r.hostId,domain:r.domain,rev:record.config_rev,local:local(),name,resync:()=>syncHost(r.hostId),onChange:render,isCurrent:()=>!closing});}
  function gainNormal(r,rev,action){const k=deviceKey(r.hostId,r.domain),l=locals[k]||(locals[k]={});
    const readState=()=>{const h=store.host(r.hostId),d=store.get(k),v=instanceView('gain',d,store.canControl(k),h?.connected&&h.synced?store.ageUpperMs(r.hostId,d?.host_sample_ms):null,{...l,gainNormalFlow:false});return {role:v.roles.gain,device:v.status.devices.gain,bootId:h?.bootId,context:d?.context,lease:store.lease(k)};};
    return submitGainAction({action,readState,local:l,run:(method,params)=>run(r.domain,rev,method,params,r.hostId),onChange:render,isCurrent:()=>!closing});
  }
  content.addEventListener('click',event=>{if(wizard&&event.target.closest('[data-wizard-backdrop]')){uiAction({dataset:{ui:'cancel-wizard'}}).catch(error);return;}const button=event.target.closest('button');if(!button||button.disabled)return;
    if(button.dataset.scanShortcutUnassign!==undefined||button.dataset.scanShortcutsReset!==undefined){shortcutCaptureError=null;if(button.dataset.scanShortcutUnassign!==undefined)scanShortcuts.unassign(button.dataset.scanShortcutUnassign);else scanShortcuts.reset();render();return;}
    const taskKey=actionKey(button.dataset);if(pendingActions.has(taskKey))return;pendingActions.set(taskKey,progressLabel(button.dataset));render();
    (async()=>{if(button.dataset.ui)return uiAction(button);
      if(button.dataset.pmTab){local().pmTab=button.dataset.pmTab;render();return;}
      const op=button.dataset.op;if(!op)return;if(op==='disconnect'){
        if(connectionView(host(),store,key(),local()).disabled)throw new Error('This instrument is in use by another window.');
        return stopDomain(route().domain);}
      const record=currentRecord();if(op==='connect')return run(route().domain,record.config_rev,'connect',{acknowledge_lifecycle:true});
      if(op==='resume')return run(route().domain,record.config_rev,'resume',{confirm:true});
      if(op==='gain-disable-current'||op==='gain-disable-tec')return gainOff(op==='gain-disable-current'?'disable_current':'disable_tec');
      if(op==='gain-read-pid')markGainPidRead(local(),host().bootId,store.get(key())?.context);
      if(op==='osa-retry-save'){
        const l=local(),scope=archiveScope();if(l.savingCapture||l.pending||!scope)return;
        const name=get('osa-name');if(!/^[A-Za-z0-9][A-Za-z0-9_-]{0,39}$/.test(name))throw new Error('Enter a short recording name');
        const owner=ownerClient(scope.hostId),originalBoot=store.host(scope.hostId)?.bootId,originalDomain=structuredClone(scope.domain);
        const current=()=>{const h=store.host(scope.hostId);return !closing&&h?.connected&&h.synced&&h.bootId===originalBoot&&session.clientFor(scope.hostId)===owner;};
        const checkOwner=()=>{if(!current())throw Object.assign(new Error('Host changed during capture recovery. Reconnect the owning Host and check saved captures before continuing.'),{outcomeUnknown:true});};
        const selected=()=>{const target=archiveScope();return target?.hostId===scope.hostId&&target.domain.kind===originalDomain.kind&&target.domain.id===originalDomain.id;};
        l.savingCapture=true;l.activity=startActivity('retry_staging','storage');const activityId=l.activity.id;render();let outcome='failed';
        try{const saved=await owner.recoverCapture({domain:originalDomain,capture_id:button.dataset.capture,name});checkOwner();
          validateReference(saved.archive_ref,scope);await syncHost(scope.hostId);checkOwner();outcome='complete';
          if(selected()&&!archiveHistory.state(scope,{copyTrace:false}).historyBusy){
            try{await archiveHistory.load(saved.archive_ref.id,saved.archive_ref.name,saved.archive_ref,{current});}
            catch(cause){checkOwner();if(selected())throw new Error('Original spectrum saved, but loading it failed: '+(cause.message||String(cause)));}
            checkOwner();
          }
          if(l.activity?.id===activityId){l.lastOperation={phase:'completed',result:{result:saved}};l.cursor=null;}
          notify('Original spectrum saved.');
        }catch(cause){if(cause.outcomeUnknown||!current())outcome='unknown';throw cause;}
        finally{if(l.activity?.id===activityId)l.activity=finishActivity(l.activity,outcome);l.savingCapture=false;render();}return;
      }
      if(op==='osa-export'){const trace=displayedTrace(),scope=archiveScope(),l=local();if(!trace||l.exporting)return;if(!scope)throw new Error('Reconnect the owning Host to export this capture.');l.exporting=true;l.exportActivity=startActivity('export','export');render();let outcome='failed';
        try{const receipt=await exportSelectedTrace(client,trace,scope);outcome=receipt?'complete':'cancelled';if(receipt){l.exportDirectory=receipt.directory;notify('Capture exported: '+receipt.directory);}}
        finally{l.exportActivity=finishActivity(l.exportActivity,outcome);l.exporting=false;render();}return;}
      let action;
      if(op==='pm-measure')action={name:'measure_kind',args:{kind:get('pm-kind')}};
      else if(op==='pm-read'||op==='pm-write'){const setting=store.get(key()).device.catalog.settings.find(s=>s.key===button.dataset.setting);
        const payload=buildPmSettingAction(setting,op==='pm-read'?'read':'write',{raw:get(`pm-value-${setting.key}`),group:get(`pm-group-${setting.key}`),selector:get(`pm-selector-${setting.key}`),confirmed:op==='pm-write'});
        delete payload.role;const name=payload.name==='read'?'read_setting':'write_setting';delete payload.name;action={name,args:payload};}
      else if(op==='pm-command')action={name:'run_maintenance',args:{command:button.dataset.command,confirm:true}};
      else {const data={...button.dataset};if(op.startsWith('gain-'))data.device=instanceView('gain',store.get(key()),store.canControl(key()),store.ageUpperMs(activeHostId(),store.get(key())?.host_sample_ms),local()).status.devices.gain;if(op==='fiber-adopt'){const side=data.side,domain=store.get(key()),stage=domain?.device?.[side],binding=baselineBinding(host(),domain,store.canControl(key())?store.lease(key()):null,stage);if(!baselineAuthorized(local().baselineConsent?.[side],binding))throw new Error('Review and check both baseline attestations first.');delete local().baselineConsent[side];}action=actionFor(op,get,data);}
      if(op==='osa-read'||op==='osa-acquire'){local().cursor=null;delete local().exportDirectory;archiveHistory.showCurrent();}
      if(op==='laser-scan-stop'){local().laserScanning=true;}
      if(['laser-scan-start','laser-scan-forward','laser-scan-backward'].includes(op)){const l=local();l.laserScanning=true;l.scanStarting=true;return runLaserAction(route().domain,record.config_rev,action).finally(()=>{l.scanStarting=false;render();});}
      return op.startsWith('laser-')?runLaserAction(route().domain,record.config_rev,action):op.startsWith('gain-')?gainNormal(route(),record.config_rev,action):run(route().domain,record.config_rev,'action',action);
    })().catch(error).finally(()=>{pendingActions.delete(taskKey);render();});
  });
  content.addEventListener('change',event=>{const element=event.target;
    if(element.id==='gain-trend-window-s'&&key()){const seconds=Number(element.value);if([60,300,900].includes(seconds)){local().gainTrendWindowS=seconds;local().inputs??=new Map();local().inputs.set(element.id,{value:element.value});render();}return;}
    if(element.dataset?.scanShortcutsEnabled!==undefined){shortcutCaptureError=null;scanShortcuts.update({...scanShortcuts.preferences,enabled:element.checked});render();return;}
    if(element.id==='laser-refresh-interval'){
      const record=currentRecord(),interval=Number(element.value);
      if(![10,30,60].includes(interval))return;
      client.saveCheckPolicy({device_id:record.device_id,config_rev:record.config_rev,expected_rev:host().registry.registry_rev,interval_s:interval,enumeration:false,readonly:false}).then(resync).catch(error);return;
    }
    if(element.dataset.attestation&&key()){const side=element.dataset.side,domain=store.get(key()),stage=domain?.device?.[side],binding=baselineBinding(host(),domain,store.canControl(key())?store.lease(key()):null,stage),l=local();l.baselineConsent??={};const c=l.baselineConsent[side]?.binding===binding?l.baselineConsent[side]:{binding};l.baselineConsent[side]={...c,[element.dataset.attestation]:element.checked};render();return;}
    if(!wizard||wizard.busy||wizard.installing)return;
    if(element.dataset.draft){const name=element.dataset.draft;wizard[name]=element.value;
      if(['category','modelId','profileId'].includes(name))renderedKey=null;
      if(name==='category'){wizard.modelId=null;wizard.profileId=null;wizard.params={};}
      if(name==='modelId'){const model=catalog.models.find(m=>m.id===wizard.modelId);wizard.profileId=model?.profiles[0]?.id;wizard.name=model?.name||'';wizard.params={};}
      if(name==='profileId')wizard.params={};
      if(['category','modelId','profileId'].includes(name)){wizard.manualPort=false;wizard.showAllPorts=false;wizard.serialScan=null;}
    }else if(element.dataset.param){const f=catalog.models.find(m=>m.id===wizard.modelId).profiles.find(p=>p.id===wizard.profileId).fields[element.dataset.param];wizard.params[element.dataset.param]=['number','integer'].includes(f.kind)?Number(element.value):element.value;}
    wizard.proof=null;wizard.connectionError=null;render();if(['category','modelId','profileId'].includes(element.dataset.draft)||element.dataset.param==='port')checkWizardDrivers().catch(error);
  });
  function targetEdit(element,eventKey){
    const l=local(),device=store.get(key())?.device;
    const edited=editTarget(element.value,element.selectionStart,eventKey,device?.operating_range_nm||device?.wavelength_range_nm);
    if(!edited)return;
    element.setSelectionRange(edited.position,edited.position+1);
    if(eventKey==='ArrowLeft'||eventKey==='ArrowRight')return;
    l.targetValue=edited.value;l.targetDirty=true;element.value=formatTarget(edited.value);element.setSelectionRange(edited.position,edited.position+1);render();
  }
  function rememberInputDraft(element,dirty=true){
    const l=local();l.inputs??=new Map();
    if(gainDraftIds.includes(element.id)){
      if(dirty){l.gainDraftDirty??=new Set();l.gainDraftDirty.add(element.id);}
      l.inputs.set(element.id,{value:element.value,...(element.type==='checkbox'?{checked:element.checked}:{})});
    }else l.inputs.set(element.id,element.value);
  }
  function scanDigitEdit(element,eventKey){
    const precision=Number(element.dataset.digits),whole=Number(element.dataset.whole),range=[Number(element.getAttribute('min')),Number(element.getAttribute('max'))];
    if(element.value.trim()===''||!Number.isFinite(Number(element.value)))return;
    const text=formatDigits(Number(element.value),precision,whole);
    const position=text===element.value&&element.selectionEnd===element.selectionStart+1?element.selectionStart:text.length-1;
    const edited=editDigits(text,position,eventKey,range,precision,whole);
    if(!edited)return;
    element.value=formatDigits(edited.value,precision,whole);element.setSelectionRange(edited.position,edited.position+1);
    rememberInputDraft(element,eventKey!=='ArrowLeft'&&eventKey!=='ArrowRight');
    if(eventKey==='ArrowLeft'||eventKey==='ArrowRight')return;
    updateSingleScanButtons(content,store.get(key())?.device);
  }
  content.addEventListener('input',event=>{if(['laser-scan-speed','laser-scan-return-speed'].includes(event.target.id))updateSingleScanButtons(content,store.get(key())?.device);});
  content.addEventListener('keydown',event=>{const element=event.target;
    if(element.dataset?.scanShortcutCapture!==undefined){if(event.key==='Tab')return;try{shortcutCaptureError=null;scanShortcuts.bind(element.dataset.scanShortcutCapture,captureScanShortcut(event));}catch(cause){shortcutCaptureError=cause.message||String(cause);}render();return;}
    if(element.disabled||event.defaultPrevented||event.isComposing||event.ctrlKey||event.metaKey||event.altKey)return;
    if(element.hasAttribute?.('data-digits')){
      const arrow=['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key);
      const selectedDigit=/^\d$/.test(event.key)&&element.selectionEnd===element.selectionStart+1&&element.value===formatDigits(Number(element.value),Number(element.dataset.digits),Number(element.dataset.whole));
      if(arrow||selectedDigit){event.preventDefault();try{scanDigitEdit(element,event.key);}catch(cause){error(cause);}}return;
    }
    if(element.id!=='laser-wavelength')return;
    if(event.key==='Enter'){event.preventDefault();if(!event.repeat)content.querySelector('[data-op="laser-goto"]')?.click();return;}
    if(!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key)&&!/^\d$/.test(event.key))return;
    event.preventDefault();try{targetEdit(element,event.key);}catch(cause){error(cause);}
  });
  window.addEventListener('keydown',event=>{
    const record=route()?currentRecord():null;
    scanShortcuts.handleKeydown(event,{isLaser:record?.model_id==='tlb6700',documentFocused:document.hasFocus?.()===true,modalOpen:Boolean(wizard||document.querySelector('[aria-modal="true"]')),findButton:op=>content.querySelector(`[data-op="${op}"]`)});
  });
  content.addEventListener('focusin',event=>{const e=event.target;if(e.id==='laser-wavelength'&&!e.disabled)e.setSelectionRange(e.value.length-1,e.value.length);});
  content.addEventListener('pointerup',event=>{const e=event.target;if(e.disabled||e.selectionEnd-e.selectionStart>1)return;
    if(e.id==='laser-wavelength'||e.hasAttribute('data-digits')){
      const css=getComputedStyle(e),measure=document.createElement('canvas').getContext('2d');measure.font=css.font;
      const advance=measure.measureText('0').width+(parseFloat(css.letterSpacing)||0);
      const offset=event.clientX-e.getBoundingClientRect().left-e.clientLeft-parseFloat(css.paddingLeft)+e.scrollLeft;
      let pos=Math.max(0,Math.min(e.value.length-1,Math.floor(offset/advance)));
      if(e.value[pos]==='.')pos=Math.min(e.value.length-1,pos+1);
      if(/\d/.test(e.value[pos]||''))e.setSelectionRange(pos,pos+1);
    }});
  function normalizeDigitDraft(e){if(!e.hasAttribute('data-digits')||e.value.trim()==='')return;
    const value=Number(e.value),min=Number(e.getAttribute('min')),max=Number(e.getAttribute('max'));if(!Number.isFinite(value)||value<min||value>max)return;
    const formatted=formatDigits(value,Number(e.dataset.digits),Number(e.dataset.whole)),position=e.value===formatted?e.selectionStart:formatted.length-1;
    e.value=formatted;if(document.activeElement===e)e.setSelectionRange(position,position+1);rememberInputDraft(e,false);
  }
  content.addEventListener('focusout',event=>normalizeDigitDraft(event.target));
  content.addEventListener('input',event=>{const field={'remote-endpoint':'endpoint','remote-name':'name','remote-listener':'listener'}[event.target.id];if(field){connections.draft[field]=event.target.value;return;}if(!key()||!event.target.id)return;rememberInputDraft(event.target);});
  content.addEventListener('keydown',event=>{
    if(!key()||event.defaultPrevented||event.isComposing||event.ctrlKey||event.metaKey||event.altKey||event.target.disabled)return;
    const id=event.target.id;
    if(event.key==='Tab'&&['gain-temp','gain-current'].includes(id)){
      const next=content.querySelector(id==='gain-temp'?'#gain-current':'#gain-temp');
      if(next&&!next.disabled){event.preventDefault();next.focus();}return;
    }
    if(event.key!=='Enter'||event.repeat)return;
    const op=id==='gain-temp'?'gain-set-temp':id==='gain-current'?'gain-set-current':['gain-pid-p','gain-pid-i','gain-pid-d'].includes(id)?'gain-set-pid':null;
    if(!op)return;event.preventDefault();if(id==='gain-temp'||id==='gain-current')normalizeDigitDraft(event.target);content.querySelector(`[data-op="${op}"]`)?.click();
  });
  content.addEventListener('keydown',event=>{if(event.key==='Escape'){if(connections.request?.phase==='Waiting')cancelPairRequest().catch(error);else if(connections.add){connections.add=false;render();}return;}
    const tab=event.target.closest('[role="tab"]');if(!tab||!['ArrowLeft','ArrowRight','Home','End','Enter',' '].includes(event.key))return;event.preventDefault();connections.tab=event.key==='Home'?'this':event.key==='End'?'other':['Enter',' '].includes(event.key)?tab.dataset.tab:connections.tab==='this'?'other':'this';render();document.getElementById('connections-tab-'+connections.tab)?.focus();});
  content.addEventListener('click',event=>{const plot=event.target.closest('[data-osa-plot]');if(plot&&key()){const box=plot.getBoundingClientRect();local().cursor=osaCursorIndex(displayedTrace(),plotFraction((event.clientX-box.left)/box.width));render();}});
  window.addEventListener('hashchange',()=>{if(wizard?.busy||wizard?.installing)return;wizard=null;session.navigate(page());render();if(page()==='#settings')loadDriverStatus().catch(error);});
  native.event.listen('host-offline',()=>{connected=false;session.offline();render();});
  native.event.listen('remote-offline',event=>{session.offline(event.payload.host_id);refreshRemote().catch(error);render();});
  native.event.listen('host-close-retained',event=>{notify(event.payload.message+' '+JSON.stringify(event.payload.report||event.payload.error));});
  const heartbeat=setInterval(()=>session.heartbeat().catch(error),2000),clock=setInterval(()=>{for(const h of store.hosts())if(h.connected)calibrate(h.host_id).catch(()=>{});},30000),remoteTimer=setInterval(async()=>{try{await pollPairRequest();if(page()==='#settings')await refreshRemote();}catch(cause){error(cause);}},1500);
  const feedbackTimer=setInterval(()=>{tickActivities(content);const background=document.querySelector('#background-work');if(background)tickActivities(background);},1000);
  const laserPoll=setInterval(()=>{
    const r=route(),h=host(),k=key();if(!r||!h||!k)return;
    const record=h.registry.devices.find(d=>r.domain.kind==='device'&&d.device_id===r.domain.id);
    if(record?.model_id!=='tlb6700')return;
    const domain=store.get(k),l=local();
    if(!laserRefreshDue({visible:document.visibilityState==='visible',connected:h.connected,owned:store.canControl(k),synced:h.connected&&h.synced,
      domain,local:l,ageUpperMs:store.ageUpperMs(r.hostId,domain?.host_sample_ms),interval:record.check_policy?.interval_s||30}))return;
    const taskKey=actionKey({op:'laser-read'});pendingActions.set(taskKey,'Refreshing…');
    run(r.domain,record.config_rev,'action',{name:'read_status',args:{}},r.hostId,true).catch(cause=>{l.laserRefreshFailed=true;error(cause);}).finally(()=>{pendingActions.delete(taskKey);render();});
  },1000);
  window.addEventListener('beforeunload',()=>{closing=true;renderer.cancel();for(const l of Object.values(locals))l.connectionGeneration=(l.connectionGeneration||0)+1;clearInterval(heartbeat);clearInterval(clock);clearInterval(remoteTimer);clearInterval(feedbackTimer);clearInterval(laserPoll);for(const {client}of session.clients())client.disconnect();});
  render();const ready=(async()=>{if(typeof native.core?.invoke==='function')appProfile=await native.core.invoke('app_profile');remoteSettings.networkOnly=appProfile.network_only;preferences=await localClient.preferences();renderedKey=null;if(!appProfile.network_only)try{await connect(true);}catch(cause){error(cause);}await refreshRemote();})().catch(cause=>{hostIssue??={code:cause?.code,message:cause?.message||String(cause)};error(new Error('App startup unavailable: '+(cause?.message||String(cause))+'. Review Settings.'));render();});
  return {render,requestRender:renderer.request,connect,resync,ready,clearNotice};
}
