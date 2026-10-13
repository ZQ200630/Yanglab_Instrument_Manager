import test from 'node:test';
import assert from 'node:assert/strict';
import * as scope from '../web/scope.js';

const id=n=>String(n).repeat(32),fields=values=>key=>String(values[key]??'');
const connected={connected:true,quality:'confirmed',previous:false,acquisition:'running',channels:[{channel:1,displayed:true},{channel:2,displayed:false},{channel:3,displayed:true},{channel:4,displayed:null}],capabilities:{channel_count:4,memory_depths:[1000,10000,100000]}};
const ready={confirmed:true,mode:'READY'};
const frame={id:id(2),captured_at_unix_ms:1000,channels:[{channel:'CH1',points:100},{channel:'CH3',points:200}]};
const record={id:id(1),name:'Bench screen',source:{parameters:{acquisition:'existing_screen',source:'screen'}},frames:[frame]};

test('Acquire defaults to a stopped 100K memory frame with final resume and generates its record name',()=>{
 const definition=scope.scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get:fields({}),channels:[1,3]});
 assert.deepEqual(definition.parameters,{acquisition:'stopped',source:'memory',memory_depth:100000,frames:1,interval_ms:0,resume_after_capture:true,channels:[1,3]});
 assert.match(definition.name,/^Waveform /);
});

test('initial physical depth readback preserves the default acquisition draft',()=>{
 for(const device of [{...connected,memory_depth:1000},{...connected,memory_depth_auto:true,memory_depth:0}]){
  const local={};scope.syncScopeDraft(local,{state:'READY',context:{connection_id:id(5)},device},id(6),0);
  assert.equal(local.inputs.get('scope-depth').value,'100000');
  const html=scope.renderScope({...local,status:{devices:{scope:device}},roles:{scope:ready}});
  assert.match(html,/<option value="100000" selected>100K<\/option>/);
  assert.match(html,/id="scope-resume"[^>]* checked/);
  assert.match(html,new RegExp('Controller memory depth: '+(device.memory_depth_auto?'Auto':'1K')));
 }
});

test('initial readback and render preserve a manually edited acquisition draft and unchecked resume',()=>{
 const local={inputs:new Map([['scope-depth',{value:'10000'}],['scope-resume',{checked:false}]])};
 const device={...connected,memory_depth:1000};
 scope.syncScopeDraft(local,{state:'READY',context:{connection_id:id(5)},device},id(6),0);
 assert.equal(local.inputs.get('scope-depth').value,'10000');
 const html=scope.renderScope({...local,status:{devices:{scope:device}},roles:{scope:ready}});
 assert.match(html,/<option value="10000" selected>10K<\/option>/);
 assert.doesNotMatch(html,/id="scope-resume"[^>]* checked/);
 assert.equal(scope.scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get:fields({'scope-resume':'false'}),channels:[1]}).parameters.resume_after_capture,false);
});

test('Acquire carries loop and final resume options and validates each capture parameter',()=>{
 const get=fields({'scope-source':'memory','scope-depth':1000,'scope-frames':3,'scope-interval':.1,'scope-resume':'true'});
 assert.deepEqual(scope.scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get,channels:[2]}).parameters,{acquisition:'stopped',source:'memory',memory_depth:1000,frames:3,interval_ms:100,resume_after_capture:true,channels:[2]});
 assert.throws(()=>scope.scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get:fields({'scope-source':'unknown'}),channels:[1]}),/Screen or Memory/);
 assert.throws(()=>scope.scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get:fields({'scope-frames':10000}),channels:[1]}),/Frames/);
 assert.throws(()=>scope.scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get:fields({'scope-interval':3600.1}),channels:[1]}),/Frame interval/);
});

test('capture defaults use only displayed true in a current verified readback',()=>{
 assert.equal(typeof scope.syncScopeDraft,'function');
 const domain={state:'READY',context:{connection_id:id(5)},device:connected};
 for(const device of [{...connected,previous:true},{...connected,quality:'historical'},{...connected,connected:false},{...connected,quality:undefined},{...connected,previous:undefined}]){
  const local={};scope.syncScopeDraft(local,{...domain,device},id(6),0);
  assert.equal(local.scopeCaptureInitialized,undefined);
 }
 for(const age of [null,undefined,NaN]){
  const local={};scope.syncScopeDraft(local,domain,id(6),age);assert.equal(local.scopeCaptureInitialized,undefined);
 }
 const local={};scope.syncScopeDraft(local,domain,id(6),0);
 assert.deepEqual(local.scopeCaptureChannels,[1,3]);
 assert.equal(local.inputs.get('scope-capture-2').checked,false);
});

