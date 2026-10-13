/** Release only temporary RAM; saved immutable Records are never removed. */
export function queuePreviewRelease({local,owner,recordId,clear=false,retry=false}){
 if(!recordId)return;
 local.scopePreviewReleases??=new Map();
 const previous=local.scopePreviewReleases.get(recordId);
 local.scopePreviewReleases.set(recordId,{...previous,owner:previous?.owner||owner,clear:clear||previous?.clear,nextAt:previous?.nextAt??0,...(retry?{failed:false,nextAt:0}:{})});
}
export async function flushPreviewReleases({local,now=Date.now(),onChange=()=>{}}){
 // A destination picker may not have pinned its export yet. Unknown outcomes
 // do not prove release, even though they end the UI's busy spinner.
 if(local.scopePreviewReleasing||local.scopeSaving||local.scopeExporting||local.scopeExportJob?.status==='uncertain')return;
 local.scopePreviewReleasing=true;let changed=false;
 try{for(const [recordId,pending]of local.scopePreviewReleases||[]){
  if(pending.failed||now<pending.nextAt)continue;
  try{
   const reply=await pending.owner.previewDiscard({record_id:recordId});
   if(reply?.discarded!==true)throw Object.assign(new Error('Preview memory release was not confirmed.'),{outcomeUnknown:true});
   local.scopePreviewReleases.delete(recordId);changed=true;
   if(local.scopeLoadedRecord===recordId){
    local.scopeIsPreview=false;
    if(pending.clear&&!local.scopeRecordSaved){
     local.scopePlotGeneration=(local.scopePlotGeneration||0)+1;
     local.scopeViews=[];local.scopeRecord=null;local.scopePlotRecord=null;local.scopeLoadedRecord=null;local.scopeRecordId=null;local.scopeTimeDomain=null;local.scopeFullTimeDomain=null;local.scopeZoomHistory=[];local.scopePreviewBudgetBlocked=false;
    }
   }
  }catch(error){pending.nextAt=now+1000;if(error.code!=='Busy'){
   // Do not replay an unconfirmed memory mutation or rebind it to a new Host.
   pending.failed=true;
   local.scopePreviewReleaseError={recordId,owner:pending.owner,cause:error,clear:pending.clear};
   if(local.scopeLoadedRecord===recordId&&!local.scopeRecordSaved)local.scopePreviewUnavailable=true;
   changed=true;
  }}
 }}finally{local.scopePreviewReleasing=false;if(changed)onChange();}
}
/** This is a metadata check only. It never replays a memory mutation. */
export async function reconcilePreviewRelease({local,onChange=()=>{}}){
 const failure=local.scopePreviewReleaseError;if(!failure||local.scopePreviewReleaseChecking)return false;
 local.scopePreviewReleaseChecking=true;onChange();
 try{
  const reply=await failure.owner.dataGet({record_id:failure.recordId});
  if(reply?.record?.id!==failure.recordId)throw new Error('Preview metadata identity mismatch.');
  if(local.scopePreviewReleaseError!==failure)return false;
  if(local.scopeLoadedRecord===failure.recordId){local.scopePreviewUnavailable=false;local.scopeIsPreview=reply.preview===true;local.scopeRecordSaved=reply.saved===true;}
  if(reply.preview===false){local.scopePreviewReleases.delete(failure.recordId);local.scopePreviewReleaseError=null;}
  else failure.reconciled=true;
  return true;
 }catch(error){if(local.scopePreviewReleaseError===failure)failure.cause=error;return false;}
 finally{local.scopePreviewReleaseChecking=false;onChange();}
}
