import test from 'node:test';
import assert from 'node:assert/strict';
import {scopeAction,scopeCaptureDefinition,renderScope} from '../web/scope.js';
import {generatorAction,renderGenerator} from '../web/generator.js';
import {createAnalysisWorkbench,renderAnalysis} from '../web/analysis.js';
import {createExperimentsWorkbench,buildExperimentDefinition,renderExperiments} from '../web/experiments.js';
import {roiFromDrag,renderWaveformPlot} from '../web/waveform-plot.js';
import {renderDataBrowser,renderJob,tickWorkbenchJobs} from '../web/data-browser.js';
const id=n=>String(n).repeat(32),calibration={x_increment_s:1e-6,x_origin_s:0,x_reference:0,y_increment_v:.01,y_origin:0,y_reference:128};
const record={schema:1,id:id(1),name:'Saved trace',comment:'Imported history',created_at_unix_ms:1000,source:{},frames:[{id:id(2),captured_at_unix_ms:1000,channels:[{channel:'CH1',points:100,calibration},{channel:'CH2',points:100,calibration}]}],analyses:[]};
const fields=values=>key=>String(values[key]??'');
test('uncertain scope capture labels cached state as previous and explains recovery without replay',()=>{
 const html=renderScope({status:{devices:{scope:{connected:true,acquisition:'running',previous:true}}},roles:{scope:{confirmed:true,mode:'FAULT'}},scopeRun:{run_id:id(5),status:'uncertain',elapsed_ms:10000,error:{message:'Single acquisition timed out'}}});
 assert.match(html,/Previous controller acquisition: running/);
 assert.match(html,/Single acquisition timed out/);
 assert.match(html,/Check original status/);
 assert.match(html,/Diagnostics for the original status/);
 assert.doesNotMatch(html,/Disconnect and reconnect/);
 assert.match(html,/not retried automatically/);
 assert.match(html,/data-wb="scope-collect" disabled/);
});
test('confirmed scope readback is not mislabeled as a previous observation',()=>{
 const html=renderScope({status:{devices:{scope:{connected:true,acquisition:'stopped',previous:false,quality:'confirmed'}}},roles:{scope:{confirmed:true,mode:'READY'}}});
 assert.match(html,/Controller acquisition: stopped/);
 assert.doesNotMatch(html,/Previous controller acquisition|Disconnect and reconnect/);
 assert.match(html,/data-wb="scope-collect">Acquire/);
});
test('fresh readback after reconnect is not hidden by an earlier uncertain capture',()=>{
 const html=renderScope({status:{devices:{scope:{connected:true,acquisition:'waiting',previous:false,quality:'confirmed'}}},roles:{scope:{confirmed:true,mode:'READY'}},scopeRun:{status:'uncertain',error:{message:'Earlier acquisition timed out'}}});
 assert.match(html,/Controller acquisition: waiting/);
 assert.doesNotMatch(html,/Previous controller acquisition|Disconnect and reconnect/);
 assert.match(html,/Earlier acquisition timed out/);
 assert.match(html,/data-wb="scope-collect">Acquire/);
});
test('uncertain UI status does not prescribe reconnect without an instrument fault',()=>{
 for(const role of [{confirmed:true,mode:'READY'},{confirmed:false,mode:'READY',unknown:true}]){
  const html=renderScope({status:{devices:{scope:{connected:true,acquisition:'stopped',previous:true}}},roles:{scope:role},scopeRun:{run_id:id(5),status:'uncertain',error:{message:'Status unavailable'}}});
  assert.match(html,/Previous controller acquisition: stopped/);
  assert.match(html,/Check original status/);
  assert.doesNotMatch(html,/Disconnect and reconnect/);
 }
});
test('scope separates instrument display from selected channels and validates repeated capture',()=>{
 const action=scopeAction('scope-display',fields({}),{channel:'2',enabled:'true'});
 assert.deepEqual(action,{name:'set_channel_display',args:{channel:2,enabled:true}});
 const def=scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get:fields({'scope-source':'memory','scope-depth':1000,'scope-frames':3,'scope-interval':.1}),channels:[1,3]});
 assert.equal(def.template_id,'waveform_capture');assert.equal(def.parameters.interval_ms,100);assert.deepEqual(def.parameters.channels,[1,3]);assert.equal(def.roles[0].domain_id,id(4));
 assert.throws(()=>scopeCaptureDefinition({hostId:id(3),deviceId:id(4),get:fields({'scope-source':'screen','scope-frames':10000,'scope-interval':0}),channels:[1]}),/frames/i);
 const html=renderScope({status:{devices:{scope:{connected:true,channels:[{channel:1,displayed:false}],capabilities:{channel_count:4,memory_depths:[1000,10000]}}}},roles:{scope:{confirmed:true,mode:'READY'}}});
 assert.match(html,/scope-capture-1/);assert.match(html,/data-op="scope-display"/);assert.match(html,/scope-source/);assert.match(html,/10000/);
 assert.match(html,/id="scope-resume"/);assert.equal(def.parameters.acquisition,'stopped');assert.equal(def.parameters.resume_after_capture,true);
});
test('generator validates complete signal before producing explicit channel commands',()=>{
 const get=fields({'generator-2-waveform':'triangle','generator-2-frequency':50,'generator-2-amplitude':2,'generator-2-offset':0});
 assert.deepEqual(generatorAction('generator-apply',get,{channel:'2'}),{name:'configure_signal',args:{channel:2,waveform:'triangle',frequency_hz:50,amplitude_vpp:2,offset_v:0}});
 assert.deepEqual(generatorAction('generator-output-off',get,{channel:'2'}),{name:'set_output',args:{channel:2,enabled:false}});
 assert.throws(()=>generatorAction('generator-apply',fields({'generator-2-waveform':'triangle','generator-2-frequency':0,'generator-2-amplitude':2,'generator-2-offset':0}),{channel:2}),/frequency/i);
 assert.match(renderGenerator({status:{devices:{}},roles:{generator:{confirmed:false}}}),/data-op="generator-apply"[^>]*disabled/);
 assert.deepEqual(generatorAction('generator-phase',fields({'generator-1-phase':90}),{channel:1}),{name:'set_phase',args:{channel:1,degrees:90}});
 assert.throws(()=>generatorAction('generator-phase',fields({'generator-1-phase':-90}),{channel:1}),/phase/i);
});
test('generator toggle requires confirmed boolean readback and produces an explicit inverse command',()=>{
 const device={connected:true,quality:'confirmed',previous:false,channels:[{channel:1,output_enabled:false},{channel:2,output_enabled:true}]};
 assert.deepEqual(generatorAction('generator-output-toggle',()=>'',{channel:1,device}),{name:'set_output',args:{channel:1,enabled:true}});
 assert.deepEqual(generatorAction('generator-output-toggle',()=>'',{channel:2,device}),{name:'set_output',args:{channel:2,enabled:false}});
 for(const change of [{connected:false},{quality:'historical'},{previous:true},{channels:[{channel:1}]},{channels:[]}])assert.throws(()=>generatorAction('generator-output-toggle',()=>'',{channel:1,device:{...device,...change}}),/sync|readback/i);
});
test('ROI uses absolute sample indices and bounded plots reject oversized native payloads',()=>{
 assert.deepEqual(roiFromDrag(80,20,{left:0,width:100},{start:100,end:1100}),{start:300,end:900});
 assert.equal(roiFromDrag(30,31,{left:0,width:100},{start:100,end:1100}),null);
 assert.throws(()=>renderWaveformPlot([{channel:'CH1',points:Array.from({length:8193},(_,x)=>({x,y:0})),calibration}]),/bounded/i);
 const html=renderWaveformPlot([{channel:'CH1',points:[{x:120,y:1},{x:180,y:2}],calibration}],{roi:{start:130,end:170}});
 assert.match(html,/data-waveform-plot/);assert.match(html,/CH1/);assert.match(html,/Time \(µs\)/);
});

