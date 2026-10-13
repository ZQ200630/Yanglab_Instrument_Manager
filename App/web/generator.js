import {input,select,numeric} from './data-browser.js';
import {renderActivity} from './activity.js';

const parts=['waveform','frequency','amplitude','offset','phase','load','polarity'];
export const generatorDraftIds=[1,2].flatMap(n=>parts.map(part=>`generator-${n}-${part}`));
export function generatorActionFields(action){
 const n=action.args?.channel;
 const fields={configure_signal:['waveform','frequency','amplitude','offset'],set_phase:['phase'],set_load:['load'],set_polarity:['polarity']};
 return action.name==='read_status'?generatorDraftIds:(fields[action.name]||[]).map(part=>`generator-${n}-${part}`);
}
// Only confirmed readback can seed controls. Unsubmitted edits belong to this connection.
export function syncGeneratorDraft(local,domain,bootId,{forceIds=[]}={}){
 const binding=JSON.stringify([bootId,domain?.context?.session_id,domain?.context?.connection_id]);
 local.inputs??=new Map();local.generatorDraftDirty??=new Set();
 if(local.generatorDraftBinding!==binding){
  for(const id of generatorDraftIds)local.inputs.delete(id);
  local.generatorDraftDirty.clear();local.generatorDraftBinding=binding;
 }
 const d=domain?.device;
 if(!domain?.context?.connection_id||d?.quality!=='confirmed'||d.previous!==false)return;
 for(const id of forceIds)local.generatorDraftDirty.delete(id);
 for(const c of d.channels||[]){
  if(![1,2].includes(Number(c.channel)))continue;
  const values={waveform:c.waveform,frequency:c.frequency_hz,amplitude:c.amplitude_vpp,offset:c.offset_v,phase:c.phase_deg,load:c.load?.kind==='high_z'?'high_z':c.load?.ohms,polarity:c.polarity};
  for(const [part,value]of Object.entries(values)){
   const id=`generator-${c.channel}-${part}`;
   if(!local.generatorDraftDirty.has(id)&&value!==null&&value!==undefined&&(typeof value!=='number'||Number.isFinite(value)))local.inputs.set(id,{value:String(value)});
  }
 }
}
export function generatorAction(op,get,data={}){if(op==='generator-read')return {name:'read_status',args:{}};const channel=Number(data.channel);if(!Number.isInteger(channel)||channel<1||channel>2)throw new Error('Invalid generator channel');const key=part=>`generator-${channel}-${part}`;
 if(op==='generator-output-toggle'){
  const d=data.device,enabled=d?.channels?.find(c=>Number(c.channel)===channel)?.output_enabled;
  if(d?.connected!==true||d.quality!=='confirmed'||d.previous!==false||typeof enabled!=='boolean')throw new Error('Sync the generator for confirmed output readback first.');
  return {name:'set_output',args:{channel,enabled:!enabled}};
 }
 if(op==='generator-output-on'||op==='generator-output-off')return {name:'set_output',args:{channel,enabled:op.endsWith('-on')}};
 if(op==='generator-apply'){const waveform=get(key('waveform'));if(!['sine','square','triangle','ramp','pulse','noise','dc'].includes(waveform))throw new Error('Unsupported waveform');return {name:'configure_signal',args:{channel,waveform,frequency_hz:numeric(get,key('frequency'),Number.MIN_VALUE,250000000,'Frequency'),amplitude_vpp:numeric(get,key('amplitude'),0,20,'Amplitude'),offset_v:numeric(get,key('offset'),-10,10,'Offset')}};}
 if(op==='generator-phase')return {name:'set_phase',args:{channel,degrees:numeric(get,key('phase'),0,360,'Phase')}};
 if(op==='generator-load'){const value=get(key('load'));return {name:'set_load',args:{channel,load:value==='high_z'?{kind:'high_z'}:{kind:'ohms',ohms:numeric(get,key('load'),1,10000,'Load')}}};}
 if(op==='generator-polarity'){const polarity=get(key('polarity'));if(!['normal','inverted'].includes(polarity))throw new Error('Invalid polarity');return {name:'set_polarity',args:{channel,polarity}};}throw new Error('Unsupported generator action');
}
export function renderGenerator(state={}){
 const d=state.status?.devices?.generator,r=state.roles?.generator,cap=d?.capabilities||{};
 const available=d?.connected===true&&r?.confirmed&&!r.hostRestricted&&!r.unknown&&!r.normalPending&&!r.safetyPending&&!state.generatorSettingsPending;
 const live=available&&r.mode==='READY',held=available&&r.mode==='STOP_HELD';
 const confirmed=d?.connected===true&&r?.confirmed&&!r.unknown&&!r.hostRestricted&&d.quality==='confirmed'&&d.previous===false,known=Array.isArray(d?.channels)&&d.channels.length>0;
 const waveforms=cap.waveforms||['sine','square','triangle','ramp','pulse','noise','dc'];
 const button=(op,label,n,style='')=>`<button class="btn ${style}" data-op="generator-${op}"${n?` data-channel="${n}"`:''}${(live||held&&['read','output-off'].includes(op))&&(op==='read'||known)?'':' disabled'}>${label}</button>`;
 const resume=r?.mode==='STOP_HELD'?`<p class="hint">Controls are held. Sync to verify the instrument, then <button class="btn" data-op="resume"${held?'':' disabled'}>Resume controls</button>.</p>`:'';
 const activity=state.generatorActivity;
 const feedback=state.generatorRecoveryMarkup||(activity&&(activity.ended===undefined||activity.outcome!=='complete')?renderActivity(activity):'');
 const status=feedback+resume;
 return `<div class="page-header generator-page-header"><h1>Function Generator</h1>${button('read','Sync')}</div>${status?`<div class="generator-status" role="status">${status}</div>`:''}<div class="generator-channels">${Array.from({length:Math.min(2,cap.channels||cap.channel_count||2)},(_,i)=>{
  const n=i+1,c=d?.channels?.find(c=>Number(c.channel)===n)||{},signal=c.signal||c;
  const previous=known&&!confirmed?' · Previous':'';
  const loads=[['high_z','High Z'],['50','50 Ω']];if(c.load?.kind==='ohms'&&Number.isFinite(c.load.ohms)&&c.load.ohms!==50)loads.push([String(c.load.ohms),`${c.load.ohms} Ω`]);
  const outputKnown=typeof c.output_enabled==='boolean';
  const canToggle=confirmed&&outputKnown&&(live||held&&c.output_enabled===true);
  const outputDescription=!outputKnown?'Output state is not synced.':confirmed?'Confirmed output readback.':'Previous output readback; Sync to verify the current state.';
  const output=`<button type="button" class="btn generator-output" role="switch" aria-label="CH${n} output" aria-description="${outputDescription}"${outputKnown?` aria-checked="${c.output_enabled}"`:''} data-op="generator-output-toggle" data-channel="${n}"${canToggle?'':' disabled'}><span class="generator-switch-track" aria-hidden="true"></span><span>${c.output_enabled===true?'Output on':c.output_enabled===false?'Output off':'Not synced'}${previous}</span></button>`;
  return `<section class="card"><div class="card-head"><h2>CH${n}</h2>${output}</div><div class="card-body"><div class="workbench-fields">${select(`generator-${n}-waveform`,'Waveform',known?waveforms:[['','Not synced'],...waveforms],signal.waveform||'')}${input(`generator-${n}-frequency`,'Frequency (Hz)',signal.frequency_hz??'','type="number" min="0" step="any"')}${input(`generator-${n}-amplitude`,'Amplitude (Vpp)',signal.amplitude_vpp??'','type="number" min="0" step="any"')}${input(`generator-${n}-offset`,'Offset (V)',signal.offset_v??'','type="number" step="any"')}</div><div class="actions">${button('apply','Apply',n,'primary')}</div><details id="generator-${n}-advanced"><summary>Advanced</summary><div class="workbench-fields">${cap.phase||cap.supports_phase?input(`generator-${n}-phase`,'Phase (deg)',c.phase_deg??'','type="number" step="any"')+button('phase','Set phase',n):''}${cap.load||cap.supports_load?select(`generator-${n}-load`,'Output load',loads,c.load?.kind==='ohms'?String(c.load.ohms):'high_z')+button('load','Set load',n):''}${cap.polarity||cap.supports_polarity?select(`generator-${n}-polarity`,'Polarity',['normal','inverted'],c.polarity||'normal')+button('polarity','Set polarity',n):''}</div><p class="hint">Output controls affect this channel's waveform connector. Sync outputs and other panel settings are preserved.</p></details></div></section>`;
 }).join('')}</div>`;
}
