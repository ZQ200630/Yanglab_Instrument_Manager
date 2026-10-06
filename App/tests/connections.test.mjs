import test from 'node:test';import assert from 'node:assert/strict';
import {setImmediate as tick} from 'node:timers/promises';
import {renderConnections,connectionEndpoint,connectionState} from '../web/connections.js';
import {mountConsole} from '../web/console-ui.js';import {createConsoleSession} from '../web/main.js';
const id='a'.repeat(32),ticket='b'.repeat(32),pin='f'.repeat(64);
test('connection endpoints are canonical explicit loopback/Tailscale addresses',()=>{
 for(const [input,output] of [['100.65.2.3','100.65.2.3:9443'],['127.0.0.1:9555','127.0.0.1:9555'],['[fd7a:115c:a1e0::2]:9555','[fd7a:115c:a1e0::2]:9555'],['::1','[::1]:9443'],['fd7a:115c:a1e0::2','[fd7a:115c:a1e0::2]:9443']])assert.equal(connectionEndpoint(input),output);
 for(const value of ['https://public.example','localhost','8.8.8.8','192.168.1.2','100.128.0.1','127.0.0.1:0','100.65.2.3:65536','100.065.2.3','127.0.0.1/path','[::ffff:100.65.2.3]:9443'])assert.throws(()=>connectionEndpoint(value),value);
});
test('two accessible tabs keep Add PC collapsed and expose no manual secrets',()=>{
 const state=connectionState();let html=renderConnections({peers:[],owner:null},state);
 assert.match(html,/role="tablist"/);assert.match(html,/Control this PC/);assert.match(html,/Control other PCs/);assert.match(html,/role="tabpanel"/);assert.doesNotMatch(html,/id="remote-(fingerprint|code|endpoint)"/);
 state.tab='other';html=renderConnections({peers:[]},state);assert.match(html,/Add PC/);assert.doesNotMatch(html,/id="remote-endpoint"/);state.add=true;state.draft.endpoint='100.65.2.3';html=renderConnections({peers:[]},state);assert.match(html,/value="100.65.2.3"/);assert.match(html,/Request connection/);assert.doesNotMatch(html,/id="remote-(fingerprint|code)"/);
});
test('owner requests need only direct Approve Reject, without a comparison number',()=>{
 const html=renderConnections({owner:{transport:{state:'LISTENING'},listener:'100.65.2.3:9443',pending:[{id:ticket,name:'Other PC',source_ip:'100.65.2.4',comparison:'919680',expires_in_ms:90000}],peers:[{id,name:'Authorized PC'}],fingerprint:pin}},connectionState());
 assert.doesNotMatch(html,/919680|Comparison number|Compare.*number/);assert.match(html,/100.65.2.4/);assert.match(html,/remote-approve/);assert.match(html,/remote-reject/);assert.doesNotMatch(html,/Open pairing|type="checkbox"/);assert.match(html,/<details[^>]*>[\s\S]*Details[\s\S]*Revoke access/);
});
test('requester waits for approval without displaying a number or asking for code entry',()=>{
 const state=connectionState();state.tab='other';state.request={request_id:ticket,phase:'Waiting',comparison:'919680',expires_in_ms:90000};
 const html=renderConnections({peers:[]},state);assert.match(html,/Waiting for approval/);assert.match(html,/Cancel request/);assert.doesNotMatch(html,/919680|Comparison number|Compare|id="remote-code"/);
});
test('network-only profile cannot enable access and long identities live in Details',()=>{
 const html=renderConnections({networkOnly:true,owner:null},connectionState());assert.match(html,/data-ui="remote-enable"[^>]*disabled/);
 const state=connectionState();state.tab='other';const paired=renderConnections({peers:[{host_id:id,name:'Owner',endpoint:'100.65.2.3:9443',fingerprint:pin,connected:false}]},state);
 assert.match(paired,/Paired/);assert.match(paired,/OFFLINE/);assert.match(paired,/<details[^>]*>[\s\S]*Details[\s\S]*Host ID/);assert.equal((paired.match(/>Connect<\/button>/g)||[]).length,1);
});

