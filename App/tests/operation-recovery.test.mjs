import test from 'node:test';import assert from 'node:assert/strict';import * as ui from '../web/console-ui.js';
const domain={kind:'device',id:'a'.repeat(32)},context={session_id:'b'.repeat(32),domain,connection_id:'c'.repeat(32),epoch:1},boot='d'.repeat(32);
test('lost receipt recovery queries the original request without executing again',async()=>{
 const calls=[];const terminal={request_id:'original',operation_id:'e'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context}};
 const client={operation:async id=>{calls.push(['query',id]);return terminal;},execute:async()=>{calls.push(['execute']);throw Error('Must not resubmit');}};
 const result=await ui.queryOriginalOperation(client,{request_id:'original',domain,boot_id:boot,context},async()=>calls.push(['sync']),()=>({boot_id:boot,context,synced:true}));
 assert.equal(result.resolved,true);assert.deepEqual(calls,[['query','original'],['sync']]);
});
test('unknown recovery does not clear uncertainty for stale context or unknown terminal',async()=>{
 const attempt={request_id:'original',domain,boot_id:boot,context};
 for(const terminal of [{status:'Outcome Unknown',phase:'timed_out_unknown'},{status:'Terminal',phase:'completed',result:{context:{...context,epoch:2}}}]){
  const result=await ui.queryOriginalOperation({operation:async()=>({request_id:'original',domain,...terminal})},attempt,async()=>{},()=>({boot_id:boot,context,synced:true}));assert.equal(result.resolved,false);
 }
});
test('original operation polling deadline preserves unknown outcome and request identity',async()=>{
 let now=0,queries=0;await assert.rejects(ui.pollOriginalOperation({operation:async()=>{queries++;return {status:'Accepted'};}},'original',{status:'Accepted'},{clock:()=>now,sleep:async()=>{now+=10;},timeoutMs:5}),error=>error.outcomeUnknown===true&&error.requestId==='original');assert.equal(queries,1);
});
