import test from 'node:test';
import assert from 'node:assert/strict';

// An absent helper is a missing feature assertion during the first red run.
const api = await import('../web/visa-resources.js').catch(error => {
 if(error.code === 'ERR_MODULE_NOT_FOUND' && error.message.includes('visa-resources.js'))return {};
 throw error;
});
const gpib = {id:'gpib-visa',access:'visa',interfaces:['GPIB'],fields:{resource:{kind:'resource',interfaces:['GPIB']}}};
const usb = {id:'usb-visa',access:'visa',interfaces:['USB'],fields:{resource:{kind:'resource',interfaces:['USB']}}};
const model = {id:'aq6370',name:'AQ6370'};
const draft = (profile=gpib) => ({modelId:model.id,profileId:profile.id,params:{}});
const selected = (resource,profile=gpib) => ({...draft(profile),manualResource:true,params:{resource}});
const inventory = visa => ({visa,errors:{}});
const waitForReply = () => {let resolve;const promise=new Promise(done=>resolve=done);return {promise,resolve};};

test('non-VISA profiles do not acquire a VISA prerequisite',()=>{
 assert.equal(api.visaChoiceReady?.(draft(),{access:'serial'},100),true);
});

test('manual VISA resources must follow the native reviewed grammar and profile interface',()=>{
 const cases = [
  ['GPIB::4::INSTR',gpib,true], ['GPIB0::04::2::INSTR',gpib,true],
  ['GPIB0::100::INSTR',gpib,false], ['gpib0::4::INSTR',gpib,false],
  ['GPIB0::4::INSTR',usb,false], ['GPIB0::4::INSTR\n',gpib,false],
  ['USB::0x1313::0x8078::P-123.abc_9::INSTR',usb,true],
  ['USB0::1313::8078::P123::2::INSTR',usb,true],
  ['USB0::0x1313::0x8078::P 123::INSTR',usb,false],
  ['USB0::1313::8078::P123::A::INSTR',usb,false],
  ['USB0::1313::8078::::INSTR',usb,false],
  ['USB0::1313::8078::P123::INSTR',gpib,false],
  ['TCPIP0::127.0.0.1::inst0::INSTR',gpib,false], ['ASRL4::INSTR',gpib,false],
  ['GPIB0::4::INSTR', {...gpib,fields:{resource:{interfaces:[]}}},false],
  ['USB0::1313::8078::'+ 'A'.repeat(240) +'::INSTR',usb,false],
 ];
 for(const [resource,profile,want] of cases)assert.equal(api.visaChoiceReady?.(selected(resource,profile),profile,100),want,resource);
 assert.equal(api.visaChoiceReady?.(selected('GPIB0::4::INSTR'),{...gpib,fields:{}},100),true);
});

test('DG4202 network VISA resources are selectable only through a reviewed TCPIP profile',async()=>{
 const profile={id:'visa',access:'visa',interfaces:['USB','GPIB','TCPIP'],fields:{resource:{interfaces:['USB','GPIB','TCPIP']}}},model={id:'dg4202'},resource='TCPIP0::DG4202.local::inst0::INSTR';
 const d={modelId:model.id,profileId:profile.id,params:{}};
 await api.refreshVisaChoices(d,model,profile,async()=>inventory([resource,'TCPIP0::host::5025::SOCKET','TCPIP0::host/path::INSTR']),()=>{});
 assert.deepEqual(d.visaScan.candidates,[resource]);assert.equal(d.params.resource,resource);assert.equal(api.visaChoiceReady(d,profile),true);
 for(const value of ['TCPIP::bench-2.local::INSTR','TCPIP01::192.0.2.1::inst02::INSTR'])assert.equal(api.visaChoiceReady({...d,manualResource:true,params:{resource:value}},profile),true);
 for(const value of ['TCPIP0::host::instx::INSTR','TCPIP0::::INSTR','TCPIP0::host::INSTR\n*RST'])assert.equal(api.visaChoiceReady({...d,manualResource:true,params:{resource:value}},profile),false);
 assert.equal(api.visaChoiceReady({...d,manualResource:true},gpib),false);
});

