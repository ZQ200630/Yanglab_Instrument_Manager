/** Data-only OSA archive reader. Nothing here opens or commands an instrument. */
const id=value=>typeof value==='string'&&/^[0-9a-f]{32}$/.test(value);
const digest=value=>typeof value==='string'&&/^[0-9a-f]{64}$/.test(value);
const shortName=value=>typeof value==='string'&&/^[A-Za-z0-9][A-Za-z0-9_-]{0,39}$/.test(value);
const integer=(value,min,max)=>Number.isSafeInteger(value)&&value>=min&&value<=max;
const size=value=>new TextEncoder().encode(JSON.stringify(value)).length;
function require(condition,message){if(!condition)throw new Error(message);}
function fields(value,keys){return value!==null&&typeof value==='object'&&!Array.isArray(value)
  &&Object.keys(value).length===keys.length&&keys.every(key=>Object.hasOwn(value,key));}
function canonical(value){return JSON.stringify(value,(_,v)=>v&&typeof v==='object'&&!Array.isArray(v)
  ?Object.fromEntries(Object.keys(v).sort().map(k=>[k,v[k]])):v);}
function same(a,b){return canonical(a)===canonical(b);}
function validDomain(value){return fields(value,['kind','id'])&&['device','setup'].includes(value.kind)&&id(value.id);}
function stamp(value){
 if(typeof value!=='string')return false;
 const m=/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,6})?\+00:00$/.exec(value);
 if(!m)return false;const [y,mo,d,h,mi,s]=m.slice(1,7).map(Number),leap=y%4===0&&(y%100!==0||y%400===0);
 const days=[31,leap?29:28,31,30,31,30,31,31,30,31,30,31][mo-1];
 return y>0&&days!==undefined&&d>=1&&d<=days&&h<24&&mi<60&&s<60;
}
function metadata(value,count){
 require(fields(value,['native_unit','trace','identity','read_started_at','read_finished_at','elapsed_s','consistency','context_before','context_after'])
   &&size(value)<=8192&&['dBm','W'].includes(value.native_unit)&&typeof value.trace==='string'&&/^[A-G]$/.test(value.trace)
   &&typeof value.identity==='string'&&value.identity.trim().length>0&&stamp(value.read_started_at)&&stamp(value.read_finished_at)
   &&Number.isFinite(value.elapsed_s)&&value.elapsed_s>=0&&value.consistency==='unproven'
   &&same(value.context_before,value.context_after),'Invalid native capture metadata');
 const c=value.context_before,unit=value.native_unit==='W'?1:0;
 require(fields(c,['transfer_format','sample_count','spacing','level_unit','x_unit','trace_attribute','active_trace','center_m','span_m','resolution_m','sweep_mode'])
   &&['ASCII','REAL,32','REAL,64'].includes(c.transfer_format)&&c.sample_count===count&&c.spacing===unit&&c.level_unit===unit
   &&c.x_unit===0&&integer(c.trace_attribute,0,4)&&c.active_trace==='TR'+value.trace&&integer(c.sweep_mode,1,3)
   &&[c.center_m,c.resolution_m].every(n=>Number.isFinite(n)&&n>0)&&Number.isFinite(c.span_m)&&c.span_m>=0,
   'Unsupported or inconsistent native capture context');
}
export function validateReference(reference,scope){
 require(fields(reference,['id','name','host_id','domain','byte_count','sample_count','sha256','metadata'])
   &&id(reference.id)&&shortName(reference.name)&&id(reference.host_id)&&validDomain(reference.domain)
   &&integer(reference.sample_count,1,200001)&&reference.byte_count===reference.sample_count*16&&digest(reference.sha256),
   'Invalid native archive reference');
 metadata(reference.metadata,reference.sample_count);
 if(scope)require(id(scope.hostId)&&validDomain(scope.domain)&&reference.host_id===scope.hostId&&same(reference.domain,scope.domain),
   'Archive belongs to another Host or instrument');
 return reference;
}
async function hash(bytes){const bytesHash=new Uint8Array(await crypto.subtle.digest('SHA-256',bytes));
 return [...bytesHash].map(n=>n.toString(16).padStart(2,'0')).join('');}
