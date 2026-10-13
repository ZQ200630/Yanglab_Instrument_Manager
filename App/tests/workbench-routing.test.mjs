import test from 'node:test';import assert from 'node:assert/strict';
import {createHostClient} from '../web/host-client.js';
import {createRemoteClient} from '../web/remote-client.js';
import {renderConsole} from '../web/console-ui.js';
import {createDeviceStore} from '../web/device-store.js';
import {actionFor} from '../web/instance-view.js';
import {renderInstrumentIcon} from '../web/instrument-icon.js';
const id=n=>String(n).repeat(32);
test('native workbench routes metadata jobs and local pickers through explicit APIs',async()=>{
 const calls=[],client=createHostClient(async(command,args)=>{calls.push([command,args]);if(command==='host_call')return{v:1,id:args.request.id,ok:true,result:{records:[]}};return{cancelled:true};},async()=>()=>{});
 assert.equal(typeof client.dataList,'function');await client.dataList({});await client.dataViewport({record_id:id(1),frame_id:id(2),channel:'CH1',max_points:2048});await client.analysisStart({request:{record_id:id(1)}});await client.experimentStart({definition:{id:id(3)}});await client.workbenchExport({record_ids:[id(1)],format:'npz'});
 assert.deepEqual(calls.filter(([name])=>name==='host_call').map(([,a])=>a.request.method),['data_list','data_viewport','analysis_start','experiment_start']);assert.equal(calls[4][0],'workbench_export');assert.equal(calls[4][1].params.format,'npz');
});
test('remote workbench calls preserve the owning Host without exposing arbitrary paths',async()=>{
 const calls=[],client=createRemoteClient(async(command,args)=>{calls.push([command,args]);if(command==='remote_call')return{v:1,id:args.request.id,ok:true,result:{records:[]}};return{cancelled:true};},async()=>()=>{},id(4));
 assert.equal(typeof client.dataList,'function');await client.dataList({});assert.equal(calls[0][1].hostId,id(4));assert.equal(calls[0][1].request.method,'data_list');
 await client.workbenchImport({append:true});assert.equal(calls[1][1].hostId,id(4));assert.deepEqual(calls[1][1].params,{append:true});
});
test('Analysis and Experiments navigation renders the real saved-data workbenches',()=>{
 const store=createDeviceStore(),host={host_id:id(1),connected:true,registry:{devices:[],setups:[]}};
 assert.match(renderConsole('#analysis',host,store,{'#analysis':{records:[]}}),/<h1>Analysis<\/h1>/);
 assert.match(renderConsole('#experiments',host,store,{'#experiments':{experiments:[],runs:[]}}),/<h1>Experiments<\/h1>/);
});
test('new real model instances select direct native family pages and actions',()=>{
 const hostId=id(1),deviceId=id(2),boot=id(3),store=createDeviceStore();
 for(const [model,title]of [['ds7054','Oscilloscope'],['mho5054','Oscilloscope'],['dg4162','Function Generator'],['dg4202','Function Generator'],['dg922pro','Function Generator']]){
  store.apply({type:'snapshot',host_id:hostId,boot_id:boot,seq:1,data:{host_id:hostId,mode:'real',registry:{devices:[{device_id:deviceId,model_id:model,name:model,config_rev:1}],setups:[]},domains:{['device:'+deviceId]:{state:'DISCONNECTED',device:null,context:{session_id:id(4),domain:{kind:'device',id:deviceId},connection_id:null,epoch:0}}}}});
  assert.match(renderConsole(`#/host/${hostId}/device/${deviceId}`,store.host(hostId),store),new RegExp(`<h1>${title}</h1>`));
 }
 assert.deepEqual(actionFor('scope-read',()=>''),{name:'read_status',args:{}});assert.deepEqual(actionFor('generator-output-off',()=>'',{channel:1}),{name:'set_output',args:{channel:1,enabled:false}});
});
test('scope and generator icons identify their instrument family in setup and overview',()=>{
 assert.match(renderInstrumentIcon('ds7054'),/data-instrument-icon="scope"/);assert.match(renderInstrumentIcon('dg922pro'),/data-instrument-icon="generator"/);
 assert.notEqual(renderInstrumentIcon('ds7054'),renderInstrumentIcon('dg4162'));
});
test('GUI file status and cancel are native window-bound commands, not Host RPCs',async()=>{
 const calls=[],client=createHostClient(async(command,args)=>{calls.push([command,args]);return{id:id(5),status:'running'};},async()=>()=>{});
 assert.equal(typeof client.workbenchFileStatus,'function');await client.workbenchFileStatus({job_id:id(5)});await client.workbenchFileCancel({job_id:id(5)});
 assert.deepEqual(calls,[['workbench_file_status',{jobId:id(5)}],['workbench_file_cancel',{jobId:id(5)}]]);
});
test('paired remote native file jobs retain owning Host identity for status and cancellation',async()=>{
 const calls=[],client=createRemoteClient(async(command,args)=>{calls.push([command,args]);return{id:id(5),status:'running'};},async()=>()=>{},id(4));
 await client.workbenchFileStatus({job_id:id(5)});await client.workbenchFileCancel({job_id:id(5)});
 assert.deepEqual(calls,[['workbench_file_status',{hostId:id(4),jobId:id(5)}],['workbench_file_cancel',{hostId:id(4),jobId:id(5)}]]);
});
