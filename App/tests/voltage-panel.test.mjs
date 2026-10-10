import test from 'node:test';
import assert from 'node:assert/strict';
import {voltage} from '../web/panels.js';
import {actionFor} from '../web/instance-view.js';

const device=()=>({connected:true,state:'READY',quality:'fresh',observed_age_s:0,
 voltage_v:Array(8).fill(.008),current_ma:[-.003,...Array(7).fill(.012)],
 requested_voltage_v:[2.6,...Array(7).fill(0)],zero_evidence:{state:'measured_zero'}});
const render=(d=device(),extra={})=>voltage({status:{devices:{voltage:d}},busy:false,...extra});

test('voltage channels use fixed three-decimal draft editors and paired measured readouts',()=>{
 const html=render();
 assert.equal((html.match(/data-digits="3" data-whole="2"/g)||[]).length,8);
 assert.match(html,/id="voltage-1"[^>]*value="02\.600"/);
 assert.match(html,/class="channel-readings"/);
 assert.match(html,/Voltage<\/dt><dd[^>]*>0\.008/);
 assert.match(html,/Current<\/dt><dd[^>]*>-0\.003/);
 assert.match(html,/Enter to apply/);
 assert.doesNotMatch(html,/Telemetry sample|Current zero evidence|Connection sets every|Last completed command|Fixed scale|Operation finished/);
});

test('voltage target rendering keeps a draft separate from requested and measured values',()=>{
 const html=render(device(),{inputs:new Map([['voltage-1',{value:'03.123'}]])});
 assert.match(html,/id="voltage-1"[^>]*value="03\.123"/);
 assert.match(html,/Voltage<\/dt><dd[^>]*>0\.008/);
 assert.doesNotMatch(html,/<dd[^>]*>3\.123/);
});

test('voltage stale and missing measurements stay truthful in compact channel status',()=>{
 const d=device();d.quality='stale';d.voltage_v[1]=null;
 const html=render(d);
 assert.match(html,/Previous readings/);
 assert.match(html,/Voltage<\/dt><dd[^>]*>Unknown/);
 assert.doesNotMatch(html,/Telemetry sample|s ago|Current zero evidence/);
});

test('voltage status failures keep the exact error visible and zero accessible',()=>{
 const d=device();d.status_error='sensor <missing>';d.quality='unknown';d.voltage_v=null;d.current_ma=null;
 const html=render(d);
 assert.match(html,/sensor &lt;missing&gt;/);
 assert.match(html,/data-op="voltage-apply" data-channel="1" disabled/);
 assert.match(html,/data-op="voltage-zero" >/);
 assert.match(html,/Readings unavailable/);
});

test('voltage apply normalizes the captured channel target to 0.001 V',()=>{
 assert.deepEqual(actionFor('voltage-apply',()=> '02.60049',{channel:'1'}),{name:'set_channel',args:{channel:1,voltage:2.6}});
 assert.deepEqual(actionFor('voltage-apply',()=> '13.9996',{channel:'8'}),{name:'set_channel',args:{channel:8,voltage:14}});
 for(const raw of ['14.0001','-.0001','NaN',''])assert.throws(()=>actionFor('voltage-apply',()=>raw,{channel:'1'}));
});

test('a pending native voltage operation cannot reuse an older completion label',()=>{
 const html=render(device(),{roles:{voltage:{normalPending:'new-request'}},activity:{kind:'set_channel',phase:'sync',started:0,ended:1,outcome:'complete'}});
 assert.doesNotMatch(html,/Operation finished/);
 assert.match(html,/Updating controller/);
});

test('unknown voltage outcomes and faults remain explicit while finite readings are historical',()=>{
 const html=render(device(),{activity:{kind:'set_channel',phase:'instrument',started:0,ended:1,outcome:'unknown'}});
 assert.match(html,/Operation unconfirmed/);
 assert.match(html,/Previous readings/);
 const d=device();d.fault={message:'controller fault'};
 const fault=render(d);
 assert.match(fault,/controller fault/);assert.match(fault,/Previous readings/);
 assert.doesNotMatch(fault,/\[object Object\]/);
});