test('later confirmed off readback clears Save while preserving valid choices and settings drafts',()=>{
 assert.equal(typeof scope.syncScopeDraft,'function');
 const local={};scope.syncScopeDraft(local,{state:'READY',context:{connection_id:id(5)},device:connected},id(6),0);
 local.scopeCaptureChannels=[1,2];local.scopeCaptureDirty=true;local.inputs.set('scope-depth',{value:'10000'});
 scope.syncScopeDraft(local,{state:'READY',context:{connection_id:id(7)},device:{...connected,channels:[{channel:4,displayed:true}]}},id(6),0);
 assert.deepEqual(local.scopeCaptureChannels,[1,2]);
 scope.syncScopeDraft(local,{state:'READY',context:{connection_id:id(7)},device:connected},id(6),0);
 assert.deepEqual(local.scopeCaptureChannels,[1]);assert.equal(local.inputs.get('scope-capture-2').checked,false);
 assert.equal(local.inputs.get('scope-depth').value,'10000');
 scope.syncScopeDraft(local,{state:'READY',context:{connection_id:id(7)},device:{...connected,memory_depth:1000}},id(6),0,{force:true});
 assert.equal(local.inputs.get('scope-depth').value,'1000');
});

test('Sync replaces a numeric depth draft with actual Auto and Screen capture remains available',()=>{
 const device={...connected,memory_depth:0,memory_depth_auto:true},domain={state:'READY',context:{connection_id:id(5)},device};
 const local={scopeCaptureInitialized:true,scopeCaptureChannels:[1,3],inputs:new Map([['scope-depth',{value:'10000'}]])};
 scope.syncScopeDraft(local,domain,id(6),0,{force:true});
 assert.equal(local.inputs.get('scope-depth').value,'auto');
 const memory=scope.renderScope({...local,status:{devices:{scope:device}},roles:{scope:ready}});
 assert.match(memory,/<option value="auto" selected disabled>Auto/);assert.match(memory,/data-wb="scope-collect" disabled/);
 assert.match(memory,/Choose a memory depth/);
 assert.throws(()=>scope.scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get:fields({'scope-depth':'auto'}),channels:[1]}),/Choose a memory depth/);
 const screen=scope.renderScope({...local,scopeSourceDraft:'screen',status:{devices:{scope:device}},roles:{scope:ready}});
 assert.match(screen,/data-wb="scope-collect">Acquire/);
 assert.equal(scope.scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get:fields({'scope-source':'screen','scope-depth':'auto'}),channels:[1]}).parameters.memory_depth,null);
 scope.syncScopeDraft(local,{...domain,device:{...device,memory_depth:1000,memory_depth_auto:false}},id(6),0,{force:true});
 assert.equal(local.inputs.get('scope-depth').value,'1000');
});

test('Scope main flow presents the plot and compact save choices before collapsed acquisition controls',()=>{
 const html=scope.renderScope({status:{devices:{scope:connected}},roles:{scope:ready}});
 const advanced=html.indexOf('<details id="scope-advanced"');
 assert.ok(advanced>0);assert.ok(html.indexOf('scope-waveform')<advanced);
 assert.ok(html.indexOf('data-wb="scope-collect"')<advanced);
 assert.match(html,/<details id="scope-advanced"[^>]*><summary>Advanced/);
 assert.doesNotMatch(html,/<details id="scope-advanced"[^>]*\bopen/);
 assert.match(html,/data-wb="scope-collect">Acquire/);
 assert.match(html,/id="scope-capture-1"[^>]* checked/);
 assert.doesNotMatch(html,/id="scope-capture-2"[^>]* checked/);
 assert.doesNotMatch(html,/id="scope-capture-4"[^>]* checked/);
 assert.match(html.slice(advanced),/id="scope-resume"[^>]*type="checkbox"/);
 assert.match(html.slice(0,advanced),/data-op="scope-display"/);
 assert.match(html,/data-op="scope-read">Sync/);
 assert.doesNotMatch(html,/scope-name|scope-acquisition|Set depth|Refresh readback|SINGLE|data-op="scope-(run|stop|depth)"/);
 assert.match(html,/class="scope-channel-row on"[^>]*data-channel="1"/);
 assert.match(html,/class="scope-channel-row off"[^>]*data-channel="2"/);
 assert.match(html,/class="scope-channel-row unknown"[^>]*data-channel="4"/);
 assert.match(html,/id="scope-capture-2"[^>]*disabled/);
});

