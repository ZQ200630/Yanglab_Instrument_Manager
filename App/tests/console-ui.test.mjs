import test from 'node:test';import assert from 'node:assert/strict';
import {renderConsole,inputValues,snapshotBarrier,currentResult,inputSnapshot,replaceMarkup,updatedHostSettings} from '../web/console-ui.js';
import {readFileSync} from 'node:fs';
import {createDeviceStore} from '../web/device-store.js';
import * as consoleUi from '../web/console-ui.js';
test('focus restoration matches the exact capture or instrument command row after refresh',()=>{
 assert.equal(typeof consoleUi.focusIdentity,'function');
 for(const [key,action]of [['archive','osa-history-load'],['device','rename'],['setup','retire-setup'],['command','pm-command']]){
  const rows=['first','second'].map(value=>({matches:()=>false,dataset:{[action==='pm-command'?'op':'ui']:action,[key]:value}}));
  const saved=consoleUi.focusIdentity(rows[1]);
  assert.equal(rows.find(e=>consoleUi.focusIdentity(e)===saved).dataset[key],'second');
 }
});
test('unified settings contains simple Connections and local settings',()=>{
 const html=renderConsole('settings',null,createDeviceStore(),{},{});
 assert.match(html,/<h1>Settings<\/h1>/);assert.match(html,/Local Host/);assert.match(html,/Connections/);
 assert.match(html,/Control this PC/);assert.match(html,/Control other PCs/);assert.doesNotMatch(html,/id="remote-(fingerprint|code)"/);assert.match(html,/DISABLED/);assert.doesNotMatch(html,/simulation|host-mode|Anaconda|Python|host-python/i);
 assert.doesNotMatch(html,/<h1>Host settings/);
});
test('empty production setup is not filled with fabricated default instrument cards',()=>{
  const store=createDeviceStore();const host={host_id:'a'.repeat(32),connected:true,mode:'real',registry:{devices:[],drafts:[],setups:[],settings:{}}};
  const html=renderConsole('devices',host,store,{},{});
  assert.match(html,/Add New/);assert.match(html,/No instruments configured/);assert.doesNotMatch(html,/GPIB0::4|Voltage Source/);
});
test('production OSA export uses the native selected-folder bridge, never a browser blob',()=>{
 const source=readFileSync(new URL('../web/console-ui.js',import.meta.url),'utf8');
 assert.doesNotMatch(source,/new Blob|createObjectURL|a\.download='spectrum\.csv'/);
 assert.match(source,/exportSelectedTrace\(client/);assert.match(source,/osa-history-load/);
});
test('a duplicate device ID on another Host cannot route into this local record',()=>{
 const host={host_id:'a'.repeat(32),mode:'real',registry:{devices:[{device_id:'c'.repeat(32),name:'Same label',model_id:'aq6370'}],setups:[]}};
 assert.match(renderConsole('#/host/'+'f'.repeat(32)+'/device/'+'c'.repeat(32),host,createDeviceStore()),/Instrument unavailable/);
});
test('registered disconnected OSA with lease offers Connect without fake measurement freshness',()=>{
 const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),s='d'.repeat(32),domain={kind:'device',id:d};
 const host={host_id:h,connected:true,mode:'real',control:{['device:'+d]:{state:'CONTROLLED',controller_session:s,control_epoch:0}},registry:{devices:[{device_id:d,name:'OSA',model_id:'aq6370',config_rev:1}],setups:[]},domains:{['device:'+d]:{state:'DISCONNECTED',device:null,context:{session_id:s,domain,connection_id:null,epoch:0}}}};
 const store=createDeviceStore();store.apply({type:'snapshot',host_id:h,boot_id:b,seq:1,data:host});store.setLease(h+'/device/'+d,{token:'e'.repeat(32),session_id:s,boot_id:b,domain,control_epoch:0,expires_in_ms:10000});
 const html=renderConsole('#/host/'+h+'/device/'+d,store.host(h),store);
 assert.match(html,/<button class="btn primary" data-op="connect" data-role="osa">Connect/);
 assert.doesNotMatch(html,/Retry disconnect/);
});
test('disconnected setup disables mutations and hides irrelevant fiber controls',()=>{const html=renderConsole('devices',null,createDeviceStore());assert.match(html,/<button disabled [^>]*data-ui="add-new"/);assert.doesNotMatch(html,/data-ui="add-setup"/);});
test('unchanged metadata does not replace a live native selector',()=>{let replacements=0;const target={set innerHTML(value){replacements++}};assert.equal(replaceMarkup(target,'form','form'),false);assert.equal(replacements,0);assert.equal(replaceMarkup(target,'form','disarmed'),true);assert.equal(replacements,1);});
test('form snapshots preserve check consent and expanded evidence during telemetry updates',()=>{
 const data=inputSnapshot([{id:'check',type:'checkbox',value:'on',checked:true},{id:'evidence',tagName:'DETAILS',open:true}]);
 assert.equal(data.get('check').checked,true);assert.equal(data.get('evidence').open,true);
});
test('snapshot barrier never accepts an older initial snapshot',async()=>{
  const barrier=snapshotBarrier();barrier.receive({seq:2});barrier.expect(3);
  let resolved=false;barrier.promise.then(()=>{resolved=true});await Promise.resolve();assert.equal(resolved,false);
  barrier.receive({seq:3});assert.equal((await barrier.promise).seq,3);
});
test('late terminal stays historical after release or a new connection',()=>{
  const context={session_id:'s',connection_id:'c',epoch:1};
  assert.equal(currentResult({control_epoch:1},{context},{lease:()=>null,get:()=>({context})},'key'),false);
  assert.equal(currentResult({control_epoch:1},{context},{lease:()=>({control_epoch:1}),get:()=>({context:{...context,epoch:2}})},'key'),false);
});
test('navigation never copies one instrument target into a different instance',()=>{
  const values=inputValues('host/device/one','host/device/two',new Map([['gain-current','60']]),new Map([['gain-current','0']]));
  assert.equal(values.get('gain-current'),'0');
});
test('editing Host name preserves the recording root and discards obsolete interpreter settings',()=>{
 const previous={host_name:'Old',python_path:'VISA',data_root:'D:/Recordings'};
 assert.deepEqual(updatedHostSettings(previous,'New'),{host_name:'New',data_root:'D:/Recordings'});
 assert.deepEqual(previous,{host_name:'Old',python_path:'VISA',data_root:'D:/Recordings'});
});

test('measurement folder path is read-only and its saved setting preserves other settings',()=>{
 const host={connected:true,registry:{settings:{data_root:'D:/Selected & data'}},archive:{active_root:'D:/Previous'}};
 const html=renderConsole('settings',host,createDeviceStore());
 assert.match(html,/id="host-data-root"[^>]*readonly/);
 assert.match(html,/value="D:\/Selected &amp; data"/);
 assert.match(html,/data-ui="choose-data-root"/);
 assert.match(html,/safe Host restart/);
 assert.deepEqual(updatedHostSettings({host_name:'Old',data_root:'D:/Previous'},'New','D:/Selected & data'),
   {host_name:'New',data_root:'D:/Selected & data'});
 assert.equal(updatedHostSettings({data_root:'D:/Previous'},'New',null).data_root,'D:/Previous');
});