test('channel colors retain physical identity when a subset is plotted',()=>{
 const html=renderWaveformPlot([{channel:'CH3',points:[{x:0,y:1},{x:1,y:2}]}]);
 assert.match(html,/--trace-color:#159677/);assert.match(html,/stroke="#159677"/);
});
test('analysis selection rejects an old viewport and never transfers raw samples',async()=>{
 let release;const slow=new Promise(resolve=>release=resolve),calls=[];
 const client={dataList:async()=>({records:[{id:record.id,name:record.name,frame_count:1}]}),dataGet:async()=>({record}),dataViewport:async p=>{calls.push(p);if(p.channel==='CH1')await slow;return {...p,points:[{x:0,y:1},{x:99,y:2}],calibration,total_points:100};}};
 const wb=createAnalysisWorkbench({client});await wb.refresh();const first=wb.load(record.id);await new Promise(r=>setImmediate(r));await wb.selectChannel('CH2');release();await first;
 assert.equal(wb.state.channel,'CH2');assert.equal(wb.state.views[0].channel,'CH2');assert.ok(calls.every(p=>p.max_points<=8192));assert.equal(wb.state.record.frames[0].channels[0].samples,undefined);
 assert.match(renderAnalysis(wb.state),/Saved trace/);assert.match(renderAnalysis(wb.state),/Imported history/);
});
test('analysis stale result remains historical after ROI changes while a fit is pending',async()=>{
 const result={id:id(6),record_id:record.id,frame_id:id(2),channel:'CH1',roi:{start:0,end:100},method:'single_drop',algorithm:'lorentzian_v1',result:{q:1000},curve:[],markers:[]};
 let response={id:id(5),status:'running',stage:'fit',elapsed_ms:10};
 const client={dataGet:async()=>({record}),dataViewport:async p=>({...p,points:[{x:0,y:1},{x:99,y:2}],calibration,total_points:100}),analysisStart:async()=>response,analysisStatus:async()=>response,jobCancel:async()=>({requested:true})};
 const wb=createAnalysisWorkbench({client});await wb.load(record.id);await wb.startAnalysis(fields({'analysis-method':'single_drop','analysis-excursion':10,'analysis-ramp':50,'analysis-wavelength':1550,'analysis-speed':1,'analysis-prominence':.1,'analysis-distance':5}));
 wb.setRoi({start:10,end:50});response={...response,status:'completed',result};await wb.poll();assert.equal(wb.state.analysis,null);assert.equal(wb.state.historicalAnalyses.length,1);
 assert.match(renderAnalysis(wb.state),/Historical result/);assert.doesNotMatch(renderAnalysis(wb.state),/NaN/);
});
test('analysis acknowledges slow work immediately and cancellation waits for native confirmation',async()=>{
 let release;const slow=new Promise(resolve=>release=resolve);let native={id:id(5),status:'running',stage:'fit',elapsed_ms:50};
 const wb=createAnalysisWorkbench({client:{analysisStart:async()=>{await slow;return native;},jobCancel:async()=>({requested:true}),analysisStatus:async()=>native}});
 wb.state.record=record;wb.state.frameId=id(2);wb.state.channel='CH1';wb.state.roi={start:0,end:100};
 const work=wb.startAnalysis(fields({'analysis-method':'fsr','analysis-excursion':10,'analysis-ramp':50,'analysis-wavelength':1550,'analysis-speed':1,'analysis-prominence':.1,'analysis-distance':5}));
 assert.equal(wb.state.job.status,'queued');assert.match(renderJob(wb.state.job),/queued/);release();await work;await wb.cancel();assert.equal(wb.state.job.status,'running');assert.equal(wb.state.job.cancel_requested,true);native={...native,status:'cancelled'};await wb.poll();assert.equal(wb.state.job.status,'cancelled');
});
test('experiment binds explicit local and remote roles, saves before run and preserves partial result links',async()=>{
 const devices=[{host_id:id(3),domain_id:id(4),name:'Scope',family:'scope',channels:4},{host_id:id(7),domain_id:id(8),name:'Generator',family:'generator',channels:2},{host_id:id(3),domain_id:id(9),name:'Laser',family:'laser',channels:1}];
 const get=fields({'experiment-name':'Q run','experiment-role-detector':id(3)+'/'+id(4),'experiment-role-scan_signal':id(7)+'/'+id(8),'experiment-role-laser':id(3)+'/'+id(9),'experiment-channel-detector':2,'experiment-channel-scan_signal':1,'experiment-source':'screen','experiment-frames':2,'experiment-interval':0,'experiment-frequency':50,'experiment-amplitude':2,'experiment-offset':0,'experiment-excursion':10,'experiment-wavelength':1550,'experiment-speed':1,'experiment-prominence':.1,'experiment-distance':10});
 const definition=buildExperimentDefinition('q_scan',get,devices);assert.equal(definition.roles[1].host_id,id(7));assert.equal(definition.roles[0].channel,2);
 let saved;const wb=createExperimentsWorkbench({client:{experimentSave:async p=>{saved=p.definition;return{definition:saved};},experimentStart:async p=>{assert.equal(p.definition.id,saved.id);return{run_id:id(6),status:'queued',stage:'preflight'};},experimentStatus:async()=>({run_id:id(6),status:'failed',stage:'persist',record_ids:[id(1)],error:'Disconnected'}),experimentList:async()=>({experiments:[],runs:[]})},devices:()=>devices});
 wb.state.templateId='q_scan';await wb.run(get);await wb.poll();assert.equal(wb.state.run.status,'failed');assert.match(renderExperiments(wb.state,devices),/Analyze/);assert.match(renderExperiments(wb.state,devices),/Disconnected/);
});
test('uncertain job is terminal and measured progress is the only source of percentages',()=>{
 const unknown=renderJob({status:'uncertain',stage:'transfer',elapsed_ms:6000,error:'Connection lost'});assert.match(unknown,/uncertain/);assert.doesNotMatch(unknown,/<progress/);
 assert.doesNotMatch(renderJob({status:'running',stage:'fit',elapsed_ms:100}),/\d+%/);
 assert.match(renderJob({status:'running',stage:'capture',elapsed_ms:100,completed_units:2,total_units:4}),/50%/);
});

test('imported frames without an acquisition timestamp do not display a fabricated epoch date',()=>{
 for(const timestamp of [0,null,undefined,-1,NaN,1e30]){
  const imported={...record,frames:[{...record.frames[0],captured_at_unix_ms:timestamp}]};
  const html=renderDataBrowser({records:[imported],record:imported,frameId:id(2)});
  assert.match(html,/Acquisition time unavailable/);
  assert.doesNotMatch(html,/1969|1970|Invalid Date/);
 }
 const html=renderDataBrowser({records:[record],record,frameId:id(2)});
 assert.ok(html.includes(new Date(1000).toLocaleString()));
 assert.doesNotMatch(html,/Acquisition time unavailable/);
});

test('terminal jobs show status once while preserving a distinct outcome detail',()=>{
 for(const status of ['completed','failed','cancelled','uncertain']){
  assert.doesNotMatch(renderJob({status,stage:status}),new RegExp(`${status} · ${status}`));
 }
 assert.match(renderJob({status:'completed',stage:'Imported 2 records'}),/completed · Imported 2 records/);
});

test('analysis feedback remains next to Analyze through delayed submission and native failure',async()=>{
 let release;const pending=new Promise(resolve=>release=resolve);
 let status={id:id(5),status:'running',stage:'Fitting original samples',elapsed_ms:1200};
 const wb=createAnalysisWorkbench({client:{analysisStart:async()=>{await pending;return status;},analysisStatus:async()=>status,workbenchImport:async()=>({cancelled:true})}});
 wb.state.record=record;wb.state.frameId=id(2);wb.state.channel='CH1';wb.state.roi={start:0,end:100};
 const work=wb.startAnalysis(fields({'analysis-method':'fsr','analysis-excursion':10,'analysis-ramp':50,'analysis-wavelength':1550,'analysis-speed':1,'analysis-prominence':.1,'analysis-distance':5}));
 const inline=()=>renderAnalysis(wb.state).split('class="analysis-inline-job">')[1]?.split('</div>')[0]||'';
 assert.match(inline(),/Submitting analysis/);assert.match(inline(),/activity-spinner/);
 release();await work;
 assert.equal(wb.state.jobPurpose,'analysis');assert.match(inline(),/Fitting original samples/);
 status={...status,status:'failed',stage:'failed',error:'Not enough resonance dips'};await wb.poll();
 assert.equal(wb.state.jobPurpose,'analysis');assert.match(inline(),/Not enough resonance dips/);
 await wb.action('analysis-append',fields({}));assert.equal(wb.state.jobPurpose,'import');assert.equal(inline(),'');
});

test('starting another fit immediately marks the previous result historical',async()=>{
 const previous={id:id(6),record_id:record.id,frame_id:id(2),channel:'CH1',roi:{start:0,end:100},method:'single_drop',algorithm:'lorentzian_v1',result:{q:1000}};
 const wb=createAnalysisWorkbench({client:{analysisStart:async()=>({id:id(5),status:'failed',stage:'failed',error:'No dips'})}});
 wb.state.record=record;wb.state.frameId=id(2);wb.state.channel='CH1';wb.state.roi={start:0,end:100};wb.state.analysis=previous;
 const work=wb.startAnalysis(fields({'analysis-method':'fsr','analysis-excursion':10,'analysis-ramp':50,'analysis-wavelength':1550,'analysis-speed':1,'analysis-prominence':.1,'analysis-distance':5}));
 assert.equal(wb.state.analysis,null);assert.deepEqual(wb.state.historicalAnalyses,[previous]);
 await work;assert.equal(wb.state.analysis,null);
 const html=renderAnalysis(wb.state);assert.match(html,/Historical result/);assert.doesNotMatch(html,/<h3>Analysis result/);
});
test('metadata frame pagination preserves source count and selects a page without raw arrays',async()=>{
 const calls=[],wb=createAnalysisWorkbench({client:{dataGet:async p=>{calls.push(p);return {record:{...record,frames:p.frame_offset?[{...record.frames[0],id:id(7)}]:record.frames},frame_count:300,frame_offset:p.frame_offset||0,more:(p.frame_offset||0)<256};},dataViewport:async p=>({...p,points:[{x:0,y:1},{x:99,y:2}],calibration,total_points:100})}});
 await wb.load(record.id);assert.equal(wb.state.frameCount,300);assert.equal(typeof wb.framePage,'function');await wb.framePage(64);assert.equal(calls[1].frame_offset,64);assert.equal(wb.state.frameId,id(7));assert.match(renderAnalysis(wb.state),/300 frames/);
});
test('a result from another record is linked to its source rather than attached to the current record',async()=>{
 let status={id:id(5),status:'running',stage:'fit'},wb=createAnalysisWorkbench({client:{analysisStart:async()=>status,analysisStatus:async()=>status}});wb.state.record=record;wb.state.frameId=id(2);wb.state.channel='CH1';wb.state.roi={start:0,end:100};
 await wb.startAnalysis(fields({'analysis-method':'single_drop','analysis-excursion':10,'analysis-ramp':50,'analysis-wavelength':1550,'analysis-speed':1,'analysis-prominence':.1,'analysis-distance':5}));wb.state.record={...record,id:id(8)};status={...status,status:'completed',result:{record_id:record.id,frame_id:id(2),channel:'CH1',roi:{start:0,end:100},result:{q:1000}}};await wb.poll();assert.equal(wb.state.historicalAnalyses.length,0);assert.equal(wb.state.staleResult.record_id,record.id);
});
test('multichannel overlay maps each calibrated time axis rather than assuming equal intervals',()=>{
 const html=renderWaveformPlot([{channel:'CH1',points:[{x:0,y:1},{x:99,y:2}],calibration},{channel:'CH2',points:[{x:0,y:1},{x:99,y:2}],calibration:{...calibration,x_increment_s:2e-6}}]);
 const paths=[...html.matchAll(/<path d="([^"]+)" fill="none" stroke="/g)].map(m=>Number(m[1].split('L').at(-1).split(',')[0]));assert.ok(paths[0]<paths[1],'the shorter time extent must end before the longer channel');
});
test('elapsed feedback advances during a pending native exchange without replacing controls',()=>{
 const node={dataset:{jobStarted:'1000',jobNativeElapsed:'500'},textContent:''};let selectors=[];tickWorkbenchJobs({querySelectorAll:selector=>{selectors.push(selector);return[node];}},6000);assert.equal(node.textContent,'5 s elapsed');assert.deepEqual(selectors,['[data-job-started]']);
});
test('saved fit loading restores source frame, channel and ROI with a bounded overlay',async()=>{
 const fit={id:id(6),record_id:record.id,frame_id:id(2),channel:'CH2',roi:{start:10,end:80},method:'fsr',algorithm:'fsr_v1',result:{fsr_nm:.1},curve:[],markers:[]};
 const wb=createAnalysisWorkbench({client:{dataGet:async()=>({record}),dataViewport:async p=>({...p,points:[{x:p.start,y:1},{x:p.end-1,y:2}],calibration,total_points:100}),analysisResult:async()=>({analysis:fit})}});await wb.load(record.id);assert.equal(typeof wb.loadAnalysis,'function');await wb.loadAnalysis(fit.id);assert.equal(wb.state.channel,'CH2');assert.deepEqual(wb.state.roi,{start:10,end:80});assert.equal(wb.state.analysis.id,fit.id);
});

