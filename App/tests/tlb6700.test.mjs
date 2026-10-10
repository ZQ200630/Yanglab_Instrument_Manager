import test from 'node:test';
import assert from 'node:assert/strict';
import * as panels from '../web/panels.js';
import {actionFor} from '../web/instance-view.js';
import {instanceView} from '../web/instance-view.js';
import {createDeviceStore} from '../web/device-store.js';
import {renderConsole} from '../web/console-ui.js';

test('known stale Host snapshot cannot enable laser controls',()=>{
  const domain={state:'READY',context:{session_id:'s',connection_id:'c',epoch:0},device:{connected:true,sample_age_s:0.1,wavelength_range_nm:[1030,1070],laser:{remote:true,operation_complete:true}}};
  const html=panels.laser(instanceView('laser',domain,true,60000));
  assert.match(html,/data-op="laser-output-on"[^>]*disabled/);
});

test('laser actions are typed and carry explicit confirmation',()=>{
  assert.deepEqual(actionFor('laser-wavelength',()=> '1061'),{name:'move_wavelength',args:{wavelength_nm:1061,confirm:true}});
  assert.deepEqual(actionFor('laser-remote',()=> ''),{name:'set_remote',args:{remote:true,confirm:true}});
  assert.deepEqual(actionFor('laser-output-off',()=> ''),{name:'control_output',args:{enabled:false,confirm:true}});
  assert.throws(()=>actionFor('laser-wavelength',()=> 'NaN'));
});
test('laser panel distinguishes readback, setpoint, identity and unknown output',()=>{
  assert.equal(typeof panels.laser,'function');
  const html=panels.laser({status:{devices:{laser:{connected:true,state:'READY',identity:{serial:'1012',head_model:'TLB-6721',head_serial:'P1001'},
    wavelength_range_nm:[1030,1070],laser:{wavelength_nm:1060.01,wavelength_setpoint_nm:1060,output_enabled:false}}}},roles:{laser:{confirmed:false}},nowMs:0});
  assert.match(html,/TLB-6721/);assert.match(html,/P1001/);assert.match(html,/1060\.01/);
  assert.match(html,/Controller-reported/);assert.match(html,/data-op="laser-output-on"[^>]*disabled/);
  const unknown=panels.laser({status:{devices:{laser:{connected:true}}},roles:{laser:{confirmed:false}}});
  assert.match(unknown,/Unknown/);assert.doesNotMatch(unknown,/Output confirmed Off/);
});
test('emission button names output effects inline',()=>{
 const html=panels.laser({status:{devices:{laser:{connected:true}}}});
 assert.match(html,/Laser Enable/);assert.match(html,/key.*interlock/i);
 assert.doesNotMatch(html,/Each change requires confirmation/);
});

test('normal reading age preserves output label and does not ban independently guarded emission or Stop',()=>{
 const device={connected:true,wavelength_range_nm:[1030,1070],laser:{wavelength_nm:1061.808,power_mw:0,current_ma:0,output_enabled:false,remote:true,operation_complete:true}};
 let header;
 for(const age of [0.1,4.9,5.1,9,60]){
  const html=panels.laser({status:{devices:{laser:{...device,sample_age_s:age}}}});
  const next=html.match(/<div class="card-head"><h2 class="card-title">Laser Status<\/h2>(.*?)<\/div>/s)[1];
  header??=next;assert.equal(next,header,'normal age must not change output badge');
  assert.match(html,/Last updated .* s ago/);
  assert.doesNotMatch(html,/Last reported|Readings are outdated|readings-stale/);
  assert.match(html,/1061\.808/);
  if(age>=35){assert.doesNotMatch(html,/data-op="laser-output-on"[^>]*disabled/);assert.match(html,/data-op="laser-scan-stop">Stop Scan/);}
 }
 for(const age of [undefined,-1]){
  const unknownAge=panels.laser({status:{devices:{laser:{...device,sample_age_s:age}}}});
  assert.match(unknownAge,/Output unknown/);
  assert.match(unknownAge,/Reading age unknown/);
 }
});

test('failed or disconnected laser samples retain visible uncertainty',()=>{
 const device={connected:true,sample_age_s:0.1,status_error:'USB read timeout',laser:{output_enabled:true}};
 const failed=panels.laser({status:{devices:{laser:device}}});
 assert.match(failed,/USB read timeout/);
 assert.match(failed,/Output unknown/);
 const disconnected=panels.laser({status:{devices:{laser:{...device,status_error:null,connected:false}}}});
 assert.match(disconnected,/Output unknown/);
 assert.match(disconnected,/Disconnected.*last readings/);
});

test('laser readings lose current status when Host disconnects or snapshot sequence has a gap',()=>{
 const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),domain={kind:'device',id:d};
 for(const loss of ['offline','gap']){
  const store=createDeviceStore(()=>100);
  store.apply({type:'snapshot',host_id:h,boot_id:b,seq:1,data:{host_id:h,registry:{devices:[{device_id:d,name:'Laser',model_id:'tlb6700',params:{},config_rev:1}],setups:[]},control:{},domains:{['device:'+d]:{state:'READY',context:{domain,session_id:'d'.repeat(32),connection_id:'e'.repeat(32),epoch:1},host_sample_ms:100,device:{connected:true,sample_age_s:0.1,laser:{wavelength_nm:1061.808,output_enabled:false}}}}}});
  store.setClock(h,100,100,100);
  if(loss==='offline')store.disconnected(h);
  else store.apply({type:'domain',host_id:h,boot_id:b,seq:3,domain,data:{}});
  const html=renderConsole('#/host/'+h+'/device/'+d,store.host(h),store);
  assert.match(html,/Output unknown/,loss);
  assert.match(html,/Reading age unknown/,loss);
  assert.match(html,/data-op="laser-output-on"[^>]*disabled/,loss);
 }
});

test('controller error-buffer flag remains visible when reading details are collapsed',()=>{
 const html=panels.laser({status:{devices:{laser:{connected:true,sample_age_s:0.1,laser:{status_byte:128}}}}});
 const main=html.split('<details id="laser-reading-details"')[0];
 assert.match(main,/role="alert"[^>]*>Controller has an error message/);
});
