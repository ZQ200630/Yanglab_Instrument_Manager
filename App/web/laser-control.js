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
