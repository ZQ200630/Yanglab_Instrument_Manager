import test from 'node:test';
import assert from 'node:assert/strict';
import {renderConsole} from '../web/console-ui.js';

function fixture({disconnectFailed=false,retained=false}={}){
 const h='a'.repeat(32),d='b'.repeat(32),key=h+'/device/'+d,domain={kind:'device',id:d};
 const context={session_id:'c'.repeat(32),domain,connection_id:'d'.repeat(32),epoch:1};
 const values={temperature_c:24,target_c:24,current_ma:0,tec_enabled:false,current_enabled:false};
 const device={connected:true,state:'READY',fields:Object.fromEntries(Object.entries(values).map(([name,value])=>[name,{value,quality:'fresh',observed_age_s:0,connection_id:context.connection_id,revision:1}]))};
 const state={domain,context,device,state:'READY',pending:0,host_sample_ms:0};
 const host={host_id:h,connected:true,synced:true,mode:'real',registry:{devices:[{device_id:d,model_id:'gain',name:'Gain Chip Driver',config_rev:1}],setups:[]},control:{['device:'+d]:{state:retained?'RETAINED':'CONTROLLED'}}};
 const store={get:()=>state,host:()=>host,canControl:()=>!retained,ageUpperMs:()=>0,history:()=>[],lease:()=>({})};
 const local={[key]:{unknown:true,disconnectFailed,operationAttempt:{request_id:'e'.repeat(32),domain,context}}};
 return renderConsole('#/host/'+h+'/device/'+d,host,store,local,{models:[{id:'gain'}]});
}

test('Gain operation uncertainty has one actionable notice and does not claim failed release',()=>{
 const html=fixture();
 assert.equal((html.match(/Operation result is uncertain/g)||[]).length,1);
 assert.doesNotMatch(html,/Command outcome is unknown/);
 assert.match(html,/data-ui="query-original"/);
 assert.match(html,/>Operation unconfirmed</);
 assert.doesNotMatch(html,/Release unconfirmed|Retry disconnect/);
 assert.match(html,/>Check status</);
});

test('Actual retained or failed Gain disconnect continues to say release unconfirmed',()=>{
 for(const options of [{retained:true},{disconnectFailed:true}]){
  const html=fixture(options);
  assert.match(html,/>Release unconfirmed</);
  assert.match(html,/>Retry disconnect</);
 }
});