test('selected analysis channel remains the explicit visible ROI reference',async()=>{
 const wb=createAnalysisWorkbench({client:{dataGet:async()=>({record}),dataViewport:async p=>({...p,points:[{x:0,y:1},{x:99,y:2}],calibration,total_points:100})}});
 await wb.load(record.id);await wb.toggleDisplay('CH2',true);
 await assert.rejects(wb.toggleDisplay('CH1',false),/selected.*reference/i);
 assert.deepEqual(wb.state.displayChannels,['CH1','CH2']);assert.equal(wb.state.views[0].channel,wb.state.channel);
 await wb.toggleDisplay('CH2',false);assert.deepEqual(wb.state.displayChannels,['CH1']);
});

test('native export file outcomes are visible and all-file failure is never reported as success',async()=>{
 let files=[{name:'trace.csv',error:'Disk full'}];
 const wb=createAnalysisWorkbench({client:{workbenchExport:async()=>({files,export_job_id:id(5)}),dataList:async()=>({records:[]})}});wb.state.record=record;
 await wb.action('analysis-export',fields({'analysis-export-format':'csv'}));
 assert.equal(wb.state.job.status,'failed');assert.match(wb.state.job.error,/1.*failed/i);assert.match(renderAnalysis(wb.state),/Disk full/);
 files=[{name:'trace.csv',path:'D:/exports/trace.csv'},{name:'trace.metadata.json',error:'Permission denied'}];
 await wb.action('analysis-export',fields({'analysis-export-format':'csv'}));
 assert.equal(wb.state.job.status,'completed');assert.match(wb.state.job.stage,/1 succeeded.*1 failed/i);assert.match(renderAnalysis(wb.state),/Permission denied/);
});

