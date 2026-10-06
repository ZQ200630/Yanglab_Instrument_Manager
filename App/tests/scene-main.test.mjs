const sessionContext = { session_id: 'ui-test-session', connection_id: null, epoch: 0 };
const roleContexts = Object.fromEntries(['osa', 'voltage', 'gain', 'pm400', 'fiber'].map(role => [role, { ...sessionContext }]));
let revision = 0;
function v2Reply(request, result) {
  revision++;
  const roles = Object.fromEntries(Object.keys(roleContexts).map(role => [role, {
    ...sessionContext, connection_id: result.devices?.[role] ? `${role}-1` : null,
    epoch: 0, revision, state: result.devices?.[role] ? 'READY' : 'DISCONNECTED',
  }]));
  return { v: 2, id: request.id, ok: true, phase: 'completed', context: request.context || sessionContext,
    result: ['ping', 'status'].includes(request.method) ? { session_id: sessionContext.session_id, revision, roles, ...result } : result };
}
import assert from 'node:assert/strict';
import test from 'node:test';

test('focused and in-flight form editing cannot hide a newly invalid stage pose', async () => {
  const previous = Object.fromEntries(['document', 'window', '__TAURI__', 'setInterval', 'setTimeout', 'clearTimeout'].map((k) => [k, globalThis[k]]));
  const fields = new Map(), elements = new Map(), listeners = new Map();
  let poll, release, delay = false, statusCalls = 0;
  const item = () => ({ innerHTML: '', textContent: '', hidden: false, value: '0', classList: { toggle() {} } });
  const root = { ...item(), querySelector(key) { if (!fields.has(key)) fields.set(key, item()); return fields.get(key); }, addEventListener() {}, removeEventListener() {} };
  const content = { ...item(), querySelectorAll: () => [] };
  const input = { ...item(), value: '0.1', closest: (q) => q === '#content' ? content : null, matches: (q) => q === 'input,select' };
  const element = (id) => {
    if (id === 'model-panel') return root;
    if (id === 'content') return content;
    if (id === 'left-x') return input;
    if (!elements.has(id)) elements.set(id, item()); return elements.get(id);
  };
  const snapshot = { mode: 'real', devices: { fiber: { connected: true,
    left: { side: 'left', serial: '2110148249-10', resource: 'SIM-LEFT', available: true,
      baseline_known: true, nominal_authorized: true, estimated_position_um: { x: 1, y: 0, z: 0 } }, right: null } } };
  try {
    globalThis.document = { activeElement: null, getElementById: element, querySelectorAll: () => [], addEventListener: (n, f) => listeners.set(n, f) };
    globalThis.window = { confirm: () => true };
    globalThis.setInterval = (f) => { poll = f; return 1; }; globalThis.setTimeout = () => 1; globalThis.clearTimeout = () => {};
    globalThis.__TAURI__ = { core: { async invoke(command, args) {
      assert.equal(command, 'worker_request');
      const method = args.request.method;
      assert.ok(['status', 'ping', 'settings_get'].includes(method), 'display never requests actions');
      if (method === 'status') { statusCalls++; if (delay) { delay = false; await new Promise((r) => { release = r; }); } }
      const result = method === 'settings_get' ? { bindings: {}, python_path: 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe' } : structuredClone(snapshot);
      return v2Reply(args.request, result);
    } } };
    await import('./legacy-main.js?test=scene-edit-fault');
    await new Promise((resolve) => setImmediate(resolve));
    listeners.get('click')({ target: { closest(q) { return q === '[data-page]' ? { dataset: { page: 'fiber' } } : null; } } });
    assert.match(content.innerHTML, /Session position estimate: <strong>Unknown<\/strong>/,
      'reload does not adopt a previous page baseline');
    assert.doesNotMatch(content.innerHTML, /manual change/);
    delay = true;
    const pending = poll();
    assert.equal(typeof release, 'function');
    globalThis.document.activeElement = input;
    snapshot.devices.fiber.left.fault = 'manual change'; snapshot.devices.fiber.left.baseline_known = false;
    release(); await pending;
    assert.doesNotMatch(content.innerHTML, /Session position estimate: <strong>X 1\.000/);
    assert.match(content.innerHTML, /Session position estimate: <strong>Unknown<\/strong>/);
    assert.match(content.innerHTML, /manual change/, 'new fault evidence must render during focused editing');
    assert.equal(input.value, '0.1', 'read-only refresh does not submit or rewrite draft');
    const before = statusCalls; await poll();
    assert.equal(statusCalls, before + 1, 'focused inputs cannot indefinitely stop status polling');
  } finally { for (const [key, value] of Object.entries(previous)) globalThis[key] = value; }
});
