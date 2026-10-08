// Finite browser transport only; never connects to the lab Host or installs a driver.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFile,mkdir} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {resolve,sep} from 'node:path';
const root=resolve(fileURLToPath(new URL('../../..',import.meta.url)));
const {chromium}=await import(pathToFileURL(process.env.YANG_LAB_PLAYWRIGHT_MODULE).href);
const html=`<!doctype html><link rel="stylesheet" href="/App/web/style.css"><style>body{min-width:900px;padding:24px}#notice{margin-top:12px}</style>
<nav id="navigation" hidden></nav><span id="sidebar-mode" hidden></span><span id="sidebar-subtitle" hidden></span><span id="session-pill" hidden></span><span id="current-page-title" hidden></span><main id="content"></main><div id="notice" hidden></div>
<script type="module">
import {mountConsole} from '/App/web/console-ui.js';
import {createDeviceStore} from '/App/web/device-store.js';
location.hash='#settings';
const h='a'.repeat(32),b='b'.repeat(32),s='c'.repeat(32),store=createDeviceStore();
const state={host_id:h,host_name:'Offline fixture',mode:'real',registry:{registry_rev:1,settings:{},devices:[],drafts:[],setups:[]},control:{},domains:{}};
const inventory={newport:{sdk:{state:'ready'},devices:[]},usb_serial:{ch340:{state:'ready'},cp210x:{state:'missing'}}};
let seq=0,subscriber,ui,reads=0,polls=0,finish;const installs=[];
function publish(){subscriber({type:'snapshot',host_id:h,boot_id:b,seq:++seq,data:state});}
const client={preferences:async()=>({pythonPath:'VISA'}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},catalog:async()=>({models:[],categories:[]}),subscribe:async fn=>{subscriber=fn;return ()=>{}},requestSnapshot:async()=>{publish();return {seq}},ping:async()=>({monotonic_ms:performance.now(),client_session_id:s,boot_id:b}),
 driverStatus:async()=>{if(++reads>20)throw Error('Fixture inventory budget exceeded');return structuredClone(inventory);},
 installDriver:async id=>{if(installs.length>=3)throw Error('Fixture install budget exceeded');installs.push(id);return {state:'running',driver:id};},
 driverInstallStatus:async()=>{if(++polls>3)throw Error('Fixture poll budget exceeded');return await new Promise(r=>finish=r);}};
const session={client,store,hostId:h,heartbeat:async()=>{},offline:()=>store.disconnected(h),navigate:()=>{},apply(event){const result=store.apply(event);ui?.render();return result}};
ui=mountConsole(session,{event:{listen(){}}});await ui.ready;
window.fixture={ready:true,installs,reads:()=>reads,polls:()=>polls,set(state){inventory.usb_serial.cp210x.state=state;},finish(job){if(job.state==='completed')inventory.usb_serial.cp210x.state='ready';finish(job);}};
</script>`;
const server=createServer(async(req,res)=>{try{
 if(req.url==='/'){res.setHeader('Content-Type','text/html');res.end(html);return;}
 const path=resolve(root,'.'+new URL(req.url,'http://localhost').pathname);
 if(!path.startsWith(resolve(root,'App/web')+sep)){res.writeHead(404).end();return;}
 res.setHeader('Content-Type',path.endsWith('.js')?'text/javascript':'text/css');res.end(await readFile(path));
}catch{res.writeHead(404).end();}});
await new Promise(yes=>server.listen(0,'127.0.0.1',yes));
let browser;
try{
 browser=await chromium.launch({executablePath:process.env.YANG_LAB_BROWSER,headless:true,timeout:15000});
 const page=await browser.newPage({viewport:{width:1200,height:1000}}),errors=[];
 page.setDefaultTimeout(10000);page.on('pageerror',e=>errors.push(e.message));
 await page.goto('http://127.0.0.1:'+server.address().port);await page.waitForFunction(()=>window.fixture?.ready);
 const card=page.locator('.drivers-card'),install=card.locator('[data-ui="install-driver"][data-driver="cp210x"]');
 await install.waitFor();assert.equal(await card.locator('[data-ui="install-driver"]').count(),1);
 assert.deepEqual(await page.evaluate(()=>fixture.installs),[],'startup only displays status');
 assert.equal(await card.getByText('Driver required',{exact:true}).count(),1);
 await card.locator('.driver-license summary').click();assert.equal(await card.locator('.driver-license pre').isVisible(),true);
 await card.locator('.driver-license summary').click();
 for(const width of [960,1440]){
  await page.setViewportSize({width,height:1000});
  const layout=await card.evaluate(e=>({height:e.getBoundingClientRect().height,fits:e.scrollWidth<=e.clientWidth,rows:[...e.querySelectorAll('.driver-row')].every(r=>r.scrollWidth<=r.clientWidth),statusX:[...e.querySelectorAll('.driver-state')].map(s=>s.getBoundingClientRect().x)}));
  console.log('Driver layout '+width,JSON.stringify(layout));
  await card.screenshot({path:resolve(root,'Result/console/driver-ui-layout.png')});
  assert.ok(layout.height<340&&layout.fits&&layout.rows,'compact, contained driver rows at '+width);
  assert.ok(Math.max(...layout.statusX)-Math.min(...layout.statusX)<1,'status column aligns');
 }
 const output=resolve(root,'Result/console/driver-ui');await mkdir(output,{recursive:true});
 await card.screenshot({path:resolve(output,'drivers.png')});
 await install.click();await page.waitForFunction(()=>fixture.polls()===1);
 assert.deepEqual(await page.evaluate(()=>fixture.installs),['cp210x']);
 assert.equal(await card.getByRole('button',{name:'Installing…'}).isDisabled(),true);
 assert.equal(await card.getByRole('button',{name:'Refresh'}).isDisabled(),true);
 await page.evaluate(()=>fixture.finish({state:'completed',driver:'cp210x',restart_required:false}));
 await card.locator('[data-driver-row="cp210x"] .driver-state.ready').waitFor();assert.equal(await card.locator('[data-ui="install-driver"]').count(),0);
 await page.evaluate(()=>fixture.set('missing'));await card.getByRole('button',{name:'Refresh'}).click();await install.waitFor();
 await install.click();await page.waitForFunction(()=>fixture.polls()===2);
 await page.evaluate(()=>fixture.finish({state:'failed',driver:'cp210x',message:'Installation cancelled in Windows.'}));
 await card.getByRole('alert').waitFor();assert.match(await card.getByRole('alert').textContent(),/cancelled/);assert.equal(await install.isEnabled(),true);
 await page.evaluate(()=>fixture.set('not_detected'));await card.getByRole('button',{name:'Refresh'}).click();
 await card.getByText('No device detected',{exact:true}).waitFor();assert.equal(await card.locator('[data-ui="install-driver"]').count(),0);
 assert.deepEqual(errors,[]);
 console.log('PASS: compact aligned 960/1440 px driver rows, license, direct CP210x install, fresh ready check, cancellation and absent-device handling; no real Host or installer.');
}finally{await browser?.close();server.closeAllConnections();await new Promise(yes=>server.close(yes));}
