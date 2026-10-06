import {esc} from './panels.js';
export const connectionState=()=>({tab:'this',add:false,draft:{endpoint:'',name:'',listener:''},request:null,busy:false,polling:false});
export function connectionEndpoint(value,port=9443){
  const input=String(value).trim();let address,chosen=String(port),v6=false;
  const bracket=input.match(/^\[([^\]]+)\](?::(\d{1,5}))?$/),ipv4=input.match(/^((?:\d{1,3}\.){3}\d{1,3})(?::(\d{1,5}))?$/);
  if(bracket){address=bracket[1];chosen=bracket[2]||chosen;v6=true;}
  else if(ipv4){address=ipv4[1];chosen=ipv4[2]||chosen;}
  else if(/^[\da-fA-F:]+$/.test(input)&&input.includes(':')){address=input;v6=true;}
  else throw new Error('Use an explicit Tailscale or loopback IP address.');
  if(!/^\d{1,5}$/.test(chosen)||Number(chosen)<1||Number(chosen)>65535)throw new Error('Port must be 1–65535.');
  if(v6){let canonical;try{canonical=new URL(`http://[${address}]/`).hostname.toLowerCase();}catch{throw new Error('Invalid IPv6 address.');}
    if(canonical!=='[::1]'&&!canonical.startsWith('[fd7a:115c:a1e0:'))throw new Error('Only loopback or Tailscale IPs are allowed.');
    return `${canonical}:${Number(chosen)}`;
  }
  const parts=address.split('.');if(parts.some(p=>! /^(0|[1-9]\d{0,2})$/.test(p)||Number(p)>255))throw new Error('Invalid IPv4 address.');
  const numbers=parts.map(Number);if(numbers[0]!==127&&!(numbers[0]===100&&numbers[1]>=64&&numbers[1]<128))throw new Error('Only loopback or Tailscale IPs are allowed.');
  return `${address}:${Number(chosen)}`;
}
const details=(id,body)=>`<details class="connection-details" id="connection-details-${esc(id)}"><summary>Details</summary>${body}</details>`;
const comparison=(number)=>`<div class="pair-comparison" aria-label="Comparison number">${esc(number)}</div>`;
function requestCard(request,busy){if(!request)return '';const waiting=request.phase==='Waiting';return `<article class="connection-request" role="status"><div class="connection-card-top"><strong>${waiting?(request.comparison?'Waiting for approval':'Connecting…'):esc(request.phase)}</strong>${waiting?`<span class="badge">${Math.ceil((request.expires_in_ms||0)/1000)}s</span>`:''}</div>
  ${request.comparison?`${comparison(request.comparison)}<p class="hint">Compare this number on both computers, then approve on the other PC.</p>`:''}
  ${request.error?`<p>${esc(request.error.message)}</p>`:''}${request.possible_owner_authorization?'<p class="warning">The owner may still have an authorization. Check its computer Details and revoke it if needed.</p>':''}
  ${waiting?`<button class="btn" data-ui="remote-cancel" ${busy?'disabled':''}>Cancel request</button>`:''}</article>`;}
