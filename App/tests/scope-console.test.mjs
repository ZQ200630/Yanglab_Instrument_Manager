import test from 'node:test';
import assert from 'node:assert/strict';
import {setImmediate as tick} from 'node:timers/promises';
import {mountConsole} from '../web/console-ui.js';
import {createConsoleSession} from '../web/main.js';

const id=n=>String(n).repeat(32),hostId=id(1),deviceId=id(2),boot=id(3),sessionId=id(4),domain={kind:'device',id:deviceId};
const calibration={x_increment_s:1e-6,x_origin_s:0,x_reference:0,y_increment_v:.01,y_origin:0,y_reference:128};
const record={id:id(7),name:'Saved screen',source:{parameters:{acquisition:'existing_screen',source:'screen'}},frames:[{id:id(8),captured_at_unix_ms:1000,channels:[{channel:'CH2',points:100,calibration},{channel:'CH3',points:100,calibration}]}]};

// Only the DOM/native boundary is controlled. All production handlers, rendering,
// routing, drafts and run/record state changes execute unchanged.
async function mountedScope({startGate,statusGate,stopGate,actionGate,readGate,depthError,mode='READY',displayed=[true,false,true,null],initialDisconnected=false,memoryAuto=false,fakeClock=false,modelId='ds7054',generatorDevice}={}){
 const prior=new Map(['document','window','location','confirm','setInterval','clearInterval','performance'].map(k=>[k,{has:Object.hasOwn(globalThis,k),value:globalThis[k]}]));let now=0;
 const nodes=new Map(),listeners=new Map(),windowEvents=new Map(),timers=[],calls=[],confirmations=[];let controls=[],subscriber,ui,seq=0,sequence=0,confirmValue=true;
 const context={session_id:sessionId,domain,connection_id:id(5),epoch:1};
 const state={host_id:hostId,host_name:'Bench',mode:'real',registry:{registry_rev:1,settings:{},devices:[{device_id:deviceId,model_id:'ds7054',name:'Detector',profile_id:'usb-visa',config_rev:1}],drafts:[],setups:[]},control:{['device:'+deviceId]:{state:'CONTROLLED',controller_session:sessionId,control_epoch:0}},domains:{['device:'+deviceId]:{state:mode,context,host_sample_ms:0,safety:mode==='STOP_HELD'?{state:'STOP_HELD'}:null,device:{connected:true,quality:'confirmed',previous:false,acquisition:'running',memory_depth:1000,channels:displayed.map((value,i)=>({channel:i+1,displayed:value})),capabilities:{channel_count:4,memory_depths:[1000,10000,100000]}}}},operations:[]};
 let run={run_id:id(6),status:'running',stage:'acquire',record_ids:[],elapsed_ms:0},plotError=false,statusError=false,readError=false,nextRecordLoad=null,nextStatus=null;
 const records=new Map([[record.id,record]]);
 const fullDevice=structuredClone(state.domains['device:'+deviceId].device);
 if(generatorDevice){state.registry.devices[0].model_id=modelId;Object.assign(fullDevice,structuredClone(generatorDevice));state.domains['device:'+deviceId].device=structuredClone(fullDevice);}
 if(memoryAuto){fullDevice.memory_depth=0;fullDevice.memory_depth_auto=true;Object.assign(state.domains['device:'+deviceId].device,{memory_depth:0,memory_depth_auto:true});}
 if(initialDisconnected){state.domains['device:'+deviceId].state='DISCONNECTED';state.domains['device:'+deviceId].context={...context,connection_id:null};state.domains['device:'+deviceId].device=null;}
 function dispatch(event,element,options={}){const value={target:element,defaultPrevented:false,preventDefault(){this.defaultPrevented=true;},...options};for(const fn of listeners.get('#content'+event)||[])fn(value);return value;}
 function parse(html){return [...html.matchAll(/<(input|select|button|details)\b([^>]*)>/g)].map(match=>{
  const [,tag,raw]=match,attrs=Object.fromEntries([...raw.matchAll(/([\w-]+)="([^"]*)"/g)].map(m=>[m[1],m[2]]));
  const element={tagName:tag.toUpperCase(),id:attrs.id||'',dataset:Object.fromEntries(Object.entries(attrs).filter(([k])=>k.startsWith('data-')).map(([k,v])=>[k.slice(5).replace(/-([a-z])/g,(_,c)=>c.toUpperCase()),v])),type:attrs.type||'',disabled:/\bdisabled\b/.test(raw),checked:/\bchecked\b/.test(raw),value:attrs.value||'',selectionStart:0,selectionEnd:0,open:false,
   matches(selector){return selector.split(',').some(s=>s.trim()===tag||s.includes('details')&&tag==='details'||s.includes('button')&&tag==='button');},closest:selector=>selector==='#content'?node('#content'):selector==='button'&&tag==='button'?element:null,focus(){document.activeElement=this;},setSelectionRange(a,b){this.selectionStart=a;this.selectionEnd=b;},getAttribute:name=>attrs[name]??null,click(){if(!this.disabled)dispatch('click',this);}};
  if(tag==='select'){const options=html.slice(match.index+match[0].length).split('</select>')[0];element.value=(options.match(/<option value="([^"]*)" selected/)||options.match(/<option value="([^"]*)"/))?.[1]||'';}
  return element;
 });}
 function node(selector){if(!nodes.has(selector)){let html='';nodes.set(selector,{scrollTop:0,scrollLeft:0,hidden:true,textContent:'',get innerHTML(){return html;},set innerHTML(value){html=value;if(selector==='#content'){if(controls.includes(document.activeElement))document.activeElement=null;controls=parse(value);}},querySelectorAll:selector=>controls.filter(e=>e.matches(selector)),querySelector:selector=>controls.find(e=>selector.startsWith('#')?e.id===selector.slice(1):[...selector.matchAll(/\[([\w-]+)="([^"]*)"\]/g)].every(([,name,value])=>e.getAttribute(name)===value)),addEventListener:(event,fn)=>{const key=selector+event;listeners.set(key,[...(listeners.get(key)||[]),fn]);}});}return nodes.get(selector);}
 const publish=()=>{state.domains['device:'+deviceId].host_sample_ms=performance.now();subscriber?.({type:'snapshot',host_id:hostId,boot_id:boot,seq:++seq,data:state});};
 const client={preferences:async()=>({}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},catalog:async()=>({models:[{id:modelId,profiles:[{id:'usb-visa',open_effects:[]}]}],categories:[]}),subscribe:async fn=>{subscriber=fn;return()=>{};},requestSnapshot:async()=>{publish();return{seq};},ping:async()=>({boot_id:boot,client_session_id:sessionId,monotonic_ms:performance.now()}),remoteStatus:async()=>({peers:[],pending:[]}),nextSequence:()=>++sequence,
  captureStart:async params=>{calls.push(['captureStart',structuredClone(params)]);const count=calls.filter(c=>c[0]==='captureStart').length,runId=count===1?id(6):id((count+8).toString(16));run={id:runId,job_id:runId,run_id:runId,kind:'capture',status:'running',stage:'acquire',completed_units:0,total_units:1,started_at_unix_ms:1000,finished_at_unix_ms:null,result:null,error:null,record_ids:[],execution_id:null,cleanup_errors:null,elapsed_ms:0};await startGate;return structuredClone(run);},experimentStatus:async params=>{calls.push(['experimentStatus',params]);const wait=nextStatus;nextStatus=null;await wait;await statusGate;if(statusError)throw new Error('Status request timed out');return structuredClone(run);},experimentStop:async params=>{calls.push(['experimentStop',params]);await stopGate;return{requested:true};},
  dataGet:async params=>{calls.push(['dataGet',params]);const wait=nextRecordLoad;nextRecordLoad=null;await wait;return{record:records.get(params.record_id),saved:false};},dataViewport:async params=>{calls.push(['dataViewport',params]);if(plotError)throw new Error('Saved plot is offline');return{...params,total_points:100,calibration,points:[{x:0,y:1},{x:99,y:2}]};},dataList:async()=>({records:[{id:record.id,name:record.name,frame_count:1}]}),
  previewSave:async params=>{calls.push(['previewSave',params]);const r=records.get(params.record_id);r.name=params.name;return{record_id:r.id,saved:true};},previewDiscard:async params=>{calls.push(['previewDiscard',params]);return{discarded:true};},workbenchExport:async params=>{calls.push(['workbenchExport',params]);return{cancelled:true};},
  prepare:async intent=>{calls.push(['prepare',structuredClone(intent)]);return{token:'proof'};},execute:async(requestId,intent)=>{calls.push(['execute',structuredClone(intent)]);const target=state.domains['device:'+deviceId];if(intent.method==='resume')target.state='READY';if(intent.method==='connect'){target.state='READY';target.context=context;target.device={connected:true,identity:`RIGOL,${modelId.toUpperCase()},BENCH,1.0`,quality:'historical',previous:true};}if(intent.params?.name==='set_channel_display'){await actionGate;fullDevice.channels.find(c=>c.channel===intent.params.args.channel).displayed=intent.params.args.enabled;target.device={...target.device,quality:'historical',previous:true};}if(intent.params?.name==='set_memory_depth'){await actionGate;if(depthError){target.state='FAULT';target.device={...target.device,connected:false,quality:'historical',previous:true};const result={request_id:requestId,session_id:sessionId,operation_id:requestId,worker_id:id('d'),domain,digest:id('e'),sequence:intent.sequence,status:'Terminal',phase:'failed_after_call_started',result:{context:target.context,error:{type:'DriverError',message:depthError}},terminal_at:1100,command:{method:intent.method,params:structuredClone(intent.params)}};state.operations.push(result);publish();return result;}fullDevice.memory_depth=intent.params.args.points;fullDevice.memory_depth_auto=false;target.device={...target.device,quality:'historical',previous:true};}
   if(generatorDevice&&['configure_signal','set_output'].includes(intent.params?.name)){const off=intent.params.name==='set_output'&&intent.params.args.enabled===false;if(off){target.context={...target.context,epoch:target.context.epoch+1};target.state='STOPPING';publish();}await actionGate;const c=fullDevice.channels.find(c=>c.channel===intent.params.args.channel);if(intent.params.name==='configure_signal'){const {channel,...signal}=intent.params.args;Object.assign(c,signal);}else c.output_enabled=intent.params.args.enabled;target.device={...target.device,quality:'historical',previous:true};if(off)target.state='STOP_HELD';}
   if(generatorDevice&&intent.params?.name==='set_phase'){await actionGate;fullDevice.channels.find(c=>c.channel===intent.params.args.channel).phase_deg=intent.params.args.degrees;target.device={...target.device,quality:'historical',previous:true};}
   if(intent.params?.name==='read_status'){await readGate;if(readError)throw new Error('Scope status is offline');target.device=structuredClone(fullDevice);}const result={request_id:requestId,session_id:sessionId,operation_id:requestId,domain,status:'Terminal',phase:'completed',result:{context:target.context,result:{}}};state.operations.push(result);publish();return result;}};
 const restore=()=>{for(const[k,v]of prior)if(v.has)globalThis[k]=v.value;else delete globalThis[k];};
 try{
  if(fakeClock)globalThis.performance={now:()=>now};
  globalThis.document={activeElement:null,querySelector:node,getElementById:value=>controls.find(e=>e.id===value)||node('#'+value)};
  globalThis.window={scrollX:0,scrollY:0,scrollTo(x,y){this.scrollX=x;this.scrollY=y;},addEventListener:(event,fn)=>windowEvents.set(event,fn)};
  globalThis.location={hash:`#/host/${hostId}/device/${deviceId}`};globalThis.confirm=message=>{confirmations.push(message);return confirmValue;};globalThis.setInterval=(fn,ms)=>{timers.push({fn,ms});return timers.length;};globalThis.clearInterval=()=>{};
  const session=createConsoleSession(client,()=>ui?.render());ui=mountConsole(session,{event:{listen:async()=>()=>{}},core:{invoke:async name=>name==='app_profile'?{network_only:false}:[]}});await ui.ready;
  session.store.setLease(`${hostId}/device/${deviceId}`,{token:id(9),boot_id:boot,session_id:sessionId,domain,control_epoch:0,expires_in_ms:fakeClock?60000:10000});publish();ui.render();
  const until=async predicate=>{const deadline=Date.now()+2000;while(!predicate()){assert.ok(Date.now()<deadline,'Mounted Scope did not settle');await tick();}};
  return {calls,confirmations,state,ui,until,publish,restore,html:()=>node('#content').innerHTML,input:value=>document.getElementById(value),content:()=>node('#content'),
   edit(value,text){const element=document.getElementById(value);element.value=text;dispatch('input',element);return element;},
   change(value,text){const element=document.getElementById(value);if(element.type==='checkbox')element.checked=text;else element.value=text;dispatch('input',element);dispatch('change',element);},
   key(value,options={}){return dispatch('keydown',document.getElementById(value),{key:'Enter',...options});},
   click(wb,allowDisabled=false){const element=controls.find(e=>e.dataset.wb===wb);assert.ok(element,'Missing action '+wb);if(!allowDisabled)assert.equal(element.disabled,false,'Disabled action '+wb);dispatch('click',{closest:s=>s==='button'?element:null});},
   op(op,channel){const element=controls.find(e=>e.dataset.op===op&&(channel===undefined||e.dataset.channel===String(channel)));assert.ok(element);assert.equal(element.disabled,false);dispatch('click',{closest:s=>s==='button'?element:null});},
   navigate(hash){location.hash=hash;windowEvents.get('hashchange')?.();},poll(){timers.find(t=>t.ms===200||t.ms===750&&timers.every(t=>t.ms!==200)).fn();},advance(ms){for(const timer of timers)if(timer.ms<=ms)timer.fn();},advanceClock(ms){now+=ms;},
   finish(recordId=record.id){if(!records.has(recordId))records.set(recordId,{...record,id:recordId,name:'Latest saved screen'});run={...run,status:'completed',stage:'completed',record_ids:[recordId]};},
   failedTransfer(){const definition=structuredClone(calls.filter(c=>c[0]==='captureStart').at(-1)[1].definition),error={code:'WaveformTransfer',message:'Waveform data exceeds the control message limit'},executionId=id('d');
    run={...run,status:'failed',stage:'failed',finished_at_unix_ms:1100,elapsed_ms:100,record_ids:[],execution_id:executionId,error,cleanup_errors:[],result:{run:{schema:1,id:executionId,experiment_id:definition.id,definition,bindings:definition.roles.map(binding=>({binding,info:{family:'scope',identity:{manufacturer:'RIGOL',model:'DS7054',serial:'BENCH',firmware:'1.0'},config_rev:1,connected:true,capabilities:['capture_screen','capture_memory']}})),status:'failed',stage:'failed',completed_units:0,total_units:1,started_at_unix_ms:1000,finished_at_unix_ms:1100,records:[],analyses:[],steps:[{stage:'acquire',role:'detector',started_at_unix_ms:1000,completed_at_unix_ms:1100,outcome:'failed',detail:error}],error,cleanup_errors:[]}}};},
   failedDepthPreparation(){this.failedTransfer();const error={code:'OutcomeUnknown',message:'failed_after_call_started: '+JSON.stringify({error:{type:'DriverError',message:'Timeout { operation: "memory depth application; state uncertain, no retry", transferred: 0 }'}})};run={...run,status:'uncertain',stage:'acquire',error,result:{run:{...run.result.run,status:'uncertain',stage:'acquire',error,steps:[{stage:'acquire',role:'detector',started_at_unix_ms:1000,completed_at_unix_ms:1100,outcome:'uncertain',detail:error}]}}};},
   historical(){Object.assign(state.domains['device:'+deviceId].device,{quality:'historical',previous:true});publish();},
   holdRecordLoad(){let resolve;nextRecordLoad=new Promise(yes=>resolve=yes);return{resolve};},holdStatus(){let resolve,reject;nextStatus=new Promise((yes,no)=>{resolve=yes;reject=no;});return{resolve,reject};},
   cancelled(){run={...run,status:'cancelled',stage:'cancelled'};},failPlot(value=true){plotError=value;},failStatus(value=true){statusError=value;},failRead(value=true){readError=value;},confirm(value){confirmValue=value;}};
 }catch(error){restore();throw error;}
}

const generatorDevice={connected:true,quality:'confirmed',previous:false,capabilities:{channels:2,waveforms:['sine','square','triangle','ramp','pulse','noise','dc'],phase:true,load:true,polarity:true},channels:[
 {channel:1,waveform:'sine',frequency_hz:1250,amplitude_vpp:3.2,offset_v:.4,output_enabled:false,phase_deg:25,load:{kind:'high_z'},polarity:'normal'},
 {channel:2,waveform:'triangle',frequency_hz:75,amplitude_vpp:1.5,offset_v:-.2,output_enabled:true,phase_deg:90,load:{kind:'ohms',ohms:75},polarity:'inverted'}]};
test('DG4202 connect automatically synchronizes both channel controls without output writes',async()=>{
 let release;const readGate=new Promise(resolve=>release=resolve),view=await mountedScope({modelId:'dg4202',generatorDevice,initialDisconnected:true,readGate});
 try{assert.equal(view.input('generator-1-frequency').value,'');view.op('connect');await view.until(()=>view.calls.some(c=>c[1]?.params?.name==='read_status'));
  assert.match(view.html(),/activity-spinner/);view.navigate('#overview');assert.match(view.html(),/Instrument overview/);view.navigate(`#/host/${hostId}/device/${deviceId}`);
  release();await view.until(()=>view.input('generator-1-frequency').value==='1250');assert.equal(view.input('generator-2-frequency').value,'75');assert.equal(view.input('generator-2-load').value,'75');
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params?.name||c[1].method),['connect','read_status']);
 }finally{release();view.restore();}
});
test('DG4202 apply automatically reads back and keeps edits made during the pending command',async()=>{
 let release;const actionGate=new Promise(resolve=>release=resolve),view=await mountedScope({modelId:'dg4202',generatorDevice,actionGate});
 try{view.edit('generator-2-frequency','150');view.op('generator-apply',2);await view.until(()=>view.calls.some(c=>c[1]?.params?.name==='configure_signal'));
  view.edit('generator-2-frequency','175');view.edit('generator-1-offset','1.1');view.publish();assert.equal(view.input('generator-2-frequency').value,'175');release();
  await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);await tick();assert.equal(view.input('generator-2-frequency').value,'175');assert.equal(view.input('generator-1-offset').value,'1.1');assert.doesNotMatch(view.html(),/generator-readback|Operation finished/);
  view.op('generator-read');await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===3);await tick();assert.equal(view.input('generator-2-frequency').value,'150');assert.equal(view.input('generator-1-offset').value,'0.4');
 }finally{release();view.restore();}
});
for(const channel of [1,2])for(const [part,value,field]of [['frequency','150','frequency_hz'],['amplitude','2','amplitude_vpp'],['offset','0.1','offset_v']])test(`DG4202 Enter in CH${channel} ${part} applies only that channel and reads back`,async()=>{
 const view=await mountedScope({modelId:'dg4202',generatorDevice});try{
  view.edit(`generator-${channel}-${part}`,value).focus();assert.equal(view.key(`generator-${channel}-${part}`).defaultPrevented,true);
  await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);await tick();
  const actions=view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params);
  assert.deepEqual(actions.map(a=>a.name),['configure_signal','read_status']);assert.equal(actions[0].args.channel,channel);assert.equal(actions[0].args[field],Number(value));
  assert.equal(document.activeElement?.id,`generator-${channel}-${part}`);
 }finally{view.restore();}
});
test('DG4202 Enter in phase submits only phase and does not apply basic drafts',async()=>{
 const view=await mountedScope({modelId:'dg4202',generatorDevice});try{
  view.edit('generator-1-frequency','300');view.edit('generator-2-phase','120');view.key('generator-2-phase');
  await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);await tick();
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params),[{name:'set_phase',args:{channel:2,degrees:120}},{name:'read_status',args:{}}]);
  assert.equal(view.input('generator-1-frequency').value,'300');assert.equal(view.input('generator-2-phase').value,'120');
 }finally{view.restore();}
});
test('DG4202 confirmed readback canonicalizes the submitted value directly in its input',async()=>{
 const view=await mountedScope({modelId:'dg4202',generatorDevice});try{
  view.edit('generator-1-frequency','1.250e3');view.key('generator-1-frequency');
  await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);await tick();
  assert.equal(view.input('generator-1-frequency').value,'1250');assert.doesNotMatch(view.html(),/generator-readback|Operation finished/);
 }finally{view.restore();}
});
test('DG4202 dropdowns, modified or repeated Enter and composition never submit',async()=>{
 const view=await mountedScope({modelId:'dg4202',generatorDevice});try{
  for(const part of ['waveform','load','polarity'])view.key(`generator-1-${part}`);
  for(const options of [{repeat:true},{isComposing:true},{defaultPrevented:true},{ctrlKey:true},{metaKey:true},{altKey:true},{key:'Tab'}])view.key('generator-1-frequency',options);
  view.input('generator-1-frequency').disabled=true;view.key('generator-1-frequency');await tick();assert.equal(view.calls.length,0);
 }finally{view.restore();}
});
test('DG4202 Enter while Apply is pending never duplicates the command and keeps newer drafts',async()=>{
 let release;const actionGate=new Promise(resolve=>release=resolve),view=await mountedScope({modelId:'dg4202',generatorDevice,actionGate});try{
  view.edit('generator-1-frequency','150');view.key('generator-1-frequency');await view.until(()=>view.calls.some(c=>c[1]?.params?.name==='configure_signal'));
  view.edit('generator-1-frequency','175');view.key('generator-1-frequency');view.key('generator-2-amplitude');await tick();
  assert.equal(view.calls.filter(c=>c[0]==='execute').length,1);release();await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);await tick();assert.equal(view.input('generator-1-frequency').value,'175');
 }finally{release();view.restore();}
});
test('DG4202 output changes remain previous until the automatic read confirms them',async()=>{
 let release;const readGate=new Promise(resolve=>release=resolve),view=await mountedScope({modelId:'dg4202',generatorDevice,readGate});
 try{view.op('generator-output-toggle',1);await view.until(()=>view.calls.some(c=>c[1]?.params?.name==='read_status'));assert.match(view.html(),/Previous/);assert.match(view.html(),/Output off/);
  release();await view.until(()=>!view.html().includes('Previous')&&!view.input('generator-1-frequency').disabled);await tick();assert.match(view.html(),/Output on/);assert.equal(view.calls.filter(c=>c[0]==='execute').length,2);
 }finally{release();view.restore();}
});
test('DG4202 refresh failure never replays an output command',async()=>{
 const view=await mountedScope({modelId:'dg4202',generatorDevice});try{view.failRead();view.op('generator-output-toggle',1);await view.until(()=>view.calls.some(c=>c[1]?.params?.name==='read_status'));await tick();assert.match(view.html(),/Previous/);
  view.advance(1000);await tick();assert.equal(view.calls.filter(c=>c[1]?.params?.name==='set_output'&&c[0]==='execute').length,1);
 }finally{view.restore();}
});
test('DG4202 Output Off confirms the fenced readback and resumes controls without enabling outputs',async()=>{
 const view=await mountedScope({modelId:'dg4202',generatorDevice});try{view.op('generator-output-toggle',2);
  await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===3);await tick();
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params.name||c[1].method),['set_output','read_status','resume']);
  assert.equal(view.state.domains['device:'+deviceId].state,'READY');assert.match(view.html(),/Output off/);
  view.op('generator-read');await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===4);await tick();
  assert.equal(view.calls.filter(c=>c[0]==='execute'&&c[1].params.name==='set_output').length,1);
 }finally{view.restore();}
});
test('DG4202 held controls permit Sync but never toggle a confirmed OFF channel ON',async()=>{
 const view=await mountedScope({modelId:'dg4202',generatorDevice});try{view.failRead();view.op('generator-output-toggle',2);
  await view.until(()=>view.calls.some(c=>c[1]?.params?.name==='read_status'));await tick();await tick();
  assert.equal(view.state.domains['device:'+deviceId].state,'STOP_HELD');assert.match(view.html(),/Resume controls/);
  assert.equal(view.calls.some(c=>c[1]?.method==='resume'),false);
  view.failRead(false);view.op('generator-read');await view.until(()=>view.calls.filter(c=>c[0]==='execute'&&c[1].params.name==='read_status').length===2);await tick();
  assert.equal(view.content().querySelector('[data-op="generator-output-toggle"][data-channel="1"]').disabled,true);
  view.op('resume');await view.until(()=>view.state.domains['device:'+deviceId].state==='READY');await tick();
  assert.equal(view.calls.filter(c=>c[0]==='execute'&&c[1].params.name==='set_output').length,1);
 }finally{view.restore();}
});
test('DG4202 completion after a connection change never queries the replacement session',async()=>{
 let release;const actionGate=new Promise(resolve=>release=resolve),view=await mountedScope({modelId:'dg4202',generatorDevice,actionGate});
 try{view.op('generator-output-toggle',1);await view.until(()=>view.calls.some(c=>c[1]?.params?.name==='set_output'));const target=view.state.domains['device:'+deviceId];target.context={...target.context,connection_id:id('e'),epoch:2};view.publish();release();await tick();await tick();
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params.name),['set_output']);
 }finally{release();view.restore();}
});

