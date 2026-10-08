import test from 'node:test';
import assert from 'node:assert/strict';
import {laser} from '../web/panels.js';
import {actionFor} from '../web/instance-view.js';

const state=()=>({status:{devices:{laser:{connected:true,sample_age_s:.1,
  wavelength_range_nm:[1045,1085],operating_range_nm:[1059,1062],max_scan_speed_nm_s:10,operating_max_speed_nm_s:1,
  identity:{head_model:'6722-P',serial:'22500001',head_serial:'0953'},laser:{output_enabled:false,
  operation_complete:true,tracking:false,remote:false,wavelength_nm:1060,wavelength_setpoint_nm:1060}}}}});
test('four scan controls describe their endpoints and expose configurable shortcut settings',()=>{
 const html=laser(state());
 for(const label of ['Full Scan','Forward Scan','Backward Scan','Stop Scan'])assert.ok(html.includes('>'+label+'</'),label);
 assert.match(html,/Start → Stop → Start/);assert.match(html,/Current → Stop/);assert.match(html,/Current → Start/);
 assert.match(html,/data-scan-shortcuts-enabled/);assert.match(html,/Shortcut: Esc/);
 const get=id=>({'laser-scan-start':'1060','laser-scan-stop':'1061','laser-scan-speed':'0.5','laser-scan-return-speed':'0.8'})[id];
 assert.deepEqual(actionFor('laser-scan-forward',get),{name:'scan_forward',args:{target_nm:1061,speed_nm_s:.5,confirm:true}});
 assert.deepEqual(actionFor('laser-scan-backward',get),{name:'scan_backward',args:{target_nm:1060,speed_nm_s:.8,confirm:true}});
});
test('single scans remain unavailable until native identity-specific qualification is present',()=>{
 const s=state();assert.match(laser(s),/data-op="laser-scan-forward" disabled/);assert.match(laser(s),/data-op="laser-scan-backward" disabled/);
 s.status.devices.laser.single_scan_supported=true;
 assert.doesNotMatch(laser(s),/data-op="laser-scan-(?:forward|backward)" disabled/);
 s.status.devices.laser.laser.operation_complete=false;
 assert.match(laser(s),/data-op="laser-scan-forward" disabled/);assert.match(laser(s),/data-op="laser-scan-stop">Stop Scan</);
});
test('initial scan fields contain bounded defaults instead of requiring an empty Stop',()=>{
 const s=state(),values=html=>Object.fromEntries([...html.matchAll(/id="(laser-scan-(?:start|stop))"[^>]*value="([^"]*)"/g)].map(m=>[m[1],m[2]]));
 assert.deepEqual(values(laser(s)),{'laser-scan-start':'1060.000','laser-scan-stop':'1062.000'});
 s.status.devices.laser.laser.wavelength_setpoint_nm=1080;
 assert.deepEqual(values(laser(s)),{'laser-scan-start':'1062.000','laser-scan-stop':'1059.000'},'an out-of-range target is bounded and the scan has a distinct endpoint');
 s.status.devices.laser.operating_range_nm=[1059.0004,1062.0004];
 delete s.status.devices.laser.laser.wavelength_setpoint_nm;
 assert.deepEqual(values(laser(s)),{'laser-scan-start':'1059.001','laser-scan-stop':'1062.000'},'defaults fit inside fractional limits at the input precision');
 s.status.devices.laser.operating_range_nm=[1059,1062];
 s.status.devices.laser.laser.wavelength_setpoint_nm=1061.999;
 assert.deepEqual(values(laser(s)),{'laser-scan-start':'1061.999','laser-scan-stop':'1059.000'},'near the upper bound, return to the opposite endpoint to meet the native minimum span');
 s.status.devices.laser.operating_range_nm=[1060,1060.015];
 s.status.devices.laser.laser.wavelength_setpoint_nm=1060.007;
 assert.deepEqual(values(laser(s)),{'laser-scan-start':'1060.000','laser-scan-stop':'1060.015'},'a narrow range uses both endpoints when the current midpoint cannot span 0.01 nm');
 s.status.devices.laser.operating_range_nm=[1060,1060.005];
 assert.match(laser(s),/data-op="laser-scan-start" disabled/,'insufficient range cannot offer a scan that the native driver will reject');
 assert.match(laser(s),/Scanning needs at least 0.01 nm/);
});
test('compact status precedes full control; one output action and identity last',()=>{
 const html=laser(state());
 assert.match(html,/>Laser Status</);assert.match(html,/>Control</);
 assert.match(html,/data-op="laser-output-on"[^>]*>Laser Enable</);
 assert.doesNotMatch(html,/Select Remote|Select Local|data-op="laser-output-off"/);
 assert.ok(html.indexOf('id="laser-control-card"')>html.indexOf('id="laser-readings-card"'));
 assert.ok(html.indexOf('id="laser-device-information"')>html.indexOf('id="laser-control-card"'));
 assert.ok(html.indexOf('class="laser-manual"')<html.indexOf('class="laser-scan"'),'manual controls are the left column');
 assert.ok(html.indexOf('data-op="laser-output-on"')>html.indexOf('class="laser-manual"'),'output belongs to the manual column');
 for(const id of ['laser-scan-start','laser-scan-stop','laser-scan-speed','laser-scan-return-speed'])assert.match(html,new RegExp('id="'+id+'"[^>]*data-digits='));
 assert.match(html,/data-op="laser-goto"[^>]*>Goto Wavelength/);
 assert.match(html,/Enter/);assert.doesNotMatch(html,/data-op="laser-tracking-/);
 assert.match(html,/id="laser-scan-start"[^>]*min="1059"[^>]*max="1062"/);
 assert.match(html,/id="laser-scan-speed"[^>]*max="1"/);
 assert.doesNotMatch(html,/Returns to Start|select digit|Updating…/);assert.match(html,/Backward Velocity/);assert.doesNotMatch(html,/Set wavelength/);assert.match(html,/wavelength-digits/);
 const on=state();on.status.devices.laser.laser.output_enabled=true;
 const enabled=laser(on);assert.match(enabled,/>Laser Disable</);assert.doesNotMatch(enabled,/data-op="laser-output-on"/);
 const ready=state();ready.status.devices.laser.target_following_enabled=true;
 assert.match(laser(ready),/data-op="laser-goto"[^>]*>Goto Wavelength/);
});
test('ordinary pending reads never lock drafts, emission or Stop; pending movement locks only new motion',()=>{
 const s=state();s.roles={laser:{context:{session_id:'s',connection_id:'c'},confirmed:true,mode:'READY',normalPending:'r',unknown:false}};
 s.pending='r';s.pendingName='read_status';s.status.devices.laser.laser.operation_complete=false;
 let html=laser(s);
 assert.doesNotMatch(html,/id="laser-wavelength"[^>]* disabled/);
 assert.match(html,/data-op="laser-output-on">Laser Enable/);
 assert.match(html,/data-op="laser-scan-stop">Stop Scan/);
 assert.match(html,/data-op="laser-goto" disabled/);
 assert.match(html,/Use Stop Scan/);
 s.status.devices.laser.laser.operation_complete=true;
 assert.doesNotMatch(laser(s),/data-op="laser-scan-start" disabled/,'readonly refresh serializes rather than banning a new move');
 s.roles.laser.unknown=true;
 html=laser(s);assert.match(html,/data-op="laser-output-on" disabled/);assert.match(html,/data-op="laser-scan-stop" disabled/);
});
test('every head starts both scan velocities at 0.1 with a lower ceiling preserved',()=>{
 for(const [range,speed] of [[[765,781],2],[[1045,1085],10],[[1510,1630],20]]) {
  const s=state(),d=s.status.devices.laser;d.wavelength_range_nm=d.operating_range_nm=range;d.max_scan_speed_nm_s=d.operating_max_speed_nm_s=speed;
  for(const id of ['laser-scan-speed','laser-scan-return-speed'])assert.match(laser(s),new RegExp('id="'+id+'"[^>]*value="00.10"'));
 }
 const s=state();s.status.devices.laser.operating_max_speed_nm_s=.05;assert.match(laser(s),/id="laser-scan-speed"[^>]*value="00.05"/);
 assert.deepEqual(actionFor('laser-goto',()=> '1060.2'),{name:'goto_wavelength',args:{wavelength_nm:1060.2,confirm:true}});
});
test('busy controller exposes Stop Scan and disable; new motion stays disabled',()=>{
 const busy=state();Object.assign(busy.status.devices.laser.laser,{operation_complete:false,output_enabled:true});
 const html=laser(busy);
 assert.match(html,/data-op="laser-scan-stop">Stop Scan</);
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