test('scope title leads directly into both cards followed by the controller status grid child',()=>{
 const idle=scope.renderScope({status:{devices:{scope:connected}},roles:{scope:ready}});
 const waiting=scope.renderScope({status:{devices:{scope:connected}},roles:{scope:ready},scopeRun:{status:'running',stage:'acquire',run_id:id(5),ui_started_at:Date.now()}});
 for(const html of [idle,waiting]){
  assert.match(html,/<div class="page-header scope-page-header"><h1>Oscilloscope<\/h1><button[^>]*data-op="scope-read"[^>]*>Sync<\/button><\/div><div class="scope-layout"><aside class="card scope-read-card">/);
  assert.equal((html.match(/data-op="scope-read"/g)||[]).length,1);
  assert.match(html,/<\/aside><section class="card scope-workbench">/);
  assert.match(html,/<\/section><div id="scope-status" class="scope-status"/);
  assert.match(html,/<\/div><\/div><details id="scope-advanced"/);
  assert.doesNotMatch(html,/id="scope-data-status"/);
  assert.doesNotMatch(html,/<div class="card-head"><h2>Channels<\/h2><button/);
 }
 assert.match(waiting.slice(waiting.indexOf('id="scope-status"'),waiting.indexOf('<details id="scope-advanced"')),/Reading stopped waveform/);
});

for(const [name,feedback,expected]of [
 ['preview failure',{scopePlotError:new Error('Viewport is offline')},/Waveform preview unavailable[\s\S]*Viewport is offline[\s\S]*data-wb="scope-reload"/],
 ['save failure',{scopeSaveError:new Error('Record storage is full')},/role="alert">Record storage is full/],
 ['pending export',{scopeExporting:true,scopeExportJob:{id:id(8),kind:'native_file',status:'running',stage:'Downloading export files',completed_units:4096,total_units:8192}},/workbench-job running[\s\S]*Downloading export files[\s\S]*data-wb="scope-export-cancel"/],
 ['unconfirmed release',{scopePreviewReleaseError:{cause:new Error('Host is offline'),reconciled:true}},/Preview memory release is unconfirmed[\s\S]*Host is offline[\s\S]*data-wb="scope-memory-check"[\s\S]*data-wb="scope-memory-retry"/]
])test(`waveform ${name} feedback stays outside the plot card and its footer`,()=>{
 const html=scope.renderScope({status:{devices:{scope:connected}},roles:{scope:ready},scopeRecord:record,scopePlotRecord:record,scopeRecordId:record.id,scopeLoadedRecord:record.id,scopeRecordSaved:false,...feedback});
 const workbench=html.match(/<section class="card scope-workbench">[\s\S]*?<\/section>/)?.[0];
 const dataStatus=html.match(/<div id="scope-data-status" class="scope-data-status"[^>]*>[\s\S]*?<\/div><\/div><details id="scope-advanced"/)?.[0];
 assert.ok(dataStatus,'data feedback must have its own final grid child');
 assert.match(dataStatus,expected);assert.doesNotMatch(workbench,expected);
 assert.match(workbench,/<span class="scope-save-state" role="status">Unsaved<\/span><\/div><\/div><\/section>$/);
 assert.match(html,/<div id="scope-status"[^>]*><p class="scope-readback hint">[^<]*<\/p><\/div><div id="scope-data-status"/);
 assert.equal((html.match(/id="scope-data-status"/g)||[]).length,1);
});

