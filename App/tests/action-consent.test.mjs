import test from 'node:test';import assert from 'node:assert/strict';import * as ui from '../web/console-ui.js';
test('baseline and nominal authorization are independent and either cancellation dispatches nothing',()=>{
 for(const denied of [0,1]){let asked=0,dispatch=0;const ok=ui.confirmInstrumentAction({method:'action',params:{name:'adopt_baseline',args:{side:'left'}},record:{name:'Left fiber'},mode:'real'},message=>{assert.match(message,asked===0?/baseline/:/nominal/);return asked++!==denied;});if(ok)dispatch++;assert.equal(dispatch,0);assert.equal(asked,denied+1);}
 const messages=[];assert.equal(ui.confirmInstrumentAction({method:'action',params:{name:'adopt_baseline',args:{side:'right'}},record:{name:'Right fiber'},mode:'real'},m=>{messages.push(m);return true;}),true);
 assert.equal(messages.length,2);assert.match(messages[0],/voltage readbacks.*baseline/);assert.match(messages[1],/uncalibrated MAX312D nominal conversion/);
});
test('ordinary connection consent discloses selected address and real output effects',()=>{
 for(const [id,effect]of [['voltage',/zero all eight channels/i],['gain',/interlock.*shutdown/i]]){let warning;const result=ui.confirmInstrumentAction({method:'connect',params:{acknowledge_lifecycle:true},record:{name:'Bench device',params:{port:'COM7'},profile_id:'serial'},model:{id,connect_effects:[],close_effects:[],profiles:[{id:'serial',open_effects:['DTR_RTS_reset_not_verified']}]},mode:'real'},m=>{warning=m;return false;});assert.equal(result,false);assert.match(warning,/Bench device.*COM7/s);assert.match(warning,effect);assert.match(warning,/DTR.*RTS/);}
});
