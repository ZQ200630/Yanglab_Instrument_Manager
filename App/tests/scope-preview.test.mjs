import test from 'node:test';
import assert from 'node:assert/strict';
import * as scope from '../web/scope.js';
const {renderScope,loadSavedScopeRecord}=scope;
import {createHostClient} from '../web/host-client.js';
const id=n=>String(n).repeat(32);
const calibration={x_increment_s:.001,x_origin_s:-.05,x_reference:0,y_increment_v:.01,y_origin:0,y_reference:0};
const record={id:id(1),name:'Preview',source:{kind:'manual'},frames:[{id:id(2),channels:[{channel:'CH1',points:100,calibration},{channel:'CH2',points:200,calibration:{...calibration,x_increment_s:.0005}}]}]};
const view=(channel='CH1')=>({channel,total_points:channel==='CH1'?100:200,calibration:channel==='CH1'?calibration:{...calibration,x_increment_s:.0005},points:[{x:0,y:0},{x:channel==='CH1'?99:199,y:1}]});
test('manual preview bridge has separate capture and explicit save methods',async()=>{
 const calls=[],client=createHostClient(async(command,{request})=>{calls.push(request);return{v:1,id:request.id,ok:true,result:{}};},async()=>()=>{});
 assert.equal(typeof client.captureStart,'function');assert.equal(typeof client.previewSave,'function');
 await client.captureStart({definition:{id:id(3)}});await client.previewSave({record_id:id(1),name:'Test',comment:''});
 assert.deepEqual(calls.map(c=>c.method),['capture_start','preview_save']);
});
test('waveform preview shows only name Save record and Export Data below the plot',()=>{
 const html=renderScope({scopeViews:[view()],scopeRecord:record,scopeRecordId:record.id,scopeLoadedRecord:record.id,scopeRecordSaved:false});
 assert.match(html,/id="scope-record-name"/);assert.match(html,/>Save record<\/button>/);assert.match(html,/>Export Data<\/button>/);
 assert.doesNotMatch(html,/Reload saved plot|Open saved data|Export saved data|original points|Reduction affects|scope-record-context/);
 assert.match(html,/Unsaved/);
});
test('loading a manual preview does not save and initializes stable physical time range',async()=>{
 const calls=[],local={},owner={dataGet:async()=>({record,saved:false}),dataViewport:async p=>{calls.push(p);return view(p.channel);},previewSave:()=>assert.fail('load must not save')};
 assert.equal(await loadSavedScopeRecord({owner,local,recordId:record.id}),true);
 assert.equal(local.scopeRecordSaved,false);assert.ok(local.scopeTimeDomain.start<0);assert.ok(local.scopeTimeDomain.end>0);
 assert.equal(local.scopeIsPreview,true);
 assert.equal(calls.length,2);
});

test('scope SVG identity is stable during unrelated status updates',()=>{
 const state={scopeViews:[view()],scopeRecord:record,scopeRecordId:record.id,scopeLoadedRecord:record.id,scopeRecordSaved:false};
 const first=renderScope(state).match(/<svg[\s\S]*?<\/svg>/)?.[0];
 assert.ok(first);assert.equal(renderScope({...state,scopeSyncError:new Error('Offline')}).match(/<svg[\s\S]*?<\/svg>/)?.[0],first);
});

test('zoom excludes channels with no overlapping calibrated samples',async()=>{
 const calls=[],shifted={...record,frames:[{...record.frames[0],channels:record.frames[0].channels.map((c,i)=>i?{...c,calibration:{...c.calibration,x_origin_s:1}}:c)}]},local={scopePlotRecord:shifted,scopeLoadedRecord:record.id,scopeFullTimeDomain:{start:-.05,end:1.1},scopeTimeDomain:{start:-.05,end:1.1},scopeViews:[view()]};
 const owner={dataViewport:async p=>{calls.push(p);return view(p.channel);}};
 assert.equal(await scope.zoomScopePlot({owner,local,timeDomain:{start:-.025,end:0}}),true);
 assert.deepEqual(calls.map(p=>p.channel),['CH1']);
});
test('physical time zoom maps each channel calibration independently and right click steps back',async()=>{
 const local={},calls=[],owner={dataGet:async()=>({record,saved:false}),dataViewport:async p=>{calls.push(p);return{...view(p.channel),points:[{x:p.start,y:0},{x:p.end-1,y:1}]};}};
 await loadSavedScopeRecord({owner,local,recordId:record.id});calls.length=0;const full={...local.scopeTimeDomain};
 assert.equal(typeof scope.zoomScopePlot,'function');await scope.zoomScopePlot({owner,local,timeDomain:{start:-.025,end:0}});
 assert.deepEqual(calls.map(p=>[p.channel,p.start,p.end]),[['CH1',25,51],['CH2',50,101]]);
 assert.equal(local.scopeZoomHistory.length,1);await scope.zoomScopePlot({owner,local,back:true});assert.deepEqual(local.scopeTimeDomain,full);
 assert.equal(local.scopeZoomHistory.length,0);
});
test('save acts on selected preview only and a late save cannot relabel a newer capture',async()=>{
 let release;const local={scopeRecord:record,scopePlotRecord:record,scopeLoadedRecord:record.id,scopeRecordId:record.id,scopeRecordSaved:false};
 const calls=[],owner={previewSave:async p=>{calls.push(p);await new Promise(resolve=>release=resolve);return{record_id:p.record_id,saved:true};}};
 assert.equal(typeof scope.saveScopePreview,'function');const pending=scope.saveScopePreview({owner,local,name:'Named record'});assert.equal(local.scopeSaving,true);
 local.scopeRecord={...record,id:id(4),name:'New preview'};local.scopeLoadedRecord=id(4);local.scopeRecordId=id(4);release();await pending;
 assert.equal(calls[0].record_id,id(1));assert.equal(calls[0].name,'Named record');assert.equal(local.scopeRecord.name,'New preview');assert.equal(local.scopeRecordSaved,false);
});
test('renaming a saved scope Record does not submit an invisible stale comment',async()=>{
 const local={scopeRecord:{...record,comment:'Old comment'},scopePlotRecord:record,scopeLoadedRecord:record.id,scopeRecordSaved:true},calls=[];
 await scope.saveScopePreview({local,name:'Renamed',owner:{dataUpdate:async p=>{calls.push(p);return{record_id:p.record_id,saved:true};}}});
 assert.deepEqual(calls,[{record_id:record.id,name:'Renamed'}]);
});