test('retained channel evidence stays visible as previous while controls remain gated',()=>{
 const previous={...connected,quality:'historical',previous:true};
 const html=scope.renderScope({status:{devices:{scope:previous}},roles:{scope:{...ready,unknown:true}},scopeCaptureInitialized:true,scopeCaptureChannels:[1,3]});
 assert.match(html,/class="scope-channel-row on"[^>]*data-channel="1"/);
 assert.match(html,/class="scope-channel-row off"[^>]*data-channel="2"/);
 assert.match(html,/id="scope-channel-1"[^>]*disabled>CH1<\/button><span class="scope-channel-state">On · Previous<\/span>/);
 assert.match(html,/id="scope-channel-2"[^>]*disabled>CH2<\/button><span class="scope-channel-state">Off · Previous<\/span>/);
 assert.match(html,/id="scope-capture-1"[^>]*checked[^>]*disabled/);
 assert.match(html,/class="scope-channel-row unknown"[^>]*data-channel="4"/);
 assert.match(html,/data-wb="scope-collect" disabled/);
 const empty=scope.renderScope({status:{devices:{scope:{connected:true,quality:'historical',previous:true}}},roles:{scope:ready}});
 assert.match(empty,/class="scope-channel-row unknown"[^>]*data-channel="1"/);
 assert.doesNotMatch(empty,/On · Previous|Off · Previous/);
});

test('faulted scope retains cached channel states as previous without reopening controls',()=>{
 const html=scope.renderScope({status:{devices:{scope:{...connected,connected:false,quality:'historical',previous:true}}},roles:{scope:{...ready,mode:'FAULT'}},scopeCaptureInitialized:true,scopeCaptureChannels:[1,3]});
 assert.match(html,/id="scope-channel-1"[^>]*disabled>CH1<\/button><span class="scope-channel-state">On · Previous<\/span>/);
 assert.match(html,/id="scope-channel-2"[^>]*disabled>CH2<\/button><span class="scope-channel-state">Off · Previous<\/span>/);
 assert.match(html,/id="scope-capture-1"[^>]*checked[^>]*disabled/);
 assert.match(html,/class="scope-channel-row unknown"[^>]*data-channel="4"/);
 assert.match(html,/data-op="scope-read" disabled/);
 assert.match(html,/data-wb="scope-collect" disabled/);
});

test('depth failure separates requested depth from previous evidence and retains its original error',()=>{
 const html=scope.renderScope({status:{devices:{scope:{...connected,connected:false,quality:'historical',previous:true,memory_depth:10000000,capabilities:{...connected.capabilities,memory_depths:[1000000,10000000]}}}},roles:{scope:{...ready,mode:'FAULT'}},inputs:new Map([['scope-depth','1000000']]),scopeSettingFailure:{action:{name:'set_memory_depth',args:{points:1000000}},error:new Error('memory depth application; state uncertain, no retry')}});
 assert.match(html,/Memory depth change failed/);
 assert.match(html,/Requested memory depth: 1M/);
 assert.match(html,/Last confirmed memory depth: 10M/);
 assert.match(html,/Current scope settings are unconfirmed/);
 assert.match(html,/memory depth application; state uncertain, no retry/);
 assert.match(html,/<option value="1000000" selected>1M<\/option>/);
 assert.doesNotMatch(html,/Acquisition outcome is uncertain|Disconnect and reconnect before/);
});

test('pending depth application identifies the request while keeping prior readings historical',()=>{
 const html=scope.renderScope({status:{devices:{scope:{...connected,memory_depth:10000000}}},roles:{scope:ready},scopeSettingsPending:true,scopeSettingAction:{name:'set_memory_depth',args:{points:1000000}}});
 assert.match(html,/Applying memory depth/);assert.match(html,/Requested memory depth: 1M/);
 assert.match(html,/Last confirmed memory depth: 10M/);assert.match(html,/Previous controller acquisition: running/);
 assert.match(html,/data-op="scope-read" disabled/);assert.match(html,/data-wb="scope-collect" disabled/);
});

