import test from 'node:test';
import assert from 'node:assert/strict';
import {laser} from '../web/panels.js';
import {actionFor} from '../web/instance-view.js';

const state=()=>({status:{devices:{laser:{connected:true,sample_age_s:.1,
  wavelength_range_nm:[1045,1085],operating_range_nm:[1059,1062],max_scan_speed_nm_s:10,operating_max_speed_nm_s:1,
  identity:{head_model:'6722-P',serial:'22500001',head_serial:'0953'},laser:{output_enabled:false,
  operation_complete:true,tracking:false,remote:false,wavelength_nm:1060,wavelength_setpoint_nm:1060}}}}});
test('compact status precedes full control; one output action and identity last',()=>{
 const html=laser(state());
 assert.match(html,/>Laser Status</);assert.match(html,/>Control</);
 assert.match(html,/data-op="laser-output-on"[^>]*>Laser Enable</);
 assert.doesNotMatch(html,/Select Remote|Select Local|data-op="laser-output-off"/);
 assert.ok(html.indexOf('id="laser-control-card"')>html.indexOf('id="laser-readings-card"'));
 assert.ok(html.indexOf('id="laser-device-information"')>html.indexOf('id="laser-control-card"'));
 assert.match(html,/id="laser-scan-start"[^>]*min="1059"[^>]*max="1062"/);
 assert.match(html,/id="laser-scan-speed"[^>]*max="1"/);
 assert.match(html,/Returns to Start/);assert.match(html,/Backward Velocity/);assert.doesNotMatch(html,/Set wavelength/);assert.match(html,/wavelength-digits/);
 const on=state();on.status.devices.laser.laser.output_enabled=true;
 const enabled=laser(on);assert.match(enabled,/>Laser Disable</);assert.doesNotMatch(enabled,/data-op="laser-output-on"/);
});
test('busy controller exposes Stop Scanning and disable; new motion stays disabled',()=>{
 const busy=state();Object.assign(busy.status.devices.laser.laser,{operation_complete:false,output_enabled:true});
 const html=laser(busy);
 assert.match(html,/data-op="laser-scan-stop">Stop Scanning</);
 assert.match(html,/data-op="laser-output-off">Laser Disable</);
 assert.match(html,/data-op="laser-scan-start" disabled/);
});
test('control actions use typed composites, never visible mode switching',()=>{
 assert.deepEqual(actionFor('laser-wavelength',()=> '1060'),{name:'move_wavelength',args:{wavelength_nm:1060,confirm:true}});
 assert.deepEqual(actionFor('laser-output-off',()=> ''),{name:'control_output',args:{enabled:false,confirm:true}});
 const get=id=>({'laser-scan-start':'1060','laser-scan-stop':'1061','laser-scan-speed':'0.5','laser-scan-return-speed':'1'})[id];
 assert.deepEqual(actionFor('laser-scan-start',get),{name:'start_scan',args:{start_nm:1060,stop_nm:1061,speed_nm_s:.5,return_speed_nm_s:1,confirm:true}});
 assert.deepEqual(actionFor('laser-scan-stop',get),{name:'stop_scan',args:{confirm:true}});
 assert.throws(()=>actionFor('laser-scan-start',()=> ''),/Enter/);
});
