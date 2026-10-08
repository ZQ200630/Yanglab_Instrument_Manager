import test from 'node:test';import assert from 'node:assert/strict';import * as ui from '../web/console-ui.js';
test('connection effects disclose selected address and real effects inline',()=>{
 assert.equal(typeof ui.connectionEffects,'function');
 for(const [id,effect]of [['voltage',/zero all eight channels/i],['gain',/current off before TEC off/i]]){
  const text=ui.connectionEffects({name:'Bench',params:{port:'COM7'},profile_id:'serial'},{id,profiles:[{id:'serial',open_effects:['DTR_RTS_reset_not_verified']}]});
  assert.match(text,/COM7/);assert.match(text,effect);assert.match(text,/DTR.*RTS/);
 }
});
test('baseline needs both attestations and the same live binding',()=>{
 assert.equal(typeof ui.baselineAuthorized,'function');
 for(const c of [{baseline:true,nominal:false},{baseline:false,nominal:true},{baseline:true,nominal:true,binding:'old'}])assert.equal(ui.baselineAuthorized({...c,binding:c.binding||'live'},'live'),false);
 assert.equal(ui.baselineAuthorized({baseline:true,nominal:true,binding:'live'},'live'),true);
 assert.equal(ui.baselineAuthorized({baseline:true,nominal:true,binding:'live'},null),false);
});
