import test from 'node:test';
import assert from 'node:assert/strict';
import {formatTarget,editTarget,formatDigits,editDigits,createTargetQueue,syncTarget,laserMotion} from '../web/wavelength-editor.js';
test('scan wavelength and velocity digits skip decimal, carry, and enforce each range',()=>{
 assert.equal(formatDigits(.2,2,2),'00.20');
 assert.equal(formatDigits(1060.8,3,4),'1060.800');
 assert.deepEqual(editDigits('00.99',4,'ArrowUp',[.01,10],2,2),{value:1,position:4});
 assert.deepEqual(editDigits('01.00',1,'ArrowRight',[.01,10],2,2),{value:1,position:3});
 assert.throws(()=>editDigits('00.01',4,'ArrowDown',[.01,10],2,2),/range/);
 assert.throws(()=>editDigits('01.00',1,'ArrowUp',[.01,1],2,2),/range/);
});
test('motion merges measured wavelength without inventing fresh power or completion from ACK',()=>{
 const device={laser:{wavelength_nm:1060,power_mw:10,output_enabled:false,operation_complete:true},motion:{wavelength_nm:1061,operation_complete:true},motion_pending:true};
 assert.deepEqual(laserMotion(device),{wavelength_nm:1061,power_mw:10,output_enabled:false,operation_complete:false});
 device.motion_pending=false;assert.equal(laserMotion(device).operation_complete,true);
});
test('fixed aligned digits, selection, stepping and two layer bounds',()=>{
 assert.equal(formatTarget(780.01),'0780.010');
 assert.deepEqual(editTarget('1060.999',7,'ArrowUp',[1045,1085]),{value:1061,position:7});
 assert.deepEqual(editTarget('1060.000',3,'ArrowRight',[1045,1085]),{value:1060,position:5});
 assert.deepEqual(editTarget('1060.000',5,'ArrowLeft',[1045,1085]),{value:1060,position:3});
 assert.throws(()=>editTarget('1085.000',7,'ArrowUp',[1045,1085]),/range/);
});
test('one active target, latest edit only, no replay after failure or context loss',async()=>{
 let complete,current=true;const sent=[],errors=[];
 const q=createTargetQueue({isCurrent:()=>current,waitReady:async()=>{},send:v=>{sent.push(v);return new Promise(yes=>complete=yes)},onError:e=>errors.push(e),delay:0});
 q.submit(1060);await new Promise(yes=>setTimeout(yes,5));q.submit(1060.1);q.submit(1060.2);complete();
 await new Promise(yes=>setTimeout(yes,5));assert.deepEqual(sent,[1060,1060.2]);
 current=false;q.submit(1060.3);complete();await new Promise(yes=>setTimeout(yes,5));assert.deepEqual(sent,[1060,1060.2]);
 const bad=createTargetQueue({isCurrent:()=>true,waitReady:async()=>{},send:async()=>{throw Error('lost')},onError:e=>errors.push(e),delay:0});bad.submit(1060);
 await new Promise(yes=>setTimeout(yes,5));assert.equal(errors.length,1);q.cancel();bad.cancel();
});
test('scan completion uses actual stop readback and never substitutes old setpoint',()=>{
 const l={laserScanning:true,targetValue:1060};
 syncTarget(l,{operation_complete:true,wavelength_nm:1060.345,wavelength_setpoint_nm:1061});
 assert.equal(l.targetValue,1060.345);assert.equal(l.laserScanning,false);
 syncTarget(l,{operation_complete:true,wavelength_nm:1060.345,wavelength_setpoint_nm:1061});assert.equal(l.targetValue,1060.345);
 syncTarget(l,{operation_complete:true,wavelength_nm:1062,wavelength_setpoint_nm:1062});assert.equal(l.targetValue,1062);
});
test('cancel during debounce releases busy without sending any target',async()=>{
 let busy=true,sent=0;
 const q=createTargetQueue({send:async()=>sent++,waitReady:async()=>{},isCurrent:()=>true,onError:assert.fail,onBusy:value=>busy=value,delay:20});
 q.submit(1060);q.cancel();await new Promise(yes=>setTimeout(yes,30));assert.equal(busy,false);assert.equal(sent,0);
});
