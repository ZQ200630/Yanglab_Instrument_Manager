import test from 'node:test';import assert from 'node:assert/strict';
import {savedTrace,scope,hostId,domain} from './osa-fixture.mjs';
import {fetchTrace} from '../web/osa.js';import {osa} from '../web/panels.js';import {actionFor} from '../web/instance-view.js';
import {osaCursorIndex} from '../web/view-model.js';import {createSharedResults} from '../web/shared-results.js';import {createDeviceStore} from '../web/device-store.js';
import {mountNativeOsa,nativeOsaRoute} from './native-console-fixture.mjs';
const boot='b'.repeat(32),key=hostId+'/device/'+domain.id,ctx={session_id:'d'.repeat(32),domain,connection_id:'e'.repeat(32),epoch:1};
const operation=(f,name='read_trace')=>({operation_id:f.reference.id,domain,status:'Terminal',phase:'completed',command:{method:'action',params:{name,args:{trace:'A'}}},result:{context:ctx,result:{archive_ref:f.reference}}});
test('stalled_read_keeps_drafts_navigation_and_previous_capture',async()=>{
 let release;const gate=new Promise(resolve=>release=resolve),f=await mountNativeOsa({gate});
 try{
  const input=f.edit('osa-name','New_run');input.focus();input.setSelectionRange(2,5);f.content().scrollTop=123;window.scrollTo(4,88);
  f.click('osa-read');await f.until(()=>f.calls.some(([method])=>method==='execute'));
  assert.match(f.html(),/Reading &amp; saving/);assert.match(f.html(),/Previous capture/);assert.match(f.html(),/data-osa-plot/);assert.doesNotMatch(f.html(),/\d+%/);
  f.publish();assert.equal(f.input('osa-name').value,'New_run');assert.equal(document.activeElement.id,'osa-name');assert.equal(document.activeElement.selectionStart,2);
  assert.equal(f.content().scrollTop,123);assert.equal(window.scrollY,88);
  f.navigate('#settings');assert.match(f.html(),/<h1>Settings/);assert.doesNotMatch(f.html(),/Anaconda|host-python/);
  f.navigate(nativeOsaRoute);assert.equal(f.input('osa-name').value,'New_run');assert.match(f.html(),/Previous capture/);
  assert.equal(f.calls.filter(([name])=>name==='execute').length,1);
  release();await f.until(()=>!f.html().includes('Reading &amp; saving')&&!f.html().includes('Loading capture'));
  assert.equal(f.calls.find(([name])=>name==='execute')[2].params.args.archive_name,'New_run');
  assert.match(f.html(),/instrument_io_ms/);assert.match(f.html(),/staging_ms/);
 }finally{release();await f.until(()=>!f.html().includes('Reading &amp; saving'));for(let i=0;i<4;i++)await new Promise(resolve=>setImmediate(resolve));f.restore();}
});
function snapshot(store,operations=[]){store.apply({type:'snapshot',host_id:hostId,boot_id:boot,seq:1,data:{mode:'real',domains:{['device:'+domain.id]:{context:ctx,device:{connected:true}}},operations}});}
test('navigation capture retention never crosses a connection generation',async()=>{
 const f=await mountNativeOsa();try{
  f.navigate('#settings');f.changeContext();f.navigate(nativeOsaRoute);
  assert.doesNotMatch(f.html(),/data-osa-plot|Previous capture/);assert.match(f.html(),/No spectrum data/);
 }finally{for(let i=0;i<4;i++)await new Promise(resolve=>setImmediate(resolve));f.restore();}
});
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
test('native capture diagnostics retain measured timings without inventing missing phases',async()=>{
 const f=await savedTrace(),record=operation(f),store=createDeviceStore();
 record.result.result.timings={instrument_io_ms:12,decode_ms:2,staging_ms:-1,aggregate_wait_ms:999};
 snapshot(store,[record]);const reader=createSharedResults(store,f.client);await reader.refresh(key);
 assert.deepEqual(reader.current(key).measuredCaptureTimings,{instrument_io_ms:12,decode_ms:2});
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

test('reading keeps the last capture explicitly previous and shows immediate busy feedback',async()=>{
 const f=await savedTrace(),trace=await fetchTrace(f.client,f.reference,scope);
 const html=osa({status:{devices:{osa:{connected:true}}},trace,pending:'request',activity:{kind:'read_trace',phase:'instrument',started:0,timings:{}}});
 assert.match(html,/Previous capture/);assert.match(html,/data-op="osa-read"[^>]*disabled[^>]*>Reading/);
 assert.match(html,/data-osa-plot/);assert.doesNotMatch(html,/100%/);
});
test('a failed new capture can display the prior capture without claiming it is current',async()=>{
 const f=await savedTrace(),trace=await fetchTrace(f.client,f.reference,scope);
 const html=osa({status:{devices:{osa:{connected:true}}},previousTrace:trace,sharedResultError:'Read failed'});
 assert.match(html,/Previous capture/);assert.match(html,/PREVIOUS/);assert.match(html,/data-osa-plot/);
});
test('an unresolved read leaves the prior spectrum visibly previous after the wait ends',async()=>{
 const f=await savedTrace(),trace=await fetchTrace(f.client,f.reference,scope);
 const html=osa({status:{devices:{osa:{connected:true}}},trace,unknown:true,activity:{kind:'read_trace',started:0,ended:100,outcome:'unknown',timings:{}}});
 assert.match(html,/Previous capture/);assert.match(html,/PREVIOUS/);
});

test('observers see unknown capture attempts without replaying or relabeling old data as current',async()=>{
 for(const withContext of [true,false]){
  const f=await savedTrace(),store=createDeviceStore();snapshot(store,[operation(f)]);const reader=createSharedResults(store,f.client);await reader.refresh(key);
  const unknown=operation(f);unknown.operation_id='2'.repeat(32);unknown.status='Outcome Unknown';unknown.phase='timed_out_unknown';unknown.result={...(withContext?{context:ctx}:{}),error:'Reply unavailable'};
  f.calls.length=0;store.apply({type:'operation',host_id:hostId,boot_id:boot,seq:2,domain,data:unknown});await reader.refresh(key);
  const state=reader.current(key);assert.equal(state.trace,undefined);assert.equal(state.previousOperationId,f.reference.id);
  assert.equal(state.resultActivity?.outcome,'unknown');assert.equal(f.calls.length,0);
 }
});
test('the same original unknown operation can hydrate a subsequently confirmed saved result',async()=>{
 const a=await savedTrace(),b=await savedTrace({id:'2'.repeat(32)}),store=createDeviceStore();snapshot(store,[operation(a)]);
 const client={archiveManifestBytes:p=>(p.id===a.reference.id?a:b).client.archiveManifestBytes(p),readArchive:p=>(p.id===a.reference.id?a:b).client.readArchive(p)};
 const reader=createSharedResults(store,client);await reader.refresh(key);
 const unknown=operation(b);unknown.status='Outcome Unknown';unknown.phase='timed_out_unknown';unknown.result={context:ctx,error:'Reply unavailable'};
 store.apply({type:'operation',host_id:hostId,boot_id:boot,seq:2,domain,data:unknown});await reader.refresh(key);assert.equal(reader.current(key).trace,undefined);
 store.apply({type:'operation',host_id:hostId,boot_id:boot,seq:3,domain,data:operation(b)});await reader.refresh(key);
 assert.equal(reader.current(key).resultOperationId,b.reference.id);assert.equal(reader.current(key).trace?.verified,true);
 assert.equal(reader.current(key).previousTrace,undefined);
});
test('unchanged native samples are not rescanned for every status or cursor update',()=>{
 let reads=0;const ys=new Proxy(Array.from({length:12000},(_,i)=>i/1000),{get(target,key,receiver){if(/^\d+$/.test(String(key)))reads++;return Reflect.get(target,key,receiver);}});
 const trace={verified:true,native_unit:'W',trace:'A',metadata:{},wavelength_nm:Array.from({length:12000},(_,i)=>1500+i/1000),native_values:ys};
 const state={status:{devices:{osa:{connected:true}}},trace};osa(state);const first=reads;
 assert.ok(first>12000);osa({...state,cursor:4});assert.ok(reads-first<10,'status/cursor redraw rescanned all samples');
});

test('shared rendering exposes loading feedback and stable immutable sample identity',async()=>{
 const f=await savedTrace(),store=createDeviceStore();snapshot(store,[operation(f)]);
 let release;const manifest=f.client.archiveManifestBytes;f.client.archiveManifestBytes=p=>new Promise(resolve=>{release=()=>resolve(manifest(p));});
 const reader=createSharedResults(store,f.client),work=reader.refresh(key);
 await new Promise(resolve=>setImmediate(resolve));assert.ok(reader.current(key).resultActivity);
 assert.equal(reader.current(key).resultActivity.phase,'manifest');release();await work;
 const one=reader.current(key,{copyTrace:false}).trace,two=reader.current(key,{copyTrace:false}).trace;
 assert.equal(one,two);assert.notEqual(one,reader.current(key).trace);
});

test('a newer failed capture ends loading feedback even when an older download is still draining',async()=>{
 const a=await savedTrace(),b=await savedTrace({id:'2'.repeat(32)}),store=createDeviceStore();snapshot(store,[operation(a)]);
 let release,failed;const sawFailure=new Promise(resolve=>failed=resolve);
 const client={archiveManifestBytes:p=>p.id===b.reference.id?new Promise(resolve=>release=()=>resolve(b.client.archiveManifestBytes(p))):a.client.archiveManifestBytes(p),readArchive:p=>p.id===b.reference.id?b.client.readArchive(p):a.client.readArchive(p)};
 const reader=createSharedResults(store,client,()=>{if(reader.current(key).sharedResultError==='Instrument read failed')failed();});await reader.refresh(key);
 store.apply({type:'operation',host_id:hostId,boot_id:boot,seq:2,domain,data:operation(b)});const older=reader.refresh(key);await new Promise(resolve=>setImmediate(resolve));
 const c=operation(b);c.operation_id='3'.repeat(32);c.phase='failed';c.result={context:ctx,error:'Instrument read failed'};
 store.apply({type:'operation',host_id:hostId,boot_id:boot,seq:3,domain,data:c});const newer=reader.refresh(key);
 try{await sawFailure;const state=reader.current(key);assert.equal(state.previousOperationId,a.reference.id);
  assert.notEqual(state.resultActivity?.ended,undefined);assert.equal(state.resultActivity.outcome,'failed');
 }finally{release();await Promise.all([older,newer]);}
 assert.equal(reader.current(key).resultActivity.outcome,'failed');
});
