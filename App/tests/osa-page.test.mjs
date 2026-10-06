import test from 'node:test';import assert from 'node:assert/strict';
import {savedTrace,scope,hostId,domain} from './osa-fixture.mjs';
import {fetchTrace} from '../web/osa.js';import {osa} from '../web/panels.js';import {actionFor} from '../web/instance-view.js';
import {osaCursorIndex} from '../web/view-model.js';import {createSharedResults} from '../web/shared-results.js';import {createDeviceStore} from '../web/device-store.js';
const boot='b'.repeat(32),key=hostId+'/device/'+domain.id,ctx={session_id:'d'.repeat(32),domain,connection_id:'e'.repeat(32),epoch:1};
const operation=(f,name='read_trace')=>({operation_id:f.reference.id,domain,status:'Terminal',phase:'completed',command:{method:'action',params:{name,args:{trace:'A'}}},result:{context:ctx,result:{archive_ref:f.reference}}});
function snapshot(store,operations=[]){store.apply({type:'snapshot',host_id:hostId,boot_id:boot,seq:1,data:{mode:'real',domains:{['device:'+domain.id]:{context:ctx,device:{connected:true}}},operations}});}
test('read-first OSA page keeps native W, raw cursor, history and native export',async()=>{
 const f=await savedTrace(),trace=await fetchTrace(f.client,f.reference,scope),html=osa({status:{devices:{osa:{connected:true}}},trace,cursor:1,archiveEntries:[f.entry],recordingRoot:'D:/Data & spectra'});
 assert.match(html,/data-op="osa-read"[^>]*>Read trace/);assert.match(html,/<details id="osa-sweep">/);assert.match(html,/<summary>Start sweep<\/summary>/);
 assert.match(html,/1e-9 W/);assert.doesNotMatch(html,/NaN|power_dbm/);assert.match(html,/data-ui="osa-history-load"/);assert.match(html,/D:\/Data &amp; spectra/);
 assert.match(html,/data-op="osa-export"[^>]*>Export/);assert.equal(osaCursorIndex(trace,1),1);
 assert.deepEqual(actionFor('osa-read',id=>id==='osa-trace'?'G':'Run_7'),{name:'read_trace',args:{trace:'G',archive_name:'Run_7'}});
 assert.throws(()=>actionFor('osa-read',id=>id==='osa-trace'?'H':'Run_7'));
});
test('historical spectrum remains visibly historical while disconnected and cannot read hardware',async()=>{
 const f=await savedTrace(),trace=await fetchTrace(f.client,f.reference,scope),html=osa({status:{devices:{}},trace,historical:true,archiveEntries:[f.entry]});
 assert.match(html,/Historical capture/);assert.match(html,/data-op="osa-read" disabled/);assert.match(html,/data-op="osa-export" >/);
});
test('two observers hydrate native archived read without acquiring control or using old boot',async()=>{
 const f=await savedTrace(),stores=[createDeviceStore(),createDeviceStore()];stores.forEach(s=>snapshot(s,[operation(f)]));
 const readers=stores.map(s=>createSharedResults(s,f.client));await Promise.all(readers.map(r=>r.refresh(key)));
 for(let i=0;i<2;i++){assert.equal(readers[i].current(key).trace.native_unit,'W');assert.equal(readers[i].current(key).trace.native_values[0],0);assert.equal(stores[i].lease(key),null);}
});
test('native current route rejects cross-Host references before fetching and clears stale spectrum',async()=>{
 const f=await savedTrace(),store=createDeviceStore();snapshot(store,[operation(f)]);const reader=createSharedResults(store,f.client);await reader.refresh(key);assert.ok(reader.current(key).trace);
 const next=await savedTrace({id:'2'.repeat(32)});next.reference.host_id='f'.repeat(32);f.calls.length=0;
 store.apply({type:'operation',host_id:hostId,boot_id:boot,seq:2,domain,data:operation(next,'acquire')});await reader.refresh(key);
 assert.equal(reader.current(key).trace,undefined);assert.match(reader.current(key).sharedResultError,/another Host/);assert.equal(f.calls.length,0);
});
test('current page change drains archive fetch without publishing or reopening hardware',{timeout:1000},async()=>{
 const f=await savedTrace(),store=createDeviceStore();snapshot(store,[operation(f)]);let release,started;const ready=new Promise(r=>started=r),read=f.client.readArchive;
 f.client.readArchive=p=>new Promise(resolve=>{release=()=>resolve(read(p));started();});const reader=createSharedResults(store,f.client),work=reader.refresh(key);await Promise.race([ready,work.then(()=>assert.fail('Native archive was not fetched'))]);
 reader.refresh(null);release();await work;assert.equal(reader.current(key).trace,undefined);
});
test('a terminal failed read invalidates the previous current capture, without retrying',async()=>{
 const f=await savedTrace(),store=createDeviceStore();snapshot(store,[operation(f)]);const reader=createSharedResults(store,f.client);await reader.refresh(key);
 const next=operation(await savedTrace({id:'2'.repeat(32)}));next.phase='failed';next.result={context:ctx,error:'Instrument read failed'};f.calls.length=0;
 store.apply({type:'operation',host_id:hostId,boot_id:boot,seq:2,domain,data:next});await reader.refresh(key);
 assert.equal(reader.current(key).trace,undefined);assert.match(reader.current(key).sharedResultError,/Instrument read failed/);assert.equal(f.calls.length,0);
});

test('two failed reads release reader slots so the next successful capture hydrates',{timeout:1000},async()=>{
 const f=await savedTrace({id:'3'.repeat(32)}),store=createDeviceStore();snapshot(store);const reader=createSharedResults(store,f.client);
 for(const [seq,id] of [[2,'1'.repeat(32)],[3,'2'.repeat(32)]]){
  const failed=operation(f);failed.operation_id=id;failed.phase='completed_readback_failed';failed.result={context:ctx,error:'Read failed'};
  store.apply({type:'operation',host_id:hostId,boot_id:boot,seq,domain,data:failed});await reader.refresh(key);
  assert.equal(reader.current(key).trace,undefined);
 }
 store.apply({type:'operation',host_id:hostId,boot_id:boot,seq:4,domain,data:operation(f)});await reader.refresh(key);
 assert.equal(reader.current(key).trace.verified,true);assert.equal(reader.current(key).resultOperationId,'3'.repeat(32));assert.ok(f.calls.length>0);
 reader.refresh(null);await reader.refresh(key);assert.equal(reader.current(key).trace.verified,true);
});
