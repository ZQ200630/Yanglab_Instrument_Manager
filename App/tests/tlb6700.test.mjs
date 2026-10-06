import test from 'node:test';
import assert from 'node:assert/strict';
import * as panels from '../web/panels.js';
import {actionFor} from '../web/instance-view.js';
import {confirmInstrumentAction} from '../web/console-ui.js';
import {instanceView} from '../web/instance-view.js';

test('known stale Host snapshot cannot enable laser controls',()=>{
  const domain={state:'READY',context:{session_id:'s',connection_id:'c',epoch:0},device:{connected:true,sample_age_s:0.1,wavelength_range_nm:[1030,1070],laser:{remote:true,operation_complete:true}}};
  const html=panels.laser(instanceView('laser',domain,true,60000));
  assert.match(html,/data-op="laser-output-on"[^>]*disabled/);
});

test('laser actions are typed and carry explicit confirmation',()=>{
  assert.deepEqual(actionFor('laser-wavelength',()=> '1061'),{name:'set_wavelength',args:{wavelength_nm:1061,confirm:true}});
  assert.deepEqual(actionFor('laser-remote',()=> ''),{name:'set_remote',args:{remote:true,confirm:true}});
  assert.deepEqual(actionFor('laser-output-off',()=> ''),{name:'set_output',args:{enabled:false,confirm:true}});
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
test('enable confirmation names emission and respects cancellation',()=>{
  let message;
  const accepted=confirmInstrumentAction({method:'action',params:{name:'set_output',args:{enabled:true,confirm:true}},record:{name:'1060 laser'},model:{id:'tlb6700'},mode:'real'},text=>{message=text;return false;});
  assert.equal(accepted,false);assert.match(message,/laser emission/i);assert.match(message,/key.*interlock/i);
});
