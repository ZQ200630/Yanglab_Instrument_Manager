// Isolated browser acceptance. The finite native-transport double never opens hardware.
// Set YANG_LAB_PLAYWRIGHT_MODULE to playwright/index.mjs and YANG_LAB_BROWSER to Edge.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFile,mkdir} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {resolve,sep} from 'node:path';
const root=resolve(fileURLToPath(new URL('../../..',import.meta.url)));
const {chromium}=await import(pathToFileURL(process.env.YANG_LAB_PLAYWRIGHT_MODULE).href);
console.log('Browser fixture starting');
const html=`<!doctype html><link rel="stylesheet" href="/App/web/style.css"><style>#content{height:480px;overflow:auto}</style>
<nav id="navigation"></nav><span id="sidebar-mode"></span><span id="sidebar-subtitle"></span><span id="session-pill"></span><span id="current-page-title"></span><main id="content"></main><div id="notice" hidden></div>
<script type="module">
import {mountConsole,replaceMarkup} from '/App/web/console-ui.js';
import {createDeviceStore} from '/App/web/device-store.js';
const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),s='d'.repeat(32),domain={kind:'device',id:d},key=h+'/device/'+d;
location.hash='#/host/'+h+'/device/'+d;
const context={session_id:'f'.repeat(32),domain,connection_id:'3'.repeat(32),epoch:1};
const state={host_id:h,host_name:'Offline browser fixture',mode:'real',registry:{registry_rev:1,settings:{},devices:[{device_id:d,name:'Bench OSA',model_id:'aq6370',profile_id:'gpib-visa',params:{resource:'GPIB0::4::INSTR'},config_rev:1}],drafts:[],setups:[]},control:{['device:'+d]:{state:'CONTROLLED',controller_session:s,control_epoch:0}},domains:{['device:'+d]:{state:'READY',device:{connected:true,state:'READY',identity:'YOKOGAWA,AQ6370D,SN,FW'},context,host_sample_ms:0}}};
const store=createDeviceStore();let seq=0,subscriber,ui,pings=0,completeRead,executions=0;
function publish(){if(++pings>300)throw new Error('Fixture budget exceeded');state.domains['device:'+d].host_sample_ms=performance.now();subscriber({type:'snapshot',host_id:h,boot_id:b,seq:++seq,data:state});}
const lease={token:'1'.repeat(32),boot_id:b,session_id:s,domain,control_epoch:0,expires_in_ms:10000};
const client={preferences:async()=>({pythonPath:'VISA'}),connect:async()=>({connected:true,mode:'real',worker_protocol:3}),disconnect:async()=>{},catalog:async()=>({models:[{id:'aq6370',name:'AQ6370',profiles:[{id:'gpib-visa',open_effects:[]}]}],categories:['OSA']}),subscribe:async fn=>{subscriber=fn;return ()=>{}},requestSnapshot:async()=>{publish();return {seq}},ping:async()=>({monotonic_ms:performance.now(),client_session_id:s,boot_id:b})};
client.nextSequence=()=>executions+1;client.prepare=async()=>({token:'proof'});
client.execute=async(id,intent)=>{if(++executions>1)throw new Error('Repeated submission');await new Promise(yes=>completeRead=yes);return {request_id:id,operation_id:'4'.repeat(32),domain,status:'Terminal',phase:'completed',result:{context,result:{}}};};
const session={client,store,hostId:h,heartbeat:async()=>{},offline:()=>store.disconnected(h),navigate:()=>{},apply(event){const r=store.apply(event);(ui?.requestRender||ui?.render)?.();return r}};
ui=mountConsole(session,{event:{listen(){}}});await ui.ready;store.setLease(key,lease);ui.render();
window.fixture={ui,state,publish,replaceMarkup,finish(){completeRead()},executions:()=>executions,flush(){ui.render()},ready:true};
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
 console.log('Isolated headless browser started');
 const page=await browser.newPage({viewport:{width:1280,height:900}}),errors=[];
 page.setDefaultTimeout(10000);
 page.on('pageerror',e=>{errors.push(e.message);console.error('Fixture error:',e.message)});
 await page.goto('http://127.0.0.1:'+server.address().port);
 await page.waitForFunction(()=>window.fixture?.ready,{},{timeout:10000});
 await page.locator('#osa-name').fill('typed-run');
 await page.locator('#osa-trace').selectOption('B');
 await page.evaluate(()=>{document.getElementById('osa-sweep').open=true;document.getElementById('content').scrollTop=170;document.getElementById('osa-name').focus();window.savedNodes={input:document.getElementById('osa-name'),select:document.getElementById('osa-trace'),details:document.getElementById('osa-sweep'),scroll:document.getElementById('content').scrollTop};fixture.state.domains['device:'+'c'.repeat(32)].pending_request_id='busy-readback';fixture.publish();fixture.flush();});
 const stable=await page.evaluate(()=>({input:savedNodes.input===document.getElementById('osa-name'),select:savedNodes.select===document.getElementById('osa-trace'),details:savedNodes.details===document.getElementById('osa-sweep'),focus:document.activeElement===savedNodes.input,value:savedNodes.input.value,selected:savedNodes.select.value,open:savedNodes.details.open,scroll:document.getElementById('content').scrollTop===savedNodes.scroll}));
 assert.deepEqual(stable,{input:true,select:true,details:true,focus:true,value:'typed-run',selected:'B',open:true,scroll:true},'telemetry must preserve live controls');
 await page.locator('#osa-trace').focus();
 await page.evaluate(()=>{fixture.state.domains['device:'+'c'.repeat(32)].pending_request_id=null;fixture.publish();fixture.flush()});
 assert.equal(await page.evaluate(()=>document.activeElement===savedNodes.select),true,'select focus survives telemetry');
 await page.locator('#osa-trace').click();await page.keyboard.press('ArrowDown');
 await page.evaluate(()=>{fixture.state.domains['device:'+'c'.repeat(32)].pending_request_id='popup-readback';fixture.publish();fixture.flush()});
 await page.keyboard.press('Enter');
 assert.equal(await page.locator('#osa-trace').inputValue(),'C','a native selection in progress survives a background update');
 await page.evaluate(()=>{fixture.state.domains['device:'+'c'.repeat(32)].pending_request_id=null;fixture.publish();fixture.flush()});
 await page.evaluate(()=>{fixture.state.domains['device:'+'c'.repeat(32)].state='FAULT';fixture.publish();fixture.flush()});
 assert.equal(await page.locator('[data-op="osa-read"]').isDisabled(),true,'fault must disable commands immediately');
 await page.evaluate(()=>{fixture.state.domains['device:'+'c'.repeat(32)].state='READY';fixture.publish();fixture.flush()});
 assert.equal(await page.locator('[data-op="osa-read"]').isDisabled(),false,'current authority re-enables eligibility');
 await page.locator('[data-op="osa-read"]').click();
 await page.waitForFunction(()=>fixture.executions()===1);
 assert.equal(await page.locator('[data-op="osa-read"]').getAttribute('aria-busy'),'true','read click immediately shows busy feedback');
 assert.match(await page.locator('[data-op="osa-read"]').textContent(),/Reading/);
 const spinner=await page.locator('[data-op="osa-read"]').evaluate(e=>getComputedStyle(e,'::before').animationName);
 assert.equal(spinner,'operation-spin');
 await page.locator('#navigation a[href="#overview"]').click();
 await page.getByRole('heading',{name:'Instrument overview'}).waitFor();
 await page.evaluate(()=>fixture.finish());
 await page.waitForFunction(()=>!document.querySelector('[aria-busy="true"]'));
 await page.evaluate(()=>{location.hash='#/host/'+'a'.repeat(32)+'/device/'+'c'.repeat(32)});
 await page.locator('[data-op="osa-read"]').waitFor();
 await page.waitForFunction(()=>!document.querySelector('[aria-busy="true"]'));
 const burst=await page.evaluate(async()=>{const start=performance.now();for(let i=0;i<100;i++)fixture.publish();await new Promise(requestAnimationFrame);return performance.now()-start});
 console.log('100 state updates, including final scheduled paint:',burst.toFixed(1)+' ms');
 await page.evaluate(()=>{fixture.state.control['device:'+'c'.repeat(32)].controller_session='9'.repeat(32);fixture.publish();fixture.flush()});
 assert.equal(await page.locator('[data-op="osa-read"]').isDisabled(),true,'foreign ownership remains read only');
 const structural=await page.evaluate(()=>{
  const root=document.createElement('div');document.body.append(root);
  const old='<section><input id="structural-input" value="old"></section> ',next=' <section><input id="structural-input" value="new"></section> ';
  root.innerHTML=old;const live=root.querySelector('input');live.value='unfinished';live.focus();
  fixture.replaceMarkup(root,old,next);
  const result={same:root.querySelector('input')===live,focus:document.activeElement===live,value:root.querySelector('input').value};root.remove();return result;
 });
 assert.deepEqual(structural,{same:true,focus:true,value:'unfinished'},'inserted whitespace must not remove a focused subtree');
 const managed=await page.evaluate(()=>{
  const root=document.createElement('div');document.body.append(root);
  const old='<input id="consent" data-managed="true" type="checkbox" checked><select id="controller" data-managed="true"><option value="one" selected>One</option><option value="two">Two</option></select>',next='<input id="consent" data-managed="true" type="checkbox"><select id="controller" data-managed="true"><option value="one" selected>One</option><option value="two">Two</option></select>';
  root.innerHTML=old;const consent=root.querySelector('input'),select=root.querySelector('select');consent.click();consent.click();select.value='two';
  fixture.replaceMarkup(root,old,next);const result={same:root.querySelector('input')===consent,checked:consent.checked,controller:select.value};root.remove();return result;
 });
 assert.deepEqual(managed,{same:true,checked:false,controller:'one'},'managed attestation and selection must follow current authority');
 assert.deepEqual(errors,[]);
 if(process.env.YANG_LAB_UI_EVIDENCE){await mkdir(process.env.YANG_LAB_UI_EVIDENCE,{recursive:true});await page.screenshot({path:resolve(process.env.YANG_LAB_UI_EVIDENCE,'browser.png')});}
 console.log('PASS: real-browser control identity, focus, edits, details, scroll and authority eligibility');
}finally{await browser?.close();server.closeAllConnections();await new Promise(yes=>server.close(yes));}
