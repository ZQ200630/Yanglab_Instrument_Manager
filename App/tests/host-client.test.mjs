import test from 'node:test';
import assert from 'node:assert/strict';
import {createHostClient} from '../web/host-client.js';
import {readFileSync} from 'node:fs';

test('Host recovery uses the confirmed native ownership boundary without Worker RPC',async()=>{
 const calls=[],receipt={recovery_authorized:true,cleanup_verified:false,physical_zero_verified:false,incident_id:'a'.repeat(32),host_stopped:true};
 const client=createHostClient(async(command,args)=>{calls.push([command,args]);return receipt;},async()=>()=>{});
 assert.equal(typeof client.recoverHost,'function');
 assert.deepEqual(await client.recoverHost(),receipt);
 assert.deepEqual(calls,[['host_recover',{confirm:true}]]);
});
test('storage recovery is a named data API, never a raw instrument request or lease',async()=>{
 const calls=[];const client=createHostClient(async(cmd,args)=>{calls.push(args.request);return {v:1,id:args.request.id,ok:true,result:{},error:null};},async()=>()=>{});
 const params={domain:{kind:'device',id:'a'.repeat(32)},capture_id:'b'.repeat(32),name:'osa'};
 await client.recoverCapture(params);
 assert.equal(calls[0].method,'recover_capture');assert.deepEqual(calls[0].params,params);
 assert.doesNotMatch(JSON.stringify(calls[0]),/lease|command|resource|port|path/);
});
test('host client has named APIs and no raw worker bypass',async()=>{
  const calls=[];const invoke=async(cmd,args)=>{calls.push([cmd,args]);return {v:1,id:args?.request?.id,ok:true,result:{},error:null};};
  const client=createHostClient(invoke,async()=>()=>{});
  assert.equal(client.request,undefined);
  await client.catalog();await client.snapshot();await client.safeStop({kind:'device',id:'a'.repeat(32)});
  assert.deepEqual(calls.map(([,args])=>args.request.method),['catalog','snapshot','safe_stop']);
  assert.ok(calls.every(([cmd])=>cmd==='host_call'));
});
test('an unknown receipt is never automatically resubmitted',async()=>{
  let count=0;const client=createHostClient(async()=>{count++;throw 'connection lost';},async()=>()=>{});
  await assert.rejects(client.execute('original',{domain:{kind:'device',id:'1'.repeat(32)}}));
  assert.equal(count,1);
});

test('native admission timeout preserves proof that the request was not sent',async()=>{
  let count=0;const client=createHostClient(async()=>{
    count++;throw {code:'AdmissionTimeout',message:'Request was not sent: admission deadline expired'};
  },async()=>()=>{});
  await assert.rejects(client.execute('original',{domain:{kind:'device',id:'1'.repeat(32)}}),error=>{
    assert.equal(error.code,'AdmissionTimeout');
    assert.equal(error.notSent,true);
    assert.equal(error.outcomeUnknown,false);
    assert.match(error.requestId,/^ui-/);
    return true;
  });
  assert.equal(count,1);
});

test('other native failures remain uncertain even if a payload claims notSent',async()=>{
  for(const cause of [
    {code:'OutcomeUnknown',message:'Exchange timed out'},
    {code:'TransportClosed',message:'Transport failed',notSent:true},
    'connection lost',
  ]){
    let count=0;const client=createHostClient(async()=>{count++;throw cause;},async()=>()=>{});
    await assert.rejects(client.execute('original',{domain:{kind:'device',id:'1'.repeat(32)}}),error=>{
      assert.equal(error.outcomeUnknown,true);
      assert.notEqual(error.notSent,true);
      assert.equal(error.code,cause?.code);
      return true;
    });
    assert.equal(count,1);
  }
});

test('historical archive APIs carry scope and bounded ranges without command tokens or paths',async()=>{
  const calls=[];const client=createHostClient(async(cmd,args)=>{calls.push(args.request);return {v:1,id:args.request.id,ok:true,result:{},error:null};},async()=>()=>{});
  const domain={kind:'device',id:'a'.repeat(32)},access={domain,name:'osa',id:'b'.repeat(32)};
  await client.listArchives({domain,offset:0,limit:4});
  await client.archiveManifest(access);await client.archiveManifestBytes(access);
  await client.readArchive({...access,offset:0,length:16384});
  assert.deepEqual(calls.map(r=>r.method),['list_archives','archive_manifest','archive_manifest_bytes','read_archive']);
  assert.deepEqual(calls[3].params,{...access,offset:0,length:16384});
  assert.doesNotMatch(JSON.stringify(calls),/lease_token|proof_id|ownership_nonce|path/);
  assert.equal(client.ackCapture,undefined);assert.equal(client.readCaptureChunk,undefined);
});

test('folder selection and native export use named native commands, never a frontend path',async()=>{
 const calls=[];const client=createHostClient(async(cmd,args)=>{calls.push([cmd,args]);return null;},async()=>()=>{});
 assert.equal(await client.chooseDataRoot(),null);
 const reference={id:'b'.repeat(32),name:'osa',host_id:'a'.repeat(32),domain:{kind:'device',id:'c'.repeat(32)}};
 assert.equal(await client.exportArchive(reference),null);
 assert.deepEqual(calls,[['choose_data_root',undefined],['export_archive',{reference}]]);
 assert.equal(client.exportToPath,undefined);
});

test('native application registers folder and reference-only export commands',()=>{
 const main=readFileSync(new URL('../src-tauri/src/main.rs',import.meta.url),'utf8');
 const gui=readFileSync(new URL('../src-tauri/src/gui.rs',import.meta.url),'utf8');
 assert.match(main,/gui::choose_data_root,/);assert.match(main,/gui::export_archive,/);
 assert.match(gui,/pub async fn export_archive\([\s\S]*?reference: ArchiveRef/);
 assert.match(gui,/receive_archive\(/);assert.match(gui,/SelectionGuard::begin/);
});
