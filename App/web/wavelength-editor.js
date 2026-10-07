// Fixed digits and a bounded latest-target sender. No selectable backend.
export function formatDigits(value,precision=3,whole=4){return Number.isFinite(value)?value.toFixed(precision).padStart(whole+precision+1,'0'):'-'.repeat(whole)+'.'+'-'.repeat(precision);}
export function formatTarget(value){return formatDigits(value);}
export function laserMotion(device){return {...(device?.laser||{}),...(device?.motion||{}),operation_complete:device?.motion_pending?false:(device?.motion?.operation_complete??device?.laser?.operation_complete)};}
export function editTarget(text,position,key,range){
 return editDigits(text,position,key,range);
}
export function editDigits(text,position,key,range,precision=3,whole=4){
 const digits=Array.from({length:whole+precision+1},(_,i)=>i).filter(i=>i!==whole);position=digits.includes(position)?position:digits.at(-1);
 if(key==='ArrowLeft'||key==='ArrowRight')return {value:Number(text),position:digits[Math.max(0,Math.min(digits.length-1,digits.indexOf(position)+(key==='ArrowLeft'?-1:1)))]};
 let value;
 if(key==='ArrowUp'||key==='ArrowDown'){const exponent=position<whole?whole-1-position:whole-position;value=Number(text)+(key==='ArrowUp'?1:-1)*10**exponent;}
 else if(/^[0-9]$/.test(key))value=Number(text.slice(0,position)+key+text.slice(position+1));
 else return null;
 value=Math.round(value*10**precision)/10**precision;
 if(!Number.isFinite(value)||!range||value<range[0]||value>range[1])throw Error('Value is outside the operating range.');
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
