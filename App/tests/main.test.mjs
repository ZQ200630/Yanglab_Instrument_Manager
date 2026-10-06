const sessionContext = { session_id: 'ui-test-session', connection_id: null, epoch: 0 };
const roleContexts = Object.fromEntries(['osa', 'voltage', 'gain', 'pm400', 'fiber'].map(role => [role, { ...sessionContext }]));
let fixtureRevision = 0;
function v2Reply(request, result) {
  const revision = ++fixtureRevision;
  const role = request.params.role;
  if (request.method === 'connect') roleContexts[role] = { ...sessionContext, connection_id: `connection-${revision}`, epoch: revision };
  if (request.method === 'disconnect') roleContexts[role] = { ...sessionContext, epoch: revision };
  const query = ['ping', 'status'].includes(request.method);
  if (query) for (const name of Object.keys(roleContexts)) {
    if (result.devices?.[name] && roleContexts[name].connection_id === null) {
      roleContexts[name] = { ...sessionContext, connection_id: `connection-${revision}-${name}`, epoch: revision };
    } else if (!result.devices?.[name] && roleContexts[name].connection_id !== null) {
      roleContexts[name] = { ...sessionContext, epoch: revision };
    }
    roleContexts[name] = { ...roleContexts[name], revision,
      state: result.devices?.[name] ? 'READY' : 'DISCONNECTED', safety: { state: 'RELEASED' } };
  }
  const context = role ? roleContexts[role] : sessionContext;
  return { v: 2, id: request.id, ok: true, phase: 'completed', context: {
    session_id: context.session_id, connection_id: context.connection_id, epoch: context.epoch },
    result: query ? { ...structuredClone(result), session_id: sessionContext.session_id,
      revision, roles: structuredClone(roleContexts) } : result };
}
import assert from 'node:assert/strict';
import test from 'node:test';

// Same dynamic-import/fake-DOM boundary as the existing production-main tests.
async function withSafetyConsole(run) {
  const previous = Object.fromEntries(['document', 'window', '__TAURI__', 'setInterval',
    'setTimeout', 'clearTimeout'].map(key => [key, globalThis[key]]));
  const elements = new Map(), listeners = new Map(), requests = [], held = new Map();
  let poll, stopHandler;
  let activeQuery = false;
  const roles = Object.fromEntries(['osa', 'voltage', 'gain', 'pm400', 'fiber'].map(role => [role,
    { session_id: 'safety-session', connection_id: `${role}-1`, epoch: 0,
      revision: 1, state: 'READY', active_request_id: null, safety: { state: 'RELEASED' } }]));
  const devices = Object.fromEntries(Object.keys(roles).map(role => [role,
    { connected: true, resource: `SIM-${role}`, state: 'READY', identity: role }]));
  devices.gain.fields = Object.fromEntries(Object.entries({ temperature_c: 24, target_c: 24,
    current_ma: 10, tec_enabled: true, current_enabled: false }).map(([key, value]) =>
    [key, { value, quality: 'fresh', observed_age_s: 0, connection_id: 'gain-1', revision: 1 }]));
  devices.fiber.left = { side: 'left', serial: '2110148249-10', resource: 'SIM-left',
    available: true, baseline_known: true, nominal_authorized: true,
    estimated_position_um: { x: 0, y: 0, z: 0 }, observed_voltage_v: { x: 1, y: 2, z: 3 } };
  const snapshot = { session_id: 'safety-session', revision: 1, roles, devices,
    mode: 'real', closing: false, host_transport: { roles: {}, failure: null, closing: false } };
  const element = id => {
    if (!elements.has(id)) elements.set(id, { innerHTML: '', textContent: '', hidden: false,
      value: id === 'gain-current' ? '10' : '', classList: { toggle() {} },
      contains() { return false; }, querySelectorAll() { return []; } });
    return elements.get(id);
  };
  const content = element('content');
  let html = '', gainTarget;
  Object.defineProperty(content, 'innerHTML', { get: () => html, set(value) {
    html = value;
    const match = value.match(/id="gain-temp"[^>]*value="([^"]*)"/);
    gainTarget = match ? { id: 'gain-temp', value: match[1], disabled: false,
      closest: selector => selector === '#content' ? content : null,
      matches: selector => selector === 'input,select' } : null;
    if (gainTarget) elements.set('gain-temp', gainTarget); else elements.delete('gain-temp');
  } });
  content.querySelectorAll = selector => selector.includes('input') && gainTarget ? [gainTarget] : [];
  const click = dataset => listeners.get('click')({ target: { closest(selector) {
    if (selector === '[data-page]' && dataset.page) return { dataset };
    if (selector === '[data-op]' && dataset.op) return { dataset, disabled: false };
    return null;
  } } });
  const settle = async () => { for (let i = 0; i < 5; i++) await new Promise(setImmediate); };
  const reply = (request, result, context = request.context) => ({ v: 2, id: request.id,
    ok: true, phase: 'completed', context, result });
  try {
    globalThis.document = { getElementById: element, activeElement: {},
      querySelectorAll: () => [], addEventListener: (name, handler) => listeners.set(name, handler) };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = callback => { poll = callback; return 1; };
    globalThis.setTimeout = () => 1;
    globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { invoke: async (command, args) => {
      if (command === 'worker_stop') return stopHandler();
      assert.equal(command, 'worker_request');
      const request = structuredClone(args.request);
      requests.push(request);
      if (request.method === 'status' && activeQuery) throw 'existing attempt: held-status';
      const key = request.method === 'action' ? request.params.name : request.method;
      if (held.has(key)) return new Promise((resolve, reject) => {
        if (key === 'status') activeQuery = true;
        const complete = (result, context = request.context) => { activeQuery = false; resolve(reply(request, result, context)); };
        complete.reject = error => { activeQuery = false; reject(error); };
        complete.fail = (phase = 'failed_after_call_started') => resolve({ v: 2, id: request.id, context: request.context, ok: false, phase,
          error: { type: 'InjectedFailure', message: 'safety call failed' } });
        held.set(key, complete);
      });
      if (request.method === 'settings_get') return reply(request, { bindings: { osa: 'SIM-osa' }, python_path: 'VISA' });
      if (['status', 'ping'].includes(request.method)) return reply(request, structuredClone(snapshot),
        { session_id: snapshot.session_id, connection_id: null, epoch: 0 });
      if (request.method === 'resume') { roles[request.params.role].state = 'READY'; roles[request.params.role].revision++;
        snapshot.host_transport.roles[request.params.role] = false; return reply(request, { resumed: true }); }
      if (request.method === 'action') roles[request.params.role].revision++;
      return reply(request, { result: false });
    } } };
    await import(`./legacy-main.js?safety=${Math.random()}`);
    await settle();
    await run({ click, settle, requests, held, snapshot, element, poll: () => poll(),
      stop: handler => { stopHandler = handler; } });
  } finally {
    for (const [key, value] of Object.entries(previous)) globalThis[key] = value;
  }
}

test('periodic and action refresh share the host single status slot without revoking authority', async () => {
  await withSafetyConsole(async ({ click, settle, snapshot, poll, requests, held, element }) => {
    click({ op: 'confirm-context', role: 'gain' }); await settle();
    held.set('status', null);
    const polling = poll(); await settle();
    const finish = held.get('status');
    click({ op: 'gain-set-current' }); await settle();
    assert.equal(requests.filter(r => r.method === 'status').length, 2, 'bootstrap plus one coalesced in-flight status');
    held.delete('status'); finish(structuredClone(snapshot)); await polling; await settle();
    snapshot.roles.gain.revision++; await poll();
    click({ page: 'gain' });
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" >/);
  });
});

test('failed safety retry exposes explicit recovery while ordinary status keeps unknown latched', async () => {
  await withSafetyConsole(async ({ click, settle, snapshot, poll, requests, held, element }) => {
    held.set('disable_current', null);
    click({ op: 'gain-disable-current' }); await settle();
    held.get('disable_current').fail(); held.delete('disable_current'); await settle();
    click({ page: 'gain' });
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
    snapshot.roles.gain = { ...snapshot.roles.gain, state: 'STOP_HELD', epoch: 2, revision: 5,
      safety: { state: 'STOP_HELD', attempt_id: 'verified-retry' } };
    snapshot.host_transport.roles.gain = true;
    click({ op: 'gain-disable-current' }); await settle(); await poll();
    assert.match(element('content').innerHTML, /Resume controls/);
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
    click({ op: 'resume', role: 'gain' }); await settle();
    assert.equal(requests.filter(r => r.method === 'resume').length, 1);
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" >/);
    assert.equal(requests.filter(r => r.params.name === 'enable_current').length, 0);
  });
});

