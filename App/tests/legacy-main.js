// Test-only v2 regression harness. Never bundled or used as a console entry.
import { probeExistingWorker, tauriClient } from '../web/api.js';
import { installCloseGuard } from '../web/lifecycle.js';
import { baselineConfirmations, cleanupWarning, connectRequest, fiberMoveRequest, markStatusUnknown, numericSetting, workerSubtitle, shutdownError } from '../web/operations.js';
import { buildPmSettingAction, recordPmSample } from '../web/pm400.js';
import { appendTelemetry, osaCursorIndex, previewStageMove } from '../web/view-model.js';
import * as panels from '../web/panels.js';
import { roles, sameConnection, canApplySnapshot, canSendNormal, canSendSafety,
  canEnableCurrent, canResume, safetyIntent, intentRank } from '../web/control-state.js';

const state = {
  page: 'overview', client: null, mode: null, desiredMode: 'real',
  pythonPath: 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe',
  bindings: { osa: 'GPIB0::4::INSTR', pm400: null, voltage: null, gain: null },
  inventory: null, status: null, trace: null, cursor: null, pmMeasurement: null, busy: false,
  lastKnown: {}, lastKnownAt: {},
  workerIdentity: null, sessionContext: null, roleContexts: {},
  roles: {}, closing: false, shutdownPending: null, shutdownHistory: [], snapshotRevision: -1,
  fiberAdopted: {},
  rawDevices: {},
  voltageHistory: [], gainHistory: [],
  bootstrapping: true,
  pmTab: 'measurement', pmKind: 'power', pmReadings: {}, pmHistory: [], pmCommandResults: {},
};

const content = document.getElementById('content');
const navigation = document.getElementById('navigation');
const notice = document.getElementById('notice');
// Sibling of the frequently redrawn controls: never detach the WebGL canvas during polling.
const panel = Object.fromEntries(
  ['overview', 'settings', 'fiber', 'osa', 'voltage', 'gain', 'pm400']
    .map((name) => [name, panels[name]]),
);

const focusSelectors = {
  stageScene: '[data-stage-scene]', page: '[data-page]', pmTab: '[data-pm-tab]', op: '[data-op]',
};

function focusedControl() {
  const active = document.activeElement;
  const root = navigation.contains?.(active) ? 'navigation'
    : content.contains?.(active) ? 'content' : null;
  if (!root) return null;
  const kind = Object.keys(focusSelectors).find((key) => active.dataset?.[key] !== undefined);
  if (!kind) return null;
  const fields = kind === 'op' ? ['op', 'role', 'channel', 'side', 'setting', 'command'] : [kind];
  return { root, page: state.page, kind,
    label: kind === 'op' ? active.textContent?.trim() : null,
    identity: Object.fromEntries(fields.map((key) => [key, active.dataset[key]])) };
}

function restoreControlFocus(previous) {
  if (!previous || previous.page !== state.page) return;
  const root = previous.root === 'navigation' ? navigation : content;
  const matches = [...root.querySelectorAll(focusSelectors[previous.kind])].filter((candidate) =>
    Object.entries(previous.identity).every(([key, value]) => candidate.dataset[key] === value));
  if (matches.length === 1 && !matches[0].disabled &&
      (previous.label === null || matches[0].textContent?.trim() === previous.label)) {
    matches[0].focus({ preventScroll: true });
  }
}

function render() {
  state.nowMs = performance.now();
  const focus = focusedControl();
  navigation.innerHTML = panels.sections.map(([id, title, icon]) =>
    `<button class="nav-item ${state.page === id ? 'active' : ''}" data-page="${id}">
      <span class="nav-icon">${icon}</span><span>${panels.esc(title)}</span></button>`).join('');
  content.innerHTML = panel[state.page](state);
  restoreControlFocus(focus);
  document.getElementById('current-page-title').textContent =
    panels.sections.find(([id]) => id === state.page)?.[1] || 'Overview';
  document.getElementById('sidebar-mode').textContent = state.mode === 'real'
    ? 'Real hardware mode'
      : state.client ? 'Worker status unknown' : 'Not started';
  document.getElementById('sidebar-subtitle').textContent = workerSubtitle(state);
  document.getElementById('sidebar-run-dot').classList.toggle('on', Boolean(state.client));
  const pill = document.getElementById('session-pill');
  pill.className = `session-pill ${state.mode === 'real' ? 'real' : ''}`;
  pill.innerHTML = `<span class="pill-dot"></span> ${state.mode === 'real' ? 'REAL HARDWARE' : state.client ? 'Status unknown' : 'Not started'}`;
}

function formContextKey() {
  if (state.page === 'settings') {
    if (!state.client || state.mode !== 'real') return null;
    // Resource menus are session configuration, not a connected instrument’s
    // form. Keep their native popup alive, but redraw if ownership/health changes.
    const roles = Object.entries(state.status?.devices || {}).sort(([a], [b]) => a.localeCompare(b))
      .map(([role, device]) => [role, device.resource, device.connected, device.state, device.status_error]);
    return JSON.stringify([state.mode, state.sessionContext, state.page, roles,
      Object.values(state.roles).map(item => [item.context, item.mode, item.confirmed])]);
  }
  const device = state.status?.devices?.[state.page];
  if (device?.connected !== true || device.status_error) return null;
  if (state.page !== 'fiber' &&
      (typeof device.resource !== 'string' || !device.resource.trim())) return null;
  if (state.page === 'fiber') {
    const available = [device.left, device.right].filter((side) => side?.available);
    if (!available.length || available.some((side) => {
      const serial = side.serial_number ?? side.serial;
      return typeof serial !== 'string' || !serial.trim() ||
        typeof side.resource !== 'string' || !side.resource.trim();
    })) return null;
  }
  const identity = state.page === 'fiber'
    ? [device.left, device.right].map((side) =>
      [side?.serial_number ?? side?.serial, side?.resource, side?.available])
    : device.resource;
  const control = state.roles[state.page];
  return JSON.stringify([state.mode, state.page, identity, control?.context,
    control?.generation, control?.confirmed, control?.mode]);
}

