import test from 'node:test';import assert from 'node:assert/strict';
import {fetchTrace,decodeTrace,decimateTrace,rawCursor,nativeExportReference} from '../web/osa.js';
const hostId='a'.repeat(32),domain={kind:'device',id:'b'.repeat(32)},scope={hostId,domain};
const hex=bytes=>[...bytes].map(b=>b.toString(16).padStart(2,'0')).join('');
const hash=async bytes=>hex(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)));
async function fixture(unit='W',count=2){
 const bytes=new Uint8Array(count*16),view=new DataView(bytes.buffer);
 for(let i=0;i<count;i++){view.setFloat64(i*16,1500+i/1000,true);view.setFloat64(i*16+8,unit==='W'?i*1e-9:-210+i/1000,true);}
 const context={transfer_format:'ASCII',sample_count:count,spacing:unit==='W'?1:0,level_unit:unit==='W'?1:0,x_unit:0,
   trace_attribute:0,active_trace:'TRA',center_m:1.55e-6,span_m:2e-9,resolution_m:2e-11,sweep_mode:1};
 const reference={id:'c'.repeat(32),name:'osa',host_id:hostId,domain,byte_count:bytes.length,sample_count:count,sha256:await hash(bytes),
   metadata:{native_unit:unit,trace:'A',identity:'YOKOGAWA,AQ6370E,TEST-BYTES,FW',read_started_at:'2026-10-06T10:00:00+00:00',
     read_finished_at:'2026-10-06T10:00:01+00:00',elapsed_s:1,consistency:'unproven',context_before:context,context_after:context}};
 const manifestBytes=new TextEncoder().encode(JSON.stringify({schema:1,source_kind:'real',name:'osa',archived_at_unix_ms:1,
   origin:{host_id:hostId,domain,device_identity:{model:'AQ6370E'},config_rev:1,operation_id:reference.id},
   descriptor:{schema:1,kind:'osa_trace',capture_id:'d'.repeat(32),point_count:count,byte_count:bytes.length,sha256:reference.sha256,metadata:reference.metadata}}));
 const manifestHash=await hash(manifestBytes),calls=[];
 const client={archiveManifestBytes:async p=>{calls.push(['manifest',p]);return {id:reference.id,name:'osa',byte_count:manifestBytes.length,sha256:manifestHash,data_hex:hex(manifestBytes)};},
   readArchive:async p=>{calls.push(['chunk',p]);return {id:reference.id,name:'osa',offset:p.offset,length:p.length,sha256:reference.sha256,data_hex:hex(bytes.subarray(p.offset,p.offset+p.length))};},
   acquire:()=>{throw Error('historical fetch must never open hardware');},execute:()=>{throw Error('historical fetch must never execute an action');}};
 return {reference,bytes,manifestBytes,client,calls};
}
test('verified native W retains zero, original float64 samples and export provenance',async()=>{
 const f=await fixture();const trace=await fetchTrace(f.client,f.reference,scope);
 assert.equal(trace.verified,true);assert.equal(trace.native_unit,'W');assert.equal(trace.native_values[0],0);
 assert.equal(trace.native_values[1],1e-9);assert.equal(trace.power_dbm,undefined);
 assert.deepEqual([...trace.wavelength_nm],[1500,1500.001]);assert.deepEqual(nativeExportReference(trace),f.reference);
 assert.deepEqual(f.calls.map(c=>c[0]),['manifest','chunk']);
 assert.equal(rawCursor(trace,1).value,1e-9);assert.equal(rawCursor(trace,1).unit,'W');
});
test('native scope uses owning Host and domain, not labels or old boot authority',async()=>{
 const f=await fixture('dBm');
 for(const wrong of [{hostId:'e'.repeat(32),domain},{hostId,domain:{kind:'device',id:'e'.repeat(32)}}])
   await assert.rejects(fetchTrace(f.client,f.reference,wrong));
 await assert.rejects(fetchTrace(f.client,{...f.reference,boot_id:'e'.repeat(32)},scope));
 assert.equal(f.calls.length,0);assert.throws(()=>nativeExportReference({reference:f.reference}));
});
test('capture trace must be a string, not a value coerced to an axis label',async()=>{
 const f=await fixture();f.reference.metadata.trace=['A'];await assert.rejects(decodeTrace(f.reference,f.bytes),/metadata/);
});
test('truncated, unbound and hash-failed history never becomes a complete displayed trace',async()=>{
 for(const fault of ['id','offset','length','sha256','hex','data']){
   const f=await fixture(),read=f.client.readArchive;
   f.client.readArchive=async p=>{const reply=await read(p);if(fault==='id')reply.id='f'.repeat(32);
     if(fault==='offset')reply.offset++;if(fault==='length')reply.length--;
     if(fault==='sha256')reply.sha256='0'.repeat(64);if(fault==='hex')reply.data_hex='aa';
     if(fault==='data')reply.data_hex='ff'.repeat(p.length);return reply;};
   await assert.rejects(fetchTrace(f.client,f.reference,scope),undefined,fault);
 }
});
test('native decoder rejects nonfinite, nonincreasing, nonpositive and negative W samples',async()=>{
 for(const [offset,value] of [[0,0],[0,Infinity],[16,1500],[8,NaN],[8,-1]]){
   const f=await fixture();new DataView(f.bytes.buffer).setFloat64(offset,value,true);
   f.reference.sha256=await hash(f.bytes);await assert.rejects(decodeTrace(f.reference,f.bytes));
 }
 const f=await fixture();await assert.rejects(decodeTrace({...f.reference,sample_count:200002},f.bytes));
 await assert.rejects(decodeTrace({...f.reference,metadata:{...f.reference.metadata,native_unit:'V'}},f.bytes));
});
test('archive downloads are bounded to two concurrent 16384-byte scoped chunks',async()=>{
 const f=await fixture('dBm',5000),read=f.client.readArchive;let active=0,maximum=0;
 f.client.readArchive=async p=>{active++;maximum=Math.max(maximum,active);await new Promise(resolve=>setTimeout(resolve,1));
   try{return await read(p);}finally{active--;}};
 const trace=await fetchTrace(f.client,f.reference,scope);assert.equal(trace.native_values.length,5000);
 assert.ok(maximum<=2);assert.equal(f.calls.filter(c=>c[0]==='chunk').length,5);
 for(const [,p] of f.calls.filter(c=>c[0]==='chunk')){assert.ok(p.length<=16384);assert.deepEqual(p.domain,domain);}
});
test('navigation cancellation stops queued downloads and never substitutes a hardware retry',async()=>{
 const f=await fixture('dBm',5000),read=f.client.readArchive;let current=true;
 f.client.readArchive=async p=>{current=false;return read(p);};
 await assert.rejects(fetchTrace(f.client,f.reference,scope,{current:()=>current}));
 assert.ok(f.calls.filter(c=>c[0]==='chunk').length<=2);
 const next=await fixture();assert.equal((await fetchTrace(next.client,next.reference,scope)).verified,true);
});
test('decimation cannot change raw cursor or export, even for a maximum capture',async()=>{
 const f=await fixture('W',200001);new DataView(f.bytes.buffer).setFloat64(100001*16+8,1.23456789,true);
 f.reference.sha256=await hash(f.bytes);const trace=await decodeTrace(f.reference,f.bytes),before=trace.native_values[100001];
 const plotted=decimateTrace(trace,1800);assert.ok(plotted.length<=1800);assert.ok(plotted.includes(100001));
 assert.equal(rawCursor(trace,.500005).index,100001);assert.equal(rawCursor(trace,.500005).value,before);
 assert.equal(nativeExportReference(trace).sha256,f.reference.sha256);assert.equal(trace.native_values[100001],before);
});
test('manifest provenance mismatch fails before a trace is published',async()=>{
 const f=await fixture(),original=JSON.parse(new TextDecoder().decode(f.manifestBytes));original.origin.host_id='e'.repeat(32);
 const bytes=new TextEncoder().encode(JSON.stringify(original));
 f.client.archiveManifestBytes=async()=>({id:f.reference.id,name:'osa',byte_count:bytes.length,sha256:await hash(bytes),data_hex:hex(bytes)});
 await assert.rejects(fetchTrace(f.client,f.reference,scope));
});
