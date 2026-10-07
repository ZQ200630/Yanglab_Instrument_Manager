// Isolated UI acceptance: production console with one finite read-only transport.
// This fixture never opens an instrument or submits an output-setting command.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFile,mkdir} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {resolve,sep} from 'node:path';
const root=resolve(fileURLToPath(new URL('../../..',import.meta.url)));
const {chromium}=await import(pathToFileURL(process.env.YANG_LAB_PLAYWRIGHT_MODULE).href);
const html=`<!doctype html><link rel="stylesheet" href="/App/web/style.css">
<div class="app-shell"><aside class="sidebar"><nav id="navigation"></nav><span id="sidebar-mode"></span><span id="sidebar-subtitle"></span></aside><div class="main-shell"><header class="topbar"><span id="current-page-title"></span><span id="session-pill"></span></header><main id="content" class="content"></main></div></div><div id="notice" hidden></div>
<script type="module">
import {mountConsole} from '/App/web/console-ui.js';
import {createDeviceStore} from '/App/web/device-store.js';
const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),s='d'.repeat(32),domain={kind:'device',id:d},key=h+'/device/'+d;
location.hash='#/host/'+h+'/device/'+d;
const context={session_id:'f'.repeat(32),domain,connection_id:'3'.repeat(32),epoch:1};
const device={connected:true,state:'READY',sample_age_s:0.1,wavelength_range_nm:null,identity:{serial:'22500001',firmware:'2.4',head_model:'6722-P',head_serial:'0953'},laser:{wavelength_nm:1061.808,power_mw:0,current_ma:0,output_enabled:false,remote:false,tracking:false,constant_power:false,operation_complete:true,status_byte:0}};
const state={host_id:h,host_name:'Offline laser UI fixture',mode:'real',registry:{registry_rev:1,settings:{},devices:[{device_id:d,name:'Bench laser',model_id:'tlb6700',profile_id:'newport-usb',params:{device_key:'6700 SN22500001'},config_rev:1}],drafts:[],setups:[]},control:{['device:'+d]:{state:'CONTROLLED',controller_session:s,control_epoch:0}},domains:{['device:'+d]:{state:'READY',device,context,host_sample_ms:0}}};
const store=createDeviceStore();let seq=0,subscriber,ui,pings=0,completeRead,executions=0;
function publish(){if(++pings>100)throw new Error('Fixture budget exceeded');state.domains['device:'+d].host_sample_ms=performance.now();subscriber({type:'snapshot',host_id:h,boot_id:b,seq:++seq,data:state});}
const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
const client={preferences:async()=>({pythonPath:'VISA'}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},catalog:async()=>({models:[{id:'tlb6700',name:'TLB-6700',profiles:[{id:'newport-usb',open_effects:[]}]}],categories:['Laser']}),subscribe:async fn=>{subscriber=fn;return ()=>{}},requestSnapshot:async()=>{publish();return {seq}},ping:async()=>({monotonic_ms:performance.now(),client_session_id:s,boot_id:b})};
client.nextSequence=()=>executions+1;client.prepare=async()=>({token:'proof'});
client.execute=async(id,intent)=>{if(++executions>1||intent.method!=='action'||intent.params.name!=='read_status'||Object.keys(intent.params.args).length)throw new Error('Only one read-only refresh is permitted');await new Promise(yes=>completeRead=yes);device.laser.wavelength_nm=1061.809;device.sample_age_s=0.1;return {request_id:id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};};
const session={client,store,hostId:h,heartbeat:async()=>{},offline:()=>store.disconnected(h),navigate:()=>{},apply(event){const r=store.apply(event);(ui?.requestRender||ui?.render)?.();return r}};
ui=mountConsole(session,{event:{listen(){}}});await ui.ready;store.setLease(key,lease);ui.render();
window.fixture={ui,device,publish,finish(){completeRead()},executions:()=>executions,flush(){ui.render()},ready:true};
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
 await page.waitForFunction(()=>window.fixture?.ready);
 const card=page.locator('#laser-readings-card'),details=card.locator('details');
 assert.equal(await card.getByText('1061.808 nm',{exact:true}).isVisible(),true);
 assert.equal(await card.getByText('0 mW',{exact:true}).isVisible(),true);
 assert.equal(await card.getByText('0 mA',{exact:true}).isVisible(),true);
 assert.equal(await card.getByText('Output disabled',{exact:true}).isVisible(),true);
 assert.equal(await card.getByText('Local control',{exact:true}).isVisible(),true);
 assert.equal(await card.getByText('Status byte',{exact:true}).isVisible(),false,'normal diagnostics stay hidden');
 assert.equal(await card.getByText('Complete',{exact:true}).isVisible(),false);
 if(process.env.YANG_LAB_UI_EVIDENCE){await mkdir(process.env.YANG_LAB_UI_EVIDENCE,{recursive:true});await card.screenshot({path:resolve(process.env.YANG_LAB_UI_EVIDENCE,'laser-readings.png')});}
 await details.locator('summary').click();
 assert.equal(await card.getByText('Status byte',{exact:true}).isVisible(),true);
 await page.evaluate(()=>{window.savedDetails=document.getElementById('laser-reading-details');fixture.publish();fixture.flush();});
 assert.equal(await page.evaluate(()=>savedDetails===document.getElementById('laser-reading-details')&&savedDetails.open),true,'telemetry preserves expanded details');
 await details.locator('summary').click();
 await card.getByRole('button',{name:'Refresh now'}).click();
 await page.waitForFunction(()=>fixture.executions()===1);
 const refresh=card.locator('[data-op="laser-read"]');
 assert.equal(await refresh.getAttribute('aria-busy'),'true');
 assert.equal(await refresh.isDisabled(),true);
 assert.match(await refresh.textContent(),/Refreshing/);
 assert.equal(await refresh.evaluate(e=>getComputedStyle(e,'::before').animationName),'operation-spin');
 await refresh.evaluate(e=>e.click());
 assert.equal(await page.evaluate(()=>fixture.executions()),1,'busy refresh cannot submit again');
 await page.evaluate(()=>fixture.finish());
 await page.waitForFunction(()=>!document.querySelector('#laser-readings-card [aria-busy="true"]'));
 assert.equal(await card.getByText('1061.809 nm',{exact:true}).isVisible(),true);
 await page.evaluate(()=>{fixture.device.sample_age_s=9;fixture.publish();fixture.flush()});
 assert.equal(await card.getByText('Last reported: Output disabled',{exact:true}).isVisible(),true);
 assert.equal(await card.getByRole('status').isVisible(),true);
 assert.equal(await page.locator('[data-op="laser-output-on"]').isDisabled(),true);
 await page.evaluate(()=>{fixture.device.sample_age_s=0.1;fixture.device.laser.output_enabled=null;fixture.publish();fixture.flush()});
 assert.equal(await card.getByText('Output unknown',{exact:true}).isVisible(),true);
 for(const width of [960,1440]){
  await page.setViewportSize({width,height:900});
  const fits=await card.locator('.laser-readings').evaluate(e=>[...e.querySelectorAll('dd')].every(value=>value.scrollWidth<=value.clientWidth+1));
  assert.equal(fits,true,'reading values fit at '+width+' px');
 }
 assert.deepEqual(errors,[]);
 console.log('PASS: laser readings, collapsed diagnostics, preserved expansion, one async read-only refresh, uncertainty and 960/1440 px layout');
}finally{await browser?.close();server.closeAllConnections();await new Promise(yes=>server.close(yes));}
