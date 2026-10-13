import {esc,input,select,numeric,renderJob,terminal} from './data-browser.js';
import {canResume} from './control-state.js';
import {renderWaveformPlot,channelColor,timeDomainForViews,sampleRangeForTime} from './waveform-plot.js';
import {renderActivity} from './activity.js';

export function scopeAction(op,get,data={}){
 if(op==='scope-read')return {name:'read_status',args:{}};
 if(op==='scope-run'||op==='scope-stop')return {name:op.slice(6),args:{}};
 if(op==='scope-display'){
  const channel=Number(data.channel);
  if(!Number.isInteger(channel)||channel<1||channel>4||!['true','false'].includes(String(data.enabled)))throw new Error('Invalid scope channel/display request');
  return {name:'set_channel_display',args:{channel,enabled:String(data.enabled)==='true'}};
 }
 if(op==='scope-depth')return {name:'set_memory_depth',args:{points:numeric(get,'scope-depth',1,250000000,'Memory depth',true)}};
 throw new Error('Unsupported scope action');
}

export function scopeCaptureDefinition({hostId,deviceId,get,channels}){
 if(!/^[a-f0-9]{32}$/.test(hostId)||!/^[a-f0-9]{32}$/.test(deviceId))throw new Error('Select a verified scope device.');
 if(!channels?.length||channels.some(c=>!Number.isInteger(c)||c<1||c>4))throw new Error('Select at least one channel to save.');
 const acquisition='stopped',source=get('scope-source')||'memory';
 if(!['screen','memory'].includes(source))throw new Error('Select Screen or Memory.');
 if(source==='memory'&&get('scope-depth')==='auto')throw new Error('Choose a memory depth before acquiring.');
 const value=id=>get(id)||({'scope-depth':'100000','scope-frames':'1','scope-interval':'0'}[id]??''),resume=get('scope-resume');
 return {schema:1,id:crypto.randomUUID().replaceAll('-',''),name:'Waveform '+new Date().toISOString(),template_id:'waveform_capture',template_version:1,
  roles:[{role:'detector',host_id:hostId,domain_id:deviceId,channel:channels[0]}],
  parameters:{acquisition,source,memory_depth:source==='memory'?numeric(value,'scope-depth',1,250000000,'Memory depth',true):null,
   frames:numeric(value,'scope-frames',1,9999,'Frames',true),interval_ms:Math.round(numeric(value,'scope-interval',0,3600,'Frame interval')*1000),resume_after_capture:resume==null||resume===''||String(resume)==='true',channels}};
}

/** Confirmed off clears Save. Only explicit settings Sync adopts physical depth. */
export function syncScopeDraft(local,domain,bootId,ageUpper,{force=false}={}){
 const device=domain?.device;
 if(!bootId||!domain?.context?.connection_id||!['READY','STOP_HELD'].includes(domain.state)
  ||!Number.isFinite(ageUpper)||ageUpper<0||device?.connected!==true||device.previous!==false||device.quality!=='confirmed')return;
 const initial=!local.scopeCaptureInitialized;
 if(initial&&!local.scopeCaptureDirty)local.scopeCaptureChannels=(device.channels||[]).filter(c=>c.displayed===true&&Number.isInteger(Number(c.channel))&&Number(c.channel)>=1&&Number(c.channel)<=4).map(c=>Number(c.channel));
 const off=new Set((device.channels||[]).filter(c=>c.displayed===false).map(c=>Number(c.channel)));
 local.scopeCaptureChannels=(local.scopeCaptureChannels||[]).filter(n=>!off.has(n));
 local.scopeCaptureInitialized=true;local.inputs??=new Map();
 for(let n=1;n<=4;n++)local.inputs.set('scope-capture-'+n,{checked:local.scopeCaptureChannels.includes(n)});
 if(!local.inputs.has('scope-depth'))local.inputs.set('scope-depth',{value:'100000'});
 if(force){const depth=device.memory_depth||device.memory_depth_points;if(device.memory_depth_auto===true)local.inputs.set('scope-depth',{value:'auto'});else if(Number.isSafeInteger(depth)&&depth>0)local.inputs.set('scope-depth',{value:String(depth)});}
}