for(const [name,label]of [['read_status','Scope settings read failed'],['set_channel_display','Channel display change failed']])test(`${name} failure is identified as settings work rather than an acquisition`,()=>{
 const html=scope.renderScope({status:{devices:{scope:{...connected,connected:false,quality:'historical',previous:true}}},roles:{scope:{...ready,mode:'FAULT'}},scopeSettingFailure:{action:{name,args:{}},error:new Error('Device reply timed out')}});
 assert.ok(html.includes(label));assert.match(html,/Device reply timed out/);
 assert.doesNotMatch(html,/Acquisition outcome is uncertain/);
});

test('fault without an acquisition attempt does not invent an acquisition failure',()=>{
 const html=scope.renderScope({status:{devices:{scope:{...connected,connected:false,quality:'historical',previous:true}}},roles:{scope:{...ready,mode:'FAULT'}}});
 assert.match(html,/Scope operation outcome is uncertain/);
 assert.doesNotMatch(html,/Acquisition outcome is uncertain|Disconnect and reconnect before/);
});

test('settings failure cannot present the unchanged confirmed cache as current even before a fault snapshot arrives',()=>{
 const html=scope.renderScope({status:{devices:{scope:{...connected,memory_depth:10000000}}},roles:{scope:ready},scopeCaptureInitialized:true,scopeCaptureChannels:[1],scopeSettingFailure:{action:{name:'set_memory_depth',args:{points:1000000}},error:new Error('memory depth application; state uncertain')}});
 assert.match(html,/Previous controller acquisition: running/);assert.match(html,/Previous controller memory depth: 10M/);
 assert.match(html,/On · Previous/);assert.match(html,/id="scope-channel-1"[^>]*disabled/);
 assert.match(html,/data-wb="scope-collect" disabled/);assert.match(html,/data-op="scope-read">Sync/);
});

test('Acquire depth application failure uses its explicit native operation and submitted definition',()=>{
 const raw='failed_after_call_started: '+JSON.stringify({error:{type:'DriverError',message:'Timeout { operation: "memory depth application; requested=1000000 last_reported=10000000; state uncertain, no retry", transferred: 0 }'}});
 const html=scope.renderScope({status:{devices:{scope:{...connected,connected:false,quality:'historical',previous:true,memory_depth:10000000}}},roles:{scope:{...ready,mode:'FAULT'}},inputs:new Map([['scope-depth','250000000']]),scopeRun:{run_id:id(5),status:'uncertain',stage:'acquire',error:{code:'OutcomeUnknown',message:raw},result:{run:{definition:{parameters:{source:'memory',memory_depth:1000000}}}}}});
 assert.match(html,/Memory depth application failed/);
 assert.match(html,/Requested memory depth: 1M/);assert.match(html,/Last confirmed memory depth: 10M/);
 assert.match(html,/Current scope settings are unconfirmed/);
 assert.match(html,/data-wb="scope-check"/);assert.match(html,/id="scope-run-details"/);
 assert.doesNotMatch(html,/Reading stopped waveform|Acquisition outcome is uncertain|Requested memory depth: 250M|no (?:VISA )?data (?:was )?transferred/i);
});

test('Acquire depth failure without a submitted definition never treats the current draft as its request',()=>{
 const html=scope.renderScope({status:{devices:{scope:{...connected,connected:false,quality:'historical',previous:true,memory_depth:10000000}}},roles:{scope:{...ready,mode:'FAULT'}},inputs:new Map([['scope-depth','1000000']]),scopeRun:{run_id:id(5),status:'uncertain',stage:'acquire',error:{message:'Timeout { operation: "memory depth application", transferred: 0 }'}}});
 assert.match(html,/Memory depth application failed/);assert.doesNotMatch(html,/Requested memory depth:/);
 assert.doesNotMatch(html,/Reading stopped waveform|Acquisition outcome is uncertain/);
});

