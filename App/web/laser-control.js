export function singleScanRates(device){
 const rates=device?.single_scan_rates_nm_s,max=device?.max_scan_speed_nm_s;
 if(device?.single_scan_supported!==true||!Number.isFinite(max)||!Array.isArray(rates)||!rates.length||rates.length>32||
    rates.some((v,i)=>!Number.isFinite(v)||v<.01||v>max||rates.slice(0,i).some(prior=>Math.abs(prior-v)<=.000001)))return [];
 return [...rates].sort((a,b)=>a-b);
}
export function singleScanRateAllowed(device,value){
 const speed=typeof value==='string'&&value.trim()?Number(value):typeof value==='number'?value:NaN;
 const cap=device?.operating_max_speed_nm_s??device?.max_scan_speed_nm_s;
 return Number.isFinite(speed)&&Number.isFinite(cap)&&speed<=cap&&singleScanRates(device).some(v=>Math.abs(v-speed)<=.000001);
}
export function updateSingleScanButtons(root,device){
 for(const [action,input] of [['forward','laser-scan-speed'],['backward','laser-scan-return-speed']]){
  const button=root.querySelector(`[data-op="laser-scan-${action}"]`);if(!button)continue;
  button.disabled=button.dataset.singleReady!=='true'||button.hasAttribute('aria-busy')||!singleScanRateAllowed(device,root.querySelector('#'+input)?.value);
 }
}
export function laserRefreshDue({visible,connected,owned,synced,domain,local={},ageUpperMs,interval}){
 if(!visible||!connected||!owned||!synced||!Number.isFinite(ageUpperMs)||ageUpperMs<0||
    !domain?.context?.connection_id||domain?.state!=='READY'||!domain.device?.connected||domain.device.status_error)return false;
 if(['pending','queuedLaser','connecting','disconnecting','disconnectInFlight','unknown','limitsSaving','laserRefreshFailed'].some(k=>local[k])||
    ['active_request_id','pending_request_id','readback_request_id','safety_request_id'].some(k=>domain[k]))return false;
 const age=domain.device.sample_age_s;
 return Number.isFinite(age)&&age>=0&&age+ageUpperMs/1000>=interval;
}
export function operatingLimits(get,device){
 const range=device?.wavelength_range_nm,maxSpeed=device?.max_scan_speed_nm_s;
 if(!Array.isArray(range)||range.length!==2||!range.every(Number.isFinite)||!Number.isFinite(maxSpeed))throw new Error('Laser head limits are unavailable.');
 const values=['laser-limit-min','laser-limit-max','laser-limit-speed'].map(id=>{
  const raw=get(id);if(typeof raw!=='string'||!raw.trim())throw new Error('Enter all three operating limits.');
  const value=Number(raw);if(!Number.isFinite(value))throw new Error('Operating limits must be numbers.');return value;
 });
 const [min_nm,max_nm,max_speed_nm_s]=values;
 if(min_nm<range[0]||max_nm>range[1]||min_nm>=max_nm||max_speed_nm_s<.01||max_speed_nm_s>maxSpeed)
  throw new Error(`Operating limits must stay within ${range[0]}–${range[1]} nm and 0.01–${maxSpeed} nm/s.`);
 return {min_nm,max_nm,max_speed_nm_s};
}
