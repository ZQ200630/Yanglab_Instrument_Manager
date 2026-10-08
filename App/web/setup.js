import {esc} from './panels.js';import {routeFor} from './routes.js';
import {renderConnections} from './connections.js';
import {cp210xLicense} from './driver-license.js';
import {serialFamily,serialChoiceReady,renderSerialField} from './serial-ports.js';
export const signature=d=>JSON.stringify([d.modelId,d.profileId,d.name,d.params]);
export function requiredDriver(modelId,profile,d){
 if(profile?.access==='newport')return 'newport';
 if(profile?.access!=='serial'||!['gain','voltage'].includes(modelId))return null;
 const selected=serialFamily(d?.serialScan?.ports?.find(p=>p.resource===d.params?.port));if(selected)return selected;
 const families=d?.serialScan?.driverFamilies||[];
 return modelId==='gain'?(families.find(f=>f.state==='missing')?.id||(families.length===1?families[0].id:'cp210x')):'ch340';
}
export const driverLabel=id=>({newport:'Newport USB',ch340:'CH340',cp210x:'CP210x (CP2102)'}[id]||'USB');
export function setupConnectionEffects(model,profile){
 const effects={gain:'Connecting turns current off before turning TEC off. Disconnecting uses the same order.',voltage:'Connecting and disconnecting set all eight channels to 0 V.',mdt693b:'Connecting and disconnecting preserve piezo output voltages.'};
 return [effects[model?.id],profile?.open_effects?.includes('DTR_RTS_reset_not_verified')?'Opening this serial port may reset the controller through its DTR/RTS lines.':null].filter(Boolean).join(' ');
}
export function renderHostIssue(issue){
 if(!issue)return '';
 const old=issue.code==='HostIncompatible',pending=issue.code==='HostStartPending';
 const title=old?'Previous Host needs to exit':pending?'Host startup is still unconfirmed':'Local Host unavailable';
 const detail=old?'A Host from a previous version is still running. Open Yang LAB Host in the system tray and choose “Stop Host safely…”. Then retry here; this app will start and connect its Rust Host automatically.':pending?'A Host process may still be starting. Check its connection before attempting another launch.':issue.message;
 return `<section class="card host-recovery" role="alert"><div class="card-body"><h2>${title}</h2><p>${esc(detail)}</p>${old?'<p class="hint">Safe stop: Voltage Source goes to zero; Gain current turns off before TEC; piezo outputs hold.</p>':''}<button class="btn primary" data-ui="connect-host">${pending?'Check connection':'Retry connection'}</button></div></section>`;
}
export function driverCheckReady(d,profile,now=performance.now()) {const c=d.driverCheck,driver=requiredDriver(d.modelId,profile,d);return !driver||Boolean(c&&c.driver===driver&&c.modelId===d.modelId&&c.profileId===d.profileId&&c.state==='ready'&&now>=c.issued&&now-c.issued<60000);}
export function controllerChoiceReady(d,now=performance.now()){const s=d.controllerScan;return Boolean(s?.state==='ready'&&(!s.modelId||s.modelId===d.modelId)&&(!s.profileId||s.profileId===d.profileId)&&now>=s.issued&&now-s.issued<60000&&s.controllers.some(c=>c.device_key===d.params?.device_key));}
export function testFormReady(d,model,profile){
 const check=d.driverCheck,scan=d.controllerScan;
 const prerequisite=!requiredDriver(d.modelId,profile,d)||check?.state==='ready'&&check.driver===requiredDriver(d.modelId,profile,d)&&check.modelId===d.modelId&&check.profileId===d.profileId;
 return Boolean(profile&&prerequisite&&serialChoiceReady(d,profile,d.serialScan?.issued??performance.now())&&(model?.id!=='tlb6700'||scan?.state==='ready'&&scan.controllers.some(c=>c.device_key===d.params?.device_key)));
}
export function canSave(d,now=performance.now()){return Boolean(d.record&&d.proof?.proof_id&&d.proof.revision===d.record.revision&&d.proof.signature===signature(d)&&now>=d.proof.issued&&now-d.proof.issued<60000);}
export function instrumentTarget(record,setups=[]){const owner=setups.find(s=>s.members.includes(record.device_id));return owner?{kind:'setup',id:owner.setup_id}:{kind:'device',id:record.device_id};}
const options=(items,value)=>items.map(([id,label])=>`<option value="${esc(id)}" ${id===value?'selected':''}>${esc(label)}</option>`).join('');
const renderSerialParameter=(key,f,d)=>`<label>${esc(({baudrate:'Baud rate',io_timeout_s:'Timeout (seconds)'})[key]||key.replaceAll('_',' '))}${f.choices?`<select class="control" data-param="${esc(key)}">${options(f.choices.map(v=>[String(v),String(v)]),String(d.params?.[key]??f.default))}</select>`:`<input class="control" data-param="${esc(key)}" type="${['number','integer'].includes(f.kind)?'number':'text'}" value="${esc(d.params?.[key]??f.default??'')}" ${f.minimum!==undefined?`min="${f.minimum}"`:''} ${f.maximum!==undefined?`max="${f.maximum}"`:''} step="${f.kind==='integer'?'1':'any'}" ${f.required?'required':''}>`}</label>`;
export function driverState(inventory,id){
 if(inventory?.checking)return 'checking';
 if(inventory?.errors?.[id==='newport'?'newport':'usb_serial'])return 'unavailable';
 if(id==='newport'){
  const status=inventory?.newport;
  if(!status)return 'unchecked';
  if(status.sdk?.state==='missing'||status.devices?.some(d=>d.driver_state==='missing'))return 'missing';
  return status.sdk?.state==='ready'&&!status.devices?.some(d=>d.driver_state!=='ready')?'ready':'unavailable';
 }
 const state=inventory?.usb_serial?.[id]?.state;
 return ['ready','missing','not_detected','unavailable'].includes(state)?state:state?'unavailable':'unchecked';
}
export function renderDriverStatus(inventory,available=true,installation={}){
 const labels={ready:'Ready',missing:'Driver required',not_detected:'No device detected',unavailable:'Check unavailable',unchecked:'Not checked',checking:'Checking…',installing:'Installing…'};
 const purposes={newport:'Newport tunable lasers',ch340:'Voltage Source · CH340 / CH341',cp210x:'Gain Driver · CP2102'};
 const rows=['newport','ch340','cp210x'].map(id=>{
  const pending=installation.installing&&installation.driver===id,state=pending?'installing':driverState(inventory,id),install=state==='missing'||pending;
  return `<div class="driver-row" data-driver-row="${id}"><div class="driver-name"><strong>${driverLabel(id)}</strong><small>${purposes[id]}</small></div><span class="driver-state ${state}" ${pending?'role="status"':''}><i aria-hidden="true"></i>${labels[state]}</span><div class="driver-action">${install?`<button class="btn primary small" data-ui="install-driver" data-driver="${id}" ${!available||installation.installing||inventory?.checking?'disabled':''} ${pending?'aria-busy="true"':''}>${pending?'Installing…':'Install'}</button>`:''}</div></div>`;
 }).join('');
 const license=driverState(inventory,'cp210x')==='missing'?`<details class="driver-license" id="settings-driver-license"><summary>Silicon Labs driver license</summary><pre>${esc(cp210xLicense)}</pre></details><span class="hint">Installing CP210x accepts this license.</span>`:'';
 const errors=['newport','usb_serial'].filter(id=>inventory?.errors?.[id]).map(id=>inventory.errors[id]);
 if(installation.error)errors.push(installation.error);
 return `<section class="card settings-section drivers-card"><div class="card-head"><div><h2>Drivers</h2><p class="card-subtitle">USB drivers for supported instruments</p></div><button class="btn small" data-ui="check-drivers" ${inventory?.checking||installation.installing||!available?'disabled':''}>Refresh</button></div><div class="driver-list">${rows}</div>${license||errors.length||!available?`<div class="driver-footer">${!available?'<p class="hint">Connect Local Host to check or install drivers.</p>':''}${license}${errors.map(message=>`<p class="driver-error" role="alert">${esc(message)}</p>`).join('')}</div>`:''}</section>`;
}
export function renderCheckPolicy(d,catalog={models:[]}) {const id=esc(d.device_id),policy=d.check_policy||{};
 return `<details id="checks-${id}" class="online-checks"><summary>Refresh</summary><label>Refresh interval (seconds)<input class="control" type="number" min="10" step="1" id="check-interval-${id}" value="${policy.interval_s||30}"></label><button class="btn" data-ui="save-checks" data-device="${id}">Save interval</button><button class="btn" data-ui="refresh-device" data-device="${id}">Refresh now</button></details>`;
}
export function laserHeadLabel(controller){
  const raw=controller.head_model||'',base=/^(?:TLB-)?([0-9]{4})(?:-[A-Za-z0-9]+)*$/.exec(raw)?.[1];
  // Display-only model-family names; never establish a tuning limit or infer a connector.
  const band=({'6712':'780','6721':'1064','6722':'1060'})[base];
  const model=raw?(raw.startsWith('TLB-')?raw:'TLB-'+raw):'Unidentified laser head';
  const serial=controller.head_serial?'Head S/N '+controller.head_serial:'Controller S/N '+controller.serial;
  return (band?band+' nm · ':'')+model+' · '+serial;
}
export function renderAddWizard(d={},catalog={categories:[],models:[]}){
  const model=catalog.models.find(m=>m.id===d.modelId),profile=model?.profiles.find(p=>p.id===d.profileId),category=d.category||model?.category||catalog.categories[0];
  return `<div class="wizard-backdrop" data-wizard-backdrop="true"></div><div class="card wizard" role="dialog" aria-modal="true" aria-label="Add New Instrument"><div class="card-head"><h2>Add New Instrument</h2><button class="btn" data-ui="cancel-wizard" ${d.busy||d.installing?'disabled':''}>Cancel</button></div><div class="card-body"><fieldset ${d.busy||d.installing?'disabled':''}>
    <label>Category<select class="control" data-draft="category">${options(catalog.categories.map(c=>[c,c]),category)}</select></label>
    <label>Manufacturer and model<select class="control" data-draft="modelId"><option value="">Select model</option>${options(catalog.models.filter(m=>m.category===category).map(m=>[m.id,`${m.manufacturer} · ${m.name}`]),d.modelId)}</select></label>
    ${!model?(d.modelId?'<p class="alert">Driver required for this model.</p>':''):`<label>Name<input class="control" data-draft="name" value="${esc(d.name||model.name)}" maxlength="128"></label>
    <label>Connection<select class="control" data-draft="profileId">${options(model.profiles.map(p=>[p.id,`${p.interfaces.join(' / ')} · ${p.access.toUpperCase()}`]),d.profileId)}</select></label>
    ${profile?.access==='serial'?renderSerialField(d,Object.entries(profile.fields).filter(([key])=>key!=='port').map(([key,f])=>renderSerialParameter(key,f,d)).join('')):profile?Object.entries(profile.fields).filter(([key])=>model.id!=="tlb6700"||key==="device_key").map(([key,f])=>key==='port'&&profile.access==='serial'?renderSerialField(d):key==='device_key'&&model.id==='tlb6700'?`<label>Laser head<select class="control" data-param="device_key" data-managed="true" ${d.controllerScan?.state==='ready'?'':'disabled'}><option value="">${d.controllerScan?.state==='checking'?'Scanning…':'Select laser head'}</option>${options((d.controllerScan?.controllers||[]).map(c=>[c.device_key,laserHeadLabel(c)]),d.params?.device_key)}</select></label><button class="btn" data-ui="scan-controllers" ${d.busy||d.controllerScan?.state==='checking'?'disabled':''}>Refresh devices</button>${d.controllerScan?.state==='failed'?`<p class="alert">Scan failed: ${esc(d.controllerScan.message)}</p>`:d.controllerScan?.state==='ready'&&!d.controllerScan.controllers.length?'<p class="alert">No TLB-6700 detected. Check USB connection, then refresh.</p>':''}`:`<label>${esc(key.replaceAll('_',' '))}${f.choices?`<select class="control" data-param="${esc(key)}">${options(f.choices.map(v=>[String(v),String(v)]),String(d.params?.[key]??f.default))}</select>`:`<input class="control" data-param="${esc(key)}" type="${['number','integer'].includes(f.kind)?'number':'text'}" value="${esc(d.params?.[key]??f.default??'')}" ${f.minimum!==undefined?`min="${f.minimum}"`:''} ${f.maximum!==undefined?`max="${f.maximum}"`:''} step="${f.kind==='integer'?'1':'any'}" ${f.required?'required':''}>`}</label>`).join(''):''}
    ${requiredDriver(d.modelId,profile,d)&&d.driverCheck?.state==='missing'?`<p class="alert">${driverLabel(requiredDriver(d.modelId,profile,d))} driver${profile?.access==='newport'?' or SDK':''} is missing. <button class="btn" data-ui="install-draft-driver" ${d.installing?'disabled':''}>${d.installing?'Installing…':'Install driver'}</button></p>`:requiredDriver(d.modelId,profile,d)&&['unavailable','not_detected'].includes(d.driverCheck?.state)?`<p class="alert">${esc(d.driverCheck.message)} <button class="btn" data-ui="check-draft-drivers">Refresh devices</button></p>`:''}
    ${d.installError?`<p class="alert">${esc(d.installError)}</p>`:''}
    ${requiredDriver(d.modelId,profile,d)==='cp210x'&&d.driverCheck?.state==='missing'?`<p class="hint">Selecting Install driver accepts the Silicon Labs driver license.</p><details><summary>Driver license</summary><pre style="white-space:pre-wrap">${esc(cp210xLicense)}</pre></details>`:''}
    ${setupConnectionEffects(model,profile)?`<p class="warning">${esc(setupConnectionEffects(model,profile))}</p>`:''}
    ${d.connectionError?`<p class="alert" role="alert">${esc(d.connectionError)}</p>`:''}
    <div class="form-actions"><button class="btn primary" data-ui="test-draft" ${d.busy||d.installing||!testFormReady(d,model,profile)?'disabled':''}>${profile?.probe_mode==='supervised'?'Connect &amp; verify':'Test Connection'}</button>
    <button class="btn primary" data-ui="save-draft" ${canSave(d)&&!d.busy?'':'disabled'}>Add &amp; Save</button></div>`}
    ${d.proof?.proof_id&&!canSave(d)?'<p class="alert">Connection check expired. Test again.</p>':''}
    ${d.releaseError?'<button class="btn danger" data-ui="safe-stop-draft">Retry connection release</button><p class="hint">Connection release failed. Retry release before continuing.</p>':''}
    ${d.message?`<p role="status">${esc(d.message)}</p>`:''}</fieldset></div></div>`;
}
export const setupChoices=r=>r.devices.filter(d=>d.model_id==='mdt693b'&&['2110148249-10','160721175410'].includes(d.expected_identity?.serial)&&!r.setups.some(s=>s.members.includes(d.device_id)));
export function renderDeviceSetup(host,catalog={models:[]},editor={}){const r=host.registry||{devices:[],setups:[],drafts:[]};const html=`<div class="page-header"><div><div class="eyebrow">LOCAL / CONFIGURATION</div><h1>Device setup</h1></div><button class="btn primary" data-ui="add-new">＋ Add New</button></div>
  <div class="device-list">${r.devices.map(d=>`<article class="card device-row"><div>${editor.rename===d.device_id?`<label>Instrument name<input class="control" id="rename-name" value="${esc(d.name)}" maxlength="128"></label><button class="btn primary" data-ui="save-rename" data-device="${d.device_id}">Save name</button><button class="btn" data-ui="cancel-editor">Cancel</button>`:`<h2>${esc(d.name)}</h2>`}<small>${esc(catalog.models.find(m=>m.id===d.model_id)?.name||d.model_id)} · S/N ${esc(d.expected_identity?.serial||d.expected_identity?.transport_serial||'Unknown')}</small>${renderCheckPolicy(d,catalog)}</div><a class="btn" href="${routeFor(host.host_id,instrumentTarget(d,r.setups))}">Open</a><button class="btn" data-ui="rename" data-device="${d.device_id}">Rename</button><button class="btn warn" data-ui="retire" data-device="${d.device_id}">Remove</button></article>`).join('')||`<div class="empty-panel">${host.connected?'No instruments configured.':'Waiting for Local Host. Device configuration will appear after connection.'}</div>`}</div>
  ${r.drafts.filter(d=>d.status!=='Cancelled').map(d=>`<article class="card device-row"><span>${esc(d.name)} · ${esc(d.status)}</span><button class="btn" data-ui="open-draft" data-device="${d.device_id}">Review draft</button><button class="btn warn" data-ui="cancel-draft" data-device="${d.device_id}">Cancel draft</button></article>`).join('')}
  ${r.setups.length||setupChoices(r).length?`<div class="section-title"><h2>Fiber coupling</h2><button class="btn" data-ui="add-setup">Add setup</button></div>${r.setups.map(s=>`<article class="card device-row"><span>${esc(s.name)} · ${s.members.length} controller(s)</span><a class="btn" href="${routeFor(host.host_id,{kind:'setup',id:s.setup_id})}">Open</a><button class="btn warn" data-ui="retire-setup" data-setup="${s.setup_id}">Remove setup</button></article>`).join('')}`:''}${editor.addSetup?`<section class="card"><div class="card-body"><label>Setup name<input class="control" id="setup-name" value="Fiber coupling" maxlength="128"></label>${setupChoices(r).map(d=>`<label><input type="checkbox" id="setup-member-${d.device_id}"> ${esc(d.name)} · ${esc(d.expected_identity.serial)}</label>`).join('')}<button class="btn primary" data-ui="save-setup">Save setup</button><button class="btn" data-ui="cancel-editor">Cancel</button></div></section>`:''}`;return host.connected?html:html.replaceAll('<button ','<button disabled ');}