test('Load replacement requires confirmation but Append preserves the saved collection',async()=>{
 const confirmations=[],calls=[];const wb=createAnalysisWorkbench({confirm:message=>{confirmations.push(message);return false;},client:{workbenchImport:async p=>{calls.push(p);return{cancelled:true};}}});wb.state.records=[record];
 await wb.action('analysis-load',fields({}));assert.equal(calls.length,0);assert.match(confirmations[0],/replaces.*saved waveform records/i);
 await wb.action('analysis-append',fields({}));assert.deepEqual(calls,[{append:true}]);assert.equal(confirmations.length,1);
});

test('saved result pending viewport cannot attach after the user changes channel',async()=>{
 let release,hold=false;const pending=new Promise(resolve=>release=resolve),fit={id:id(6),record_id:record.id,frame_id:id(2),channel:'CH1',roi:{start:10,end:80},method:'fsr',algorithm:'fsr_v1',result:{fsr_nm:.1},curve:[],markers:[]};
 const wb=createAnalysisWorkbench({client:{dataGet:async()=>({record}),analysisResult:async()=>({analysis:fit}),dataViewport:async p=>{if(hold&&p.channel==='CH1')await pending;return {...p,points:[{x:p.start,y:1},{x:p.end-1,y:2}],calibration,total_points:100};}}});
 await wb.load(record.id);hold=true;const saved=wb.loadAnalysis(fit.id);await new Promise(r=>setImmediate(r));await wb.selectChannel('CH2');release();await saved;
 assert.equal(wb.state.channel,'CH2');assert.equal(wb.state.analysis,null);
});