test('detected VISA selections require a current matching inventory that includes the selected resource',()=>{
 const d={...draft(),params:{resource:'GPIB0::4::INSTR'},visaScan:{modelId:model.id,profileId:gpib.id,state:'ready',resources:['GPIB0::4::INSTR'],candidates:['GPIB0::4::INSTR'],issued:100}};
 assert.equal(api.visaChoiceReady?.(d,gpib,100),true);
 assert.equal(api.visaChoiceReady?.(d,gpib,60099),true);
 assert.equal(api.visaChoiceReady?.(d,gpib,60100),false);
 assert.equal(api.visaChoiceReady?.(d,gpib,99),false);
 for(const change of [{state:'checking'},{state:'failed'},{modelId:'pm400'},{profileId:'usb-visa'},{resources:[]},{issued:NaN}])assert.equal(api.visaChoiceReady?.({...d,visaScan:{...d.visaScan,...change}},gpib,100),false);
 assert.equal(api.visaChoiceReady?.({...d,params:{resource:'TCPIP0::local::INSTR'}},gpib,100),false);
 assert.equal(api.visaChoiceReady?.({...d,visaScan:null},gpib,100),false);
});

test('refresh filters by resource field interfaces and treats every returned address as an unverified candidate',async()=>{
 const d=draft();
 const profile={...gpib,interfaces:['USB'],fields:{resource:{interfaces:['GPIB']}}};
 await api.refreshVisaChoices?.(d,model,profile,async()=>inventory(['USB0::1313::8078::P123::INSTR','GPIB0::4::INSTR','GPIB0::04::INSTR','GPIB0::4::INSTR','TCPIP0::localhost::INSTR','GPIB0::100::INSTR']),()=>{});
 assert.equal(d.visaScan?.state,'ready');
 assert.deepEqual(d.visaScan.resources,['GPIB0::04::INSTR','GPIB0::4::INSTR','TCPIP0::localhost::INSTR','USB0::1313::8078::P123::INSTR']);
 assert.deepEqual(d.visaScan.candidates,['GPIB0::04::INSTR','GPIB0::4::INSTR']);
 assert.equal(d.params.resource,undefined);
 assert.equal(api.visaChoiceReady(d,profile),false);
});

test('one candidate may be proposed only for an untouched detected selection',async()=>{
 const d=draft();
 await api.refreshVisaChoices?.(d,model,gpib,async()=>inventory(['GPIB0::4::INSTR']),()=>{});
 assert.equal(d.params.resource,'GPIB0::4::INSTR');
 assert.equal(api.visaChoiceReady(d,gpib),true);
 const manual={...draft(),manualResource:true};
 await api.refreshVisaChoices?.(manual,model,gpib,async()=>inventory(['GPIB0::4::INSTR']),()=>{});
 assert.equal(manual.params.resource,undefined);
 const chosen={...draft(),params:{resource:'GPIB0::8::INSTR'},proof:{proof_id:'existing'}};
 await api.refreshVisaChoices?.(chosen,model,gpib,async()=>inventory(['GPIB0::4::INSTR','GPIB0::8::INSTR']),()=>{});
 assert.equal(chosen.params.resource,'GPIB0::8::INSTR');
 assert.equal(chosen.proof.proof_id,'existing');
});

test('a disappeared address invalidates proof and never silently chooses the replacement',async()=>{
 for(const manualResource of [false,true]){
  const d={...draft(),manualResource,params:{resource:'GPIB0::4::INSTR'},proof:{proof_id:'old'}};
  await api.refreshVisaChoices?.(d,model,gpib,async()=>inventory(['GPIB0::8::INSTR']),()=>{});
  assert.equal(d.proof,null);
  assert.equal(d.params.resource,'GPIB0::4::INSTR');
  await api.refreshVisaChoices?.(d,model,gpib,async()=>inventory(['GPIB0::8::INSTR']),()=>{});
  assert.equal(d.params.resource,'GPIB0::4::INSTR');
  assert.equal(api.visaChoiceReady(d,gpib),manualResource);
 }
});

test('pending refresh acknowledges immediately and keeps previous results as historical data',async()=>{
 const reply=waitForReply(),d={...draft(),params:{resource:'GPIB0::4::INSTR'},visaScan:{modelId:model.id,profileId:gpib.id,state:'ready',resources:['GPIB0::4::INSTR'],candidates:['GPIB0::4::INSTR'],issued:1}};
 let changes=0;
 const pending=api.refreshVisaChoices?.(d,model,gpib,()=>reply.promise,()=>{changes++;});
 assert.equal(d.visaScan?.state,'checking');
 assert.equal(changes,1);
 assert.deepEqual(d.visaScan.resources,['GPIB0::4::INSTR']);
 assert.equal(api.visaChoiceReady(d,gpib),false);
 reply.resolve(inventory(['GPIB0::4::INSTR']));await pending;
 assert.equal(d.visaScan.state,'ready');assert.equal(changes,2);
});

