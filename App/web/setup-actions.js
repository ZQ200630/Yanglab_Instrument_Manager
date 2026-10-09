import {signature,canSave,driverCheckReady,controllerChoiceReady,requiredDriver,driverLabel} from './setup.js';import {deviceKey} from './routes.js';
import {serialCandidates,validPort,serialChoiceReady} from './serial-ports.js';
import {sameJsonValue as same} from './json-value.js';
const sameInstance=(a,b)=>typeof a==='string'&&typeof b==='string'&&a.toUpperCase()===b.toUpperCase();
const validId=value=>typeof value==='string'&&/^[a-f0-9]{32}$/.test(value);
export function draftConnectionState(snapshot,{controlled=false,outcomeUnknown=false,pending=false}={}){
 const retained=Boolean(snapshot?.context?.connection_id||snapshot?.responsibility),device=snapshot?.device;
 const busy=pending||snapshot?.pending>0||['active_request_id','pending_request_id','safety_request_id','readback_request_id'].some(name=>Boolean(snapshot?.[name]))||['CONNECTING','CLOSING'].includes(snapshot?.state);
 return {retained,busy,unknown:outcomeUnknown,healthy:Boolean(controlled&&!busy&&!outcomeUnknown&&snapshot?.context?.connection_id&&
  ['READY','ACTIVE'].includes(snapshot?.state)&&device?.connected===true&&['READY','ACTIVE'].includes(device.state)),fault:typeof device?.fault==='string'?device.fault:null};
}
export function draftFailureMessage(cause,snapshot){
 const raw=cause?.message||String(cause);let evidence;
 try{if(raw.length<=32768)evidence=JSON.parse(raw);}catch{}
 const failure=evidence?.error||evidence?.result?.error;
 const state=draftConnectionState(snapshot);
 if(state.retained&&state.fault){const explanation=typeof failure?.message==='string'?failure.message:raw.startsWith('{')?'Check connection status before continuing.':raw;
  return explanation.includes(state.fault)?explanation:`${state.fault}. Connection release is unconfirmed. ${explanation}`;}
 if(failure?.type==='ManualVerificationRequired'||failure?.code==='ManualVerificationRequired')return failure.message||'Instrument session is unavailable. Check connection and release status.';
 return typeof failure?.message==='string'?failure.message:typeof evidence?.message==='string'?evidence.message:raw.startsWith('{')?'Device verification failed. Check the selected port and connection status.':raw;
}
export async function refreshSerialChoices(d,model,profile,read,changed){
 if(profile?.access!=='serial'){d.serialScan=null;return;}
 const previous=d.serialScan?.ports?.find(p=>p.resource===d.params?.port),scan={modelId:model?.id,profileId:profile.id,state:'checking',ports:[],candidates:[],issued:performance.now()};
 d.serialScan=scan;changed();
 try{const reply=await read();if(d.serialScan!==scan||d.modelId!==scan.modelId||d.profileId!==scan.profileId)return;
  if(reply?.errors?.serial)throw Error(reply.errors.serial);
  if(!Array.isArray(reply?.serial)||reply.serial.length>4096||reply.serial.some(p=>!validPort(p?.resource)||typeof p.description!=='string'||p.description.length>2048||typeof p.serial!=='string'||p.serial.length>512||typeof p.instance_id!=='string'||p.instance_id.length>=2048||p.instance_id.includes('\0')||[p.vid,p.pid].some(v=>v!==null&&(!Number.isInteger(v)||v<0||v>65535)))||new Set(reply.serial.map(p=>p.resource)).size!==reply.serial.length)throw Error('Serial port inventory unavailable. Refresh or enter a port manually.');
  scan.ports=reply.serial.toSorted((a,b)=>Number(a.resource.slice(3))-Number(b.resource.slice(3)));scan.candidates=serialCandidates(scan.ports,model.id);scan.driverFamilies=['ch340','cp210x'].filter(id=>reply.usb_serial?.[id]?.devices?.length).map(id=>({id,state:reply.usb_serial[id].state}));scan.state='ready';scan.issued=performance.now();
  const selected=scan.ports.find(p=>p.resource===d.params?.port),hadSelection=Boolean(d.params?.port),replaced=previous&&selected&&(['vid','pid','serial'].some(key=>previous[key]!==selected[key])||!sameInstance(previous.instance_id,selected.instance_id));
  if(!selected||replaced)d.proof=null;
  if(!d.manualPort&&(!selected||replaced)){delete d.params.port;if(!hadSelection&&scan.candidates.length===1)d.params.port=scan.candidates[0].resource;}
 }catch(cause){if(d.serialScan===scan&&d.modelId===scan.modelId&&d.profileId===scan.profileId){scan.state='failed';scan.message=cause.message||String(cause);d.proof=null;}}
 finally{changed();}
}
export async function refreshDraftDrivers(d,model,profile,read,changed){
  const driver=requiredDriver(model?.id,profile,d),check={modelId:model?.id,profileId:profile?.id,driver,state:'checking',issued:performance.now(),message:'Checking required driver…'};
  d.driverCheck=driver?check:null;changed();
  if(!driver)return;
  try{const inventory=await read();
    if(d.driverCheck!==check||d.modelId!==check.modelId||d.profileId!==check.profileId)return;
    if(driver!=='newport') {
      const status=inventory?.usb_serial?.[driver];
      if(inventory?.errors?.usb_serial||!status||!['ready','missing','unavailable','not_detected'].includes(status.state))throw new Error(inventory?.errors?.usb_serial||'Driver status unavailable.');
      const selected=inventory?.serial?.find(p=>p.resource===d.params?.port);
      let state=status.state;
      if(d.params?.port&&(selected||!d.manualPort)){
        const matches=selected?.instance_id?status.devices?.filter(p=>sameInstance(p.instance_id,selected.instance_id)):[];
        state=matches?.length===1&&['ready','missing','unavailable'].includes(matches[0].driver_state)?matches[0].driver_state:'unavailable';
      }else if(d.serialScan?.state==='ready'&&!d.params?.port&&state!=='missing'){
        check.state='unavailable';check.message='Select a serial port to check its driver.';return;
      }
      check.state=state;check.issued=performance.now();
      check.message=state==='not_detected'?`No ${driverLabel(driver)} device detected. Connect the device, then refresh.`:
        state==='unavailable'?'Windows device status is unconfirmed or reports a fault. Check the USB connection and Device Manager, then refresh.':
        state==='missing'?`Required ${driverLabel(driver)} driver is missing.`:'Driver ready. Connection verification will check the instrument identity.';
      return;
    }
    const status=inventory?.newport;
    if(inventory?.errors?.newport||!status)throw new Error(inventory?.errors?.newport||'Driver status unavailable.');
    check.state=status.sdk?.state==='missing'||status.devices?.some(x=>x.driver_state==='missing')?'missing':
      status.sdk?.state==='ready'&&!status.devices?.some(x=>x.driver_state!=='ready')?'ready':'unavailable';
    check.message=check.state==='ready'?'Compatible Newport SDK and a ready USB controller detected. Test Connection will verify the selected controller identity.':
      check.state==='missing'?'Required Newport driver or SDK is missing. Install the official package, then check again.':
      'Driver readiness is unconfirmed. Check the SDK and Windows device status, then reconnect the controller and check again.';
    check.issued=performance.now();
  }catch(cause){if(d.driverCheck===check&&d.modelId===check.modelId&&d.profileId===check.profileId){check.state='unavailable';check.message=cause.message||String(cause);}}
  finally{changed();}
}
export async function refreshControllerChoices(d,model,profile,read,changed,previousScan=d.controllerScan){
  const previous=previousScan?.controllers?.find(c=>c.device_key===d.params?.device_key);
  const scan={modelId:model?.id,profileId:profile?.id,state:'checking',controllers:[],issued:performance.now()};
  d.controllerScan=model?.id==='tlb6700'&&profile?.access==='newport'?scan:null;changed();
  if(!d.controllerScan)return;
  try{const reply=await read();
    if(d.controllerScan!==scan||d.modelId!==scan.modelId||d.profileId!==scan.profileId)return;
    const items=reply?.controllers;
    if(!Array.isArray(items)||items.length>32||items.some(x=>!x||typeof x.serial!=='string'||!/^\d{1,16}$/.test(x.serial)||x.device_key!=='6700 SN'+x.serial||
      (x.head_model!==undefined||x.head_serial!==undefined)&&(typeof x.head_model!=='string'||typeof x.head_serial!=='string'||!/^(?:TLB-)?\d{4}(?:-[A-Za-z0-9]+)*$/.test(x.head_model)||x.head_model.length>64||!/^[A-Za-z0-9_-]{1,64}$/.test(x.head_serial)))||new Set(items.map(x=>x.device_key)).size!==items.length)throw new Error('Invalid controller scan.');
    const selected=items.find(c=>c.device_key===d.params?.device_key);
    if(previous&&selected&&(previous.head_model!==selected.head_model||previous.head_serial!==selected.head_serial))d.proof=null;
    scan.controllers=items;scan.state='ready';scan.issued=performance.now();
    if(!items.some(x=>x.device_key===d.params?.device_key)){
      const previouslySelected=d.params.device_key;delete d.params.device_key;d.proof=null;
      if(!previouslySelected&&items.length===1)d.params.device_key=items[0].device_key;
    }
  }catch(cause){if(d.controllerScan===scan&&d.modelId===scan.modelId&&d.profileId===scan.profileId){scan.state='failed';scan.message=cause.message||String(cause);d.proof=null;}}
  finally{changed();}
}
export function createSetupActions(session,_legacyConfirm,resync,run){
  const client=session.client,store=session.store;const host=()=>store.host(session.hostId),rev=()=>host().registry.registry_rev;
  async function ensure(d){
    if(d.record){if(d.record.mode==='unverified'||d.recordSignature!==signature(d)){
      await client.cancelDraft({draft_id:d.record.device_id,expected_rev:rev()});store.dropLease(deviceKey(session.hostId,{kind:'device',id:d.record.device_id}));d.record=null;d.proof=null;await resync();
    }else return d.record;}
    d.record=await client.createDraft({model_id:d.modelId,profile_id:d.profileId,params:d.params,name:d.name,expected_rev:rev()});
    d.recordSignature=signature(d);await resync();return d.record;
  }
  async function acquire(d){const record=await ensure(d),domain={kind:'device',id:record.device_id},key=deviceKey(session.hostId,domain);
    let lease=store.lease(key);if(!lease){lease=await client.acquire(domain);store.setLease(key,lease);}return {record,domain,key,lease};}
  async function awaitDraftRelease(d,key,{now=()=>performance.now(),wait=ms=>new Promise(resolve=>setTimeout(resolve,ms)),releaseTimeoutMs=10000}={}){
    const attempt=d.releaseAttempt,deadline=now()+releaseTimeoutMs;
    do {
      await resync();const currentHost=host(),snapshot=store.get(key),ticket=attempt.ticket;
      if(currentHost?.bootId!==attempt.bootId)throw new Error(`${attempt.fault} Host changed; connection release is unconfirmed.`);
      if(!ticket)throw new Error(`${attempt.fault} Release acceptance is unknown. Check the original release status; it will not be replayed.`);
      const receipt=currentHost.cleanup_attempts?.flat().find(receipt=>same(receipt?.ticket,ticket));
      if(receipt?.error||receipt?.resource_release_confirmed===false)throw new Error(`${attempt.fault} Connection release failed: ${receipt.error?.message||'resource release is unconfirmed'}.`);
      const result=receipt?.result,cleanup=result?.result?.cleanup,control=currentHost.control?.[attempt.domain.kind+':'+attempt.domain.id];
      if(currentHost.connected&&currentHost.synced&&currentHost.seq>attempt.seq&&receipt?.resource_release_confirmed===true&&receipt.error===null&&
        result?.ok===true&&result.phase==='completed'&&result.result?.connected===false&&result.result?.effective_intent==='disconnect'&&
        validId(cleanup?.attempt_id)&&Array.isArray(cleanup.unreleased)&&cleanup.unreleased.length===0&&Array.isArray(cleanup.steps)&&cleanup.steps.length>0&&cleanup.steps.every(step=>step?.error===null)&&
        result.context?.connection_id===null&&same(result.context?.domain,attempt.domain)&&result.context?.session_id===attempt.context.session_id&&result.context.epoch>attempt.context.epoch&&
        same(snapshot?.context,result.context)&&snapshot?.state==='DISCONNECTED'&&snapshot.responsibility===false&&snapshot.pending===0&&
        ['active_request_id','pending_request_id','safety_request_id','readback_request_id'].every(name=>snapshot[name]===null)&&control?.state==='AVAILABLE'&&control.control_epoch===ticket.control_epoch){
        if(store.lease(key)?.token===attempt.lease.token)store.dropLease(key);
        d.releaseAttempt=null;return;
      }
      if(now()>=deadline)break;await wait(Math.min(150,Math.max(1,deadline-now())));
    }while(now()<=deadline);
    throw new Error(`${attempt.fault} Connection release is unconfirmed. Test again to check this original release; no command will be replayed.`);
  }
  return {
    async prepare(d,model,options={}){
      if(d.releaseAttempt){options.onProgress?.('Checking the original connection release…');await awaitDraftRelease(d,deviceKey(session.hostId,d.releaseAttempt.domain),options);}
      if(options.cancelled?.())return;
      const profile=model?.profiles?.find(p=>p.id===d.profileId);
      if(profile&&!driverCheckReady(d,profile))throw new Error('Check the required driver before preparing this connection.');
      if(profile&&!serialChoiceReady(d,profile))throw new Error('Choose a detected serial port or enter a port manually before connecting.');
      const record=await ensure(d),domain={kind:'device',id:record.device_id},key=deviceKey(session.hostId,domain),snapshot=store.get(key);
      const state=draftConnectionState(snapshot,{...options,controlled:store.canControl(key)});
      if(state.unknown||d.connectionOutcomeUnknown)throw Object.assign(new Error('Connection outcome is unknown. Check status or disconnect before continuing.'),{outcomeUnknown:true});
      if(state.busy)throw new Error('An instrument operation is in progress. Wait for its outcome before connecting.');
      if(state.healthy)return;
      if(state.retained){
        const lease=store.lease(key);if(!lease||!store.canControl(key))throw new Error('Current connection authority is unavailable. Keep the retained connection until its owner confirms release.');
        const fault=state.fault||'The retained instrument connection is not healthy.';
        d.releaseAttempt={bootId:host().bootId,seq:host().seq,domain,context:structuredClone(snapshot.context),lease,ticket:null,fault};
        options.onProgress?.(`${fault} Releasing the connection safely…`);
        const acceptance=await client.release(domain,lease),ticket=acceptance?.ticket;
        if(acceptance?.accepted!==true||!validId(ticket?.ticket_id)||ticket.boot_id!==d.releaseAttempt.bootId||!same(ticket.domain,domain)||ticket.control_epoch!==lease.control_epoch+1||ticket.cause!=='Released')
          throw new Error(`${fault} Release acceptance is unconfirmed. Check the original release status.`);
        d.releaseAttempt.ticket=ticket;await awaitDraftRelease(d,key,options);
      }
      if(options.cancelled?.())return;
      if(options.outcomeUnknown||d.connectionOutcomeUnknown)throw Object.assign(new Error('Connection outcome is unknown. Check status before reconnecting.'),{outcomeUnknown:true});
      options.onProgress?.('Connecting and initializing safely…');
      const selected=await acquire(d);if(options.cancelled?.())return;let outcome;
      try{outcome=await run(selected.domain,selected.record.revision,'connect',{acknowledge_lifecycle:true});}
      catch(cause){if(cause?.outcomeUnknown)d.connectionOutcomeUnknown=true;throw cause;}
      if(outcome?.phase!=='completed'){d.connectionOutcomeUnknown=true;throw Object.assign(new Error('Connection is unconfirmed. Check device status or disconnect before continuing.'),{outcomeUnknown:true});}},
    async test(d,model,profile){
      if(!profile||!driverCheckReady(d,profile))throw new Error('Check the required driver before testing this connection.');
      if(!serialChoiceReady(d,profile))throw new Error('Refresh serial ports and select the device before testing.');
      if(model.id==='tlb6700'&&!controllerChoiceReady(d))throw new Error('Select a detected controller before testing.');
      const {record,lease}=await acquire(d);const issued=performance.now();
      const proof=await client.testConnection({draft_id:record.device_id,expected_rev:rev(),consent:{accepted:true,mode:host().mode,config_digest:record.config_digest,
        open_effects:profile.open_effects,supervised:profile.probe_mode==='supervised',retain_session:profile.probe_mode==='supervised'},
        lease_token:lease.token,control_epoch:lease.control_epoch,request_id:crypto.randomUUID().replaceAll('-',''),sequence:client.nextSequence()});
      if(!proof.proof_id)throw new Error('No valid verification proof. Session may remain retained.');
      d.proof={...proof,issued,revision:record.revision,signature:signature(d)};d.message=null;await resync();
    },
    async save(d){if(!canSave(d))throw new Error('Verification expired or fields changed. Test again.');
      const device=await client.saveDevice({draft_id:d.record.device_id,proof_id:d.proof.proof_id,expected_rev:rev()});d.proof=null;await resync();return device;},
    async cancel(d){if(d.record){await client.cancelDraft({draft_id:d.record.device_id,expected_rev:rev()});store.dropLease(deviceKey(session.hostId,{kind:'device',id:d.record.device_id}));await resync();}},
  };
}

