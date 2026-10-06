/** PM400-specific capability-aware panel; no transport or raw SCPI lives here. */

function esc(value) {
  return String(value ?? '').replace(/[&<>"']/g, (character) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[character]);
}

function inputFor(setting, busy) {
  const id = `pm-value-${setting.key}`;
  const disabled = setting.write_supported && !busy ? '' : 'disabled';
  if (!setting.writable) return '';
  const options = {
    bool: [['true', 'On'], ['false', 'Off']],
    power_unit: [['W', 'W'], ['DBM', 'dBm']],
    adapter_type: [['photodiode', 'Photodiode'], ['thermal', 'Thermopile'], ['pyro', 'Pyroelectric']],
  }[setting.kind];
  if (options) return `<select class="control" id="${esc(id)}" ${disabled}>${options.map(([value, label]) =>
    `<option value="${value}">${esc(label)}</option>`).join('')}</select>`;
  if (setting.kind === 'measurement_kind') return `<select class="control" id="${esc(id)}" ${disabled || !setting.choices?.length ? 'disabled' : ''}>
    ${(setting.choices || []).map((choice) =>
      `<option value="${esc(choice.key)}" ${choice.supported ? '' : 'disabled'}>${esc(choice.label)} · ${esc(choice.key)}</option>`).join('')}</select>`;
  const htmlType = setting.kind === 'date' ? 'date' : setting.kind === 'time' ? 'time'
    : ['int', 'register8', 'register16', 'line_frequency', 'float'].includes(setting.kind)
      ? 'number' : 'text';
  const step = setting.kind === 'float' ? 'any' : '1';
  return `<input class="control" id="${esc(id)}" type="${htmlType}" step="${step}"
    placeholder="Target value" ${disabled}>`;
}

function settingRow(setting, reading, busy) {
  const readDisabled = !setting.read_supported || busy ? 'disabled' : '';
  const writeDisabled = !setting.write_supported || busy ? 'disabled' : '';
  const reason = !setting.write_supported && setting.writable
    ? `Write unavailable: ${(setting.write_requirements || []).join(', ') || 'Unavailable for this sensor or function'}`
    : setting.sensitive ? 'Sensitive setting: writing requires a separate confirmation' : '';
  const selector = setting.selectors?.length ? `<select class="control pm-select" id="pm-selector-${esc(setting.key)}" ${busy ? 'disabled' : ''}>
    <option value="">Current value</option>${setting.selectors.map((value) =>
      `<option value="${esc(value)}">${esc(value.toUpperCase())}</option>`).join('')}</select>` : '';
  const group = setting.grouped ? `<select class="control pm-select" id="pm-group-${esc(setting.key)}" ${busy ? 'disabled' : ''}>
    ${['measurement', 'auxiliary', 'operation', 'questionable'].map((value) =>
      `<option value="${value}">${value}</option>`).join('')}</select>` : '';
  return `<div class="pm-setting"><div class="pm-setting-title"><strong>${esc(setting.label)}</strong>
    <small>${esc(setting.key)}${setting.sensitive ? ' · CONFIRM' : ''}</small></div>
    <div class="pm-setting-value">${reading === undefined ? '—' : esc(
      typeof reading === 'object' ? JSON.stringify(reading) : reading)}</div>
    <div class="pm-setting-controls">${group}${selector}${inputFor(setting, busy)}
      ${setting.readable ? `<button class="btn small" data-op="pm-read" data-setting="${esc(setting.key)}" ${readDisabled}>Read</button>` : ''}
      ${setting.writable ? `<button class="btn small ${setting.sensitive ? 'warn' : ''}" data-op="pm-write" data-setting="${esc(setting.key)}" ${writeDisabled}>Apply</button>` : ''}
    </div>${reason ? `<small class="pm-setting-hint">${esc(reason)}</small>` : ''}</div>`;
}

function section(title, items, state) {
  if (!items.length) return '';
  return `<div class="section-title"><h2>${esc(title)}</h2><span>${items.length} items</span></div>
    <div class="pm-settings card">${items.map((item) =>
      settingRow(item, state.pmReadings?.[item.key], state.busy)).join('')}</div>`;
}

function plotHistory(history, kind) {
  if (!history?.length) return '<div class="plot-empty">Measurement trend appears after readings are taken</div>';
  const values = history.filter((sample) => sample.kind === kind)
    .map((sample) => Number(sample.value)).filter(Number.isFinite).slice(-60);
  if (!values.length) return '<div class="plot-empty">No valid values yet</div>';
  const min = Math.min(...values), max = Math.max(...values), span = max - min || 1;
  const points = values.map((value, index) =>
    `${(20 + index * 560 / Math.max(1, values.length - 1)).toFixed(1)},${(155 - (value - min) / span * 120).toFixed(1)}`).join(' ');
  return `<svg viewBox="0 0 600 180" role="img" aria-label="PM400 recent measurement trend">
    <path d="M20 35V155H580" stroke="#39515e" fill="none"/>
    <polyline points="${points}" fill="none" stroke="#54d6cf" stroke-width="2"/></svg>`;
}

function measurementTab(state, catalog) {
  const items = catalog.measurements || [];
  const chosen = items.some((item) => item.key === state.pmKind && item.supported)
    ? state.pmKind : items.find((item) => item.supported)?.key || '';
  const measurement = state.pmMeasurement;
  return `<div class="device-layout"><div class="stack"><div class="card"><div class="card-head"><h2 class="card-title">Live measurement</h2></div>
    <div class="card-body"><div class="field"><label for="pm-kind">Measurement type</label><select class="control" id="pm-kind" ${state.busy ? 'disabled' : ''}>
    ${items.map((item) => `<option value="${esc(item.key)}" ${item.key === chosen ? 'selected' : ''} ${item.supported ? '' : 'disabled'}>
      ${esc(item.label)} · ${esc(item.unit)}</option>`).join('')}</select></div>
    <div class="metric pm-reading"><span class="metric-label">LAST READING</span>
      <span class="metric-value cyan">${measurement ? `${esc(measurement.value)} ${esc(measurement.unit)}` : '—'}</span>
      <span class="metric-meta">${measurement ? esc(measurement.kind || chosen) : 'Not read yet'}</span></div>
    <div class="form-actions"><button class="btn primary" data-op="pm-measure" ${chosen && !state.busy ? '' : 'disabled'}>Read once</button></div>
    <p class="hint">Available measurements depend on the connected sensor. Measurement preserves sensor calibration settings.</p></div></div></div>
    <div class="card"><div class="card-head"><h2 class="card-title">Session trend</h2></div>
      <div class="card-body"><div class="plot-box pm-plot">${plotHistory(state.pmHistory, chosen)}</div>
      <p class="hint">Shows readings of the selected type from this session. Readings are not automatically saved as experiment results.</p></div></div></div>`;
}

function commandSection(catalog, state) {
  const groups = [
    ['measurement', 'Measurement controls'], ['sense', 'Sensor calibration'], ['status', 'Status controls'],
    ['system', 'System'], ['root', 'Instrument operations'],
  ];
  return groups.map(([key, title]) => {
    const items = (catalog.commands || []).filter((item) => item.section === key);
    if (!items.length) return '';
    return `<div class="section-title"><h2>${title}</h2></div><div class="card pm-commands">
      ${items.map((item) => {
        const returned = state.pmCommandResults || {};
        const hasResult = Object.hasOwn(returned, item.key);
        const value = returned[item.key];
        const shown = value === null ? 'Completed (no return value)'
          : typeof value === 'object' ? JSON.stringify(value) : String(value);
        return `<div><strong>${esc(item.label)}</strong><small>${esc(item.key)}</small>
        <button class="btn small ${item.confirm ? 'warn' : ''}" data-op="pm-command" data-command="${esc(item.key)}"
          ${item.supported && !state.busy ? '' : 'disabled'}>Run</button>
        ${hasResult ? `<output class="pm-command-result">Last successful result: ${esc(shown)}</output>` : ''}</div>`;
      }).join('')}</div>`;
  }).join('');
}

export function renderPm400(state, connectionAction = () => '') {
  const device = state.status?.devices?.pm400;
  const header = `<div class="page-header"><div><div class="eyebrow">INSTRUMENT / OPTICAL POWER</div>
    <h1>PM400 power meter</h1><p class="page-intro">Measurements, sensor calibration, status and system settings use the PM400 driver’s supported operation catalog.</p></div>
    <div class="page-actions">${connectionAction('pm400', state)}</div></div>`;
  if (!device) return header + `<div class="card empty-panel"><span class="empty-icon">◌</span><h2>PM400 not connected</h2>
    <p>Select a separate VISA resource in Device setup. Connection and normal close preserve front-panel settings.</p>
    <div class="form-actions" style="justify-content:center"><button class="btn" data-page="settings">Open device setup</button></div></div>`;
  const operational = device.connected === true && !device.status_error &&
    ['READY', 'ACTIVE'].includes(device.state);
  const panelState = { ...state, busy: state.busy || !operational };
  const sensor = device.sensor || {};
  const caps = sensor.capabilities || {};
  const catalog = device.catalog || { measurements: [], settings: [], commands: [] };
  const tab = state.pmTab || 'measurement';
  const tabs = [
    ['measurement', 'Measurement'], ['sensor', 'Sensor settings'],
    ['system', 'System / status'], ['advanced', 'Advanced operations'],
  ];
  const items = catalog.settings || [];
  const preexistingErrors = Array.isArray(device.last_preexisting_errors)
    ? device.last_preexisting_errors : [];
  const errorEvidence = `<div class="card"><div class="card-head"><h2 class="card-title">Errors present before the last checked operation</h2></div>
    <div class="card-body"><p class="hint">Cached by the driver, not the current error queue. This view sends no instrument commands.</p>
    ${preexistingErrors.length ? `<ul>${preexistingErrors.map((error) =>
      `<li><strong>${esc(error.code)}</strong> ${esc(error.message)} <small class="mono-muted">${esc(error.raw)}</small></li>`).join('')}</ul>`
      : '<p class="hint">No cached records</p>'}</div></div>`;
  const sensorContent = section('Sensor corrections and ranges', items.filter((item) => item.section === 'sense'), panelState)
    + section('Input and adapter', items.filter((item) => item.section === 'input'), panelState);
  const systemContent = errorEvidence
    + section('System and identity', items.filter((item) => ['root', 'system'].includes(item.section)), panelState)
    + section('Status registers', items.filter((item) => item.section === 'status'), panelState)
    + section('Display and calibration information', items.filter((item) => ['display', 'calibration'].includes(item.section)), panelState);
  const advancedContent = section('Measurement configuration', items.filter((item) => item.section === 'measurement'), panelState)
    + commandSection(catalog, panelState);
  const body = { measurement: measurementTab(panelState, catalog), sensor: sensorContent,
    system: systemContent, advanced: advancedContent }[tab] || measurementTab(panelState, catalog);
  return header + `<div class="card pm-identity"><div><span class="eyebrow">INSTRUMENT</span>
      <strong>${esc(device.instrument?.manufacturer || 'THORLABS')} ${esc(device.instrument?.model || 'PM400')}</strong>
      <small>${esc(device.instrument?.serial_number || '')}</small></div>
      <div><span class="eyebrow">SENSOR</span><strong>${esc(sensor.name || sensor.model || 'Unidentified')}</strong>
      <small>${esc(sensor.serial_number || '')}</small></div>
      <div><span class="eyebrow">CAPABILITIES</span><strong>${Object.entries(caps).filter(([, enabled]) => enabled).map(([name]) => esc(name)).join(' · ') || 'Not read'}</strong>
      <small>Unsupported operations are disabled below</small></div></div>
    <div class="pm-tabs" role="tablist">${tabs.map(([id, label]) =>
      `<button role="tab" aria-selected="${id === tab}" class="${id === tab ? 'active' : ''}" data-pm-tab="${id}">${label}</button>`).join('')}</div>
    ${operational ? '' : `<div class="alert">PM400 controls unavailable: ${esc(device.status_error || device.state || 'Status unknown')}. Check the instrument before retrying disconnect or recovering the connection.</div>`}
    <div class="pm-tab-body">${body}</div><div class="alert" style="margin-top:20px">
      Zero calibration, responsivity, adapter type and reset change instrument state; each requires a separate confirmation. Readings and replies do not replace independent physical verification.</div>`;
}

export function recordPmSample(history, measurement, fallbackKind) {
  if (!measurement || typeof measurement.value !== 'number' ||
      !Number.isFinite(measurement.value)) return history;
  const kind = typeof measurement.kind === 'string' && measurement.kind
    ? measurement.kind : fallbackKind;
  return [...history, { kind, value: measurement.value }].slice(-60);
}

export function parsePmValue(kind, raw) {
  if (typeof raw !== 'string' || !raw.trim()) throw new Error('PM400 value must be finite or selected');
  if (kind === 'bool') {
    if (!['true', 'false'].includes(raw)) throw new Error('PM400 value must be boolean');
    return raw === 'true';
  }
  if (['int', 'register8', 'register16', 'line_frequency'].includes(kind)) {
    const value = Number(raw);
    if (!Number.isSafeInteger(value)) throw new Error('PM400 value must be an integer');
    if (kind === 'register8' && (value < 0 || value > 255)) throw new Error('PM400 register8 range is 0–255');
    if (kind === 'register16' && (value < 0 || value > 65535)) throw new Error('PM400 register16 range is 0–65535');
    if (kind === 'line_frequency' && ![50, 60].includes(value)) throw new Error('PM400 line frequency range is 50 or 60 Hz');
    return value;
  }
  if (kind === 'float') {
    const value = Number(raw);
    if (!Number.isFinite(value)) throw new Error('PM400 value must be finite');
    return value;
  }
  if (kind === 'power_unit' && ['W', 'DBM'].includes(raw)) return raw;
  if (kind === 'adapter_type' && ['photodiode', 'thermal', 'pyro'].includes(raw)) return raw;
  if (kind === 'measurement_kind' && ['power', 'current', 'voltage', 'energy', 'frequency',
    'power_density', 'energy_density', 'resistance', 'temperature'].includes(raw)) return raw;
  if (kind === 'date' && /^\d{4}-\d{2}-\d{2}$/.test(raw)) return raw;
  if (kind === 'time' && /^\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?$/.test(raw)) return raw;
  throw new Error(`Unsupported PM400 ${kind} value`);
}

export function buildPmSettingAction(setting, operation, options = {}) {
  if (!setting || typeof setting.key !== 'string') throw new Error('Unknown PM400 setting');
  if (!['read', 'write'].includes(operation)) throw new Error('Unknown PM400 operation');
  if (operation === 'read' && (!setting.readable || !setting.read_supported)) {
    throw new Error('PM400 read is unsupported by this sensor');
  }
  if (operation === 'write' && (!setting.writable || !setting.write_supported)) {
    throw new Error('PM400 write is unsupported by this sensor');
  }
  const payload = { role: 'pm400', name: operation, setting: setting.key };
  if (setting.grouped) {
    if (!['measurement', 'auxiliary', 'operation', 'questionable'].includes(options.group)) {
      throw new Error('Choose a PM400 status group');
    }
    payload.group = options.group;
  }
  if (options.selector) {
    if (!setting.selectors?.includes(options.selector)) {
      throw new Error('Unsupported PM400 limit selector');
    }
    payload.selector = options.selector;
  }
  if (operation === 'write') {
    if (setting.sensitive && options.confirmed !== true) {
      throw new Error('PM400 sensitive write requires confirmation');
    }
    if (!options.selector) {
      if (setting.kind === 'measurement_kind' && !setting.choices?.some(
        (choice) => choice.key === options.raw && choice.supported)) {
        throw new Error('PM400 measurement kind is unsupported by this sensor');
      }
      payload.value = parsePmValue(setting.kind, options.raw);
    }
    if (setting.sensitive) payload.confirm = true;
  }
  return payload;
}