export function renderConnections(remote={},state=connectionState()){
  const owner=remote.owner,local=Boolean(remote.localConnected??owner)&&!remote.networkOnly,on=owner?.transport?.state==='LISTENING',waiting=state.request?.phase==='Waiting';
  return `<section class="card settings-section connections"><div class="card-head"><h2>Connections</h2></div><div class="card-body">
  <div class="connection-tabs" role="tablist" aria-label="Computer connections">${[['this','Control this PC'],['other','Control other PCs']].map(([tab,label])=>`<button class="connection-tab" id="connections-tab-${tab}" role="tab" aria-selected="${state.tab===tab}" aria-controls="connections-panel-${tab}" tabindex="${state.tab===tab?'0':'-1'}" data-ui="connections-tab" data-tab="${tab}">${label}</button>`).join('')}</div>
  <div role="tabpanel" id="connections-panel-${state.tab}" aria-labelledby="connections-tab-${state.tab}">${state.tab==='this'?`
    <article class="connection-card"><div class="connection-card-top"><div><h3>Allow connections</h3><p class="hint">${remote.networkOnly?'Unavailable in a network-only profile':esc(owner?.listener||'Configure this computer’s address')}</p></div><span class="badge">${esc(owner?.transport?.state||'DISABLED')}</span></div>
    <details id="remote-access" class="connection-details" ${owner?.listener?'':'open'}><summary>Listener settings</summary><label for="remote-listener">Tailscale IP and port</label><input class="control" id="remote-listener" placeholder="100.x.x.x:9443" value="${esc(state.draft.listener||owner?.listener||'')}" ${local?'':'disabled'}><div class="form-actions"><button class="btn primary" data-ui="remote-enable" ${local?'':'disabled'}>Allow connections</button><button class="btn warn" data-ui="remote-disable" ${owner?.listener&&local?'':'disabled'}>Disable</button></div></details>
    ${owner?.fingerprint?details('local',`<p>Host ID <code>${esc(owner.host_id)}</code></p><p>Certificate <code>${esc(owner.fingerprint)}</code></p>`):''}</article>
    ${(owner?.pending||[]).map(p=>`<article class="connection-request"><div class="connection-card-top"><div><h3>${esc(p.name)}</h3><p class="hint">${esc(p.source_ip||'Legacy request')}</p></div><span class="badge">${Math.ceil((p.expires_in_ms||0)/1000)}s</span></div>${p.comparison?`${comparison(p.comparison)}<p class="hint">Compare with the requesting PC before approving.</p>`:'<p class="hint">Legacy pairing request.</p>'}<div class="form-actions"><button class="btn primary" data-ui="remote-approve" data-peer="${esc(p.id)}" ${!on||state.busy?'disabled':''}>Approve</button>${p.comparison?`<button class="btn" data-ui="remote-reject" data-peer="${esc(p.id)}" ${state.busy?'disabled':''}>Reject</button>`:''}</div></article>`).join('')}
    <h3 class="connections-subtitle">Authorized computers</h3>${(owner?.peers||[]).map(p=>`<article class="connection-card"><h3>${esc(p.name)}</h3><span class="connection-label">Authorized</span>${details(p.id,`<p>Computer ID <code>${esc(p.id)}</code></p><button class="btn warn" data-ui="remote-revoke" data-peer="${esc(p.id)}">Revoke access</button>`)}</article>`).join('')||'<p class="hint">No authorized computers.</p>'}
  `:`<div class="connections-toolbar"><h3>Paired computers</h3><button class="btn primary" data-ui="remote-add" ${waiting||state.busy?'disabled':''}>＋ Add PC</button></div>
    ${(remote.peers||[]).map(p=>`<article class="connection-card"><div class="connection-card-top"><div><h3>${esc(p.name)}</h3><p class="hint">${esc(p.endpoint)}</p></div><div class="connection-actions"><span class="connection-label">Paired</span><span class="badge">${p.connected?'ONLINE':'OFFLINE'}</span><button class="btn ${p.connected?'':'primary'}" data-ui="remote-${p.connected?'disconnect':'connect'}" data-host="${esc(p.host_id)}">${p.connected?'Disconnect':'Connect'}</button></div></div>${details(p.host_id,`<p>Host ID <code>${esc(p.host_id)}</code></p><p>Certificate <code>${esc(p.fingerprint||'')}</code></p><button class="btn warn" data-ui="remote-forget" data-host="${esc(p.host_id)}">Forget computer</button>`)}</article>`).join('')||'<p class="hint">No paired computers.</p>'}
    ${state.add?`<article class="connection-card connection-add"><h3>Add PC</h3><label for="remote-endpoint">Tailscale IP</label><input class="control" id="remote-endpoint" placeholder="100.x.x.x" value="${esc(state.draft.endpoint)}" ${waiting||state.busy?'disabled':''}><p class="hint">Default port: 9443</p><details id="remote-nickname" class="connection-details"><summary>Details</summary><label for="remote-name">Nickname (optional)</label><input class="control" id="remote-name" maxlength="128" value="${esc(state.draft.name)}" ${waiting||state.busy?'disabled':''}></details><div class="form-actions"><button class="btn primary" data-ui="remote-request" ${waiting||state.busy?'disabled':''}>Request connection</button><button class="btn" data-ui="remote-add-cancel" ${waiting||state.busy?'disabled':''}>Cancel</button></div></article>`:''}
    ${requestCard(state.request,state.busy)}
  `}</div></div></section>`;
}
