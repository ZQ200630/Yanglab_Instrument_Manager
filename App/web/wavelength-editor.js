// Fixed digits and a bounded latest-target sender. No selectable backend.
export function formatTarget(value){return Number.isFinite(value)?value.toFixed(3).padStart(8,'0'):'----.---';}
export function laserMotion(device){return {...(device?.laser||{}),...(device?.motion||{}),operation_complete:device?.motion_pending?false:(device?.motion?.operation_complete??device?.laser?.operation_complete)};}
export function editTarget(text,position,key,range){
 const digits=[0,1,2,3,5,6,7];position=digits.includes(position)?position:7;
 if(key==='ArrowLeft'||key==='ArrowRight')return {value:Number(text),position:digits[Math.max(0,Math.min(6,digits.indexOf(position)+(key==='ArrowLeft'?-1:1)))]};
 let value;
 if(key==='ArrowUp'||key==='ArrowDown'){const exponent=position<4?3-position:4-position;value=Number(text)+(key==='ArrowUp'?1:-1)*10**exponent;}
 else if(/^[0-9]$/.test(key))value=Number(text.slice(0,position)+key+text.slice(position+1));
 else return null;
 value=Math.round(value*1000)/1000;
 if(!Number.isFinite(value)||!range||value<range[0]||value>range[1])throw Error('Target wavelength is outside the operating range.');
 return {value,position};
}
export function syncTarget(local,sample){
 if(!sample||local.targetSending||local.scanStarting||local.pending)return;
 if(local.laserScanning&&sample.operation_complete===true&&Number.isFinite(sample.wavelength_nm)){
  local.targetValue=sample.wavelength_nm;local.laserScanning=false;local.observedTarget=sample.wavelength_setpoint_nm;return;
 }
 if(!local.laserScanning&&Number.isFinite(sample.wavelength_setpoint_nm)&&sample.wavelength_setpoint_nm!==local.observedTarget){
  local.targetValue=sample.wavelength_setpoint_nm;local.observedTarget=sample.wavelength_setpoint_nm;
 }
}
export function createTargetQueue({send,waitReady,isCurrent,onError,onBusy=()=>{},delay=120}){
 let latest=null,running=false,timer=null,generation=0;
 async function drain(){
  timer=null;if(running)return;running=true;const version=generation;onBusy(true);
  try{while(latest!==null&&version===generation&&isCurrent()){
    await waitReady();if(version!==generation||!isCurrent())break;
    const value=latest;latest=null;await send(value);
  }}catch(cause){latest=null;onError(cause);}
  finally{running=false;if(version!==generation||!isCurrent())latest=null;onBusy(false);}
 }
 return {submit(value){latest=value;if(!running){clearTimeout(timer);timer=setTimeout(drain,delay);}},cancel(){generation++;latest=null;clearTimeout(timer);timer=null;if(!running)onBusy(false);}};
}