test('mounted Acquire submits the stopped memory contract with immediate feedback and retains draft focus',async()=>{
 let release;const gate=new Promise(resolve=>release=resolve),view=await mountedScope({startGate:gate});
 try{
  assert.equal(view.input('scope-capture-1').checked,true);assert.equal(view.input('scope-capture-2').checked,false);assert.equal(view.input('scope-capture-3').checked,true);assert.equal(view.input('scope-capture-4').checked,false);
  assert.equal(view.input('scope-source').value,'memory');assert.equal(view.input('scope-depth').value,'100000');assert.equal(view.input('scope-resume').checked,true);
  assert.match(view.html(),/Controller memory depth: 1K/);
  assert.equal(view.calls.length,0,'mounting must not act on the scope');
  view.change('scope-capture-1',false);
  view.click('scope-collect');assert.match(view.html(),/activity-spinner/);assert.match(view.html(),/Submitting acquisition/);assert.equal(view.confirmations.length,0);
  assert.deepEqual(view.calls[0][1].definition.parameters,{acquisition:'stopped',source:'memory',memory_depth:100000,frames:1,interval_ms:0,resume_after_capture:true,channels:[3]});
  const input=view.edit('scope-interval','2.5');input.focus();input.setSelectionRange(0,1);view.content().scrollTop=83;view.publish();
  assert.equal(view.input('scope-interval').value,'2.5');assert.equal(document.activeElement.id,'scope-interval');assert.equal(view.input('scope-interval').selectionStart,0);assert.equal(view.content().scrollTop,83);
  view.navigate('#overview');assert.match(view.html(),/Instrument overview/);view.navigate(`#/host/${hostId}/device/${deviceId}`);assert.equal(view.input('scope-capture-1').checked,false);assert.equal(view.input('scope-capture-2').checked,false);assert.equal(view.input('scope-interval').value,'2.5');
  release();await view.until(()=>view.html().includes('Reading stopped waveform'));
 }finally{release();view.restore();}
});

