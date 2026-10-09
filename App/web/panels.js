import {renderGainPanel} from './gain-panel.js';
import {formatTarget,formatDigits,laserMotion} from './wavelength-editor.js';
import {singleScanRates,singleScanRateAllowed} from './laser-control.js';
import {defaultScanShortcutPreferences,renderScanShortcutHint,renderScanShortcuts} from './scan-shortcuts.js';
import { describeStage, previewStageMove, sampleAgeLabel, voltageRows } from './view-model.js';
import {baselineConfirmations} from './operations.js';
import { renderPm400 } from './pm400.js';
import {decimateTrace} from './osa.js';
import { canSendNormal, canSendSafety, canResume, intentRank } from './control-state.js';
import {activityBusy,activityLabel} from './activity.js';

export const sections = [
  ['overview', 'Overview', '◈'], ['settings', 'Device setup', '⚙'],
  ['fiber', 'Fiber stages', '⌁'], ['osa', 'Spectrum analyzer', '⌁'],
  ['voltage', 'Voltage source', '▦'], ['gain', 'Gain Driver', '◉'],
  ['pm400', 'PM400 power meter', '◌'],
];

export const roleLabels = {
  osa: 'OSA AQ6370', voltage: 'Voltage Source', gain: 'Gain Chip Driver',
  pm400: 'PM400', fiber: 'Fiber Stages', laser: 'TLB-6700 laser',
};

