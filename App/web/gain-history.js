/** Bounded Host observations projected onto the browser monotonic display clock. */
export function appendGainHistory(previous,{domain,bootId,ageUpperMs,nowMs=performance.now()}){
 const context=domain?.context,field=domain?.device?.fields?.temperature_c;
 const identity=JSON.stringify([bootId,context?.session_id,context?.connection_id]);
 const old=previous||[],retained=old.length&&old.at(-1).identity!==identity?[]:old,last=retained.at(-1);
 if(!context?.connection_id)return retained;
 const age=Number.isFinite(field?.observed_age_s)&&Number.isFinite(ageUpperMs)?Math.max(0,field.observed_age_s)+Math.max(0,ageUpperMs)/1000:null;
 const fresh=field?.quality==='fresh'&&age!==null&&age<=5&&Number.isFinite(field.value)&&Number.isSafeInteger(field.revision)&&field.connection_id===context.connection_id&&domain.device?.connected===true&&['READY','ACTIVE'].includes(domain.device.state);
 const lastObserved=retained.findLast(point=>!point.gap);
 if(fresh&&lastObserved&&field.revision<=lastObserved.revision)return retained;
 const quality=fresh?'fresh':field?.quality==='error'?'error':field?.quality==='stale'||age>5?'stale':'unknown';
 if(!fresh&&last?.gap&&last.quality===quality&&last.revision===field?.revision)return retained;
 const observedAtMs=fresh?nowMs-age*1000:last?last.observedAtMs+.001:nowMs-(age!==null?age*1000:Number.isFinite(ageUpperMs)?Math.max(0,ageUpperMs):0);
 if(fresh&&last&&observedAtMs<last.observedAtMs)return retained;
 const point={identity,boot_id:bootId,session_id:context.session_id,connection_id:context.connection_id,revision:field?.revision??null,observedAtMs,temperature_c:fresh?field.value:null,quality};
 if(!fresh){point.gap=true;point.reason=field?.reason||field?.error||domain?.device?.fault||'Temperature observation unavailable';}
 const target=domain.device?.fields?.target_c;if(fresh&&target?.quality==='fresh'&&target.connection_id===context.connection_id&&target.revision===field.revision&&Number.isFinite(target.value))point.target_c=target.value;
 return [...retained,point].filter(item=>item.observedAtMs>=nowMs-960000).slice(-4801);
}