test('Acquire depth failure arriving before its fault snapshot gates stale READY readback until explicit recovery',()=>{
 const state={status:{devices:{scope:{...connected,memory_depth:10000000}}},roles:{scope:ready},scopeCaptureInitialized:true,scopeCaptureChannels:[1],scopeRun:{run_id:id(5),status:'uncertain',stage:'acquire',error:{message:'Timeout { operation: "memory depth application", transferred: 0 }'}}};
 const blocked=scope.renderScope(state);
 assert.match(blocked,/Previous controller acquisition: running/);assert.match(blocked,/Previous controller memory depth: 10M/);
 assert.match(blocked,/On · Previous/);assert.match(blocked,/id="scope-channel-1"[^>]*disabled/);
 assert.match(blocked,/data-wb="scope-collect" disabled/);assert.match(blocked,/data-op="scope-read">Sync/);
 const recovered=scope.renderScope({...state,scopeDepthRecoveredRunId:id(5)});
 assert.match(recovered,/Controller acquisition: running/);assert.match(recovered,/data-wb="scope-collect">Acquire/);
 assert.doesNotMatch(recovered,/Current scope settings are unconfirmed|On · Previous/);
 assert.match(scope.renderScope({...state,scopeDepthRecoveredRunId:id(6)}),/data-wb="scope-collect" disabled/);
});

for(const [operation,label]of [['memory depth preflight','Memory depth preflight failed'],['depth acquisition-state confirmation','Memory depth acquisition-state confirmation failed'],['depth STOP restoration','Memory depth STOP restoration failed']])test(`Acquire identifies ${operation} before claiming waveform transfer`,()=>{
 const html=scope.renderScope({roles:{scope:{...ready,mode:'FAULT'}},scopeRun:{run_id:id(5),status:'uncertain',stage:'acquire',error:{message:`Timeout { operation: "${operation}; state uncertain, no retry", transferred: 0 }`}}});
 assert.ok(html.includes(label));assert.doesNotMatch(html,/Reading stopped waveform|Acquisition outcome is uncertain/);
});

test('historical or unknown readback cannot preselect capture channels',()=>{
 for(const state of [{device:{...connected,previous:true},role:ready},{device:{...connected,quality:'historical'},role:ready},{device:connected,role:{...ready,unknown:true}}]){
  assert.doesNotMatch(scope.renderScope({status:{devices:{scope:state.device}},roles:{scope:state.role}}),/id="scope-capture-\d"[^>]* checked/);
 }
});

test('an identity-only connected scope offers a visible status read before Read & Save',()=>{
 const html=scope.renderScope({status:{devices:{scope:{connected:true,quality:'historical',previous:true,identity:'RIGOL,DS7054,BENCH,1.0'}}},roles:{scope:ready}});
 const main=html.slice(0,html.indexOf('<details id="scope-advanced"'));
 assert.match(main,/data-op="scope-read">Sync/);
 assert.match(main,/data-wb="scope-collect" disabled/);
});

test('STOP_HELD exposes Resume controls only for its verified current safety epoch',()=>{
 const held={...ready,mode:'STOP_HELD',stopHeld:true,context:{session_id:id(5),connection_id:id(6),epoch:2},safetyEpoch:2,safety:{state:'STOP_HELD'}};
 const html=scope.renderScope({status:{devices:{scope:connected}},roles:{scope:held}});
 assert.match(html,/data-op="resume" data-role="scope">Resume controls/);
 assert.match(html,/does not send RUN/);
 assert.match(html,/data-wb="scope-collect" disabled/);
 for(const role of [{...held,safetyEpoch:1},{...held,unknown:true},{...held,hostRestricted:true},{...held,normalPending:true}])assert.doesNotMatch(scope.renderScope({status:{devices:{scope:connected}},roles:{scope:role}}),/data-op="resume"/);
});

test('scope preview moves capture details out of the plot while keeping record naming',()=>{
 const html=scope.renderScope({status:{devices:{scope:connected}},roles:{scope:ready},scopeRecord:record,scopeRecordId:record.id,scopeLoadedRecord:record.id,scopeAcquisition:'fresh',scopeViews:[]});
 assert.doesNotMatch(html,/Existing screen|Read at|scope-record-context|Frame 1/);
 assert.match(html,/Bench screen/);assert.match(html,/id="scope-record-name"/);
 assert.doesNotMatch(html,/fresh trigger|synchronized/i);
});