test('off-page saved result cannot replace a newer frame selection in the same record',async()=>{
 let release,entered;const pending=new Promise(resolve=>release=resolve),ready=new Promise(resolve=>entered=resolve);
 const page={...record,frames:[...record.frames,{...record.frames[0],id:id(7)}]},fit={id:id(6),record_id:record.id,frame_id:id(9),channel:'CH1',roi:{start:10,end:80},method:'fsr',algorithm:'fsr_v1',result:{fsr_nm:.1},curve:[],markers:[]};
 const wb=createAnalysisWorkbench({client:{dataGet:async p=>{if(p.frame_id===id(9)){entered();await pending;return {record:{...record,frames:[{...record.frames[0],id:id(9)}]}};}return {record:page};},analysisResult:async()=>({analysis:fit}),dataViewport:async p=>({...p,points:[{x:p.start,y:1},{x:p.end-1,y:2}],calibration,total_points:100})}});
 await wb.load(record.id);const saved=wb.loadAnalysis(fit.id);await ready;await wb.selectFrame(id(7));release();await saved;
 assert.equal(wb.state.frameId,id(7));assert.equal(wb.state.views[0].frame_id,id(7));assert.equal(wb.state.analysis,null);assert.equal(wb.state.error,null);
});

test('GUI-owned native file jobs use their own progress and cancellation bridge',async()=>{
 const calls=[];let status={id:id(5),kind:'native_file',status:'running',stage:'Uploading selected file',completed_units:16384,total_units:32768};
 const wb=createAnalysisWorkbench({client:{workbenchImport:async()=>status,workbenchFileStatus:async p=>{calls.push(['status',p]);return status;},workbenchFileCancel:async p=>{calls.push(['cancel',p]);return {...status,cancel_requested:true};},dataList:async()=>({records:[]})}});
 await wb.action('analysis-append',fields({}));assert.match(renderJob(wb.state.job),/50%/);await wb.cancel();assert.deepEqual(calls.map(([method])=>method),['cancel','status']);assert.equal(wb.state.job.status,'running');
 status={...status,status:'cancelled'};await wb.poll();assert.equal(wb.state.job.status,'cancelled');
});

