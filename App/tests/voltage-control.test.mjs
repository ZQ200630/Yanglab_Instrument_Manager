import test from 'node:test';
import assert from 'node:assert/strict';
import {syncVoltageDraft,voltageDraftIds} from '../web/voltage-control.js';
import {appendTelemetry} from '../web/view-model.js';
const domain=()=>({context:{session_id:'session',connection_id:'one',epoch:1},device:{requested_voltage_v:[2.6,...Array(7).fill(0)]}});
test('voltage targets initialize from commanded voltage and retain edits during telemetry and zero epochs',()=>{
 const local={},d=domain();syncVoltageDraft(local,d,'boot');
 assert.equal(local.inputs.get('voltage-1').value,'02.600');
 local.voltageDraftDirty.add('voltage-1');local.inputs.set('voltage-1',{value:'03.123'});
 local.voltageHistory=[{received_at:100,voltage_v:Array(8).fill(1)}];const history=local.voltageHistory;
 d.device.requested_voltage_v[0]=0;d.context.epoch++;syncVoltageDraft(local,d,'boot');
 assert.equal(local.inputs.get('voltage-1').value,'03.123');
 assert.equal(local.inputs.get('voltage-2').value,'00.000');
 assert.equal(local.voltageHistory,history,'a safety epoch keeps the current connection history');
});
test('new voltage connection or Host boot clears stale drafts without borrowing measured values',()=>{
 const local={},d=domain();syncVoltageDraft(local,d,'boot');
 local.voltageDraftDirty.add('voltage-1');local.inputs.set('voltage-1',{value:'09.123'});
 local.voltageHistory=[{received_at:100,voltage_v:Array(8).fill(1)}];
 d.context.connection_id='two';d.device.requested_voltage_v=Array(8).fill(0);syncVoltageDraft(local,d,'boot');
 assert.equal(local.inputs.get('voltage-1').value,'00.000');assert.equal(local.voltageDraftDirty.size,0);
 assert.deepEqual(local.voltageHistory,[]);
 local.voltageDraftDirty.add('voltage-1');local.inputs.set('voltage-1',{value:'04.000'});
 d.device.requested_voltage_v=null;d.device.voltage_v=Array(8).fill(8);syncVoltageDraft(local,d,'new-boot');
 for(const id of voltageDraftIds)assert.equal(local.inputs.has(id),false);
 local.voltageHistory=appendTelemetry(local.voltageHistory,{received_at:1,voltage_v:Array(8).fill(.5)},'voltage');
 assert.equal(local.voltageHistory.length,1);assert.equal(local.voltageHistory[0].received_at,1,'a new Worker monotonic clock accepts new telemetry immediately');
});