// Only native command/event boundary and finite DOM nodes are substituted.
async function mounted({networkOnly=false,connectFailure=false,savedPeers=[]}={}){
 const keys=['document','window','location','confirm','setInterval','clearInterval'],prior=new Map(keys.map(k=>[k,{exists:Object.hasOwn(globalThis,k),value:globalThis[k]}]));const nodes=new Map(),listeners=new Map(),intervals=[],calls=[],windowEvents=new Map();let ui,seq=0,subscriber,paired=savedPeers,request={request_id:ticket,phase:'Waiting',comparison:'919680',expires_in_ms:120000};
 let controls=[],ownerExpiry=120000;
 const parseControls=markup=>{const result=[],details=[];for(const match of markup.matchAll(/<(\/?)(input|select|button|summary|details)\b([^>]*)>/g)){
  const [,closing,tag,raw]=match;if(closing){if(tag==='details')details.pop();continue;}
  const attrs=Object.fromEntries([...raw.matchAll(/([\w-]+)="([^"]*)"/g)].map(m=>[m[1],m[2]]));
  const element={tagName:tag.toUpperCase(),id:attrs.id||'',dataset:Object.fromEntries(Object.entries(attrs).filter(([k])=>k.startsWith('data-')).map(([k,v])=>[k.slice(5),v])),value:attrs.value||'',type:attrs.type||'',open:/\bopen\b/.test(raw),disabled:/\bdisabled\b/.test(raw),parent:details.at(-1),
   matches(selector){return selector.split(',').some(part=>{part=part.trim();if(part==='input'||part==='select')return tag===part;if(part==='details[id]')return tag==='details'&&Boolean(this.id);if(part==='[role="tab"]')return attrs.role==='tab';if(part.includes('button'))return tag==='button'&&Boolean(this.dataset.ui);if(part.includes('summary'))return tag==='summary'&&Boolean(this.parent?.id);return false;});},
   closest(selector){if(selector==='#content')return nodes.get('#content');if(selector==='details[id]')return this.parent;return null;},focus(){globalThis.document.activeElement=this;}};
  result.push(element);if(tag==='details')details.push(element);
 }return result;};
 const node=s=>{if(!nodes.has(s)){let markup='';nodes.set(s,{id:s.slice(1),value:'',hidden:true,textContent:'',get innerHTML(){return markup;},set innerHTML(value){markup=value;if(s==='#content'){if(controls.includes(globalThis.document.activeElement))globalThis.document.activeElement=null;controls=parseControls(value);}},querySelectorAll:selector=>s==='#content'?controls.filter(e=>e.matches(selector)):[],addEventListener:(type,fn)=>listeners.set(type,[...(listeners.get(type)||[]),fn]),focus(){globalThis.document.activeElement=this;}});}return nodes.get(s);};
 const state={host_id:id,host_name:'Local PC',mode:'real',registry:{devices:[],setups:[],drafts:[],settings:{}},domains:{},control:{}};
 const publish=()=>subscriber?.({type:'snapshot',host_id:id,boot_id:'c'.repeat(32),seq:++seq,data:state});
 const client={preferences:async()=>({pythonPath:'VISA'}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),catalog:async()=>({models:[],categories:[]}),subscribe:async fn=>{subscriber=fn;return()=>{};},requestSnapshot:async()=>{publish();return{seq};},ping:async()=>({boot_id:'c'.repeat(32),client_session_id:'d'.repeat(32),monotonic_ms:performance.now()}),remoteStatus:async()=>({listener:'127.0.0.1:9443',transport:{state:'LISTENING'},pending:[{id:ticket,name:'Peer',comparison:'919680',expires_in_ms:ownerExpiry},{id:'9'.repeat(32),name:'Second peer',comparison:'096479',expires_in_ms:ownerExpiry}],peers:[]}),approvePeer:async selected=>calls.push(['approve',selected]),rejectPeer:async selected=>calls.push(['reject',selected]),disconnect:async()=>{}};
 const native={event:{listen:async()=>()=>{}},core:{invoke:async(name,args)=>{calls.push([name,args]);if(name==='app_profile')return{network_only:networkOnly};if(name==='remote_peers')return paired;if(name==='remote_pair_request')return request;if(name==='remote_pair_status')return request;if(name==='remote_pair_cancel'){request={...request,phase:'Cancelled',possible_owner_authorization:true};return request;}if(name==='remote_connect'&&connectFailure)throw Error('Owner offline');throw Error('Unexpected native command '+name);}}};
 try{globalThis.document={querySelector:node,getElementById:i=>node('#'+i),activeElement:null};globalThis.window={addEventListener:(type,fn)=>windowEvents.set(type,fn)};globalThis.location={hash:'#settings'};globalThis.confirm=()=>{calls.push(['confirm']);return true;};globalThis.setInterval=(fn,ms)=>{intervals.push({fn,ms});return intervals.length;};globalThis.clearInterval=()=>{};const session=createConsoleSession(client,()=>ui?.render());ui=mountConsole(session,native);await ui.ready;
  const fire=(type,target,extras={})=>(listeners.get(type)||[]).forEach(fn=>fn({target,...extras}));return{calls,session,ui,html:()=>node('#content').innerHTML,input(field,value){const e=controls.find(e=>e.id===field)||node('#'+field);e.value=value;fire('input',e);},click(action,extra={}){fire('click',{closest:selector=>selector==='button'?{disabled:false,dataset:{ui:action,...extra}}:null});},key(key,extra={}){fire('keydown',{closest:()=>null},{key,preventDefault(){},...extra});},async poll(){await intervals.find(i=>i.ms===1500).fn();await tick();},expire(ms){ownerExpiry=ms;request={...request,expires_in_ms:ms};},focusAction(action,peer){const e=controls.find(e=>e.dataset.ui===action&&(!peer||e.dataset.peer===peer));assert.ok(e,'rendered action exists');e.focus();},focusSummary(id){const e=controls.find(e=>e.tagName==='SUMMARY'&&e.parent?.id===id);assert.ok(e,'rendered summary exists');e.focus();},focused:()=>globalThis.document.activeElement,navigate(hash){globalThis.location.hash=hash;windowEvents.get('hashchange')?.();},complete(){paired=[{host_id:'e'.repeat(32),name:'Owner',endpoint:'100.65.2.3:9443',fingerprint:pin}];request={request_id:ticket,phase:'Completed',peer:paired[0]};},restore(){for(const [k,v]of prior)if(v.exists)globalThis[k]=v.value;else delete globalThis[k];}};
 }catch(e){for(const [k,v]of prior)if(v.exists)globalThis[k]=v.value;else delete globalThis[k];throw e;}
}
test('mounted request occurs once, status polling and tab/page changes preserve drafts',async()=>{
 const f=await mounted();try{f.click('connections-tab',{tab:'other'});await tick();f.click('remote-add');await tick();f.input('remote-endpoint','100.65.2.3');f.input('remote-name','Laser PC');await f.poll();f.navigate('#overview');f.navigate('#settings');assert.match(f.html(),/value="100.65.2.3"/);
 f.click('remote-request');f.click('remote-request');await tick();assert.match(f.html(),/Waiting for approval/);assert.doesNotMatch(f.html(),/919680/);await f.poll();f.click('connections-tab',{tab:'this'});await tick();f.click('connections-tab',{tab:'other'});await tick();assert.match(f.html(),/Waiting for approval/);assert.equal(f.calls.filter(([n])=>n==='remote_pair_request').length,1);assert.equal(f.calls.find(([n])=>n==='remote_pair_request')[1].endpoint,'100.65.2.3:9443');assert.ok(f.calls.some(([n])=>n==='remote_pair_status'));assert.equal(f.session.store.hosts().filter(h=>h.remote).length,0);
 f.click('remote-cancel');await tick();assert.match(f.html(),/Cancelled/);assert.match(f.html(),/owner.*authorization/i);assert.equal(f.calls.filter(([n])=>n==='remote_pair_cancel').length,1);
 }finally{f.restore();}
});
test('mounted owner Approve Reject are direct, without an extra confirmation',async()=>{const f=await mounted();try{f.click('remote-approve',{peer:ticket});await tick();f.click('remote-reject',{peer:ticket});await tick();assert.equal(f.calls.filter(([n])=>n==='approve').length,1);assert.equal(f.calls.filter(([n])=>n==='reject').length,1);assert.ok(!f.calls.some(([n])=>n==='confirm'));}finally{f.restore();}});
test('completed pairing with failed connection remains Paired Offline without replay',async()=>{const f=await mounted({networkOnly:true,connectFailure:true});try{f.click('connections-tab',{tab:'other'});f.click('remote-add');f.input('remote-endpoint','100.65.2.3');f.click('remote-request');await tick();f.complete();await f.poll();assert.match(f.html(),/Paired/);assert.match(f.html(),/OFFLINE/);assert.match(f.html(),/>Connect<\/button>/);await f.poll();assert.equal(f.calls.filter(([n])=>n==='remote_pair_request').length,1);assert.equal(f.calls.filter(([n])=>n==='remote_connect').length,1);assert.equal(f.session.hostId,null);}finally{f.restore();}});
test('saved trusted peer uses ordinary Connect after App restart, never requests approval again',async()=>{
 const peer={host_id:'e'.repeat(32),name:'Owner',endpoint:'100.65.2.3:9443',fingerprint:pin};
 const f=await mounted({networkOnly:true,connectFailure:true,savedPeers:[peer]});try{
  f.click('connections-tab',{tab:'other'});assert.match(f.html(),/Paired/);assert.match(f.html(),/>Connect<\/button>/);assert.doesNotMatch(f.html(),/Request connection|Approve/);
  f.click('remote-connect',{host:peer.host_id});await tick();await f.poll();f.click('remote-connect',{host:peer.host_id});await tick();
  assert.deepEqual(f.calls.filter(([n])=>n==='remote_connect').map(([,a])=>a),[{hostId:'e'.repeat(32)},{hostId:'e'.repeat(32)}]);
  assert.ok(!f.calls.some(([n])=>n.startsWith('remote_pair_')||n==='approve'||n==='reject'||n==='remote_forget'));assert.match(f.html(),/Paired/);assert.match(f.html(),/OFFLINE/);
 }finally{f.restore();}
});
test('Escape cancels only the current native request',async()=>{const f=await mounted();try{f.click('connections-tab',{tab:'other'});f.click('remote-add');f.input('remote-endpoint','127.0.0.1');f.click('remote-request');await tick();f.key('Escape');await tick();assert.equal(f.calls.filter(([n])=>n==='remote_pair_cancel').length,1);}finally{f.restore();}});
test('countdown refresh preserves the exact Approve Reject Cancel and Details keyboard target',async()=>{
 const f=await mounted();try{
  const second='9'.repeat(32);
  for(const [action,ms]of [['remote-approve',118000],['remote-reject',116000]]){f.focusAction(action,second);f.expire(ms);await f.poll();assert.equal(f.focused()?.dataset.ui,action);assert.equal(f.focused()?.dataset.peer,second);}
  f.click('connections-tab',{tab:'other'});f.click('remote-add');f.input('remote-endpoint','127.0.0.1');f.click('remote-request');await tick();
  f.focusAction('remote-cancel');f.expire(114000);await f.poll();assert.equal(f.focused()?.dataset.ui,'remote-cancel');
  f.focusSummary('remote-nickname');f.expire(112000);await f.poll();assert.equal(f.focused()?.tagName,'SUMMARY');assert.equal(f.focused()?.parent.id,'remote-nickname');
  assert.ok(!f.calls.some(([n])=>n==='approve'||n==='reject'||n==='remote_pair_cancel'));
 }finally{f.restore();}
});