test('old restricted status after successful resume cannot defeat later healthy status', async () => {
  await withSafetyConsole(async ({ click, settle, snapshot, poll, requests, held, element }) => {
    snapshot.roles.gain = { ...snapshot.roles.gain, state: 'STOP_HELD', epoch: 2, revision: 5,
      safety: { state: 'STOP_HELD', attempt_id: 'verified-stop' } };
    snapshot.host_transport.roles.gain = true;
    await poll(); click({ page: 'gain' });
    const old = structuredClone(snapshot);
    held.set('status', null); const pending = poll(); await settle();
    click({ op: 'resume', role: 'gain' }); await settle();
    const finish = held.get('status'); held.delete('status'); finish(old);
    await pending; await settle(); await poll();
    assert.equal(snapshot.roles.gain.state, 'READY');
    assert.equal(snapshot.host_transport.roles.gain, false);
    assert.equal(requests.filter(r => r.method === 'resume').length, 1);
    assert.equal(/data-op="gain-enable-current" >/.test(element('content').innerHTML), true,
      'backend READY and authorized resume must survive the older restricted status');
    assert.equal(requests.filter(r => r.method === 'action').length, 0);
  });
});

test('pre-resume status cannot re-latch Gain and recovery waits for one fresh coalesced query', async () => {
  await withSafetyConsole(async ({ click, settle, snapshot, poll, requests, held, element }) => {
    snapshot.roles.gain = { ...snapshot.roles.gain, state: 'STOP_HELD', epoch: 2, revision: 5,
      safety: { state: 'STOP_HELD', attempt_id: 'verified-stop' } };
    snapshot.host_transport.roles.gain = true;
    await poll(); click({ page: 'gain' });
    const old = structuredClone(snapshot);
    old.host_transport.roles.voltage = true;
    held.set('status', null); const pending = poll(); await settle();
    const oldReply = held.get('status');
    const before = requests.filter(r => r.method === 'status').length;
    click({ op: 'resume', role: 'gain' }); await settle();
    assert.equal(snapshot.roles.gain.state, 'READY');
    assert.equal(snapshot.roles.gain.revision, 6);
    assert.equal(requests.filter(r => r.method === 'resume').length, 1);
    oldReply(old); await pending; await settle();
    assert.equal(requests.filter(r => r.method === 'status').length, before + 1,
      'resume must follow the pre-recovery query with one new query');
    assert.match(element('content').innerHTML, /data-op="gain-set-current" disabled/,
      'old evidence cannot authorize ordinary actions before the new query');
    click({ op: 'confirm-context', role: 'gain' }); await settle();
    assert.match(element('content').innerHTML, /data-op="gain-set-current" disabled/);
    const polls = Array.from({ length: 20 }, () => poll()); await settle();
    assert.equal(requests.filter(r => r.method === 'status').length, before + 1);
    const freshReply = held.get('status'); held.delete('status'); freshReply(structuredClone(snapshot));
    await Promise.all(polls); await settle();
    click({ page: 'gain' });
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" >/);
    assert.doesNotMatch(element('content').innerHTML, /Host transport \/ operation outcome unknown/);
    click({ page: 'voltage' });
    assert.match(element('content').innerHTML, /data-op="voltage-apply" data-channel="1" disabled/,
      'unrelated role restriction from the older query must still apply');
    assert.equal(requests.filter(r => r.method === 'action').length, 0, 'recovery never replays output');
  });
});

for (const oldState of ['STOP_HELD', 'UNKNOWN', 'FAULT', 'old-connection']) {
  test(`pre-recovery ${oldState} evidence with a later revision cannot replace recovered role context`, async () => {
    await withSafetyConsole(async ({ click, settle, snapshot, poll, held, element }) => {
      snapshot.roles.gain = { ...snapshot.roles.gain, state: 'STOP_HELD', epoch: 2, revision: 5,
        safety: { state: 'STOP_HELD', attempt_id: 'verified-stop' } };
      snapshot.host_transport.roles.gain = true; await poll(); click({ page: 'gain' });
      const old = structuredClone(snapshot);
      old.roles.gain.revision = 7;
      old.roles.gain.state = oldState === 'old-connection' ? 'READY' : oldState;
      if (oldState === 'old-connection') old.roles.gain.connection_id = 'obsolete-gain';
      snapshot.roles.gain.revision = 8;
      held.set('status', null); const pending = poll(); await settle();
      click({ op: 'resume', role: 'gain' }); await settle();
      const finish = held.get('status'); held.delete('status'); finish(old);
      await pending; await settle(); await poll();
      assert.equal(/data-op="gain-enable-current" >/.test(element('content').innerHTML), true,
        'query start ordering must fence every role authority field, not only host restriction or revision');
    });
  });
}

for (const restriction of ['host-same-revision', 'host-missing-role-and-device', 'backend-fault', 'query-failure', 'new-safety']) {
  test(`post-recovery ${restriction} still revokes authority and healthy polling cannot clear it`, async () => {
    await withSafetyConsole(async ({ click, settle, snapshot, poll, held, element, requests }) => {
      snapshot.roles.gain = { ...snapshot.roles.gain, state: 'STOP_HELD', epoch: 2, revision: 5,
        safety: { state: 'STOP_HELD', attempt_id: 'verified-stop' } };
      snapshot.host_transport.roles.gain = true; await poll(); click({ page: 'gain' });
      held.set('status', null);
      click({ op: 'resume', role: 'gain' }); await settle();
      assert.equal(requests.filter(r => r.method === 'resume').length, 1);
      assert.match(element('content').innerHTML, /data-op="gain-set-current" disabled/);
      const frame = structuredClone(snapshot);
      if (restriction.startsWith('host-')) {
        frame.host_transport.roles.gain = true;
        frame.roles.gain.revision = 5; // The host's evidence is independent of device revision.
        if (restriction === 'host-missing-role-and-device') { delete frame.roles.gain; delete frame.devices.gain; }
      }
      if (restriction === 'backend-fault') frame.roles.gain.state = 'FAULT';
      if (restriction === 'new-safety') {
        held.set('disable_current', null); click({ op: 'gain-disable-current' }); await settle();
      }
      const finish = held.get('status'); held.delete('status');
      if (restriction === 'query-failure') finish.reject('fresh query delivery unknown'); else finish(frame);
      await settle();
      snapshot.roles.gain.revision++;
      await poll(); click({ page: 'gain' });
      assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
      assert.match(element('content').innerHTML, /data-op="gain-disable-current" >/);
      if (restriction === 'new-safety') {
        held.get('disable_current')({ result: false }); held.delete('disable_current'); await settle();
        snapshot.roles.gain.revision++; await poll();
        assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/,
          'a newer safety request cancels pending post-resume confirmation');
      }
    });
  });
}

for (const phase of ['rejected_before_call', 'failed_after_call_started']) {
  test(`${phase} resume cannot fence out pending restriction evidence`, async () => {
    await withSafetyConsole(async ({ click, settle, snapshot, poll, held, element }) => {
      snapshot.roles.gain = { ...snapshot.roles.gain, state: 'STOP_HELD', epoch: 2, revision: 5,
        safety: { state: 'STOP_HELD', attempt_id: 'verified-stop' } };
      await poll(); click({ page: 'gain' });
      const old = structuredClone(snapshot); old.host_transport.roles.gain = true;
      held.set('status', null); const pending = poll(); await settle();
      held.set('resume', null); click({ op: 'resume', role: 'gain' }); await settle();
      held.get('resume').fail(phase); held.delete('resume'); await settle();
      const finish = held.get('status'); held.delete('status'); finish(old); await pending; await settle();
      snapshot.roles.gain.state = 'READY'; snapshot.roles.gain.revision++; await poll();
      assert.equal(/Host transport \/ operation outcome unknown/.test(element('content').innerHTML), true,
        'only successful authorized recovery may advance the query cutoff');
      assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
    });
  });
}

test('pre-recovery global closing evidence still blocks every role', async () => {
  await withSafetyConsole(async ({ click, settle, snapshot, poll, held, element }) => {
    snapshot.roles.gain = { ...snapshot.roles.gain, state: 'STOP_HELD', epoch: 2, revision: 5,
      safety: { state: 'STOP_HELD', attempt_id: 'verified-stop' } };
    snapshot.host_transport.roles.gain = true; await poll(); click({ page: 'gain' });
    const old = structuredClone(snapshot); old.host_transport.closing = true;
    held.set('status', null); const pending = poll(); await settle();
    click({ op: 'resume', role: 'gain' }); await settle();
    const finish = held.get('status'); held.delete('status'); finish(old);
    await pending; await settle(); await poll();
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
    click({ page: 'voltage' });
    assert.match(element('content').innerHTML, /data-op="voltage-apply" data-channel="1" disabled/);
  });
});

