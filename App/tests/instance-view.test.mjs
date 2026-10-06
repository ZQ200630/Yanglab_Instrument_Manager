import test from 'node:test';import assert from 'node:assert/strict';
import {instanceView,actionFor} from '../web/instance-view.js';
import {osa} from '../web/panels.js';
test('an unknown sample age cannot enable Gain current even with a lease',()=>{
  const context={session_id:'a'.repeat(32),connection_id:'b'.repeat(32),epoch:1,domain:{kind:'device',id:'c'.repeat(32)}};
  const state=instanceView('gain',{context,state:'READY',device:{connected:true,fields:{tec_enabled:{value:true,quality:'fresh',observed_age_s:0}}}},true,null,{});
  assert.equal(state.roles.gain.unknown,true);assert.equal(state.status.devices.gain.fields.tec_enabled.quality,'unknown');
});
test('instance action parsing rejects blank values and enforces physical limits',()=>{
  assert.throws(()=>actionFor('voltage-apply',()=>'',{channel:'1'}));
  assert.throws(()=>actionFor('voltage-apply',()=> '24',{channel:'1'}));
  assert.deepEqual(actionFor('voltage-apply',()=> '1.2',{channel:'1'}),{name:'set_channel',args:{channel:1,voltage:1.2}});
  assert.throws(()=>actionFor('stage-move',id=>id.endsWith('-x')?'0.3':'0',{side:'left'}));
  assert.deepEqual(actionFor('fiber-move',id=>id.endsWith('-y')?'0.1':'0',{side:'right'}),{name:'move',args:{side:'right',dx:0,dy:0.1,dz:0}});
});

test('OSA recording name is a validated short name, never a destination path',()=>{
 assert.deepEqual(actionFor('osa-acquire',id=>id==='osa-trace'?'G':'Run_7'),{name:'acquire',args:{trace:'G',archive_name:'Run_7'}});
 for(const name of ['', '../outside','a/b','_a','光谱','a'.repeat(41)])
   assert.throws(()=>actionFor('osa-acquire',id=>id==='osa-trace'?'A':name));
});

test('OSA page provides a short default recording name without a writable filesystem path',()=>{
 const html=osa({status:{devices:{osa:{connected:true}}}});
 assert.match(html,/id="osa-name"[^>]*value="osa"[^>]*maxlength="40"/);
 assert.match(html,/Recording name/);assert.doesNotMatch(html,/id="osa-path"/);
});
