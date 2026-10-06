import {esc} from './panels.js';import {deviceKey,routeFor} from './routes.js';import {instrumentTarget} from './setup.js';
import {renderInstrumentIcon} from './instrument-icon.js';
export function navigateTag(d,setups=[]){return {deviceId:d.device_id,href:routeFor(d.hostId,instrumentTarget(d,setups))};}
export function overviewCounts(devices,setups,hosts){const count=source=>{const rows=devices.filter(d=>(d.source||'LOCAL')===source);return {total:rows.length,online:rows.filter(d=>d.communication==='ONLINE').length}};return {local:count('LOCAL'),remote:count('REMOTE')};}
export function communication(d,state,host,store){
  if(!host?.connected)return 'OFFLINE';
  const sample=state?.host_sample_ms,age=store.ageUpperMs(host.host_id,sample);
  if(state?.device?.connected===true && Number.isFinite(age) && age<5000 && !state.device.status_error)return 'ONLINE';
  if(state?.availability?.communication==='ONLINE'){
    const observed=store.ageUpperMs(host.host_id,state.availability.last_success_ms);
    if(Number.isFinite(observed)&&observed<90000)return 'ONLINE';
  }
  return 'UNKNOWN';
}
export function renderOverview(host,store){
  const hosts=store.hosts?.()||[host].filter(Boolean);
  if(host&&!hosts.some(h=>h.host_id===host.host_id))hosts.unshift(host);
  const rows=hosts.flatMap(owner=>{const registry=owner.registry||{devices:[],setups:[]};return registry.devices.map(d=>{
    const target=instrumentTarget(d,registry.setups),state=store.get(deviceKey(owner.host_id,target));
    const age=owner.connected?store.ageUpperMs(owner.host_id,state?.host_sample_ms):null;
    return {...d,hostId:owner.host_id,owner,source:owner.remote?'REMOTE':'LOCAL',communication:communication(d,state,owner,store),state,target,ageLabel:Number.isFinite(age)?`${(age/1000).toFixed(1)} s`:'Unknown'};
  });});
  const counts=overviewCounts(rows,[],hosts);
  return `<div class="page-header"><h1>Instrument overview</h1></div><div class="overview-summary"><section class="card summary-card"><span class="eyebrow">Local Devices</span><strong>${counts.local.online} online <small>/ ${counts.local.total} configured</small></strong><span>${esc(host?.host_name||'Local computer')} · ${host?.connected?'Host online':'Host offline'}</span></section><section class="card summary-card"><span class="eyebrow">Remote Devices</span><strong>${counts.remote.online} online <small>/ ${counts.remote.total} configured</small></strong><span>${hosts.filter(h=>h.remote&&h.connected).length} Host(s) connected</span></section></div>
    <div class="section-title"><h2>Instrument status</h2></div><div class="instrument-grid">${rows.map(d=>{const href=routeFor(d.hostId,d.target),state=d.state;return `<article class="card status-card"><div class="card-head"><a class="instrument-name instrument-identity" href="${href}">${renderInstrumentIcon(d.model_id)}<span>${esc(d.name)}</span></a><div class="tags"><a class="badge" href="${href}">${d.source}</a><a class="badge ${d.communication==='ONLINE'?'ready':'warn'}" href="${href}">${d.communication}</a></div></div><div class="card-body"><div><strong>${state?.context?.connection_id?'Session connected':'Session closed'}</strong><small>${esc(d.owner.host_name||d.hostId)} · ${esc(d.params?.resource||d.params?.port||'')}</small><small>${state?.availability?.detected===true?'Detected · ':''}${!d.owner.connected||state?.communication==='UNKNOWN'?'Freshness Unknown':state?.device?.state||'No current reading'}</small><small>Sample age: ${d.ageLabel}</small><small>${d.owner.control?.[d.target.kind+':'+d.target.id]?.controller_session?'Control held':'No controller'}</small><a class="btn small" href="${href}">Open Instrument</a></div></div></article>`}).join('')||'<div class="empty-panel">No instruments configured.</div>'}</div>`;
}
