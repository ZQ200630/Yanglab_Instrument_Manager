import {esc} from './panels.js';

const interfacesFor=profile=>profile?.fields?.resource?.interfaces??profile?.interfaces??[];

// Keep this grammar aligned with host/catalog.rs::reviewed_resource.
function reviewedResource(resource,interfaces){
 if(typeof resource!=='string'||!resource.length||resource.length>256||/[\x00-\x1f\x7f]/.test(resource)||!Array.isArray(interfaces))return false;
 const gpib=/^GPIB\d*::\d{1,2}(?:::\d{1,2})?::INSTR$/.test(resource);
 const usb=/^USB\d*::[A-Za-z0-9]+::[A-Za-z0-9]+::[A-Za-z0-9_.-]+(?:::\d+)?::INSTR$/.test(resource);
 const tcpip=resource.match(/^TCPIP\d*::([A-Za-z0-9.-]{1,253})(?:::inst\d+)?::INSTR$/);
 return gpib&&interfaces.includes('GPIB')||usb&&interfaces.includes('USB')||Boolean(tcpip&&/[A-Za-z0-9]/.test(tcpip[1])&&interfaces.includes('TCPIP'));
}

export function visaChoiceReady(d,profile,now=performance.now()){
 if(profile?.access!=='visa')return true;
 const resource=d.params?.resource;
 if(!reviewedResource(resource,interfacesFor(profile)))return false;
 if(d.manualResource)return true;
 const scan=d.visaScan;
 return Boolean(scan?.state==='ready'&&scan.modelId===d.modelId&&scan.profileId===d.profileId&&scan.profileId===profile.id&&Number.isFinite(scan.issued)&&now>=scan.issued&&now-scan.issued<60000&&Array.isArray(scan.resources)&&scan.resources.includes(resource));
}

const pendingScans=new WeakMap();
const currentScan=(d,scan)=>d.visaScan===scan&&d.modelId===scan.modelId&&d.profileId===scan.profileId;

function inventoryResources(reply){
 if(!reply||!Array.isArray(reply.visa)||reply.visa.length>4096||!reply.errors||typeof reply.errors!=='object'||Array.isArray(reply.errors))throw Error('VISA inventory returned an invalid response. Refresh or enter the resource manually.');
 if(reply.errors.visa!==undefined&&typeof reply.errors.visa!=='string')throw Error('VISA inventory returned an invalid error response.');
 if(reply.errors.visa)throw Error(reply.errors.visa);
 for(const resource of reply.visa)if(typeof resource!=='string'||!resource.length||resource.length>256||/[\x00-\x1f\x7f]/.test(resource))throw Error('VISA inventory returned invalid resource entries. Refresh or enter the resource manually.');
 return [...new Set(reply.visa.filter(resource=>reviewedResource(resource,['GPIB','USB','TCPIP'])))].sort();
}

export function refreshVisaChoices(d,model,profile,read,changed){
 if(profile?.access!=='visa'){d.visaScan=null;return Promise.resolve();}
 const pending=pendingScans.get(d);
 if(pending&&currentScan(d,pending.scan)&&pending.scan.state==='checking')return pending.promise;
 const previous=d.visaScan,matching=previous?.modelId===model?.id&&previous?.profileId===profile.id;
 const selected=d.params?.resource,manual=Boolean(d.manualResource);
 const scan={modelId:model?.id,profileId:profile.id,state:'checking',resources:matching?previous.resources:[],candidates:matching?previous.candidates:[],issued:performance.now(),message:''};
 d.visaScan=scan;changed();
 const promise=(async()=>{
  try{
   const reply=await read();
   if(!currentScan(d,scan))return;
   const resources=inventoryResources(reply),candidates=resources.filter(resource=>reviewedResource(resource,interfacesFor(profile)));
   scan.resources=resources;scan.candidates=candidates;scan.state='ready';scan.issued=performance.now();
   const current=d.params?.resource;
   if(current&&!candidates.includes(current))d.proof=null;
   if(!selected&&!manual&&!current&&!d.manualResource&&candidates.length===1){d.params??={};d.params.resource=candidates[0];}
  }catch(error){
   if(!currentScan(d,scan))return;
   scan.state='failed';scan.issued=performance.now();scan.message=String(error?.message||error||'VISA inventory is unavailable.').slice(0,2048);d.proof=null;
  }finally{
   if(currentScan(d,scan))changed();
   if(pendingScans.get(d)?.scan===scan)pendingScans.delete(d);
  }
 })();
 pendingScans.set(d,{scan,promise});
 return promise;
}

export function renderVisaField(d,profile,advanced=''){
 const now=performance.now(),scan=d.visaScan,matching=scan?.modelId===d.modelId&&scan?.profileId===d.profileId&&scan?.profileId===profile?.id;
 const pending=matching&&scan.state==='checking',fresh=matching&&scan.state==='ready'&&Number.isFinite(scan.issued)&&now>=scan.issued&&now-scan.issued<60000;
 const candidates=matching&&Array.isArray(scan.candidates)?scan.candidates.filter(resource=>reviewedResource(resource,interfacesFor(profile))):[];
 const resource=d.params?.resource||'',missing=resource&&!candidates.includes(resource);
 const placeholder=pending?'Checking VISA resources…':'Select VISA resource';
 const field=d.manualResource?`<input class="control" data-param="resource" type="text" value="${esc(resource)}" maxlength="256" placeholder="${interfacesFor(profile).includes('USB')?'USB0::…::INSTR':'GPIB0::4::INSTR'}">`:`<select class="control" data-param="resource" data-managed="true" ${pending?'disabled':''}><option value="">${placeholder}</option>${candidates.map(value=>`<option value="${esc(value)}" ${value===resource?'selected':''}>${esc(value)}</option>`).join('')}${missing?`<option value="${esc(resource)}" selected>${esc(resource)} · ${fresh?'unavailable':'previous selection'}</option>`:''}</select>`;
 let status;
 if(pending){const elapsed=Number.isFinite(scan.issued)&&now>=scan.issued?` <span class="activity-elapsed" data-activity-elapsed="${scan.issued}" data-activity-ended="">${Math.floor((now-scan.issued)/1000)} s</span> elapsed.`:'';status=`<p class="hint" role="status"><progress aria-label="Checking VISA resources"></progress> Checking VISA resources…${elapsed}${candidates.length?' Previous results are shown while checking.':''}</p>`;}
 else if(matching&&scan.state==='failed')status=`<p class="alert" role="alert">${esc(scan.message||'VISA inventory is unavailable.')}${candidates.length?' Previous results are shown.':''} Refresh or enter the resource manually in Advanced connection settings.</p>`;
 else if(!fresh)status=`<p class="hint">${candidates.length?'Previous results. ':''}Refresh VISA resources before testing, or enter the resource manually in Advanced connection settings.</p>`;
 else if(!candidates.length)status='<p class="hint">No VISA resources detected for this connection. Check the cable and refresh, or enter the resource manually in Advanced connection settings.</p>';
 else status='<p class="hint">Detected addresses are candidates. Test Connection verifies the instrument identity.</p>';
 return `<div class="visa-picker"><label>Resource${field}</label><button class="btn small" data-ui="scan-visa" ${pending||d.busy?'disabled':''}>${pending?'Checking…':'Refresh VISA resources'}</button>${status}<details id="visa-advanced" class="visa-advanced" ${d.manualResource?'open':''}><summary>Advanced connection settings</summary><div class="form-actions"><button class="btn small" data-ui="visa-manual">${d.manualResource?'Use detected resources':'Enter resource manually'}</button></div>${advanced}</details></div>`;
}