/** Reads native preview or saved data. Plot failure never changes capture evidence. */
export async function loadSavedScopeRecord({owner,local,recordId,onChange=()=>{}}){
 const generation=local.scopePlotGeneration=(local.scopePlotGeneration||0)+1,current=()=>local.scopePlotGeneration===generation;
 local.scopeRecordId=recordId;local.scopePlotBusy=true;local.scopePlotError=null;local.scopePlotStarted=Date.now();onChange();
 try{
  const data=await owner.dataGet({record_id:recordId});if(!current())return false;
  const record=data.record,frame=record?.frames?.[0];
  if(!frame?.channels?.length)throw new Error('The saved record has no waveform frame to display.');
  const views=await Promise.all(frame.channels.map(channel=>owner.dataViewport({record_id:recordId,frame_id:frame.id,channel:channel.channel,start:0,end:channel.points??channel.sample_count,max_points:2048})));
  if(!current())return false;
  renderWaveformPlot(views);
  local.scopeRecord=record;local.scopeViews=views;local.scopePlotRecord=record;local.scopeLoadedRecord=recordId;local.scopeHiddenChannels=[];
  local.scopeRecordSaved=data.saved!==false&&record.saved!==false;
  local.scopeIsPreview=data.preview===true||data.saved===false;
  local.scopeTimeDomain=timeDomainForViews(views);local.scopeFullTimeDomain=local.scopeTimeDomain;local.scopeZoomHistory=[];
  local.scopeSaveError=null;local.scopeReferenceCalibration=views[0]?.calibration;
  local.scopePreviewUnavailable=false;
  return true;
 }catch(cause){if(current())local.scopePlotError=cause;return false;}
 finally{if(current()){local.scopePlotBusy=false;onChange();}}
}

/** A time selection is independently mapped to each channel's calibrated samples. */
export async function zoomScopePlot({owner,local,timeDomain,back=false,onChange=()=>{}}){
 const record=local.scopePlotRecord,frame=record?.frames?.[0],full=local.scopeFullTimeDomain;
 if(!frame||!full)return false;
 const history=local.scopeZoomHistory||[],range=back?history.at(-1):timeDomain;
 if(!range)return false;
 const target={start:Math.max(full.start,range.start),end:Math.min(full.end,range.end)};
 if(!Number.isFinite(target.start)||!Number.isFinite(target.end)||target.start>=target.end)return false;
 const previous=local.scopeTimeDomain,token=local.scopePlotGeneration=(local.scopePlotGeneration||0)+1;
 local.scopePlotBusy=true;local.scopePlotStarted=Date.now();local.scopePlotError=null;onChange();
 try{
  const requests=frame.channels.flatMap(channel=>{
   const calibration=channel.calibration||local.scopeViews.find(v=>v.channel===channel.channel)?.calibration;
   const samples=sampleRangeForTime(target,calibration,channel.points??channel.sample_count);
   return samples?[{record_id:record.id,frame_id:frame.id,channel:channel.channel,...samples,max_points:2048}]:[];
  });
  if(!requests.length)throw new Error('No samples overlap this time range.');
  const views=await Promise.all(requests.map(request=>owner.dataViewport(request)));
  if(local.scopePlotGeneration!==token||local.scopeLoadedRecord!==record.id)return false;
  renderWaveformPlot(views,{timeDomain:target});
  local.scopeViews=views;local.scopeTimeDomain=target;local.scopeZoomHistory=back?history.slice(0,-1):[...history,previous].slice(-64);
  return true;
 }catch(cause){if(local.scopePlotGeneration===token)local.scopePlotError=cause;return false;}
 finally{if(local.scopePlotGeneration===token){local.scopePlotBusy=false;onChange();}}
}

