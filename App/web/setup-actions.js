import {signature,canSave,driverCheckReady,controllerChoiceReady,requiredDriver,driverLabel} from './setup.js';import {deviceKey} from './routes.js';
export async function refreshDraftDrivers(d,model,profile,read,changed){
  const driver=requiredDriver(model?.id,profile),check={modelId:model?.id,profileId:profile?.id,driver,state:'checking',issued:performance.now(),message:'Checking required driver…'};
  d.driverCheck=driver?check:null;changed();
  if(!driver)return;
  try{const inventory=await read();
    if(d.driverCheck!==check||d.modelId!==check.modelId||d.profileId!==check.profileId)return;
    if(driver!=='newport') {
      const status=inventory?.usb_serial?.[driver];
      if(inventory?.errors?.usb_serial||!status||!['ready','missing','unavailable','not_detected'].includes(status.state))throw new Error(inventory?.errors?.usb_serial||'Driver status unavailable.');
      check.state=status.state;check.issued=performance.now();
      check.message=status.state==='not_detected'?`No ${driverLabel(driver)} device detected. Connect the device, then refresh.`:
        status.state==='unavailable'?'Windows device status is unconfirmed or reports a fault. Check the USB connection and Device Manager, then refresh.':
        status.state==='missing'?`Required ${driverLabel(driver)} driver is missing.`:'Driver ready. Test Connection will verify the instrument identity.';
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
  return {
    async prepare(d,model){
      const profile=model?.profiles?.find(p=>p.id===d.profileId);
      if(profile&&!driverCheckReady(d,profile))throw new Error('Check the required driver before preparing this connection.');
      const selected=await acquire(d);await run(selected.domain,selected.record.revision,'connect',{acknowledge_lifecycle:true});},
    async test(d,model,profile){
      if(!profile||!driverCheckReady(d,profile))throw new Error('Check the required driver before testing this connection.');
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
 const pending=refreshDraftDrivers(d,model,profile,driverRead,changed),check=d.driverCheck;
 await pending;
 if(d.driverCheck!==check||d.modelId!==model?.id||d.profileId!==profile?.id)return;
 if(check?.state==='ready'&&model?.id==='tlb6700')await refreshControllerChoices(d,model,profile,scanRead,changed,previousScan);
}

export async function installDriverPackage(driver,client,readState,{wait=ms=>new Promise(r=>setTimeout(r,ms)),now=()=>performance.now()}={}){
 if(!['newport','ch340','cp210x'].includes(driver))throw new Error('Unknown driver package.');
 let status=await readState();
 if(status?.state==='ready')return;
 if(status?.state!=='missing')throw new Error(status?.message||'A missing driver is not confirmed. Connect the device and refresh.');
 let job=await client.installDriver(driver);const deadline=now()+20*60*1000;
 while(job.state==='running'){
  if(job.driver!==driver)throw new Error('Another driver installation is running. Wait for it to finish.');
  if(now()>deadline)throw new Error('Driver installation is still running. Wait for Windows to finish before refreshing devices.');
  await wait(1000);job=await client.driverInstallStatus();
 }
 if(job.driver!==driver)throw new Error('Driver installation status changed. Refresh devices before continuing.');
 if(!['completed','no_change'].includes(job.state))throw new Error(job.message||'Driver installation failed.');
 if(job.restart_required)throw new Error('Driver installed. Restart Windows, then check again.');
 status=await readState();
 if(status?.state!=='ready')throw new Error(status?.message||'Driver readiness is unconfirmed. Refresh devices before continuing.');
}
export async function installMissingDriver(d,model,profile,client,changed,options={}){
 const driver=requiredDriver(d.modelId,profile),check=d.driverCheck;
 if(d.busy||d.installing)return;
 if(!driver||check?.driver!==driver||check.modelId!==d.modelId||check.profileId!==d.profileId||check.state!=='missing')throw new Error('Install a driver only after the selected device reports a missing driver.');
 d.installing=true;d.proof=null;d.installError=null;changed();
 try{
  await installDriverPackage(driver,client,async()=>{await refreshDraftDrivers(d,model,profile,()=>client.driverStatus(),changed);return d.driverCheck;},options);
 }catch(cause){d.installError=cause.message||String(cause);throw cause;}
 finally{d.installing=false;changed();}
}
