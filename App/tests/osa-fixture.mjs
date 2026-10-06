// Finite saved bytes only: this fixture is never a hardware/backend substitute.
export const hostId='a'.repeat(32),domain={kind:'device',id:'c'.repeat(32)},scope={hostId,domain};
export const hex=b=>[...b].map(n=>n.toString(16).padStart(2,'0')).join('');
export const hash=async b=>hex(new Uint8Array(await crypto.subtle.digest('SHA-256',b)));
export async function savedTrace({id='1'.repeat(32),unit='W',count=2,first=1500}={}){
 const bytes=new Uint8Array(count*16),v=new DataView(bytes.buffer);
 for(let i=0;i<count;i++){v.setFloat64(i*16,first+i/1000,true);v.setFloat64(i*16+8,unit==='W'?i*1e-9:-210+i/1000,true);}
 const c={transfer_format:'ASCII',sample_count:count,spacing:unit==='W'?1:0,level_unit:unit==='W'?1:0,x_unit:0,trace_attribute:0,active_trace:'TRA',center_m:1.55e-6,span_m:2e-9,resolution_m:2e-11,sweep_mode:1};
 const reference={id,name:'osa',host_id:hostId,domain,byte_count:bytes.length,sample_count:count,sha256:await hash(bytes),metadata:{native_unit:unit,trace:'A',identity:'YOKOGAWA,AQ6370E,FINITE-BYTES,FW',read_started_at:'2026-10-06T10:00:00+00:00',read_finished_at:'2026-10-06T10:00:01+00:00',elapsed_s:1,consistency:'unproven',context_before:c,context_after:c}};
 const manifest=new TextEncoder().encode(JSON.stringify({schema:1,source_kind:'real',name:'osa',archived_at_unix_ms:1,origin:{host_id:hostId,domain,device_identity:{model:'AQ6370E'},config_rev:1,operation_id:id},descriptor:{schema:1,kind:'osa_trace',capture_id:'d'.repeat(32),point_count:count,byte_count:bytes.length,sha256:reference.sha256,metadata:reference.metadata}}));
 const mh=await hash(manifest),calls=[],entry={id,name:'osa',state:'complete',reference,error:null};
 const client={listArchives:async p=>{calls.push(['list',p]);return {entries:[entry],next_offset:p.offset+1,has_more:false};},archiveManifestBytes:async p=>{calls.push(['manifest',p]);return {id,name:'osa',byte_count:manifest.length,sha256:mh,data_hex:hex(manifest)};},readArchive:async p=>{calls.push(['chunk',p]);return {id,name:'osa',offset:p.offset,length:p.length,sha256:reference.sha256,data_hex:hex(bytes.subarray(p.offset,p.offset+p.length))};},execute:()=>{throw Error('No hardware action is permitted by this fixture');}};
 return {reference,entry,client,calls,bytes};
}