test('mounted preview failure permits read-only retry and direct export never replays acquisition',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await view.until(()=>view.calls.some(c=>c[0]==='captureStart'));await tick();view.failPlot();view.finish();view.poll();
  await view.until(()=>view.html().includes('Waveform preview unavailable'));
  assert.doesNotMatch(view.html(),/workbench-job completed|workbench-job uncertain/);
  view.failPlot(false);view.click('scope-reload');await view.until(()=>view.html().includes('data-waveform-plot'));
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);
  assert.equal(view.calls.filter(c=>c[0]==='experimentStatus').length,1);
  assert.equal(view.input('scope-display-2').checked,true);assert.equal(view.input('scope-display-3').checked,true);
  view.click('scope-export');await view.until(()=>view.calls.some(c=>c[0]==='workbenchExport'));assert.equal(location.hash,`#/host/${hostId}/device/${deviceId}`);
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);
 }finally{view.restore();}
});

test('mounted Save record is explicit and does not trigger another acquisition',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.finish();view.poll();await view.until(()=>view.html().includes('data-waveform-plot'));
  assert.equal(view.calls.some(c=>c[0]==='previewSave'),false);assert.match(view.html(),/Unsaved/);
  view.edit('scope-record-name','Ring 01');view.click('scope-save');await view.until(()=>view.calls.some(c=>c[0]==='previewSave'));await tick();
  assert.deepEqual(view.calls.find(c=>c[0]==='previewSave')[1],{record_id:record.id,name:'Ring 01',comment:''});
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);assert.match(view.html(),/>Saved</);
 }finally{view.restore();}
});

