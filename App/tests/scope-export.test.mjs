import test from 'node:test';
import assert from 'node:assert/strict';
import {exportScopeData,pollScopeExport,cancelScopeExport} from '../web/scope-export.js';
test('unsaved export does not save a record and polls only its original native job',async()=>{
 const calls=[],local={},owner={workbenchExport:async p=>{calls.push(p);return{id:'job',status:'queued'};},workbenchFileStatus:async p=>{calls.push(p);return{id:'job',status:'completed',result:{files:[{path:'wave.csv'}]}};},previewSave:()=>assert.fail('Export must not save Record')};
 await exportScopeData({owner,local,recordId:'preview'});assert.equal(local.scopeExporting,true);
 await pollScopeExport({local});assert.equal(local.scopeExporting,false);assert.equal(local.scopeExportJob.status,'completed');
 assert.deepEqual(calls,[{record_ids:['preview'],format:'csv'},{job_id:'job'}]);
});
test('native file selection cancellation ends pending feedback immediately',async()=>{
 const local={};await exportScopeData({owner:{workbenchExport:async()=>({cancelled:true})},local,recordId:'preview'});
 assert.equal(local.scopeExporting,false);assert.equal(local.scopeExportJob.status,'cancelled');
});
test('export error or mismatched status is uncertain without replay',async()=>{
 let exports=0;const local={},owner={workbenchExport:async()=>{exports++;return{id:'original'};},workbenchFileStatus:async()=>({id:'replacement',status:'completed'})};
 await exportScopeData({owner,local,recordId:'preview'});await pollScopeExport({local});await pollScopeExport({local});
 assert.equal(local.scopeExportJob.status,'uncertain');assert.equal(local.scopeExporting,false);assert.equal(exports,1);
});
test('export cancellation waits for native confirmation and all-file failures are not success',async()=>{
 let cancelled=false;const local={},owner={workbenchExport:async()=>({id:'job'}),workbenchFileCancel:async()=>{cancelled=true;},workbenchFileStatus:async()=>({id:'job',status:'completed',result:{files:[{error:'Disk full'}]}})};
 await exportScopeData({owner,local,recordId:'preview'});await cancelScopeExport({local});
 assert.equal(cancelled,true);assert.equal(local.scopeExportJob.status,'failed');assert.equal(local.scopeExporting,false);
});
