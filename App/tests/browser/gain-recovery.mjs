// The production wizard and store run in Chromium with a finite typed Host double.
// No lab Host, native bridge, serial port, SDK or instrument transport is reachable.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFile,mkdir} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {resolve,sep} from 'node:path';
import {releasedDomainWithCachedSample} from '../released-domain-fixture.mjs';
const root=resolve(fileURLToPath(new URL('../../..',import.meta.url)));
const {chromium}=await import(pathToFileURL(process.env.YANG_LAB_PLAYWRIGHT_MODULE).href);
const html=`<!doctype html><link rel="stylesheet" href="/App/web/style.css">
<div class="app-shell"><aside class="sidebar"><nav id="navigation"></nav><span id="sidebar-mode"></span><span id="sidebar-subtitle"></span></aside><div class="main-shell"><header class="topbar"><span id="current-page-title"></span><span id="session-pill"></span></header><div id="background-work" hidden></div><main id="content" class="content"></main></div></div><div id="notice" hidden></div>
<script type="module">
import {mountConsole} from '/App/web/console-ui.js';
import {createConsoleSession} from '/App/web/main.js';
${releasedDomainWithCachedSample.toString()}
const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),s='d'.repeat(32),domain={kind:'device',id:d},key=h+'/device/'+d;
location.hash='#devices';
const mode=new URL(location.href).searchParams.get('mode')||'delayed',fault='Gain temperature deviation exceeds 3 degC';
const context={session_id:'f'.repeat(32),domain,connection_id:'3'.repeat(32),epoch:1};
const draft={device_id:d,revision:1,mode:'real',config_digest:'digest',model_id:'gain',profile_id:'cp210x-serial',params:{port:'COM4'},name:'Gain Chip Driver'};
const state={host_id:h,host_name:'Offline Gain fixture',mode:'real',registry:{registry_rev:1,settings:{},devices:[],drafts:[draft],setups:[]},
 control:{['device:'+d]:{state:'CONTROLLED',controller_session:s,control_epoch:0}},cleanup_attempts:[],
 domains:{['device:'+d]:{state:'READY',context,responsibility:true,pending:0,active_request_id:null,pending_request_id:null,safety_request_id:null,readback_request_id:null,
 device:{state:mode==='healthy'?'READY':'FAULT',connected:mode==='healthy',fault:mode==='healthy'?null:fault},host_sample_ms:0}}};
const profile={id:'cp210x-serial',access:'serial',interfaces:['Serial'],probe_mode:'supervised',open_effects:['DTR_RTS_reset_not_verified'],fields:{port:{kind:'serial',required:true}}};
const inventory={serial:[{resource:'COM4',vid:0x10c4,pid:0xea60,serial:'GAIN-A',instance_id:'USB\\\\VID_10C4&PID_EA60\\\\GAIN-A',description:'CP2102'}],usb_serial:{cp210x:{state:'ready',devices:[{instance_id:'USB\\\\VID_10C4&PID_EA60\\\\GAIN-A',driver_state:'ready'}]}}};
const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000},ticket={ticket_id:'7'.repeat(32),boot_id:b,domain,control_epoch:1,cause:'Released',baseline_invalidated:false};
let seq=0,subscriber,ui,finishRelease,syncs=0;const calls=[];
function publish(){state.domains['device:'+d].host_sample_ms=performance.now();subscriber({type:'snapshot',host_id:h,boot_id:b,seq:++seq,data:state});}
function complete(){
 const released={...context,connection_id:null,epoch:2},cleanup={attempt_id:'6'.repeat(32),unreleased:[],steps:[{role:'gain',action:'current_off_then_tec_off',error:null}]};
 state.cleanup_attempts=[[{attempt_id:'8'.repeat(32),ticket,error:null,resource_release_confirmed:true,result:{ok:true,phase:'completed',context:released,result:{connected:false,effective_intent:'disconnect',cleanup}}}]];
 state.domains['device:'+d]=releasedDomainWithCachedSample(released,state.domains['device:'+d].device);state.control['device:'+d]={state:'AVAILABLE',controller_session:null,control_epoch:1};publish();finishRelease?.();
}
const client={preferences:async()=>({}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},catalog:async()=>({categories:['Custom'],models:[{id:'gain',name:'Gain Chip Driver',category:'Custom',manufacturer:'Yang Lab',profiles:[profile]}]}),
 subscribe:async fn=>{subscriber=fn;return ()=>{};},requestSnapshot:async()=>{if(++syncs>250)throw Error('Finite snapshot budget exceeded');publish();return {seq};},ping:async()=>({monotonic_ms:performance.now(),client_session_id:s,boot_id:b}),driverStatus:async()=>inventory,
 release:async(selected,basis)=>{if(calls.includes('release')||basis.token!==lease.token||JSON.stringify(selected)!==JSON.stringify(domain))throw Error('Unexpected or replayed release');calls.push('release');state.control['device:'+d]={state:'RETAINED',controller_session:null,control_epoch:1};
  if(mode==='unknown')throw Object.assign(Error('Release reply unavailable'),{outcomeUnknown:true});if(mode==='delayed')await new Promise(resolve=>finishRelease=resolve);return {accepted:true,ticket};},
 acquire:async selected=>{if(state.control['device:'+d].state!=='AVAILABLE'||state.domains['device:'+d].responsibility!==false||state.domains['device:'+d].context.connection_id!==null)throw Error('Acquisition before confirmed release');calls.push('acquire');state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:1};return {...lease,token:'2'.repeat(32),control_epoch:1};},
 prepare:async intent=>{if(intent.method!=='connect'||intent.params.acknowledge_lifecycle!==true)throw Error('Unexpected connection');calls.push('prepare');return {token:'proof'};},
 execute:async(requestId,intent)=>{if(calls.includes('connect'))throw Error('Replayed connect');calls.push('connect');const next={...context,connection_id:'4'.repeat(32),epoch:3};state.domains['device:'+d]={state:'READY',context:next,responsibility:true,pending:0,device:{state:'READY',connected:true}};return {request_id:requestId,operation_id:'5'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context:next,result:{connected:true}}};},
 testConnection:async()=>{if(state.domains['device:'+d].device.connected!==true)throw Error('Verification of a faulted session');calls.push('verify');return {proof_id:'6'.repeat(32)};},
 cancelDraft:async()=>{if(state.domains['device:'+d].context.connection_id!==null||state.domains['device:'+d].responsibility!==false)throw Error('Cancelling retained configuration');calls.push('cancel');state.registry.drafts=[];publish();return {cancelled:true};},
 saveDevice:async()=>{calls.push('save');const record={...draft,config_rev:1};state.registry.devices=[record];state.registry.drafts=[];return record;},
 nextSequence:()=>calls.length+1,renew:async token=>({...lease,token,control_epoch:state.control['device:'+d].control_epoch})};
const session=createConsoleSession(client,()=>ui?.render());ui=mountConsole(session,{event:{listen(){}}});await ui.ready;session.store.setLease(key,lease);
window.fixture={ready:true,calls,state,complete,syncs:()=>syncs};
</script>`;
const server=createServer(async(req,res)=>{try{
 if(new URL(req.url,'http://localhost').pathname==='/'){res.setHeader('Content-Type','text/html');res.end(html);return;}
 const path=resolve(root,'.'+new URL(req.url,'http://localhost').pathname);
 if(!path.startsWith(resolve(root,'App/web')+sep)){res.writeHead(404).end();return;}
 res.setHeader('Content-Type',path.endsWith('.js')?'text/javascript':'text/css');res.end(await readFile(path));
}catch{res.writeHead(404).end();}});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const output=resolve(root,'Result/gain-failed/browser');await mkdir(output,{recursive:true});let browser;
try{
 browser=await chromium.launch({executablePath:process.env.YANG_LAB_BROWSER,headless:true,timeout:15000});
 async function open(mode){const page=await browser.newPage({viewport:{width:1280,height:1000}});page.setDefaultTimeout(15000);const errors=[];page.on('pageerror',error=>errors.push(error.message));await page.goto('http://127.0.0.1:'+server.address().port+'/?mode='+mode);await page.waitForFunction(()=>window.fixture?.ready);await page.locator('[data-ui="open-draft"]').click();await page.getByRole('button',{name:'Connect & verify',exact:true}).waitFor();return {page,errors};}
 const delayed=await open('delayed');
 await delayed.page.getByRole('button',{name:'Connect & verify',exact:true}).click();await delayed.page.waitForFunction(()=>fixture.calls.includes('release'));
 assert.equal(await delayed.page.locator('[data-ui="test-draft"]').isDisabled(),true);
 assert.match(await delayed.page.locator('.wizard').textContent(),/Gain temperature deviation exceeds 3 degC.*Releasing/s);
 assert.deepEqual(await delayed.page.evaluate(()=>fixture.calls),['release']);await delayed.page.screenshot({path:resolve(output,'release-pending.png')});
 await delayed.page.evaluate(()=>fixture.complete());await delayed.page.waitForFunction(()=>fixture.calls.includes('verify'));
 assert.deepEqual(await delayed.page.evaluate(()=>fixture.calls),['release','acquire','prepare','connect','verify']);
 assert.equal(await delayed.page.getByRole('button',{name:'Add & Save',exact:true}).isDisabled(),false);await delayed.page.screenshot({path:resolve(output,'verified.png')});
 await delayed.page.getByRole('button',{name:'Add & Save',exact:true}).click();await delayed.page.waitForFunction(()=>fixture.calls.includes('save'));assert.deepEqual(delayed.errors,[]);await delayed.page.close();
 console.log('PASS: delayed confirmed release keeps progress visible, then reacquires, connects, verifies and saves exactly once.');
 const cancelled=await open('delayed');await cancelled.page.getByRole('button',{name:'Connect & verify',exact:true}).click();await cancelled.page.waitForFunction(()=>fixture.calls.includes('release'));
 await cancelled.page.locator('[data-wizard-backdrop]').click({position:{x:10,y:10}});assert.deepEqual(await cancelled.page.evaluate(()=>fixture.calls),['release']);
 await cancelled.page.evaluate(()=>fixture.complete());await cancelled.page.waitForFunction(()=>fixture.calls.includes('cancel'));
 assert.deepEqual(await cancelled.page.evaluate(()=>fixture.calls),['release','cancel']);assert.equal(await cancelled.page.locator('.wizard').count(),0);assert.deepEqual(cancelled.errors,[]);await cancelled.page.close();
 console.log('PASS: Cancel during the delayed release waits for confirmation and retires the draft without reacquiring or reopening Gain.');
 const pending=await open('pending');await pending.page.getByRole('button',{name:'Connect & verify',exact:true}).click();await pending.page.locator('#notice').filter({hasText:'Connection release is unconfirmed'}).waitFor({state:'visible'});
 assert.match(await pending.page.locator('#notice').textContent(),/Gain temperature deviation exceeds 3 degC/);assert.deepEqual(await pending.page.evaluate(()=>fixture.calls),['release']);
 await pending.page.screenshot({path:resolve(output,'release-unconfirmed.png')});const syncs=await pending.page.evaluate(()=>fixture.syncs());await pending.page.getByRole('button',{name:'Connect & verify',exact:true}).click();await pending.page.waitForFunction(before=>fixture.syncs()>before,syncs);
 assert.deepEqual(await pending.page.evaluate(()=>fixture.calls),['release'],'repeat click queries the original release rather than replaying it');assert.equal(await pending.page.getByRole('button',{name:'Add & Save',exact:true}).isDisabled(),true);
 await pending.page.evaluate(()=>fixture.complete());await pending.page.waitForFunction(()=>fixture.calls.includes('verify'));assert.deepEqual(await pending.page.evaluate(()=>fixture.calls),['release','acquire','prepare','connect','verify']);assert.deepEqual(pending.errors,[]);await pending.page.close();
 console.log('PASS: uncertain release exposes the cached fault; a repeated click only queries its original ticket before confirmed recovery.');
 const unknown=await open('unknown');await unknown.page.getByRole('button',{name:'Connect & verify',exact:true}).click();await unknown.page.locator('#notice').filter({hasText:'Release reply unavailable'}).waitFor({state:'visible'});
 await unknown.page.getByRole('button',{name:'Connect & verify',exact:true}).click();await unknown.page.locator('#notice').filter({hasText:'Release acceptance is unknown'}).waitFor({state:'visible'});assert.deepEqual(await unknown.page.evaluate(()=>fixture.calls),['release']);assert.deepEqual(unknown.errors,[]);await unknown.page.close();
 console.log('PASS: a lost release acceptance cannot replay release, acquire authority or reconnect.');
 const healthy=await open('healthy');await healthy.page.getByRole('button',{name:'Connect & verify',exact:true}).click();await healthy.page.waitForFunction(()=>fixture.calls.includes('verify'));assert.deepEqual(await healthy.page.evaluate(()=>fixture.calls),['verify']);assert.deepEqual(healthy.errors,[]);await healthy.page.close();
 console.log('PASS: an existing healthy Gain session verifies without disconnecting or reconnecting.');
}finally{await browser?.close();server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}