test('timeout recovery waits for quiescence and a late resume cannot erase a newer safety restriction', async () => {
  await withSafetyConsole(async ({ click, settle, snapshot, poll, requests, held, element }) => {
    click({ op: 'confirm-context', role: 'gain' }); await settle();
    held.set('wait_stable', null); click({ op: 'gain-stable' }); await settle();
    held.get('wait_stable').reject('action timed out; attempt remains tracked'); held.delete('wait_stable'); await settle();
    click({ page: 'gain' });
    snapshot.roles.gain = { ...snapshot.roles.gain, state: 'STOP_HELD', epoch: 2, revision: 5,
      active_request_id: 'old-wait', safety: { state: 'STOP_HELD', attempt_id: 'verified-retry' } };
    snapshot.host_transport.roles.gain = true; await poll();
    click({ op: 'resume', role: 'gain' }); await settle();
    assert.equal(requests.filter(r => r.method === 'resume').length, 0);
    snapshot.roles.gain.active_request_id = null; snapshot.roles.gain.revision++; await poll();
    held.set('resume', null); click({ op: 'resume', role: 'gain' }); await settle();
    assert.equal(requests.filter(r => r.method === 'resume').length, 1);
    const oldResume = held.get('resume');
    click({ op: 'gain-disable-tec' }); await settle();
    oldResume({ resumed: true }); held.delete('resume'); await settle();
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
    snapshot.roles.gain.revision++; await poll();
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
    click({ op: 'resume', role: 'gain' }); await settle();
    snapshot.roles.gain.revision++; await poll();
    assert.equal(requests.filter(r => r.method === 'resume').length, 2);
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" >/);
    assert.equal(requests.filter(r => r.params.name === 'enable_current').length, 0);
  });
});

test('an outstanding OSA action permits other safety intents and cache polls but no second ordinary OSA call', async () => {
  await withSafetyConsole(async ({ click, settle, requests, held, poll }) => {
    click({ op: 'confirm-context', role: 'osa' }); await settle();
    held.set('acquire', null);
    click({ op: 'osa-acquire' }); await settle();
    assert.equal(requests.filter(r => r.params.name === 'acquire').length, 1);
    click({ op: 'osa-acquire' });
    click({ op: 'voltage-zero' });
    click({ op: 'gain-disable-current' });
    await settle();
    const before = requests.filter(r => r.method === 'status').length;
    await poll();
    assert.equal(requests.filter(r => r.params.name === 'acquire').length, 1);
    assert.ok(requests.some(r => r.params.name === 'zero'));
    assert.ok(requests.some(r => r.params.name === 'disable_current'));
    assert.equal(requests.filter(r => r.method === 'status').length, before + 1);
    held.get('acquire')({ result: { wavelength_nm: [1550], power_dbm: [-20] } }); await settle();
  });
});

test('reload is read-only for already armed Fiber and requires new baseline adoption', async () => {
  await withSafetyConsole(async ({ click, settle, requests, element, snapshot, poll }) => {
    click({ page: 'fiber' });
    assert.match(element('content').innerHTML, /data-op="fiber-move" data-side="left" disabled/);
    click({ op: 'confirm-context', role: 'fiber' }); await settle();
    assert.match(element('content').innerHTML, /data-op="fiber-move" data-side="left" disabled/);
    assert.ok(requests.every(r => ['status', 'ping', 'settings_get'].includes(r.method)));
    click({ op: 'fiber-adopt', side: 'left' }); await settle();
    assert.ok(requests.some(r => r.params.name === 'adopt_baseline' && r.params.confirm === true));
    assert.match(element('content').innerHTML, /data-op="fiber-move" data-side="left" >/);
    snapshot.roles.fiber.revision++;
    snapshot.devices.fiber.left.baseline_known = false;
    snapshot.devices.fiber.left.fault = 'manual change';
    await poll();
    assert.match(element('content').innerHTML, /data-op="fiber-move" data-side="left" disabled/);
    assert.match(element('content').innerHTML, /manual change/);
    snapshot.devices.fiber.left.baseline_known = true; snapshot.devices.fiber.left.fault = null;
    snapshot.roles.fiber.revision++;
    await poll();
    assert.match(element('content').innerHTML, /data-op="fiber-move" data-side="left" disabled/,
      'later cache cannot reinstate an invalidated page baseline');
  });
});

for (const restriction of ['host restriction', 'backend UNKNOWN']) {
  test(`adopted Fiber loses published pose and authority after ${restriction} while keeping observed volts`, async () => {
    await withSafetyConsole(async ({ click, settle, requests, element, snapshot, poll }) => {
      click({ op: 'confirm-context', role: 'fiber' }); await settle();
      click({ op: 'fiber-adopt', side: 'left' }); await settle();
      click({ page: 'fiber' });
      assert.match(element('content').innerHTML, /X 0\.000 µm/);
      assert.match(element('content').innerHTML, />ARMED</);
      const actionCount = requests.filter(request => request.method === 'action').length;
      snapshot.roles.fiber.revision++;
      snapshot.devices.fiber.left.observed_voltage_v = { x: 4, y: 5, z: 6 };
      if (restriction === 'host restriction') snapshot.host_transport.roles.fiber = true;
      else snapshot.roles.fiber.state = 'UNKNOWN';
      await poll();
      const html = element('content').innerHTML;
      assert.doesNotMatch(html, /X 0\.000 µm/);
      assert.doesNotMatch(html, />ARMED</);
      assert.match(html, /Session position estimate: <strong>Unknown<\/strong>/);
      assert.match(html, />READ ONLY</);
      assert.match(html, /data-op="fiber-move" data-side="left" disabled/);
      for (const voltage of ['4.000 V', '5.000 V', '6.000 V']) assert.ok(html.includes(voltage));
      assert.equal(requests.filter(request => request.method === 'action').length, actionCount,
        'loss of authority must not send movement or baseline writes');
    });
  });
}

test('safety duplicates attach while stronger intents still send and wait never enables current', async () => {
  await withSafetyConsole(async ({ click, settle, requests, held, element }) => {
    click({ op: 'confirm-context', role: 'gain' }); await settle();
    held.set('wait_stable', null); held.set('disable_current', null); held.set('disable_tec', null);
    click({ op: 'gain-stable' }); await settle();
    click({ op: 'gain-disable-current' }); await settle();
    click({ op: 'gain-disable-current' }); click({ op: 'gain-disable-tec' }); await settle();
    assert.equal(requests.filter(r => r.params.name === 'disable_current').length, 1);
    assert.equal(requests.filter(r => r.params.name === 'disable_tec').length, 1);
    click({ page: 'gain' });
    assert.match(element('content').innerHTML, /stability wait still active/);
    held.get('wait_stable')({ result: true });
    held.get('disable_current')({ result: false }); held.get('disable_tec')({ result: false });
    await settle();
    assert.ok(!requests.some(r => r.params.name === 'enable_current'));
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
  });
});

test('same-resource reconnect and reversed status cannot republish an old OSA trace or permission', async () => {
  await withSafetyConsole(async ({ click, settle, held, snapshot, element, poll, requests }) => {
    click({ op: 'confirm-context', role: 'osa' }); await settle();
    held.set('acquire', null); click({ op: 'osa-acquire' }); await settle();
    const old = structuredClone(snapshot);
    snapshot.roles.osa = { ...snapshot.roles.osa, connection_id: 'osa-2', epoch: 2, revision: 10 };
    snapshot.revision = 10;
    await poll();
    held.get('acquire')({ result: { wavelength_nm: [1599], power_dbm: [-1] } }); await settle();
    click({ page: 'osa' });
    assert.doesNotMatch(element('content').innerHTML, /1599/);
    assert.match(element('content').innerHTML, /data-op="osa-acquire" disabled/);
    Object.assign(snapshot, old); await poll();
    click({ op: 'confirm-context', role: 'osa' }); await settle();
    click({ op: 'disconnect', role: 'osa' }); await settle();
    assert.equal(requests.findLast(r => r.method === 'disconnect').context.connection_id, 'osa-2');
  });
});

test('host restrictions block an absent device and an old healthy status cannot unlock it', async () => {
  await withSafetyConsole(async ({ click, settle, snapshot, poll, requests, element }) => {
    snapshot.host_transport.roles.osa = true;
    delete snapshot.devices.osa;
    snapshot.roles.osa = { ...snapshot.roles.osa, connection_id: null, epoch: 1, revision: 2, state: 'DISCONNECTED' };
    await poll();
    snapshot.host_transport.roles.osa = false;
    await poll();
    click({ op: 'connect', role: 'osa' }); await settle();
    assert.equal(requests.filter(r => r.method === 'connect').length, 0);
    click({ page: 'settings' });
    assert.match(element('content').innerHTML, /data-op="connect" data-role="osa" disabled/);
  });
});