test('mounted loop options submit one manual capture without a modal or direct STOP/RUN',async()=>{
 const view=await mountedScope();try{
  view.change('scope-source','screen');assert.equal(view.calls.length,0);assert.equal(view.input('scope-frames').disabled,false);
  view.edit('scope-frames','3');view.edit('scope-interval','0.5');view.change('scope-resume',true);view.confirm(false);view.click('scope-collect');
  await view.until(()=>view.calls.some(c=>c[0]==='captureStart'));
  assert.equal(view.confirmations.length,0);assert.deepEqual(view.calls[0][1].definition.parameters,{acquisition:'stopped',source:'screen',memory_depth:null,frames:3,interval_ms:500,resume_after_capture:true,channels:[1,3]});
  assert.equal(view.calls.some(c=>c[0]==='execute'),false);
 }finally{view.restore();}
});

test('unchecked final resume persists through telemetry and navigation into the submitted acquisition',async()=>{
 const view=await mountedScope();try{
  view.change('scope-resume',false);view.publish();await tick();
  view.navigate('#overview');view.navigate(`#/host/${hostId}/device/${deviceId}`);
  assert.equal(view.input('scope-resume').checked,false);assert.equal(view.calls.length,0);
  view.click('scope-collect');await view.until(()=>view.calls.some(c=>c[0]==='captureStart'));
  assert.equal(view.calls.find(c=>c[0]==='captureStart')[1].definition.parameters.resume_after_capture,false);
 }finally{view.restore();}
});

