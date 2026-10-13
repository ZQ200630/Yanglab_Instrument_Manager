import test from 'node:test';
import assert from 'node:assert/strict';
import {renderGenerator} from '../web/generator.js';
import {instanceView} from '../web/instance-view.js';
import {startActivity,finishActivity} from '../web/activity.js';

const context={session_id:'a'.repeat(32),connection_id:'b'.repeat(32),epoch:1};
function domain(){return {context,state:'READY',device:{connected:true,quality:'confirmed',previous:false,
 capabilities:{channels:1},channels:[{channel:1,waveform:'sine',frequency_hz:1250,amplitude_vpp:2,offset_v:0,output_enabled:true}]}};}

function outputSwitch(html,channel=1){
 const controls=[...html.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)]
  .filter(([,attrs])=>/data-op="generator-output-(?:toggle|on|off)"/.test(attrs)&&attrs.includes(`data-channel="${channel}"`));
 assert.equal(controls.length,1,`CH${channel} has exactly one output control`);
 const [markup,attrs,label]=controls[0];
 assert.match(attrs,/\brole="switch"/);
 assert.match(attrs,/\bdata-op="generator-output-toggle"/);
 return {markup,attrs,label};
}

for(const [name,owned,age,edit]of [
 ['disconnected instrument',true,0,d=>{d.device.connected=false;}],
 ['lost control ownership',false,0,()=>{}],
 ['unknown observation age',true,null,()=>{}],
])test(`generator retained state is previous after ${name}`,()=>{
 const d=domain();edit(d);
 const html=renderGenerator(instanceView('generator',d,owned,age));
 assert.match(html,/Output on/);
 assert.match(html,/Previous/);
 assert.match(html,/id="generator-1-frequency"[^>]*value="1250"/);
 assert.match(html,/data-op="generator-apply"[^>]*disabled/);
 assert.match(outputSwitch(html).attrs,/\bdisabled\b/);
});

test('generator restricted Host state cannot label retained readback as current',()=>{
 const state=instanceView('generator',domain(),true,0);
 state.roles.generator.hostRestricted=true;
 const html=renderGenerator(state);
 assert.match(html,/Output on/);
 assert.match(html,/Previous/);
 assert.match(html,/id="generator-1-frequency"[^>]*value="1250"/);
 assert.match(outputSwitch(html).attrs,/\bdisabled\b/);
});

test('generator preserves confirmed current readback while controls wait for a non-conflicting state update',()=>{
 const html=renderGenerator(instanceView('generator',domain(),true,0,{pending:'request-1'}));
 assert.match(html,/Output on/);
 assert.match(html,/id="generator-1-frequency"[^>]*value="1250"/);
 assert.doesNotMatch(html,/Previous/);
 assert.match(html,/data-op="generator-apply"[^>]*disabled/);
 assert.match(outputSwitch(html).attrs,/\bdisabled\b/);
 assert.match(outputSwitch(html).attrs,/\baria-checked="true"/);
});

for(const enabled of [false,true])test(`generator header switch exposes confirmed ${enabled?'ON':'OFF'} readback and permits its inverse in READY`,()=>{
 const d=domain();d.device.channels[0].output_enabled=enabled;
 const html=renderGenerator(instanceView('generator',d,true,0));
 const control=outputSwitch(html);
 assert.match(control.attrs,new RegExp(`\\baria-checked="${enabled}"`));
 assert.doesNotMatch(control.attrs,/\bdisabled\b/);
 const header=html.match(/<div class="card-head">([\s\S]*?)<\/div>/)?.[1];
 assert.ok(header?.includes(control.markup),'output switch is in its channel header');
});

test('generator renders one independently checked switch for each channel',()=>{
 const d=domain();d.device.capabilities.channels=2;
 d.device.channels.push({...d.device.channels[0],channel:2,output_enabled:false});
 const html=renderGenerator(instanceView('generator',d,true,0));
 assert.match(outputSwitch(html,1).attrs,/\baria-checked="true"/);
 assert.match(outputSwitch(html,2).attrs,/\baria-checked="false"/);
});

for(const value of [undefined,null,'false'])test(`generator missing or non-boolean output readback ${String(value)} never implies OFF`,()=>{
 const d=domain();d.device.channels[0].output_enabled=value;
 const control=outputSwitch(renderGenerator(instanceView('generator',d,true,0)));
 assert.match(control.attrs,/\bdisabled\b/);
 assert.doesNotMatch(control.attrs,/\baria-checked=/);
 assert.match(control.attrs,/aria-description="Output state is not synced\."/);
});

