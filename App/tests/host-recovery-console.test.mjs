import test from 'node:test';
import assert from 'node:assert/strict';
import {setImmediate as tick} from 'node:timers/promises';
import {mountConsole} from '../web/console-ui.js';
import {createConsoleSession} from '../web/main.js';

const hostId='1'.repeat(32),boot='2'.repeat(32),clientId='3'.repeat(32);
const cleanup={resource_released:false,process_exit:{confirmed:true,success:false,code:19},error:'Worker cleanup receipt is missing'};
const receipt={recovery_authorized:true,cleanup_verified:false,physical_zero_verified:false,incident_id:'a'.repeat(32),host_stopped:true};

async function mountedRecovery({gate,result=receipt,startupCode,initialCleanup=cleanup,stopError}={}){
 const prior=new Map(['document','window','location','confirm','setInterval','clearInterval'].map(k=>[k,{has:Object.hasOwn(globalThis,k),value:globalThis[k]}]));
 const nodes=new Map(),listeners=new Map(),windowEvents=new Map(),calls=[],confirmations=[];let controls=[],subscriber,ui,seq=0,recovered=false,allowed=true;
 const state={host_id:hostId,mode:'real',host_name:'Bench',worker_cleanup:initialCleanup,registry:{registry_rev:1,settings:{},devices:[],drafts:[],setups:[]},control:{},domains:{},operations:[]};
 function dispatch(event,element){for(const fn of listeners.get('#content'+event)||[])fn({target:element,defaultPrevented:false,preventDefault(){this.defaultPrevented=true;}});}
 function parse(html){return [...html.matchAll(/<(input|select|button|details)\b([^>]*)>/g)].map(([,tag,raw])=>{
  const attrs=Object.fromEntries([...raw.matchAll(/([\w-]+)="([^"]*)"/g)].map(m=>[m[1],m[2]]));
  const element={tagName:tag.toUpperCase(),id:attrs.id||'',dataset:Object.fromEntries(Object.entries(attrs).filter(([k])=>k.startsWith('data-')).map(([k,v])=>[k.slice(5).replace(/-([a-z])/g,(_,c)=>c.toUpperCase()),v])),type:attrs.type||'',disabled:/\bdisabled\b/.test(raw),value:attrs.value||'',open:false,
   matches:selector=>selector.split(',').some(s=>s.trim()===tag||s.includes('details')&&tag==='details'||s.includes('button')&&tag==='button'),closest:selector=>selector==='#content'?node('#content'):selector==='button'&&tag==='button'?element:null,getAttribute:name=>attrs[name]??null};return element;
 });}
 function node(selector){if(!nodes.has(selector)){let html='';nodes.set(selector,{scrollTop:0,scrollLeft:0,hidden:true,textContent:'',get innerHTML(){return html;},set innerHTML(value){html=value;if(selector==='#content')controls=parse(value);},querySelectorAll:selector=>controls.filter(e=>e.matches(selector)),querySelector:selector=>controls.find(e=>selector.startsWith('#')?e.id===selector.slice(1):[...selector.matchAll(/\[([\w-]+)="([^"]*)"\]/g)].every(([,name,value])=>e.getAttribute(name)===value)),addEventListener:(event,fn)=>{const key=selector+event;listeners.set(key,[...(listeners.get(key)||[]),fn]);}});}return nodes.get(selector);}
 const client={preferences:async()=>({}),connect:async()=>{calls.push('connect');if(startupCode&&!recovered)throw Object.assign(new Error('Worker ownership unresolved'),{code:startupCode});return{connected:true,mode:'real',worker_protocol:3};},disconnect:async()=>{calls.push('disconnect');},catalog:async()=>({models:[],categories:[]}),subscribe:async fn=>{subscriber=fn;return()=>{calls.push('unsubscribe');subscriber=null;};},requestSnapshot:async()=>{subscriber?.({type:'snapshot',host_id:hostId,boot_id:recovered?'4'.repeat(32):boot,seq:++seq,data:state});return{seq};},ping:async()=>({boot_id:recovered?'4'.repeat(32):boot,client_session_id:clientId,monotonic_ms:performance.now()}),driverStatus:async()=>({}),remoteStatus:async()=>({peers:[],pending:[]}),stopHost:async()=>{calls.push('stop');state.worker_cleanup=structuredClone(cleanup);if(stopError)throw new Error(stopError);return{resource_released:false,process_exit:{confirmed:true,success:false}};},
  recoverHost:async()=>{calls.push('recover');await gate;if(result instanceof Error)throw result;recovered=true;state.worker_cleanup=null;return structuredClone(result);}};
 const restore=()=>{for(const[k,v]of prior)if(v.has)globalThis[k]=v.value;else delete globalThis[k];};
 try{
  globalThis.document={activeElement:null,querySelector:node,getElementById:id=>controls.find(e=>e.id===id)};
  globalThis.window={scrollX:0,scrollY:0,scrollTo(){},addEventListener:(name,fn)=>windowEvents.set(name,fn)};globalThis.location={hash:'#settings'};globalThis.confirm=message=>{confirmations.push(message);return allowed;};globalThis.setInterval=()=>0;globalThis.clearInterval=()=>{};
  const session=createConsoleSession(client,()=>ui?.render());
  ui=mountConsole(session,{core:{invoke:async name=>name==='app_profile'?{network_only:false}:[]},event:{listen:async()=>()=>{}}});await ui.ready;
  const until=async predicate=>{const end=Date.now()+2000;while(!predicate()){assert.ok(Date.now()<end,'Recovery UI did not settle');await tick();}};
  return {ui,session,calls,confirmations,restore,until,html:()=>node('#content').innerHTML,notice:()=>node('#notice').textContent,
   click(name){const button=controls.find(e=>e.dataset.ui===name);assert.ok(button,'Missing '+name);assert.equal(button.disabled,false,'Disabled '+name);dispatch('click',{closest:s=>s==='button'?button:null});},
   navigate(hash){location.hash=hash;windowEvents.get('hashchange')?.();},confirm(value){allowed=value;}};
 }catch(error){restore();throw error;}
}

test('mounted Settings recovers explicitly, retains evidence and reconnects the Host once',async()=>{
 let release;const gate=new Promise(resolve=>release=resolve),f=await mountedRecovery({gate});
 try{
  assert.equal(f.calls.filter(c=>c==='recover').length,0);f.confirm(false);f.click('recover-host');await tick();assert.equal(f.calls.filter(c=>c==='recover').length,0);
  f.confirm(true);f.click('recover-host');await f.until(()=>f.calls.includes('recover'));assert.match(f.html(),/Recovering Host/);assert.match(f.html(),/activity-spinner/);
  f.navigate('#overview');assert.match(f.html(),/Instrument overview/);f.navigate('#settings');assert.match(f.html(),/Recovering Host/);
  release();await f.until(()=>f.calls.filter(c=>c==='connect').length===2);await tick();
  assert.deepEqual(f.calls,['connect','recover','disconnect','unsubscribe','connect']);
  assert.match(f.html(),/Host recovery authorized/);assert.match(f.html(),/Worker cleanup receipt is missing/);assert.match(f.html(),/a{32}/);
  assert.doesNotMatch(f.html(),/data-ui="recover-host"|activity-spinner/);
 }finally{release();f.restore();}
});

test('mounted Settings keeps an unsuccessful recovery connected and shows the failure',async()=>{
 const f=await mountedRecovery({result:new Error('Original Worker exit cannot be verified')});
 try{f.click('recover-host');await f.until(()=>f.html().includes('Original Worker exit cannot be verified'));await tick();
  assert.deepEqual(f.calls,['connect','recover']);assert.equal(f.session.store.host(hostId).connected,true);
  assert.match(f.html(),/Worker cleanup receipt is missing/);assert.doesNotMatch(f.html(),/activity-spinner/);
 }finally{f.restore();}
});

test('a disconnected ownership issue can recover through the native client without an active Host session',async()=>{
 const f=await mountedRecovery({startupCode:'OwnershipUnknown'});
 try{f.click('recover-host');await f.until(()=>f.calls.filter(c=>c==='connect').length===2);await tick();
  assert.deepEqual(f.calls,['connect','disconnect','recover','disconnect','connect']);
  assert.match(f.html(),/Host recovery authorized/);
 }finally{f.restore();}
});

test('a retained Stop Host refreshes read-only shutdown evidence and preserves its original error',async()=>{
 const f=await mountedRecovery({initialCleanup:null,stopError:'Normal cleanup receipt missing'});
 try{assert.doesNotMatch(f.html(),/data-ui="recover-host"/);f.click('stop-host');await f.until(()=>f.html().includes('data-ui="recover-host"'));await tick();
  assert.deepEqual(f.calls,['connect','stop']);assert.equal(f.session.store.host(hostId).connected,true);
  assert.match(f.html(),/Normal cleanup receipt missing/);assert.match(f.html(),/Worker cleanup receipt is missing/);
 }finally{f.restore();}
});
