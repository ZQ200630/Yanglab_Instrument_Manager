import { describeStage, previewStageMove, sampleAgeLabel, voltageRows } from './view-model.js';
import {baselineConfirmations} from './operations.js';
import { renderPm400 } from './pm400.js';
import {decimateTrace} from './osa.js';
import { canSendNormal, canSendSafety, canResume, canEnableCurrent, gainEvidence, gainFields, intentRank } from './control-state.js';

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
  const device=state.status?.devices?.laser, sample=device?.laser||{}, identity=device?.identity||{};
  const readable=device?.connected===true&&!roleBlocked(state,'laser')&&!device?.status_error;
  const fresh=Number.isFinite(device?.sample_age_s)&&device.sample_age_s<5&&state.roles?.laser?.unknown!==true;
  const range=device?.wavelength_range_nm, reviewed=Array.isArray(range)&&range.length===2&&range.every(Number.isFinite);
  const controls=readable&&fresh&&reviewed&&sample.operation_complete===true;
  const remote=controls&&sample.remote===true;
  const display=(value,unit='')=>Number.isFinite(value)?`${value} ${unit}`.trim():'Unknown';
  const toggle=(value,on,off)=>value===true?on:value===false?off:'Unknown';
  const button=(op,label,enabled=false,kind='')=>`<button class="btn ${kind}" data-op="laser-${op}"${enabled?'':' disabled'}>${label}</button>`;
  return pageHeader('INSTRUMENT / LASER','TLB-6700 laser controller','',connectionAction('laser',state))+(device?`
    ${device.status_error?`<p class="alert">Status read failed: ${esc(device.status_error)}</p>`:''}
    <div class="device-layout"><div class="stack"><section class="card"><div class="card-head"><h2 class="card-title">Controller-reported readings</h2>${badge(toggle(sample.output_enabled,'Output enabled','Output disabled'),sample.output_enabled===true?'warn':'')}</div><div class="card-body">
    <p class="channel-reading">${esc(display(sample.wavelength_nm,'nm'))}</p><p class="hint">Wavelength readback · This is controller telemetry, not an independent wavelength measurement.</p>
    <p>Power: <strong>${esc(display(sample.power_mw,'mW'))}</strong> · Current: <strong>${esc(display(sample.current_ma,'mA'))}</strong></p>
    <p>Mode: ${esc(toggle(sample.remote,'Remote','Local'))} · Tracking: ${esc(toggle(sample.tracking,'On','Off'))} · Control: ${esc(toggle(sample.constant_power,'Constant power','Constant current'))}</p>
    <p>Operation: ${esc(toggle(sample.operation_complete,'Complete','Busy'))} · Status byte: ${esc(display(sample.status_byte))}</p><p class="hint">${esc(sampleAgeLabel(device.sample_age_s,'Status sample'))}${fresh?'':' · Stale or freshness unknown'}</p>${button('read','Read status',readable)}</div></section>
    <section class="card"><div class="card-head"><h2 class="card-title">Controller and laser head</h2></div><div class="card-body"><p>Controller S/N <strong>${esc(identity.serial||'Unknown')}</strong> · Firmware ${esc(identity.firmware||'Unknown')}</p><p>Head <strong>${esc(identity.head_model||'Unknown')}</strong> · S/N <strong>${esc(identity.head_serial||'Unknown')}</strong></p><p class="hint">${reviewed?`Reviewed standard head range: ${esc(range[0])}–${esc(range[1])} nm`:'Head control limits have not been reviewed. Read-only operation is available.'}</p></div></section></div>
    <div class="stack"><section class="card"><div class="card-head"><h2 class="card-title">Control</h2></div><div class="card-body"><p class="hint">Remote locks front-panel controls. Wavelength and tracking changes can move the tuning motor.</p><div class="form-actions">${button('remote','Select Remote',controls)}${button('local','Select Local',controls)}</div>
    <label for="laser-wavelength">Next wavelength target (nm)</label><input id="laser-wavelength" class="control" type="number" step="0.001" min="${reviewed?range[0]:1}" max="${reviewed?range[1]:5000}" value="${Number.isFinite(sample.wavelength_setpoint_nm)?sample.wavelength_setpoint_nm:''}" ${remote?'':'disabled'}><p class="hint">Current setpoint: ${esc(display(sample.wavelength_setpoint_nm,'nm'))}</p>${button('wavelength','Apply wavelength',remote&&sample.tracking===true)}
    <label for="laser-piezo">Next piezo target (%)</label><input id="laser-piezo" class="control" type="number" min="0" max="100" step="0.1" value="${Number.isFinite(sample.piezo_percent)?sample.piezo_percent:''}" ${remote?'':'disabled'}><p class="hint">Current piezo setpoint: ${esc(display(sample.piezo_percent,'%'))}</p>${button('piezo','Apply piezo',remote)}
    <div class="form-actions">${button('tracking-on','Tracking On',remote)}${button('tracking-off','Tracking Off',remote)}</div></div></section>
    <section class="card"><div class="card-head"><h2 class="card-title">Laser output</h2></div><div class="card-body"><p class="warning">The key and interlock govern laser emission. An accepted command does not prove optical output.</p><div class="form-actions">${button('output-on','Enable laser output',remote,'danger')}${button('output-off','Disable laser output',readable&&fresh&&sample.remote===true)}</div><p class="hint">Power setpoint: ${esc(display(sample.power_setpoint_mw,'mW'))} · Current setpoint: ${esc(display(sample.current_setpoint_ma,'mA'))}. Power and current changes await reviewed head limits.</p></div></section></div></div>`:empty('laser'));
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