test('STOP_HELD needs explicit resume with current context and does not replay old input', async () => {
  await withSafetyConsole(async ({ click, settle, snapshot, poll, requests, element }) => {
    snapshot.roles.gain = { ...snapshot.roles.gain, state: 'STOP_HELD', epoch: 1, revision: 2,
      safety: { state: 'STOP_HELD', attempt_id: 'gain-stop-1' } };
    await poll(); click({ page: 'gain' });
    assert.match(element('content').innerHTML, /Resume controls/);
    snapshot.roles.gain.state = 'READY'; snapshot.roles.gain.revision = 3;
    await poll();
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
    click({ op: 'resume', role: 'gain' }); await settle();
    const resume = requests.find(r => r.method === 'resume');
    assert.deepEqual(resume.params, { role: 'gain', confirm: true });
    assert.equal(resume.context.epoch, 1);
    assert.equal(requests.filter(r => r.method === 'action').length, 0);
  });
});

test('failed global close retains each original attempt and requires a second explicit operation', async () => {
  await withSafetyConsole(async ({ click, settle, stop, poll, element, requests }) => {
    let stops = 0;
    const evidence = { active_attempt: null, attempts: [{ id: 'close-1', phase: 'failed_after_call_started',
      error: { type: 'CleanupFailure', message: 'original failure' } }],
      received_cleanup_report: null, process_exit: null };
    stop(() => {
      stops++;
      if (stops === 1) throw `cleanup failed; shutdown evidence: ${JSON.stringify(evidence)}\nworker stderr: old log`;
      return { attempt_id: 'close-2', steps: [], unreleased: [], voltage_zero: null,
        process_exit: { confirmed: true, success: true, code: 0 } };
    });
    click({ op: 'stop' }); await settle();
    click({ page: 'overview' });
    assert.match(element('content').innerHTML, /close-1/);
    assert.match(element('content').innerHTML, /original failure/);
    await poll();
    assert.equal(stops, 1);
    click({ op: 'gain-enable-current' }); await settle();
    assert.ok(!requests.some(r => r.params.name === 'enable_current'));
    click({ op: 'stop' }); await settle();
    assert.equal(stops, 2);
    assert.match(element('content').innerHTML, /close-1/);
    assert.match(element('content').innerHTML, /close-2/);
  });
});

test('focused Gain drafts cannot freeze evidence aging and same-resource reconnect clears the draft', async () => {
  await withSafetyConsole(async ({ click, settle, element, snapshot, poll }) => {
    click({ op: 'confirm-context', role: 'gain' }); await settle(); click({ page: 'gain' });
    element('gain-temp').value = '28'; globalThis.document.activeElement = element('gain-temp');
    snapshot.roles.gain.revision++;
    snapshot.devices.gain.fields.current_ma.observed_age_s = 8;
    await poll();
    assert.equal(element('gain-temp').value, '28');
    assert.match(element('content').innerHTML, /stale.*8\.0/);
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
    snapshot.roles.gain = { ...snapshot.roles.gain, connection_id: 'gain-2', epoch: 2, revision: 10 };
    await poll();
    assert.equal(element('gain-temp').value, '24');
    assert.match(element('content').innerHTML, /data-op="gain-set-temp" disabled/);
  });
});

test('Gain writes invalidate affected readback before the reply and late PM results cannot refill history', async () => {
  await withSafetyConsole(async ({ click, settle, held, element, snapshot, poll }) => {
    click({ op: 'confirm-context', role: 'gain' }); await settle();
    held.set('set_current', null); click({ op: 'gain-set-current' }); await settle();
    click({ page: 'gain' });
    assert.match(element('content').innerHTML, /data-gain-field="current_ma"[\s\S]*?unknown/);
    held.get('set_current')({ result: 10 }); await settle();
    snapshot.devices.pm400.catalog = { measurements: [{ key: 'power', label: 'Power', unit: 'W', supported: true }],
      settings: [], commands: [] }; snapshot.roles.pm400.revision++;
    await poll(); click({ op: 'confirm-context', role: 'pm400' }); await settle();
    held.set('measure', null); click({ op: 'pm-measure' }); await settle();
    snapshot.roles.pm400 = { ...snapshot.roles.pm400, connection_id: 'pm400-2', epoch: 2, revision: 10 };
    await poll();
    held.get('measure')({ result: { value: 123.456, kind: 'power', unit: 'W' } }); await settle();
    click({ page: 'pm400' });
    assert.doesNotMatch(element('content').innerHTML, /123\.456/);
    assert.match(element('content').innerHTML, /data-op="pm-measure" disabled/);
  });
});

test('new connection eligibility does not retain an old pending request or late transport failure', async () => {
  await withSafetyConsole(async ({ click, settle, held, snapshot, poll, requests }) => {
    click({ op: 'confirm-context', role: 'osa' }); await settle();
    held.set('acquire', null); click({ op: 'osa-acquire' }); await settle();
    const old = held.get('acquire'); held.delete('acquire');
    snapshot.roles.osa = { ...snapshot.roles.osa, connection_id: 'osa-2', epoch: 2, revision: 10 };
    await poll(); click({ op: 'confirm-context', role: 'osa' }); await settle();
    click({ op: 'osa-acquire' }); await settle();
    assert.equal(requests.filter(r => r.params.name === 'acquire').length, 2);
    old.reject('old connection transport failure'); await settle();
    click({ op: 'osa-acquire' }); await settle();
    assert.equal(requests.filter(r => r.params.name === 'acquire').length, 3);
  });
});

test('Fiber disconnect immediately hides the adopted pose while the safety reply is pending', async () => {
  await withSafetyConsole(async ({ click, settle, held, element }) => {
    click({ op: 'confirm-context', role: 'fiber' }); await settle();
    click({ op: 'fiber-adopt', side: 'left' }); await settle(); click({ page: 'fiber' });
    assert.match(element('content').innerHTML, /X 0\.000 µm/);
    held.set('disconnect', null); click({ op: 'disconnect', role: 'fiber' }); await settle();
    assert.doesNotMatch(element('content').innerHTML, /X 0\.000 µm/);
    assert.match(element('content').innerHTML, /data-op="fiber-move" data-side="left" disabled/);
    held.get('disconnect')({ connected: false }); await settle();
  });
});

test('unknown Gain write outcome cannot revive old fresh readback on an unchanged cache poll', async () => {
  await withSafetyConsole(async ({ click, settle, held, element, poll }) => {
    click({ op: 'confirm-context', role: 'gain' }); await settle();
    held.set('set_current', null); click({ op: 'gain-set-current' }); await settle();
    held.get('set_current').reject('write timed out'); await settle();
    await poll(); click({ page: 'gain' });
    const field = /data-gain-field="current_ma"([\s\S]*?)<\/div>/.exec(element('content').innerHTML)[1];
    assert.match(field, /unknown/);
    assert.doesNotMatch(field, /fresh/);
  });
});

test('coalesced status callers and a later stale snapshot cannot unlock a newer stop epoch', async () => {
  await withSafetyConsole(async ({ click, settle, held, snapshot, poll, element }) => {
    click({ op: 'confirm-context', role: 'gain' }); await settle();
    const old = structuredClone(snapshot);
    held.set('status', null); const first = poll(); await settle(); const firstReply = held.get('status');
    const second = poll(); await settle();
    const newer = structuredClone(snapshot);
    newer.revision = 8; newer.roles.gain = { ...newer.roles.gain, epoch: 2, revision: 8, state: 'STOP_HELD',
      safety: { state: 'STOP_HELD', attempt_id: 'verified-final' } };
    newer.devices.gain.fields.current_ma.value = 7;
    firstReply(newer); await second; await first;
    held.set('status', null); const stale = poll(); await settle(); held.get('status')(old); await stale;
    held.delete('status'); click({ page: 'gain' });
    assert.match(element('content').innerHTML, /7\.000 mA/);
    assert.match(element('content').innerHTML, /data-op="gain-enable-current" disabled/);
    assert.match(element('content').innerHTML, /Resume controls/);
  });
});

test('a lost status reply preserves identified Voltage and Gain safety entry points', async () => {
  await withSafetyConsole(async ({ click, settle, held, poll, element }) => {
    held.set('status', null); const pending = poll(); await settle();
    held.get('status').reject('status transport unavailable'); await pending;
    click({ page: 'voltage' });
    assert.match(element('content').innerHTML, /data-op="voltage-zero" >/);
    assert.match(element('content').innerHTML, /data-op="voltage-apply" data-channel="1" disabled/);
    click({ page: 'gain' });
    assert.match(element('content').innerHTML, /data-op="gain-disable-current" >/);
    assert.match(element('content').innerHTML, /data-op="gain-disable-tec" >/);
  });
});

