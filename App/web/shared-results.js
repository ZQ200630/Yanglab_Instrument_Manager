import {recordPmSample} from './pm400.js';
import {fetchTrace} from './osa.js';
const equal=(a,b)=>JSON.stringify(a)===JSON.stringify(b);
function slot(record){const command=record.command;if(command?.method!=='action')return null;const p=command.params;
  if(['acquire','read_trace'].includes(p?.name))return 'osa_trace';
  if(p?.name==='measure_kind')return p.name;
  if(p?.name==='read_setting')return p.name+':'+p.args.setting;
  if(p?.name==='run_maintenance')return p.name+':'+p.args.command;return null;}
export async function readOperationResult(client,domain,reply,boot){
  let value=reply?.result;const meta=value?.result_ref;
  if(meta){if(meta.boot_id!==boot||!Number.isSafeInteger(meta.size)||meta.size<1||meta.size>16*1024*1024||!equal(meta.domain,domain)||!/^([0-9a-f]{64})$/.test(meta.checksum))throw new Error('Result metadata mismatch');
    const bytes=new Uint8Array(meta.size);
    for(let offset=0;offset<meta.size;){const chunk=await client.readResult({domain,id:meta.id,offset,length:Math.min(256*1024,meta.size-offset)});
      if(chunk.id!==meta.id||chunk.offset!==offset||chunk.checksum!==meta.checksum||chunk.size!==meta.size||typeof chunk.data_hex!=='string'||!/^([0-9a-f]{2})+$/.test(chunk.data_hex))throw new Error('Result chunk mismatch');
      const data=Uint8Array.from(chunk.data_hex.match(/../g),n=>parseInt(n,16));if(data.length>256*1024||offset+data.length>meta.size)throw new Error('Result chunk overflow');bytes.set(data,offset);offset+=data.length;}
    const checksum=[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(n=>n.toString(16).padStart(2,'0')).join('');if(checksum!==meta.checksum)throw new Error('Result checksum failed');value=JSON.parse(new TextDecoder().decode(bytes));
  }
  return value?.result??value;
}
/** One selected instance, two readers, sixteen coalesced pending display slots. */
export function createSharedResults(store,client,onChange=()=>{}){
  let scope=null,values={},desired=new Map(),pending=new Map(),running=new Map(),done=new Map(),idle=Promise.resolve(),settle=null;
  function identity(key){const d=store.get(key),host=store.host(d?.hostId);return host?.connected&&host.synced&&d?.context?{key,hostId:d.hostId,boot:host.bootId,context:d.context}:null;}
  function current(entry){return equal(scope,entry.scope)&&equal(identity(entry.scope.key),entry.scope)&&desired.get(entry.slot)?.record.operation_id===entry.record.operation_id;}
  function finish(){if(!running.size&&!pending.size){settle?.();settle=null;}}
  function pump(){while(running.size<2&&pending.size){const [s,entry]=pending.entries().next().value;pending.delete(s);const id=entry.scope.boot+'/'+entry.record.operation_id;
    if(running.has(id))continue;
    const work=Promise.resolve().then(async()=>{try{if(entry.record.phase!=='completed')throw new Error(entry.record.result?.error?.message||entry.record.result?.error||entry.record.phase);
      let result=await readOperationResult(client,entry.record.domain,entry.record.result,entry.scope.boot);
      if(entry.slot==='osa_trace'){
        const reference=result?.archive_ref;
        if(reference){if(reference.id!==entry.record.operation_id)throw new Error('Archive operation mismatch');
          result=await fetchTrace(client,reference,{hostId:entry.scope.hostId,domain:entry.record.domain},{current:()=>current(entry)});}
        else if(typeof client.readArchive==='function')throw new Error('Native archive reference unavailable');
      }
      if(!current(entry))return;
      const p=entry.record.command.params;
      if(entry.slot==='osa_trace'&&(result?.verified===true||(Array.isArray(result?.wavelength_nm)&&Array.isArray(result?.power_dbm)&&result.wavelength_nm.length===result.power_dbm.length))){values.trace=result;values.resultOperationId=entry.record.operation_id;}
      if(p.name==='measure_kind'){values.pmMeasurement=result;values.pmHistory=recordPmSample(values.pmHistory||[],result,p.args.kind);}
      if(p.name==='read_setting')values.pmReadings={...values.pmReadings,[p.args.setting]:result};
      if(p.name==='run_maintenance')values.pmCommandResults={...values.pmCommandResults,[p.args.command]:result};
      done.set(entry.slot,id);values.sharedResultError=null;onChange(entry.scope.key);
    }catch(error){if(current(entry)){done.set(entry.slot,id);values.sharedResultError=error.message;onChange(entry.scope.key);}}
    finally{running.delete(id);pump();finish();}});running.set(id,work);
  }finish();}
  return Object.freeze({
    current(key){return equal(scope,identity(key))?structuredClone(values):{};},
    refresh(key){const next=identity(key);if(!equal(next,scope)){scope=next;values={};desired.clear();pending.clear();done.clear();}
      if(!scope){finish();return idle;}
      const latest=new Map();for(const item of store.operationRecords(key)){const r=item.record,s=slot(r);if(s&&item.bootId===scope.boot&&r.status==='Terminal'&&(s==='osa_trace'||r.phase==='completed')&&equal(r.result?.context,scope.context))latest.set(s,{...item,slot:s,scope:structuredClone(scope)});}
      while(latest.size>16)latest.delete(latest.keys().next().value);
      for(const [s,entry]of latest){if(desired.get(s)?.record.operation_id!==entry.record.operation_id){
        const p=entry.record.command.params;
        if(s==='osa_trace'){delete values.trace;delete values.resultOperationId;}
        if(p.name==='measure_kind')delete values.pmMeasurement;
        if(p.name==='read_setting'&&values.pmReadings)delete values.pmReadings[p.args.setting];
        if(p.name==='run_maintenance'&&values.pmCommandResults)delete values.pmCommandResults[p.args.command];
      }}
      desired=latest;pending.clear();for(const s of done.keys())if(!latest.has(s))done.delete(s);
      for(const [s,entry]of latest){const id=scope.boot+'/'+entry.record.operation_id;if(done.get(s)!==id&&!running.has(id))pending.set(s,entry);}
      if((pending.size||running.size)&&!settle)idle=new Promise(resolve=>settle=resolve);pump();return idle;
    },
  });
}
