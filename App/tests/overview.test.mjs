import test from 'node:test';import assert from 'node:assert/strict';import {overviewCounts,renderOverview,navigateTag} from '../web/overview.js';
test('overview has no configuration or add-new actions, even when empty or offline',()=>{
 for(const connected of [true,false]){
  const html=renderOverview({host_id:'a'.repeat(32),connected,registry:{devices:[],setups:[]}}, {get:()=>null,ageUpperMs:()=>null});
  assert.match(html,/Instrument overview/);assert.doesNotMatch(html,/Add New|Connect local Host|Host settings|href="#devices"/);
 }
});
test('counts_physical_devices_not_types_or_setup',()=>{const devices=[{device_id:'a',source:'LOCAL'},{device_id:'b',source:'LOCAL'}];assert.equal(overviewCounts(devices,[{members:['a','b']}],[]).local.total,2);assert.equal(overviewCounts(devices,[],[]).remote.total,0);});
test('tag_navigation_never_opens_hardware',()=>{const record={hostId:'a'.repeat(32),device_id:'b'.repeat(32)};assert.equal(navigateTag(record).deviceId,record.device_id);const html=renderOverview({host_id:record.hostId,connected:true,mode:'real',registry:{devices:[{...record,name:'OSA',model_id:'aq6370',params:{}}],setups:[]}}, {get:()=>null,ageUpperMs:()=>null});assert.match(html,/UNKNOWN/);assert.match(html,/LOCAL/);assert.match(html,/Remote Devices/);assert.doesNotMatch(html,/LOCAL SESSION|Confirm each action|5 instrument types/);});
test('physical card separates setup controller ownership and sample age',()=>{
 const id='b'.repeat(32),sid='c'.repeat(32),host={host_id:'a'.repeat(32),connected:true,registry:{devices:[{device_id:id,name:'Left',model_id:'mdt693b',params:{}}],setups:[{setup_id:sid,members:[id]}]},control:{['setup:'+sid]:{controller_session:'owner'}}};
 const html=renderOverview(host,{get:()=>({host_sample_ms:1,device:{}}),ageUpperMs:()=>2345});assert.match(html,/Control held/);assert.match(html,/Sample age: 2\.3 s/);
});