export async function refreshDraftConnection(d,model,profile,driverRead,scanRead,changed){
 const previousScan=d.controllerScan;
 d.controllerScan=null;
 let inventory;const read=()=>inventory??=Promise.resolve().then(driverRead);
 if(profile?.access==='serial')await refreshSerialChoices(d,model,profile,read,changed);else d.serialScan=null;
 if(d.modelId!==model?.id||d.profileId!==profile?.id)return;
 const pending=refreshDraftDrivers(d,model,profile,read,changed),check=d.driverCheck;
 await pending;
 if(d.driverCheck!==check||d.modelId!==model?.id||d.profileId!==profile?.id)return;
 if(check?.state==='ready'&&model?.id==='tlb6700')await refreshControllerChoices(d,model,profile,scanRead,changed,previousScan);
}

export async function installDriverPackage(driver,client,readState,{wait=ms=>new Promise(r=>setTimeout(r,ms)),now=()=>performance.now(),isCurrent=()=>true}={}){
 if(!['newport','ch340','cp210x'].includes(driver))throw new Error('Unknown driver package.');
 const check=()=>{if(!isCurrent())throw new Error('Local Host changed. Refresh drivers before continuing.');};
 check();let status=await readState();check();
 if(status?.state==='ready')return;
 if(status?.state!=='missing')throw new Error(status?.message||'A missing driver is not confirmed. Connect the device and refresh.');
 let job=await client.installDriver(driver);check();const deadline=now()+20*60*1000;
 while(job.state==='running'){
  if(job.driver!==driver)throw new Error('Another driver installation is running. Wait for it to finish.');
  if(now()>deadline)throw new Error('Driver installation is still running. Wait for Windows to finish before refreshing devices.');
  await wait(1000);check();job=await client.driverInstallStatus();check();
 }
 if(job.driver!==driver)throw new Error('Driver installation status changed. Refresh devices before continuing.');
 if(!['completed','no_change'].includes(job.state))throw new Error(job.message||'Driver installation failed.');
 if(job.restart_required)throw new Error('Driver installed. Restart Windows, then check again.');
 check();status=await readState();check();
 if(status?.state!=='ready')throw new Error(status?.message||'Driver readiness is unconfirmed. Refresh devices before continuing.');
}
export async function installMissingDriver(d,model,profile,client,changed,options={}){
 const driver=requiredDriver(d.modelId,profile,d),check=d.driverCheck;
 if(d.busy||d.installing)return;
 if(!driver||check?.driver!==driver||check.modelId!==d.modelId||check.profileId!==d.profileId||check.state!=='missing')throw new Error('Install a driver only after the selected device reports a missing driver.');
 d.installing=true;d.proof=null;d.installError=null;changed();
 try{
  await installDriverPackage(driver,client,async()=>{const inventory=await client.driverStatus();if(profile?.access==='serial')await refreshSerialChoices(d,model,profile,async()=>inventory,changed);await refreshDraftDrivers(d,model,profile,async()=>inventory,changed);return d.driverCheck;},options);
 }catch(cause){d.installError=cause.message||String(cause);throw cause;}
 finally{d.installing=false;changed();}
}