function captureFormDrafts() {
  return [...content.querySelectorAll('input[id], select[id], textarea[id]')]
    .filter((field) => !field.disabled)
    .map((field) => [field.id, field.value]);
}

function restoreFormDrafts(drafts) {
  const stageSides = new Set();
  for (const [id, value] of drafts) {
    const field = document.getElementById(id);
    if (field && !field.disabled) {
      field.value = value;
      const side = /^(left|right)-[xyz]$/.exec(id)?.[1];
      if (side) stageSides.add(side);
    }
  }
  for (const side of stageSides) updateStagePreview(side);
}

function isEditingForm() {
  return Boolean(document.activeElement?.closest('#content') &&
    document.activeElement?.matches('input,select'));
}

function showNotice(message, error = false) {
  notice.textContent = message;
  notice.className = `notice${error ? ' error' : ''}`;
  notice.hidden = false;
  clearTimeout(showNotice.timer);
  showNotice.timer = setTimeout(() => { notice.hidden = true; }, error ? 10000 : 6000);
}

function confirmAction(message) {
  return window.confirm(`${message}\n\nVerify the instrument, connections and nearby personnel before confirming.`);
}

async function request(method, params = {}) {
  if (!state.client) throw new Error('Start the worker first');
  const role = ['connect', 'disconnect', 'action', 'resume'].includes(method) ? params.role : null;
  const context = role ? state.roleContexts[role] : state.sessionContext;
  const control = role ? state.roles[role] : null;
  const generation = control?.generation;
  const pending = control?.normalPending;
  const client = state.client;
  if (!context && !['ping', 'status'].includes(method)) throw new Error('Worker identity is not confirmed');
  try {
    const reply = await client.request(method, params, context || null);
    if (client !== state.client) throw new Error('Reply from a previous worker retained as history only; not applied');
    if (role) {
      const active = state.roles[role];
      const intent = safetyIntent(method, params);
      const connecting = method === 'connect' && reply.context?.session_id === context?.session_id &&
        active && pending && active.normalPending === pending && !state.closing && !active.safetyPending &&
        !active.unknown && !active.hostRestricted && !active.stopHeld &&
        (active.context.connection_id === null || sameConnection(active.context, reply.context)) &&
        reply.context.epoch >= active.context.epoch;
      const current = sameConnection(active?.context, reply.context) &&
        reply.context.epoch >= active.context.epoch &&
        (intent || (active.generation === generation && context.epoch === reply.context.epoch));
      if (!connecting && !current) throw new Error('Reply from a previous connection or generation ignored; control authority remains unchanged');
      if (connecting) {
        if (!sameConnection(active.context, reply.context)) adoptRole(role, reply.context, -1, true);
        else active.confirmed = true;
      }
      else if (reply.context.epoch > active.context.epoch) {
        revokeRole(role);
        active.context = { ...reply.context };
        state.roleContexts[role] = { ...reply.context };
      }
      if (intent) {
        active.stopHeld = intent !== 'disconnect';
        active.mode = intent === 'disconnect' ? 'DISCONNECTED' : 'STOP_HELD';
        active.progress = 'Completed (driver call; not a physical measurement)';
      }
      if (method === 'resume') {
        if (reply.result?.resumed !== true || state.closing) throw new Error('Resume was not authorized by the current backend');
        active.unknown = false; active.hostRestricted = false;
        active.stopHeld = false; active.confirmed = false;
        // Query revisions do not date independently sampled host restrictions.
        // Fence the whole role until a query begun after this recovery confirms it.
        active.recoveryStatusAfter = statusSequence;
        active.recoveryPending = { context: { ...active.context }, generation: active.generation };
        active.progress = 'Resume authorized; waiting for current status confirmation';
      }
    }
    return reply.result;
  } catch (error) {
    const active = state.roles[role];
    const belongs = active && ((active === control && control.generation === generation &&
      sameConnection(context, control.context)) || (method === 'connect' && pending && active.normalPending === pending));
    if (client === state.client && role && belongs &&
        (error.transportUnknown || ['failed_after_call_started', 'completed_readback_failed'].includes(error.phase))) {
      revokeRole(role);
      active.unknown = true;
      active.mode = 'UNKNOWN';
      active.progress = 'Unknown';
    }
    throw error;
  }
}

function clearRoleResults(role) {
  delete state.lastKnown[role];
  delete state.lastKnownAt[role];
  if (role === 'osa') { state.trace = null; state.cursor = null; }
  if (role === 'voltage') state.voltageHistory = [];
  if (role === 'gain') state.gainHistory = [];
  if (role === 'pm400') {
    state.pmMeasurement = null; state.pmReadings = {}; state.pmHistory = []; state.pmCommandResults = {};
  }
  if (role === 'fiber') state.fiberAdopted = {};
}