function connectionAction(role, state, blocked = false) {
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
  return `<svg class="trend" data-trend="${name}" viewBox="0 0 200 54" aria-label="Last ${values.length} host samples; fixed scale ${minimum}–${maximum}">
    <path d="M4 50H196M4 27H196M4 4H196" stroke="#2a404a" stroke-dasharray="2 3"/>
    <polyline points="${points}" fill="none" stroke="#54d6cf" stroke-width="2" vector-effect="non-scaling-stroke"/>
    <circle cx="${last[0]}" cy="${last[1]}" r="3" fill="#54d6cf"/></svg>`;
}

export function settings(state) {
  const inventory = state.inventory || { serial: [], visa: [], fiber: {}, suggestions: {}, errors: {} };
  const workerState = !state.client ? 'STOPPED' : state.mode === 'unknown' ? 'UNKNOWN' : 'RUNNING';
  const serial = inventory.serial || [];
  const visa = inventory.visa || [];
  const selected = state.bindings || {};
  const reservedFiberPorts = new Set([inventory.fiber?.left?.resource,
    inventory.fiber?.right?.resource,
    ...(inventory.fiber?.unknown || []).map((item) => item.resource)]
    .filter(Boolean).map(serialKey));
  const serialChoices = serial.map((port) => port.resource)
    .filter((resource) => !reservedFiberPorts.has(serialKey(resource)));
  const roles = [
    ['osa', 'OSA AQ6370', visa], ['pm400', 'PM400', visa],
    ['voltage', '8-channel voltage source', serialChoices],
    ['gain', 'Gain Chip Driver', serialChoices],
  ];
  const bindingRows = roles.map(([role, name, resources]) => {
    const connected = Boolean(state.status?.devices?.[role]);
    const isSerial = role === 'voltage' || role === 'gain';
    const suggested = isSerial ? inventory.suggestions?.[role] || [] : [];
    const suggestionLabel = role === 'voltage' ? 'CH340 suggested'
      : role === 'gain' ? 'CP210x suggested' : 'Discovered resources';
    const reservedBinding = isSerial && selected[role]
      && reservedFiberPorts.has(serialKey(selected[role]));
    return `<div class="binding-row"><div class="binding-name"><strong>${esc(name)}</strong><small>${role === 'voltage' ? 'CH340 · 0–14 V' : role === 'gain' ? 'CP210x · 0–200 mA' : 'VISA RESOURCE'}</small>${reservedBinding ? '<small>Saved port is reserved for a fiber stage; choose another</small>' : ''}</div>
      <select class="control" data-binding="${role}" ${connected || state.mode === 'unknown' ? 'disabled' : ''}>${resourceOptions(resources, reservedBinding ? null : selected[role], suggested, {
        label: suggestionLabel, scanned: Boolean(state.inventory),
        restLabel: isSerial ? 'Other serial ports' : 'VISA resources',
      })}</select>
      ${connectionAction(role, state, reservedBinding)}</div>`;
  }).join('');
  const left = inventory.fiber?.left;
  const right = inventory.fiber?.right;
  const errors = Object.entries(inventory.errors || {}).map(([name, message]) =>
    `<p class="hint">${esc(name)}: ${esc(message)}</p>`).join('');
  return pageHeader('WORKSPACE / CONFIGURATION', 'Device setup', 'Inventory scans list system resources. Connections and output changes require separate actions.',
    `<button class="btn" data-op="refresh" ${state.client && state.mode !== 'unknown' ? '' : 'disabled'}>Scan available devices</button>`) +
    `<div class="device-layout"><div class="stack"><div class="card"><div class="card-head"><div><h2 class="card-title">Worker</h2><p class="card-subtitle">Python runs in the Anaconda VISA environment</p></div>${badge(workerState, workerState === 'RUNNING' ? 'ready' : workerState === 'UNKNOWN' ? 'warn' : '')}</div>
      <div class="card-body"><div class="form-row two"><div class="field"><label for="python-path">Python executable</label><input id="python-path" value="${esc(state.pythonPath)}" ${state.client ? 'disabled' : ''}></div>
      </div>
      ${workerState === 'RUNNING' && state.workerIdentity?.python_executable && state.workerIdentity?.project_root
        ? `<div class="worker-identity"><strong>Runtime paths reported by worker</strong><p>Running Python: ${esc(state.workerIdentity.python_executable)}</p><p>Loaded code directory: ${esc(state.workerIdentity.project_root)}</p></div>` : ''}
      <div class="form-actions">${state.client
        ? `<button class="btn warn" data-op="stop">${workerState === 'UNKNOWN' ? 'Retry worker shutdown' : 'Stop worker'}</button>`
        : `<button class="btn primary" data-op="start" ${state.bootstrapping ? 'disabled' : ''}>${state.bootstrapping ? 'Checking existing worker…' : 'Start worker'}</button>`}
      <span class="hint">Starting the worker does not open instrument ports.</span></div></div></div>
      <div class="card"><div class="card-head"><div><h2 class="card-title">Resource bindings</h2><p class="card-subtitle">OSA and PM400 must use separate VISA resources.</p></div><button class="btn small" data-op="save-settings" ${state.client && state.mode !== 'unknown' ? '' : 'disabled'}>Save settings</button></div>
      <div class="card-body"><div class="binding-list">${bindingRows}</div></div></div></div>
      <div class="stack"><div class="card"><div class="card-head"><div><h2 class="card-title">Fiber stages</h2><p class="card-subtitle">Left and right identified by USB serial number</p></div>${badge(left || right ? 'DETECTED' : 'UNKNOWN', left || right ? 'ready' : 'warn')}</div>
      <div class="card-body"><div class="metric"><span class="metric-label">LEFT · 2110148249-10</span><span class="metric-value" style="font-size:13px">${esc(left?.resource || 'Not detected')}</span></div>
      <div style="height:8px"></div><div class="metric"><span class="metric-label">RIGHT · 160721175410</span><span class="metric-value" style="font-size:13px">${esc(right?.resource || 'Not detected')}</span></div>
      <div class="form-actions">${connectionAction('fiber', state)}</div>
      <p class="hint">Connection reads controller state without changing outputs. Communication failures appear in the console.</p></div></div>
      <div class="card"><div class="card-head"><h2 class="card-title">Discovered resources</h2></div><div class="card-body">
      <div class="eyebrow">VISA · ${visa.length}</div>${visa.length ? visa.map((r) => `<p class="mono-muted">${esc(r)}</p>`).join('') : '<p class="hint">No VISA inventory results</p>'}
      <div class="separator"></div><div class="eyebrow">SERIAL · ${serial.length}</div>${serial.length ? serial.map((r) => `<p class="mono-muted">${esc(r.resource)} · ${esc(r.description)} · ${esc(r.serial || 'No serial number')}</p>`).join('') : '<p class="hint">No serial inventory results</p>'}${errors}</div></div></div></div>`;
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

function nativePlot(trace,cursor){
  const xs=trace.wavelength_nm,ys=trace.native_values,xmin=xs[0],xmax=xs.at(-1);let ymin=ys[0],ymax=ys[0];
  for(const y of ys){ymin=Math.min(ymin,y);ymax=Math.max(ymax,y);}
  const scale=Math.max(Math.abs(ymin),Math.abs(ymax))||1,lo=ymin/scale,span=ymax/scale-lo||1;
  const px=i=>48+(xs[i]-xmin)/(xmax-xmin||1)*718,py=i=>282-(ys[i]/scale-lo)/span*235;
  const points=decimateTrace(trace).map(i=>`${px(i).toFixed(1)},${py(i).toFixed(1)}`).join(' ');
  const active=Number.isInteger(cursor)&&cursor>=0&&cursor<xs.length;
  return `<svg viewBox="0 0 800 320" role="img" aria-label="OSA spectrum in ${esc(trace.native_unit)}; click to inspect a raw sample" data-osa-plot><path d="M48 47V282H766" stroke="#82979f" fill="none"/><polyline points="${points}" fill="none" stroke="#178c87" stroke-width="2"/>${active?`<path d="M${px(cursor).toFixed(1)} 47V282" stroke="#bc7628" stroke-dasharray="4 4"/><circle cx="${px(cursor).toFixed(1)}" cy="${py(cursor).toFixed(1)}" r="5" fill="#bc7628"/>`:''}<text x="48" y="304" class="axis-label">${esc(xmin)} nm</text><text x="766" y="304" text-anchor="end" class="axis-label">${esc(xmax)} nm</text><text x="8" y="36" class="axis-label">${esc(trace.native_unit)}</text><text x="8" y="52" class="axis-label">${esc(ymax)}</text><text x="8" y="282" class="axis-label">${esc(ymin)}</text></svg>`;
}

export function osa(state) {
  const device = state.status?.devices?.osa;
  const blocked = roleBlocked(state, 'osa') || device?.connected !== true || Boolean(device?.status_error);
  const cursor = Number.isInteger(state.cursor) ? state.cursor : null;
  const native=state.trace?.verified===true,ys=native?state.trace.native_values:state.trace?.power_dbm;
  const cursorValid = cursor !== null && Number.isFinite(state.trace?.wavelength_nm?.[cursor])&&Number.isFinite(ys?.[cursor]);
  const cursorText = cursorValid
    ? native?`${state.trace.wavelength_nm[cursor]} nm · ${ys[cursor]} ${state.trace.native_unit}`:`${state.trace.wavelength_nm[cursor].toFixed(3)} nm · ${ys[cursor].toFixed(3).replace('-', '−')} dBm`
    : 'Click the spectrum to inspect the nearest raw sample';
  const unavailable=blocked||state.archiveAvailable===false,disabled=unavailable?'disabled':'';
  const entries=state.archiveEntries||[];
  return pageHeader('INSTRUMENT / SPECTRUM','Optical spectrum analyzer','Read and save the existing trace. Front-panel settings are preserved.',connectionAction('osa',state))+`<div class="device-layout"><div class="card"><div class="card-head"><div><h2 class="card-title">${state.historical?'Historical capture':'Spectrum'} · Trace ${esc(state.trace?.trace||'A')}</h2><p class="card-subtitle">${esc(state.trace?.metadata?.identity||device?.identity||'Not connected')}</p></div>${badge(state.historical?'HISTORICAL':device?.connected?'ONLINE':'OFFLINE')}</div><div class="card-body"><div class="plot-box">${plot(state.trace,cursor)}</div><p class="cursor-readout">${esc(cursorText)}</p><p class="hint">DATA POINTS · ${state.trace?.wavelength_nm?.length||'—'}</p>${native?`<p class="hint">${esc(state.trace.metadata.read_finished_at)} · ${esc(state.trace.native_unit)} · Consistency unproven</p>`:''}</div></div>
  <div class="stack"><div class="card"><div class="card-head"><h2 class="card-title">Capture</h2></div><div class="card-body">${device?.status_error?`<div class="alert">Status read failed: ${esc(device.status_error)}</div>`:''}<div class="field"><label for="osa-name">Recording name</label><input id="osa-name" class="control" value="osa" maxlength="40" pattern="[A-Za-z0-9][A-Za-z0-9_-]{0,39}"></div><div class="field"><label for="osa-trace">Trace</label><select id="osa-trace" class="control">${['A','B','C','D','E','F','G'].map(t=>`<option value="${t}">${t}</option>`).join('')}</select></div><div class="form-actions"><button class="btn primary" data-op="osa-read" ${disabled}>Read trace</button><button class="btn" data-op="osa-export" ${!state.trace||state.exporting?'disabled':''}>Export CSV</button></div><details id="osa-sweep"><summary>Start sweep</summary><p class="hint">Runs one sweep. The driver verifies SINGLE or AUTO on the instrument; REPEAT is refused.</p><button class="btn" data-op="osa-acquire" ${disabled}>Start sweep</button></details><p class="hint">Save folder: ${esc(state.recordingRoot||'See Settings')}</p>${state.exportDirectory?`<p class="hint">Exported: ${esc(state.exportDirectory)}</p>`:''}</div></div>
  <div class="card"><div class="card-head"><h2 class="card-title">Saved captures</h2><button class="btn small" data-ui="osa-history-refresh" ${state.historyBusy?'disabled':''}>Refresh</button></div><div class="card-body">${state.historyError?`<p class="alert">${esc(state.historyError)}</p>`:''}${entries.map(e=>`<p><button class="btn small" data-ui="osa-history-load" data-archive="${esc(e.id)}" data-name="${esc(e.name)}" ${e.state!=='complete'||state.historyBusy?'disabled':''}>${esc(e.name)}</button> <span class="hint">${esc(e.state)} · ${esc(e.reference?.metadata?.read_finished_at||e.error||'Incomplete capture')}</span></p>`).join('')||'<p class="hint">No captures loaded</p>'}${state.historyHasMore?`<button class="btn" data-ui="osa-history-more" ${state.historyBusy?'disabled':''}>Load more</button>`:''}${state.historical?'<button class="btn" data-ui="osa-current">Show current</button>':''}</div></div></div></div>`;
}

export function voltage(state) {
  const device = state.status?.devices?.voltage;
  const blocked = roleBlocked(state, 'voltage') || device?.connected !== true || Boolean(device?.status_error);
  const rows = voltageRows(device);
  const age = sampleAgeLabel(device?.sample_age_s, 'Telemetry sample');
  return pageHeader('INSTRUMENT / OUTPUT', '8-channel voltage source', 'Monitor voltage and current per channel. The driver ramps in steps of at most 0.1 V, at least 50 ms apart.', connectionAction('voltage', state)) +
    (device ? `<div class="alert">${device.status_error ? `Status read failed: ${esc(device.status_error)}. Emergency zero is still available, but telemetry may not confirm the result.` : ''}Connection sets every channel to 0 V. Current zero evidence: ${esc(rows[0].zeroEvidence)}; This is host-observed telemetry only.</div>
    <p class="hint">${esc(age)} · Readings come from the latest telemetry frame. Inputs are next targets, not actual outputs.</p>
    <div class="section-title"><h2>Output channels</h2><button class="btn danger" data-op="voltage-zero" ${safetyBlocked(state, 'voltage') ? 'disabled' : ''}>Zero all channels</button></div>
    <div class="channel-grid">${rows.map((row) => `<div class="card channel-card"><div class="channel-head"><strong>CH ${row.channel.toString().padStart(2,'0')}</strong><span class="micro-led"></span></div>
      <span class="channel-reading">Measured ${esc(row.observed)}</span><span class="channel-current">Last completed command ${esc(row.requested)}</span><span class="channel-current">Measured current ${esc(row.current)}</span>
      ${trendSvg(state.voltageHistory, (sample) => sample.voltage_v[row.channel - 1], 0, 14, `voltage-${row.channel}`)}
      <span class="trend-scale">Fixed scale 0–14 V · Recent host samples</span>
      <label class="channel-target" for="voltage-${row.channel}">Next target voltage</label>
      <div class="channel-control"><input class="control" type="number" step="0.1" min="0" max="14" id="voltage-${row.channel}" value="${Number.isFinite(device.voltage_v?.[row.channel-1]) ? device.voltage_v[row.channel-1].toFixed(2) : '0'}" aria-label="CH${row.channel} Target voltage"><button class="btn small" data-op="voltage-apply" data-channel="${row.channel}" ${blocked ? 'disabled' : ''}>Apply</button></div></div>`).join('')}</div>` : empty('voltage'));
}

export function gain(state) {
  const device = state.status?.devices?.gain;
  const blocked = roleBlocked(state, 'gain') || device?.connected !== true || Boolean(device?.status_error);
  const safetyDisabled = safetyBlocked(state, 'gain') ? 'disabled' : '';
  const now = state.nowMs ?? performance.now();
  const evidence = Object.fromEntries(gainFields.map(name => [name, gainEvidence(device, name, now)]));
  const labels = { temperature_c: 'Measured temperature', target_c: 'Target temperature', current_ma: 'Current setpoint',
    tec_enabled: 'TEC', current_enabled: 'Current output' };
  const metrics = gainFields.map(name => {
    const field = evidence[name];
    const value = field.value === true ? 'On' : field.value === false ? 'Off'
      : Number.isFinite(field.value) ? `${field.value.toFixed(3)} ${name === 'current_ma' ? 'mA' : '°C'}` : 'Unknown';
    return `<div class="metric" data-gain-field="${name}"><span class="metric-label">${labels[name]}</span>
      <span class="metric-value">${esc(value)}${field.quality !== 'fresh' && value !== 'Unknown' ? ' (historical)' : ''}</span>
      <span class="metric-meta">${esc(field.quality)} · ${field.age === null ? 'Age unknown' : `${field.age.toFixed(1)} s ago`}</span>
      ${field.error || field.reason ? `<small>${esc(field.error || field.reason)}</small>` : ''}</div>`;
  }).join('');
  const enableCurrent = canEnableCurrent(state.roles?.gain, device, now);
  const tecOn = evidence.tec_enabled.quality === 'fresh' && evidence.tec_enabled.value === true;
  return pageHeader('INSTRUMENT / THERMAL', 'Gain Chip Driver', 'Temperature, TEC and injection current are protected by driver interlocks and the watchdog.', connectionAction('gain', state)) +
    (device ? `<div class="device-layout"><div class="stack"><div class="card"><div class="card-head"><div><h2 class="card-title">Field readbacks</h2><p class="card-subtitle">${esc(device.resource || '')}</p></div>${badge(device.state, device.connected ? 'ready' : 'warn')}</div><div class="card-body">${device.status_error ? `<div class="alert">Status read failed: ${esc(device.status_error)}; Check the watchdog and instrument outputs.</div>` : ''}<div class="metrics">${metrics}</div>
    <p class="hint">The five fields are sampled separately; values older than 5 s are stale. Reading TEC or current enable state may trigger an interlock shutdown.</p>
    ${device.last_command ? `<details><summary>Last confirmed command (not an independent readback)</summary><pre>${esc(JSON.stringify(device.last_command, null, 2))}</pre></details>` : ''}</div></div>
    <div class="card"><div class="card-head"><h2 class="card-title">Temperature trend</h2></div><div class="card-body">${trendSvg(state.gainHistory, (sample) => sample.temperature_c, 15, 40, 'gain-temperature')}<p class="hint">Fixed scale 15–40 °C · Recent host samples; not an independent temperature measurement.</p></div></div>
    <div class="card"><div class="card-head"><h2 class="card-title">Temperature and current setpoints</h2></div><div class="card-body"><div class="form-row two"><div class="field"><label for="gain-temp">Target temperature · 15–40 °C</label><input id="gain-temp" type="number" class="control" min="15" max="40" step="0.1" value="${esc(evidence.target_c.value)}"></div><div class="field"><label for="gain-current">Current setpoint · 0–200 mA</label><input id="gain-current" type="number" class="control" min="0" max="200" step="1" value="${esc(evidence.current_ma.value)}"></div></div><div class="form-actions"><button class="btn" data-op="gain-set-temp" ${blocked ? 'disabled' : ''}>Set temperature</button><button class="btn" data-op="gain-set-current" ${blocked ? 'disabled' : ''}>Set current</button></div></div></div></div>
    <div class="stack"><div class="card"><div class="card-head"><h2 class="card-title">Output controls</h2></div><div class="card-body"><p class="hint">To enable current, TEC must be on and measured temperature must stay within target ±0.2 °C for at least 5 s. The driver verifies this interlock.</p><div class="form-actions"><button class="btn primary" data-op="gain-enable-tec" ${blocked ? 'disabled' : ''}>Enable TEC</button><button class="btn warn" data-op="gain-disable-tec" ${safetyDisabled}>Disable TEC</button></div><div class="separator"></div><div class="form-actions"><button class="btn" data-op="gain-stable" ${blocked || !tecOn ? 'disabled' : ''}>Wait for stability</button><button class="btn primary" data-op="gain-enable-current" ${enableCurrent ? '' : 'disabled'}>Enable current</button><button class="btn danger" data-op="gain-disable-current" ${safetyDisabled}>Disable current</button></div></div></div>
    <div class="alert">Shutdown disables current before TEC. Communication failures are reported; sending a command does not prove physical shutdown.</div></div></div>` : empty('gain'));
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