function hexBytes(value,length){
 require(typeof value==='string'&&value.length===length*2&&/^[0-9a-f]+$/.test(value),'Archive byte encoding or length mismatch');
 const bytes=new Uint8Array(length);for(let i=0;i<length;i++)bytes[i]=parseInt(value.slice(i*2,i*2+2),16);return bytes;
}
function validateManifest(value,reference){
 require(fields(value,['schema','source_kind','name','archived_at_unix_ms','origin','descriptor'])
   &&value.schema===1&&value.source_kind==='real'&&value.name===reference.name
   &&integer(value.archived_at_unix_ms,1,Number.MAX_SAFE_INTEGER),'Invalid archive manifest');
 const o=value.origin,d=value.descriptor;
 require(fields(o,['host_id','domain','device_identity','config_rev','operation_id'])&&o.host_id===reference.host_id
   &&same(o.domain,reference.domain)&&o.operation_id===reference.id&&integer(o.config_rev,1,Number.MAX_SAFE_INTEGER)
   &&o.device_identity!==null&&typeof o.device_identity==='object'&&!Array.isArray(o.device_identity)
   &&Object.keys(o.device_identity).length>0&&size(o.device_identity)<=4096,'Archive provenance mismatch');
 require(fields(d,['schema','kind','capture_id','point_count','byte_count','sha256','metadata'])
   &&d.schema===1&&d.kind==='osa_trace'&&id(d.capture_id)&&d.point_count===reference.sample_count&&d.byte_count===reference.byte_count
   &&d.sha256===reference.sha256&&same(d.metadata,reference.metadata),'Archive descriptor mismatch');
}
export async function decodeTrace(reference,bytes){
 validateReference(reference);require(bytes instanceof Uint8Array&&bytes.byteLength===reference.byte_count,'Incomplete native capture');
 require(await hash(bytes)===reference.sha256,'Native archive SHA256 failed');
 const x=new Float64Array(reference.sample_count),y=new Float64Array(reference.sample_count),view=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);
 let previous=0;
 for(let i=0;i<x.length;i++){
   x[i]=view.getFloat64(i*16,true);y[i]=view.getFloat64(i*16+8,true);
   require(Number.isFinite(x[i])&&x[i]>previous&&Number.isFinite(y[i])&&(reference.metadata.native_unit!=='W'||y[i]>=0),'Invalid native wavelength or power sample');
   previous=x[i];
 }
 return {verified:true,reference:structuredClone(reference),metadata:structuredClone(reference.metadata),native_unit:reference.metadata.native_unit,
   wavelength_nm:x,native_values:y,trace:reference.metadata.trace};
}
// Shared per-window slots, including old-page transfers still draining. Release
// hands a slot directly to its waiter; a newly arriving request cannot steal it.
let active=0;const waiting=[];
async function withSlot(run){
 if(active<2)active++;else {require(waiting.length<8,'Archive download queue is full');await new Promise(resolve=>waiting.push(resolve));}
 try{return await run();}finally{const next=waiting.shift();if(next)next();else active--;}
}
export async function fetchTrace(client,reference,scope,{current=()=>true}={}){
 validateReference(reference,scope);const check=()=>require(current(),'Archive display request cancelled');check();
 const access={domain:reference.domain,name:reference.name,id:reference.id};
 const reply=await client.archiveManifestBytes(access);check();
 require(fields(reply,['id','name','byte_count','sha256','data_hex'])&&reply.id===reference.id&&reply.name===reference.name
   &&integer(reply.byte_count,1,16384)&&digest(reply.sha256),'Manifest reply mismatch');
 const manifestBytes=hexBytes(reply.data_hex,reply.byte_count);require(await hash(manifestBytes)===reply.sha256,'Manifest SHA256 failed');check();
 const manifest=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(manifestBytes));validateManifest(manifest,reference);
 const bytes=new Uint8Array(reference.byte_count);let next=0,failure=null;
 async function worker(){
   while(next<bytes.length){check();if(failure)throw failure;const offset=next,length=Math.min(16384,bytes.length-offset);next+=length;
     try{const chunk=await withSlot(()=>{check();if(failure)throw failure;return client.readArchive({...access,offset,length});});check();
       require(fields(chunk,['id','name','offset','length','sha256','data_hex'])&&chunk.id===reference.id&&chunk.name===reference.name
         &&chunk.offset===offset&&chunk.length===length&&chunk.sha256===reference.sha256,'Archive chunk scope or range mismatch');
       bytes.set(hexBytes(chunk.data_hex,length),offset);
     }catch(error){failure=error;throw error;}
   }
 }
 const results=await Promise.allSettled([worker(),worker()]);check();
 const failed=results.find(result=>result.status==='rejected');if(failed)throw failed.reason;
 const trace=await decodeTrace(reference,bytes);check();
 return {...trace,manifest,manifest_bytes:manifestBytes};
}
export function decimateTrace(trace,maximum=1800){
 require(integer(maximum,4,4000),'Invalid plot point budget');
 const y=trace.native_values,n=y.length;if(n<=maximum)return Array.from({length:n},(_,i)=>i);
 const indices=[0],buckets=Math.floor((maximum-2)/2),width=(n-2)/buckets;
 for(let b=0;b<buckets;b++){
   const first=1+Math.floor(b*width),end=1+Math.floor((b+1)*width);let low=first,high=first;
   for(let i=first+1;i<end;i++){if(y[i]<y[low])low=i;if(y[i]>y[high])high=i;}
   if(low===high)indices.push(low);else indices.push(Math.min(low,high),Math.max(low,high));
 }
 indices.push(n-1);return indices;
}
export function rawCursor(trace,fraction){
 const x=trace?.wavelength_nm,y=trace?.native_values;if(!x?.length||x.length!==y?.length)return null;
 const f=Number.isFinite(fraction)?Math.max(0,Math.min(1,fraction)):0,target=x[0]+f*(x.at(-1)-x[0]);
 let low=0,high=x.length-1;while(low<high){const mid=Math.floor((low+high)/2);if(x[mid]<target)low=mid+1;else high=mid;}
 const index=low>0&&Math.abs(x[low-1]-target)<=Math.abs(x[low]-target)?low-1:low;
 return {index,wavelength_nm:x[index],value:y[index],unit:trace.native_unit};
}
export function nativeExportReference(trace){require(trace?.verified===true,'Select a verified capture before export');
 validateReference(trace.reference);return structuredClone(trace.reference);}