export function renderSettings(host,preferences,remote={peers:[],owner:null},connections){return `<h1>Settings</h1>
  <section class="card settings-section local-host-card"><div class="card-head"><h2>Local Host</h2><span class="badge">${host?.hostBusy?'CONNECTING':host?.connected?'ONLINE':host?.hostIssue?.code==='HostIncompatible'?'UPDATE REQUIRED':'OFFLINE'}</span></div><div class="card-body">
  <p class="host-status-text">${host?.hostBusy?'Starting and connecting automatically…':host?.connected?'Connected. Ready to use your instruments.':'Local Host is unavailable.'}</p><p class="hint">Starts and connects automatically when you open the app.</p>
  ${!host?.connected&&!host?.hostBusy&&!host?.hostIssue&&!remote.networkOnly?'<button class="btn primary" data-ui="connect-host">Retry connection</button>':''}
  <details id="host-advanced" class="host-advanced"><summary>Advanced settings</summary><div class="host-advanced-content">
  <label>Host name<div class="host-setting-row"><input class="control" id="host-name" value="${esc(host?.registry?.settings?.host_name||'Local computer')}" ${host?.connected?'':'disabled'}><button class="btn" data-ui="save-host" ${host?.connected?'':'disabled'}>Save name</button></div></label>
  <label>Measurement data folder<input class="control" id="host-data-root" readonly value="${esc(host?.registry?.settings?.data_root||host?.archive?.active_root||'')}"></label><button class="btn" data-ui="choose-data-root" ${host?.connected?'':'disabled'}>Change folder</button>
  <p class="hint">The default folder works automatically. Changes are saved immediately and apply after a safe Host restart.</p>
  <div class="host-maintenance"><p class="hint">Stop Host disconnects all instruments safely. Disconnect GUI leaves the Host running.</p><div class="form-actions"><button class="btn warn" data-ui="stop-host" ${host?.connected?'':'disabled'}>Stop Host</button><button class="btn" data-ui="disconnect-host" ${host?.connected?'':'disabled'}>Disconnect GUI</button></div></div>
  </div></details></div></section>
  ${renderDriverStatus(host?.driverInventory,host?.connected,host?.driverInstallation)}${renderConnections({...remote,localConnected:host?.connected},connections)}`;}
export function renderSetup(key,store){const state=store.get(key);return `<h1>Fiber coupling</h1><p>Laboratory coordinates: +X right · +Y away from operator · +Z up</p><p class="warning">Session-only open-loop estimate. Baseline adoption requires explicit operator confirmation.</p><pre>${esc(JSON.stringify(state?.device??{},null,2))}</pre>`;}