test('structured acquisition errors render readable English with collapsed raw Details',()=>{
 const html=scope.renderScope({scopeRun:{status:'failed',stage:'failed',error:{code:'CaptureFailed',message:JSON.stringify({code:'Transfer',message:'No waveform data returned'})}}});
 assert.match(html,/No waveform data returned/);
 const details=html.indexOf('<details id="scope-run-details"');assert.ok(details>0);
 assert.doesNotMatch(html.slice(0,details),/\{&quot;code&quot;/);
 assert.match(html.slice(details),/<summary>Details/);
});

test('loop native stages render understandable waiting feedback without claiming a stopped final state',()=>{
 for(const [stage,label] of [['resume_scope','Starting acquisition'],['wait_interval','Waiting for next frame']]){
  const html=scope.renderScope({scopeRun:{status:'running',stage,run_id:id(5)}});assert.match(html,new RegExp(label));assert.match(html,/activity-spinner/);
 }
 assert.doesNotMatch(scope.renderScope({scopeRun:{status:'cancelled',stage:'cancelled'}}),/Scope is stopped/);
});

test('an uncertain native call error unwraps its prefixed JSON while keeping the raw evidence',()=>{
 const raw='failed_after_call_started: '+JSON.stringify({code:'Timeout',message:'Screen transfer timed out'});
 const html=scope.renderScope({scopeRun:{status:'uncertain',stage:'acquire',run_id:id(5),error:raw}});
 const details=html.indexOf('<details id="scope-run-details"');
 assert.ok(html.indexOf('data-wb="scope-check"')<html.indexOf('class="scope-job-error"'),'original-status action must precede long error text in the status area');
 assert.match(html.slice(0,details),/>Screen transfer timed out</);
 assert.doesNotMatch(html.slice(0,details),/failed_after_call_started|&quot;code&quot;/);
 assert.match(html.slice(details),/failed_after_call_started/);
 const fault=scope.renderScope({status:{devices:{scope:connected}},roles:{scope:{...ready,mode:'FAULT'}},scopeRun:{status:'uncertain',stage:'acquire',run_id:id(5),error:raw}});
 assert.ok(fault.indexOf('data-wb="scope-check"')<fault.indexOf('Acquisition outcome is uncertain'),'recovery action must stay visible before fault explanation');
});

test('saved plot failure preserves completed acquisition and offers saved-data-only recovery',async()=>{
 assert.equal(typeof scope.loadSavedScopeRecord,'function');
 const run={run_id:id(5),status:'completed',record_ids:[record.id]},local={scopeRun:run,scopeViews:[]};
 await scope.loadSavedScopeRecord({owner:{dataGet:async()=>({record}),dataViewport:async()=>{throw new Error('Saved viewport unavailable');}},local,recordId:record.id});
 assert.equal(local.scopeRun,run);assert.equal(local.scopeRun.status,'completed');assert.equal(local.scopeRecordId,record.id);
 assert.equal(local.scopePlotBusy,false);assert.match(local.scopePlotError.message,/viewport unavailable/);
 const html=scope.renderScope(local);
 assert.match(html,/Waveform preview unavailable/);assert.match(html,/data-wb="scope-reload"/);
 assert.doesNotMatch(html,/data-wb="scope-analyze"/);assert.match(html,/data-wb="scope-export"/);
 assert.doesNotMatch(html,/uncertain/);
});

test('loading saved plots requests bounded viewports for captured channels and labels retained plots previous',async()=>{
 assert.equal(typeof scope.loadSavedScopeRecord,'function');
 let release;const pending=new Promise(resolve=>release=resolve),requests=[],local={scopeViews:[{channel:'CH2',points:[]}],scopeLoadedRecord:id(7)};
 const work=scope.loadSavedScopeRecord({owner:{dataGet:async()=>{await pending;return{record};},dataViewport:async p=>{requests.push(p);return{channel:p.channel,points:[],total_points:p.end};}},local,recordId:record.id});
 assert.equal(local.scopePlotBusy,true);assert.match(scope.renderScope(local),/Previous waveform/);
 release();await work;
 assert.deepEqual(requests,[{record_id:record.id,frame_id:frame.id,channel:'CH1',start:0,end:100,max_points:2048},{record_id:record.id,frame_id:frame.id,channel:'CH3',start:0,end:200,max_points:2048}]);
 assert.deepEqual(local.scopeViews.map(v=>v.channel),['CH1','CH3']);assert.equal(local.scopeLoadedRecord,record.id);
 assert.deepEqual(local.scopeHiddenChannels,[]);
});
