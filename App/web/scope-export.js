import {terminal} from './data-browser.js';
const message=error=>error?.message||String(error);
function finish(local,onFinish){
 const job=local.scopeExportJob;
 if(job.status==='completed'){
  const files=job.result?.files||job.result?.report?.files;
  if(Array.isArray(files)&&(!files.length||files.some(file=>file.error))){job.status='failed';job.error='Export incomplete. '+files.filter(file=>file.error).map(file=>file.error).join(' · ');}
 }
 local.scopeExporting=!terminal(job.status);
 if(!local.scopeExporting)onFinish?.(job);
}
export async function exportScopeData({owner,local,recordId,onChange=()=>{},onFinish}){
 if(local.scopeExporting||!recordId)return;
 local.scopeExportOwner=owner;local.scopeExporting=true;
 local.scopeExportJob={status:'queued',stage:'Choose destination / exporting',ui_started_at:Date.now()};onChange();
 try{
  const reply=await owner.workbenchExport({record_ids:[recordId],format:'csv'});
  local.scopeExportJob=reply===null||reply?.cancelled?{status:'cancelled',stage:'File selection cancelled'}:{...reply,id:reply?.id||reply?.job_id,status:reply?.status||(reply?.id||reply?.job_id?'queued':'completed'),ui_started_at:local.scopeExportJob.ui_started_at};
 }catch(error){local.scopeExportJob={...local.scopeExportJob,status:error.outcomeUnknown?'uncertain':'failed',error:message(error)};}
 finish(local,onFinish);onChange();
}
export async function pollScopeExport({local,force=false,onChange=()=>{},onFinish}){
 const job=local.scopeExportJob,owner=local.scopeExportOwner;
 if(!job?.id||!owner||local.scopeExportPolling||terminal(job.status)&&!force)return;
 local.scopeExportPolling=true;
 try{
  const reply=await owner.workbenchFileStatus({job_id:job.id});
  if(local.scopeExportJob!==job)return;
  if((reply?.id||reply?.job_id)!==job.id)throw new Error('Export job identity mismatch.');
  local.scopeExportJob={...job,...reply};
 }catch(error){if(local.scopeExportJob===job)local.scopeExportJob={...job,status:'uncertain',error:'Export status unavailable. '+message(error)};}
 finally{local.scopeExportPolling=false;finish(local,onFinish);onChange();}
}
export async function cancelScopeExport({local,onChange=()=>{},onFinish}){
 const job=local.scopeExportJob;
 if(!job?.id||terminal(job.status)||job.cancel_requested)return;
 job.cancel_requested=true;onChange();
 try{await local.scopeExportOwner.workbenchFileCancel({job_id:job.id});await pollScopeExport({local,onChange,onFinish});}
 catch(error){if(local.scopeExportJob===job){local.scopeExportJob={...job,status:'uncertain',error:'Cancellation is unconfirmed. '+message(error)};finish(local,onFinish);onChange();}}
}
