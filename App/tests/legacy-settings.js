// Test-only retired v2 device configuration renderer. Never bundled.
import {esc,connectionAction} from '../web/panels.js';
function pageHeader(eyebrow, title, intro, actions = '') {
  return `<div class="page-header"><div><div class="eyebrow">${esc(eyebrow)}</div>
    <h1>${esc(title)}</h1><p class="page-intro">${esc(intro)}</p></div>
    <div class="page-actions">${actions}</div></div>`;
}

function badge(text, kind = '') {
  return `<span class="badge ${kind}">${esc(text)}</span>`;
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
