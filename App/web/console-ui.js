import {formatTarget,editTarget,formatDigits,editDigits,createTargetQueue,syncTarget,laserMotion} from './wavelength-editor.js';
import {createNotice} from './notice.js';
import * as panels from './panels.js';
import {renderDeviceSetup,renderAddWizard,renderSettings,instrumentTarget,signature,setupChoices,driverCheckReady,controllerChoiceReady} from './setup.js';
import {connectLocalHost} from './host-client.js';
import {createSetupActions,refreshDraftConnection} from './setup-actions.js';
import {connectionView,connectionReleased,ensureInstrumentControl} from './connection.js';
import {instanceView,actionFor} from './instance-view.js';
import {deviceKey,routeFor,parseRoute} from './routes.js';
import {renderInstrumentIcon} from './instrument-icon.js';
import {buildPmSettingAction} from './pm400.js';
import {appendTelemetry,osaCursorIndex} from './view-model.js';
import {renderOverview} from './overview.js';
import {createSharedResults} from './shared-results.js';
import {createArchiveHistory,exportSelectedTrace,plotFraction} from './osa.js';
import {replaceMarkup,createRenderScheduler} from './render.js';
import {laserRefreshDue,operatingLimits} from './laser-control.js';
export {replaceMarkup,createRenderScheduler} from './render.js';
export {createSharedResults} from './shared-results.js';
import {pollOriginalOperation,queryOriginalOperation} from './operation-recovery.js';
export {pollOriginalOperation,queryOriginalOperation} from './operation-recovery.js';
const esc=panels.esc;
const driver={aq6370:'osa',voltage:'voltage',gain:'gain',pm400:'pm400',mdt693b:'mdt',tlb6700:'laser'};
export function connectionEffects(record,model){
 if(!model)throw new Error('Trusted connection effects are unavailable.');
 const effects={tlb6700:'',voltage:'Opening will zero all eight channels. Closing immediately zeros them again.',gain:'Opening may trigger interlock shutdown. Closing disables current before TEC.',aq6370:'',pm400:'',mdt693b:'Outputs are held and motion remains disarmed.','fiber-coupling':'Piezo outputs are held; baseline adoption is separate.'};
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
export function snapshotBarrier(){let latest=null,minimum=null,resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no});
  const check=()=>{if(minimum!==null&&latest?.seq>=minimum)resolve(latest);};
  return {promise,reject,receive(event){latest=event;check();},expect(seq){minimum=seq;check();}};}
