import {esc} from './panels.js';
export function serialFamily(port){
 if(port?.vid===0x1a86&&[0x7523,0x5523].includes(port.pid))return 'ch340';
 if(port?.vid===0x10c4&&[0xea60,0xea63,0xea70,0xea71,0xea7a,0xea7b].includes(port.pid))return 'cp210x';
 return null;
}
export function serialCandidates(ports,modelId){return ports.filter(p=>modelId==='gain'?Boolean(serialFamily(p)):modelId==='voltage'?serialFamily(p)==='ch340':modelId==='mdt693b'?p.vid===0x0403:true);}
export function validPort(port){return typeof port==='string'&&/^COM[1-9]\d{0,4}$/.test(port)&&Number(port.slice(3))<=65535;}
export function serialChoiceReady(d,profile,now=performance.now()){
 if(profile?.access!=='serial')return true;
 if(!validPort(d.params?.port))return false;
 if(d.manualPort)return true;
 const scan=d.serialScan;return Boolean(scan?.state==='ready'&&scan.modelId===d.modelId&&scan.profileId===d.profileId&&now>=scan.issued&&now-scan.issued<60000&&scan.ports.some(p=>p.resource===d.params.port));
}
export function renderSerialField(d,advanced=''){
 const scan=d.serialScan,pending=scan?.state==='checking',selected=scan?.ports.find(p=>p.resource===d.params?.port),visible=d.showAllPorts?scan?.ports:scan?.candidates;
 const ports=selected&&!visible?.some(p=>p.resource===selected.resource)?[...(visible||[]),selected]:visible;
 const label=p=>`${p.resource} · ${{ch340:'CH340 / CH341',cp210x:'CP210x (CP2102)'}[serialFamily(p)]||p.description||'Serial port'}${p.serial?' · S/N '+p.serial:''}${serialFamily(p)&&p.description?' · '+p.description:''}`;
 return `<div class="serial-picker"><label>Serial port${d.manualPort?`<input class="control" data-param="port" value="${esc(d.params?.port||'')}" placeholder="COM4">`:`<select class="control" data-param="port" data-managed="true" ${pending?'disabled':''}><option value="">${pending?'Scanning serial ports…':'Select serial port'}</option>${(ports||[]).map(p=>`<option value="${esc(p.resource)}" ${p.resource===d.params?.port?'selected':''}>${esc(label(p))}</option>`).join('')}</select>`}</label><button class="btn small" data-ui="scan-serial" ${pending?'disabled':''}>${pending?'Scanning…':'Refresh serial ports'}</button>${scan?.state==='failed'?`<p class="alert">${esc(scan.message)}</p>`:scan?.state==='ready'&&!scan.candidates.length?'<p class="hint">No matching adapter detected. Check the USB cable or refresh.</p>':'<p class="hint">Suggested USB adapters. Verification checks the instrument identity.</p>'}<details id="serial-advanced" class="serial-advanced" ${d.manualPort||d.showAllPorts?'open':''}><summary>Advanced connection settings</summary><div class="form-actions"><button class="btn small" data-ui="serial-show-all">${d.showAllPorts?'Show matching ports':'Show all serial ports'}</button><button class="btn small" data-ui="serial-manual">${d.manualPort?'Use detected ports':'Enter port manually'}</button></div>${advanced}</details></div>`;
}
