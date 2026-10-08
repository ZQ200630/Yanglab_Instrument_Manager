// Finite scheduler metadata only. This fixture opens no instrument or transport.
export function releasedDomainWithCachedSample(context,sample={connected:true,state:'READY'}){
 const attempt='5'.repeat(32);
 return {domain:context.domain,hostId:'a'.repeat(32),state:'DISCONNECTED',
  context:structuredClone(context),device:structuredClone(sample),responsibility:false,pending:0,
  active_request_id:null,pending_request_id:null,safety_request_id:null,readback_request_id:null,
  safety:{state:'DISCONNECTED',phase:'completed',error:null,context:structuredClone(context),
   attempt_id:attempt,result:{attempt_id:attempt,connected:false,effective_intent:'disconnect',
    cleanup:{attempt_id:'6'.repeat(32),unreleased:[],steps:[{role:'laser',action:'preserving_close',error:null}]}}}};
}
