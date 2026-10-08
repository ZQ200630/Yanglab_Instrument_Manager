import test from 'node:test';
import assert from 'node:assert/strict';
import {createHostClient} from '../web/host-client.js';
import {renderSettings} from '../web/setup.js';
import {installDriverPackage} from '../web/setup-actions.js';
import {renderOverview} from '../web/overview.js';
import {createDeviceStore} from '../web/device-store.js';

test('native Host bridge retains remote pairing and all Laser/driver methods together',async()=>{
 const calls=[],client=createHostClient(async(name,{request}={})=>{calls.push(request?.method||name);return request?{v:1,id:request.id,ok:true,result:{}}:{}},async()=>()=>{});
 await client.startHost({pythonPath:'ignored.exe'});await client.driverStatus();await client.scanLasers();await client.installDriver('cp210x');await client.driverInstallStatus();await client.refreshDevice({device_id:'device'});await client.remoteStatus();await client.remoteListener('127.0.0.1:9443');await client.approvePeer('peer');
 assert.deepEqual(calls,['host_start','driver_status','scan_lasers','install_driver','driver_install_status','refresh_device','remote_status','remote_listener','remote_approve']);
});
test('native settings retain compact driver rows and paired connections without interpreter configuration',()=>{
 const html=renderSettings({connected:true,registry:{settings:{}},driverInventory:{newport:{sdk:{state:'ready'},devices:[]},usb_serial:{ch340:{state:'not_detected'},cp210x:{state:'missing'}},serial:[],visa:{},errors:{},fiber:{left:null,right:null},suggestions:{}}},{});
 assert.match(html,/Connections/);assert.match(html,/Control other PCs/);assert.match(html,/data-driver-row="cp210x"/);assert.match(html,/Silicon Labs driver license/);assert.doesNotMatch(html,/Anaconda|Python|host-python|backend/i);
});
test('remote Laser overview displays the owner and detected controller key',()=>{
 const store=createDeviceStore(),h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32);
 store.apply({type:'snapshot',host_id:h,boot_id:b,seq:1,data:{host_id:h,host_name:'Laser PC',registry:{devices:[{device_id:d,name:'1060 nm',model_id:'tlb6700',params:{device_key:'6700 SN1012'}}],setups:[],drafts:[]},domains:{},control:{}}});store.setLocation(h,true);
 const html=renderOverview(null,store);assert.match(html,/Laser PC/);assert.match(html,/6700 SN1012/);assert.match(html,/REMOTE/);assert.match(html,new RegExp('#/host/'+h+'/device/'+d));
});
for(const stage of ['inventory','launch','wait','poll','postcheck'])test('driver installation stops when local Host changes during '+stage,async()=>{
 let current=true,reads=0;const calls=[];
 const client={installDriver:async()=>{calls.push('install');if(stage==='launch')current=false;return {state:'running',driver:'cp210x'};},driverInstallStatus:async()=>{calls.push('poll');if(stage==='poll')current=false;return {state:'completed',driver:'cp210x'};}};
 const readState=async()=>{calls.push('inventory');if(++reads===1){if(stage==='inventory')current=false;return {state:'missing'};}if(stage==='postcheck')current=false;return {state:'ready'};};
 await assert.rejects(installDriverPackage('cp210x',client,readState,{isCurrent:()=>current,wait:async()=>{calls.push('wait');if(stage==='wait')current=false;},now:()=>0}),/Host changed/);
 assert.deepEqual(calls,stage==='inventory'?['inventory']:stage==='launch'?['inventory','install']:stage==='wait'?['inventory','install','wait']:stage==='poll'?['inventory','install','wait','poll']:['inventory','install','wait','poll','inventory']);
});
