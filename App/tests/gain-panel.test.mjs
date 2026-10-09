import test from 'node:test';
import assert from 'node:assert/strict';
import {gain} from '../web/panels.js';

const values={temperature_c:22.5,target_c:23,current_ma:40,tec_enabled:true,current_enabled:false};
function state(overrides={}){return {nowMs:10000,roles:{gain:{confirmed:true,mode:'READY',context:{session_id:'s',connection_id:'c'}}},status:{devices:{gain:{connected:true,state:'READY',fields:Object.fromEntries(Object.entries(values).map(([name,value])=>[name,{value,quality:'fresh',observed_age_s:0,connection_id:'c',revision:1}])),pid:{values:[1,2,3],quality:'fresh',connection_id:'c'},timing:{roundTripMs:0,receivedAtMs:10000},...overrides}}}};}
test('Gain targets use bounded fixed digit editors with the same selection style as wavelength',()=>{
 const html=gain(state());
 for(const [id,whole,min,max,value]of [['gain-temp',2,15,40,'23.000'],['gain-current',3,0,200,'040.000']]){
  const input=html.match(new RegExp(`<input[^>]*id="${id}"[^>]*>`))?.[0];
  assert.ok(input);assert.match(input,/type="text"/);assert.match(input,/inputmode="decimal"/);
  assert.match(input,/class="[^"]*wavelength-digits/);assert.match(input,/data-digits="3"/);
  for(const [name,expected]of [['data-whole',whole],['min',min],['max',max],['value',value]])assert.match(input,new RegExp(`${name}="${expected}"`));
 }
 assert.match(html,/Click a digit/);assert.match(html,/Enter to apply/);assert.match(html,/Tab to switch/);
});

test('unknown Gain targets stay empty and edited partial text remains a local draft',()=>{
 const unknown=gain(state({fields:{}}));
 for(const id of ['gain-temp','gain-current'])assert.match(unknown,new RegExp(`id="${id}"[^>]*value=""`));
 const s=state();s.inputs=new Map([['gain-temp',{value:'26.'}],['gain-current',{value:''}]]);
 const html=gain(s);assert.match(html,/id="gain-temp"[^>]*value="26\."/);assert.match(html,/id="gain-current"[^>]*value=""/);
});

test('Gain dashboard groups trend/status above two control columns without per-field ages',()=>{
 const html=gain(state());
 assert.match(html,/data-gain-panel="trend"/);assert.match(html,/data-gain-panel="status"/);
 assert.match(html,/Temperature Control/);assert.match(html,/Current Control/);
 assert.doesNotMatch(html,/fresh|s ago|Field readbacks|Wait for stability/);
 for(const name of Object.keys(values))assert.match(html,new RegExp(`data-gain-field="${name}"`));
 for(const id of ['gain-pid-p','gain-pid-i','gain-pid-d','gain-soft-start','gain-smooth-change','gain-ramp-step','gain-ramp-interval-ms','gain-stable-timeout'])assert.match(html,new RegExp(`id="${id}"`));
 assert.match(html,/data-op="gain-enable-current" >/);
 assert.ok(html.indexOf('data-gain-panel="status"')<html.indexOf('Temperature Control'));
});
test('Gain output toggles are state-derived and safety Off survives a normal pending operation',()=>{
 const s=state();s.roles.gain.normalPending={};s.status.devices.gain.fields.current_enabled.value=true;
 const html=gain(s);assert.match(html,/data-op="gain-disable-current" >/);assert.match(html,/data-op="gain-disable-tec" >/);
 assert.doesNotMatch(html,/data-op="gain-enable-current"/);assert.match(html,/data-op="gain-set-current" disabled/);
});

test('Gain command invalidation shows readback waiting while preserving normal and safety gates',()=>{
 const s=state();s.roles.gain.normalPending=true;s.gainNormalFlow=true;s.pendingName='set_temperature';
 for(const field of Object.values(s.status.devices.gain.fields)){field.quality='unknown';field.reason='command started; await a later observed snapshot';}
 const html=gain(s);
 assert.match(html,/Waiting for controller readback/i);
 assert.doesNotMatch(html,/Readings are stale or unavailable/);
 assert.doesNotMatch(html,/>Check status</);
 assert.match(html,/data-op="gain-set-temp" disabled/);
 assert.match(html,/data-op="gain-enable-current" disabled/);
 assert.match(html,/data-op="gain-disable-current" >/);
 assert.match(html,/data-op="gain-disable-tec" >/);
});

