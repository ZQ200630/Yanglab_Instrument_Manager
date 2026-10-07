// Native bridge/DOM boundary only. Production mounting, handlers, rendering,
// result hydration, drafts, navigation and lifecycle feedback remain intact.
import {setImmediate as tick} from 'node:timers/promises';
import assert from 'node:assert/strict';
import {mountConsole} from '../web/console-ui.js';
import {createConsoleSession} from '../web/main.js';
import {savedTrace,hostId,domain} from './osa-fixture.mjs';
export const nativeOsaRoute=`#/host/${hostId}/device/${domain.id}`;
export async function mountNativeOsa({gate}={}){
 const captures=[await savedTrace(),await savedTrace({id:'2'.repeat(32)})],boot='b'.repeat(32),sessionId='d'.repeat(32),context={session_id:'f'.repeat(32),domain,connection_id:'e'.repeat(32),epoch:1};
 const globals=['document','window','location','confirm','setInterval','clearInterval'],prior=new Map(globals.map(k=>[k,{has:Object.hasOwn(globalThis,k),value:globalThis[k]}]));
 const nodes=new Map(),listeners=new Map(),windowEvents=new Map(),calls=[];let controls=[],ui,subscriber,seq=0,sequence=0;
 const operation=f=>({operation_id:f.reference.id,domain,status:'Terminal',phase:'completed',command:{method:'action',params:{name:'read_trace',args:{trace:'A'}}},result:{context,result:{archive_ref:f.reference,timings:{instrument_io_ms:12,decode_ms:2,staging_ms:3}}}});
 const state={host_id:hostId,host_name:'Bench',mode:'real',archive:{active_root:'D:/Data',available:true},control:{[`${domain.kind}:${domain.id}`]:{state:'CONTROLLED',controller_session:sessionId,control_epoch:0}},
  registry:{registry_rev:1,settings:{host_name:'Bench'},devices:[{device_id:domain.id,name:'Bench OSA',model_id:'aq6370',profile_id:'gpib-visa',config_rev:1,params:{resource:'GPIB0::4::INSTR'}}],drafts:[],setups:[]},
  domains:{[`${domain.kind}:${domain.id}`]:{context,state:'READY',device:{connected:true,state:'READY',identity:'YOKOGAWA,AQ6370E,HOST-OSA-4,1.0'}}},operations:[operation(captures[0])]};
 const parse=html=>[...html.matchAll(/<(input|select|button|details)\b([^>]*)>/g)].map(([,tag,raw])=>{
  const attrs=Object.fromEntries([...raw.matchAll(/([\w-]+)="([^"]*)"/g)].map(m=>[m[1],m[2]]));
  const element={tagName:tag.toUpperCase(),id:attrs.id||'',dataset:Object.fromEntries(Object.entries(attrs).filter(([k])=>k.startsWith('data-')).map(([k,v])=>[k.slice(5),v])),type:attrs.type||'',disabled:/\bdisabled\b/.test(raw),value:attrs.value||'',selectionStart:0,selectionEnd:0,open:false,
   matches(selector){return selector.split(',').some(s=>s.trim()===tag||s.includes('details')&&tag==='details'||s.includes('button')&&tag==='button');},closest:s=>s==='#content'?node('#content'):null,
   focus(){document.activeElement=this;},setSelectionRange(start,end){this.selectionStart=start;this.selectionEnd=end;}};
  if(tag==='select')element.value='A';return element;
 });
 function node(selector){if(!nodes.has(selector)){let html='';nodes.set(selector,{scrollTop:0,scrollLeft:0,hidden:true,textContent:'',get innerHTML(){return html;},set innerHTML(value){html=value;if(selector==='#content'){if(controls.includes(document.activeElement))document.activeElement=null;controls=parse(value);}},querySelectorAll:s=>selector==='#content'?controls.filter(e=>e.matches(s)):[],addEventListener:(event,handler)=>{const key=selector+event;listeners.set(key,[...(listeners.get(key)||[]),handler]);}});}return nodes.get(selector);}
 const publish=()=>subscriber?.({type:'snapshot',host_id:hostId,boot_id:boot,seq:++seq,data:state});
 const client={preferences:async()=>({}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},
  catalog:async()=>({categories:['OSA'],models:[{id:'aq6370',profiles:[{id:'gpib-visa',open_effects:[]}]}]}),subscribe:async fn=>{subscriber=fn;return()=>{};},requestSnapshot:async()=>{publish();return {seq};},ping:async()=>({boot_id:boot,client_session_id:sessionId,monotonic_ms:performance.now()}),
  remoteStatus:async()=>({peers:[],pending:[]}),listArchives:async()=>({entries:[],offset:0,more:false}),
  archiveManifestBytes:p=>captures.find(f=>f.reference.id===p.id).client.archiveManifestBytes(p),readArchive:p=>captures.find(f=>f.reference.id===p.id).client.readArchive(p),
  prepare:async intent=>{calls.push(['prepare',structuredClone(intent)]);return {token:'proof'};},
  execute:async(id,intent)=>{calls.push(['execute',id,structuredClone(intent)]);await gate;const result={...operation(captures[1]),request_id:id};state.operations.push(result);publish();return result;},
  nextSequence:()=>++sequence,renew:async()=>{throw new Error('No live timer in finite fixture');}};
 try{
  globalThis.document={activeElement:null,querySelector:node,getElementById:id=>controls.find(e=>e.id===id)||node('#'+id)};
  globalThis.window={scrollX:0,scrollY:0,scrollTo(x,y){this.scrollX=x;this.scrollY=y;},addEventListener:(event,fn)=>windowEvents.set(event,fn)};globalThis.location={hash:nativeOsaRoute};globalThis.confirm=()=>true;globalThis.setInterval=()=>1;globalThis.clearInterval=()=>{};
  const session=createConsoleSession(client,()=>ui?.render());ui=mountConsole(session,{event:{listen:async()=>()=>{}},core:{invoke:async name=>{if(name==='app_profile')return{network_only:false};if(name==='remote_peers')return[];throw Error(name);}}});await ui.ready;
  const key=`${hostId}/device/${domain.id}`;session.store.setLease(key,{token:'1'.repeat(32),boot_id:boot,session_id:sessionId,domain,control_epoch:0,expires_in_ms:10000});publish();
  const until=async predicate=>{const deadline=Date.now()+2000;while(!predicate()){assert.ok(Date.now()<deadline,'Mounted native UI did not settle');await tick();}};
  await until(()=>node('#content').innerHTML.includes('data-osa-plot'));
  return{calls,ui,session,publish,until,html:()=>node('#content').innerHTML,input:id=>document.getElementById(id),content:()=>node('#content'),
   changeContext(){state.domains[`${domain.kind}:${domain.id}`].context={...context,connection_id:'7'.repeat(32),epoch:2};publish();},
   edit(id,value){const input=document.getElementById(id);input.value=value;for(const fn of listeners.get('#contentinput')||[])fn({target:input});return input;},
   click:op=>{for(const fn of listeners.get('#contentclick')||[])fn({target:{closest:s=>s==='button'?{disabled:false,dataset:{op}}:null}});},
   navigate(hash){location.hash=hash;windowEvents.get('hashchange')?.();},restore(){for(const[k,v]of prior)if(v.has)globalThis[k]=v.value;else delete globalThis[k];}};
 }catch(e){for(const[k,v]of prior)if(v.has)globalThis[k]=v.value;else delete globalThis[k];throw e;}
}
