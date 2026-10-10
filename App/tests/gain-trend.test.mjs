import test from 'node:test';
import assert from 'node:assert/strict';
import {gainTrendModel} from '../web/gain-trend.js';
test('Temperature trend uses elapsed time and includes literal zero in its scale',()=>{
 const model=gainTrendModel([{observedAtMs:1000,temperature_c:0,target_c:35},{observedAtMs:2000,temperature_c:1},{observedAtMs:10000,temperature_c:2}],{nowMs:10000,windowS:60});
 assert.equal(model.segments[0].length,3);assert.ok(model.yMin<0&&model.yMax>35);
 const [a,b,c]=model.segments[0];assert.ok(Math.abs((c.x-b.x)/(b.x-a.x)-8)<.001);
});
test('Temperature trend breaks at gaps and connection transitions and slides without adding points',()=>{
 const history=[{observedAtMs:1000,temperature_c:20,connection_id:'a'},{observedAtMs:2000,gap:true,temperature_c:null},{observedAtMs:3000,temperature_c:21,connection_id:'a'},{observedAtMs:4000,temperature_c:22,connection_id:'b'}];
 const before=gainTrendModel(history,{nowMs:5000,windowS:60}),after=gainTrendModel(history,{nowMs:10000,windowS:60});
 assert.equal(before.segments.length,3);assert.equal(after.segments.flat().length,3);assert.ok(after.segments[0][0].x<before.segments[0][0].x);
});
test('Temperature trend rejects invalid future/old data without fabricating samples',()=>{
 const model=gainTrendModel([{observedAtMs:1,temperature_c:20},{observedAtMs:90000,temperature_c:21},{observedAtMs:99900,temperature_c:Infinity},{observedAtMs:101000,temperature_c:22}],{nowMs:100000,windowS:60});
 assert.equal(model.segments.flat().length,1);assert.equal(model.segments[0][0].temperature_c,21);
});