function revokeRole(role) {
  const control = state.roles[role];
  if (control) { control.confirmed = false; control.generation++; control.recoveryPending = null; }
  clearRoleResults(role);
  // Revoke already rendered evidence immediately, before any later cache reply.
  for (const device of [state.status?.devices?.[role], state.rawDevices[role]]) {
    if (role === 'fiber' && device) for (const side of ['left', 'right']) {
      if (device[side]) device[side] = { ...device[side], baseline_known: false,
        nominal_authorized: false, estimated_position_um: null };
    }
    if (role === 'gain' && device?.fields) for (const [name, field] of Object.entries(device.fields)) {
      device.fields[name] = { ...field, quality: 'unknown', reason: 'Control context revoked; waiting for a current-generation readback' };
    }
  }
}

function invalidatePendingGain(device, control) {
  const names = new Set();
  const fieldsByAction = { set_temperature: ['target_c'], set_current: ['current_ma'],
    enable_current: ['current_ma', 'current_enabled'], enable_tec: ['tec_enabled'] };
  for (const name of fieldsByAction[control?.normalPending?.name] || []) names.add(name);
  if (control?.safetyPending) {
    names.add('current_enabled');
    if (control.safetyPending.intent !== 'current_off') names.add('tec_enabled');
  }
  for (const name of names) if (device?.fields?.[name]) {
    device.fields[name] = { ...device.fields[name], quality: 'unknown', reason: 'Operation pending; waiting for an independent readback' };
  }
}

function adoptRole(role, context, revision, confirmed = false) {
  const previous = state.roles[role];
  const retired = previous?.retired || new Set();
  if (previous?.context.connection_id && !sameConnection(previous.context, context)) {
    retired.add(previous.context.connection_id);
  }
  clearRoleResults(role);
  state.roles[role] = { context: { ...context }, revision, mode: 'UNKNOWN', confirmed,
    normalPending: previous?.normalPending?.method === 'connect' ? previous.normalPending : null,
    safetyPending: null,
    generation: (previous?.generation || 0) + 1, retired };
  state.roleContexts[role] = { ...context };
  return state.roles[role];
}

function acceptContexts(snapshot) {
  if (!snapshot?.session_id) throw new Error('Worker did not provide a session identity');
  state.sessionContext = { session_id: snapshot.session_id, connection_id: null, epoch: 0 };
  state.roles = {}; state.roleContexts = {}; state.rawDevices = {}; state.snapshotRevision = -1;
  for (const role of roles) {
    const value = snapshot.roles?.[role];
    if (value?.session_id === snapshot.session_id) adoptRole(role,
      { session_id: value.session_id, connection_id: value.connection_id, epoch: value.epoch }, -1,
      value.connection_id === null);
  }
}

function acceptStatus(snapshot, timing, handshake = false, sequence = 0) {
  if (handshake) acceptContexts(snapshot);
  if (snapshot?.session_id !== state.sessionContext?.session_id) {
    throw new Error('Worker session changed; an explicit startup handshake is required');
  }
  const rawDevices = { ...state.rawDevices };
  const devices = {};
  if (snapshot.closing || snapshot.host_transport?.closing) state.closing = true;
  for (const role of roles) {
    let control = state.roles[role];
    if (!control) continue;
    if (control.recoveryStatusAfter != null && sequence <= control.recoveryStatusAfter) {
      // Keep the existing presentation/age; do not adopt old identity, safety,
      // backend mode or host restriction. Other roles and closing still apply.
      if (state.status?.devices?.[role]) devices[role] = structuredClone(state.status.devices[role]);
      continue;
    }
    const value = snapshot.roles?.[role];
    const restricted = Boolean(snapshot.host_transport?.failure || snapshot.host_transport?.roles?.[role]);
    if (restricted && !control.hostRestricted) { revokeRole(role); control.hostRestricted = true; }
    const context = value && { session_id: value.session_id, connection_id: value.connection_id, epoch: value.epoch };
    let accepted = false;
    const newConnection = context && context.session_id === state.sessionContext.session_id &&
      !sameConnection(control.context, context) && !control.retired.has(context.connection_id) &&
      value.revision > control.revision && context.epoch >= control.context.epoch &&
      (control.context.connection_id === null || context.connection_id === null ||
        context.epoch > control.context.epoch || control.normalPending?.method === 'connect');
    if (newConnection) {
      const confirmed = control.normalPending?.method === 'connect' && !restricted;
      control = adoptRole(role, context, -1, confirmed);
      control.hostRestricted = restricted;
    }
    if (value && canApplySnapshot(control, { context, revision: value.revision })) {
      accepted = true;
      if (context.epoch > control.context.epoch) revokeRole(role);
      control.context = context;
      state.roleContexts[role] = { ...context };
      control.revision = value.revision;
      control.backendMode = value.state;
      control.activeRequest = value.active_request_id || value.pending_request_id;
      control.safety = structuredClone(value.safety || {});
      control.safetyEpoch = context.epoch;
      if (value.state === 'STOP_HELD') control.stopHeld = true;
      if (['UNKNOWN', 'FAULT', 'RETAINED'].includes(value.state)) control.unknown = true;
      if (snapshot.devices?.[role]) rawDevices[role] = { ...structuredClone(snapshot.devices[role]), timing };
      else delete rawDevices[role];
      const recovery = control.recoveryPending;
      if (recovery && recovery.generation === control.generation &&
          sameConnection(recovery.context, context) && recovery.context.epoch === context.epoch &&
          value.state === 'READY' && !control.unknown && !control.hostRestricted && !state.closing) {
        control.confirmed = true;
        control.recoveryPending = null;
        control.progress = 'Controls restored; previous commands were not replayed';
      }
    }
    control.mode = state.closing ? 'CLOSING' : control.hostRestricted || control.unknown ? 'UNKNOWN'
      : control.safetyPending ? 'STOPPING' : control.stopHeld ? 'STOP_HELD' : control.backendMode || 'UNKNOWN';
    const device = rawDevices[role] && structuredClone(rawDevices[role]);
    if (!device) continue;
    devices[role] = device;
    if (role === 'gain') invalidatePendingGain(device, control);
    if (role === 'fiber') {
      for (const side of ['left', 'right']) if (device[side]) {
        if (control.hostRestricted || control.unknown || !device[side].baseline_known ||
            device[side].restricted || device[side].fault) delete state.fiberAdopted[side];
        if (!state.fiberAdopted[side]) device[side] = { ...device[side], baseline_known: false,
          nominal_authorized: false, estimated_position_um: null };
      }
    }
    if (control.hostRestricted || control.unknown) devices[role] = { ...device, state: 'UNKNOWN',
      status_error: device.status_error || 'Host transport / operation outcome unknown; explicit recovery required' };
    if (accepted && device.connected && !device.status_error && !control.unknown && !control.hostRestricted) {
      state.lastKnown[role] = device;
      state.lastKnownAt[role] = snapshot.observed_at;
      if (role === 'voltage') state.voltageHistory = appendTelemetry(state.voltageHistory, device, role);
      if (role === 'gain') state.gainHistory = appendTelemetry(state.gainHistory, device, role);
    }
  }
  const newer = Number.isSafeInteger(snapshot.revision) && snapshot.revision > state.snapshotRevision;
  state.rawDevices = rawDevices;
  state.status = { ...(newer || !state.status ? structuredClone(snapshot) : state.status), devices };
  if (newer) state.snapshotRevision = snapshot.revision;
  state.mode = state.status.mode;
}

