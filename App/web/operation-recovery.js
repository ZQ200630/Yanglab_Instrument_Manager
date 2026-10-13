import {sameJsonValue as equal} from './json-value.js';
const unknown=(message,requestId)=>Object.assign(new Error(message),{outcomeUnknown:true,requestId});
/** Bounds only the UI wait. It neither cancels nor replays native work. */
export function replyDeadline({timeoutMs=15000,requestId,current=()=>true,message='Outcome unknown. Check the original status; no command was replayed.',contextMessage='Operation context changed. Check status before continuing.'}={}){
  let expired=false,timer;
  const timeout=new Promise((resolve,reject)=>{timer=setTimeout(()=>{expired=true;reject(Object.assign(unknown(message,requestId),{waitExpired:true}));},timeoutMs);timer?.unref?.();});
  // A deadline can expire between waits; keep its rejection observed immediately.
  timeout.catch(()=>{});
  const check=()=>{if(expired)throw Object.assign(unknown(message,requestId),{waitExpired:true});if(!current())throw unknown(contextMessage,requestId);};
  return {check,async wait(work){check();const value=await Promise.race([Promise.resolve().then(()=>{check();return work();}),timeout]);check();return value;},close(){clearTimeout(timer);}};
}
export async function boundedReply(work,options){const deadline=replyDeadline(options);try{return await deadline.wait(work);}finally{deadline.close();}}
export async function pollOriginalOperation(client,requestId,record,{clock=()=>performance.now(),sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms)),timeoutMs=190000,terminal,current=()=>true,validate=value=>value}={}){
  const until=clock()+timeoutMs;
  const deadline=replyDeadline({timeoutMs,requestId,current});
  try{record=validate(record);while(record.status==='Accepted'){
    if(clock()>=until)throw unknown('Outcome unknown. Query this original operation; never resubmit.',requestId);
    const event=await deadline.wait(()=>Promise.race([sleep(250).then(()=>null),...(terminal?[terminal]:[])]));if(event)return validate(event);
    record=validate(await deadline.wait(()=>boundedReply(()=>Promise.race([client.operation(requestId),...(terminal?[terminal]:[])]),{requestId,current})));
  }return record;}finally{deadline.close();}
}
export async function queryOriginalOperation(client,attempt,resync,scope){
  if(!attempt?.request_id||!attempt.domain)throw new Error('Original operation identity is unavailable. Safely release this instance.');
  const record=await boundedReply(()=>client.operation(attempt.request_id),{requestId:attempt.request_id});
  if(record.request_id!==attempt.request_id||!equal(record.domain,attempt.domain)||attempt.caller_session&&record.session_id!==attempt.caller_session)throw new Error('Original operation identity mismatch');
  let syncError;try{await boundedReply(resync,{requestId:attempt.request_id});}catch(error){syncError=error.message;}
  const current=scope(),context=record.result?.context??(record.phase==='rejected_before_call'?attempt.context:null);
  const resolved=!syncError&&record.status==='Terminal'&&!['timed_out_unknown','completed_readback_failed'].includes(record.phase)&&
    current?.synced&&current.boot_id===attempt.boot_id&&Boolean(context)&&equal(current.context,context);
  return {record,resolved:Boolean(resolved),syncError};
}