export async function saveScopePreview({owner,local,name,onChange=()=>{}}){
 if(local.scopePreviewUnavailable)throw new Error('Check preview memory before saving this source.');
 const record=local.scopePlotRecord||local.scopeRecord,recordId=local.scopeLoadedRecord;
 if(!recordId||record?.id!==recordId||local.scopeSaving)return false;
 const label=String(name??'').trim();if(!label)throw new Error('Enter a record name.');
 local.scopeSaving=true;local.scopeSaveError=null;onChange();
 try{
  const reply=local.scopeRecordSaved?await owner.dataUpdate({record_id:recordId,name:label}):await owner.previewSave({record_id:recordId,name:label,comment:record.comment||''});
  if(local.scopeLoadedRecord===recordId){local.scopeRecordSaved=true;local.scopeRecord={...record,name:label};local.scopePlotRecord=local.scopeRecord;}
  return reply;
 }catch(cause){if(local.scopeLoadedRecord===recordId)local.scopeSaveError=cause;throw cause;}
 finally{local.scopeSaving=false;onChange();}
}

function errorText(error){
 let value=error;
 for(let i=0;i<6;i++){
  if(value&&typeof value==='object'){value=value.message??value.error;continue;}
  if(typeof value!=='string')return 'The waveform request failed. Open Details for the original error.';
  const json=value.replace(/^failed_after_call_started:\s*/, '');
  try{const parsed=JSON.parse(json);if(parsed&&typeof parsed==='object'){value=parsed;continue;}}catch{}
  return value;
 }
 return 'The waveform request failed. Open Details for the original error.';
}
export function scopeDepthFailure(job){
 if(!job?.error||!['failed','uncertain'].includes(job.status))return null;
 const message=errorText(job.error).toLowerCase();
 return [['memory depth preflight','Memory depth preflight failed'],['depth acquisition-state confirmation','Memory depth acquisition-state confirmation failed'],['memory depth application','Memory depth application failed'],['depth stop restoration','Memory depth STOP restoration failed']].find(([operation])=>message.includes(operation))?.[1]??null;
}
function renderScopeJob(job,acquisition,depthFailure){
 if(!job)return '';
 if(job.status==='completed')return '';
 const stages={preflight:'Checking scope',reserve:'Preparing acquisition',resume_scope:'Starting acquisition',wait_interval:'Waiting for next frame',acquire:acquisition==='existing_screen'?'Reading existing screen':'Reading stopped waveform',persist:'Preparing preview',cleanup:'Finishing request'};
 const readable={...job,stage:depthFailure||stages[job.stage]||job.stage,error:job.error?errorText(job.error):null};
 return renderJob({...readable,error:null},'scope-cancel')+(job.error?`<p class="scope-job-error">${esc(readable.error)}</p><details id="scope-run-details"><summary>Details</summary><pre>${esc(JSON.stringify(job,null,2))}</pre></details>`:'');
}
export function renderScope(state={}){
 const device=state.status?.devices?.scope,role=state.roles?.scope,cap=device?.capabilities||{},fault=role?.mode==='FAULT',captureDepthFailure=scopeDepthFailure(state.scopeRun);
 const depthRecoveryPending=Boolean(captureDepthFailure&&(!state.scopeRun?.run_id||state.scopeDepthRecoveredRunId!==state.scopeRun.run_id));
 const live=device?.connected===true&&role?.confirmed&&role.mode==='READY'&&!role.unknown&&!role.hostRestricted&&!role.normalPending&&!role.safetyPending&&!state.scopeJobActive&&!state.scopeSettingsPending;
 const current=device?.connected===true&&role?.confirmed&&!role.unknown&&!role.hostRestricted&&!fault&&!state.scopeSettingsPending&&!state.scopeSettingFailure&&!depthRecoveryPending&&device.previous===false&&device.quality==='confirmed';
 const canRead=live&&!state.scopeSettingFailure&&!depthRecoveryPending&&(current||state.scopeCaptureInitialized);
 const channels=Array.isArray(device?.channels)?device.channels:[],depths=cap.memory_depths||cap.supported_memory_depths||device?.memory_depths||[];
 const selected=state.scopeCaptureChannels??(current?channels.filter(c=>c.displayed===true).map(c=>Number(c.channel)):[]);
 const source=state.scopeSourceDraft||'memory';
 const depthDraft=state.inputs?.get('scope-depth'),depth=depthDraft?.value??depthDraft??100000,needsDepth=source==='memory'&&depth==='auto';
 const depthLabel=v=>Number(v)>=1000000?`${Number(v)/1000000}M`:Number(v)>=1000?`${Number(v)/1000}K`:String(v);
 const measuredDepth=device?.memory_depth||device?.memory_depth_points,depthReadback=device?.memory_depth_auto===true?'Auto':Number.isSafeInteger(measuredDepth)&&measuredDepth>0?depthLabel(measuredDepth):null;
 const button=(op,label,extra='',enabled=live)=>`<button class="btn" data-op="scope-${op}"${extra?' '+extra:''}${enabled?'':' disabled'}>${label}</button>`;
 const previous=device?.previous===true||device?.quality==='historical'||role?.unknown||state.scopeJobActive||state.scopeSettingsPending||state.scopeSettingFailure||depthRecoveryPending||fault;
 const readback=device?.connected||previous?`${previous?'Previous controller acquisition: ':'Controller acquisition: '}${esc(device?.acquisition||device?.acquisition_state||'Not yet read')}${depthReadback===null?'':` · ${previous?'Previous controller memory depth: ':'Controller memory depth: '}${esc(depthReadback)}`}`:'Connect the selected scope to read its settings.';
 const settingFailure=state.scopeSettingFailure,setting=settingFailure||(state.scopeSettingsPending?{action:state.scopeSettingAction}:null),settingName=setting?.action?.name;
 const settingLabels={set_memory_depth:['Applying memory depth…','Memory depth change failed'],set_channel_display:['Changing channel display…','Channel display change failed'],read_status:['Reading scope settings…','Scope settings read failed']};
 const settingLabel=(settingLabels[settingName]||['Updating scope settings…','Scope settings change failed'])[settingFailure?1:0];
 const requestedDepth=settingName==='set_memory_depth'&&Number.isSafeInteger(setting.action.args?.points)?setting.action.args.points:null;
 const settingFeedback=setting?`<p class="alert" role="status">${esc(settingLabel)}${requestedDepth===null?'':` · Requested memory depth: ${esc(depthLabel(requestedDepth))} · Last confirmed memory depth: ${esc(depthReadback??'Not yet read')}`}${settingFailure?`<br>${esc(errorText(settingFailure.error))}<br>Current scope settings are unconfirmed.`:''}</p>`:'';
 const captureParameters=state.scopeRun?.result?.run?.definition?.parameters;
 const captureRequestedDepth=captureParameters?.source==='memory'&&Number.isSafeInteger(captureParameters.memory_depth)&&captureParameters.memory_depth>0?captureParameters.memory_depth:null;
 const captureDepthFeedback=depthRecoveryPending?`<p class="alert" role="status">${captureRequestedDepth===null?'':`Requested memory depth: ${esc(depthLabel(captureRequestedDepth))} · Last confirmed memory depth: ${esc(depthReadback??'Not yet read')}<br>`}Current scope settings are unconfirmed.</p>`:'';
 const faultOutcome=settingFailure?'Scope controls are unavailable after this failure.':captureDepthFailure?'Memory depth operation outcome is uncertain.':['failed','uncertain'].includes(state.scopeRun?.status)?'Acquisition outcome is uncertain.':'Scope operation outcome is uncertain.';
 const recovery=fault?`<p class="alert" role="status">${faultOutcome} The command is not retried automatically. See Diagnostics for the original status. Disconnect preserves the scope settings and does not stop acquisition.</p>`:'';
 const resume=!state.closing&&!state.scopeJobActive&&role?.confirmed&&!role.unknown&&!role.hostRestricted&&canResume(role)?'<p class="scope-held" role="status">Controls are paused after a confirmed stop. Resume controls does not send RUN or start acquisition. <button class="btn" data-op="resume" data-role="scope">Resume controls</button></p>':'';
 const capturedRecordId=state.scopeRun?.record_ids?.[0]||state.scopeRun?.result?.record_ids?.[0],recordId=capturedRecordId||state.scopeRecordId;
 const previousPlot=Boolean(state.scopeViews?.length&&(state.scopeJobActive||state.scopeSettingsPending||settingFailure||state.scopePlotBusy||state.scopePlotError||state.scopePreviewUnavailable||state.scopeLoadedRecord!==recordId||terminal(state.scopeRun?.status)&&!capturedRecordId));
 const visible=(state.scopeViews||[]).filter(v=>!state.scopeHiddenChannels?.includes(v.channel));
 const plotSelections=state.scopeViews?.length?`<div class="scope-plot-channels" aria-label="Saved channels to plot">Plot ${state.scopeViews.map(view=>`<label><input id="scope-display-${esc(view.channel.replace('CH',''))}" type="checkbox"${state.scopeHiddenChannels?.includes(view.channel)?'':' checked'}> ${esc(view.channel)}</label>`).join('')}</div>`:'';
 const loading=state.scopePlotBusy?'<span class="scope-plot-loading" role="status"><span class="activity-spinner"></span> Loading waveform…</span>':'';
 const plotError=state.scopePlotError?`<p class="alert" role="status">Waveform preview unavailable. ${esc(errorText(state.scopePlotError))} <button class="btn small" data-wb="scope-reload" data-record="${esc(recordId)}"${state.scopeJobActive||state.scopePlotBusy?' disabled':''}>Retry preview</button></p>`:'';
 const saveReady=!state.scopePreviewUnavailable&&Boolean(state.scopeLoadedRecord&&state.scopePlotRecord?.id===state.scopeLoadedRecord||state.scopeRecord?.id===state.scopeLoadedRecord);
 const savedActions=recordId?`<div class="scope-record-actions">${input('scope-record-name','Record name',state.scopePlotRecord?.name||state.scopeRecord?.name||'Waveform')}<button class="btn primary" data-wb="scope-save"${saveReady&&!state.scopeSaving&&!state.scopeJobActive?'':' disabled'}>${state.scopeSaving?'Saving…':'Save record'}</button><button class="btn" data-wb="scope-export" data-record="${esc(state.scopeLoadedRecord||recordId)}"${saveReady&&!state.scopeExporting&&state.scopeExportJob?.status!=='uncertain'?'':' disabled'}>${state.scopeExporting?'Exporting…':'Export Data'}</button><span class="scope-save-state" role="status">${saveReady?state.scopeRecordSaved?'Saved':'Unsaved':''}</span></div>`:'';
 const saveError=recordId&&state.scopeSaveError?`<p class="alert" role="alert">${esc(errorText(state.scopeSaveError))}</p>`:'';
 const exportFeedback=state.scopeExportJob&&(!terminal(state.scopeExportJob.status)||['failed','uncertain'].includes(state.scopeExportJob.status))?renderJob(state.scopeExportJob,'scope-export-cancel'):'';
 const memoryRecovery=state.scopePreviewBudgetBlocked&&state.scopeIsPreview&&!state.scopeRecordSaved?'<p class="alert" role="status">The previous unsaved waveform occupies the preview memory. Save record, or discard it before acquiring again. <button class="btn warn" data-wb="scope-discard">Discard unsaved preview</button></p>':'';
 const releaseError=state.scopePreviewReleaseError?`<p class="alert" role="status">Preview memory release is unconfirmed. Saved Records are not deleted. ${esc(errorText(state.scopePreviewReleaseError.cause))} <button class="btn" data-wb="scope-memory-check"${state.scopePreviewReleaseChecking?' disabled':''}>${state.scopePreviewReleaseChecking?'Checking…':'Check preview memory'}</button>${state.scopePreviewReleaseError.reconciled?'<button class="btn" data-wb="scope-memory-retry">Retry memory release</button>':''}</p>`:'';
 const dataFeedback=plotError+saveError+exportFeedback+releaseError;
 const dataStatus=dataFeedback?`<div id="scope-data-status" class="scope-data-status" role="region" aria-label="Waveform data status">${dataFeedback}</div>`:'';
 const rows=Array.from({length:4},(_,i)=>{const n=i+1,enabled=channels.find(c=>Number(c.channel)===n)?.displayed,known=enabled===true||enabled===false,on=known&&enabled===true,off=known&&enabled===false,confirmed=known&&current;
  const channelState=known?`${on?'On':'Off'}${confirmed?'':' · Previous'}`:'Unknown';
  const channelActionLabel=!known?'Unknown':!confirmed?`${on?'On':'Off'} (previous)`:`${on?'On; turn Off':'Off; turn On'}`;
  return `<div class="scope-channel-row ${on?'on':off?'off':'unknown'}" data-channel="${n}" style="--channel-color:${channelColor('CH'+n)}"><button id="scope-channel-${n}" class="scope-channel-name" data-op="scope-display" data-channel="${n}" data-enabled="${enabled!==true}" aria-label="CH${n} instrument display ${channelActionLabel}"${live&&confirmed?'':' disabled'}>CH${n}</button><span class="scope-channel-state">${channelState}</span><label><input id="scope-capture-${n}" type="checkbox"${selected.includes(n)&&!off?' checked':''}${off||!confirmed||state.scopeJobActive?' disabled':''}> Save</label></div>`;}).join('');
 const syncError=state.scopeSyncError?`<p id="scope-sync-error" class="alert" role="status">Waveform received; settings could not be refreshed. ${esc(errorText(state.scopeSyncError))}</p>`:'';
 const controls=`<aside class="card scope-read-card"><div class="card-head"><h2>Channels</h2></div><div class="card-body"><div class="scope-save-bar"><div class="scope-channel-list" aria-label="Instrument channels and channels to save">${rows}</div><button class="btn primary" data-wb="scope-collect"${canRead&&!state.scopePlotBusy&&!needsDepth?'':' disabled'}>Acquire</button></div></div></aside>`;
 const plot=`<section class="card scope-workbench"><div class="card-head"><h2>${previousPlot?'Previous waveform':'Waveform'}</h2>${plotSelections}</div><div class="card-body"><div class="scope-waveform">${renderWaveformPlot(visible,{timeDomain:state.scopeTimeDomain,referenceCalibration:state.scopeReferenceCalibration,clipId:'scope-waveform-clip'})}${loading}</div>${savedActions}</div></section>`;
 const options=(depths.length?depths:[100000]).map(v=>[v,depthLabel(v)]);if(depth==='auto')options.unshift(['auto','Auto']);
 const depthControl=select('scope-depth','Memory depth',options,depth,live&&source==='memory'?'':'disabled').replace('<option value="auto" selected>','<option value="auto" selected disabled>');
 const activity=state.scopeActivity?.ended===undefined||['failed','unknown'].includes(state.scopeActivity?.outcome)?renderActivity(state.scopeActivity):'';
 const status=`<div id="scope-status" class="scope-status" role="region" aria-label="Oscilloscope status"><p class="scope-readback hint">${readback}</p>${renderScopeJob(state.scopeRun,state.scopeAcquisition,captureDepthFailure)}${activity}${state.scopeRecoveryMarkup||''}${settingFeedback}${captureDepthFeedback}${recovery}${resume}${syncError}${memoryRecovery}${needsDepth?'<p class="hint">Choose a memory depth in Advanced before acquiring.</p>':''}</div>`;
 return `<div class="page-header scope-page-header"><h1>Oscilloscope</h1>${button('read','Sync')}</div><div class="scope-layout">${controls}${plot}${status}${dataStatus}</div><details id="scope-advanced" class="card scope-advanced"><summary>Advanced</summary><div class="card-body"><div class="workbench-fields">${select('scope-source','Source',[['memory','Memory'],['screen','Screen']],source)}${depthControl}${input('scope-frames','Frames',1,'type="number" min="1" max="9999"')}${input('scope-interval','Interval between frames (s)',0,'type="number" min="0" max="3600" step="0.1"')}<label class="scope-resume"><input id="scope-resume" type="checkbox"${state.inputs?.get('scope-resume')?.checked===false?'':' checked'}> Resume after transfer</label></div><p class="hint">Changing depth refreshes the stopped buffer. Unsaved previews are temporary. Save record adds them to Record/Data.</p></div></details>`;
}
