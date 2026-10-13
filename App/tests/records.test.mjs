import test from 'node:test';
import assert from 'node:assert/strict';
import {createAnalysisWorkbench,renderAnalysis} from '../web/analysis.js';
const id=n=>String(n).repeat(32),hostId=id('a'),domain={kind:'device',id:id('b')};
const calibration={x_increment_s:1e-6,x_origin_s:0,x_reference:0,y_increment_v:.01,y_origin:0,y_reference:128};
const record={schema:1,id:id(1),name:'Saved trace',comment:'Operator note',created_at_unix_ms:1000,source:{kind:'manual'},frames:[{id:id(2),captured_at_unix_ms:1000,channels:[{channel:'CH1',points:100,calibration},{channel:'CH2',points:100,calibration:{...calibration,x_increment_s:2e-6}}]}],analyses:[]};
const fields=values=>key=>String(values[key]??'');
const waveClient=(data=record)=>({dataList:async()=>({records:[data]}),dataGet:async()=>({record:data}),dataViewport:async p=>({...p,points:[{x:p.start,y:1},{x:p.end-1,y:2}],calibration:data.frames[0].channels.find(c=>c.channel===p.channel).calibration,total_points:100})});
async function recordsModule(){let mod;try{mod=await import('../web/records.js');}catch{}assert.equal(typeof mod?.createRecordsWorkbench,'function','Records workbench must expose the saved-record adapter');return mod;}
async function archive(){
 const context={transfer_format:'ASCII',sample_count:2,spacing:0,level_unit:0,x_unit:0,trace_attribute:0,active_trace:'TRA',center_m:1.55e-6,span_m:2e-9,resolution_m:2e-11,sweep_mode:1};
 return {id:id('c'),name:'spectrum',host_id:hostId,domain,byte_count:32,sample_count:2,sha256:'d'.repeat(64),metadata:{native_unit:'dBm',trace:'A',identity:'YOKOGAWA,AQ6370E,SERIAL,FW',read_started_at:'2026-10-06T10:00:00+00:00',read_finished_at:'2026-10-06T10:00:01+00:00',elapsed_s:1,consistency:'unproven',context_before:context,context_after:context}};
}