test('a connect reply remains current when polling has already adopted its connecting instance', async () => {
  await withSafetyConsole(async ({ click, settle, held, snapshot, poll, element }) => {
    snapshot.roles.osa = { ...snapshot.roles.osa, connection_id: null, epoch: 1, revision: 2, state: 'DISCONNECTED' };
    delete snapshot.devices.osa; await poll();
    held.set('connect', null); click({ op: 'connect', role: 'osa' }); await settle();
    snapshot.roles.osa = { ...snapshot.roles.osa, connection_id: 'osa-2', revision: 3, state: 'CONNECTING' };
    await poll();
    snapshot.roles.osa.state = 'READY'; snapshot.roles.osa.revision = 4;
    snapshot.devices.osa = { connected: true, resource: 'SIM-osa', state: 'READY' };
    held.get('connect')({ connected: true }, { session_id: snapshot.session_id, connection_id: 'osa-2', epoch: 1 });
    await settle(); click({ page: 'osa' });
    assert.doesNotMatch(element('notice').textContent, /previous connection or generation/);
    assert.match(element('content').innerHTML, /data-op="osa-acquire" >/);
  });
});

test('a chosen VISA Python path remains selected and is saved after worker startup', async () => {
  const chosen = 'E:/Miniconda/envs/VISA/python.exe';
  const old = 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe';
  const elements = new Map();
  const listeners = new Map();
  const calls = [];
  const previous = {
    document: globalThis.document, window: globalThis.window,
    tauri: globalThis.__TAURI__, setInterval: globalThis.setInterval,
    setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout,
  };
  const element = (id) => {
    if (!elements.has(id)) elements.set(id, {
      innerHTML: '', textContent: '', className: '', hidden: false,
      value: id === 'python-path' ? chosen : id === 'worker-mode' ? 'real' : '',
      classList: { toggle() {} },
    });
    return elements.get(id);
  };
  const click = (dataset) => listeners.get('click')({
    target: { closest(selector) {
      if (selector === '[data-page]' && dataset.page) return { dataset };
      if (selector === '[data-op]' && dataset.op) return { dataset, disabled: false };
      return null;
    } },
  });

  try {
    globalThis.document = {
      getElementById: element,
      querySelectorAll: () => [],
      addEventListener: (name, handler) => listeners.set(name, handler),
    };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = () => 1;
    globalThis.setTimeout = () => 1;
    globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { invoke: async (command, args) => {
      calls.push({ command, args });
      if (command === 'worker_start') return { mode: 'real', connected: false,
        python_executable: chosen,
        project_root: 'C:/Program Files/SIL Instrument Console/resources',
        environment_name: 'VISA', protocol_version: 2, session_id: sessionContext.session_id, roles: roleContexts };
      if (command === 'worker_stop') return { steps: [], unreleased: [], voltage_zero: null };
      if (command !== 'worker_request') throw new Error(`unexpected ${command}`);
      const { method, id } = args.request;
      if (method === 'status' && !calls.some((call) => call.command === 'worker_start')) {
        throw new Error('instrument worker is not running');
      }
      const result = method === 'settings_get'
        ? { version: 1, python_path: old,
          bindings: { osa: 'GPIB0::4::INSTR', pm400: null, voltage: null, gain: null } }
        : method === 'settings_save' ? args.request.params.settings
          : { mode: 'real', devices: {}, observed_at: '2026-09-25T12:00:00Z', last_cleanup: null };
      return v2Reply(args.request, result);
    } } };
    await import('./legacy-main.js?test=chosen-python-path');
    await new Promise((resolve) => setImmediate(resolve));
    click({ page: 'settings' });
    click({ op: 'start' });
    for (let i = 0; i < 5; i += 1) await new Promise((resolve) => setImmediate(resolve));
    assert.equal(calls.find((call) => call.command === 'worker_start').args.config.pythonPath, chosen);
    assert.match(element('content').innerHTML, /value="E:\/Miniconda\/envs\/VISA\/python\.exe"/);
    assert.match(element('content').innerHTML, /Running Python/);
    assert.match(element('content').innerHTML, /E:\/Miniconda\/envs\/VISA\/python\.exe/);
    assert.match(element('content').innerHTML, /C:\/Program Files\/SIL Instrument Console\/resources/);
    click({ op: 'save-settings' });
    for (let i = 0; i < 5; i += 1) await new Promise((resolve) => setImmediate(resolve));
    const saved = calls.find((call) => call.args?.request?.method === 'settings_save');
    assert.equal(saved.args.request.params.settings.python_path, chosen);
    click({ op: 'stop' });
    for (let i = 0; i < 5; i += 1) await new Promise((resolve) => setImmediate(resolve));
    assert.doesNotMatch(element('content').innerHTML, /Running Python/);
    assert.doesNotMatch(element('content').innerHTML, /C:\/Program Files\/SIL Instrument Console\/resources/);
  } finally {
    globalThis.document = previous.document;
    globalThis.window = previous.window;
    globalThis.__TAURI__ = previous.tauri;
    globalThis.setInterval = previous.setInterval;
    globalThis.setTimeout = previous.setTimeout;
    globalThis.clearTimeout = previous.clearTimeout;
  }
});

test('reattaching to an existing worker shows its running code identity without restarting it', async () => {
  const previous = {
    document: globalThis.document, window: globalThis.window,
    tauri: globalThis.__TAURI__, setInterval: globalThis.setInterval,
    setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout,
  };
  const elements = new Map();
  const listeners = new Map();
  const methods = [];
  let failPing = false;
  const element = (id) => {
    if (!elements.has(id)) elements.set(id, {
      innerHTML: '', textContent: '', className: '', hidden: false,
      value: '', classList: { toggle() {} },
    });
    return elements.get(id);
  };
  try {
    globalThis.document = {
      getElementById: element,
      addEventListener: (name, handler) => listeners.set(name, handler),
    };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = () => 1;
    globalThis.setTimeout = () => 1;
    globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { invoke: async (command, args) => {
      if (command === 'worker_stop') return { steps: [], unreleased: [], voltage_zero: null };
      assert.equal(command, 'worker_request', 'reattachment must not restart the worker');
      const { method, id } = args.request;
      methods.push(method);
      if (method === 'ping' && failPing) {
        return { v: 2, id, ok: false, phase: 'failed_after_call_started', context: sessionContext, error: { type: 'Timeout', message: 'read timed out' } };
      }
      const result = method === 'status'
        ? { mode: 'real', devices: {}, observed_at: '2026-09-25T12:00:00Z', last_cleanup: null }
        : method === 'settings_get'
          ? { version: 1, python_path: 'E:/OldConda/envs/VISA/python.exe', bindings: {} }
          : method === 'ping'
            ? { mode: 'real', python_executable: 'D:/Anaconda/envs/VISA/python.exe',
              project_root: 'C:/Program Files/SIL Instrument Console/resources',
              environment_name: 'VISA', protocol_version: 2, session_id: sessionContext.session_id, roles: roleContexts }
            : assert.fail(`unexpected method ${method}`);
      return v2Reply(args.request, result);
    } } };
    await import('./legacy-main.js?test=existing-worker-identity');
    await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-page]' ? { dataset: { page: 'settings' } } : null;
    } } });
    assert.deepEqual(methods, ['status', 'ping', 'settings_get']);
    assert.match(element('content').innerHTML, /C:\/Program Files\/SIL Instrument Console\/resources/);
    assert.match(element('content').innerHTML, /id="python-path" value="D:\/Anaconda\/envs\/VISA\/python\.exe"/);
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-op]' ? { dataset: { op: 'stop' }, disabled: false } : null;
    } } });
    for (let i = 0; i < 5; i += 1) await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest: selector => selector === '[data-page]'
      ? { dataset: { page: 'settings' } } : null } });
    assert.match(element('content').innerHTML, /id="python-path" value="D:\/Anaconda\/envs\/VISA\/python\.exe"/);
    methods.length = 0;
    failPing = true;
    await import('./legacy-main.js?test=existing-worker-identity-ping-failure');
    await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-page]' ? { dataset: { page: 'settings' } } : null;
    } } });
    assert.deepEqual(methods, ['status', 'ping', 'settings_get']);
    assert.doesNotMatch(element('content').innerHTML, /Running Python/);
    assert.match(element('content').innerHTML, /Stop worker/);
    assert.match(element('notice').textContent, /runtime path read failed/);
  } finally {
    globalThis.document = previous.document;
    globalThis.window = previous.window;
    globalThis.__TAURI__ = previous.tauri;
    globalThis.setInterval = previous.setInterval;
    globalThis.setTimeout = previous.setTimeout;
    globalThis.clearTimeout = previous.clearTimeout;
  }
});

