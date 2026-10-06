import test from 'node:test';import assert from 'node:assert/strict';import {createArchiveHistory,exportSelectedTrace,plotFraction} from '../web/osa.js';import {savedTrace,scope} from './osa-fixture.mjs';
test('history uses current Host/domain data routes after restart, never old lease or boot',async()=>{
 const f=await savedTrace(),history=createArchiveHistory(f.client);history.select(scope);await history.list();await history.load(f.reference.id,'osa');
 const state=history.state(scope);assert.equal(state.historical,true);assert.equal(state.trace.native_unit,'W');assert.equal(state.archiveEntries.length,1);
 assert.deepEqual(f.calls[0][1],{domain:scope.domain,offset:0,limit:4});assert.equal(JSON.stringify(f.calls).includes('lease'),false);
});
test('history invalidation discards delayed pages and spectrum downloads',{timeout:1000},async()=>{
 const f=await savedTrace();let release;f.client.listArchives=()=>new Promise(r=>release=r);const history=createArchiveHistory(f.client);history.select(scope);const work=history.list();history.select(null);release({entries:[f.entry],next_offset:1,has_more:false});await work;assert.deepEqual(history.state(scope),{});
});
test('history refuses foreign scopes and oversized pages without granting authority',async()=>{
 const f=await savedTrace(),history=createArchiveHistory(f.client);history.select(scope);f.client.listArchives=async()=>({entries:[f.entry,f.entry,f.entry,f.entry,f.entry],next_offset:5,has_more:false});await assert.rejects(history.list(),/page/);assert.equal(history.state(scope).trace,undefined);
 f.entry.reference.host_id='f'.repeat(32);f.client.listArchives=async()=>({entries:[f.entry],next_offset:1,has_more:false});await assert.rejects(history.list(),/another Host/);
});
test('native export sends only verified ref, preserves W and reports cancellation without action',async()=>{
 const f=await savedTrace(),history=createArchiveHistory(f.client);history.select(scope);await history.list();await history.load(f.reference.id,'osa');let sent;
 const trace=history.state(scope).trace;assert.equal(await exportSelectedTrace({exportArchive:async r=>{sent=r;return null;}},trace,scope),null);assert.deepEqual(sent,f.reference);
 await assert.rejects(exportSelectedTrace(f.client,{...trace,verified:false},scope));assert.equal(plotFraction(48/800),0);assert.equal(plotFraction(766/800),1);
});
test('failed historical selection hides previous data rather than falling back to a live spectrum',async()=>{
 const f=await savedTrace(),history=createArchiveHistory(f.client);history.select(scope);await history.list();await history.load(f.reference.id,'osa');
 f.client.readArchive=async()=>{throw Error('Transfer failed');};await assert.rejects(history.load(f.reference.id,'osa'));
 const state=history.state(scope);assert.equal(state.historical,true);assert.equal(state.trace,null);assert.equal(state.historyError,'Transfer failed');
 history.showCurrent();assert.equal(history.state(scope).historical,false);assert.equal(Object.hasOwn(history.state(scope),'trace'),false);
});