test('mounted status timeout checks only original status and never starts another read automatically',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.failStatus();view.poll();await view.until(()=>view.html().includes('Status request timed out'));
  assert.match(view.html(),/Check original status/);view.poll();view.poll();await tick();
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);assert.equal(view.calls.filter(c=>c[0]==='experimentStatus').length,1);
  view.click('scope-check');await view.until(()=>view.calls.filter(c=>c[0]==='experimentStatus').length===2);
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);
 }finally{view.restore();}
});

test('mounted Resume controls uses the existing resume method without sending acquisition RUN',async()=>{
 const view=await mountedScope({mode:'STOP_HELD'});try{
  view.op('resume');await view.until(()=>view.calls.some(c=>c[0]==='execute'));
  const intent=view.calls.find(c=>c[0]==='execute')[1];assert.equal(intent.method,'resume');assert.deepEqual(intent.params,{confirm:true});
  assert.equal(view.calls.some(c=>c[0]==='execute'&&c[1].params?.name==='run'),false);
 }finally{view.restore();}
});

test('explicit scope Connect reads its status once to initialize the channel choices',async()=>{
 const view=await mountedScope({initialDisconnected:true});try{
  assert.equal(view.calls.length,0);view.op('connect');
  await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);
  await view.until(()=>view.input('scope-capture-1').checked);
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>[c[1].method,c[1].params]),[['connect',{acknowledge_lifecycle:true}],['action',{name:'read_status',args:{}}]]);
  assert.equal(view.input('scope-capture-2').checked,false);view.publish();await tick();
  assert.equal(view.input('scope-depth').value,'100000');assert.equal(view.input('scope-resume').checked,true);
  assert.match(view.html(),/Controller memory depth: 1K/);
  assert.equal(view.calls.filter(c=>c[0]==='execute').length,2,'telemetry must not repeat status reads');
 }finally{view.restore();}
});

test('channel click applies then reads back and clears confirmed off Save without optimistic enabled state',async()=>{
 let releaseAction,releaseRead;const actionGate=new Promise(resolve=>releaseAction=resolve),readGate=new Promise(resolve=>releaseRead=resolve),view=await mountedScope({actionGate,readGate});try{
  view.confirm(false);view.op('scope-display',1);await view.until(()=>view.calls.some(c=>c[0]==='execute'));
  assert.equal(view.confirmations.length,0);assert.equal(view.input('scope-capture-1').checked,true);
  assert.deepEqual(view.calls.find(c=>c[0]==='execute')[1].params,{name:'set_channel_display',args:{channel:1,enabled:false}});
  releaseAction();await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);
  assert.equal(view.input('scope-capture-1').checked,true);assert.match(view.html(),/activity-spinner/);
  releaseRead();await view.until(()=>!view.input('scope-capture-1').checked);
  assert.equal(view.input('scope-capture-1').disabled,true);
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params.name),['set_channel_display','read_status']);
  view.publish();await tick();assert.equal(view.calls.filter(c=>c[0]==='execute').length,2);
 }finally{releaseAction();releaseRead();view.restore();}
});