test('native file job completed before submission reply still publishes per-file outcomes',async()=>{
 const wb=createAnalysisWorkbench({client:{workbenchExport:async()=>({id:id(5),kind:'native_file',status:'completed',result:{files:[{name:'trace.csv',error:'Disk full'}]}}),dataList:async()=>({records:[]})}});wb.state.record=record;
 await wb.action('analysis-export',fields({'analysis-export-format':'csv'}));assert.equal(wb.state.job.status,'failed');assert.match(renderAnalysis(wb.state),/Disk full/);
});

test('persisted native RunRecord history retains Analyze links and stable run identity',async()=>{
 const wb=createExperimentsWorkbench({client:{experimentTemplates:async()=>({templates:[]}),experimentList:async()=>({runs:[{id:id(9),status:'failed',records:[id(1)],experiment_id:id(8)}]})}});
 await wb.refresh();assert.equal(wb.state.runs[0].run_id,id(9));assert.deepEqual(wb.state.runs[0].record_ids,[id(1)]);assert.match(renderExperiments(wb.state),/Analyze/);
});
test('remote import stays unavailable until its separate authorization is granted',async()=>{
 let imports=0;const wb=createAnalysisWorkbench({remote:true,client:{workbenchImport:async()=>{imports++;return{cancelled:true};}}});
 await assert.rejects(wb.action('analysis-append',fields({})),/remote import.*authorization/i);assert.equal(imports,0);
 assert.doesNotMatch(renderAnalysis(wb.state),/data-wb="analysis-load"/);assert.match(renderAnalysis(wb.state),/Record\/Data/);assert.match(renderAnalysis(wb.state),/Remote import.*authorization/i);
});