test('status polling preserves keyboard focus on the same enabled console control', async () => {
  const previous = {
    document: globalThis.document, window: globalThis.window,
    tauri: globalThis.__TAURI__, setInterval: globalThis.setInterval,
    setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout,
  };
  const listeners = new Map();
  const elements = new Map();
  const body = { tagName: 'BODY' };
  let active = body, scene, navButton, adoptButton, gainButton, poll;
  let actionRequests = 0;
  const makeScene = () => ({
    dataset: { stageScene: 'left' },
    closest(selector) { return selector === '[data-stage-scene]' ? this : null; },
    focus() { active = this; },
  });
  const makeButton = (dataset, disabled = false, textContent = '') => ({
    tagName: 'BUTTON', dataset, disabled, textContent,
    closest(selector) {
      return (selector === '[data-page]' && dataset.page) ||
        (selector === '[data-op]' && dataset.op) ? this : null;
    },
    focus() { active = this; },
  });
  const navigation = {
    html: '',
    set innerHTML(value) { this.html = value; navButton = makeButton({ page: 'fiber' });
      if (active?.dataset?.page) active = body; },
    get innerHTML() { return this.html; },
    contains(target) { return target === navButton; },
    querySelectorAll(selector) { return selector === '[data-page]' ? [navButton] : []; },
  };
  const content = {
    html: '',
    set innerHTML(value) { this.html = value; scene = makeScene();
      adoptButton = value.includes('data-op="fiber-adopt"')
        ? makeButton({ op: 'fiber-adopt', side: 'left' },
          /data-op="fiber-adopt" data-side="left" disabled/.test(value), 'Adopt current baseline') : null;
      const gainMarkup = value.match(/data-op="gain-enable-tec"([^>]*)>([^<]+)<\/button>/);
      gainButton = gainMarkup ? makeButton({ op: 'gain-enable-tec' }, gainMarkup[1].includes('disabled'), gainMarkup[2]) : null;
      if (active === scene || active?.dataset?.stageScene || active?.dataset?.op) active = body; },
    get innerHTML() { return this.html; },
    contains(target) { return target === scene || target === adoptButton || target === gainButton; },
    querySelector(selector) {
      return selector === '[data-stage-scene="left"]' && this.html.includes('data-stage-scene="left"')
        ? scene : null;
    },
    querySelectorAll(selector) {
      return selector === '[data-op]' ? [adoptButton, gainButton].filter(Boolean)
        : selector === '[data-stage-scene]' ? [scene] : [];
    },
  };
  const element = (id) => {
    if (id === 'content') return content;
    if (id === 'navigation') return navigation;
    if (!elements.has(id)) elements.set(id, {
      innerHTML: '', textContent: '', className: '', hidden: false, value: '',
      classList: { toggle() {} },
    });
    return elements.get(id);
  };
  const status = { mode: 'real', observed_at: '2026-09-25T12:00:00Z',
    devices: { fiber: { connected: true, state: 'READY',
      left: { side: 'left', serial: '2110148249-10', available: true,
        observed_voltage_v: { x: 0, y: 0, z: 0 }, baseline_known: false },
      right: null } }, last_cleanup: null };

  try {
    globalThis.document = {
      get activeElement() { return active; },
      getElementById: element, querySelectorAll: () => [],
      addEventListener: (name, handler) => listeners.set(name, handler),
    };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = (callback) => { poll = callback; return 1; };
    globalThis.setTimeout = () => 1;
    globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { invoke: async (command, args) => {
      if (command !== 'worker_request') throw new Error(`unexpected ${command}`);
      if (args.request.method === 'action') actionRequests += 1;
      const result = args.request.method === 'settings_get'
        ? { version: 1, python_path: 'D:/Anaconda/envs/VISA/python.exe', bindings: {} }
        : status;
      return v2Reply(args.request, result);
    } } };
    await import('./legacy-main.js?test=stage-focus');
    await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-page]' ? { dataset: { page: 'fiber' } } : null;
    } } });
    assert.match(content.innerHTML, /data-stage-scene="left"/);
    listeners.get('click')({ target: { closest: q => q === '[data-op]'
      ? { dataset: { op: 'confirm-context', role: 'fiber' } } : null } });
    await new Promise(setImmediate);
    const focusedScene = scene;
    focusedScene.focus();
    await poll();
    assert.notEqual(scene, focusedScene, 'polling replaces the scene DOM node');
    assert.equal(globalThis.document.activeElement, scene,
      'the new scene must regain focus so arrow keys continue to work');
    navButton.focus();
    const focusedNav = navButton;
    await poll();
    assert.notEqual(navButton, focusedNav, 'polling replaces navigation buttons');
    assert.equal(globalThis.document.activeElement, navButton,
      'the same sidebar destination must remain keyboard-focused');
    adoptButton.focus();
    const focusedAdopt = adoptButton;
    await poll();
    assert.notEqual(adoptButton, focusedAdopt, 'polling replaces panel buttons');
    assert.equal(globalThis.document.activeElement, adoptButton,
      'the same enabled panel action must remain keyboard-focused');
    status.devices.fiber.left.available = false;
    await poll();
    assert.equal(adoptButton.disabled, true, 'the new action is unavailable');
    assert.equal(globalThis.document.activeElement, body,
      'polling must not focus an action that became disabled');
    status.devices.gain = { connected: true, state: 'READY', temperature_c: 24,
      target_c: 24, current_ma: 0, tec_enabled: false, current_enabled: false };
    await poll();
    listeners.get('click')({ target: { closest: q => q === '[data-op]'
      ? { dataset: { op: 'confirm-context', role: 'gain' } } : null } });
    await new Promise(setImmediate);
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-page]' ? { dataset: { page: 'gain' } } : null;
    } } });
    assert.equal(gainButton.textContent, 'Enable TEC');
    gainButton.focus();
    await poll();
    assert.equal(globalThis.document.activeElement, gainButton,
      'unchanged action meaning should keep focus');
    status.devices.gain.tec_enabled = true;
    await poll();
    assert.equal(gainButton.textContent, 'Enable TEC');
    assert.equal(globalThis.document.activeElement, gainButton,
      'Gain enable keeps its fixed meaning when telemetry changes');
    assert.equal(actionRequests, 0, 'restoring focus must never invoke a device action');
  } finally {
    globalThis.document = previous.document;
    globalThis.window = previous.window;
    globalThis.__TAURI__ = previous.tauri;
    globalThis.setInterval = previous.setInterval;
    globalThis.setTimeout = previous.setTimeout;
    globalThis.clearTimeout = previous.clearTimeout;
  }
});

test('resource selection survives healthy polls but yields to changed ownership or lost status', async () => {
  const names = ['document', 'window', '__TAURI__', 'setInterval', 'setTimeout', 'clearTimeout'];
  const previous = Object.fromEntries(names.map((key) => [key, globalThis[key]]));
  const listeners = new Map(), fixed = new Map(), calls = [];
  const body = { closest: () => null, matches: () => false };
  let poll, binding, delay = false, release, fail = false, statusCalls = 0;
  const snapshot = { mode: 'real', devices: {}, last_cleanup: null };
  const content = {
    html: '',
    set innerHTML(html) {
      this.html = html;
      globalThis.document.activeElement = body; // Replacing DOM removes focus and the native popup.
      const row = /<select[^>]*data-binding="osa"([^>]*)>([\s\S]*?)<\/select>/.exec(html);
      binding = row ? { dataset: { binding: 'osa' },
        value: /<option value="([^"]*)" selected/.exec(row[2])?.[1] || '',
        disabled: /disabled/.test(row[1]),
        closest: (q) => q === '#content' ? content : null,
        matches: (q) => q === 'input,select' || q === '[data-binding]',
      } : null;
    },
    get innerHTML() { return this.html; },
    querySelectorAll: () => [],
  };
  const element = (id) => {
    if (id === 'content') return content;
    if (!fixed.has(id)) fixed.set(id, { innerHTML: '', textContent: '', hidden: false,
      classList: { toggle() {} } });
    return fixed.get(id);
  };
  try {
    globalThis.document = { activeElement: body, getElementById: element,
      addEventListener: (name, handler) => listeners.set(name, handler) };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = (callback) => { poll = callback; return 1; };
    globalThis.setTimeout = () => 1; globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { async invoke(command, args) {
      assert.equal(command, 'worker_request');
      const { method, id, params } = args.request;
      calls.push({ method, params });
      if (method === 'status') {
        statusCalls++;
        if (delay) { delay = false; await new Promise((resolve) => { release = resolve; }); }
        if (fail) throw new Error('worker status unavailable');
      }
      const result = method === 'settings_get'
        ? { version: 1, python_path: 'D:/Anaconda/envs/VISA/python.exe',
          bindings: { osa: 'GPIB0::4::INSTR' } }
        : structuredClone(snapshot);
      return v2Reply(args.request, result);
    } } };
    await import('./legacy-main.js?test=settings-selection-poll');
    await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest: (q) => q === '[data-page]'
      ? { dataset: { page: 'settings' } } : null } });
    const original = binding;
    globalThis.document.activeElement = original;
    const before = statusCalls;
    await poll(); await poll();
    assert.equal(binding, original, 'a healthy poll must retain the actual select node/native popup');
    assert.equal(globalThis.document.activeElement, original);
    assert.equal(statusCalls, before + 2, 'editing must not stop status reads');
    globalThis.document.activeElement = body;
    delay = true;
    const pending = poll();
    globalThis.document.activeElement = original;
    release(); await pending;
    assert.equal(binding, original, 'focus acquired during an in-flight read also retains the menu');
    original.value = 'SIM-OSA';
    listeners.get('change')({ target: original });
    assert.equal(binding.value, 'SIM-OSA', 'the chosen resource must reach the rendered binding');
    globalThis.document.activeElement = binding;
    const chosen = binding;
    snapshot.devices.osa = { connected: true, resource: 'SIM-OSA', state: 'READY' };
    await poll();
    assert.notEqual(binding, chosen, 'a newly owned role must update the configuration controls');
    assert.equal(binding.disabled, true);
    snapshot.devices = {};
    await poll();
    globalThis.document.activeElement = binding;
    fail = true;
    await poll();
    assert.equal(binding.disabled, true, 'lost worker status must disable resource editing');
    assert.match(content.innerHTML, /UNKNOWN/);
    assert.ok(calls.every(({ method }) => ['status', 'ping', 'settings_get'].includes(method)),
      'opening or preserving a selection must not connect, write or save anything');
  } finally {
    for (const [key, value] of Object.entries(previous)) globalThis[key] = value;
  }
});

