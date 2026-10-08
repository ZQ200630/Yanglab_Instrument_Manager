import {previewStageMove} from './view-model.js';
export function instanceView(kind,domain,owned,ageUpper,local={}){
  const context=domain?.context;
  // A released connection may retain its last sample in scheduler metadata.
  // It cannot initialize live controls or defaults for the next connection.
  const disconnected=domain?.state==='DISCONNECTED'&&context?.connection_id===null;
  const device=disconnected||domain?.device==null?null:structuredClone(domain.device);
  const known=Number.isFinite(ageUpper)&&ageUpper>=0;
  if(device){device.timing=known?{roundTripMs:ageUpper,receivedAtMs:performance.now()}:null;
    if(kind==='laser'&&known)device.sample_age_s=Number.isFinite(device.sample_age_s)?Math.max(0,device.sample_age_s)+ageUpper/1000:null;
    if(!known){for(const field of Object.values(device.fields||{}))field.quality='unknown';device.sample_age_s=null;}}
  const role={context,confirmed:owned,mode:domain?.state||'UNKNOWN',backendMode:domain?.state,
    normalPending:local.pending||domain?.active_request_id||domain?.pending_request_id||domain?.readback_request_id,
    safetyPending:domain?.safety_request_id,hostRestricted:!owned,unknown:!known||local.unknown,
    stopHeld:domain?.state==='STOP_HELD',safety:domain?.safety,safetyEpoch:context?.epoch};
  role.canConnect=Boolean(owned&&domain?.state==='DISCONNECTED'&&context?.connection_id===null&&
    !device&&!role.normalPending&&!role.safetyPending&&!local.unknown);
  return {...local,client:true,mode:local.mode,status:{devices:{[kind]:device}},roles:{[kind]:role},nowMs:performance.now(),busy:Boolean(local.pending)};
}
function number(get,id,min,max){const text=get(id);if(typeof text!=='string'||!text.trim())throw new Error('Enter a target value');const n=Number(text);if(!Number.isFinite(n)||n<min||n>max)throw new Error(`Target must be ${min}–${max}`);return n;}
export function actionFor(op,get,data={}){
  if(op==='laser-scan-start')return {name:'start_scan',args:{start_nm:number(get,'laser-scan-start',1,5000),stop_nm:number(get,'laser-scan-stop',1,5000),speed_nm_s:number(get,'laser-scan-speed',.01,20),return_speed_nm_s:number(get,'laser-scan-return-speed',.01,20),confirm:true}};
  if(op==='laser-scan-stop')return {name:'stop_scan',args:{confirm:true}};
  if(op==='laser-scan-forward'||op==='laser-scan-backward')return {name:op==='laser-scan-forward'?'scan_forward':'scan_backward',args:{target_nm:number(get,op==='laser-scan-forward'?'laser-scan-stop':'laser-scan-start',1,5000),speed_nm_s:number(get,op==='laser-scan-forward'?'laser-scan-speed':'laser-scan-return-speed',.01,20),confirm:true}};
  if(op==='laser-read')return {name:'read_status',args:{}};
  if(op==='laser-wavelength')return {name:'move_wavelength',args:{wavelength_nm:number(get,'laser-wavelength',1,5000),confirm:true}};
  if(op==='laser-goto')return {name:'goto_wavelength',args:{wavelength_nm:number(get,'laser-wavelength',1,5000),confirm:true}};
  if(op==='laser-piezo')return {name:'control_piezo',args:{percent:number(get,'laser-piezo',0,100),confirm:true}};
  if(op==='laser-remote'||op==='laser-local')return {name:'set_remote',args:{remote:op==='laser-remote',confirm:true}};
  if(op==='laser-output-on'||op==='laser-output-off')return {name:'control_output',args:{enabled:op==='laser-output-on',confirm:true}};
  if(op==='laser-tracking-on'||op==='laser-tracking-off')return {name:'control_tracking',args:{enabled:op==='laser-tracking-on',confirm:true}};
  op={'fiber-move':'stage-move','fiber-adopt':'stage-baseline'}[op]||op;
  if(op==='osa-acquire'||op==='osa-read'){const name=get('osa-name'),trace=get('osa-trace')||'A';if(!/^[A-Za-z0-9][A-Za-z0-9_-]{0,39}$/.test(name))throw new Error('Use a recording name of 1–40 letters, digits, hyphens or underscores, beginning with a letter or digit.');if(!/^[A-G]$/.test(trace))throw new Error('Select trace A–G');return {name:op==='osa-read'?'read_trace':'acquire',args:{trace,archive_name:name}};}
  if(op==='voltage-apply'){const channel=Number(data.channel);if(!Number.isInteger(channel)||channel<1||channel>8)throw new Error('Invalid channel');return {name:'set_channel',args:{channel,voltage:number(get,`voltage-${channel}`,0,14)}};}
  if(op==='voltage-zero')return {name:'zero',args:{}};
  if(op==='gain-set-temp')return {name:'set_temperature',args:{temperature_c:number(get,'gain-temp',15,40)}};
  if(op==='gain-set-current')return {name:'set_current',args:{current_ma:number(get,'gain-current',0,200)}};
  const gain={'gain-enable-tec':'enable_tec','gain-disable-tec':'disable_tec','gain-enable-current':'enable_current','gain-disable-current':'disable_current','gain-stable':'wait_stable'};
  if(gain[op])return {name:gain[op],args:{}};
  if(op==='stage-baseline')return {name:'adopt_baseline',args:{side:data.side,confirm:true,allow_nominal:true}};
  if(op==='stage-move'){const args={side:data.side,dx:number(get,`${data.side}-x`,-1,1),dy:number(get,`${data.side}-y`,-1,1),dz:number(get,`${data.side}-z`,-1,1)};
    const check=previewStageMove(data.side,{x:args.dx,y:args.dy,z:args.dz});if(!check.allowed)throw new Error(check.reason);return {name:'move',args};}
  throw new Error('Unsupported instrument action');
}
