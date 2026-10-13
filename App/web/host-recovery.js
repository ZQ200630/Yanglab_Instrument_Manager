const esc=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));

export function hostRecoveryAvailable(host){
 if(host?.remote)return false;
 const cleanup=host?.worker_cleanup,exit=cleanup?.process_exit;
 const stopped=cleanup?.resource_released===true&&exit?.confirmed===true&&exit?.success===true;
 return Boolean(exit?.confirmed===true&&!stopped||!host?.connected&&['OwnershipUnknown','WorkerRecoveryRequired'].includes(host?.hostIssue?.code));
}
export function hostCleanupUnconfirmed(host){
 if(host?.remote)return false;
 const cleanup=host?.worker_cleanup,stop=host?.hostStopReport;
 const attempted=Boolean(cleanup?.active_attempt||cleanup?.attempts?.length||cleanup?.received_cleanup_report||cleanup?.process_exit||cleanup?.error);
 return hostRecoveryAvailable(host)||Boolean(attempted&&cleanup.resource_released!==true||stop&&
  (stop.resource_released!==true||stop.process_exit?.confirmed!==true||stop.process_exit?.success!==true));
}

const confirmation='Recover this Local Host? The old Worker must have exited; native recovery will verify this before proceeding. All clients will disconnect and unsaved waveform previews will be lost. Instrument and output state remain unverified. Old commands are not replayed. Reconnect and complete the required read-only verification before any instrument action.';

export function createHostRecovery({client,host,confirm,disconnect,clearConnection,reconnect,onChange=()=>{},timeoutMs=30000}){
 const state={busy:false,phase:null,started:null,ended:null,error:null,receipt:null,previousCleanup:null,previousHostIssue:null};
 async function bounded(action){
  let timer;
  try{return await Promise.race([Promise.resolve().then(action),new Promise((_,reject)=>{timer=setTimeout(()=>reject(Object.assign(new Error('Host recovery outcome is unconfirmed. No request was replayed; review the Host before retrying.'),{outcomeUnknown:true})),timeoutMs);})]);}
  finally{clearTimeout(timer);}
 }
 return {state,async run(){
  if(state.busy)return null;
  if(!hostRecoveryAvailable(host()))throw new Error('Host recovery is not available without confirmed Worker exit or a disconnected ownership issue.');
  if(!confirm(confirmation))return null;
  Object.assign(state,{busy:true,phase:'recovering',started:performance.now(),ended:null,error:null,receipt:null,previousCleanup:structuredClone(host()?.worker_cleanup??state.previousCleanup),previousHostIssue:structuredClone(host()?.hostIssue??state.previousHostIssue)});onChange(state);
  try{
   const receipt=await bounded(()=>client.recoverHost());
   if(receipt?.recovery_authorized!==true||receipt.host_stopped!==true||!/^[0-9a-f]{32}$/.test(receipt.incident_id||''))throw new Error('Host recovery authorization is unconfirmed. The existing connection was retained.');
   state.receipt=structuredClone(receipt);state.phase='disconnecting';onChange(state);
   await bounded(disconnect);clearConnection();
   state.phase='reconnecting';onChange(state);await bounded(reconnect);
   state.phase='complete';return receipt;
  }catch(cause){state.phase=cause?.outcomeUnknown?'unknown':'failed';state.error=cause?.message||String(cause);throw cause;}
  finally{state.busy=false;state.ended=performance.now();onChange(state);}
 }};
}

export function renderHostRecovery(host){
 const state=host?.hostRecovery,available=hostRecoveryAvailable(host),busy=state?.busy,unconfirmed=hostCleanupUnconfirmed(host);
 if(!unconfirmed&&!state?.receipt&&!state?.error)return '';
 const labels={recovering:'Recovering Host…',disconnecting:'Disconnecting the previous GUI session…',reconnecting:'Reconnecting Local Host…'};
 const feedback=busy?`<div class="operation-feedback" role="status" aria-live="polite"><span class="activity-spinner" aria-hidden="true"></span><strong>${esc(labels[state.phase]||'Recovering Host…')}</strong><span data-activity-elapsed="${state.started}" data-activity-ended="">${Math.floor(Math.max(0,performance.now()-state.started)/1000)} s</span><small>Native ownership verification is pending. You can keep browsing.</small></div>`:
  state?.error?`<p class="alert" role="alert">${esc(state.error)}</p>`:state?.receipt?'<p role="status">Host recovery authorized. Cleanup and instrument/output state remain unverified. Complete the required read-only verification before instrument actions.</p>':'';
 return `<div class="host-recovery"><p class="warning">${unconfirmed&&!available?'Connection release is unconfirmed. The previous Worker may still be active; automatic restart is blocked.':'The previous Worker did not verify normal shutdown. Recovery requires its confirmed exit; instrument/output state remains unverified.'}</p>${available?`<button class="btn warn" data-ui="recover-host" ${busy||host?.hostBusy?'disabled':''}${busy?' aria-busy="true"':''}>${busy?'Recovering Host…':'Recover Host'}</button>`:''}${feedback}<details id="host-recovery-diagnostics"><summary>Recovery diagnostics</summary><pre>${esc(JSON.stringify({worker_cleanup:state?.previousCleanup??host?.worker_cleanup,host_issue:state?.previousHostIssue,stop_report:host?.hostStopReport,previous_operations:state?.previousOperations,recovery:state?.receipt,error:state?.error,native_io:host?.native_io},null,2))}</pre></details></div>`;
}
