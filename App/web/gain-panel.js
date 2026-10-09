import {gainEvidence,gainFields} from './control-state.js';
import {gainCanOperate,gainCanStart,gainCanStop} from './gain-control.js';
import {renderGainTrend} from './gain-trend.js';

export function renderGainPanel(state,{esc,pageHeader,connectionAction,empty,badge,roleProgress}){
 const device=state.status?.devices?.gain,role=state.roles?.gain,now=state.nowMs??performance.now();
 const automaticResume=role?.stopHeld&&gainCanOperate(role,device,now);
 const header=pageHeader('INSTRUMENT / THERMAL','Gain Chip Driver','Temperature and injection current, in one place.',automaticResume?'':connectionAction('gain',state));
 if(!device)return header+empty('gain');
 const evidence=Object.fromEntries(gainFields.map(name=>[name,gainEvidence(device,name,now)]));
 const normal=gainCanOperate(role,device,now),safety=gainCanStop(role),start=gainCanStart(role,device,now);
 const confirmed=name=>device.connected===true&&['READY','ACTIVE'].includes(device.state)&&evidence[name].quality==='fresh'&&evidence[name].connection_id===role?.context?.connection_id;
 const on=name=>confirmed(name)&&evidence[name].value===true;
 const known=name=>confirmed(name)&&typeof evidence[name].value==='boolean';
 const fieldValue=(name)=>{const f=evidence[name];return typeof f.value==='boolean'?(confirmed(name)?(f.value?'On':'Off'):'Unknown'):Number.isFinite(f.value)?f.value.toFixed(3):'Unknown';};
 const value=(id,fallback='')=>{const stored=state.inputs?.get(id);return esc(stored&&typeof stored==='object'?stored.value??fallback:stored??fallback);};
 const checked=id=>state.inputs?.get(id)?.checked??true;
 const buttons=(name,label,enabled,style='')=>`<button class="btn ${style}" data-op="${name}" ${enabled?'':'disabled'}>${label}</button>`;
 const stoppingNeeded=name=>on(name)||device.current_operation?.active||state.gainNormalFlow||['start_current','ramp_current','enable_current','enable_tec'].includes(state.pendingName)||Boolean(role?.safetyPending);
 const outputToggle=(name,label,canOn)=>known(name)?buttons(`gain-${stoppingNeeded(name)?'disable':'enable'}-${label.toLowerCase()}`,`${stoppingNeeded(name)?'Turn off':'Turn on'} ${label}`,stoppingNeeded(name)?safety:canOn,stoppingNeeded(name)?'danger':'primary'):
  `${buttons(`gain-enable-${label.toLowerCase()}`,`Turn on ${label}`,false,'primary')}${buttons(`gain-disable-${label.toLowerCase()}`,`Turn off ${label}`,safety,'danger')}`;
 const stale=gainFields.some(name=>!confirmed(name));
 const fault=device.state==='FAULT'||Boolean(device.fault);
 const faultText=typeof device.fault==='string'?device.fault:device.fault?.message;
 const notice=fault?String(faultText||device.status_error||device.observation_error||'Gain fault. Check the controller before reconnecting.')+' · Last readings shown.':device.status_error?String(device.status_error):device.connected!==true?'Disconnected · showing last readings.':role?.unknown?(state.gainRecoveryNotice?'':'Command outcome is unknown. Check its status before continuing.'):stale?'Readings are stale or unavailable. Normal controls resume when confirmed readings return.':!role?.confirmed||role?.hostRestricted?'Take control to change settings.':'';
 const pid=device.pid,validPid=pid?.quality==='fresh'&&pid.connection_id===role?.context?.connection_id&&Array.isArray(pid.values)&&pid.values.length===3&&pid.values.every(Number.isFinite);
 const op=device.current_operation,phases={checking_tec:'Checking TEC',waiting_stable:'Waiting for temperature to stabilize',enabling:'Enabling current',ramping:'Ramping current',verifying:'Verifying current',completed:'Current target reached',failed:'Current operation failed',canceled:'Current operation stopped'};
 const progress=op?.active?`<div class="gain-operation" role="status">
  <div>
  <span class="gain-operation-dot">
  </span>
  <strong>${esc(phases[op.phase]||'Working')}</strong>${op.phase==='ramping'&&Number.isFinite(op.current_ma)?`<span class="mono">${op.current_ma.toFixed(3)} mA</span>`:''}</div>
  <progress ${op.phase==='ramping'&&Number.isInteger(op.steps_completed)&&Number.isInteger(op.steps_total)&&op.steps_total>0?`value="${Math.min(op.steps_total,Math.max(0,op.steps_completed))}" max="${op.steps_total}"`:''}>
  </progress>
  </div>`:op?.error&&op.phase!=='canceled'?`<div class="gain-notice" role="status">${esc(op.error)}</div>`:'';
 const labels={temperature_c:'Temperature',target_c:'Target',current_ma:'Controller current setting',tec_enabled:'TEC',current_enabled:'Current output'};
 const metric=name=>`<div class="gain-status-reading" data-gain-field="${name}">
  <dt>${labels[name]}</dt>
  <dd>${esc(fieldValue(name))}${['temperature_c','target_c','current_ma'].includes(name)&&Number.isFinite(evidence[name].value)?`<small>${name==='current_ma'?'mA':'°C'}</small>`:''}</dd>
  </div>`;
 return header+(role?.safetyPending&&roleProgress?roleProgress("gain",state):"")+`<div class="gain-dashboard">${notice?`<div class="gain-notice" role="status">${esc(notice)}</div>`:''}<div class="gain-monitor-grid">
  <section class="card gain-trend-card" data-gain-panel="trend">
  <div class="card-head">
  <div>
  <h2 class="card-title">Temperature trend</h2>
  <div class="gain-chart-legend">
  <span>Temperature</span>
  <span>Target</span>
  </div>
  </div>
  <label class="gain-window">Window<select class="control" id="gain-trend-window-s" aria-label="Temperature trend time window">${[60,300,900].map(s=>`<option value="${s}" ${Number(state.gainTrendWindowS??300)===s?'selected':''}>${s/60} min</option>`).join('')}</select>
  </label>
  </div>
  <div class="card-body">${renderGainTrend(state.gainHistory||[],{nowMs:now,windowS:state.gainTrendWindowS??300})}</div>
  </section>
  <section class="card gain-status-card" data-gain-panel="status">
  <div class="card-head">
  <h2 class="card-title">Overall status</h2>${badge(fault?'FAULT':notice||role?.unknown?'Check status':device.state||'Unknown',fault?'fault':notice||role?.unknown?'warn':'ready')}</div>
  <div class="card-body">
  <dl class="gain-status-grid">${gainFields.map(metric).join('')}</dl>
  <p class="gain-status-note">${on('current_enabled')?'Current output is on.':on('tec_enabled')?'TEC is on · current output is off.':known('tec_enabled')&&known('current_enabled')?'Outputs are off.':'Output state requires confirmation.'}</p>
  </div>
  </section>
  </div>${progress}<div class="gain-control-grid">
  <section class="card gain-control-card" data-gain-panel="temperature">
  <div class="card-head">
  <h2 class="card-title">Temperature Control</h2>
  <span class="gain-control-unit">15–40 °C</span>
  </div>
  <div class="card-body">
  <div class="gain-target-block"><div class="gain-target-row">
  <div class="field">
  <label for="gain-temp">Target temperature (°C)</label>
  <input id="gain-temp" type="number" class="control gain-target" min="15" max="40" step="0.1" aria-describedby="gain-temp-keyboard" value="${value('gain-temp',Number.isFinite(evidence.target_c.value)?evidence.target_c.value:'')}">
  </div>${buttons('gain-set-temp','Apply temperature',normal)}</div>
  <p class="hint" id="gain-temp-keyboard">Enter to apply · Tab to switch to current</p></div>
  <div class="gain-output-row">
  <div>
  <strong>TEC output</strong>
  <p>Turning TEC off also turns current off first.</p>
  </div>
  <div class="gain-output-actions">${outputToggle('tec_enabled','TEC',normal)}</div>
  </div>
  <details class="gain-advanced" id="gain-pid-details">
  <summary>PID tuning</summary>
  <p class="hint">${validPid?'Controller coefficients · Enter to apply PID':'Read controller coefficients before editing.'}</p>
  <div class="gain-pid-row">${['p','i','d'].map((letter,i)=>`<div class="field">
  <label for="gain-pid-${letter}">${letter.toUpperCase()}</label>
  <input class="control" id="gain-pid-${letter}" type="number" min="0" max="999.999" step="0.001" value="${value(`gain-pid-${letter}`,validPid?pid.values[i]:'')}">
  </div>`).join('')}</div>
  <div class="form-actions">${buttons('gain-set-pid','Apply PID',normal&&validPid)}${buttons('gain-read-pid','Read PID',normal)}</div>
  </details>
  </div>
  </section>
  <section class="card gain-control-card" data-gain-panel="current">
  <div class="card-head">
  <h2 class="card-title">Current Control</h2>
  <span class="gain-control-unit">0–200 mA</span>
  </div>
  <div class="card-body">
  <div class="gain-target-block"><div class="gain-target-row">
  <div class="field">
  <label for="gain-current">Target current (mA)</label>
  <input id="gain-current" type="number" class="control gain-target" min="0" max="200" step="0.1" aria-describedby="gain-current-keyboard" value="${value('gain-current',Number.isFinite(evidence.current_ma.value)?evidence.current_ma.value:'')}">
  </div>${buttons('gain-set-current','Apply current',normal)}</div>
  <p class="hint" id="gain-current-keyboard">Enter to apply · Tab to switch to temperature</p></div>
  <div class="gain-output-row">
  <div>
  <strong>Current output</strong>
  <p>${on('tec_enabled')?'Start waits for temperature stability automatically.':'Turn TEC on before starting current.'}</p>
  </div>
  <div class="gain-output-actions">${outputToggle('current_enabled','current',start)}</div>
  </div>
  <div class="gain-options">
  <label>
  <input type="checkbox" id="gain-soft-start" ${checked('gain-soft-start')?'checked':''}> Soft start</label>
  <label>
  <input type="checkbox" id="gain-smooth-change" ${checked('gain-smooth-change')?'checked':''}> Smooth changes</label>
  </div>
  <details class="gain-advanced" id="gain-ramp-details">
  <summary>Ramp settings</summary>
  <div class="gain-ramp-row">
  <div class="field">
  <label for="gain-ramp-step">Step (mA)</label>
  <input class="control" type="number" id="gain-ramp-step" min="0.001" max="1" step="0.001" value="${value('gain-ramp-step',1)}">
  </div>
  <div class="field">
  <label for="gain-ramp-interval-ms">Interval (ms)</label>
  <input class="control" type="number" id="gain-ramp-interval-ms" min="50" max="180000" step="10" value="${value('gain-ramp-interval-ms',100)}">
  </div>
  <div class="field">
  <label for="gain-stable-timeout">Start timeout (s)</label>
  <input class="control" type="number" id="gain-stable-timeout" min="0.05" max="180" step="1" value="${value('gain-stable-timeout',30)}">
  </div>
  </div>
  <p class="hint">Current starts after temperature stays within ±0.2 °C for 5 s. Start timeout covers the whole operation. Controller activation starts at 3 mA, then ramps to your target.</p>
  </details>
  </div>
  </section>
  </div>
  </div>`;
}