export function esc(value) {
  return String(value ?? '').replace(/[&<>"']/g, (character) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[character]);
}

export function laser(state) {
 const device=state.status?.devices?.laser,sample=laserMotion(device),identity=device?.identity||{};
 const role=state.roles?.laser;
 const live=device?.connected===true&&!device.status_error&&!state.limitsSaving&&(!role||
   role.confirmed&&role.context?.connection_id&&role.mode==='READY'&&!role.unknown&&!role.hostRestricted&&!role.safetyPending&&!role.stopHeld);
 const readPending=['read_status','read_motion'].includes(state.pendingName);
 const idle=(!role?.normalPending&&!state.pending)||readPending;
 const readable=live&&idle;
 const ageKnown=Number.isFinite(device?.sample_age_s)&&device.sample_age_s>=0,interval=state.laserRefreshInterval||30;
 // Native preflight guards commands; ordinary display age never bans Stop.
 const range=device?.wavelength_range_nm,reviewed=Array.isArray(range)&&range.length===2&&range.every(Number.isFinite);
 const limitsEditable=readable&&reviewed&&!state.remote;
 const operating=device?.operating_range_nm||range,cap=device?.operating_max_speed_nm_s??device?.max_scan_speed_nm_s;
 const moving=['moving','holding','hold_timed_out','stopping'].includes(device?.move?.phase);
 const inheritedTracking=device?.move===null&&device?.motion_pending===false&&sample.operation_complete===false&&sample.tracking===true;
 const movementIdle=readable&&reviewed&&!moving&&!state.targetSending&&!state.scanStarting&&!state.queuedStop;
 const controls=movementIdle&&sample.operation_complete===true;
 // Typed motion composites verify a hold before taking over inherited tracking.
 // Piezo remains an ordinary OPC-gated control.
 const compositeControls=movementIdle&&(sample.operation_complete===true||inheritedTracking);
 const stopping=live&&!state.queuedStop;
 const targetEditable=device?.connected===true&&reviewed&&!state.limitsSaving;
 const restriction=role?.unknown||state.unknown?'The previous command outcome is unknown. Check its status before sending commands.':
   !live?'Connect and take control to send commands.':!reviewed?'Laser head limits are unavailable.':
   state.queuedStop?'Stop Scan is pending. Waiting for the current command and hold verification.':
   state.targetSending?'Goto is pending. Waiting for the current exchange and command confirmation.':
   state.scanStarting?'Scan start is pending. Waiting for the current exchange and command confirmation.':
   !idle?'A command is pending. New movement is available after its result is confirmed.':
   moving?'An App movement is active. Use Stop Scan to hold before starting another move.':
   device?.motion_pending?'Waiting for motion readback before starting another move.':
   inheritedTracking?'Tracking is already on. A new move will stop and verify a hold before starting.':
   sample.operation_complete===false?'Controller reports busy. Use Stop Scan to verify a hold before starting another move.':
   sample.operation_complete!==true?'Movement status is unknown. Refresh readings before starting a move.':'';
 const display=(v,u='')=>Number.isFinite(v)?`${v} ${u}`.trim():'Unknown';
 const toggle=(v,on,off)=>v===true?on:v===false?off:'Unknown';
 const button=(op,label,enabled=false,kind='',attributes='')=>`<button class="btn ${kind}" data-op="laser-${op}"${attributes}${enabled?'':' disabled'}>${label}</button>`;
 const reported=device?.connected===true&&ageKnown&&state.roles?.laser?.unknown!==true&&!device.status_error;
 const output=toggle(sample.output_enabled,'Output enabled','Output disabled'),outputLabel=reported&&output!=='Unknown'?output:'Output unknown';
 const notice=device?.state==='FAULT'?'Connection fault · showing last readings.':!device?.connected?'Disconnected · showing last readings.':!ageKnown?'Reading age unknown.':state.roles?.laser?.unknown?'Connection status unknown.':'';
 const outputButton=sample.output_enabled===true?button('output-off','Laser Disable',live&&!state.outputSending,'laser-disable'):button('output-on','Laser Enable',live&&reviewed&&sample.output_enabled===false&&!state.outputSending,'laser-enable');
 const roundedRange=reviewed&&Array.isArray(operating)&&operating.length===2&&operating.every(Number.isFinite)?[Math.ceil(operating[0]*1000)/1000,Math.floor(operating[1]*1000)/1000]:null;
 const scanRange=roundedRange&&roundedRange[0]<=roundedRange[1]?roundedRange:null;
 const scanPossible=scanRange&&scanRange[1]-scanRange[0]>=0.009999;
 let scanStart=scanRange?Math.max(scanRange[0],Math.min(scanRange[1],Number.isFinite(sample.wavelength_setpoint_nm)?Math.round(sample.wavelength_setpoint_nm*1000)/1000:scanRange[0])):'';
 let scanStop=scanRange?(scanRange[1]-scanStart>=0.009999?scanRange[1]:scanRange[0]):'';
 if(scanPossible&&Math.abs(scanStop-scanStart)<0.009999){scanStart=scanRange[0];scanStop=scanRange[1];}
 const field=(id,label,value,min,max,step,enabled)=>`<label for="${id}">${label}<input id="${id}" class="control" type="number" min="${min}" max="${max}" step="${step}" value="${esc(value)}"${enabled?'':' disabled'}></label>`;
 const digits=(id,label,value,min,max,precision,whole)=>`<label for="${id}">${label}<input id="${id}" class="control scan-digits" type="text" inputmode="decimal" min="${min}" max="${max}" data-digits="${precision}" data-whole="${whole}" value="${esc(value===''?'':formatDigits(value,precision,whole))}"${targetEditable?'':' disabled'}></label>`;
 const shortcuts=state.scanShortcutPreferences||defaultScanShortcutPreferences();
 const singleRates=singleScanRates(device),singleSupported=singleRates.length>0;
 const defaultVelocity=Number.isFinite(cap)?Math.min(.1,cap):null;
 const scanAction=(op,label,description,enabled,kind='',single=false)=>`<div class="laser-scan-action">${button(op,label,single?enabled&&singleScanRateAllowed(device,defaultVelocity):enabled,kind,single?` data-single-ready="${enabled?'true':'false'}"`:'')}<small class="laser-scan-description">${description}</small>${renderScanShortcutHint(shortcuts,'laser-'+op)}</div>`;
 return pageHeader('INSTRUMENT / LASER','Laser','',connectionAction('laser',state))+(device?`<div class="laser-console">
 ${device.status_error?`<p class="alert laser-status-error" role="alert">Could not refresh readings: ${esc(device.status_error)}</p>`:''}
 <section class="card" id="laser-readings-card"><div class="card-head"><h2 class="card-title">Laser Status</h2>${badge(outputLabel,sample.output_enabled===true||!reported||output==='Unknown'?'warn':'')}</div><div class="card-body">
 ${!device.status_error&&notice?`<p class="laser-reading-notice" role="status">${esc(notice)}</p>`:''}
 ${reported&&(sample.status_byte&128)?'<p class="laser-reading-notice laser-controller-error" role="alert">Controller has an error message. See Reading details.</p>':''}
 <dl class="laser-readings ${reported?'':'readings-stale'}"><div class="laser-wavelength-reading"><dt>Wavelength <small>Controller readback</small></dt><dd>${esc(display(sample.wavelength_nm,'nm'))}</dd></div><div><dt>Power</dt><dd>${esc(display(sample.power_mw,'mW'))}</dd></div><div><dt>Current</dt><dd>${esc(display(sample.current_ma,'mA'))}</dd></div></dl>
 <div class="laser-readings-footer"><div class="laser-mode-line"><span>${esc(toggle(sample.constant_power,'Constant power','Constant current'))}</span>${reported&&sample.operation_complete===false?'<span>Controller busy</span>':''}<span>${esc(ageKnown?sampleAgeLabel(device.sample_age_s,'Last updated'):'Update time unknown')}</span></div><div class="laser-refresh"><label for="laser-refresh-interval">Refresh<select id="laser-refresh-interval" class="control" data-managed="true">${[10,30,60].map(v=>`<option value="${v}"${v===interval?' selected':''}>${v} s</option>`).join('')}</select></label>${button('read','Refresh now',readable)}</div></div>
 <details id="laser-reading-details" class="laser-reading-details"><summary>Reading details</summary><dl><div><dt>Operation</dt><dd>${esc(toggle(sample.operation_complete,'Complete','Busy'))}</dd></div><div><dt>Status byte</dt><dd>${esc(display(sample.status_byte))}</dd></div><div><dt>Panel</dt><dd>${esc(toggle(sample.remote,'Remote control','Local control'))}</dd></div></dl><p class="hint">Controller-reported values. Wavelength readback is not an independent wavelength measurement.</p></details></div></section>
 <section class="card" id="laser-control-card"><div class="card-head"><h2 class="card-title">Control</h2></div><div class="card-body">
 ${!reviewed?'<p class="alert">This laser head is not in the supported model table. Tuning is unavailable.</p>':''}
 <div class="laser-control-columns"><section class="laser-manual"><h3>Target Wavelength</h3><div class="laser-target-row"><div class="laser-target-value"><input id="laser-wavelength" class="control wavelength-digits" type="text" inputmode="decimal" aria-label="Target Wavelength (nm)" data-managed="true" readonly value="${esc(formatTarget(state.targetValue??sample.wavelength_setpoint_nm))}"${targetEditable?'':' disabled'}><span>nm</span></div></div><p class="hint">${state.targetDirty?'Draft · ':''}Enter or Goto applies this value. Tracking turns off after arrival.</p><div class="form-actions laser-manual-actions">${outputButton}${button('goto','Goto Wavelength',compositeControls)}</div></section>
 <section class="laser-scan"><h3>Scanning</h3><div class="laser-scan-fields">
 ${digits('laser-scan-start','Start Wavelength (nm)',scanStart,reviewed?operating[0]:1,reviewed?operating[1]:5000,3,4)}
 ${digits('laser-scan-stop','Stop Wavelength (nm)',scanStop,reviewed?operating[0]:1,reviewed?operating[1]:5000,3,4)}
 ${digits('laser-scan-speed','Forward Velocity (nm/s)',Number.isFinite(cap)?Math.min(.1,cap):'',0.01,cap??20,2,2)}
 ${digits('laser-scan-return-speed','Backward Velocity (nm/s)',Number.isFinite(cap)?Math.min(.1,cap):'',0.01,cap??20,2,2)}
 </div>${reviewed&&!scanPossible?'<p class="hint">Scanning needs at least 0.01 nm between Start and Stop. Widen the operating limits to scan.</p>':''}<div class="form-actions laser-scan-actions">
 ${scanAction('scan-start','Full Scan','Start → Stop → Start',compositeControls&&scanPossible,'primary')}
 ${scanAction('scan-forward','Forward Scan','Current → Stop',compositeControls&&singleSupported,'',true)}
 ${scanAction('scan-backward','Backward Scan','Current → Start',compositeControls&&singleSupported,'',true)}
 ${scanAction('scan-stop','Stop Scan','Stop & hold position',stopping)}
 </div>
 </section></div>
 <div class="laser-scan-preferences">${device.move?`<p class="hint laser-move-status" role="status">${esc(device.move.message)}${moving&&Number.isFinite(device.move.elapsed_s)?` · ${Math.floor(device.move.elapsed_s)} s`:''}</p>`:''}${restriction?`<p class="hint laser-control-restriction">${esc(restriction)}</p>`:''}<p class="hint laser-single-scan-qualification">${singleSupported?`Verified single-pass speeds: ${singleRates.map(v=>esc(v)).join(', ')} nm/s. Choose one of these speeds for Forward / Backward.`:'Forward / Backward: awaiting speed and hold verification for this controller, laser head and USB driver.'}</p>${renderScanShortcuts(shortcuts,{error:state.scanShortcutError})}</div>
 <div class="laser-options"><details id="laser-fine-tuning"><summary>Fine tuning</summary><div class="laser-inline-field">${field('laser-piezo','Piezo (%)',sample.piezo_percent??'',0,100,'0.01',controls)}${button('piezo','Set piezo',controls)}</div><p class="hint">Fine cavity tuning, 0–100%. This is not a wavelength in nm.</p></details>
 <details id="laser-operating-limits"><summary>Operating limits</summary><p class="hint">Your limits can only narrow the hardware range. They also cap the move to Start and the return scan.</p><div class="laser-limit-fields">
 ${field('laser-limit-min','Minimum (nm)',reviewed?operating[0]:'',reviewed?range[0]:1,reviewed?range[1]:5000,'0.01',limitsEditable)}
 ${field('laser-limit-max','Maximum (nm)',reviewed?operating[1]:'',reviewed?range[0]:1,reviewed?range[1]:5000,'0.01',limitsEditable)}
 ${field('laser-limit-speed','Max scan speed (nm/s)',cap??'',0.01,device.max_scan_speed_nm_s??20,'0.01',limitsEditable)}
 </div><button class="btn" data-ui="save-laser-limits"${limitsEditable&&sample.operation_complete===true?'':' disabled'}>Save & reconnect</button><p class="hint">${state.remote?'Edit and save these limits on the owning Host on its local computer.':'Reconnects with the saved limits; output settings are preserved.'}</p></details></div>
 </div></section>
 <details id="laser-device-information" class="card"><summary>Device information</summary><div class="card-body"><p>Head <strong>${esc(identity.head_model||'Unknown')}</strong> · S/N ${esc(identity.head_serial||'Unknown')}</p><p>Controller S/N ${esc(identity.serial||'Unknown')} · Firmware ${esc(identity.firmware||'Unknown')}</p><p class="hint">${reviewed?`Hardware envelope: ${esc(range[0])}–${esc(range[1])} nm · Max scan speed ${esc(display(device.max_scan_speed_nm_s,'nm/s'))}`:'Hardware limits unknown.'}</p><p class="hint">The key and interlock govern emission. An accepted command does not prove optical output.</p></div></details></div>`:empty('laser'));
}

function pageHeader(eyebrow, title, intro, actions = '') {
  return `<div class="page-header"><div><div class="eyebrow">${esc(eyebrow)}</div>
    <h1>${esc(title)}</h1><p class="page-intro">${esc(intro)}</p></div>
    <div class="page-actions">${actions}</div></div>`;
}

function badge(text, kind = '') {
  return `<span class="badge ${kind}">${esc(text)}</span>`;
}

function empty(role) {
  return `<div class="card empty-panel"><span class="empty-icon">◇</span>
    <h2>${esc(roleLabels[role])} not connected</h2>
    <p>Choose a resource in Device setup, then connect. Opening the console does not connect or change instrument outputs.</p>
    <div class="form-actions" style="justify-content:center"><button class="btn" data-page="settings">Open device setup</button></div></div>`;
}

export function connectionAction(role, state, blocked = false) {
  if(role==='osa'){
    const ticket=state.status?.devices?.osa?.unstaged_capture||state.lastOperation?.result?.result?.unstaged_capture;
    const id=ticket?.capture_id||(state.lastOperation?.phase==='completed_readback_failed'
      ?state.lastOperation?.result?.result?.capture?.capture_id:null);
    if(/^[0-9a-f]{32}$/.test(id||''))return `<span class="hint">Read completed; saving failed.</span><button class="btn" data-op="osa-retry-save" data-capture="${esc(id)}" ${state.pending||state.savingCapture?'disabled':''}>${state.savingCapture?'Saving…':'Retry save'}</button>`;
  }
  if(state.hideConnectionAction){
    const control=state.roles?.[role];
    return !state.closing&&control?.confirmed&&!control.hostRestricted&&!control.unknown&&canResume(control)
      ? `<button class="btn" data-op="resume" data-role="${role}">Resume controls</button>` : '';
  }
  if (!state.client) return '';
  const device = state.status?.devices?.[role];
  const control = state.roles?.[role];
  const owned = device || control?.context?.connection_id;
  const disabled = (control ? owned ? !canSendSafety(control)
    : (control.canConnect===undefined ? control.context?.connection_id !== null || control.normalPending || control.hostRestricted || control.unknown : !control.canConnect) || state.closing
    : state.mode === 'unknown') || (!device && blocked && control?.canConnect!==true) ? ' disabled' : '';
  if (owned) return `${roleProgress(role, state)}<button class="btn warn" data-op="disconnect" data-role="${role}"${disabled}>${device?.connected ? 'Disconnect' : 'Retry disconnect (resource retained)'}</button>`;
  return `<button class="btn primary" data-op="connect" data-role="${role}"${disabled}>Connect</button>`;
}

function roleBlocked(state, role) {
  return state.roles ? !canSendNormal(state.roles[role]) : Boolean(state.busy);
}

function safetyBlocked(state, role) {
  return state.roles ? !canSendSafety(state.roles[role]) : false;
}

function roleProgress(role, state) {
  const control = state.roles?.[role];
  if (!control) return '';
  const safety = control.safety;
  const phase = { REQUESTED: 'Accepted', INITIAL_RUNNING: 'Accepted; initial call running',
    WAITING_OLD: 'Waiting for prior call', FINAL_RUNNING: 'Waiting for final call', STOP_HELD: 'Completed',
    RETAINED: 'Unknown; resource retained', CLOSING: 'Close accepted' }[safety?.state];
  const accepted = control.safetyPending && control.safetyEpoch > control.safetyPending.sentEpoch &&
    intentRank(safety?.effective_intent) >= intentRank(control.safetyPending.intent);
  const closingAccepted = state.closing && control.safetyEpoch > control.shutdownEpoch &&
    safety?.effective_intent === 'disconnect';
  const progress = control.safetyPending ? accepted && phase || 'Pending'
    : closingAccepted && phase || control.progress || phase ||
    (control.normalPending || control.activeRequest ? 'Operation pending' : control.mode);
  const waiting = role === 'gain' && control.safetyPending && control.normalPending?.name === 'wait_stable'
    ? '; Shutdown pending; stability wait still active' : '';
  const confirm = control.mode === 'READY' && !control.confirmed
    ? `<button class="btn" data-op="confirm-context" data-role="${role}">Confirm control context</button>` : '';
  const resume = !state.closing && canResume(control)
    ? `<button class="btn" data-op="resume" data-role="${role}" ${control.normalPending || control.safetyPending || control.activeRequest ? 'disabled' : ''}>Resume controls</button>` : '';
  const attempts = safety?.attempts?.length ? `<details><summary>Device call attempts (original evidence)</summary><pre>${esc(JSON.stringify({ attempts: safety.attempts, outcomes: safety.outcomes }, null, 2))}</pre></details>` : '';
  return `<span class="hint">${esc(progress)}${esc(waiting)}${safety?.attempt_id ? ` · attempt ${esc(safety.attempt_id)}` : ''}</span>${confirm}${resume}${attempts}`;
}

export function overview(state) {
  const devices = state.status?.devices || {};
  const retained = Object.values(devices).filter((device) => device.connected !== true).length;
  const ready = Object.values(devices).filter((device) => device.connected).length;
  const inventory = state.inventory;
  const discovery = (role, resource) => {
    if (!inventory) return 'Inventory not scanned';
    if (role === 'fiber') {
      const sides = Number(Boolean(inventory.fiber?.left)) + Number(Boolean(inventory.fiber?.right));
      return sides ? `Inventory found ${sides}/2 sides` : 'Neither side found';
    }
    if (!resource) return 'No resource bound';
    const resources = role === 'osa' || role === 'pm400'
      ? inventory.visa || [] : (inventory.serial || []).map((port) => port.resource);
    return resources.some((candidate) => candidate?.toLowerCase() === resource.toLowerCase())
      ? 'Found in inventory' : 'Bound resource not found';
  };
  const cards = Object.keys(roleLabels).map((role) => {
    const device = devices[role];
    const connected = device?.connected === true;
    const previous = state.lastKnown?.[role];
    const stale = !device && Boolean(previous);
    const problem = device?.status_error;
    const resource = device?.resource || state.bindings?.[role] || previous?.resource || null;
    const statusTime = device ? state.status?.observed_at : state.lastKnownAt?.[role];
    const identity = role === 'fiber'
      ? 'Left / right NanoMax 300 / MDT693B'
      : device?.identity || device?.instrument?.model || previous?.identity
        || previous?.instrument?.model || resource || 'Select a resource';
    return `<div class="card status-card ${connected ? 'connected' : ''}">
      <div class="card-head"><div><div class="card-title">${esc(roleLabels[role])}</div>
      <p class="card-subtitle">${esc(identity)}</p></div>${badge(connected ? 'READY' : problem ? 'FAULT' : device ? 'RETAINED' : stale ? 'STALE' : 'OFFLINE', connected ? 'ready' : problem ? 'fault' : stale || device ? 'warn' : '')}</div>
      <div class="card-body"><div><strong>${connected ? 'Connected' : device ? 'Resource retained by worker; release unconfirmed' : stale ? 'Previously identified; now disconnected' : 'Disconnected'}</strong>
      <small>${esc(resource || (role === 'fiber' ? 'Sides identified by serial number' : '—'))}</small>
      <small>${esc(discovery(role, resource))} · Last status ${esc(statusTime || 'Unknown')}</small>
      ${problem ? `<small class="status-error">${esc(problem)}</small>` : ''}${roleProgress(role, state)}</div>
      <span class="status-symbol">${role === 'voltage' ? '▦' : role === 'fiber' ? '⌁' : '◈'}</span></div></div>`;
  }).join('');
  const cleanup = state.status?.last_cleanup;
  const history = (state.shutdownHistory || []).map((attempt, index) => `<details><summary>Shutdown attempt ${index + 1} · Original report and process exit evidence</summary><pre class="cleanup-evidence">${esc(JSON.stringify(attempt, null, 2))}</pre></details>`).join('');
  const unresolved = cleanup?.unreleased?.length ? `Unreleased: ${cleanup.unreleased.join(', ')}`
    : cleanup ? 'No unreleased resources reported' : 'No cleanup report';
  const cleanupDetails = cleanup ? `<div class="section-title"><h2>Latest cleanup report</h2><span>Host observations and driver reports</span></div>
    <div class="card"><div class="card-body"><p class="hint">${esc(unresolved)}. Voltage zero: ${esc(cleanup.voltage_zero?.state === 'measured_zero'
      ? 'Zero confirmed by host telemetry; not an independent physical measurement' : cleanup.voltage_zero ? 'Zero not confirmed by host telemetry' : 'No voltage-source evidence')}. </p>
      ${cleanup.process_exit ? `<p class="hint">${cleanup.process_exit.confirmed ? 'Process exited' : 'Process exit unconfirmed'}; Exit code ${esc(cleanup.process_exit.code ?? 'Unknown')}; ${cleanup.process_exit.success ? 'Normal exit' : 'Abnormal exit'}. ${cleanup.resource_release_verified === false ? 'Resource release unconfirmed; process exit does not prove physical outputs are safe.' : ''}</p>` : ''}
      ${cleanup.received_cleanup_report ? `<details><summary>Cleanup report received before exit (original evidence; does not override this failure)</summary><pre class="cleanup-evidence">${esc(JSON.stringify(cleanup.received_cleanup_report, null, 2))}</pre></details>` : ''}
      <div class="cleanup-steps">${(cleanup.steps || []).map((step) => `<div class="cleanup-step ${step.ok ? '' : 'failed'}">
        <span>${esc(step.role)} · ${esc(step.action)}</span><strong>${step.ok ? 'Executed' : 'Failed'}</strong>${step.error ? `<small>${esc(step.error)}</small>` : ''}</div>`).join('') || '<p class="hint">No instrument cleanup steps in this attempt.</p>'}</div></div></div>` : '';
  return pageHeader('LABORATORY / OVERVIEW', 'Instrument overview', 'Monitor spectra, temperature, voltage, power and fiber coupling in one window.',
    `<button class="btn" data-op="refresh" ${state.client && state.mode !== 'unknown' ? '' : 'disabled'}>Refresh inventory</button>`) +
    `<div class="overview-intro"><div class="card overview-lead"><div class="ornament"></div><div class="card-body">
      <div class="eyebrow">LOCAL SESSION</div><h2>Check the state.<br>Confirm each action.</h2>
      <p>Select an instrument, verify its identity and readings, then operate it. Physical outputs and estimated stage positions are labeled separately.</p>
      <button class="btn primary" data-page="settings">Configure bench →</button></div></div>
      <div class="card overview-side"><div class="card-body"><div class="eyebrow">READY DEVICES</div>
      <div class="big-number">${ready}<small> / 5 instrument types</small></div>
      <p class="hint">${esc(state.mode ? 'Real hardware' : 'Worker not started')}</p>
      <div class="separator"></div><p class="hint">${esc(unresolved)}</p></div></div></div>
    <div class="section-title"><h2>Instrument status</h2><span>${retained} ROLE(S) RETAINED</span></div><div class="card-grid">${cards}</div>${cleanupDetails}${history}`;
}

function resourceOptions(values, selected, suggestions = [], { label = 'Discovered resources', scanned = false,
  restLabel = 'Other resources' } = {}) {
  const available = [...new Set(values.filter(Boolean))];
  const recommended = [...new Set(suggestions)].filter((value) => available.includes(value));
  const other = available.filter((value) => !recommended.includes(value));
  const group = (name, items) => items.length ? `<optgroup label="${esc(name)}">${items.map((value) =>
    `<option value="${esc(value)}" ${value === selected ? 'selected' : ''}>${esc(value)}</option>`).join('')}</optgroup>` : '';
  const saved = selected && !available.includes(selected)
    ? group(scanned ? 'Previous binding (not found)' : 'Saved binding', [selected]) : '';
  return `<option value="">No resource selected</option>${saved}${group(label, recommended)}${group(restLabel, other)}`;
}

function serialKey(resource) {
  const normalized = String(resource || '').trim().toLowerCase();
  const prefix = String.fromCharCode(92, 92, 46, 92); // Windows device namespace.
  const candidate = normalized.startsWith(prefix) ? normalized.slice(prefix.length) : normalized;
  return /^com[0-9]+$/.test(candidate) ? candidate : normalized;
}

function trendSvg(history, valueAt, minimum, maximum, name) {
  const values = (history || []).map(valueAt).filter((value) => Number.isFinite(value));
  if (!values.length) return '<div class="trend-empty">Waiting for host samples</div>';
  const points = values.map((value, index) => {
    const x = values.length === 1 ? 100 : 4 + index / (values.length - 1) * 192;
    const y = 50 - Math.max(0, Math.min(1, (value - minimum) / (maximum - minimum))) * 46;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(' ');
  const last = points.split(' ').at(-1).split(',');
  return `<svg class="trend" data-trend="${name}" viewBox="0 0 200 54" preserveAspectRatio="none" aria-label="Last ${values.length} host samples; fixed scale ${minimum}–${maximum}">
    <path d="M4 50H196M4 27H196M4 4H196" stroke="#2a404a" stroke-dasharray="2 3"/>
    <polyline points="${points}" fill="none" stroke="#54d6cf" stroke-width="2" vector-effect="non-scaling-stroke"/>
    <circle cx="${last[0]}" cy="${last[1]}" r="3" fill="#54d6cf"/></svg>`;
}


function plot(trace, cursor) {
  if(trace?.verified===true)return nativePlot(trace,cursor);
  if (!trace?.wavelength_nm?.length || trace.wavelength_nm.length !== trace?.power_dbm?.length) {
    return '<div class="plot-empty"><strong>No spectrum data</strong>Acquire a trace using the instrument’s current front-panel settings.</div>';
  }
  const xs = trace.wavelength_nm;
  const ys = trace.power_dbm;
  let xmin = Infinity, xmax = -Infinity, ymin = Infinity, ymax = -Infinity;
  for (let index = 0; index < xs.length; index += 1) {
    const x = xs[index], y = ys[index];
    if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
    xmin = Math.min(xmin, x); xmax = Math.max(xmax, x);
    ymin = Math.min(ymin, y); ymax = Math.max(ymax, y);
  }
  if (!Number.isFinite(xmin)) return '<div class="plot-empty">Spectrum contains no valid values</div>';
  const xspan = xmax - xmin || 1, yspan = ymax - ymin || 1;
  // Keep both extrema in each bucket so narrow spectral peaks remain visible.
  // Bounding the SVG also avoids huge DOM nodes on high-resolution OSA traces.
  const bucketSize = Math.max(1, Math.ceil(xs.length / 1000));
  const plotted = [];
  for (let start = 0; start < xs.length; start += bucketSize) {
    let low = -1, high = -1;
    for (let index = start; index < Math.min(xs.length, start + bucketSize); index += 1) {
      if (!Number.isFinite(xs[index]) || !Number.isFinite(ys[index])) continue;
      if (low < 0 || ys[index] < ys[low]) low = index;
      if (high < 0 || ys[index] > ys[high]) high = index;
    }
    if (low < 0) continue;
    if (low === high) plotted.push(low);
    else plotted.push(...(low < high ? [low, high] : [high, low]));
  }
  const points = plotted.map((index) => `${(48 + (xs[index] - xmin) / xspan * 718).toFixed(1)},${(282 - (ys[index] - ymin) / yspan * 235).toFixed(1)}`).join(' ');
  const active = Number.isInteger(cursor) && cursor >= 0 && cursor < xs.length
    && Number.isFinite(xs[cursor]) && Number.isFinite(ys[cursor]);
  const markerX = active ? 48 + (xs[cursor] - xmin) / xspan * 718 : null;
  const markerY = active ? 282 - (ys[cursor] - ymin) / yspan * 235 : null;
  return `<svg viewBox="0 0 800 320" role="img" aria-label="OSA spectrum; click to inspect a sample" data-osa-plot><defs><linearGradient id="plotfill" x1="0" x2="0" y1="0" y2="1"><stop stop-color="#54d6cf" stop-opacity=".22"/><stop offset="1" stop-color="#54d6cf" stop-opacity="0"/></linearGradient></defs>
    <path d="M48 47V282H766" stroke="#39515e" fill="none"/>
    <path d="M48 100H766M48 160H766M48 220H766" stroke="#28404b" stroke-dasharray="3 5"/>
    <polygon points="48,282 ${points} 766,282" fill="url(#plotfill)"/>
    <polyline points="${points}" fill="none" stroke="#54d6cf" stroke-width="2" vector-effect="non-scaling-stroke"/>
    ${active ? `<path d="M${markerX.toFixed(1)} 47V282" stroke="#f0a85e" stroke-width="1" stroke-dasharray="4 4"/><circle cx="${markerX.toFixed(1)}" cy="${markerY.toFixed(1)}" r="5" fill="#f0a85e"/>` : ''}
    <text x="48" y="304" class="axis-label">${xmin.toFixed(1)} nm</text><text x="766" y="304" text-anchor="end" class="axis-label">${xmax.toFixed(1)} nm</text>
    <text x="8" y="52" class="axis-label">${ymax.toFixed(1)}</text><text x="8" y="282" class="axis-label">${ymin.toFixed(1)}</text></svg>`;
}

// Captures are immutable; cache their bounds and decimated polyline, not cursors.
// Weak keys release the cache when a selected capture is no longer retained.
const nativePlots=new WeakMap();
function nativePlot(trace,cursor){
  const xs=trace.wavelength_nm,ys=trace.native_values;
  let cached=nativePlots.get(trace);
  if(!cached){const xmin=xs[0],xmax=xs.at(-1);let ymin=ys[0],ymax=ys[0];
    for(const y of ys){ymin=Math.min(ymin,y);ymax=Math.max(ymax,y);}
    const scale=Math.max(Math.abs(ymin),Math.abs(ymax))||1,lo=ymin/scale,span=ymax/scale-lo||1;
    const px=i=>48+(xs[i]-xmin)/(xmax-xmin||1)*718,py=i=>282-(ys[i]/scale-lo)/span*235;
    const points=decimateTrace(trace).map(i=>`${px(i).toFixed(1)},${py(i).toFixed(1)}`).join(' ');
    cached={xmin,xmax,ymin,ymax,px,py,points};nativePlots.set(trace,cached);
  }
  const {xmin,xmax,ymin,ymax,px,py,points}=cached;
  const active=Number.isInteger(cursor)&&cursor>=0&&cursor<xs.length;
  return `<svg viewBox="0 0 800 320" role="img" aria-label="OSA spectrum in ${esc(trace.native_unit)}; click to inspect a raw sample" data-osa-plot><path d="M48 47V282H766" stroke="#82979f" fill="none"/><polyline points="${points}" fill="none" stroke="#178c87" stroke-width="2"/>${active?`<path d="M${px(cursor).toFixed(1)} 47V282" stroke="#bc7628" stroke-dasharray="4 4"/><circle cx="${px(cursor).toFixed(1)}" cy="${py(cursor).toFixed(1)}" r="5" fill="#bc7628"/>`:''}<text x="48" y="304" class="axis-label">${esc(xmin)} nm</text><text x="766" y="304" text-anchor="end" class="axis-label">${esc(xmax)} nm</text><text x="8" y="36" class="axis-label">${esc(trace.native_unit)}</text><text x="8" y="52" class="axis-label">${esc(ymax)}</text><text x="8" y="282" class="axis-label">${esc(ymin)}</text></svg>`;
}

export function osa(state) {
  const reading=Boolean(state.pending&&['read_trace','acquire'].includes(state.activity?.kind)||state.roles?.osa?.normalPending);
  const unresolvedRead=state.unknown&&['read_trace','acquire'].includes(state.activity?.kind);
  const previous=Boolean(!state.historical&&(state.previousTrace&&!state.trace||state.trace&&(reading||unresolvedRead)));
  if(!state.trace&&!state.historical&&state.previousTrace)state={...state,trace:state.previousTrace};
  const device = state.status?.devices?.osa;
  const blocked = roleBlocked(state, 'osa') || device?.connected !== true || Boolean(device?.status_error)||Boolean(device?.unstaged_capture)||state.savingCapture;
  const cursor = Number.isInteger(state.cursor) ? state.cursor : null;
  const native=state.trace?.verified===true,ys=native?state.trace.native_values:state.trace?.power_dbm;
  const cursorValid = cursor !== null && Number.isFinite(state.trace?.wavelength_nm?.[cursor])&&Number.isFinite(ys?.[cursor]);
  const cursorText = cursorValid
    ? native?`${state.trace.wavelength_nm[cursor]} nm · ${ys[cursor]} ${state.trace.native_unit}`:`${state.trace.wavelength_nm[cursor].toFixed(3)} nm · ${ys[cursor].toFixed(3).replace('-', '−')} dBm`
    : 'Click the spectrum to inspect the nearest raw sample';
  const unavailable=blocked||state.archiveAvailable===false||Boolean(state.pending),disabled=unavailable?'disabled':'';
  const entries=state.archiveEntries||[];
  return pageHeader('INSTRUMENT / SPECTRUM','Optical spectrum analyzer','Read and save the existing trace. Front-panel settings are preserved.',connectionAction('osa',state))+`<div class="device-layout"><div class="card"><div class="card-head"><div><h2 class="card-title">${state.historical?'Historical capture':previous?'Previous capture':'Spectrum'} · Trace ${esc(state.trace?.trace||'A')}</h2><p class="card-subtitle">${esc(state.trace?.metadata?.identity||device?.identity||'Not connected')}</p></div>${badge(state.historical?'HISTORICAL':previous?'PREVIOUS':device?.connected?'ONLINE':'OFFLINE')}</div><div class="card-body"><div class="plot-box">${plot(state.trace,cursor)}</div><p class="cursor-readout">${esc(cursorText)}</p><p class="hint">DATA POINTS · ${state.trace?.wavelength_nm?.length||'—'}</p>${native?`<p class="hint">${esc(state.trace.metadata.read_finished_at)} · ${esc(state.trace.native_unit)} · Consistency unproven</p>`:''}</div></div>
  <div class="stack"><div class="card"><div class="card-head"><h2 class="card-title">Capture</h2></div><div class="card-body">${device?.status_error?`<div class="alert">Status read failed: ${esc(device.status_error)}</div>`:''}<div class="field"><label for="osa-name">Recording name</label><input id="osa-name" class="control" value="osa" maxlength="40" pattern="[A-Za-z0-9][A-Za-z0-9_-]{0,39}"></div><div class="field"><label for="osa-trace">Trace</label><select id="osa-trace" class="control">${['A','B','C','D','E','F','G'].map(t=>`<option value="${t}">${t}</option>`).join('')}</select></div><div class="form-actions"><button class="btn primary" data-op="osa-read" ${disabled}>${reading?'Reading…':'Read trace'}</button><button class="btn" data-op="osa-export" ${!state.trace||state.exporting?'disabled':''}>${state.exporting?'Exporting…':'Export CSV'}</button></div><details id="osa-sweep"><summary>Start sweep</summary><p class="hint">Runs one sweep. The driver verifies SINGLE or AUTO on the instrument; REPEAT is refused.</p><button class="btn" data-op="osa-acquire" ${disabled}>${reading?'Reading…':'Start sweep'}</button></details><p class="hint">Save folder: ${esc(state.recordingRoot||'See Settings')}</p>${state.exportDirectory?`<p class="hint">Exported: ${esc(state.exportDirectory)}</p>`:''}</div></div>
  <div class="card"><div class="card-head"><h2 class="card-title">Saved captures</h2><button class="btn small" data-ui="osa-history-refresh" ${state.historyBusy?'disabled':''}>Refresh</button></div><div class="card-body">${state.historyError?`<p class="alert">${esc(state.historyError)}</p>`:''}${entries.map(e=>`<p><button class="btn small" data-ui="osa-history-load" data-archive="${esc(e.id)}" data-name="${esc(e.name)}" ${e.state!=='complete'||state.historyBusy?'disabled':''}>${esc(e.name)}</button> <span class="hint">${esc(e.state)} · ${esc(e.reference?.metadata?.read_finished_at||e.error||'Incomplete capture')}</span></p>`).join('')||'<p class="hint">No captures loaded</p>'}${state.historyHasMore?`<button class="btn" data-ui="osa-history-more" ${state.historyBusy?'disabled':''}>Load more</button>`:''}${state.historical?'<button class="btn" data-ui="osa-current">Show current</button>':''}</div></div></div></div>`;
}

export function voltage(state) {
  const device = state.status?.devices?.voltage;
  const blocked = roleBlocked(state, 'voltage') || device?.connected !== true || Boolean(device?.status_error);
  const rows = voltageRows(device);
  const role=state.roles?.voltage,work=state.voltageActivity||state.activity;
  const busy=Boolean(role?.normalPending||role?.safetyPending||state.pending||activityBusy(work));
  const error=device?.status_error||device?.observation_error||device?.fault;
  const unknown=Boolean(role?.unknown||state.unknown||work?.outcome==='unknown');
  const stale=device?.connected!==true||unknown||Boolean(error)||['stale','unknown','error'].includes(device?.quality);
  const warning=Boolean(error||unknown||work?.outcome==='failed');
  const status=error?String(error.message||error):unknown?'Operation unconfirmed. Check status before continuing.':busy?(activityBusy(work)?activityLabel(work):'Updating controller…'):work?.outcome==='failed'?'Update failed. Check the controller.':role?.stopHeld?'Outputs held · Resume controls to apply targets.':'';
  const target=(channel)=>{const id=`voltage-${channel}`,stored=state.inputs?.get(id);return stored!==undefined?(typeof stored==='object'?stored.value:stored):formatDigits(Number.isFinite(device?.requested_voltage_v?.[channel-1])?device.requested_voltage_v[channel-1]:0,3,2);};
  const reading=(values,index,unit)=>Number.isFinite(values?.[index])?`${esc(values[index].toFixed(3))}<small>${unit}</small>`:'Unknown';
  return pageHeader('INSTRUMENT / OUTPUT', '8-channel voltage source', '', connectionAction('voltage', state)) +
    (device ? `<section class="voltage-panel"><div class="section-title voltage-toolbar"><div><h2>Output channels</h2><p class="voltage-panel-status ${warning?'stale-warning':''}" role="status" aria-live="polite">${esc(status||'0–14 V · Enter to apply each target')}</p></div><button class="btn danger" data-op="voltage-zero" ${safetyBlocked(state, 'voltage') ? 'disabled' : ''}>Zero all channels</button></div>
    <div class="channel-grid">${rows.map((row) => {const index=row.channel-1,missing=!Number.isFinite(device.voltage_v?.[index])||!Number.isFinite(device.current_ma?.[index]);return `<article class="card channel-card"><div class="channel-head"><strong>CH ${row.channel.toString().padStart(2,'0')}</strong><span class="micro-led ${stale||missing?'uncertain':''}" aria-label="${stale||missing?'Readings unavailable or previous':'Measured output'}"></span></div>
      <dl class="channel-readings${stale?' readings-stale':''}"><div><dt>Voltage</dt><dd aria-label="Measured voltage">${reading(device.voltage_v,index,'V')}</dd></div><div><dt>Current</dt><dd aria-label="Measured current">${reading(device.current_ma,index,'mA')}</dd></div></dl>
      <div class="channel-status ${stale||missing?'stale-warning':''}">${missing?'Readings unavailable':stale?'Previous readings':'Measured output'}</div>
      <div class="channel-trend">${state.voltageHistory?.length?trendSvg(state.voltageHistory, (sample) => sample.voltage_v[index], 0, 14, `voltage-${row.channel}`):''}</div>
      <label class="channel-target" for="voltage-${row.channel}">Target voltage <span>V</span></label>
      <div class="channel-control"><input class="control voltage-digits" type="text" inputmode="decimal" min="0" max="14" id="voltage-${row.channel}" data-digits="3" data-whole="2" value="${esc(target(row.channel))}" aria-label="CH${row.channel} Target voltage"><button class="btn small" data-op="voltage-apply" data-channel="${row.channel}" ${blocked ? 'disabled' : ''}>Apply</button></div></article>`;}).join('')}</div></section>` : empty('voltage'));
}

export function gain(state) {
  return renderGainPanel(state,{esc,pageHeader,connectionAction,empty,badge,roleProgress});
}

export function pm400(state) {
  return renderPm400({ ...state, busy: roleBlocked(state, 'pm400') }, connectionAction);
}

function stageScene(side, view) {
  return `<div class="stage-reference" data-stage-scene="${side}" tabindex="0" role="note" aria-label="${side} fiber stage directions. ${esc(view.towardChip)} toward chip, +Y away from operator, +Z up.">
    <strong>${side === 'left' ? 'Left stage → Chip' : 'Chip ← Right stage'}</strong><span>Lab directions only. Motion uses the controls below.</span></div>`;
}

function stageCard(side, status, rotation, blocked,consent={}) {
  const view = describeStage(status || { side, available: false });
  const preview = previewStageMove(side, { x: 0, y: 0, z: 0 });
  const sideLabel = side === 'left' ? 'LEFT' : 'RIGHT';
  return `<div class="card stage-card"><div class="card-head"><div><h2 class="card-title">${sideLabel}</h2><p class="card-subtitle mono">${esc(view.serial)} · ${esc(status?.resource || 'Disconnected')}</p></div>
    ${badge(status?.fault ? 'FAULT' : view.available ? (view.canMove ? 'ARMED' : 'READ ONLY') : 'MISSING', status?.fault ? 'fault' : view.canMove ? 'ready' : 'warn')}</div>
    <div class="card-body">${stageScene(side, view, status, rotation)}<div class="stage-data"><div class="stage-direction">${esc(view.towardChip)} = toward chip · +Y away from operator · +Z up</div>
      <div class="stage-voltage">${['X','Y','Z'].map((axis,index) => `<div><span>LOGICAL ${axis}</span><strong>${esc(view.voltage[index])}</strong></div>`).join('')}</div>
      <p class="hint">Session position estimate: <strong>${esc(view.position)}</strong>${view.position !== 'Unknown' ? ' (open-loop estimate, not measured displacement)' : ''}</p>
      ${view.fault ? `<div class="alert">${esc(view.fault)}</div>` : ''}
      <div class="stage-controls"><div class="form-row"><div class="field"><label for="${side}-x">ΔX · µm</label><input class="control" type="number" step="0.05" id="${side}-x" data-stage-axis="${side}" value="0"></div><div class="field"><label for="${side}-y">ΔY · µm</label><input class="control" type="number" step="0.05" id="${side}-y" data-stage-axis="${side}" value="0"></div><div class="field"><label for="${side}-z">ΔZ · µm</label><input class="control" type="number" step="0.05" id="${side}-z" data-stage-axis="${side}" value="0"></div></div>
      ${baselineConfirmations(side).map((message,index)=>`<label><input type="checkbox" data-managed="true" data-attestation="${index?'nominal':'baseline'}" data-side="${side}" ${consent[index?'nominal':'baseline']?'checked':''} ${blocked?'disabled':''}> ${esc(message)}</label>`).join('')}<div class="stage-preview" id="preview-${side}">${esc(preview.reason)}</div><div class="form-actions"><button class="btn primary" data-op="fiber-move" data-side="${side}" ${view.canMove && !blocked ? '' : 'disabled'}>Move relative</button><button class="btn" data-op="fiber-adopt" data-side="${side}" ${view.available && !view.restricted && !view.fault && !blocked && consent.baseline && consent.nominal ? '' : 'disabled'}>Adopt current baseline</button></div></div></div></div></div>`;
}

export function fiber(state) {
  const device = state.status?.devices?.fiber;
  return pageHeader('SETUP / FIBER COUPLING', 'Dual fiber stages', 'Lab coordinates: +X right, +Y away from operator, +Z up. Controllers are bound to each side by serial number.', connectionAction('fiber', state)) +
    (device ? `<div class="alert">MDT readings are voltages. Positions are session-only open-loop estimates after baseline adoption. Faults stop motion and hold voltage, without automatic rollback or zeroing.</div><div class="section-title"><h2>Left and right stages</h2><span>MAX312D NOMINAL · 75 V CEILING</span></div><div class="fiber-layout">${stageCard('left', device.left, state.stageView?.left, roleBlocked(state, 'fiber'),state.baselineConsent?.left)}${stageCard('right', device.right, state.stageView?.right, roleBlocked(state, 'fiber'),state.baselineConsent?.right)}</div>` : empty('fiber'));
}
