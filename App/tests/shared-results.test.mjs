import test from 'node:test';import assert from 'node:assert/strict';import * as ui from '../web/console-ui.js';import {createDeviceStore} from '../web/device-store.js';
const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),domain={kind:'device',id:d},key=h+'/device/'+d;
const ctx={session_id:'d'.repeat(32),domain,connection_id:'e'.repeat(32),epoch:1};
function op(id,value){return {operation_id:id,domain,status:'Terminal',phase:'completed',command:{method:'action',params:{name:'acquire',args:{trace:'A'}}},result:{context:ctx,result:{result:{wavelength_nm:[value],power_dbm:[-20],trace:'A'}}}};}
function snapshot(store,operations=[]){store.apply({type:'snapshot',host_id:h,boot_id:b,seq:1,data:{mode:'real',domains:{['device:'+d]:{context:ctx,device:{connected:true}}},operations}});}
test('observer and initiating session hydrate the same live operation without a lease',async()=>{
 const stores=[createDeviceStore(),createDeviceStore()];for(const store of stores){snapshot(store);store.apply({type:'operation',host_id:h,boot_id:b,seq:2,domain,data:op('1'.repeat(32),1550)});}
 const readers=stores.map(store=>ui.createSharedResults(store,{},()=>{}));await Promise.all(readers.map(r=>r.refresh(key)));
 assert.deepEqual(readers[0].current(key).trace.wavelength_nm,[1550]);assert.deepEqual(readers[1].current(key).trace,readers[0].current(key).trace);
});
test('snapshot catch-up renders a completed shared result and context changes make it historical',async()=>{
 const store=createDeviceStore();snapshot(store,[op('1'.repeat(32),1550)]);const reader=ui.createSharedResults(store,{},()=>{});await reader.refresh(key);
 assert.deepEqual(reader.current(key).trace.wavelength_nm,[1550]);store.apply({type:'domain',host_id:h,boot_id:b,seq:2,domain,data:{context:{...ctx,epoch:2,connection_id:'f'.repeat(32)},device:{connected:true}}});
 assert.equal(reader.current(key).trace,undefined);await reader.refresh(key);assert.equal(reader.current(key).trace,undefined);
 assert.equal(store.history(key).some(e=>e.type==='operation'),true);
});
test('reversed opaque fetch completion never replaces a newer spectrum',{timeout:1000},async()=>{
 const store=createDeviceStore();snapshot(store);const pending=new Map();const client={readResult:params=>new Promise(resolve=>pending.set(params.id,resolve))};
 let completeFresh;const freshSeen=new Promise(resolve=>completeFresh=resolve);const reader=ui.createSharedResults(store,client,()=>{if(reader.current(key).trace?.wavelength_nm[0]===1551)completeFresh();});
 async function opaque(id,value,seq){const bytes=new TextEncoder().encode(JSON.stringify({result:{wavelength_nm:[value],power_dbm:[-20]}}));const checksum=[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(n=>n.toString(16).padStart(2,'0')).join('');const record=op(id,value);record.result.result={result_ref:{id,size:bytes.length,checksum,domain,boot_id:b}};store.apply({type:'operation',host_id:h,boot_id:b,seq,domain,data:record});const work=reader.refresh(key);return {work,reply:{id,offset:0,size:bytes.length,checksum,data_hex:[...bytes].map(n=>n.toString(16).padStart(2,'0')).join('')}};}
 const old=await opaque('1'.repeat(32),1550,2),fresh=await opaque('2'.repeat(32),1551,3);pending.get('2'.repeat(32))(fresh.reply);await freshSeen;
 assert.deepEqual(reader.current(key).trace.wavelength_nm,[1551]);pending.get('1'.repeat(32))(old.reply);await Promise.all([old.work,fresh.work]);assert.deepEqual(reader.current(key).trace.wavelength_nm,[1551]);
});

test('a new failed spectrum clears the old current trace instead of relabeling it',async()=>{
 const store=createDeviceStore();snapshot(store,[op('1'.repeat(32),1550)]);const reader=ui.createSharedResults(store,{readResult:async()=>{throw Error('Unavailable result');}});await reader.refresh(key);
 assert.deepEqual(reader.current(key).trace.wavelength_nm,[1550]);const next=op('2'.repeat(32),1551);next.result.result={result_ref:{id:'2'.repeat(32),boot_id:b,domain,size:1,checksum:'0'.repeat(64)}};
 store.apply({type:'operation',host_id:h,boot_id:b,seq:2,domain,data:next});await reader.refresh(key);
 assert.equal(reader.current(key).trace,undefined);assert.equal(reader.current(key).resultOperationId,undefined);assert.match(reader.current(key).sharedResultError,/Unavailable/);
});

test('opaque references from a different boot never become current results',async()=>{
 const store=createDeviceStore();const record=op('1'.repeat(32),1550);const bytes=new TextEncoder().encode(JSON.stringify({result:{wavelength_nm:[1550],power_dbm:[-20]}}));const checksum=[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(n=>n.toString(16).padStart(2,'0')).join('');
 record.result.result={result_ref:{id:'1'.repeat(32),boot_id:'f'.repeat(32),domain,size:bytes.length,checksum}};snapshot(store,[record]);
 const client={readResult:async()=>({id:'1'.repeat(32),offset:0,size:bytes.length,checksum,data_hex:[...bytes].map(n=>n.toString(16).padStart(2,'0')).join('')})};const reader=ui.createSharedResults(store,client);await reader.refresh(key);
 assert.equal(reader.current(key).trace,undefined);assert.match(reader.current(key).sharedResultError,/metadata/);
});

test('failed replacement retains prior bytes only under an explicit previous-capture slot',async()=>{
 const store=createDeviceStore();snapshot(store,[op('1'.repeat(32),1550)]);const reader=ui.createSharedResults(store,{});await reader.refresh(key);
 const next=op('2'.repeat(32),1551);next.phase='failed';next.result={context:ctx,error:'Read failed'};
 store.apply({type:'operation',host_id:h,boot_id:b,seq:2,domain,data:next});await reader.refresh(key);
 assert.equal(reader.current(key).trace,undefined);assert.equal(reader.current(key).resultOperationId,undefined);
 assert.deepEqual(reader.current(key).previousTrace.wavelength_nm,[1550]);assert.equal(reader.current(key).previousOperationId,'1'.repeat(32));
});