let statusRefresh = null;
let statusSequence = 0;
function refreshStatus() {
  if (statusRefresh) return statusRefresh;
  statusRefresh = refreshStatusOnce().finally(() => { statusRefresh = null; });
  return statusRefresh;
}
async function refreshStatusOnce() {
  if (!state.client) return;
  const sequence = ++statusSequence;
  try {
    const started = performance.now();
    const snapshot = await request('status');
    const receivedAtMs = performance.now();
    acceptStatus(snapshot, { roundTripMs: receivedAtMs - started, receivedAtMs }, false, sequence);
    return sequence;
  } catch (error) {
    if (error.phase === 'rejected_before_call') throw error;
    markStatusUnknown(state);
    for (const role of roles) if (state.roles[role]) {
      revokeRole(role); state.roles[role].unknown = true; state.roles[role].mode = 'UNKNOWN';
      if (state.rawDevices[role]) state.status.devices[role] = { ...structuredClone(state.rawDevices[role]),
        connected: false, state: 'UNKNOWN', status_error: 'Status reply lost; connection identity and safety controls retained. Readings are historical only' };
    }
    throw error;
  }
}

async function restoreWorker() {
  let client = null;
  try {
    client = tauriClient();
    const started = performance.now();
    const existing = await probeExistingWorker(client);
    if (!existing) return;
    state.client = client;
    const receivedAtMs = performance.now();
    acceptStatus(existing, { roundTripMs: receivedAtMs - started, receivedAtMs }, true);
    state.desiredMode = existing.mode;
    try {
      const identity = await request('ping');
      state.workerIdentity = typeof identity?.python_executable === 'string' &&
        typeof identity?.project_root === 'string' ? identity : null;
    } catch (error) {
      // A failed read must not discard the live worker or its shutdown control.
      state.workerIdentity = null;
      showNotice(`Worker restored, but runtime path read failed: ${error?.message || error}`, true);
    }
    try {
      const settings = await request('settings_get');
      // A live worker’s interpreter wins over an older saved startup choice.
      state.pythonPath = state.workerIdentity?.python_executable || settings.python_path;
      state.bindings = settings.bindings;
    } catch (error) {
      showNotice(`Worker restored, but settings read failed: ${error?.message || error}`, true);
    }
  } catch (error) {
    if (client) {
      // An unexpected host error is not evidence that its worker exited.
      // Keep a handle so the operator can still request orderly shutdown.
      state.client = client;
      state.mode = 'unknown';
    }
    if (!/Open this console in the Tauri desktop window/.test(error?.message || '')) {
      showNotice(`Existing worker check failed: ${error?.message || error}`, true);
    }
  } finally {
    state.bootstrapping = false;
    render();
  }
}

async function busy(task, successMessage = null, options = {}) {
  const { role, intent, method } = options;
  const control = state.roles[role];
  let pending;
  if (role) {
    if (!control) throw new Error('Device control context unknown');
    if (intent) {
      if (!canSendSafety(control)) throw new Error('No verifiable device connection identity');
      if (control.safetyPending && intentRank(intent) <= intentRank(control.safetyPending.intent)) return;
      revokeRole(role);
      pending = { intent, sentEpoch: control.context.epoch };
      control.safetyPending = pending;
      control.mode = intent === 'disconnect' ? 'CLOSING' : 'STOPPING';
      control.progress = 'Pending';
    } else {
      const connecting = method === 'connect' && control.context.connection_id === null &&
        !control.normalPending && !control.hostRestricted && !control.unknown && !state.closing;
      const resuming = method === 'resume' && !state.closing && canResume(control);
      if (!connecting && !resuming && !canSendNormal(control)) throw new Error('Controls are restricted; check the connection context and operation status');
      pending = { method, name: options.name };
      control.normalPending = pending;
    }
    if (role === 'gain') invalidatePendingGain(state.status?.devices?.gain, control);
  } else {
    if (state.busy) return;
    state.busy = true;
  }
  render();
  try {
    await task();
    if (successMessage) showNotice(successMessage);
  } catch (error) {
    showNotice(error?.message || String(error), true);
    if (state.client) {
      try { await refreshStatus(); } catch { /* keep the original failure visible */ }
    }
  } finally {
    if (role) {
      const active = state.roles[role];
      const key = intent ? 'safetyPending' : 'normalPending';
      if (active?.[key] === pending) active[key] = null;
      if (active) active.mode = state.closing ? 'CLOSING' : active.hostRestricted || active.unknown ? 'UNKNOWN'
        : active.safetyPending ? 'STOPPING' : active.stopHeld ? 'STOP_HELD' : active.backendMode || 'UNKNOWN';
    } else state.busy = false;
    render();
  }
}

