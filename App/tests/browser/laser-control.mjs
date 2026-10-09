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
if(new URL(location.href).searchParams.has('imported')){
 device.target_following_enabled=true;device.move=null;device.motion_pending=false;
 Object.assign(device.laser,{operation_complete:false,tracking:true});
 device.motion={operation_complete:false,tracking:true,wavelength_nm:1061.808,wavelength_setpoint_nm:1061.808};device.motion_age_s=.1;
}
const state={host_id:h,host_name:'Offline laser UI fixture',mode:'real',registry:{registry_rev:1,settings:{},devices:[{device_id:d,name:'Bench laser',model_id:'tlb6700',profile_id:'newport-usb',params:{device_key:'6700 SN22500001'},config_rev:1}],drafts:[],setups:[]},control:{['device:'+d]:{state:'CONTROLLED',controller_session:s,control_epoch:0}},domains:{['device:'+d]:{state:'READY',device,context,host_sample_ms:0}}};
let seq=0,subscriber,ui,pings=0,completeRead,executions=0,saves=0,stops=0,trackingWrites=0,followingWrites=0,singleWrites=0,wireBusy=false,intents=[],unknownNext=false;
function publish(){if(++pings>100)throw new Error('Fixture budget exceeded');state.domains['device:'+d].host_sample_ms=performance.now();subscriber({type:'snapshot',host_id:h,boot_id:b,seq:++seq,data:state});}
const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
const client={preferences:async()=>({pythonPath:'VISA'}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},catalog:async()=>({models:[{id:'tlb6700',name:'TLB-6700',profiles:[{id:'newport-usb',open_effects:[]}]}],categories:['Laser']}),subscribe:async fn=>{subscriber=fn;return ()=>{}},requestSnapshot:async()=>{publish();return {seq}},ping:async()=>({monotonic_ms:performance.now(),client_session_id:s,boot_id:b})};
client.renew=async()=>({expires_in_ms:10000});
client.safeStop=async()=>{if(++stops>1)throw new Error('Stop budget exceeded');context.connection_id=null;context.epoch=2;Object.assign(state.domains['device:'+d],releasedDomainWithCachedSample(context,device));state.control['device:'+d]={state:'AVAILABLE',control_epoch:1};return {accepted:true}};
client.saveLaserLimits=async args=>{if(++saves>1||args.config_rev!==1||args.expected_rev!==1||JSON.stringify(args.limits)!==JSON.stringify({min_nm:1060,max_nm:1061,max_speed_nm_s:.5}))throw new Error('Unexpected limits');const record=state.registry.devices[0];record.config_rev=2;record.params={...record.params,operating_min_nm:1060,operating_max_nm:1061,scan_speed_limit_nm_s:.5};state.registry.registry_rev=2;device.operating_range_nm=[1060,1061];device.operating_max_speed_nm_s=.5;return state.registry};
client.acquire=async()=>{state.control['device:'+d]={state:'CONTROLLED',controller_session:s,control_epoch:1};return {...lease,control_epoch:1}};
client.nextSequence=()=>executions+1;client.prepare=async()=>({token:'proof'});
client.execute=async(id,intent)=>{
 if(wireBusy)throw Error('Concurrent native exchange');wireBusy=true;
 if(++executions>12)throw Error('Finite intent budget exceeded');intents.push(structuredClone(intent.params));
 try {
  if(intent.method==='connect'){
   if(intent.config_rev!==2||saves!==1)throw Error('Unreviewed connect');
   context.connection_id='5'.repeat(32);context.epoch=3;Object.assign(state.domains['device:'+d],{context:{...context},state:'READY',device});
  }else {
   const a=intent.params,args=a.args;
   if(!['start_scan','stop_scan','goto_wavelength','control_output','read_status','scan_forward','scan_backward'].includes(a.name))throw Error('Unreviewed intent '+a.name);
   if(a.name==='start_scan'&&JSON.stringify(args)!==JSON.stringify({start_nm:1060,stop_nm:1061,speed_nm_s:.5,return_speed_nm_s:.8,confirm:true}))throw Error('Scan arguments');
   if(a.name==='goto_wavelength'&&(![1060.502,1060.503].includes(args.wavelength_nm)||args.confirm!==true))throw Error('Goto arguments');
   if(['scan_forward','scan_backward'].includes(a.name)){
    if(++singleWrites>2||!device.single_scan_supported||args.target_nm!==(a.name==='scan_forward'?1061:1060)||args.speed_nm_s!==.5||args.confirm!==true)throw Error('Single-pass arguments');
   }
   await new Promise(yes=>completeRead=yes);
   if(unknownNext){unknownNext=false;return {request_id:id,operation_id:executions.toString(16).padStart(32,'0'),domain,status:'Outcome Unknown',phase:'timed_out_unknown',result:{context}};}
   if(a.name==='start_scan'){device.laser.operation_complete=false;device.move={kind:'full_scan',phase:'moving',message:'Scanning one round trip.',elapsed_s:0};}
   if(a.name==='goto_wavelength'){device.laser.operation_complete=false;device.laser.tracking=true;device.laser.wavelength_setpoint_nm=args.wavelength_nm;device.move={kind:'goto',phase:'moving',target_nm:args.wavelength_nm,message:'Moving to target.',elapsed_s:0};}
   if(a.name==='stop_scan'){device.laser.operation_complete=true;device.laser.tracking=false;device.move={kind:'stop',phase:'stopped',message:'Stopped; current position held.'};}
   if(a.name==='control_output')device.laser.output_enabled=args.enabled;
   if(['scan_forward','scan_backward'].includes(a.name)){device.laser.wavelength_nm=args.target_nm;device.laser.wavelength_setpoint_nm=args.target_nm;device.laser.operation_complete=true;device.laser.tracking=false;device.move={kind:'single_scan',phase:'arrived',message:'Target reached; tracking off.'};}
  }
  // This finite sink publishes explicit modeled observations, not physical
  // completion inferred from an ACK. Real completion is tested in Rust.
  device.motion_pending=false;device.motion=null;device.sample_age_s=.1;
  return {request_id:id,operation_id:executions.toString(16).padStart(32,'0'),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};
 } finally {wireBusy=false;}
};
const session=createConsoleSession(client,()=>ui?.requestRender()),store=session.store;
ui=mountConsole(session,{event:{listen(){}}});await ui.ready;store.setLease(key,lease);ui.render();
window.fixture={ui,device,publish,intents:()=>intents,changeContext(){context.epoch++;context.connection_id='6'.repeat(32);publish();ui.render()},unknownNext(){unknownNext=true},arrive(){device.laser.wavelength_nm=device.laser.wavelength_setpoint_nm;device.laser.operation_complete=true;device.laser.tracking=false;device.move={...device.move,phase:'arrived',message:'Target reached; tracking off.'};publish();ui.render();},finish(){if(!completeRead)throw Error('No pending exchange');const done=completeRead;completeRead=null;done()},executions:()=>executions,singles:()=>singleWrites,tracking:()=>trackingWrites,following:()=>followingWrites,saves:()=>saves,flush(){ui.render()},ready:true};
// Explicit modeled hold observations; this UI sink never infers physical hold
// from the accepted Goto. Native finite transport tests cover that boundary.
window.fixture.hold=(complete,phase='holding')=>{device.laser.operation_complete=complete;device.laser.tracking=!complete;device.move={...device.move,phase,elapsed_s:4,message:'Verifying Tracking Off; waiting for controller hold.'};publish();ui.render();};
window.fixture.held=offTarget=>{device.laser.wavelength_nm=device.laser.wavelength_setpoint_nm+(offTarget?.04:.014);device.laser.operation_complete=true;device.laser.tracking=false;device.move={...device.move,phase:offTarget?'held_off_target':'arrived',message:offTarget?'Stopped; readback differs from target. Tracking is off.':'Move complete; tracking is off.'};publish();ui.render();};
</script>`;
const server=createServer(async(req,res)=>{try{
 if(new URL(req.url,'http://localhost').pathname==='/'){res.setHeader('Content-Type','text/html');res.end(html);return;}
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
 await page.locator('#laser-scan-speed').focus();await page.locator('#laser-scan-speed').press('ArrowDown');assert.equal(await page.locator('#laser-scan-speed').inputValue(),'00.09','untouched default adjusts the least significant digit');await page.locator('#laser-scan-speed').press('ArrowUp');
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
 assert.equal(await start.isDisabled(),true,'busy native motor blocks another scan');assert.equal(await page.locator('#laser-wavelength').isEnabled(),true,'scan preserves local draft editing');
 await control.locator('#laser-scan-shortcuts > summary').click();
 await page.locator('[data-scan-shortcuts-enabled]').uncheck();
 await control.getByRole('heading',{name:'Control',exact:true}).click();await page.keyboard.press('Escape');assert.equal(await page.evaluate(()=>fixture.executions()),1,'disabled shortcut never sends Stop');
 await page.locator('[data-scan-shortcuts-enabled]').check();
 await control.getByRole('heading',{name:'Control',exact:true}).click();await page.keyboard.press('Escape');await page.waitForFunction(()=>fixture.executions()===2);
 assert.match(await control.locator('[data-op="laser-scan-stop"]').textContent(),/Stopping scan/);
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 await control.locator('#laser-scan-shortcuts > summary').click();
 const target=page.locator('#laser-wavelength');assert.equal(await target.inputValue(),'1061.808','Stop position becomes the initial target');
 await target.focus();await target.evaluate(e=>e.setSelectionRange(0,1));await target.pressSequentially('1060500');await target.press('ArrowUp');await target.press('ArrowUp');
 assert.equal(await target.inputValue(),'1060.502');
 await page.waitForTimeout(250);assert.equal(await page.evaluate(()=>fixture.executions()),2,'editing and arrows never communicate');
 await page.evaluate(()=>{fixture.publish();fixture.flush()});assert.equal(await target.inputValue(),'1060.502','telemetry preserves the draft');
 await target.press('Enter');await page.waitForFunction(()=>fixture.executions()===3);
 await target.press('Enter');assert.equal(await page.evaluate(()=>fixture.executions()),3,'repeat Enter cannot duplicate the commit');
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 assert.equal(await control.locator('[data-op="laser-goto"]').isDisabled(),true,'owned move blocks only conflicting new movement');
 assert.equal(await control.locator('[data-op="laser-scan-stop"]').isEnabled(),true);
 await control.locator('[data-op="laser-output-on"]').click();await page.waitForFunction(()=>fixture.executions()===4);await page.evaluate(()=>fixture.finish());
 await page.waitForFunction(()=>document.querySelector('[data-op="laser-output-off"]')?.disabled===false);
 await control.locator('[data-op="laser-output-off"]').click();await page.waitForFunction(()=>fixture.executions()===5);await page.evaluate(()=>fixture.finish());
 await page.waitForFunction(()=>document.querySelector('[data-op="laser-output-on"]')?.disabled===false);
 assert.equal(await page.evaluate(()=>fixture.device.laser.operation_complete),false,'emission switching does not invent arrival or stop the motor');
 await page.evaluate(()=>fixture.arrive());await page.waitForFunction(()=>document.querySelector('[data-op="laser-goto"]')?.disabled===false);
 await target.focus();await target.press('ArrowUp');assert.equal(await target.inputValue(),'1060.503');
 await control.locator('[data-op="laser-goto"]').click();await page.waitForFunction(()=>fixture.executions()===6);
 assert.deepEqual(await page.evaluate(()=>fixture.intents().filter(x=>x.name==='goto_wavelength').map(x=>x.args)),[{wavelength_nm:1060.502,confirm:true},{wavelength_nm:1060.503,confirm:true}],'Enter and Goto use the same typed action');
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 await page.evaluate(()=>{fixture.device.sample_age_s=31;fixture.publish();fixture.flush()});await page.waitForFunction(()=>fixture.executions()===7);
 assert.equal(await target.isEnabled(),true,'ordinary read leaves drafting available');
 assert.equal(await control.locator('[data-op="laser-output-on"]').isEnabled(),true,'ordinary read leaves emission available');
 assert.equal(await control.locator('[data-op="laser-scan-stop"]').isEnabled(),true,'Stop can be requested during a delayed read');
 await target.focus();await target.press('ArrowUp');assert.equal(await target.inputValue(),'1060.504');
 await control.locator('[data-op="laser-scan-stop"]').click();assert.equal(await page.evaluate(()=>fixture.executions()),7,'Stop waits for the exact read; no overlapping exchange');
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>fixture.executions()===8);await page.evaluate(()=>fixture.finish());
 await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 assert.equal(await target.inputValue(),'1060.504','Stop and readback preserve a newer draft');
 assert.equal(await control.locator('[data-op="laser-goto"]').isEnabled(),true,'verified Stop restores movement');
 await page.locator('#laser-operating-limits > summary').click();
 await page.locator('#laser-limit-min').fill('1060');await page.locator('#laser-limit-max').fill('1061');await page.locator('#laser-limit-speed').fill('.5');
 await control.getByRole('button',{name:'Save & reconnect',exact:true}).click();

 try {await page.waitForFunction(()=>fixture.executions()===9&&!document.querySelector('#laser-control-card [aria-busy="true"]'));}
 catch(cause){console.log(await page.locator('#notice').textContent());throw cause;}
 assert.equal(await page.evaluate(()=>fixture.saves()),1,'one limits save');
 assert.equal(await page.locator('#laser-scan-start').getAttribute('min'),'1060');
 assert.equal(await page.locator('#laser-scan-stop').getAttribute('max'),'1061');
 assert.equal(await page.locator('#laser-scan-speed').getAttribute('max'),'0.5');
 assert.equal(await page.locator('#laser-scan-stop').inputValue(),'1061.000','limits change resets the scan draft to bounded defaults');
 assert.equal(await page.locator('#notice').isVisible(),false,'no reconnect error');
 await page.evaluate(()=>{fixture.device.sample_age_s=31;fixture.publish();fixture.flush()});
 await page.waitForFunction(()=>fixture.executions()===10);
 assert.match(await status.locator('[data-op="laser-read"]').textContent(),/Refreshing/);
 assert.equal(await status.locator('[data-op="laser-read"]').getAttribute('aria-busy'),'true');
 await status.locator('[data-op="laser-read"]').evaluate(e=>e.click());
 assert.equal(await page.evaluate(()=>fixture.executions()),10,'automatic refresh suppresses duplicate clicks');
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
 // A queued action belongs to the exact original connection, even if the
 // preceding read finishes successfully after a reconnect.
 await page.locator('[data-op="laser-read"]').click();await page.waitForFunction(()=>fixture.executions()===1);
 await control.locator('[data-op="laser-scan-stop"]').click();assert.equal(await page.evaluate(()=>fixture.executions()),1);
 await page.evaluate(()=>{fixture.changeContext();fixture.finish()});
 await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 assert.equal(await page.evaluate(()=>fixture.executions()),1,'changed connection prevents queued Stop from reaching the new connection');
 assert.match(await page.locator('#notice').textContent(),/authority changed/);
 await page.reload();await page.waitForFunction(()=>window.fixture?.ready);
 await target.focus();await target.evaluate(e=>e.setSelectionRange(0,1));await target.pressSequentially('1060502');
 await page.evaluate(()=>fixture.unknownNext());await target.press('Enter');await page.waitForFunction(()=>fixture.executions()===1);
 await page.evaluate(()=>fixture.finish());await page.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
 await target.press('Enter');await control.locator('[data-op="laser-goto"]').evaluate(e=>e.click());
 assert.equal(await page.evaluate(()=>fixture.executions()),1,'unknown setter is never automatically replayed or committed again');
 assert.equal(await control.locator('[data-op="laser-goto"]').isDisabled(),true);
 assert.equal(await target.inputValue(),'1060.502','uncertain outcome preserves the local draft');
 await page.evaluate(()=>localStorage.setItem('yanglab.scan-shortcuts.v1','{"version":99}'));await page.reload();await page.waitForFunction(()=>window.fixture?.ready);
 assert.equal(await page.locator('#laser-scan-shortcuts').evaluate(e=>e.open),false);
 assert.equal(await page.locator('[data-scan-shortcut-error]').isVisible(),true,'invalid saved configuration explains why shortcuts are off without opening settings');
 assert.match(await page.locator('[data-scan-shortcut-error]').textContent(),/Could not read saved/);
 // Preserving reconnect can return OPC=false and tracking already on without
 // any App-owned move. This must not strand the explicit Goto/Full controls.
 for(const full of [false,true]){
  const imported=await browser.newPage({viewport:{width:1440,height:1000}});
  imported.setDefaultTimeout(10000);imported.on('pageerror',e=>errors.push(e.message));
  try{
   await imported.goto('http://127.0.0.1:'+server.address().port+'/?imported=1');
   await imported.waitForFunction(()=>window.fixture?.ready);
   const card=imported.locator('#laser-control-card');
   assert.equal(await imported.evaluate(()=>fixture.executions()),0,'reconnect is passive and never clears tracking automatically');
   assert.equal(await card.locator('[data-op="laser-goto"]').isEnabled(),true,'inherited tracking permits an explicit Goto');
   assert.equal(await card.locator('[data-op="laser-scan-start"]').isEnabled(),true,'inherited tracking permits an explicit Full Scan');
   assert.equal(await card.locator('[data-op="laser-scan-stop"]').isEnabled(),true);
   assert.doesNotMatch(await card.locator('.laser-control-restriction').textContent(),/Movement is active/,'inherited tracking is not an App-owned move');
   if(full){
    for(const [id,value] of [['laser-scan-start','1060'],['laser-scan-stop','1061'],['laser-scan-speed','.5'],['laser-scan-return-speed','.8']])await imported.locator('#'+id).fill(value);
    await card.locator('[data-op="laser-scan-start"]').click();
   }else{
    const draft=imported.locator('#laser-wavelength');await draft.focus();await draft.evaluate(e=>e.setSelectionRange(0,1));await draft.pressSequentially('1060502');
    assert.equal(await imported.evaluate(()=>fixture.executions()),0,'digits remain local even with imported tracking');
    await draft.press('Enter');
   }
   await imported.waitForFunction(()=>fixture.executions()===1);
   assert.equal(await imported.evaluate(()=>fixture.intents()[0].name),full?'start_scan':'goto_wavelength');
   await imported.evaluate(()=>fixture.finish());await imported.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
   assert.equal(await card.locator('[data-op="laser-goto"]').isDisabled(),true,'an accepted owned move blocks overlapping new motion');
   assert.equal(await card.locator('[data-op="laser-scan-start"]').isDisabled(),true);
   assert.equal(await card.locator('[data-op="laser-scan-stop"]').isEnabled(),true);
  }finally{await imported.close();}
 }
 // Repeated commits preserve the next draft throughout the distinct holding
 // phase, then unlock from a confirmed, non-identical readback.
 const repeated=await browser.newPage({viewport:{width:1440,height:1000}});
 repeated.setDefaultTimeout(10000);repeated.on('pageerror',e=>errors.push(e.message));
 try{
  await repeated.goto('http://127.0.0.1:'+server.address().port);
  await repeated.waitForFunction(()=>window.fixture?.ready);
  const card=repeated.locator('#laser-control-card'),draft=repeated.locator('#laser-wavelength');
  for(let cycle=0;cycle<5;cycle++){
   const value=cycle%2?'1060503':'1060502';
   await draft.focus();await draft.evaluate(e=>e.setSelectionRange(0,1));await draft.pressSequentially(value);
   assert.equal(await repeated.evaluate(()=>fixture.executions()),cycle,'digits remain local on every cycle');
   await draft.press('Enter');await repeated.waitForFunction(n=>fixture.executions()===n,cycle+1);
   await repeated.evaluate(()=>fixture.finish());await repeated.waitForFunction(()=>!document.querySelector('#laser-control-card [aria-busy="true"]'));
   for(const complete of [false,true]){
    await repeated.evaluate(v=>fixture.hold(v),complete);
    assert.equal(await card.locator('[data-op="laser-goto"]').isDisabled(),true,'OPC alone never finishes the holding lifecycle');
    assert.equal(await card.locator('[data-op="laser-scan-start"]').isDisabled(),true);
    assert.equal(await card.locator('[data-op="laser-scan-stop"]').isEnabled(),true);
    assert.equal(await draft.isEnabled(),true);
   }
   await draft.focus();await draft.press('ArrowUp');const nextDraft=await draft.inputValue();
   await repeated.evaluate(off=>fixture.held(off),cycle===4);
   await repeated.waitForFunction(()=>document.querySelector('[data-op="laser-goto"]')?.disabled===false);
   assert.equal(await card.locator('[data-op="laser-scan-start"]').isEnabled(),true);
   assert.equal(await draft.inputValue(),nextDraft,'a held readback never replaces the next local draft');
   assert.equal(await repeated.evaluate(()=>fixture.executions()),cycle+1,'hold observations never replay Goto');
  }
  assert.match(await card.locator('.laser-move-status').textContent(),/readback differs from target/);
 }finally{await repeated.close();}
 assert.deepEqual(errors,[]);
 console.log('PASS: compact status, large Control, single output button, preserved inputs, draft-only Enter/Goto, five repeated Gotos with distinct hold verification and non-identical readback, independent emission, serialized Stop, context change rejects queued Stop, unknown setter never replays, passive reconnect with inherited tracking admits explicit Goto/Full, independent velocities, busy Stop, saved limits/reconnect, visible automatic refresh and 800/960/1440 px layout');
}finally{await browser?.close();server.closeAllConnections();await new Promise(yes=>server.close(yes));}