test('status polling keeps an unsubmitted Gain target instead of replacing it with telemetry', async () => {
  const previous = {
    document: globalThis.document, window: globalThis.window,
    tauri: globalThis.__TAURI__, setInterval: globalThis.setInterval,
    setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout,
  };
  const listeners = new Map();
  const fixed = new Map();
  const body = { tagName: 'BODY', closest() { return null; }, matches() { return false; } };
  let poll, gainTarget, actionRequests = 0, delayNextStatus = false, releaseStatus;
  const content = {
    html: '',
    set innerHTML(value) {
      this.html = value;
      const match = value.match(/id="gain-temp"[^>]*value="([^"]*)"/);
      gainTarget = match ? { id: 'gain-temp', value: match[1], disabled: false,
        closest(selector) { return selector === '#content' ? content : null; },
        matches(selector) { return selector === 'input,select'; } } : null;
    },
    get innerHTML() { return this.html; },
    querySelectorAll(selector) {
      return selector.includes('input') && gainTarget ? [gainTarget] : [];
    },
  };
  const element = (id) => {
    if (id === 'content') return content;
    if (id === 'gain-temp') return gainTarget;
    if (!fixed.has(id)) fixed.set(id, {
      innerHTML: '', textContent: '', className: '', hidden: false,
      value: '', classList: { toggle() {} },
    });
    return fixed.get(id);
  };
  const status = { mode: 'real', observed_at: '2026-09-25T12:00:00Z',
    devices: { gain: { connected: true, state: 'READY', resource: 'SIM-GAIN',
      temperature_c: 24, target_c: 24, current_ma: 0,
      tec_enabled: false, current_enabled: false,
      fields: { target_c: { value: 24, quality: 'fresh', observed_age_s: 0 } } } }, last_cleanup: null };
  try {
    globalThis.document = {
      activeElement: body, getElementById: element,
      addEventListener: (name, handler) => listeners.set(name, handler),
    };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = (callback) => { poll = callback; return 1; };
    globalThis.setTimeout = () => 1;
    globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { invoke: async (command, args) => {
      if (command !== 'worker_request') throw new Error(`unexpected ${command}`);
      if (args.request.method === 'action') actionRequests += 1;
      if (args.request.method === 'status' && delayNextStatus) {
        delayNextStatus = false;
        await new Promise((resolve) => { releaseStatus = resolve; });
      }
      const result = args.request.method === 'settings_get'
        ? { version: 1, python_path: 'D:/Anaconda/envs/VISA/python.exe', bindings: {} }
        : status;
      return v2Reply(args.request, result);
    } } };
    await import('./legacy-main.js?test=gain-draft-poll');
    await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-page]' ? { dataset: { page: 'gain' } } : null;
    } } });
    assert.equal(gainTarget.value, '24');
    gainTarget.value = '26.5';
    const oldField = gainTarget;
    await poll();
    assert.notEqual(gainTarget, oldField, 'polling replaces the form node');
    assert.equal(gainTarget.value, '26.5', 'an unsubmitted target must survive telemetry refresh');
    assert.equal(actionRequests, 0, 'preserving a draft must not send a device action');
    status.devices.gain.resource = undefined;
    gainTarget.value = '30';
    await poll();
    assert.equal(gainTarget.value, '24',
      'a target must not carry forward when the connected resource identity is missing');
    status.devices.gain.resource = 'SIM-GAIN';
    await poll(); // Re-establish complete identity before editing this connection again.
    gainTarget.value = '26.5';
    delayNextStatus = true;
    const pending = poll();
    assert.equal(typeof releaseStatus, 'function', 'the status request must remain in flight');
    gainTarget.value = '28';
    globalThis.document.activeElement = gainTarget;
    releaseStatus();
    await pending;
    assert.equal(gainTarget.value, '28',
      'a response started before typing must not overwrite newer focused input');
  } finally {
    globalThis.document = previous.document;
    globalThis.window = previous.window;
    globalThis.__TAURI__ = previous.tauri;
    globalThis.setInterval = previous.setInterval;
    globalThis.setTimeout = previous.setTimeout;
    globalThis.clearTimeout = previous.clearTimeout;
  }
});

test('status polling keeps a Fiber move preview aligned with its unsubmitted displacement', async () => {
  const previous = {
    document: globalThis.document, window: globalThis.window,
    tauri: globalThis.__TAURI__, setInterval: globalThis.setInterval,
    setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout,
  };
  const listeners = new Map();
  const fixed = new Map();
  const fields = new Map();
  const body = { tagName: 'BODY', closest() { return null; }, matches() { return false; } };
  let poll, preview, actionRequests = 0;
  const content = {
    html: '',
    set innerHTML(value) {
      this.html = value;
      fields.clear();
      for (const axis of ['x', 'y', 'z']) {
        const id = `left-${axis}`;
        const match = value.match(new RegExp(`id="${id}"[^>]*value="([^"]*)"`));
        if (match) fields.set(id, { id, value: match[1], disabled: false });
      }
      const match = value.match(/class="stage-preview" id="preview-left">([^<]*)</);
      preview = match ? { textContent: match[1], classList: { toggle() {} } } : null;
    },
    get innerHTML() { return this.html; },
    querySelectorAll(selector) {
      return selector.includes('input') ? [...fields.values()] : [];
    },
  };
  const element = (id) => {
    if (id === 'content') return content;
    if (id === 'preview-left') return preview;
    if (fields.has(id)) return fields.get(id);
    if (!fixed.has(id)) fixed.set(id, {
      innerHTML: '', textContent: '', className: '', hidden: false,
      value: '', classList: { toggle() {} },
    });
    return fixed.get(id);
  };
  const status = { mode: 'real', observed_at: '2026-09-25T12:00:00Z',
    devices: { fiber: { connected: true, state: 'READY',
      left: { side: 'left', serial: '2110148249-10', resource: 'SIM-LEFT',
        available: true, baseline_known: true,
        estimated_position_um: { x: 0, y: 0, z: 0 },
        observed_voltage_v: { x: 0, y: 0, z: 0 } }, right: null } },
    last_cleanup: null };
  try {
    globalThis.document = {
      activeElement: body, getElementById: element,
      addEventListener: (name, handler) => listeners.set(name, handler),
    };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = (callback) => { poll = callback; return 1; };
    globalThis.setTimeout = () => 1;
    globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { invoke: async (command, args) => {
      if (command !== 'worker_request') throw new Error(`unexpected ${command}`);
      if (args.request.method === 'action') actionRequests += 1;
      const result = args.request.method === 'settings_get'
        ? { version: 1, python_path: 'D:/Anaconda/envs/VISA/python.exe', bindings: {} }
        : status;
      return v2Reply(args.request, result);
    } } };
    await import('./legacy-main.js?test=fiber-draft-preview');
    await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-page]' ? { dataset: { page: 'fiber' } } : null;
    } } });
    fields.get('left-x').value = '0.1';
    listeners.get('input')({ target: { dataset: { stageAxis: 'left' } } });
    assert.match(preview.textContent, /X \+0\.100 µm/);
    await poll();
    assert.equal(fields.get('left-x').value, '0.1');
    assert.match(preview.textContent, /X \+0\.100 µm/,
      'the preview must reflect the retained displacement after redraw');
    assert.equal(actionRequests, 0, 'restoring a move draft must not move a stage');
    status.devices.fiber.left.resource = null;
    fields.get('left-x').value = '0.2';
    await poll();
    assert.equal(fields.get('left-x').value, '0',
      'a move draft must not carry forward when stage identity is incomplete');
  } finally {
    globalThis.document = previous.document;
    globalThis.window = previous.window;
    globalThis.__TAURI__ = previous.tauri;
    globalThis.setInterval = previous.setInterval;
    globalThis.setTimeout = previous.setTimeout;
    globalThis.clearTimeout = previous.clearTimeout;
  }
});

