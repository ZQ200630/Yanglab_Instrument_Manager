import test from 'node:test';import assert from 'node:assert/strict';
let history={};try{history=await import('../web/gain-history.js');}catch(error){if(error.code!=='ERR_MODULE_NOT_FOUND')throw error;}
const connection_id='e'.repeat(32),context={session_id:'f'.repeat(32),connection_id,epoch:1},bootId='b'.repeat(32);
const fixture=(revision,nowMs,changes={})=>({bootId,ageUpperMs:100,nowMs,domain:{receivedAt:nowMs,context,state:'READY',host_sample_ms:nowMs-100,device:{connected:true,state:'READY',fields:{temperature_c:{value:25,connection_id,revision,quality:'fresh',observed_age_s:.2},target_c:{value:25,connection_id,revision,quality:'fresh',observed_age_s:.2}},...changes}}});
test('history uses observation time instead of repaint or uniformly spaced revision indexes',()=>{
 assert.equal(typeof history.appendGainHistory,'function');let points=history.appendGainHistory([],fixture(1,1000));assert.equal(points[0].observedAtMs,700);assert.equal(points[0].temperature_c,25);assert.equal(points[0].target_c,25);
 points=history.appendGainHistory(points,fixture(1,2500));assert.equal(points.length,1,'a heartbeat repaint is not a measurement');points=history.appendGainHistory(points,fixture(2,4800));assert.equal(points[1].observedAtMs,4500);
});
test('stale and error events create one explicit gap without repeating phantom points',()=>{
 assert.equal(typeof history.appendGainHistory,'function');const good=fixture(1,1000),bad=fixture(2,2000,{fields:{temperature_c:{...good.domain.device.fields.temperature_c,quality:'stale',revision:2}}});let points=history.appendGainHistory([],good);points=history.appendGainHistory(points,bad);assert.equal(points[1].gap,true);assert.equal(points[1].temperature_c,null);points=history.appendGainHistory(points,{...bad,nowMs:3000});assert.equal(points.length,2);
 points=history.appendGainHistory(points,fixture(3,4000));assert.equal(points.length,3);assert.equal(points[2].temperature_c,25);
});
test('receipt projection keeps a rapid resumed observation after its gap without accepting old revisions',()=>{
 assert.equal(typeof history.appendGainHistory,'function');let points=history.appendGainHistory([],fixture(1,1000));const bad=fixture(2,1100);bad.domain.device.fields.temperature_c.quality='error';points=history.appendGainHistory(points,bad);
 points=history.appendGainHistory(points,fixture(3,1200));assert.equal(points.at(-1).revision,3);assert.equal(points.at(-1).temperature_c,25);
 const old=fixture(1,1400);old.domain.device.fields.temperature_c.quality='stale';points=history.appendGainHistory(points,old);const count=points.length;points=history.appendGainHistory(points,fixture(1,2000));assert.equal(points.length,count,'an old sample cannot return as a new point after a gap');
});
test('boot or connection replacement resets history and rejects mismatched or regressing evidence',()=>{
 assert.equal(typeof history.appendGainHistory,'function');let points=history.appendGainHistory([],fixture(5,5000));points=history.appendGainHistory(points,fixture(4,6000));assert.equal(points.length,1);
 const next=fixture(1,7000);next.bootId='9'.repeat(32);points=history.appendGainHistory(points,next);assert.equal(points.length,1);assert.equal(points[0].revision,1);
 const wrong=fixture(2,8000);wrong.domain.context={...context,connection_id:'7'.repeat(32)};points=history.appendGainHistory(points,wrong);assert.equal(points.filter(p=>p.temperature_c!==null).length,0,'prior connection fields are not new data');
});
test('sixteen minute time retention expires only actual observations on a new update',()=>{
 assert.equal(typeof history.appendGainHistory,'function');let points=[];for(let i=1;i<=1100;i++)points=history.appendGainHistory(points,fixture(i,i*1000));assert.equal(points.length,960);assert.ok(points.every(point=>point.observedAtMs>=1100000-960000));
 points=history.appendGainHistory(points,fixture(1101,2100000));assert.equal(points.length,1,'expired observations are removed after a real new update');
});
test('five Hz observations retain fifteen full minutes and the inclusive sixteen minute bounded window',()=>{
 assert.equal(typeof history.appendGainHistory,'function');let points=[];
 for(let i=1;i<=5000;i++){
  const sample=fixture(i,i*200);sample.ageUpperMs=0;sample.domain.device.fields.temperature_c.observed_age_s=0;sample.domain.device.fields.temperature_c.value=20+i/1000;
  points=history.appendGainHistory(points,sample);
  assert.strictEqual(history.appendGainHistory(points,{...sample,nowMs:sample.nowMs+10}),points,'a heartbeat never adds a measurement');
 }
 assert.equal(points.length,4801);assert.equal(points[0].revision,200);assert.equal(points.at(-1).revision,5000);
 const visible=points.filter(point=>point.observedAtMs>=1000000-900000);
 assert.equal(visible.length,4501,'the full fifteen minute interval includes both endpoints');
 assert.equal(visible.at(-1).observedAtMs-visible[0].observedAtMs,900000);
 assert.equal(new Set(visible.map(point=>point.revision)).size,4501);assert.equal(new Set(visible.map(point=>point.temperature_c)).size,4501);
 assert.ok(points.every(point=>point.observedAtMs>=1000000-960000));
});
test('dense or irregular observed updates remain bounded without inventing uniform timestamps',()=>{
 let points=[];for(let i=1;i<=5000;i++){const sample=fixture(i,i*190+(i%2)*5);sample.ageUpperMs=0;sample.domain.device.fields.temperature_c.observed_age_s=0;points=history.appendGainHistory(points,sample);}
 assert.equal(points.length,4801);assert.equal(points[0].revision,200);assert.equal(points.at(-1).revision,5000);
 assert.equal(points[1].observedAtMs-points[0].observedAtMs,195);assert.equal(points[2].observedAtMs-points[1].observedAtMs,185);
});