test('manual waveform required analysis calibration stays blank and Analysis has only analysis controls',async()=>{
 const wb=createAnalysisWorkbench({client:waveClient()});await wb.load(record.id);
 const html=renderAnalysis(wb.state);
 for(const field of ['excursion','ramp','wavelength','speed'])assert.match(html,new RegExp(`id="analysis-${field}" value=""`));
 assert.doesNotMatch(html,/data-wb="analysis-(load|append|export|save|clear|delete)/);
 assert.match(html,/Record\/Data/);
});
test('analysis hydrates experiment values and saved-result values without mutating source metadata',async()=>{
 const data={...record,source:{kind:'experiment',template_id:'q_scan',parameters:{excursion_pm:24,ramp_hz:70,wavelength_nm:1560,scan_speed_nm_s:2}},analyses:[{id:id(9),frame_id:id(2),channel:'CH1',method:'fsr',parameters:{scan_speed_nm_s:3,prominence_fraction:.2},created_at_unix_ms:2000}]};
 const wb=createAnalysisWorkbench({client:waveClient(data)});await wb.load(data.id);
 assert.equal(wb.state.parameters['analysis-excursion'],'24');assert.equal(wb.state.parameters['analysis-speed'],'3');assert.equal(wb.state.parameters['analysis-method'],'fsr');
 wb.setParameter('analysis-excursion','37');await wb.refresh();await wb.load(data.id);
 assert.equal(wb.state.parameters['analysis-excursion'],'37');assert.equal(data.source.parameters.excursion_pm,24);
 assert.match(renderAnalysis(wb.state),/supplied.*not measured/i);
});
test('manual blank edits and per-record analysis drafts survive record switching',async()=>{
 const other={...record,id:id(3),source:{kind:'experiment',parameters:{excursion_pm:99}}};
 const client=waveClient();client.dataGet=async p=>({record:p.record_id===record.id?record:other});
 const wb=createAnalysisWorkbench({client});await wb.load(record.id);wb.setParameter('analysis-excursion','');wb.setParameter('analysis-wavelength','1549.7');
 await wb.load(other.id);assert.equal(wb.state.parameters['analysis-excursion'],'99');await wb.load(record.id);
 assert.equal(wb.state.parameters['analysis-wavelength'],'1549.7');assert.equal(wb.state.parameters['analysis-excursion'],'');
});
test('analysis physical zoom maps each channel independently and channel visibility preserves window and history',async()=>{
 const calls=[],client=waveClient(),base=client.dataViewport;client.dataViewport=async p=>{calls.push(p);return base(p);};
 const wb=createAnalysisWorkbench({client});await wb.load(record.id);await wb.toggleDisplay('CH2',true);await wb.zoomTime({start:20e-6,end:40e-6});
 assert.deepEqual(calls.slice(-2).map(p=>[p.channel,p.start,p.end]),[['CH1',20,41],['CH2',10,21]]);
 const window={...wb.state.timeDomain};await wb.selectChannel('CH2');assert.deepEqual(wb.state.timeDomain,window);await wb.toggleDisplay('CH1',false);
 assert.deepEqual(wb.state.timeDomain,window);await wb.zoomBack();assert.ok(wb.state.timeDomain.end>190e-6);
});
test('Records combines saved waveforms and sealed OSA entries without listing a volatile preview',async()=>{
 const {createRecordsWorkbench,renderRecords}=await recordsModule(),reference=await archive(),calls=[];
 const wb=createRecordsWorkbench({hostId,hostName:'Lab Host',devices:()=>[{device_id:domain.id,model_id:'aq6370',name:'OSA'}],client:{...waveClient(),listArchives:async p=>{calls.push(p);return {entries:[{id:reference.id,name:reference.name,state:'complete',reference,error:null}],next_offset:1,has_more:false};}}});
 await wb.refresh();assert.deepEqual(wb.state.entries.map(e=>e.kind),['waveform','spectrum']);assert.equal(wb.state.entries.length,2);
 assert.deepEqual(calls,[{domain,offset:0,limit:4}]);assert.match(renderRecords(wb.state),/Saved trace/);assert.match(renderRecords(wb.state),/spectrum/);
 assert.doesNotMatch(renderRecords(wb.state),/Unsaved preview/);
});
test('Records export stays bound to the selected kind and identity while selection changes',async()=>{
 const {createRecordsWorkbench}=await recordsModule(),reference=await archive();let release;const pending=new Promise(r=>release=r),calls=[];
 const wb=createRecordsWorkbench({hostId,devices:[{device_id:domain.id,model_id:'aq6370'}],client:{...waveClient(),listArchives:async()=>({entries:[{id:reference.id,name:reference.name,state:'complete',reference,error:null}],next_offset:1,has_more:false}),workbenchExport:async p=>{calls.push(p);await pending;return {files:[{name:'trace.csv',path:'D:/export/trace.csv'}]};},exportArchive:async p=>{calls.push(p);return {path:'D:/export/spectrum'};}}});
 await wb.refresh();await wb.load('waveform:'+record.id);const work=wb.action('records-export',fields({'records-export-format':'csv'}));
 wb.state.selected=wb.state.entries.find(e=>e.kind==='spectrum');release();await work;
 assert.deepEqual(calls[0],{record_ids:[record.id],format:'csv'});assert.equal(wb.state.selected.kind,'spectrum');
 await wb.action('records-export',fields({}));assert.deepEqual(calls[1],reference);
});
test('Records only offers compatible analysis and validates another Host archive before publish',async()=>{
 const {createRecordsWorkbench,renderRecords}=await recordsModule(),reference=await archive(),analyzed=[];
 const wb=createRecordsWorkbench({hostId,devices:[{device_id:domain.id,model_id:'aq6370'}],onAnalyze:id=>analyzed.push(id),client:{...waveClient(),listArchives:async()=>({entries:[{id:reference.id,name:reference.name,state:'complete',reference,error:null}],next_offset:1,has_more:false})}});
 await wb.refresh();await wb.load('waveform:'+record.id);await wb.action('records-analyze',fields({}));assert.deepEqual(analyzed,[record.id]);
 wb.state.selected=wb.state.entries.find(e=>e.kind==='spectrum');await assert.rejects(wb.action('records-analyze',fields({})),/compatible/i);assert.doesNotMatch(renderRecords(wb.state),/data-wb="records-analyze"/);
 reference.host_id=id('e');await wb.refresh();assert.equal(wb.state.entries.filter(e=>e.kind==='spectrum').length,0);assert.match(wb.state.error,/another Host|instrument/);
});
test('Records keeps previous rows visible during delayed refresh and discards stale waveform loads',async()=>{
 const {createRecordsWorkbench}=await recordsModule();let release,delay=false;const pending=new Promise(r=>release=r),client=waveClient(),list=client.dataList;
 client.dataList=async()=>{if(delay)await pending;return list();};const wb=createRecordsWorkbench({hostId,client});await wb.refresh();delay=true;const refresh=wb.refresh();assert.equal(wb.state.listBusy,true);assert.equal(wb.state.entries.length,1);release();await refresh;
});
test('explicit saved analysis fills unknown fields while retaining operator calibration edits',async()=>{
 const saved={id:id(8),record_id:record.id,frame_id:id(2),channel:'CH1',roi:{start:10,end:80},method:'fsr',parameters:{scan_speed_nm_s:4,wavelength_nm:1561},curve:[],markers:[]};
 const wb=createAnalysisWorkbench({client:{...waveClient(),analysisResult:async()=>({analysis:saved})}});await wb.load(record.id);wb.setParameter('analysis-wavelength','1548');await wb.loadAnalysis(saved.id);
 assert.equal(wb.state.parameters['analysis-speed'],'4');assert.equal(wb.state.parameters['analysis-wavelength'],'1548');assert.equal(wb.state.parameters['analysis-method'],'fsr');
});
test('FSR requires only its own calibration while unknown Q calibration stays blank',async()=>{
 let request;const wb=createAnalysisWorkbench({client:{...waveClient(),analysisStart:async p=>{request=p.request;return {id:id(6),status:'running'};}}});await wb.load(record.id);
 await wb.startAnalysis(fields({'analysis-method':'fsr','analysis-speed':2,'analysis-prominence':.1,'analysis-distance':10}));
 assert.equal(request.scan_speed_nm_s,2);assert.equal(request.excursion_pm,0);assert.equal(wb.state.parameters['analysis-excursion'],'');
});
test('Records offers append import on owning Host and keeps remote import authorization closed',async()=>{
 const {createRecordsWorkbench,renderRecords}=await recordsModule(),calls=[];
 const wb=createRecordsWorkbench({hostId,client:{...waveClient(),workbenchImport:async p=>{calls.push(p);return {cancelled:true};}}});
 await wb.action('records-import',fields({}));assert.deepEqual(calls,[{append:true}]);assert.equal(wb.state.job.status,'cancelled');assert.match(renderRecords(wb.state),/Import Records/);
 const remote=createRecordsWorkbench({hostId,remote:true,client:{workbenchImport:async()=>{throw new Error('must never run');}}});await assert.rejects(remote.action('records-import',fields({})),/authorization/i);assert.match(renderRecords(remote.state),/data-wb="records-import" disabled/);
});
test('partial OSA archives stay visible without view export or invented manual origin',async()=>{
 const {createRecordsWorkbench,renderRecords}=await recordsModule();const wb=createRecordsWorkbench({hostId,devices:[{device_id:domain.id,model_id:'aq6370',name:'OSA'}],client:{...waveClient(),listArchives:async()=>({entries:[{id:id('f'),name:'partial',state:'partial',reference:null,error:'Capture incomplete'}],next_offset:1,has_more:false})}});await wb.refresh();
 assert.equal(wb.state.entries.length,2);const html=renderRecords(wb.state);assert.match(html,/Incomplete archive/);assert.match(html,/Origin unavailable/);await assert.rejects(wb.load(wb.state.entries[1].key),/complete saved/);
});
test('failed new Record load labels retained data with its previous source and disables stale frame export',async()=>{
 const {createRecordsWorkbench,renderRecords}=await recordsModule(),other={...record,id:id(4),name:'New selected record'};let reject;
 const pending=new Promise((_resolve,fail)=>reject=fail),client=waveClient();client.dataList=async()=>({records:[record,other]});client.dataGet=async p=>p.record_id===record.id?{record}:pending;
 const wb=createRecordsWorkbench({hostId,client});await wb.refresh();await wb.load('waveform:'+record.id);const loading=wb.load('waveform:'+other.id);
 assert.match(renderRecords(wb.state),/Previous waveform.*Saved trace/);assert.match(renderRecords(wb.state),/data-wb="records-export-frame" disabled/);
 reject(new Error('Record unavailable'));await loading;const failed=renderRecords(wb.state);assert.match(failed,/Previous waveform.*Saved trace/);assert.match(failed,/data-wb="records-export-frame" disabled/);
 await assert.rejects(wb.action('records-export-frame',fields({})),/selected Record.*frame|frame.*selected Record/i);
});
test('spectrum renders reuse immutable bounded display markup on unrelated refresh',async()=>{
 const {renderRecords}=await recordsModule();let reads=0;const samples=Array.from({length:2000},(_,i)=>-40+Math.sin(i)),native=new Proxy(samples,{get(target,key){if(/^\d+$/.test(String(key)))reads++;return Reflect.get(target,key);}}),trace={verified:true,reference:{id:id('c')},wavelength_nm:Float64Array.from({length:2000},(_,i)=>1500+i/1000),native_values:native,native_unit:'dBm'};
 const state={entries:[],selected:{key:'spectrum',id:id('c'),kind:'spectrum',name:'Spectrum',complete:true},trace};renderRecords(state);const first=reads;assert.ok(first>0);renderRecords(state);assert.equal(reads,first,'immutable spectrum must not be reduced again for unrelated UI refresh');
});
test('unused native zero sentinels never become known calibration during saved-result hydration',async()=>{
 const data={...record,analyses:[{id:id(9),frame_id:id(2),channel:'CH1',method:'fsr',parameters:{excursion_pm:0,ramp_hz:0,wavelength_nm:0,scan_speed_nm_s:2},created_at_unix_ms:1000}]};const wb=createAnalysisWorkbench({client:waveClient(data)});await wb.load(data.id);
 assert.equal(wb.state.parameters['analysis-speed'],'2');for(const field of ['excursion','ramp','wavelength'])assert.equal(wb.state.parameters['analysis-'+field],'');
});
test('switching to a disjoint calibrated channel keeps ROI unavailable and cannot submit analysis',async()=>{
 const data={...record,frames:[{...record.frames[0],channels:[{channel:'CH1',points:100,calibration:{...calibration,x_increment_s:.001}},{channel:'CH2',points:100,calibration:{...calibration,x_increment_s:.001,x_origin_s:10}}]}]};
 let submissions=0;const wb=createAnalysisWorkbench({client:{...waveClient(data),analysisStart:async()=>{submissions++;return {id:id(6),status:'running'};}}});await wb.load(data.id);await wb.zoomTime({start:.02,end:.04});await wb.selectChannel('CH2');
 assert.equal(wb.state.roi,null);assert.deepEqual(wb.state.views,[]);assert.deepEqual(wb.state.timeDomain,{start:.02,end:.04});assert.match(renderAnalysis(wb.state),/data-wb="analysis-fit" disabled/);
 await assert.rejects(wb.startAnalysis(fields({'analysis-method':'fsr','analysis-speed':2,'analysis-prominence':.1,'analysis-distance':10})),/ROI first/);assert.equal(submissions,0);
 await wb.selectChannel('CH1');assert.deepEqual(wb.state.roi,{start:20,end:41});await wb.selectChannel('CH2');await wb.resetRoi();assert.deepEqual(wb.state.roi,{start:0,end:100});assert.doesNotMatch(renderAnalysis(wb.state),/data-wb="analysis-fit" disabled/);
});