test('same pending refresh is deduplicated instead of issuing another inventory read',async()=>{
 const reply=waitForReply(),d=draft();let reads=0;
 const read=()=>{reads++;return reply.promise;};
 const first=api.refreshVisaChoices?.(d,model,gpib,read,()=>{});
 const second=api.refreshVisaChoices?.(d,model,gpib,read,()=>{});
 assert.equal(reads,1);
 reply.resolve(inventory(['GPIB0::4::INSTR']));await Promise.all([first,second]);
 assert.equal(d.visaScan.state,'ready');
});

test('late inventory replies cannot overwrite a newer scan or a different model and profile',async()=>{
 const old=waitForReply(),d=draft();
 const first=api.refreshVisaChoices?.(d,model,gpib,()=>old.promise,()=>{});
 d.profileId=usb.id;
 await api.refreshVisaChoices?.(d,model,usb,async()=>inventory(['USB0::1313::8078::P123::INSTR']),()=>{});
 old.resolve(inventory(['GPIB0::4::INSTR']));await first;
 assert.equal(d.visaScan?.profileId,usb.id);assert.equal(d.params.resource,'USB0::1313::8078::P123::INSTR');
 const late=waitForReply(),changed=draft();
 const pending=api.refreshVisaChoices?.(changed,model,gpib,()=>late.promise,()=>{});
 changed.modelId='pm400';late.resolve(inventory(['GPIB0::4::INSTR']));await pending;
 assert.equal(changed.params.resource,undefined);
 assert.notEqual(changed.visaScan?.state,'ready');
});

test('an operator selection or manual edit during refresh is never replaced by a late proposal',async()=>{
 for(const [resource,manualResource] of [['GPIB0::8::INSTR',false],['GPIB0::9::INSTR',true]]){
  const reply=waitForReply(),d=draft();
  const pending=api.refreshVisaChoices?.(d,model,gpib,()=>reply.promise,()=>{});
  d.params.resource=resource;d.manualResource=manualResource;d.proof={proof_id:'new'};
  reply.resolve(inventory(['GPIB0::4::INSTR']));await pending;
  assert.equal(d.params.resource,resource);assert.equal(d.proof,null);
 }
});

test('empty, failed and malformed inventories never claim a detected selection is ready',async()=>{
 const cases=[
  [inventory([]),'ready'],[{visa:[],errors:{visa:'VISA service unavailable'}},'failed'],
  [null,'failed'],[{},'failed'],[{visa:{},errors:{}},'failed'],
  [{visa:[],errors:[]},'failed'],[{visa:[null],errors:{}},'failed'],[{visa:Array(1),errors:{}},'failed'],
  [{visa:[{resource:'GPIB0::4::INSTR'}],errors:{}},'failed'],
  [{visa:['GPIB0::4::INSTR\u0000'],errors:{}},'failed'],
  [{visa:['A'.repeat(257)],errors:{}},'failed'],
  [{visa:Array(4097).fill('GPIB0::4::INSTR'),errors:{}},'failed'],
 ];
 for(const [reply,want] of cases){
  const d={...draft(),params:{resource:'GPIB0::4::INSTR'},proof:{proof_id:'old'}};
  await api.refreshVisaChoices?.(d,model,gpib,async()=>reply,()=>{});
  assert.equal(d.visaScan?.state,want);assert.equal(api.visaChoiceReady(d,gpib),false);assert.equal(d.proof,null);
  if(want==='failed')assert.ok(d.visaScan.message.length);
 }
 const d=draft();await api.refreshVisaChoices?.(d,model,gpib,async()=>{throw Error('Read timed out');},()=>{});
 assert.equal(d.visaScan?.state,'failed');assert.match(d.visaScan.message,/Read timed out/);
});

test('the bounded inventory accepts 4096 records and keeps only one copy of each address',async()=>{
 const d=draft();await api.refreshVisaChoices?.(d,model,gpib,async()=>inventory(Array(4096).fill('GPIB0::4::INSTR')),()=>{});
 assert.equal(d.visaScan?.state,'ready');assert.deepEqual(d.visaScan.candidates,['GPIB0::4::INSTR']);
});

test('non-VISA refresh does not invoke inventory enumeration',async()=>{
 const d=draft();let reads=0;
 await api.refreshVisaChoices?.(d,model,{id:'serial',access:'serial'},async()=>{reads++;return inventory([]);},()=>{});
 assert.equal(reads,0);assert.equal(d.visaScan,null);
});