export function currentResult(intent,reply,store,key){return Boolean(store.lease(key)?.control_epoch===intent.control_epoch&&JSON.stringify(store.get(key)?.context)===JSON.stringify(reply.context));}
export function updatedHostSettings(previous,hostName,pythonPath,dataRoot){return {...previous,host_name:hostName,python_path:pythonPath,...(dataRoot?{data_root:dataRoot}:{})};}
export function renderConsole(page,host,store,local={},catalog={models:[]},preferences=null){
  if(page==='settings'||page==='host')return renderSettings(host,preferences);
  if(page==='devices')return renderDeviceSetup(host||{registry:{devices:[],drafts:[],setups:[]}},catalog,local['#devices']);
  const route=parseRoute(page.startsWith('#')?page:'#'+page);
  if(route){const record=route.hostId!==(host?.host_id||host?.hostId)?null:route.domain.kind==='setup'?host?.registry?.setups.find(r=>r.setup_id===route.domain.id):host?.registry?.devices.find(r=>r.device_id===route.domain.id);
    if(!record)return '<h1>Instrument unavailable</h1><p>Its Host or configuration is not available.</p>';
    const key=deviceKey(route.hostId,route.domain),value=store.get(key),kind=route.domain.kind==='setup'?'fiber':driver[record.model_id];
    const own=store.canControl(key),age=kind==='laser'&&(!host.connected||!host.synced)?null:store.ageUpperMs(route.hostId,value?.host_sample_ms),view=instanceView(kind,value,own,age,{...local[key],mode:host.mode});
    view.hideConnectionAction=true;
    if(kind==='laser')view.laserRefreshInterval=record.check_policy?.interval_s||30;
    if(kind==='fiber'){view.baselineConsent={};for(const side of ['left','right']){const stage=value?.device?.[side],binding=baselineBinding(host,value,own?store.lease(key):null,stage),consent=local[key]?.baselineConsent?.[side];view.baselineConsent[side]=consent?.binding===binding?consent:{};}}

    const iconModel=route.domain.kind==='setup'?'fiber-coupling':record.model_id,connection=connectionView(host,store,key,local[key]);
    const header=`<div class="instance-bar"><div class="instrument-identity">${renderInstrumentIcon(iconModel)}<strong>${esc(record.name)}</strong></div><span class="badge">${host.remote?'REMOTE':'LOCAL'}</span><div class="instance-connection"><span class="badge">${esc(connection.status)}</span><button class="btn ${connection.operation==='connect'?'primary':'warn'}" data-op="${connection.operation}" data-role="${kind}"${connection.disabled?' disabled':''}>${esc(connection.label)}</button></div></div>`;
    const model=route.domain.kind==='setup'?{id:'fiber-coupling'}:catalog.models?.find(m=>m.id===record.model_id);
    const effect=model?connectionEffects(record,model):'';
    const lifecycle=effect?`<p class="warning">${esc(effect)}</p>`:'';
    const recovery=view.unknown&&view.operationAttempt&&!local[key]?.disconnecting&&!local[key]?.disconnectInFlight?`<div class="alert">Operation result is uncertain. Check its status or disconnect.<button class="btn" data-ui="query-original" ${view.pending?'disabled':''}>Check status</button></div>`:'';
    const history=store.history(key).filter(e=>e.type==='operation').slice(-16).map(e=>({operation_id:e.data?.operation_id,phase:e.data?.phase,context:e.data?.result?.context}));
    const evidence='<details id="instrument-diagnostics"><summary>Diagnostics</summary><pre>'+esc(JSON.stringify({identity:record.expected_identity,context:value?.context,control:host.control?.[route.domain.kind+':'+route.domain.id],safety:value?.safety,operation:view.operationAttempt,outcome:view.lastOperation,latest_result:view.resultOperationId,result_error:view.sharedResultError,history},null,2))+'</pre></details>';
    const resultNote=view.sharedResultError?'<p class="alert">Spectrum unavailable. See Diagnostics for details.</p>':'';
    if(kind==='mdt')return header+'<h1>MDT693B controller</h1><p>Configure a Fiber setup to use laboratory coordinates.</p><a class="btn" href="#devices">Device setup</a>'+evidence;
    return header+lifecycle+recovery+resultNote+(panels[kind]?.(view)||'<p>Driver required</p>')+evidence;
  }
  return renderOverview(host,store);
}
export function mountConsole(session,native){
  const client=session.client,store=session.store,content=document.querySelector('#content'),locals={},catalog={models:[],categories:[]};
  let wizard=null,connected=false,busyHost=false,resyncing=null,renderedKey=null,lastMarkup=null,snapshotWaiter=null,unlisten=null,preferences=null,driverInventory=null,deviceEditor={},driverRefresh=null;const pendingActions=new Map();
  const host=()=>store.host(session.hostId),page=()=>location.hash||'#overview';
  const route=()=>parseRoute(page()),key=()=>route()?deviceKey(route().hostId,route().domain):null;
  const shared=createSharedResults(store,client,k=>{if(locals[k])locals[k].cursor=null;render();});
  const archiveHistory=createArchiveHistory(client,()=>render());
  const archiveScope=()=>{const r=route(),h=host();return connected&&h?.connected&&h.synced&&r?.hostId===session.hostId&&r.domain.kind==='device'
    &&h.registry.devices.some(d=>d.device_id===r.domain.id&&d.model_id==='aq6370')?{hostId:r.hostId,domain:r.domain}:null;};
  const displayedTrace=()=>{const state=archiveHistory.state(archiveScope());return state.historical?state.trace:shared.current(key()).trace;};
  const local=()=>{const k=key();if(!k)throw new Error('Select an instrument');return locals[k]||(locals[k]={});};
  const notice=createNotice(document.querySelector('#notice'));
  const notify=(message,isError=false)=>notice.show(message,isError),clearNotice=()=>notice.hide();
  function error(cause){notify(cause?.message||String(cause),true);}
  function restoreInputs(saved){for(const element of content.querySelectorAll('input,select,details[id]')){if(element.dataset.managed)continue;const id=element.id||element.dataset.draft||'param:'+element.dataset.param;if(saved.has(id)){const item=saved.get(id);if(item&&typeof item==='object'){if('value' in item)element.value=item.value;if('checked' in item)element.checked=item.checked;if('open' in item)element.open=item.open;}else element.value=item;}}}
  const renderer=createRenderScheduler(paint);
  function render(){renderer.flush();}
  function paint(){const h=host(),p=page(),nextKey=key()||p;const live=inputSnapshot(content.querySelectorAll('input,select,details[id]'));
    for(const [k,l]of Object.entries(locals))if(!l.disconnectInFlight&&l.disconnectAccepted&&h?.bootId===l.disconnectEvidenceBoot&&h?.seq>l.disconnectEvidenceSeq&&connectionReleased(h,store.get(k))){l.disconnecting=null;l.disconnectFailed=false;l.unknown=false;l.disconnectAccepted=false;delete l.operationAttempt;}
    for(const [k,l]of Object.entries(locals))if(l.baselineConsent){const domain=store.get(k);for(const side of ['left','right'])if(l.baselineConsent[side]?.binding!==baselineBinding(h,domain,store.canControl(k)?store.lease(k):null,domain?.device?.[side]))delete l.baselineConsent[side];}
    for(const [k,l] of Object.entries(locals))syncTarget(l,laserMotion(store.get(k)?.device));
    shared.refresh(key());archiveHistory.select(archiveScope());const viewLocals={...locals};if(key())viewLocals[key()]={...locals[key()],...shared.current(key()),...archiveHistory.state(archiveScope()),recordingRoot:h?.archive?.active_root,archiveAvailable:h?.archive?.available};
    const samePage=renderedKey===nextKey,saved=inputValues(renderedKey,nextKey,live,locals[nextKey]?.inputs||new Map());
    const active=document.activeElement?.id||document.activeElement?.dataset?.draft||document.activeElement?.dataset?.param;const selection=[document.activeElement?.selectionStart,document.activeElement?.selectionEnd];const editing=Boolean(document.activeElement?.closest('#content')&&document.activeElement?.matches('input,select'));
    const navigation=document.querySelector('#navigation'),links='<a href="#overview">Overview</a><a href="#devices">Device setup</a><a href="#settings">Settings</a>';
    if(navigation.innerHTML!==links)navigation.innerHTML=links;
    document.querySelector('#sidebar-mode').textContent='Local Host';document.querySelector('#sidebar-subtitle').textContent=busyHost?'Connecting…':connected?'Connected':'Disconnected';
    document.querySelector('#session-pill').textContent=(busyHost?'CONNECTING':connected?'ONLINE':'OFFLINE');
    document.querySelector('#current-page-title').textContent=p==='#devices'?'Device setup':['#host','#settings'].includes(p)?'Settings':route()?'Instrument':'Overview';
    let markup=renderConsole(p==='#devices'?'devices':['#host','#settings'].includes(p)?'settings':p,{...h,driverInventory},store,{...viewLocals,'#devices':deviceEditor},catalog,preferences);
    if(wizard)markup+=renderAddWizard(wizard,catalog);
    markup=markup.replace(/<button\b([^>]*)>([\s\S]*?)<\/button>/g,(whole,attrs,label)=>{const data={};for(const match of attrs.matchAll(/data-(ui|op|device|setup|side|channel|setting|command)="([^"]*)"/g))data[match[1]]=match[2];const text=pendingActions.get(actionKey(data));return text?`<button ${attrs} ${/\bdisabled\b/.test(attrs)?'':'disabled'} aria-busy="true">${esc(text)}</button>`:whole;});
    const replaced=replaceMarkup(content,samePage?lastMarkup:null,markup);lastMarkup=markup;
    if(replaced){restoreInputs(saved);if(editing&&samePage&&active){const element=[...content.querySelectorAll('input,select')].find(e=>(e.id||e.dataset.draft||e.dataset.param)===active);element?.focus();if(selection[0]!==null&&selection[0]!==undefined&&element?.setSelectionRange)element.setSelectionRange(...selection);}}renderedKey=nextKey;
    if(route()&&h){const domain=store.get(key()),r=route().domain,k=r.kind==='setup'?'fiber':driver[h?.registry.devices.find(d=>d.device_id===r.id)?.model_id];
      const state=instanceView(k,domain,store.canControl(key()),store.ageUpperMs(session.hostId,domain?.host_sample_ms),{...local(),...shared.current(key()),mode:h.mode});
      if(k==='gain')local().gainHistory=appendTelemetry(local().gainHistory||[],state.status.devices[k],k);
      if(k==='voltage')local().voltageHistory=appendTelemetry(local().voltageHistory||[],state.status.devices[k],k);
    }
  }
  async function calibrate(){const id=session.hostId,boot=store.host(id)?.bootId,start=performance.now(),ping=await client.ping(),end=performance.now();
    if(id!==session.hostId||boot!==store.host(id)?.bootId)return;
    store.setClock(id,ping.monotonic_ms,start,end);
    session.clientIdentity=ping.boot_id===boot&&/^[0-9a-f]{32}$/.test(ping.client_session_id)?{hostId:id,bootId:boot,sessionId:ping.client_session_id}:null;
  }
  async function resync(){if(resyncing)return resyncing;resyncing=(async()=>{
    snapshotWaiter=snapshotBarrier();const received=snapshotWaiter.promise;
    const timer=setTimeout(()=>snapshotWaiter?.reject(new Error('Snapshot unavailable. Controls remain disarmed.')),10000);
    try {const receipt=await client.requestSnapshot();snapshotWaiter.expect(receipt.seq);await received;await calibrate();} finally{clearTimeout(timer);snapshotWaiter=null;}
  })().finally(()=>{resyncing=null;});return resyncing;}
  function loadDriverStatus(){
    if(!connected)return Promise.resolve();
    if(driverRefresh)return driverRefresh;
    const boot=host()?.bootId;driverInventory={...driverInventory,checking:true};render();
    driverRefresh=(async()=>{try{const inventory=await client.driverStatus();if(connected&&host()?.bootId===boot)driverInventory=inventory;}
      catch(cause){if(connected&&host()?.bootId===boot)driverInventory={errors:{newport:cause.message||String(cause)}};throw cause;}
      finally{driverRefresh=null;if(driverInventory?.checking)driverInventory.checking=false;render();}})();
    return driverRefresh;
  }
  async function connect(automatic=false){if(busyHost||connected)return;busyHost=true;render();try{if(automatic){if(!preferences)throw new Error('Review and save local Host settings before starting.');await connectLocalHost(client,preferences);}else await client.connect();Object.assign(catalog,await client.catalog());
      unlisten=await client.subscribe(event=>{const applied=session.apply(event);if(event.type==='snapshot')snapshotWaiter?.receive(event);if(applied.requiresSnapshot)resync().catch(error);});await resync();
      connected=true;if(page()==='#settings')loadDriverStatus().catch(error);
    }catch(cause){unlisten?.();unlisten=null;try{await client.disconnect();}catch(cleanup){error(cleanup);}connected=false;session.offline();throw cause;
    }finally{busyHost=false;render();}}
  async function stopDomain(domain){const k=deviceKey(session.hostId,domain),l=locals[k]||(locals[k]={});
    l.targetQueue?.cancel();delete l.targetQueue;
    if(l.disconnectInFlight)throw new Error('Disconnect is already in progress.');
    const lease=store.lease(k);if(lease){l.disconnectOwner=lease.session_id;l.disconnectEpoch=lease.control_epoch;l.disconnectBoot=lease.boot_id;}
    l.disconnectEvidenceBoot=host()?.bootId;l.disconnectEvidenceSeq=host()?.seq;l.disconnectAccepted=false;l.disconnectInFlight=true;
    l.connectionGeneration=(l.connectionGeneration||0)+1;l.disconnecting=performance.now();l.unknown=true;store.dropLease(k);render();
    try{const report=await client.safeStop(domain);
      if(report?.accepted!==true)throw new Error('Disconnect acceptance is unconfirmed.');
      // A coalesced snapshot requested before stop acceptance cannot prove release.
      if(resyncing)await resyncing;
      if(host()?.bootId!==l.disconnectEvidenceBoot)throw new Error('Host changed during disconnect. Release is unconfirmed.');
      l.disconnectEvidenceSeq=host()?.seq;l.disconnectAccepted=true;await resync();return report;}
    catch(cause){l.disconnecting=null;l.disconnectFailed=true;throw cause;}
    finally{l.disconnectInFlight=false;render();}}
  async function run(domain,rev,method,params){
    const k=deviceKey(session.hostId,domain),l=locals[k]||(locals[k]={});
    if(l.pending||l.connecting)throw new Error('An operation is already in progress.');
    if(method==='connect'){const view=connectionView(host(),store,k,l);if(view.operation!=='connect'||view.disabled)throw new Error('Connection is unavailable. Check status or disconnect first.');}
    if(l.unknown&&method!=='connect')throw new Error('Check the operation status or disconnect before continuing.');
    const registry=host().registry,record=domain.kind==='setup'?registry.setups.find(s=>s.setup_id===domain.id):[...registry.devices,...registry.drafts].find(d=>d.device_id===domain.id);
    const model=domain.kind==='setup'?{id:'fiber-coupling'}:catalog.models.find(m=>m.id===record?.model_id);
    if(method==='connect'&&!model)throw new Error('Trusted connection effects are unavailable. No connection was made.');
    let acquired=false;
    if(method==='connect'){
      l.targetQueue?.cancel();delete l.targetQueue;delete l.targetValue;delete l.observedTarget;l.targetSending=false;l.laserScanning=false;
      const generation=l.connectionGeneration||0;l.connecting=true;render();
      const identity=session.clientIdentity;
      if(identity?.hostId===session.hostId&&identity.bootId===host()?.bootId){l.disconnectOwner=identity.sessionId;l.disconnectBoot=identity.bootId;l.disconnectEpoch=host()?.control?.[domain.kind+':'+domain.id]?.control_epoch;}
      try{({acquired}=await ensureInstrumentControl(session,domain,resync,()=>generation===(l.connectionGeneration||0)));l.unknown=false;}
      catch(cause){if(cause.outcomeUnknown||cause.cleanupError)l.unknown=true;throw cause;}
      finally{l.connecting=false;render();}
    }
    const snapshot=store.get(k),lease=store.lease(k);
    if(!store.canControl(k)||!lease||!snapshot?.context)throw new Error('Instrument is in use or connection authority is unavailable.');
    const intent={domain,lease_token:lease.token,control_epoch:lease.control_epoch,config_rev:rev,context:snapshot.context,method,params,sequence:client.nextSequence(),confirmation:null};
    const requestId=crypto.randomUUID().replaceAll('-','');l.pending=requestId;render();
    l.operationAttempt={request_id:requestId,domain,boot_id:lease.boot_id,context:intent.context};
    try{const proof=await client.prepare(intent);intent.confirmation=proof.token;
      let record=await client.execute(requestId,intent);record=await pollOriginalOperation(client,requestId,record);
      l.lastOperation=record;
      if(record.status==='Outcome Unknown'||record.phase==='timed_out_unknown'){l.unknown=true;return record;}
      if(record.phase!=='completed')throw new Error(record.result?.error?.message||record.result?.error||record.phase);
      if(method==='connect'||method==='action'&&params.name==='read_status')delete l.laserRefreshFailed;
      await resync();
      if(!currentResult(intent,record.result,store,k))return record;
      if(key()===k)await shared.refresh(k);return record;
    }catch(cause){if(cause.outcomeUnknown||cause.code==='AdmissionPending'){l.unknown=true;l.unknownRequestId=requestId;}
      else if(acquired&&!store.get(k)?.context?.connection_id){try{await client.release(domain,lease);store.dropLease(k);await resync();}catch(cleanup){l.unknown=true;cause.cleanupError=cleanup;}}
      throw cause;
    }finally{l.pending=null;if(!l.unknown)delete l.operationAttempt;render();}
  }
  async function runLaserAction(domain,rev,action){
    const k=deviceKey(session.hostId,domain),boot=host()?.bootId,context=JSON.stringify(store.get(k)?.context);
    const record=await run(domain,rev,'action',action);
    // ACK proves acceptance, while this separate read observes motion. An
    // unknown ACK is never repaired by reading or replaying a setter.
    if(record.phase==='completed'&&host()?.bootId===boot&&key()===k&&store.canControl(k)&&
       !locals[k]?.unknown&&JSON.stringify(store.get(k)?.context)===context&&
       ['set_target_wavelength','control_piezo','start_scan','stop_scan'].includes(action.name))
      await run(domain,rev,'action',{name:'read_motion',args:{}});
    return record;
  }
  const setup=createSetupActions(session,null,resync,run);
  async function checkWizardDrivers(){if(!wizard)return;const draft=wizard,model=catalog.models.find(m=>m.id===draft.modelId),profile=model?.profiles.find(p=>p.id===draft.profileId);
    return refreshDraftConnection(draft,model,profile,()=>client.driverStatus(),()=>client.scanLasers(),()=>{if(wizard===draft)render();});
  }
  async function releaseDraft(d){if(!d.record)return;const domain={kind:'device',id:d.record.device_id},k=deviceKey(session.hostId,domain);
    if(store.lease(k)){await stopDomain(domain);const deadline=performance.now()+10000;while(!connectionReleased(host(),store.get(k))){if(performance.now()>deadline)throw new Error('Connection release is unconfirmed. Retry Cancel.');await new Promise(r=>setTimeout(r,150));await resync();}}
  }
  async function installWizardDriver(fromSettings=false){const draft=fromSettings?(driverInventory||={}):wizard;if(!draft||draft.busy||draft.installing)return;draft.installing=true;draft.proof=null;draft.installError=null;render();
    try{let job=await client.installDriver();const deadline=performance.now()+20*60*1000;
      while(job.state==='running'){if(performance.now()>deadline)throw new Error('Driver installation is still running. Check Settings before retrying.');await new Promise(r=>setTimeout(r,1000));job=await client.driverInstallStatus();}
      if(job.state!=='completed')throw new Error(job.message||'Driver installation failed.');
      if(job.restart_required)throw new Error('Driver installed. Restart Windows, then check again.');
      driverInventory=await client.driverStatus();if(wizard===draft)await checkWizardDrivers();
    }catch(cause){draft.installError=cause.message||String(cause);throw cause;}finally{draft.installing=false;render();}
  }
  function currentRecord(){const r=route();return r.domain.kind==='setup'?host().registry.setups.find(s=>s.setup_id===r.domain.id):host().registry.devices.find(d=>d.device_id===r.domain.id);}
  const get=id=>document.getElementById(id)?.value??'';
  const actionKey=data=>JSON.stringify([key()||page(),...['ui','op','device','setup','side','channel','setting','command'].map(k=>data[k]||'')]);
  function progressLabel(data){return ({'connect':'Connecting…','disconnect':'Disconnecting…','osa-read':'Reading trace…','osa-acquire':'Acquiring…','osa-export':'Exporting…','test-draft':'Testing connection…','save-draft':'Saving…','prepare-draft':'Connecting…','cancel-wizard':'Closing…','cancel-draft':'Closing…','refresh-device':'Refreshing…','scan-controllers':'Scanning…','check-draft-drivers':'Checking…','check-drivers':'Checking…','save-checks':'Saving…','save-rename':'Saving…','save-host':'Saving…','retire':'Removing…','retire-setup':'Removing…','save-setup':'Saving…','stop-host':'Stopping Host…','connect-host':'Connecting…','start-host':'Starting Host…','install-draft-driver':'Installing…','install-driver':'Installing…','query-original':'Checking status…','laser-read':'Refreshing…','laser-scan-start':'Starting scan…','laser-scan-stop':'Stopping scan…','laser-wavelength':'Tuning…','laser-piezo':'Adjusting…','save-laser-limits':'Saving & reconnecting…','pm-measure':'Reading…'}[data.ui||data.op]||'Working…');}
  async function uiAction(button){const name=button.dataset.ui;
    if(name==='save-laser-limits'){
      const r=route(),k=key(),l=local(),record=currentRecord(),domain=store.get(k);
      if(record.model_id!=='tlb6700'||!store.canControl(k)||l.pending||l.limitsSaving)throw new Error('Laser controls are unavailable.');
      const limits=operatingLimits(get,domain?.device);
      l.limitsSaving=true;render();
      try{
        await stopDomain(r.domain);
        const deadline=performance.now()+15000;
        while(!connectionReleased(host(),store.get(k))){
          if(performance.now()>deadline)throw new Error('Disconnect is still pending. Limits were not saved.');
          await new Promise(yes=>setTimeout(yes,150));await resync();
        }
        const latest=host().registry.devices.find(d=>d.device_id===record.device_id);
        await client.saveLaserLimits({device_id:latest.device_id,config_rev:latest.config_rev,expected_rev:host().registry.registry_rev,limits});
        await resync();
        for(const id of ['laser-scan-start','laser-scan-stop','laser-scan-speed','laser-scan-return-speed','laser-wavelength'])l.inputs?.delete(id);
        delete l.targetValue;delete l.observedTarget;renderedKey=null;
        const saved=host().registry.devices.find(d=>d.device_id===record.device_id);
        await run(r.domain,saved.config_rev,'connect',{acknowledge_lifecycle:true});
      }finally{l.limitsSaving=false;render();}
      return;
    }
    if(name==='check-drivers')return loadDriverStatus();
    if(name==='check-draft-drivers'||name==='scan-controllers')return checkWizardDrivers();
    if(name==='install-draft-driver')return installWizardDriver();
    if(name==='install-driver')return installWizardDriver(true);
    if(name==='osa-history-refresh'||name==='osa-history-more')return archiveHistory.list(name==='osa-history-more');
    if(name==='osa-history-load'){local().cursor=null;delete local().exportDirectory;return archiveHistory.load(button.dataset.archive,button.dataset.name);}
    if(name==='osa-current'){local().cursor=null;archiveHistory.showCurrent();return;}
    if(busyHost)throw new Error('Local Host connection is in progress.');
    if(name==='connect-host')return connect(true);
    if(name==='disconnect-host'){await client.disconnect();unlisten?.();unlisten=null;connected=false;session.offline();return;}
    if(name==='start-host'){await client.startHost({pythonPath:get('host-python')});return connect();}
    if(name==='stop-host'){const report=await client.stopHost();if(!report.resource_released||report.process_exit?.confirmed!==true||report.process_exit?.success!==true)throw new Error('Host stop is retained. Keep this window open.');connected=false;await client.disconnect();session.offline();return;}
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
    if(name==='test-draft'||name==='prepare-draft'||name==='save-draft'){if(!wizard)throw new Error('Open a draft first');if(wizard.busy||wizard.installing)return;clearNotice();const model=catalog.models.find(m=>m.id===wizard.modelId),profile=model?.profiles.find(p=>p.id===wizard.profileId);wizard.busy=true;render();try{
      if(name==='test-draft'){if(!driverCheckReady(wizard,profile)||(model?.id==='tlb6700'&&!controllerChoiceReady(wizard)))await checkWizardDrivers();await setup.test(wizard,model,profile);}else if(name==='prepare-draft')await setup.prepare(wizard,model);else{const saved=await setup.save(wizard);wizard=null;location.hash=routeFor(session.hostId,{kind:'device',id:saved.device_id});}
    }catch(cause){if(name==='test-draft'){wizard.proof=null;throw new Error('Connection failed: '+(cause.message||String(cause)));}throw cause;}finally{if(wizard){wizard.busy=false;if(wizard.cancelRequested){wizard.cancelRequested=false;await uiAction({dataset:{ui:'cancel-wizard'}});}}render();}return;}
    if(name==='rename'){deviceEditor={rename:button.dataset.device};renderedKey=null;render();return;}
    if(name==='cancel-editor'){deviceEditor={};renderedKey=null;render();return;}
    if(name==='save-rename'||name==='retire'){const d=host().registry.devices.find(d=>d.device_id===button.dataset.device),params={device_id:d.device_id,config_rev:d.config_rev,expected_rev:host().registry.registry_rev};
      if(name==='save-rename'){params.name=get('rename-name').trim();if(!params.name)throw new Error('Enter an instrument name.');await client.renameDevice(params);deviceEditor={};}else await client.retireDevice(params);return resync();}
    if(name==='add-setup'){if(!setupChoices(host().registry).length)throw new Error('Register a controller with one of the two known serial identities first.');deviceEditor={addSetup:true};renderedKey=null;render();return;}
    if(name==='save-setup'){const members=setupChoices(host().registry).filter(d=>document.getElementById('setup-member-'+d.device_id)?.checked).map(d=>d.device_id),name=get('setup-name').trim();if(!members.length||!name)throw new Error('Enter a name and select a controller.');await client.saveSetup({name,members,expected_rev:host().registry.registry_rev});deviceEditor={};return resync();}
    if(name==='retire-setup'){const s=host().registry.setups.find(s=>s.setup_id===button.dataset.setup);const report=await client.retireSetup({setup_id:s.setup_id,config_rev:s.config_rev,expected_rev:host().registry.registry_rev});if(report.restart_required)notify('Setup removed. Restart Host before reassigning its controllers.');return resync();}
    if(name==='safe-stop')return stopDomain(route().domain);
    if(name==='query-original'){
      const k=key(),l=local();const result=await queryOriginalOperation(client,l.operationAttempt,resync,()=>({boot_id:host()?.bootId,context:store.get(k)?.context,synced:host()?.connected&&host()?.synced}));
      l.lastOperation=result.record;l.unknown=!result.resolved;
      if(!result.resolved)notify('Operation status remains uncertain. Disconnect or check again.');
      if(result.resolved&&key()===k)await shared.refresh(k);render();return;
    }
  }
  content.addEventListener('click',event=>{if(wizard&&event.target.closest('[data-wizard-backdrop]')){uiAction({dataset:{ui:'cancel-wizard'}}).catch(error);return;}const button=event.target.closest('button');if(!button||button.disabled)return;
    const taskKey=actionKey(button.dataset);if(pendingActions.has(taskKey))return;pendingActions.set(taskKey,progressLabel(button.dataset));render();
    (async()=>{if(button.dataset.ui)return uiAction(button);
      if(button.dataset.pmTab){local().pmTab=button.dataset.pmTab;render();return;}
      const op=button.dataset.op;if(!op)return;if(op==='disconnect'){
        if(connectionView(host(),store,key(),local()).disabled)throw new Error('This instrument is in use by another window.');
        return stopDomain(route().domain);}
      const record=currentRecord();if(op==='connect')return run(route().domain,record.config_rev,'connect',{acknowledge_lifecycle:true});
      if(op==='resume')return run(route().domain,record.config_rev,'resume',{confirm:true});
      if(op==='osa-export'){const trace=displayedTrace(),scope=archiveScope(),l=local();if(!trace||l.exporting)return;if(!scope)throw new Error('Reconnect the owning Host to export this capture.');l.exporting=true;render();
        try{const receipt=await exportSelectedTrace(client,trace,scope);if(receipt){l.exportDirectory=receipt.directory;notify('Capture exported: '+receipt.directory);}}
        finally{l.exporting=false;render();}return;}
      let action;
      if(op==='pm-measure')action={name:'measure_kind',args:{kind:get('pm-kind')}};
      else if(op==='pm-read'||op==='pm-write'){const setting=store.get(key()).device.catalog.settings.find(s=>s.key===button.dataset.setting);
        const payload=buildPmSettingAction(setting,op==='pm-read'?'read':'write',{raw:get(`pm-value-${setting.key}`),group:get(`pm-group-${setting.key}`),selector:get(`pm-selector-${setting.key}`),confirmed:op==='pm-write'});
        delete payload.role;const name=payload.name==='read'?'read_setting':'write_setting';delete payload.name;action={name,args:payload};}
      else if(op==='pm-command')action={name:'run_maintenance',args:{command:button.dataset.command,confirm:true}};
      else {const data={...button.dataset};if(op==='fiber-adopt'){const side=data.side,domain=store.get(key()),stage=domain?.device?.[side],binding=baselineBinding(host(),domain,store.canControl(key())?store.lease(key()):null,stage);if(!baselineAuthorized(local().baselineConsent?.[side],binding))throw new Error('Review and check both baseline attestations first.');delete local().baselineConsent[side];}action=actionFor(op,get,data);}
      if(op==='osa-read'||op==='osa-acquire'){local().cursor=null;delete local().exportDirectory;archiveHistory.showCurrent();}
      if(op==='laser-scan-stop'){local().laserScanning=true;}
      if(op==='laser-scan-start'){const l=local();l.targetQueue?.cancel();l.laserScanning=true;l.scanStarting=true;return runLaserAction(route().domain,record.config_rev,action).finally(()=>{l.scanStarting=false;render();});}
      return op.startsWith('laser-')?runLaserAction(route().domain,record.config_rev,action):run(route().domain,record.config_rev,'action',action);
    })().catch(error).finally(()=>{pendingActions.delete(taskKey);render();});
  });
  content.addEventListener('change',event=>{const element=event.target;
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
    }else if(element.dataset.param){const f=catalog.models.find(m=>m.id===wizard.modelId).profiles.find(p=>p.id===wizard.profileId).fields[element.dataset.param];wizard.params[element.dataset.param]=['number','integer'].includes(f.kind)?Number(element.value):element.value;}
    wizard.proof=null;render();if(['category','modelId','profileId'].includes(element.dataset.draft))checkWizardDrivers().catch(error);
  });
  function targetEdit(element,eventKey){
    const k=key(),r=route(),l=local(),domain=store.get(k),device=domain?.device;
    const edited=editTarget(element.value,element.selectionStart,eventKey,device?.operating_range_nm||device?.wavelength_range_nm);
    if(!edited)return;
    element.setSelectionRange(edited.position,edited.position+1);
    if(eventKey==='ArrowLeft'||eventKey==='ArrowRight')return;
    const context=JSON.stringify(domain.context),boot=host().bootId,token=store.lease(k)?.token;
    l.targetValue=edited.value;element.value=formatTarget(edited.value);element.setSelectionRange(edited.position,edited.position+1);
    if(!l.targetQueue){
      const isCurrent=()=>key()===k&&host()?.bootId===boot&&store.canControl(k)&&store.lease(k)?.token===token&&JSON.stringify(store.get(k)?.context)===context&&!l.unknown&&!l.laserScanning&&!l.limitsSaving;
      const queue=createTargetQueue({isCurrent,onBusy:busy=>{if(l.targetQueue!==queue)return;l.targetSending=busy;if(!busy)delete l.targetQueue;render();},onError:error,
        waitReady:async()=>{const deadline=performance.now()+30000;while(isCurrent()&&laserMotion(store.get(k)?.device).operation_complete!==true){
          if(performance.now()>deadline)throw Error('Motor is still busy. Latest target was not sent.');
          await run(r.domain,currentRecord().config_rev,'action',{name:'read_motion',args:{}});
        }},send:value=>runLaserAction(r.domain,currentRecord().config_rev,{name:'set_target_wavelength',args:{wavelength_nm:value,confirm:true}})});l.targetQueue=queue;
    }
    l.targetSending=true;l.targetQueue.submit(edited.value);render();
  }
  function scanDigitEdit(element,key){
    const precision=Number(element.dataset.digits),whole=Number(element.dataset.whole),range=[Number(element.getAttribute('min')),Number(element.getAttribute('max'))];
    if(element.value.trim()===''||!Number.isFinite(Number(element.value)))return;
    const text=formatDigits(Number(element.value),precision,whole);
    const position=text===element.value&&element.selectionEnd===element.selectionStart+1?element.selectionStart:text.length-1;
    const edited=editDigits(text,position,key,range,precision,whole);
    if(!edited)return;
    element.value=formatDigits(edited.value,precision,whole);element.setSelectionRange(edited.position,edited.position+1);
    const l=local();l.inputs??=new Map();l.inputs.set(element.id,element.value);
  }
  content.addEventListener('keydown',event=>{const element=event.target;if(element.disabled||event.ctrlKey||event.metaKey||event.altKey)return;
    if(element.hasAttribute('data-digits')){
      const arrow=['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key);
      const selectedDigit=/^\d$/.test(event.key)&&element.selectionEnd===element.selectionStart+1&&element.value===formatDigits(Number(element.value),Number(element.dataset.digits),Number(element.dataset.whole));
      if(arrow||selectedDigit){event.preventDefault();try{scanDigitEdit(element,event.key);}catch(cause){error(cause);}}return;
    }
    if(element.id!=='laser-wavelength')return;
    if(!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key)&&!/^\d$/.test(event.key))return;
    event.preventDefault();try{targetEdit(element,event.key);}catch(cause){error(cause);}
  });
  content.addEventListener('focusin',event=>{const e=event.target;if(e.id==='laser-wavelength'&&!e.disabled)e.setSelectionRange(7,8);});
  content.addEventListener('pointerup',event=>{const e=event.target;if(e.disabled||e.selectionEnd-e.selectionStart>1)return;
    if(e.id==='laser-wavelength'||e.hasAttribute('data-digits')){const dot=e.value.indexOf('.'),last=e.value.length-1,pos=e.selectionStart===dot?dot+1:Math.min(last,e.selectionStart??last);if(pos>=0)e.setSelectionRange(pos,pos+1);}});
  content.addEventListener('focusout',event=>{const e=event.target;if(!e.hasAttribute('data-digits')||e.value.trim()==='')return;
    const value=Number(e.value),min=Number(e.getAttribute('min')),max=Number(e.getAttribute('max'));if(!Number.isFinite(value)||value<min||value>max)return;
    e.value=formatDigits(value,Number(e.dataset.digits),Number(e.dataset.whole));const l=local();l.inputs??=new Map();l.inputs.set(e.id,e.value);
  });
  content.addEventListener('input',event=>{if(!key()||!event.target.id)return;const l=local();l.inputs??=new Map();l.inputs.set(event.target.id,event.target.value);});
  content.addEventListener('click',event=>{const plot=event.target.closest('[data-osa-plot]');if(plot&&key()){const box=plot.getBoundingClientRect();local().cursor=osaCursorIndex(displayedTrace(),plotFraction((event.clientX-box.left)/box.width));render();}});
  window.addEventListener('hashchange',()=>{if(wizard?.busy||wizard?.installing)return;wizard=null;session.navigate(page());render();if(page()==='#settings')loadDriverStatus().catch(error);});
  native.event.listen('host-offline',()=>{connected=false;session.offline();render();});
  native.event.listen('host-close-retained',event=>{notify(event.payload.message+' '+JSON.stringify(event.payload.report||event.payload.error));});
  const heartbeat=setInterval(()=>session.heartbeat().catch(error),2000),clock=setInterval(()=>{if(connected)calibrate().catch(()=>{});},30000);
  const laserPoll=setInterval(()=>{
    const r=route(),h=host(),k=key();if(!r||!h||!k)return;
    const record=h.registry.devices.find(d=>r.domain.kind==='device'&&d.device_id===r.domain.id);
    if(record?.model_id!=='tlb6700')return;
    const domain=store.get(k),l=local();
    if(!laserRefreshDue({visible:document.visibilityState==='visible',connected,owned:store.canControl(k),synced:h.connected&&h.synced,
      domain,local:l,ageUpperMs:store.ageUpperMs(session.hostId,domain?.host_sample_ms),interval:record.check_policy?.interval_s||30}))return;
    const taskKey=actionKey({op:'laser-read'});pendingActions.set(taskKey,'Refreshing…');
    run(r.domain,record.config_rev,'action',{name:'read_status',args:{}}).catch(cause=>{l.laserRefreshFailed=true;error(cause);}).finally(()=>{pendingActions.delete(taskKey);render();});
  },1000);
  window.addEventListener('beforeunload',()=>{renderer.cancel();for(const l of Object.values(locals))l.connectionGeneration=(l.connectionGeneration||0)+1;clearInterval(heartbeat);clearInterval(clock);clearInterval(laserPoll);for(const l of Object.values(locals))l.targetQueue?.cancel();client.disconnect();});
  render();const ready=(async()=>{preferences=await client.preferences();renderedKey=null;await connect(true);})().catch(cause=>{error(new Error('Local Host unavailable: '+(cause?.message||String(cause))+'. Review Settings.'));render();});
  return {render,requestRender:renderer.request,connect,resync,ready,clearNotice};
}
