// Production UI with a finite, explicitly injected intent sink. No device access.
// Three typed controls and one bounded limits/reconnect flow; no hardware access.
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
location.hash='#/host/'+h+'/device/'+d;
const context={session_id:'f'.repeat(32),domain,connection_id:'3'.repeat(32),epoch:1};
const device={connected:true,state:'READY',target_following_enabled:false,sample_age_s:0.1,wavelength_range_nm:[1045,1085],operating_range_nm:[1059,1062],max_scan_speed_nm_s:10,operating_max_speed_nm_s:1,identity:{serial:'22500001',firmware:'2.4',head_model:'6722-P',head_serial:'0953'},laser:{wavelength_nm:1061.808,wavelength_setpoint_nm:1061.808,piezo_percent:50,power_mw:0,current_ma:0,output_enabled:false,remote:false,tracking:false,constant_power:false,operation_complete:true,status_byte:0}};
const state={host_id:h,host_name:'Offline laser UI fixture',mode:'real',registry:{registry_rev:1,settings:{},devices:[{device_id:d,name:'Bench laser',model_id:'tlb6700',profile_id:'newport-usb',params:{device_key:'6700 SN22500001'},config_rev:1}],drafts:[],setups:[]},control:{['device:'+d]:{state:'CONTROLLED',controller_session:s,control_epoch:0}},domains:{['device:'+d]:{state:'READY',device,context,host_sample_ms:0}}};
let seq=0,subscriber,ui,pings=0,completeRead,executions=0,saves=0,stops=0,trackingWrites=0,followingWrites=0,singleWrites=0;
function publish(){if(++pings>100)throw new Error('Fixture budget exceeded');state.domains['device:'+d].host_sample_ms=performance.now();subscriber({type:'snapshot',host_id:h,boot_id:b,seq:++seq,data:state});}
const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
const client={preferences:async()=>({pythonPath:'VISA'}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},catalog:async()=>({models:[{id:'tlb6700',name:'TLB-6700',profiles:[{id:'newport-usb',open_effects:[]}]}],categories:['Laser']}),subscribe:async fn=>{subscriber=fn;return ()=>{}},requestSnapshot:async()=>{publish();return {seq}},ping:async()=>({monotonic_ms:performance.now(),client_session_id:s,boot_id:b})};
client.renew=async()=>({expires_in_ms:10000});
client.safeStop=async()=>{if(++stops>1)throw new Error('Stop budget exceeded');context.connection_id=null;context.epoch=2;Object.assign(state.domains['device:'+d],releasedDomainWithCachedSample(context,device));state.control['device:'+d]={state:'AVAILABLE',control_epoch:1};return {accepted:true}};
client.saveLaserLimits=async args=>{if(++saves>1||args.config_rev!==1||args.expected_rev!==1||JSON.stringify(args.limits)!==JSON.stringify({min_nm:1060,max_nm:1061,max_speed_nm_s:.5}))throw new Error('Unexpected limits');const record=state.registry.devices[0];record.config_rev=2;record.params={...record.params,operating_min_nm:1060,operating_max_nm:1061,scan_speed_limit_nm_s:.5};state.registry.registry_rev=2;device.operating_range_nm=[1060,1061];device.operating_max_speed_nm_s=.5;return state.registry};
client.acquire=async()=>{state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:1};return {...lease,control_epoch:1}};
client.nextSequence=()=>executions+1;client.prepare=async()=>({token:'proof'});
client.execute=async(id,intent)=>{
 if(intent.method==='action'&&['scan_forward','scan_backward'].includes(intent.params.name)){
  const expected=singleWrites===0?{name:'scan_forward',args:{target_nm:1061,speed_nm_s:.5,confirm:true}}:{name:'scan_backward',args:{target_nm:1060,speed_nm_s:.5,confirm:true}};
  if(++singleWrites>2||!device.single_scan_supported||JSON.stringify(intent.params)!==JSON.stringify(expected))throw Error('Unexpected one-way intent');
  await new Promise(yes=>completeRead=yes);device.laser.wavelength_nm=intent.params.args.target_nm;device.laser.wavelength_setpoint_nm=intent.params.args.target_nm;device.laser.operation_complete=true;device.motion_pending=true;
  return {request_id:id,operation_id:'2'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};
 }
 if(intent.method==='action'&&intent.params.name==='control_tracking'){
  if(++trackingWrites>2||JSON.stringify(intent.params.args)!==JSON.stringify({enabled:trackingWrites===1,confirm:true}))throw Error('Unexpected Tracking toggle');
  await new Promise(yes=>completeRead=yes);device.target_following_enabled=intent.params.args.enabled;device.laser.tracking=intent.params.args.enabled;device.motion=null;
  return {request_id:id,operation_id:'9'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};
 }
 if(executions===4&&intent.method==='action'&&intent.params.name==='set_target_wavelength'){
  if(++followingWrites>1||!device.laser.tracking||JSON.stringify(intent.params.args)!==JSON.stringify({wavelength_nm:1060.503,confirm:true}))throw Error('Unexpected following target');
  await new Promise(yes=>completeRead=yes);device.laser.wavelength_nm=1060.503;device.laser.wavelength_setpoint_nm=1060.503;device.laser.tracking=false;device.motion_pending=true;
  return {request_id:id,operation_id:'a'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};
 }
 if(intent.method==='action'&&intent.params.name==='read_motion'){
  if(Object.keys(intent.params.args).length)throw Error('Motion read takes no arguments');
  device.motion={wavelength_nm:device.laser.wavelength_nm,wavelength_setpoint_nm:device.laser.wavelength_setpoint_nm,tracking:device.laser.tracking,operation_complete:device.laser.operation_complete};device.motion_pending=false;
  return {request_id:id,operation_id:'8'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};
 }
 if(executions===5){executions++;if(intent.method!=='action'||intent.params.name!=='read_status'||Object.keys(intent.params.args).length)throw new Error('Only one automatic read allowed');await new Promise(yes=>completeRead=yes);device.sample_age_s=.1;return {request_id:id,operation_id:'7'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};}
 if(executions===4){executions++;if(intent.method!=='connect'||intent.config_rev!==2)throw new Error('Reconnect must use saved revision');context.connection_id='5'.repeat(32);context.epoch=3;state.domains['device:'+d].context={...context};state.domains['device:'+d].state='READY';state.domains['device:'+d].device=device;device.sample_age_s=.1;return {request_id:id,operation_id:'6'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};}
 const expected=[{name:'start_scan' ,args:{start_nm:1060,stop_nm:1061,speed_nm_s:.5,return_speed_nm_s:.8,confirm:true}},
 {name:'stop_scan',args:{confirm:true}},{name:'set_target_wavelength',args:{wavelength_nm:1060.5,confirm:true}},{name:'set_target_wavelength',args:{wavelength_nm:1060.502,confirm:true}}][executions++];
 if(!expected||intent.method!=='action'||JSON.stringify(intent.params)!==JSON.stringify(expected))throw new Error('Unreviewed fixture intent: '+JSON.stringify(intent.params));
 await new Promise(yes=>completeRead=yes);
 if(expected.name==='start_scan')device.laser.operation_complete=false;
 else device.laser.operation_complete=true;
 if(expected.name==='set_target_wavelength'){if(device.laser.tracking)device.laser.wavelength_nm=expected.args.wavelength_nm;device.laser.wavelength_setpoint_nm=expected.args.wavelength_nm;}
 device.motion_pending=true;
 device.sample_age_s=.1;
 return {request_id:id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};
};
const session=createConsoleSession(client,()=>ui?.requestRender()),store=session.store;
ui=mountConsole(session,{event:{listen(){}}});await ui.ready;store.setLease(key,lease);ui.render();
window.fixture={ui,device,publish,finish(){completeRead()},executions:()=>executions,singles:()=>singleWrites,tracking:()=>trackingWrites,following:()=>followingWrites,saves:()=>saves,flush(){ui.render()},ready:true};
</script>`;
const server=createServer(async(req,res)=>{try{
 if(req.url==='/'){res.setHeader('Content-Type','text/html');res.end(html);return;}
 const path=resolve(root,'.'+new URL(req.url,'http://localhost').pathname);
 if(!path.startsWith(root+sep)||!path.startsWith(resolve(root,'App/web')+sep)){res.writeHead(404).end();return;}
 res.setHeader('Content-Type',path.endsWith('.js')?'text/javascript':'text/css');res.end(await readFile(path));
}catch{res.writeHead(404).end();}});
await new Promise(yes=>server.listen(0,'127.0.0.1',yes));
let browser;
try{
 browser=await chromium.launch({executablePath:process.env.YANG_LAB_BROWSER,headless:true,timeout:15000});
 const page=await browser.newPage({viewport:{width:1440,height:900}}),errors=[];
 page.setDefaultTimeout(10000);page.on('pageerror',e=>errors.push(e.message));
 await page.goto('http://127.0.0.1:'+server.address().port);
 try{await page.waitForFunction(()=>window.fixture?.ready);}catch(cause){console.log(JSON.stringify({errors,notice:await page.locator('#notice').textContent()}));throw cause;}
 const status=page.locator('#laser-readings-card'),control=page.locator('#laser-control-card');
 assert.equal(await status.getByRole('heading',{name:'Laser Status'}).isVisible(),true);
 assert.equal(await control.getByRole('button',{name:'Laser Enable',exact:true}).isEnabled(),true);
 assert.equal(await control.getByRole('button',{name:'Laser Disable',exact:true}).count(),0);
 assert.equal(await control.getByText('Fine cavity tuning, 0–100%. This is not a wavelength in nm.',{exact:true}).isVisible(),false);
 assert.equal(await control.getByRole('button',{name:'Full Scan',exact:true}).isVisible(),true);
 for(const name of ['Forward Scan','Backward Scan'])assert.equal(await control.getByRole('button',{name,exact:true}).isDisabled(),true,'unqualified controller is gated');
 assert.equal(await control.locator('[data-scan-shortcut-hint="laser-scan-stop"]').textContent(),'Esc');
 await control.locator('#laser-scan-shortcuts > summary').click();
 const stopBinding=page.locator('[data-scan-shortcut-capture="laser-scan-stop"]');
 await stopBinding.focus();await stopBinding.press('Tab');assert.notEqual(await page.evaluate(()=>document.activeElement?.id),'shortcut-laser-scan-stop','Tab leaves a shortcut binding for normal keyboard navigation');
 await stopBinding.focus();await stopBinding.press('F8');assert.equal(await stopBinding.inputValue(),'F8');
 assert.equal(await control.locator('[data-scan-shortcut-hint="laser-scan-stop"]').textContent(),'F8');
 await control.locator('[data-scan-shortcuts-reset]').click();assert.equal(await stopBinding.inputValue(),'Escape');
 await control.locator('#laser-scan-shortcuts > summary').click();
 const size=await status.evaluate(e=>e.getBoundingClientRect().height);
 assert.ok(size<270,'status is compact: '+size);
 assert.ok((await control.evaluate(e=>e.getBoundingClientRect().height))>size,'Control gets more space');
 for(const id of ['laser-wavelength','laser-scan-start','laser-scan-stop','laser-scan-speed','laser-scan-return-speed']){
  const input=page.locator('#'+id),text=await input.inputValue();
  for(const position of [...text].map((value,index)=>/\d/.test(value)?index:null).filter(value=>value!==null)){
   await control.getByRole('heading',{name:'Control',exact:true}).click();
   const point=await input.evaluate((e,index)=>{const css=getComputedStyle(e),canvas=document.createElement('canvas'),ctx=canvas.getContext('2d');ctx.font=css.font;const advance=ctx.measureText('0').width+(parseFloat(css.letterSpacing)||0);return{x:e.clientLeft+parseFloat(css.paddingLeft)+(index+.75)*advance-e.scrollLeft,y:e.clientHeight/2};},position);
   await input.click({position:point});assert.deepEqual(await input.evaluate(e=>[e.selectionStart,e.selectionEnd]),[position,position+1],id+' first click at digit '+position);
  }
 }
 assert.equal(await page.locator('#laser-scan-stop').inputValue(),'1062.000','Stop is initialized to the operating maximum');
 assert.equal(await page.evaluate(()=>fixture.executions()),0,'click selection and defaults never send hardware intents');
 if(process.env.YANG_LAB_UI_EVIDENCE){await mkdir(process.env.YANG_LAB_UI_EVIDENCE,{recursive:true});await control.getByRole('heading',{name:'Control',exact:true}).click();await control.screenshot({path:resolve(process.env.YANG_LAB_UI_EVIDENCE,'laser-control-preview.png')});}
 const scanStart=page.locator('#laser-scan-start');await scanStart.focus();await scanStart.evaluate(e=>e.setSelectionRange(0,1));
 for(const [digit,next] of [['1',1],['0',2],['6',3],['1',5],['8',6],['0',7],['8',7]]){
  await scanStart.press(digit);assert.deepEqual(await scanStart.evaluate(e=>[e.selectionStart,e.selectionEnd]),[next,next+1],'digit input advances to the next digit');
 }
 assert.equal(await scanStart.inputValue(),'1061.808');
 await page.locator('#laser-scan-speed').focus();await page.locator('#laser-scan-speed').press('ArrowDown');assert.equal(await page.locator('#laser-scan-speed').inputValue(),'00.99','untouched default adjusts the least significant digit');await page.locator('#laser-scan-speed').press('ArrowUp');
 await page.locator('#laser-scan-start').focus();await page.locator('#laser-scan-start').press('ArrowUp');assert.equal(await page.locator('#laser-scan-start').inputValue(),'1061.809');
 await page.locator('#laser-scan-start').fill('1060');await page.locator('#laser-scan-stop').fill('1061');await page.locator('#laser-scan-speed').fill('0.5');await page.locator('#laser-scan-return-speed').fill('0.8');
 await page.locator('#laser-scan-speed').focus();await page.locator('#laser-scan-speed').press('ArrowUp');assert.equal(await page.locator('#laser-scan-speed').inputValue(),'00.51');await page.locator('#laser-scan-speed').press('ArrowDown');
 await page.locator('#laser-scan-return-speed').focus();await page.locator('#laser-scan-return-speed').press('ArrowLeft');await page.locator('#laser-scan-return-speed').press('ArrowUp');assert.equal(await page.locator('#laser-scan-return-speed').inputValue(),'00.90');await page.locator('#laser-scan-return-speed').press('ArrowDown');
 await page.locator('#laser-scan-stop').focus();await page.locator('#laser-scan-stop').press('ArrowUp');assert.equal(await page.locator('#laser-scan-stop').inputValue(),'1061.001');await page.locator('#laser-scan-stop').press('ArrowDown');
 assert.equal(await page.evaluate(()=>fixture.executions()),0,'scan drafts do not communicate until Start');
 const columns=await page.evaluate(()=>['.laser-manual','.laser-scan'].map(s=>{const r=document.querySelector(s).getBoundingClientRect();return {x:r.x,y:r.y}}));assert.ok(columns[0].x<columns[1].x);assert.equal(columns[0].y,columns[1].y);
 const layout=await page.evaluate(()=>{const box=s=>{const r=document.querySelector(s).getBoundingClientRect();return {y:r.y,width:r.width,bottom:r.bottom};};return {left:box('.laser-manual'),right:box('.laser-scan'),manual:box('.laser-manual-actions .btn'),scan:box('.laser-scan .form-actions .btn')};});
 assert.ok(layout.left.width<layout.right.width,'scanning receives more width');
 assert.ok(Math.abs(layout.manual.bottom-layout.scan.bottom)<1,'primary actions align at the bottom');
 await page.evaluate(()=>{fixture.publish();fixture.flush()});
 assert.equal(await page.locator('#laser-scan-stop').inputValue(),'1061.000','telemetry preserves editing');
 if(process.env.YANG_LAB_UI_EVIDENCE){await mkdir(process.env.YANG_LAB_UI_EVIDENCE,{recursive:true});await page.screenshot({path:resolve(process.env.YANG_LAB_UI_EVIDENCE,'laser-control.png'),fullPage:true});}
 await control.getByRole('button',{name:'Full Scan',exact:true}).click();
 await page.waitForFunction(()=>fixture.executions()===1);
 const start=control.locator('[data-op="laser-scan-start"]');
 assert.equal(await start.getAttribute('aria-busy'),'true');assert.match(await start.textContent(),/Starting scan/);
 await start.evaluate(e=>e.click());assert.equal(await page.evaluate(()=>fixture.executions()),1);
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 assert.equal(await control.locator('[data-op="laser-scan-stop"]').isEnabled(),true);
 assert.equal(await start.isDisabled(),true,'busy native motor blocks another scan');assert.equal(await page.locator('#laser-wavelength').isDisabled(),true,'scan greys target');
 await control.locator('#laser-scan-shortcuts > summary').click();
 await page.locator('[data-scan-shortcuts-enabled]').uncheck();
 await control.getByRole('heading',{name:'Control',exact:true}).click();await page.keyboard.press('Escape');assert.equal(await page.evaluate(()=>fixture.executions()),1,'disabled shortcut never sends Stop');
 await page.locator('[data-scan-shortcuts-enabled]').check();
 await control.getByRole('heading',{name:'Control',exact:true}).click();await page.keyboard.press('Escape');await page.waitForFunction(()=>fixture.executions()===2);
 assert.match(await control.locator('[data-op="laser-scan-stop"]').textContent(),/Stopping scan/);
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 await control.locator('#laser-scan-shortcuts > summary').click();
 const target=page.locator('#laser-wavelength');assert.equal(await target.inputValue(),'1061.808','stop position becomes target');
 await target.focus();await target.evaluate(e=>e.setSelectionRange(0,1));await target.pressSequentially('1060500');
 await page.waitForFunction(()=>fixture.executions()===3);assert.equal(await target.isEnabled(),true,'editing continues during one target request');
 await target.press('ArrowUp');await target.press('ArrowUp');await page.evaluate(()=>fixture.finish());
 await page.waitForFunction(()=>fixture.executions()===4);await page.evaluate(()=>fixture.finish());
 await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 assert.equal(await status.getByText('1061.808 nm',{exact:true}).isVisible(),true,'Tracking Off stores target without moving actual wavelength');
 assert.equal(await target.inputValue(),'1060.502');
 assert.equal(await page.locator('.target-activity').count(),0,'no updating indicator while editing target');
 const tracking=control.locator('[data-op="laser-tracking-on"]');assert.equal(await tracking.textContent(),'Tracking Off');
 await tracking.click();await page.waitForFunction(()=>fixture.tracking()===1);await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>document.querySelector('[data-op="laser-tracking-off"]')?.textContent==='Tracking On');
 const active=control.locator('[data-op="laser-tracking-off"]');assert.equal(await active.getAttribute('aria-pressed'),'true');assert.equal(await active.evaluate(e=>getComputedStyle(e).backgroundColor),'rgb(8, 127, 131)');
 await target.focus();await target.press('ArrowUp');await page.waitForFunction(()=>fixture.following()===1);assert.equal(await target.inputValue(),'1060.503');assert.equal(await target.isEnabled(),true);
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>document.querySelector('#laser-readings-card').textContent.includes('1060.503 nm'));
 assert.equal(await active.textContent(),'Tracking On','software following stays on when motor returns Ready');
 await active.click();await page.waitForFunction(()=>fixture.tracking()===2);await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>document.querySelector('[data-op="laser-tracking-on"]')?.textContent==='Tracking Off');
 await page.locator('#laser-operating-limits > summary').click();
 await page.locator('#laser-limit-min').fill('1060');await page.locator('#laser-limit-max').fill('1061');await page.locator('#laser-limit-speed').fill('.5');
 await control.getByRole('button',{name:'Save & reconnect',exact:true}).click();

 try {await page.waitForFunction(()=>fixture.executions()===5&&!document.querySelector('#laser-control-card [aria-busy="true"]'));}
 catch(cause){console.log(await page.locator('#notice').textContent());throw cause;}
 assert.equal(await page.evaluate(()=>fixture.saves()),1,'one limits save');
 assert.equal(await page.locator('#laser-scan-start').getAttribute('min'),'1060');
 assert.equal(await page.locator('#laser-scan-stop').getAttribute('max'),'1061');
 assert.equal(await page.locator('#laser-scan-speed').getAttribute('max'),'0.5');
 assert.equal(await page.locator('#laser-scan-stop').inputValue(),'1061.000','limits change resets the scan draft to bounded defaults');
 assert.equal(await page.locator('#notice').isVisible(),false,'no reconnect error');
 await page.evaluate(()=>{fixture.device.sample_age_s=31;fixture.publish();fixture.flush()});
 await page.waitForFunction(()=>fixture.executions()===6);
 assert.match(await status.locator('[data-op="laser-read"]').textContent(),/Refreshing/);
 assert.equal(await status.locator('[data-op="laser-read"]').getAttribute('aria-busy'),'true');
 await status.locator('[data-op="laser-read"]').evaluate(e=>e.click());
 assert.equal(await page.evaluate(()=>fixture.executions()),6,'automatic refresh suppresses duplicate clicks');
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>!document.querySelector('#laser-readings-card [aria-busy="true"]'));
 // Development-only fixture explicitly supplies a modeled qualified capability.
 // This exercises UI intents without qualifying any real controller or firmware.
 await page.evaluate(()=>{fixture.device.single_scan_supported=true;fixture.publish();fixture.flush()});
 await page.locator('#laser-scan-start').fill('1060');await page.locator('#laser-scan-stop').fill('1061');
 await page.locator('#laser-scan-speed').fill('.5');await page.locator('#laser-scan-return-speed').fill('.5');
 await control.locator('#laser-scan-shortcuts > summary').click();
 await page.locator('[data-scan-shortcut-capture="laser-scan-forward"]').press('F6');
 await page.locator('[data-scan-shortcut-capture="laser-scan-stop"]').press('F8');
 assert.equal(await page.evaluate(()=>fixture.singles()),0,'capturing bindings never moves a laser');
 await control.getByRole('heading',{name:'Control',exact:true}).click();await page.keyboard.press('F6');
 await page.waitForFunction(()=>fixture.singles()===1);await page.keyboard.press('F6');assert.equal(await page.evaluate(()=>fixture.singles()),1,'held or conflicting action never duplicates motion');
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 await control.getByRole('button',{name:'Backward Scan',exact:true}).click();await page.waitForFunction(()=>fixture.singles()===2);
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 await control.locator('#laser-scan-shortcuts > summary').click();
 for(const width of [800,960,1440]){
  await page.setViewportSize({width,height:1000});
  assert.equal(await control.evaluate(e=>e.scrollWidth<=e.clientWidth+1),true,'controls fit at '+width+' px');
  if(process.env.YANG_LAB_UI_EVIDENCE)await page.screenshot({path:resolve(process.env.YANG_LAB_UI_EVIDENCE,'laser-control-'+width+'.png'),fullPage:true});
 }
 await page.reload();await page.waitForFunction(()=>window.fixture?.ready);
 assert.equal(await page.locator('[data-scan-shortcut-hint="laser-scan-stop"]').textContent(),'F8','binding survives app reload');
 assert.equal(await page.locator('[data-scan-shortcut-hint="laser-scan-forward"]').textContent(),'F6');
 await page.keyboard.press('F6');assert.equal(await page.evaluate(()=>fixture.singles()),0,'unqualified reloaded capability cannot dispatch a motion shortcut');
 await page.evaluate(()=>localStorage.setItem('yanglab.scan-shortcuts.v1','{"version":99}'));await page.reload();await page.waitForFunction(()=>window.fixture?.ready);
 assert.equal(await page.locator('#laser-scan-shortcuts').evaluate(e=>e.open),false);
 assert.equal(await page.locator('[data-scan-shortcut-error]').isVisible(),true,'invalid saved configuration explains why shortcuts are off without opening settings');
 assert.match(await page.locator('[data-scan-shortcut-error]').textContent(),/Could not read saved/);
 assert.deepEqual(errors,[]);
 console.log('PASS: compact status, large Control, single output button, preserved inputs, keyboard target coalescing, independent velocities, busy Stop, saved limits/reconnect, visible automatic refresh and 800/960/1440 px layout');
}finally{await browser?.close();server.closeAllConnections();await new Promise(yes=>server.close(yes));}
