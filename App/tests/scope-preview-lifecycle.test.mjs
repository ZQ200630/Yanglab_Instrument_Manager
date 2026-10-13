import test from 'node:test';
import assert from 'node:assert/strict';
import {queuePreviewRelease,flushPreviewReleases,reconcilePreviewRelease} from '../web/scope-preview-lifecycle.js';
import {renderScope} from '../web/scope.js';
test('old preview stays available through export picker and pending save',async()=>{
 const calls=[],owner={previewDiscard:async p=>{calls.push(p);return{discarded:true};}},local={scopeExporting:true};
 queuePreviewRelease({local,owner,recordId:'old'});await flushPreviewReleases({local});assert.equal(calls.length,0);
 local.scopeExporting=false;local.scopeSaving=true;await flushPreviewReleases({local});assert.equal(calls.length,0);
 local.scopeSaving=false;await flushPreviewReleases({local});assert.deepEqual(calls,[{record_id:'old'}]);assert.equal(local.scopePreviewReleases.size,0);
});
test('Busy retains cleanup obligation until actual release, saved display stays intact',async()=>{
 let attempts=0;const owner={previewDiscard:async()=>{if(!attempts++)throw Object.assign(new Error('In use'),{code:'Busy'});return{discarded:true};}},local={scopeLoadedRecord:'saved',scopeRecordSaved:true,scopeIsPreview:true,scopeViews:[{channel:'CH1'}]};
 queuePreviewRelease({local,owner,recordId:'saved'});await flushPreviewReleases({local,now:0});assert.equal(local.scopePreviewReleases.size,1);assert.equal(local.scopeIsPreview,true);
 await flushPreviewReleases({local,now:1001});assert.equal(local.scopePreviewReleases.size,0);assert.equal(local.scopeIsPreview,false);assert.equal(local.scopeViews.length,1);
});
test('uncertain export is not a confirmed release boundary',async()=>{
 let calls=0;const local={scopeExportJob:{id:'export',status:'uncertain'}},owner={previewDiscard:async()=>{calls++;return{discarded:true};}};
 queuePreviewRelease({local,owner,recordId:'old'});await flushPreviewReleases({local});assert.equal(calls,0);
 local.scopeExportJob.status='completed';await flushPreviewReleases({local});assert.equal(calls,1);
});
test('lost release receipt is visible and source actions wait for read-only reconciliation',async()=>{
 let discards=0,reads=0;const local={scopeLoadedRecord:'old',scopeRecordId:'old',scopePlotRecord:{id:'old'},scopeRecordSaved:false,scopeIsPreview:true},owner={previewDiscard:async()=>{discards++;throw Object.assign(new Error('Lost receipt'),{outcomeUnknown:true});},dataGet:async()=>{reads++;return{record:{id:'old'},saved:false,preview:true};}};
 queuePreviewRelease({local,owner,recordId:'old'});await flushPreviewReleases({local});await flushPreviewReleases({local,now:Date.now()+2000});assert.equal(discards,1);assert.equal(local.scopePreviewUnavailable,true);
 assert.match(renderScope(local),/Preview memory release is unconfirmed/);assert.match(renderScope(local),/data-wb="scope-save" disabled/);
 assert.equal(await reconcilePreviewRelease({local}),true);assert.equal(reads,1);assert.equal(discards,1);assert.equal(local.scopePreviewUnavailable,false);
});
test('ordinary requeue never drops uncertain release quarantine or rebinds its owner',async()=>{
 let calls=0;const owner={previewDiscard:async()=>{calls++;throw Object.assign(new Error('Lost receipt'),{outcomeUnknown:true});}},replacement={previewDiscard:async()=>assert.fail('must not rebind')},local={};
 queuePreviewRelease({local,owner,recordId:'old'});await flushPreviewReleases({local});
 queuePreviewRelease({local,owner:replacement,recordId:'old'});await flushPreviewReleases({local,now:Date.now()+2000});
 assert.equal(calls,1);assert.equal(local.scopePreviewReleases.get('old').owner,owner);assert.equal(local.scopePreviewReleases.get('old').failed,true);
});
test('explicit unsaved discard clears display only after actual release',async()=>{
 const local={scopeLoadedRecord:'preview',scopeRecordId:'preview',scopeIsPreview:true,scopeRecordSaved:false,scopeViews:[{}]},owner={previewDiscard:async()=>({discarded:true})};
 queuePreviewRelease({local,owner,recordId:'preview',clear:true});assert.equal(local.scopeViews.length,1);
 await flushPreviewReleases({local});assert.deepEqual(local.scopeViews,[]);assert.equal(local.scopeLoadedRecord,null);
});
