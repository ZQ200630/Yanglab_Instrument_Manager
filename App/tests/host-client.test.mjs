import test from 'node:test';
import assert from 'node:assert/strict';
import {createHostClient} from '../web/host-client.js';
import {readFileSync} from 'node:fs';
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