test('pending channel action keeps its CH identity and feedback after both cards inside the scope grid',async()=>{
 let releaseAction,releaseRead;const actionGate=new Promise(resolve=>releaseAction=resolve),readGate=new Promise(resolve=>releaseRead=resolve),view=await mountedScope({actionGate,readGate});try{
  view.op('scope-display',1);
  await view.until(()=>view.calls.some(c=>c[0]==='execute'));
  let html=view.html();
  assert.match(html,/id="scope-channel-1"[^>]*aria-busy="true"[^>]*>CH1<\/button>/);
  assert.doesNotMatch(html,/id="scope-channel-1"[^>]*>Working…<\/button>/);
  assert.ok(html.indexOf('class="scope-layout"')<html.indexOf('id="scope-status"'));
  assert.ok(html.indexOf('class="card scope-workbench"')<html.indexOf('id="scope-status"'));
  assert.ok(html.indexOf('operation-feedback')>html.indexOf('id="scope-status"'));
  assert.ok(html.indexOf('operation-feedback')<html.indexOf('<details id="scope-advanced"'));
  assert.match(html,/class="scope-channel-row on" data-channel="1"/);
  releaseAction();await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);
  html=view.html();assert.match(html,/class="scope-channel-row on" data-channel="1"/);
  assert.equal(view.input('scope-capture-1').checked,true);
  releaseRead();await view.until(()=>!view.input('scope-capture-1').checked);
  assert.match(view.html(),/class="scope-channel-row off" data-channel="1"/);
 }finally{releaseAction();releaseRead();await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);await tick();view.restore();}
});

test('depth selection auto applies and Sync replaces local depth with hardware readback only',async()=>{
 const view=await mountedScope();try{
  view.change('scope-depth','10000');await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params),[{name:'set_memory_depth',args:{points:10000}},{name:'read_status',args:{}}]);
  await tick();view.edit('scope-depth','1000');view.op('scope-read');await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===3);await tick();
  assert.equal(view.input('scope-depth').value,'10000');assert.equal(view.confirmations.length,0);
 }finally{view.restore();}
});

test('depth timeout keeps navigation usable and exposes the original setting failure without replay',async()=>{
 let release;const actionGate=new Promise(resolve=>release=resolve),view=await mountedScope({actionGate,depthError:'Timeout { operation: "memory depth application; state uncertain, no retry", transferred: 0 }'});
 try{
  view.change('scope-depth','10000');await view.until(()=>view.calls.some(c=>c[0]==='execute'));
  assert.match(view.html(),/activity-spinner/);assert.equal(view.input('scope-depth').value,'10000');
  view.navigate('#overview');assert.match(view.html(),/Instrument overview/);view.navigate(`#/host/${hostId}/device/${deviceId}`);
  release();await view.until(()=>view.html().includes('Operation failed'));await tick();
  assert.match(view.html(),/Memory depth change failed/);
  assert.match(view.html(),/Requested memory depth: 10K/);assert.match(view.html(),/Last confirmed memory depth: 1K/);
  assert.match(view.html(),/memory depth application; state uncertain, no retry/);
  assert.match(view.html(),/On · Previous/);assert.match(view.html(),/Off · Previous/);
  assert.equal(view.input('scope-depth').value,'10000');assert.equal(view.input('scope-channel-1').disabled,true);
  const disconnect=view.html().match(/<button[^>]*data-op="disconnect"[^>]*>/)?.[0];assert.ok(disconnect);assert.doesNotMatch(disconnect,/\bdisabled\b/);assert.match(view.html(),/data-wb="scope-collect" disabled/);
  assert.doesNotMatch(view.html(),/Acquisition outcome is uncertain|Disconnect and reconnect before/);
  view.publish();view.poll();view.poll();await tick();
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params),[{name:'set_memory_depth',args:{points:10000}}]);
  assert.equal(view.calls.some(c=>c[0]==='captureStart'),false);
 }finally{release();view.ui.clearNotice();view.restore();}
});

test('depth failure keeps the previous waveform visible after a completed manual capture',async()=>{
 const view=await mountedScope({depthError:'memory depth application; state uncertain, no retry'});
 try{
  view.click('scope-collect');await tick();view.finish();view.poll();await view.until(()=>view.html().includes('data-waveform-plot'));
  await view.until(()=>view.calls.some(c=>c[1]?.params?.name==='read_status'));await tick();
  view.change('scope-depth','10000');await view.until(()=>view.html().includes('Operation failed'));await tick();
  assert.match(view.html(),/Previous waveform/);assert.match(view.html(),/data-waveform-plot/);assert.match(view.html(),/Unsaved/);
  assert.match(view.html(),/Memory depth change failed/);assert.doesNotMatch(view.html(),/Acquisition outcome is uncertain/);
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params.name),['read_status','set_memory_depth']);
 }finally{view.ui.clearNotice();view.restore();}
});

test('Acquire preparation failure gates stale READY controls and explicit Sync confirms recovery without replay',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.failedDepthPreparation();view.poll();await view.until(()=>view.html().includes('Memory depth application failed'));
  assert.match(view.html(),/Previous controller acquisition: running/);assert.match(view.html(),/On · Previous/);assert.match(view.html(),/data-wb="scope-collect" disabled/);
  view.click('scope-collect',true);await tick();assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);view.ui.clearNotice();
  view.op('scope-read');await view.until(()=>view.calls.some(c=>c[1]?.params?.name==='read_status'));await tick();
  assert.match(view.html(),/Controller acquisition: running/);assert.match(view.html(),/data-wb="scope-collect">Acquire/);
  assert.doesNotMatch(view.html(),/Current scope settings are unconfirmed|On · Previous/);
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params.name),['read_status']);
 }finally{view.ui.clearNotice();view.restore();}
});

test('Acquire preparation recovery remains blocked after a failed Sync',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.failedDepthPreparation();view.poll();await view.until(()=>view.html().includes('Memory depth application failed'));
  view.failRead();view.op('scope-read');await view.until(()=>view.html().includes('Scope settings read failed'));await tick();
  assert.match(view.html(),/Previous controller acquisition: running/);assert.match(view.html(),/data-wb="scope-collect" disabled/);
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params.name),['read_status']);
 }finally{view.ui.clearNotice();view.restore();}
});

test('a verified new connection clears the old Acquire depth recovery gate after its fresh status read',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.failedDepthPreparation();view.poll();await view.until(()=>view.html().includes('Memory depth application failed'));
  const d=view.state.domains['device:'+deviceId];d.context.connection_id=id('c');d.context.epoch+=1;Object.assign(d,{state:'DISCONNECTED',context:{...d.context,connection_id:null},device:null});view.publish();view.ui.render();view.op('connect');
  await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);await tick();
  assert.match(view.html(),/Controller acquisition: running/);assert.match(view.html(),/data-wb="scope-collect">Acquire/);
  assert.doesNotMatch(view.html(),/Current scope settings are unconfirmed|On · Previous/);
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params.name||c[1].method),['connect','read_status']);
 }finally{view.ui.clearNotice();view.restore();}
});