test('Gain native safety wait does not use a legacy Pending label or expose its attempt id',()=>{
 const s=state();s.hideConnectionAction=true;s.roles.gain.safetyPending=true;s.roles.gain.mode='STOP_HELD';s.roles.gain.stopHeld=true;
 s.roles.gain.safety={state:'STOP_HELD',phase:'completed',attempt_id:'e3ea12c0142f263b28e42f8675a4b1cb'};
 s.gainSafetyActivity={kind:'disable_tec',phase:'sync',started:0};
 const html=gain(s);
 assert.doesNotMatch(html,/Pending|e3ea12c0142f263b28e42f8675a4b1cb/);
 assert.match(html,/Checking status/i);
 assert.match(html,/data-op="gain-disable-tec" >/);
});

test('Gain pending feedback cannot hide genuine failed or disconnected readbacks',()=>{
 for(const [device,text]of [[{fault:'Thermistor fault',state:'FAULT'},'Thermistor fault'],[{status_error:'Controller did not respond'},'Controller did not respond'],[{connected:false},'Disconnected']]){
  const s=state(device);s.roles.gain.normalPending=true;
  assert.match(gain(s),new RegExp(text));
 }
});

test('Gain command waiting cannot soften old or foreign-connection readback evidence',()=>{
 for(const fieldChange of [{quality:'unknown',observed_age_s:7},{quality:'unknown',connection_id:'previous-connection'}]){
  const s=state();s.roles.gain.normalPending=true;
  Object.assign(s.status.devices.gain.fields.current_enabled,fieldChange);
  const html=gain(s);
  assert.match(html,/Readings are stale or unavailable/);
  assert.doesNotMatch(html,/Waiting for controller readback/);
  assert.match(html,/data-op="gain-set-current" disabled/);
 }
});
test('Unknown Gain state cannot look like Off and PID is never invented',()=>{
 const html=gain(state({fields:{},pid:null}));assert.match(html,/Unknown/);assert.doesNotMatch(html,/>Off</);
 assert.match(html,/data-op="gain-enable-current" disabled/);assert.match(html,/id="gain-pid-p"[^>]*value=""/);
});
test('Gain progress is indeterminate while waiting and counted during ramping',()=>{
 const wait=gain(state({current_operation:{active:true,phase:'waiting_stable',kind:'start_current',steps_completed:0,steps_total:10}}));
 assert.match(wait,/Waiting for temperature/);assert.doesNotMatch(wait,/<progress[^>]*value=/);
 const ramp=gain(state({current_operation:{active:true,phase:'ramping',kind:'start_current',steps_completed:2,steps_total:10}}));
 assert.match(ramp,/<progress[^>]*value="2"[^>]*max="10"/);
});
test('Current startup can be turned Off before the output has enabled',()=>{
 const s=state({current_operation:{active:true,phase:'waiting_stable',kind:'start_current'}});s.pendingName='start_current';s.roles.gain.normalPending='request';
 assert.match(gain(s),/data-op="gain-disable-current" >/);
});
test('An asynchronous Gain watchdog fault stays concrete and visible after connection becomes unhealthy',()=>{
 const html=gain(state({state:'FAULT',connected:false,fault:'Thermal watchdog: temperature <limit>',current_operation:null}));
 assert.match(html,/Thermal watchdog: temperature &lt;limit&gt;/);assert.match(html,/>FAULT</);assert.doesNotMatch(html,/Disconnected · showing last readings/);
});
test('Gain numeric drafts render edited string values rather than replacing them with telemetry',()=>{
 const s=state();s.inputs=new Map([['gain-temp','26.5'],['gain-current','50'],['gain-pid-p','0.5']]);
 const html=gain(s);for(const [id,value]of s.inputs)assert.match(html,new RegExp(`id="${id}"[^>]*value="${value}"`));
});
test('Off remains available during metadata resume before the captured normal action starts',()=>{
 const s=state();s.gainNormalFlow=true;s.pendingName='resume';s.roles.gain.normalPending='metadata-request';
 assert.match(gain(s),/data-op="gain-disable-current" >/);
});