for(const enabled of [false,true])test(`generator historical ${enabled?'ON':'OFF'} output readback remains previous and cannot toggle`,()=>{
 const d=domain();Object.assign(d.device,{quality:'historical',previous:true});d.device.channels[0].output_enabled=enabled;
 const html=renderGenerator(instanceView('generator',d,true,0));
 const control=outputSwitch(html);
 assert.match(control.attrs,new RegExp(`\\baria-checked="${enabled}"`));
 assert.match(control.attrs,/\bdisabled\b/);
 assert.match(html,/Output (on|off)[^<]*Previous/);
 assert.match(control.attrs,/aria-description="Previous output readback; Sync to verify the current state\."/);
});

for(const [name,local,edit]of [
 ['generator settings',{generatorSettingsPending:true},()=>{}],
 ['unknown operation',{unknown:true},()=>{}],
 ['active request',{},d=>{d.active_request_id='request-1';}],
 ['pending request',{},d=>{d.pending_request_id='request-1';}],
 ['readback request',{},d=>{d.readback_request_id='request-1';}],
 ['safety request',{},d=>{d.safety_request_id='request-1';}],
])test(`generator output switch is disabled during ${name}`,()=>{
 const d=domain();edit(d);
 const control=outputSwitch(renderGenerator(instanceView('generator',d,true,0,local)));
 assert.match(control.attrs,/\bdisabled\b/);
 assert.match(control.attrs,/\baria-checked="true"/);
});

for(const enabled of [false,true])test(`generator held controls ${enabled?'permit OFF for confirmed ON':'cannot enable confirmed OFF'}`,()=>{
 const d=domain();d.state='STOP_HELD';d.device.channels[0].output_enabled=enabled;
 const control=outputSwitch(renderGenerator(instanceView('generator',d,true,0)));
 assert.match(control.attrs,new RegExp(`\\baria-checked="${enabled}"`));
 if(enabled)assert.doesNotMatch(control.attrs,/\bdisabled\b/);
 else assert.match(control.attrs,/\bdisabled\b/);
});

test('generator shows confirmed settings directly in controls without a duplicate readback summary',()=>{
 const html=renderGenerator(instanceView('generator',domain(),true,0));
 assert.match(html,/id="generator-1-frequency"[^>]*value="1250"/);
 assert.match(html,/id="generator-1-amplitude"[^>]*value="2"/);
 assert.match(html,/id="generator-1-offset"[^>]*value="0"/);
 assert.doesNotMatch(html,/<p\b[^>]*>[\s\S]*?Readback:|generator-readback/);
});

for(const [name,generatorActivity]of [
 ['idle',undefined],
 ['successfully completed',finishActivity(startActivity('read_status','sync',0),'complete',300)],
 ['completed at timestamp zero',finishActivity(startActivity('read_status','sync',0),'complete',0)],
])test(`generator ${name} page has no completion banner or empty status region`,()=>{
 const html=renderGenerator(instanceView('generator',domain(),true,0,{generatorActivity}));
 assert.doesNotMatch(html,/operation-feedback|generator-status/);
});

test('generator pending work keeps honest waiting feedback',()=>{
 const html=renderGenerator(instanceView('generator',domain(),true,0,{generatorActivity:startActivity('configure_signal','instrument',0)}));
 assert.match(html,/activity-spinner/);assert.match(html,/Waiting for instrument/);assert.match(html,/data-activity-elapsed="0"/);
});

for(const [outcome,label]of [['failed','Operation failed'],['unknown','Outcome unknown'],['cancelled','Export cancelled']])test(`generator ${outcome} outcome remains visible`,()=>{
 const generatorActivity=finishActivity(startActivity('read_status','sync',0),outcome,300);
 const html=renderGenerator(instanceView('generator',domain(),true,0,{generatorActivity}));
 assert.match(html,new RegExp(label));assert.match(html,/generator-status/);
});

test('generator recovery and held-control notices remain visible without a success banner',()=>{
 const d=domain();d.state='STOP_HELD';
 const html=renderGenerator(instanceView('generator',d,true,0,{generatorActivity:finishActivity(startActivity('read_status','sync',0),'complete',300),generatorRecoveryMarkup:'<div class="alert">Check original operation</div>'}));
 assert.match(html,/Check original operation/);assert.match(html,/Resume controls/);assert.doesNotMatch(html,/Operation finished/);
});