async function startWorker() {
  if (state.bootstrapping) return;
  const input = document.getElementById('python-path');
  state.pythonPath = input?.value?.trim() || '';
  state.desiredMode = 'real';
  if (state.desiredMode === 'real' &&
      !confirmAction('Start a real-hardware worker. This step does not open instrument ports.')) return;
  await busy(async () => {
    const client = tauriClient();
    const identity = await client.start({ mode: state.desiredMode, pythonPath: state.pythonPath });
    state.client = client;
    state.sessionContext = null;
    state.roleContexts = {};
    state.closing = false;
    acceptContexts(identity);
    state.workerIdentity = identity;
    state.mode = state.desiredMode;
    const settings = await request('settings_get');
    // Keep the interpreter just launched; the saved path may be from an older session.
    // The operator can persist this selection with the separate Save action.
    state.bindings = settings.bindings;
    await refreshStatus();
  }, 'Worker started; no instruments connected.');
}

async function stopWorker() {
  if (state.shutdownPending) return state.shutdownPending;
  if (!confirmAction('Stop worker: attempt voltage zero, disable Gain current before TEC, and hold fiber-stage voltages.')) return;
  try {
    const report = await shutdownWorker();
    const warning = cleanupWarning(report);
    if (warning) {
      state.page = 'overview';
      throw new Error(warning);
    }
    showNotice('Worker stopped. Cleanup reports do not replace physical output measurements.');
  } catch (error) { showNotice(error?.message || String(error), true); }
  render();
}

function shutdownWorker() {
  if (state.shutdownPending) return state.shutdownPending;
  if (!state.client) return Promise.resolve({ steps: [], unreleased: [], voltage_zero: null });
  state.closing = true;
  state.page = 'overview';
  for (const role of roles) if (state.roles[role]) {
    revokeRole(role); state.roles[role].mode = 'CLOSING'; state.roles[role].progress = 'Shutdown pending';
    state.roles[role].shutdownEpoch = state.roles[role].context.epoch;
  }
  render();
  state.shutdownPending = Promise.resolve().then(() => state.client.stop()).then(report => {
    state.shutdownHistory.push(structuredClone({ report }));
    recordShutdown(report);
    return report;
  }).catch(cause => {
    const error = shutdownError(cause);
    state.shutdownHistory.push(structuredClone({ error: error.message,
      evidence: error.shutdownEvidence || null }));
    for (const role of roles) if (state.roles[role]) state.roles[role].progress = 'Shutdown result unknown; waiting for an explicit retry';
    throw error;
  }).finally(() => { state.shutdownPending = null; render(); });
  return state.shutdownPending;
}

function recordShutdown(report) {
  state.client = null;
  state.mode = null;
  state.workerIdentity = null;
  state.status = { devices: {}, last_cleanup: report };
  state.rawDevices = {};
  state.roles = {};
  state.roleContexts = {};
  state.sessionContext = null;
  state.trace = null;
  state.cursor = null;
  state.pmMeasurement = null;
  state.pmReadings = {};
  state.pmHistory = [];
  state.pmCommandResults = {};
  state.voltageHistory = [];
  state.gainHistory = [];
}

async function registerNativeClose(restoration) {
  const handle = globalThis.__TAURI__?.window?.getCurrentWindow?.();
  if (!handle) return; // The simulation-only browser preview has no native window.
  try {
    await installCloseGuard(handle, {
      active: () => Boolean(state.client) || state.bootstrapping,
      confirm: async () => {
        await restoration;
        return state.client ? confirmAction('Clean up all instruments before closing: attempt voltage zero, disable Gain current before TEC, and hold fiber-stage voltages.') : true;
      },
      stop: async () => {
        await restoration;
        if (!state.client) return { steps: [], unreleased: [], voltage_zero: null };
        return shutdownWorker();
      },
      report: (report, warning) => {
        if (warning) {
          state.page = 'overview';
          showNotice(warning, true);
        }
        state.busy = false;
        render();
      },
      error: (error) => {
        state.busy = false;
        showNotice(`Window remains open; instrument cleanup result unknown: ${error?.message || error}`, true);
        render();
      },
    });
  } catch (error) {
    showNotice(`Unable to enable window close protection: ${error?.message || error}; Use Stop worker before closing the window.`, true);
  }
}

async function enumerateDevices() {
  if (!confirmAction('Scan serial and VISA resources. This only lists resources and does not connect instruments.')) return;
  await busy(async () => { state.inventory = await request('inventory'); }, 'Resource scan completed.');
}

async function saveSettings() {
  const selected = Object.fromEntries(
    [...document.querySelectorAll('[data-binding]')]
      .map((element) => [element.dataset.binding, element.value || null]),
  );
  state.bindings = { ...state.bindings, ...selected };
  await busy(async () => {
    const saved = await request('settings_save', {
      settings: { version: 1, python_path: state.pythonPath, bindings: state.bindings },
    });
    state.bindings = saved.bindings;
  }, 'Resource bindings saved; no instruments connected.');
}

