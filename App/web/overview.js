import {esc} from './panels.js';import {deviceKey,routeFor} from './routes.js';import {instrumentTarget} from './setup.js';
import {renderInstrumentIcon} from './instrument-icon.js';
export function navigateTag(d,setups=[]){return {deviceId:d.device_id,href:routeFor(d.hostId,instrumentTarget(d,setups))};}
export function overviewCounts(devices,setups,hosts){const count=source=>{const rows=devices.filter(d=>(d.source||'LOCAL')===source);return {total:rows.length,online:rows.filter(d=>d.communication==='ONLINE').length}};return {local:count('LOCAL'),remote:count('REMOTE')};}
export function communication(d,state,host,store){
  if(!host?.connected)return 'UNKNOWN';
  const sample=state?.host_sample_ms,age=store.ageUpperMs(host.host_id,sample);
  if(state?.device?.connected===true && Number.isFinite(age) && age<5000 && !state.device.status_error)return 'ONLINE';
  if(state?.availability?.communication==='ONLINE'){
    const observed=store.ageUpperMs(host.host_id,state.availability.last_success_ms);
    if(Number.isFinite(observed)&&observed<90000)return 'ONLINE';
  }
  return 'UNKNOWN';
}
export function renderOverview(host,store){const registry=host?.registry||{devices:[],setups:[]};
  const rows=registry.devices.map(d=>{const target=instrumentTarget(d,registry.setups),state=store.get(deviceKey(host.host_id,target)),age=host?.connected?store.ageUpperMs(host.host_id,state?.host_sample_ms):null;return {...d,source:'LOCAL',communication:communication(d,state,host,store),state,target,ageLabel:Number.isFinite(age)?`${(age/1000).toFixed(1)} s`:'Unknown'}});
  const counts=overviewCounts(rows,registry.setups,[]);
  return `<div class="page-header"><h1>Instrument overview</h1></div><div class="overview-summary"><section class="card summary-card"><span class="eyebrow">Local Devices</span><strong>${counts.local.online} online <small>/ ${counts.local.total} configured</small></strong><span>${esc(host?.host_name||'Local computer')} · ${host?.connected?'Host online':'Host offline'}</span></section><section class="card summary-card"><span class="eyebrow">Remote Devices</span><strong>0 connected</strong><span>Remote connections are not available in this local-only release.</span></section></div>
    <div class="section-title"><h2>Instrument status</h2></div><div class="instrument-grid">${rows.map(d=>{const href=routeFor(host.host_id,d.target),state=d.state;return `<article class="card status-card"><div class="card-head"><a class="instrument-name instrument-identity" href="${href}">${renderInstrumentIcon(d.model_id)}<span>${esc(d.name)}</span></a><div class="tags"><a class="badge" href="${href}">LOCAL</a><a class="badge ${d.communication==='ONLINE'?'ready':'warn'}" href="${href}">${d.communication}</a></div></div><div class="card-body"><div><strong>${state?.context?.connection_id?'Session connected':'Session closed'}</strong><small>${esc(d.model_id)} · ${esc(d.params.resource||d.params.port||'')}</small><small>${state?.availability?.detected===true?'Detected · ':''}${state?.communication==='UNKNOWN'?'Freshness Unknown':state?.device?.state||'No current reading'}</small><small>Sample age: ${d.ageLabel}</small><small>${host?.control?.[d.target.kind+':'+d.target.id]?.controller_session?'Control held':'No controller'}${d.target.kind==='setup'?' · Fiber setup':''}</small><a class="btn small" href="${href}">Open Instrument</a></div></div></article>`}).join('')||'<div class="empty-panel">No instruments configured.</div>'}</div>`;
}