test('Auto depth connect keeps the 100K draft and explicit Sync adopts physical Auto without a write',async()=>{
 const view=await mountedScope({memoryAuto:true,initialDisconnected:true});try{
  view.op('connect');await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===2);await tick();
  assert.equal(view.input('scope-depth').value,'100000');assert.equal(view.input('scope-resume').checked,true);assert.match(view.html(),/data-wb="scope-collect">Acquire/);
  assert.match(view.html(),/Controller memory depth: Auto/);
  view.edit('scope-depth','1000');view.op('scope-read');await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===3);await tick();
  assert.equal(view.input('scope-depth').value,'auto');assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params.name||c[1].method),['connect','read_status','read_status']);
  view.change('scope-source','screen');assert.match(view.html(),/data-wb="scope-collect">Acquire/);
  view.change('scope-source','memory');assert.match(view.html(),/data-wb="scope-collect" disabled/);assert.equal(view.calls.some(c=>c[0]==='captureStart'),false);
  view.change('scope-depth','10000');await view.until(()=>view.calls.filter(c=>c[0]==='execute').length===5);await tick();
  assert.equal(view.input('scope-depth').value,'10000');assert.match(view.html(),/data-wb="scope-collect">Acquire/);
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').slice(-2).map(c=>c[1].params),[{name:'set_memory_depth',args:{points:10000}},{name:'read_status',args:{}}]);
  view.click('scope-collect');await view.until(()=>view.calls.some(c=>c[0]==='captureStart'));assert.equal(view.calls.find(c=>c[0]==='captureStart')[1].definition.parameters.memory_depth,10000);
 }finally{view.restore();}
});

test('setting completion after navigation does not issue a readback on a stale route',async()=>{
 let release;const actionGate=new Promise(resolve=>release=resolve),view=await mountedScope({actionGate});try{
  view.op('scope-display',1);await view.until(()=>view.calls.some(c=>c[0]==='execute'));
  view.navigate('#overview');release();await tick();await tick();
  assert.equal(view.calls.filter(c=>c[0]==='execute').length,1);assert.match(view.html(),/Instrument overview/);
 }finally{release();view.restore();}
});

test('setting completion after context change does not read the new connection',async()=>{
 let release;const actionGate=new Promise(resolve=>release=resolve),view=await mountedScope({actionGate});try{
  view.op('scope-display',1);await view.until(()=>view.calls.some(c=>c[0]==='execute'));
  view.state.domains['device:'+deviceId].context={...view.state.domains['device:'+deviceId].context,connection_id:id('c')};view.publish();release();await tick();await tick();
  assert.equal(view.calls.filter(c=>c[0]==='execute').length,1);
 }finally{release();view.restore();}
});

test('successful preview refreshes settings once and preserves its waveform if sync fails',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.historical();view.failRead();view.finish();view.poll();
  await view.until(()=>view.html().includes('scope-sync-error'));
  assert.doesNotMatch(view.html(),/workbench-job completed/);assert.match(view.html(),/data-waveform-plot/);assert.match(view.html(),/Unsaved/);
  assert.match(view.html(),/Scope status is offline/);
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params),[{name:'read_status',args:{}}]);
  view.publish();view.poll();await tick();assert.equal(view.calls.filter(c=>c[0]==='execute').length,1);
 }finally{view.restore();}
});

test('a finished off-page acquisition synchronizes once upon navigation back',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.navigate('#overview');view.historical();view.finish();view.poll();
  await view.until(()=>view.calls.some(c=>c[0]==='dataViewport'));await tick();assert.equal(view.calls.some(c=>c[0]==='execute'),false);
  view.navigate(`#/host/${hostId}/device/${deviceId}`);await view.until(()=>view.calls.some(c=>c[0]==='execute'));await tick();
  assert.equal(view.input('scope-channel-1').disabled,false);view.publish();await tick();assert.equal(view.calls.filter(c=>c[0]==='execute').length,1);
 }finally{view.restore();}
});

for(const busyKey of ['active_request_id','pending_request_id','readback_request_id'])test(`final settings sync waits for ${busyKey} to clear without polling the completed experiment`,async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.historical();view.state.domains['device:'+deviceId][busyKey]=id('d');view.publish();view.finish();view.poll();
  await view.until(()=>view.calls.some(c=>c[0]==='dataViewport'));await tick();assert.equal(view.calls.some(c=>c[0]==='execute'),false);
  delete view.state.domains['device:'+deviceId][busyKey];view.publish();view.poll();
  await view.until(()=>view.calls.some(c=>c[0]==='execute'));await tick();assert.equal(view.input('scope-channel-1').disabled,false);
  view.poll();view.publish();await tick();assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params),[{name:'read_status',args:{}}]);
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);assert.equal(view.calls.filter(c=>c[0]==='experimentStatus').length,1);
 }finally{view.restore();}
});

test('deferred settings sync stops after its bounded wait and exposes a Sync warning',async()=>{
 const view=await mountedScope({fakeClock:true});try{
  view.click('scope-collect');await tick();view.historical();view.state.domains['device:'+deviceId].active_request_id=id('d');view.publish();view.finish();view.poll();
  await view.until(()=>view.calls.some(c=>c[0]==='dataViewport'));await tick();view.advanceClock(10001);view.poll();await tick();
  assert.match(view.html(),/scope-sync-error/);assert.match(view.html(),/refreshed in time.*Sync/);
  delete view.state.domains['device:'+deviceId].active_request_id;view.publish();view.poll();await tick();assert.equal(view.calls.some(c=>c[0]==='execute'),false);
 }finally{view.restore();}
});

test('clearing busy after the final sync deadline cannot issue a late automatic read',async()=>{
 const view=await mountedScope({fakeClock:true});try{
  view.click('scope-collect');await tick();view.historical();view.state.domains['device:'+deviceId].active_request_id=id('d');view.publish();view.finish();view.poll();
  await view.until(()=>view.calls.some(c=>c[0]==='dataViewport'));await tick();view.advanceClock(10001);delete view.state.domains['device:'+deviceId].active_request_id;view.publish();view.poll();await tick();
  assert.equal(view.calls.some(c=>c[0]==='execute'),false);assert.match(view.html(),/scope-sync-error/);
 }finally{view.restore();}
});

test('deferred settings query is attempted once even when that read fails',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.historical();view.state.domains['device:'+deviceId].active_request_id=id('d');view.publish();view.finish();view.poll();
  await view.until(()=>view.calls.some(c=>c[0]==='dataViewport'));await tick();view.failRead();delete view.state.domains['device:'+deviceId].active_request_id;view.publish();view.poll();
  await view.until(()=>view.html().includes('scope-sync-error'));view.poll();view.publish();await tick();
  assert.equal(view.calls.filter(c=>c[0]==='execute').length,1);assert.equal(view.calls.filter(c=>c[0]==='experimentStatus').length,1);assert.doesNotMatch(view.html(),/workbench-job completed/);assert.match(view.html(),/Waveform received/);
 }finally{view.restore();}
});

test('delayed scope status remains one in-flight request while navigation is usable',async()=>{
 let release;const gate=new Promise(resolve=>release=resolve),view=await mountedScope({statusGate:gate});try{
  view.click('scope-collect');await tick();view.poll();view.poll();view.poll();
  assert.equal(view.calls.filter(c=>c[0]==='experimentStatus').length,1);
  view.navigate('#overview');assert.match(view.html(),/Instrument overview/);
  release();await tick();assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);
 }finally{release();view.restore();}
});