function connectionMessage(role, resource) {
  if (role === 'voltage') return `Connect CH340 voltage source ${resource}. This writes 0 V to all 8 channels and waits for telemetry confirmation.`;
  if (role === 'gain') return `Connect CP210x Gain Driver ${resource}. If current is on while TEC is off, the driver performs a safety shutdown.`;
  if (role === 'fiber') return 'Connect left and right MDT693B controllers by fixed USB serial number. Connection reads state without changing axis voltages. A single detected side is allowed.';
  if (role === 'osa') return `Connect OSA ${resource} without changing front-panel sweep settings. Later shutdown or disconnect may abort an active sweep.`;
  return `Connect PM400 ${resource} without changing front-panel settings.`;
}

async function connect(role) {
  const resource = role === 'fiber' ? null : state.bindings[role];
  const params = connectRequest(role, resource);
  if (!confirmAction(connectionMessage(role, resource))) return;
  await busy(async () => {
    if (role === 'voltage') state.voltageHistory = [];
    if (role === 'gain') state.gainHistory = [];
    await request('connect', params);
    if (role === 'osa') {
      state.trace = null;
      state.cursor = null;
    }
    if (role === 'pm400') {
      state.pmMeasurement = null;
      state.pmReadings = {};
      state.pmHistory = [];
      state.pmCommandResults = {};
    }
    await refreshStatus();
  }, `${panels.roleLabels[role]} connection request completed.`, { role, method: 'connect' });
}

async function disconnect(role) {
  if (state.roles[role]?.safetyPending?.intent === 'disconnect') return;
  const message = role === 'fiber'
    ? 'Disconnect fiber stages while holding existing piezo voltages.'
    : role === 'voltage' ? 'Disconnect the voltage source: immediately send 0 V to all 8 channels and wait for available telemetry evidence.'
    : role === 'gain' ? 'Disconnect Gain Driver: disable current before TEC.'
    : role === 'osa' ? 'Disconnect OSA. This may abort an active sweep; front-panel measurement settings are preserved.'
    : `Disconnect ${panels.roleLabels[role]}.`;
  if (!confirmAction(message)) return;
  await busy(async () => {
    await request('disconnect', { role });
    if (role === 'osa') {
      state.trace = null;
      state.cursor = null;
    }
    await refreshStatus();
    if (role === 'pm400') {
      state.pmMeasurement = null;
      state.pmReadings = {};
      state.pmHistory = [];
      state.pmCommandResults = {};
    }
    if (role === 'voltage') state.voltageHistory = [];
    if (role === 'gain') state.gainHistory = [];
  }, `${panels.roleLabels[role]} disconnect request completed.`, { role, intent: 'disconnect' });
}

async function deviceAction(params, message, successMessage) {
  const intent = safetyIntent('action', params);
  const control = state.roles[params.role];
  if (intent && control?.safetyPending && intentRank(intent) <= intentRank(control.safetyPending.intent)) return;
  if (!intent && !canSendNormal(control)) throw new Error('Controls are restricted for this instrument');
  if (params.name === 'enable_current' && !canEnableCurrent(control,
      state.status?.devices?.gain)) throw new Error('Enabling current requires fresh readbacks of all five fields, TEC On, and driver verification of the 5 s stability window');
  if (params.name === 'move' && !state.fiberAdopted[params.side]) throw new Error('Explicitly re-adopt this side’s baseline for the current page');
  const confirmations = Array.isArray(message) ? message : [message];
  for (const item of confirmations) {
    if (!confirmAction(item)) return;
  }
  await busy(async () => {
    const outcome = await request('action', params);
    if (params.role === 'fiber' && params.name === 'adopt_baseline') state.fiberAdopted[params.side] = true;
    if (params.role === 'osa' && params.name === 'acquire') {
      state.trace = outcome.result;
      state.cursor = null;
    }
    if (params.role === 'pm400' && typeof outcome.result?.value === 'number') {
      state.pmMeasurement = outcome.result;
      state.pmHistory = recordPmSample(state.pmHistory, outcome.result, params.kind || state.pmKind);
    }
    await refreshStatus();
  }, successMessage, { role: params.role, intent, method: 'action', name: params.name });
}

function stageInputs(side) {
  const displacement = {};
  for (const axis of ['x', 'y', 'z']) {
    const raw = document.getElementById(`${side}-${axis}`)?.value;
    displacement[axis] = raw?.trim() ? Number(raw) : Number.NaN;
  }
  return displacement;
}

function updateStagePreview(side) {
  const preview = previewStageMove(side, stageInputs(side));
  const box = document.getElementById(`preview-${side}`);
  if (box) {
    box.textContent = preview.allowed ? `${preview.reason} · ${preview.label}` : preview.reason;
    box.classList.toggle('invalid', !preview.allowed);
  }
}

function currentStageDrafts() {
  return state.page === 'fiber' ? { left: stageInputs('left'), right: stageInputs('right') } : {};
}