test('a PM400 error-queue command leaves its returned errors visible in the advanced panel', async () => {
  const previous = {
    document: globalThis.document, window: globalThis.window,
    tauri: globalThis.__TAURI__, setInterval: globalThis.setInterval,
    setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout,
  };
  const listeners = new Map();
  const fixed = new Map();
  const content = { innerHTML: '', contains() { return false; }, querySelectorAll() { return []; } };
  const element = (id) => {
    if (id === 'content') return content;
    if (!fixed.has(id)) fixed.set(id, {
      innerHTML: '', textContent: '', className: '', hidden: false,
      value: '', classList: { toggle() {} },
    });
    return fixed.get(id);
  };
  const device = { connected: true, state: 'READY', resource: 'SIM-PM400',
    instrument: { manufacturer: 'THORLABS', model: 'PM400' },
    sensor: { name: 'SIM-SENSOR', capabilities: {} },
    catalog: { measurements: [], settings: [], commands: [
      { key: 'system.drain_errors', section: 'system', label: 'Read error queue', supported: true },
    ] } };
  const status = { mode: 'real', observed_at: '2026-09-25T12:00:00Z',
    devices: { pm400: device }, last_cleanup: null };
  let actions = 0;
  try {
    globalThis.document = {
      activeElement: { tagName: 'BODY' }, getElementById: element,
      addEventListener: (name, handler) => listeners.set(name, handler),
    };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = () => 1;
    globalThis.setTimeout = () => 1;
    globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { invoke: async (command, args) => {
      if (command !== 'worker_request') throw new Error(`unexpected ${command}`);
      let result;
      if (args.request.method === 'settings_get') {
        result = { version: 1, python_path: 'D:/Anaconda/envs/VISA/python.exe',
          bindings: { pm400: 'SIM-PM400' } };
      } else if (args.request.method === 'action') {
        actions += 1;
        assert.deepEqual(args.request.params, { role: 'pm400', name: 'command',
          command: 'system.drain_errors' });
        result = { result: [{ code: -200, message: 'Execution error', raw: '-200,"Execution error"' }],
          status: device };
      } else if (args.request.method === 'disconnect') {
        delete status.devices.pm400;
        result = {};
      } else if (args.request.method === 'connect') {
        status.devices.pm400 = device;
        result = {};
      } else result = status;
      return v2Reply(args.request, result);
    } } };
    await import('./legacy-main.js?test=pm-error-command-result');
    await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest: q => q === '[data-op]'
      ? { dataset: { op: 'confirm-context', role: 'pm400' } } : null } });
    await new Promise(setImmediate);
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-page]' ? { dataset: { page: 'pm400' } } : null;
    } } });
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-pm-tab]' ? { dataset: { pmTab: 'advanced' } } : null;
    } } });
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-op]' ? { dataset: { op: 'pm-command',
        command: 'system.drain_errors' }, disabled: false } : null;
    } } });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(actions, 1);
    assert.match(content.innerHTML, /Execution error/);
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-op]' ? { dataset: { op: 'disconnect', role: 'pm400' },
        disabled: false } : null;
    } } });
    await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest(selector) {
      return selector === '[data-op]' ? { dataset: { op: 'connect', role: 'pm400' },
        disabled: false } : null;
    } } });
    await new Promise((resolve) => setImmediate(resolve));
    assert.doesNotMatch(content.innerHTML, /Execution error/,
      'an error queue from the old connection must not appear after reconnect');
  } finally {
    globalThis.document = previous.document;
    globalThis.window = previous.window;
    globalThis.__TAURI__ = previous.tauri;
    globalThis.setInterval = previous.setInterval;
    globalThis.setTimeout = previous.setTimeout;
    globalThis.clearTimeout = previous.clearTimeout;
  }
});

test('an OSA trace is not carried into a later connection', async () => {
  const previous = {
    document: globalThis.document, window: globalThis.window,
    tauri: globalThis.__TAURI__, setInterval: globalThis.setInterval,
    setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout,
  };
  const listeners = new Map();
  const fixed = new Map();
  const content = { innerHTML: '', contains() { return false; }, querySelectorAll() { return []; } };
  const element = (id) => {
    if (id === 'content') return content;
    if (!fixed.has(id)) fixed.set(id, {
      innerHTML: '', textContent: '', className: '', hidden: false,
      value: id === 'python-path' ? 'D:/Anaconda/envs/VISA/python.exe'
        : id === 'worker-mode' ? 'real' : '', classList: { toggle() {} },
    });
    return fixed.get(id);
  };
  const click = (dataset) => listeners.get('click')({ target: { closest(selector) {
    if (selector === '[data-page]' && dataset.page) return { dataset };
    if (selector === '[data-op]' && dataset.op) return { dataset, disabled: false };
    return null;
  } } });
  const settle = async () => {
    for (let i = 0; i < 5; i += 1) await new Promise((resolve) => setImmediate(resolve));
  };
  let running = false;
  let identity = null;
  let connections = 0;
  try {
    globalThis.document = {
      activeElement: { tagName: 'BODY' }, getElementById: element,
      addEventListener: (name, handler) => listeners.set(name, handler),
    };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = () => 1;
    globalThis.setTimeout = () => 1;
    globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { invoke: async (command, args) => {
      if (command === 'worker_start') {
        running = true;
        return { mode: 'real', connected: false,
          python_executable: 'D:/Anaconda/envs/VISA/python.exe',
          project_root: 'D:/SIL_Experiments', environment_name: 'VISA', protocol_version: 2, session_id: sessionContext.session_id, roles: roleContexts };
      }
      assert.equal(command, 'worker_request');
      const { method, id } = args.request;
      if (method === 'status' && !running) throw new Error('instrument worker is not running');
      let result;
      if (method === 'settings_get') {
        result = { version: 1, python_path: 'D:/Anaconda/envs/VISA/python.exe',
          bindings: { osa: 'GPIB0::4::INSTR', pm400: null, voltage: null, gain: null } };
      } else if (method === 'connect') {
        identity = ++connections === 1 ? 'OSA-A' : 'OSA-B';
        result = { connected: true };
      } else if (method === 'disconnect') {
        identity = null;
        result = { connected: false };
      } else if (method === 'action') {
        assert.deepEqual(args.request.params, { role: 'osa', name: 'acquire', trace: 'A' });
        result = { result: { trace: 'A', wavelength_nm: [1500, 1501], power_dbm: [-80, -70] } };
      } else {
        result = { mode: 'real', observed_at: '2026-09-25T12:00:00Z',
          devices: identity ? { osa: { connected: true, state: 'READY',
            identity, resource: 'GPIB0::4::INSTR' } } : {}, last_cleanup: null };
      }
      return v2Reply(args.request, result);
    } } };
    await import('./legacy-main.js?test=osa-connection-trace');
    await settle();
    click({ page: 'settings' });
    click({ op: 'start' });
    await settle();
    click({ op: 'connect', role: 'osa' });
    await settle();
    click({ page: 'osa' });
    click({ op: 'osa-acquire' });
    await settle();
    assert.match(content.innerHTML, /DATA POINTS/);
    assert.match(content.innerHTML, /1500/);
    assert.match(content.innerHTML, /data-op="osa-export" >/);
    click({ op: 'disconnect', role: 'osa' });
    await settle();
    click({ op: 'connect', role: 'osa' });
    await settle();
    assert.match(content.innerHTML, /OSA-B/);
    assert.doesNotMatch(content.innerHTML, /1500/,
      'the old wavelength must not appear under a new instrument identity');
    assert.match(content.innerHTML, /data-op="osa-export" disabled/,
      'a trace from the previous connection must not remain exportable');
  } finally {
    globalThis.document = previous.document;
    globalThis.window = previous.window;
    globalThis.__TAURI__ = previous.tauri;
    globalThis.setInterval = previous.setInterval;
    globalThis.setTimeout = previous.setTimeout;
    globalThis.clearTimeout = previous.clearTimeout;
  }
});
