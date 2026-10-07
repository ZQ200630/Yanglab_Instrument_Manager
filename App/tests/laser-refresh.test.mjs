import test from 'node:test';
import assert from 'node:assert/strict';
import {laserRefreshDue,operatingLimits} from '../web/laser-control.js';
const ready=()=>({visible:true,connected:true,owned:true,synced:true,interval:30,ageUpperMs:100,
 domain:{state:'READY',context:{connection_id:'c'},device:{connected:true,sample_age_s:31}},local:{}});
test('foreground polling is due only for a healthy owned idle connected laser',()=>{
 assert.equal(laserRefreshDue(ready()),true);
 for(const [field,value] of [['visible',false],['connected',false],['owned',false],['synced',false],['ageUpperMs',null]]){
  assert.equal(laserRefreshDue({...ready(),[field]:value}),false,field);
 }
 for(const flag of ['pending','connecting','disconnecting','disconnectInFlight','unknown','limitsSaving','laserRefreshFailed']){
  assert.equal(laserRefreshDue({...ready(),local:{[flag]:true}}),false,flag);
 }
 for(const flag of ['active_request_id','pending_request_id','readback_request_id','safety_request_id']){
  const state=ready();state.domain[flag]='r';assert.equal(laserRefreshDue(state),false,flag);
 }
 const state=ready();state.domain.device.sample_age_s=1;assert.equal(laserRefreshDue(state),false);
 state.domain.device.sample_age_s=null;assert.equal(laserRefreshDue(state),false);
});
test('operator limits reject empty, reversed, too wide and excessive speed before reconnect',()=>{
 const device={wavelength_range_nm:[1045,1085],max_scan_speed_nm_s:10};
 const get=id=>({'laser-limit-min':'1050','laser-limit-max':'1080','laser-limit-speed':'1'})[id];
 assert.deepEqual(operatingLimits(get,device),{min_nm:1050,max_nm:1080,max_speed_nm_s:1});
 for(const values of [['','1080','1'],['1080','1050','1'],['1044','1080','1'],['1050','1080','11']]){
  const ids=['laser-limit-min','laser-limit-max','laser-limit-speed'];
  assert.throws(()=>operatingLimits(id=>values[ids.indexOf(id)],device));
 }
});