test('scope cancellation stays requested until the Host confirms a cancellation boundary',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.click('scope-cancel');
  await view.until(()=>view.html().includes('Cancellation requested'));
  assert.match(view.html(),/waiting for the confirmed boundary/);assert.doesNotMatch(view.html(),/workbench-job cancelled/);
  await tick();view.cancelled();view.poll();await view.until(()=>view.html().includes('workbench-job cancelled'));
  assert.equal(view.calls.filter(c=>c[0]==='experimentStop').length,1);assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,1);
 }finally{view.restore();}
});

test('an active scope run discovers its saved result within the next 200 ms status interval',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.finish();view.advance(200);
  await view.until(()=>view.html().includes('data-waveform-plot'));
  assert.equal(view.calls.filter(c=>c[0]==='experimentStatus').length,1);
 }finally{view.restore();}
});

test('cancellation checks the original scope run after navigation during a delayed stop receipt',async()=>{
 let release;const gate=new Promise(resolve=>release=resolve),view=await mountedScope({stopGate:gate});try{
  view.click('scope-collect');await tick();view.click('scope-cancel');
  await view.until(()=>view.calls.some(c=>c[0]==='experimentStop'));
  view.navigate('#overview');view.cancelled();release();await tick();await tick();
  view.navigate(`#/host/${hostId}/device/${deviceId}`);
  await view.until(()=>view.html().includes('workbench-job cancelled'));
  assert.deepEqual(view.calls.filter(c=>c[0]==='experimentStatus').map(c=>c[1]),[{run_id:id(6)}]);
  assert.doesNotMatch(view.html(),/Cancellation is unconfirmed/);
 }finally{release();view.restore();}
});

test('a completed read can be repeated with its preserved channel draft after Worker marks readback historical',async()=>{
 const view=await mountedScope();try{
  view.click('scope-collect');await tick();view.historical();view.finish();view.poll();
  await view.until(()=>view.html().includes('data-waveform-plot'));
  await view.until(()=>view.calls.some(c=>c[0]==='execute'));await tick();view.click('scope-collect');await view.until(()=>view.calls.filter(c=>c[0]==='captureStart').length===2);
  assert.deepEqual(view.calls.filter(c=>c[0]==='captureStart').at(-1)[1].definition.parameters,{acquisition:'stopped',source:'memory',memory_depth:1000,frames:1,interval_ms:0,resume_after_capture:true,channels:[1,3]});
  assert.deepEqual(view.calls.filter(c=>c[0]==='execute').map(c=>c[1].params.name),['read_status'],'each completed acquisition refreshes status once');
 }finally{view.restore();}
});

test('failed waveform transfer preserves the previous preview and a fresh Acquire completes without saving',async()=>{
 const view=await mountedScope(),plot=()=>view.html().match(/<svg\b[^>]*data-waveform-plot="true"[\s\S]*?<\/svg>/)?.[0];try{
  // Establish the previous unsaved preview through the same real UI flow.
  view.click('scope-collect');await tick();view.finish();view.poll();
  await view.until(()=>plot()&&!view.content().querySelector('[data-wb="scope-collect"]').disabled);
  const previousPlot=plot();assert.ok(previousPlot);assert.match(view.html(),/Unsaved/);

  view.click('scope-collect');await view.until(()=>view.calls.filter(c=>c[0]==='captureStart').length===2);await tick();
  assert.match(view.html(),/<h2>Previous waveform<\/h2>/);assert.equal(plot(),previousPlot);
  view.failedTransfer();view.historical();view.poll();await view.until(()=>view.html().includes('workbench-job failed'));
  assert.match(view.html(),/Waveform data exceeds the control message limit/);
  assert.doesNotMatch(view.html(),/workbench-job (running|queued|uncertain)/);
  assert.equal(view.content().querySelector('[data-wb="scope-collect"]').disabled,false);
  assert.equal(plot(),previousPlot);assert.match(view.html(),/<h2>Previous waveform<\/h2>/);
  assert.deepEqual(view.calls.filter(c=>c[0]==='dataGet').map(c=>c[1].record_id),[record.id]);
  view.poll();await tick();assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,2,'terminal failure must not replay acquisition');

  view.click('scope-collect');await view.until(()=>view.calls.filter(c=>c[0]==='captureStart').length===3);await tick();
  assert.match(view.html(),/workbench-job running/);assert.doesNotMatch(view.html(),/Waveform data exceeds the control message limit/);
  view.finish(id('b'));view.poll();await view.until(()=>view.html().includes('Latest saved screen'));
  assert.deepEqual(view.calls.filter(c=>c[0]==='experimentStatus').map(c=>c[1].run_id),[id(6),id('a'),id('b')]);
  assert.deepEqual(view.calls.filter(c=>c[0]==='dataGet').map(c=>c[1].record_id),[record.id,id('b')]);
  assert.match(view.html(),/data-waveform-plot/);assert.match(view.html(),/Unsaved/);
  assert.equal(view.calls.some(c=>c[0]==='previewSave'),false);assert.equal(location.hash,`#/host/${hostId}/device/${deviceId}`);
 }finally{view.restore();}
});

test('previous plot cannot supersede latest preview while acquisition is active',async()=>{
 const view=await mountedScope();let hold;try{
  view.click('scope-collect');await tick();view.finish();view.poll();await view.until(()=>view.html().includes('data-waveform-plot'));
  view.click('scope-collect');await tick();hold=view.holdRecordLoad();
  view.finish(id('b'));view.poll();await tick();hold.resolve();
  await view.until(()=>view.html().includes('Latest saved screen'));
  assert.deepEqual(view.calls.filter(c=>c[0]==='dataGet').map(c=>c[1].record_id),[record.id,id('b')]);
  assert.match(view.html(),new RegExp('data-record="'+id('b')+'"'));
  assert.doesNotMatch(view.html(),/<h2>Previous waveform/);
 }finally{hold?.resolve();view.restore();}
});

test('a delayed old status rejection cannot overwrite a newer scope run',async()=>{
 const view=await mountedScope();let hold;try{
  view.click('scope-collect');await tick();view.failStatus();view.poll();await view.until(()=>view.html().includes('Status request timed out'));
  view.failStatus(false);hold=view.holdStatus();view.click('scope-check');view.click('scope-collect');await tick();
  hold.reject(new Error('Old status request failed'));await tick();await tick();
  assert.match(view.html(),/workbench-job running/);assert.doesNotMatch(view.html(),/Old status request failed/);
  view.finish(id('b'));view.poll();await view.until(()=>view.html().includes('Latest saved screen'));
  assert.equal(view.calls.filter(c=>c[0]==='captureStart').length,2);
 }finally{hold?.resolve();view.restore();}
});