function exportTrace() {
  const trace = state.trace;
  if (!trace?.wavelength_nm?.length) throw new Error('No spectrum available to export');
  const rows = ['wavelength_nm,power_dbm'];
  for (let index = 0; index < trace.wavelength_nm.length; index += 1) {
    rows.push(`${trace.wavelength_nm[index]},${trace.power_dbm[index]}`);
  }
  const blob = new Blob([rows.join('\r\n') + '\r\n'], { type: 'text/csv;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = `osa-trace-${trace.trace || 'A'}-${new Date().toISOString().replaceAll(':', '-')}.csv`;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  showNotice('Spectrum CSV export requested; choose the save location.');
}

function pmSetting(key) {
  const entry = state.status?.devices?.pm400?.catalog?.settings?.find((item) => item.key === key);
  if (!entry) throw new Error('PM400 setting is not in the connected sensor catalog');
  return entry;
}

function pmSettingOptions(setting) {
  const key = setting.key;
  return {
    raw: document.getElementById(`pm-value-${key}`)?.value,
    selector: document.getElementById(`pm-selector-${key}`)?.value || null,
    group: document.getElementById(`pm-group-${key}`)?.value || null,
  };
}

async function pmSettingOperation(operation, key) {
  const setting = pmSetting(key);
  const options = pmSettingOptions(setting);
  if (operation === 'write') {
    const valueLabel = options.selector ? options.selector.toUpperCase() : options.raw;
    const warning = setting.sensitive
      ? 'This sensitive setting changes sensor response, calibration or adapter type.'
      : 'This changes the current PM400 setting.';
    if (!confirmAction(`${warning}\n${setting.label} → ${valueLabel}; Verify the sensor model and current connection.`)) return;
    options.confirmed = true;
  }
  const payload = buildPmSettingAction(setting, operation, options);
  await busy(async () => {
    const outcome = await request('action', payload);
    state.pmReadings[key] = outcome.result;
    await refreshStatus();
  }, operation === 'read' ? `${setting.label} read completed.` : `${setting.label} write and readback completed.`, { role: 'pm400', method: 'action' });
}

async function pmCommand(key) {
  const command = state.status?.devices?.pm400?.catalog?.commands?.find((item) => item.key === key);
  if (!command || !command.supported) throw new Error('PM400 command is not supported');
  const warning = command.confirm
    ? `Sensitive PM400 operation: ${command.label}. This operation may change front-panel settings or sensor calibration.`
    : `Run PM400 operation: ${command.label}. `;
  if (!confirmAction(warning)) return;
  const payload = { role: 'pm400', name: 'command', command: key };
  if (command.confirm) payload.confirm = true;
  await busy(async () => {
    const outcome = await request('action', payload);
    state.pmCommandResults[key] = outcome.result;
    if (typeof outcome.result?.value === 'number') {
      state.pmMeasurement = outcome.result;
      state.pmHistory = recordPmSample(state.pmHistory, outcome.result, state.pmKind);
    }
    await refreshStatus();
  }, `${command.label} request completed; verify instrument state.`, { role: 'pm400', method: 'action' });
}

async function runOperation(button) {
  const operation = button.dataset.op;
  const role = button.dataset.role;
  if (operation === 'confirm-context') {
    const control = state.roles[role];
    if (!control || control.mode !== 'READY' || control.hostRestricted || control.unknown || control.recoveryPending || state.closing) return;
    if (!confirmAction(`Confirm the current ${panels.roleLabels[role]} connection identity: ${control.context.connection_id}. This restores controls for this page only; fiber motion still requires baseline re-adoption.`)) return;
    control.confirmed = true;
    render();
    return;
  }
  if (operation === 'resume') {
    if (!confirmAction('Resume controls by releasing the scheduling hold. Outputs remain unchanged and previous inputs are not replayed.')) return;
    return busy(async () => {
      await request('resume', { role, confirm: true });
      const cutoff = state.roles[role].recoveryStatusAfter;
      const sequence = await refreshStatus();
      // At most one successor per successful resume; competing callers share it.
      if (sequence <= cutoff) await refreshStatus();
    }, 'Resume request completed. Controls still depend on current status evidence; previous commands were not replayed.', { role, method: 'resume' });
  }
  if (operation === 'start') return startWorker();
  if (operation === 'stop') return stopWorker();
  if (operation === 'refresh') return enumerateDevices();
  if (operation === 'save-settings') return saveSettings();
  if (operation === 'connect') return connect(role);
  if (operation === 'disconnect') return disconnect(role);
  if (operation === 'osa-export') return exportTrace();
  if (operation === 'osa-acquire') {
    const trace = document.getElementById('osa-trace')?.value || 'A';
    return deviceAction({ role: 'osa', name: 'acquire', trace },
      `Trigger an OSA Trace ${trace} sweep using the current front-panel settings.`, 'Spectrum acquisition completed.');
  }
  if (operation === 'voltage-apply') {
    const channel = Number(button.dataset.channel);
    const voltage = numericSetting(document.getElementById(`voltage-${channel}`)?.value, 0, 14, 'Voltage');
    return deviceAction({ role: 'voltage', name: 'set_channel', channel, voltage },
      `Ramp CH${channel} to ${voltage.toFixed(3)} V. The driver limits each step to ≤0.1 V with intervals ≥50 ms.`,
      `CH${channel} voltage command completed; verify telemetry.`);
  }
  if (operation === 'voltage-zero') return deviceAction({ role: 'voltage', name: 'zero' },
    'Immediately send 0 V to all 8 voltage-source channels. Command success does not prove physical zero.', 'Zero command sent; verify telemetry evidence.');
  if (operation === 'gain-set-temp') {
    const temperature_c = numericSetting(document.getElementById('gain-temp')?.value, 15, 40, 'Temperature');
    return deviceAction({ role: 'gain', name: 'set_temperature', temperature_c },
      `Set Gain TEC target temperature to ${temperature_c.toFixed(2)} °C.`, 'Target temperature set.');
  }
  if (operation === 'gain-set-current') {
    const current_ma = numericSetting(document.getElementById('gain-current')?.value, 0, 200, 'Current');
    return deviceAction({ role: 'gain', name: 'set_current', current_ma },
      `Set Gain current to ${current_ma.toFixed(2)} mA. If output is enabled, this may change actual current.`,
      'Current setpoint request completed.');
  }
  if (['gain-enable-tec', 'gain-disable-tec'].includes(operation)) {
    const disable = operation === 'gain-disable-tec';
    return deviceAction({ role: 'gain', name: disable ? 'disable_tec' : 'enable_tec' },
      disable ? 'Disable TEC. Current must be disabled first; the driver enforces this interlock.' : 'Enable Gain TEC temperature control.',
      'TEC operation completed; verify state.');
  }
  if (operation === 'gain-stable') return deviceAction({ role: 'gain', name: 'wait_stable', timeout_s: 60 },
    'Wait until measured temperature stays within target ±0.2 °C for at least 5 s. A timeout does not enable current.',
    'The driver reports stable temperature. Enabling current requires a separate confirmation.');
  if (['gain-enable-current', 'gain-disable-current'].includes(operation)) {
    const disable = operation === 'gain-disable-current';
    return deviceAction({ role: 'gain', name: disable ? 'disable_current' : 'enable_current' },
      disable ? 'Disable Gain current output.' : 'Enable Gain current output. The driver rechecks TEC and the stability window.',
      'Current output operation completed; verify state.');
  }
  if (operation === 'pm-measure') {
    const kind = document.getElementById('pm-kind')?.value || state.pmKind;
    const item = state.status?.devices?.pm400?.catalog?.measurements?.find((entry) => entry.key === kind);
    if (!item?.supported) throw new Error('The connected PM400 sensor does not support this measurement type');
    state.pmKind = kind;
    return deviceAction({ role: 'pm400', name: 'measure', kind },
      `Read ${item.label} once from the connected sensor, preserving current calibration settings.`, 'PM400 measurement completed.');
  }
  if (operation === 'pm-read' || operation === 'pm-write') {
    return pmSettingOperation(operation === 'pm-read' ? 'read' : 'write', button.dataset.setting);
  }
  if (operation === 'pm-command') return pmCommand(button.dataset.command);
  if (operation === 'fiber-adopt') {
    const side = button.dataset.side;
    return deviceAction({ role: 'fiber', name: 'adopt_baseline', side, confirm: true, allow_nominal: true },
      baselineConfirmations(side),
      'Baseline adopted; displacement remains an open-loop estimate.');
  }
  if (operation === 'fiber-move') {
    const side = button.dataset.side;
    const displacement = stageInputs(side);
    const params = fiberMoveRequest(side, displacement);
    const preview = previewStageMove(side, displacement);
    return deviceAction(params,
      `Move the ${side === 'left' ? 'left' : 'right'} fiber stage: ${preview.label}. ${preview.reason}. Position is a session-only open-loop estimate, not measured displacement.`,
      'Motion command completed; verify MDT voltages and the physical optical path.');
  }
  throw new Error(`Unsupported interface action: ${operation}`);
}

document.addEventListener('click', (event) => {
  const osaPlot = event.target.closest('[data-osa-plot]');
  if (osaPlot && state.trace) {
    const bounds = osaPlot.getBoundingClientRect();
    const plotX = (event.clientX - bounds.left) / bounds.width * 800;
    state.cursor = osaCursorIndex(state.trace, (plotX - 48) / 718);
    render();
    return;
  }
  const pmTab = event.target.closest('[data-pm-tab]');
  if (pmTab) {
    state.pmTab = pmTab.dataset.pmTab;
    render();
    return;
  }
  const pageButton = event.target.closest('[data-page]');
  if (pageButton) {
    state.page = pageButton.dataset.page;
    render();
    return;
  }
  const operationButton = event.target.closest('[data-op]');
  if (!operationButton || operationButton.disabled) return;
  Promise.resolve().then(() => runOperation(operationButton)).catch((error) => {
    showNotice(error?.message || String(error), true);
  });
});

document.addEventListener('change', (event) => {
  if (event.target.matches('[data-binding]')) {
    state.bindings[event.target.dataset.binding] = event.target.value || null;
    render(); // A formerly reserved serial binding may now be safe to select.
  }
  if (event.target.id === 'python-path') state.pythonPath = event.target.value;
  if (event.target.id === 'pm-kind') state.pmKind = event.target.value;
});

document.addEventListener('input', (event) => {
  const side = event.target.dataset.stageAxis;
  if (!side) return;
  updateStagePreview(side);
});

setInterval(async () => {
  document.getElementById('clock').textContent = new Date().toLocaleTimeString('en-US', { hour12: false });
  if (!state.client) return;
  const context = formContextKey();
  const drafts = context ? captureFormDrafts() : [];
  try {
    await refreshStatus();
    // Only the settings resource menu may retain its native popup. Instrument
    // controls must redraw to revoke permissions and age labels while typing.
    if (state.page === 'settings' && context && context === formContextKey() && isEditingForm()) return;
    const latestDrafts = isEditingForm() ? captureFormDrafts() : drafts;
    render();
    if (context && context === formContextKey()) restoreFormDrafts(latestDrafts);
  } catch (error) {
    showNotice(`Status refresh failed: ${error?.message || error}`, true);
    render();
  }
}, 2500);

document.getElementById('clock').textContent = new Date().toLocaleTimeString('en-US', { hour12: false });
render();
const restoration = restoreWorker();
registerNativeClose(restoration);