test('detected picker exposes candidates with manual fallback and collapses supplied advanced settings',()=>{
 const d={...draft(),params:{resource:'GPIB0::4::INSTR'},visaScan:{modelId:model.id,profileId:gpib.id,state:'ready',resources:['GPIB0::4::INSTR','USB0::1313::8078::P123::INSTR'],candidates:['GPIB0::4::INSTR'],issued:performance.now()}};
 const advanced='<label>Timeout (seconds)<input data-param="timeout_s" value="30"></label>';
 const html=api.renderVisaField?.(d,gpib,advanced)??'';
 assert.match(html,/<label>Resource<select[^>]*data-param="resource"[^>]*data-managed="true"/);
 assert.match(html,/<option[^>]*value="GPIB0::4::INSTR"[^>]*selected/);
 assert.doesNotMatch(html,/USB0::1313/);
 assert.match(html,/data-ui="scan-visa"/);assert.match(html,/data-ui="visa-manual"/);
 assert.match(html,/<details[^>]*id="visa-advanced"[^>]*>/);
 assert.doesNotMatch(html,/<details[^>]*id="visa-advanced"[^>]*\sopen(?:\s|>)/);
 assert.ok(html.includes(advanced));
 assert.match(html,/candidate/i);assert.match(html,/verifies.*identity/i);
 assert.doesNotMatch(html,/<input[^>]*data-param="resource"/);
});

test('manual picker preserves and escapes the address without exposing it as a verified resource',()=>{
 const d=selected('GPIB0::4::INSTR"<script>');
 const html=api.renderVisaField?.(d,gpib)??'';
 assert.match(html,/<input[^>]*data-param="resource"[^>]*value="GPIB0::4::INSTR&quot;/);
 assert.doesNotMatch(html,/<script>/);
 assert.match(html,/Use detected resources/);
 assert.doesNotMatch(html,/<select[^>]*data-param="resource"/);
});

test('checking VISA resources is explicit and marks retained choices as previous inventory',()=>{
 const d={...draft(),params:{resource:'GPIB0::4::INSTR'},visaScan:{modelId:model.id,profileId:gpib.id,state:'checking',resources:['GPIB0::4::INSTR'],candidates:['GPIB0::4::INSTR'],issued:performance.now()-2500}};
 const html=api.renderVisaField?.(d,gpib)??'';
 assert.match(html,/<select[^>]*data-param="resource"[^>]*disabled/);
 assert.match(html,/<button[^>]*data-ui="scan-visa"[^>]*disabled/);
 assert.match(html,/<progress[^>]*>/);assert.doesNotMatch(html,/<progress[^>]*value=/);
 assert.match(html,/Checking VISA resources/);assert.match(html,/Previous/i);
 assert.match(html,/<span[^>]*class="activity-elapsed"[^>]*data-activity-elapsed="[-\d.]+"[^>]*data-activity-ended=""[^>]*>2\s*s<\/span> elapsed/);
});

test('empty and unavailable inventories explain refresh and manual fallback without inventing identity',()=>{
 const empty={...draft(),visaScan:{modelId:model.id,profileId:gpib.id,state:'ready',resources:[],candidates:[],issued:performance.now()}};
 const html=api.renderVisaField?.(empty,gpib)??'';
 assert.match(html,/No VISA resources detected/);assert.match(html,/manually/);assert.doesNotMatch(html,/AQ6370/);
 const failed={...draft(),visaScan:{modelId:model.id,profileId:gpib.id,state:'failed',resources:[],candidates:[],issued:performance.now(),message:'Cable <unplugged> & "offline"'}};
 const failure=api.renderVisaField?.(failed,gpib)??'';
 assert.match(failure,/Cable &lt;unplugged&gt; &amp;/);assert.doesNotMatch(failure,/<unplugged>/);
 assert.match(failure,/data-ui="visa-manual"/);assert.match(failure,/role="alert"/);
});

test('expired, mismatched and disappeared choices remain explicit previous or unavailable selections',()=>{
 const d={...draft(),params:{resource:'GPIB0::4::INSTR'},visaScan:{modelId:model.id,profileId:gpib.id,state:'ready',resources:['GPIB0::8::INSTR'],candidates:['GPIB0::8::INSTR'],issued:performance.now()}};
 const html=api.renderVisaField?.(d,gpib)??'';
 assert.match(html,/<option[^>]*value="GPIB0::4::INSTR"[^>]*selected[^>]*>[^<]*unavailable/i);
 const old=api.renderVisaField?.({...d,visaScan:{...d.visaScan,issued:performance.now()-60001}},gpib)??'';
 assert.match(old,/Previous/);assert.match(old,/refresh/i);
 const mismatch=api.renderVisaField?.({...d,visaScan:{...d.visaScan,profileId:usb.id}},gpib)??'';
 assert.doesNotMatch(mismatch,/GPIB0::8::INSTR/);assert.match(mismatch,/refresh/i);
});