export function plotFraction(fraction){return Math.max(0,Math.min(1,(fraction*800-48)/718));}
export async function exportSelectedTrace(client,trace,scope){
 const reference=nativeExportReference(trace);validateReference(reference,scope);return (client.forHost?.(scope.hostId)||client).exportArchive(reference);
}
/** One selected archive scope; four entries per page, at most 128 retained. */
export function createArchiveHistory(client,onChange=()=>{}){
 let selected=null,generation=0,entries=[],offset=0,more=false,busy=false,error=null,trace=null,chosen=null;
 function current(scope,token){return token===generation&&same(scope,selected);}
 function reset(){generation++;entries=[];offset=0;more=false;busy=false;error=null;trace=null;chosen=null;}
 return Object.freeze({
  select(scope){if(!same(scope,selected)){reset();selected=scope?structuredClone(scope):null;}},
  state(scope){if(!selected||!same(scope,selected))return {};
    return structuredClone({archiveEntries:entries,historyHasMore:more,historyBusy:busy,historyError:error,historical:Boolean(chosen),...(chosen?{trace}: {})});},
  showCurrent(){generation++;busy=false;error=null;trace=null;chosen=null;onChange();},
  async list(append=false){
    require(selected&&!busy,'Archive history is unavailable or busy');const scope=structuredClone(selected),token=++generation;
    busy=true;error=null;if(!append){entries=[];offset=0;more=false;}const start=offset;onChange();
    try{const page=await (client.forHost?.(scope.hostId)||client).listArchives({domain:scope.domain,offset:start,limit:4});if(!current(scope,token))return;
      require(fields(page,['entries','next_offset','has_more'])&&Array.isArray(page.entries)&&page.entries.length<=4
        &&integer(page.next_offset,0,4096)&&page.next_offset===start+page.entries.length&&typeof page.has_more==='boolean'
        &&(!page.has_more||page.entries.length>0),'Invalid archive history page');
      const known=new Set(entries.map(e=>e.name+'/'+e.id));for(const e of page.entries){
        require(fields(e,['id','name','state','reference','error'])&&id(e.id)&&shortName(e.name)&&['complete','partial'].includes(e.state)
          &&(e.error===null||typeof e.error==='string'&&e.error.length<=8192),'Invalid archive entry');
        if(e.state==='complete'){validateReference(e.reference,scope);require(e.reference.id===e.id&&e.reference.name===e.name&&e.error===null,'Archive entry mismatch');}
        else require(e.reference===null,'Incomplete archive has a complete reference');
        require(!known.has(e.name+'/'+e.id),'Repeated archive entry');known.add(e.name+'/'+e.id);
      }
      entries.push(...structuredClone(page.entries));entries=entries.slice(0,128);offset=page.next_offset;more=page.has_more&&entries.length<128;
    }catch(cause){if(current(scope,token))error=cause.message;throw cause;}
    finally{if(current(scope,token)){busy=false;onChange();}}
  },
  async load(id,name){
    require(selected&&!busy,'Archive history is unavailable or busy');const entry=entries.find(e=>e.id===id&&e.name===name);
    require(entry?.state==='complete','Select a complete saved capture');const scope=structuredClone(selected),token=++generation;
    busy=true;error=null;trace=null;chosen=entry.reference;onChange();
    try{const result=await fetchTrace(client.forHost?.(scope.hostId)||client,entry.reference,scope,{current:()=>current(scope,token)});if(current(scope,token))trace=result;}
    catch(cause){if(current(scope,token))error=cause.message;throw cause;}
    finally{if(current(scope,token)){busy=false;onChange();}}
  },
 });
}
