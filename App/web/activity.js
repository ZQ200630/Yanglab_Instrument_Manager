/** Presentation only. An elapsed timer never proves device progress or release. */
const esc=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const labels={prepare:'Preparing…',authority:'Connecting…',sync:'Checking status…',manifest:'Loading capture…',download:'Downloading spectrum…',verify:'Verifying spectrum…',export:'Choose folder / exporting…',disconnect:'Disconnecting…',history:'Loading saved captures…'};
let serial=0;
export function startActivity(kind,phase,now=performance.now()) {return {id:++serial,kind,phase,started:now,phaseStarted:now,timings:{},progress:null};}
export function advanceActivity(work,phase,now=performance.now(),progress=null) {
 if(!work||work.ended!==undefined)return work;
 const changed=phase!==work.phase;
 return {...work,phase,phaseStarted:changed?now:work.phaseStarted,progress,
  timings:changed?{...work.timings,[work.phase]:(work.timings[work.phase]||0)+Math.max(0,now-work.phaseStarted)}:work.timings};
}
export function finishActivity(work,outcome='complete',now=performance.now()) {
 if(!work||work.ended!==undefined)return work;
 return {...work,ended:now,outcome,progress:null,timings:{...work.timings,[work.phase]:(work.timings[work.phase]||0)+Math.max(0,now-work.phaseStarted)}};
}
export function activityBusy(work) {return Boolean(work&&work.ended===undefined);}
function label(work) {
 if(work.ended!==undefined)return {unknown:'Outcome unknown — check status',failed:'Operation failed',cancelled:'Export cancelled'}[work.outcome]||
  (work.kind==='load'?'Spectrum ready':work.kind==='export'?'Capture exported':work.kind==='disconnect'?'Release check finished':'Operation finished');
 if(work.phase==='instrument')return ['read_trace','acquire'].includes(work.kind)?'Reading & saving…':work.kind==='connect'?'Opening instrument…':'Waiting for instrument…';
 return labels[work.phase]||'Working…';
}
const elapsed=(work,now)=>`${Math.floor(Math.max(0,(work.ended??now)-work.started)/1000)} s`;
export function renderActivity(work,now=performance.now()) {
 if(!work)return '';const busy=activityBusy(work),p=work.progress;
 const known=busy&&work.phase==='download'&&Number.isSafeInteger(p?.received)&&Number.isSafeInteger(p?.total)&&p.total>0&&p.received>=0&&p.received<=p.total;
 return `<div class="operation-feedback" role="status" aria-live="polite" aria-atomic="true">${busy?'<span class="activity-spinner" aria-hidden="true"></span>':''}<strong>${esc(label(work))}</strong><span class="activity-elapsed" aria-hidden="true" data-activity-elapsed="${work.started}" data-activity-ended="${work.ended??''}">${elapsed(work,now)}</span>${known?`<progress value="${p.received}" max="${p.total}" aria-label="Received spectrum bytes"></progress><small>${Math.floor(p.received/p.total*100)}%</small>`:''}${busy?`<small data-activity-slow="${work.started}"${now-work.started<5000?' hidden':''}>Still working · You can keep browsing.</small>`:''}</div>`;
}
/** Clock ticks touch only text/visibility, leaving inputs, plot and focus intact. */
export function tickActivities(root,now=performance.now()) {
 for(const node of root.querySelectorAll('[data-activity-elapsed]')) {
  const started=Number(node.dataset.activityElapsed),ended=node.dataset.activityEnded;
  node.textContent=elapsed({started,...(ended!==''?{ended:Number(ended)}:{})},now);
 }
 for(const node of root.querySelectorAll('[data-activity-slow]'))node.hidden=now-Number(node.dataset.activitySlow)<5000;
}
