const equal=(a,b)=>JSON.stringify(a)===JSON.stringify(b);
export async function pollOriginalOperation(client,requestId,record,{clock=()=>performance.now(),sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms)),timeoutMs=190000}={}){
  const until=clock()+timeoutMs;
  while(record.status==='Accepted'){
    if(clock()>=until)throw Object.assign(new Error('Outcome unknown. Query this original operation; never resubmit.'),{outcomeUnknown:true,requestId});
    await sleep(250);record=await client.operation(requestId);
  }
  return record;
}
export async function queryOriginalOperation(client,attempt,resync,scope){
  if(!attempt?.request_id||!attempt.domain)throw new Error('Original operation identity is unavailable. Safely release this instance.');
  const record=await client.operation(attempt.request_id);
  if(record.request_id!==attempt.request_id||!equal(record.domain,attempt.domain))throw new Error('Original operation identity mismatch');
  let syncError;try{await resync();}catch(error){syncError=error.message;}
  const current=scope(),context=record.result?.context??(record.phase==='rejected_before_call'?attempt.context:null);
  const resolved=!syncError&&record.status==='Terminal'&&!['timed_out_unknown','completed_readback_failed'].includes(record.phase)&&
    current?.synced&&current.boot_id===attempt.boot_id&&Boolean(context)&&equal(current.context,context);
  return {record,resolved:Boolean(resolved),syncError};
}
