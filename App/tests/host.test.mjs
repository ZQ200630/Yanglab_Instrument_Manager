import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';

import { createClient, probeExistingWorker } from './legacy-api.js';
import { installCloseGuard } from '../web/lifecycle.js';

test('ping and status callers share bounded query work and preserve typed failures', async () => {
  const wire = [];
  let release;
  const client = createClient((_command, { request }) => {
    wire.push(request);
    return new Promise(resolve => { release = () => resolve({ v: 2, id: request.id, ok: true,
      phase: 'completed', context: request.context, result: { method: request.method } }); });
  });
  const statuses = Array.from({ length: 100 }, () => client.request('status'));
  const pings = Array.from({ length: 100 }, () => client.request('ping'));
  await new Promise(setImmediate);
  assert.equal(wire.length, 1);
  assert.equal(wire[0].method, 'status');
  await assert.rejects(client.request('status', {}, { session_id: 'other', connection_id: null, epoch: 0 }),
    error => error.phase === 'rejected_before_call' && error.transportUnknown === false);
  release(); await Promise.all(statuses); await new Promise(setImmediate);
  assert.deepEqual(wire.map(r => r.method), ['status', 'ping']);
  release(); await Promise.all(pings);
  const next = client.request('status'); await new Promise(setImmediate);
  assert.equal(wire.length, 3); release(); await next;
  const rejected = createClient(async (_, { request }) => ({ v: 2, id: request.id, ok: false,
    phase: 'rejected_before_call', context: request.context, error: { type: 'HostAdmission', message: 'full' } }));
  await assert.rejects(rejected.request('status'), error => error.phase === 'rejected_before_call' && !error.transportUnknown);
  const uncertain = createClient(async () => { throw 'existing attempt: arbitrary transport error'; });
  await assert.rejects(uncertain.request('status'), error => error.transportUnknown === true);
});

test('the UI sends correlated, versioned requests and unwraps replies', async () => {
  const calls = [];
  const client = createClient(async (command, args) => {
    calls.push({ command, args });
    if (command === 'worker_start') return { running: true };
    if (command === 'worker_stop') return { stopped: true };
    return { v: 2, id: args.request.id, ok: true, phase: 'completed', context: null, result: { mode: 'real' } };
  });

  await client.start({ mode: 'real', pythonPath: 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe' });
  const reply = await client.request('ping');
  assert.deepEqual(reply.result, { mode: 'real' });
  assert.equal(reply.phase, 'completed');
  assert.equal(reply.requestId, calls[1].args.request.id);
  assert.equal(calls[1].args.request.context, null);
  assert.equal(calls[1].command, 'worker_request');
  assert.equal(calls[1].args.request.v, 2);
  assert.equal(calls[1].args.request.method, 'ping');
  assert.deepEqual(calls[1].args.request.params, {});
  await client.stop();
  assert.equal(calls[2].command, 'worker_stop');
});

test('the UI refuses a response from another request', async () => {
  const client = createClient(async () => ({ v: 2, id: 'stale', ok: true, phase: 'completed', context: null, result: {} }));
  await assert.rejects(client.request('status'), /response ID/);
});

test('a reloaded client cannot accept an old in-flight reply with a reused request ID', async () => {
  let oldId;
  const beforeReload = createClient(async (_command, args) => {
    oldId = args.request.id;
    return { v: 2, id: oldId, ok: true, phase: 'completed', context: null, result: { mode: 'real' } };
  });
  await beforeReload.request('status');

  let newId;
  const afterReload = createClient(async (_command, args) => {
    newId = args.request.id;
    return { v: 2, id: oldId, ok: true, phase: 'completed', context: null, result: { mode: 'real' } };
  });
  await assert.rejects(afterReload.request('status'), /response ID/);
  assert.notEqual(newId, oldId);
});

test('a worker failure is reported rather than converted to success', async () => {
  const client = createClient(async () => { throw new Error('worker exited'); });
  await assert.rejects(client.request('status'), /worker exited/);
});

test('the UI surfaces a typed instrument error', async () => {
  const client = createClient(async (_command, args) => ({
    v: 2, id: args.request.id, ok: false, phase: 'failed_after_call_started', context: null,
    error: { type: 'StageConnectionError', message: 'MDT prompt timed out' },
  }));
  await assert.rejects(client.request('connect', { role: 'fiber' }), /MDT prompt timed out/);
});

test('page reload reattaches read-only to an existing worker without restarting it', async () => {
  const methods = [];
  const client = { async request(method) { methods.push(method); return { result: { mode: 'real', devices: {} } }; } };
  assert.deepEqual(await probeExistingWorker(client), { mode: 'real', devices: {} });
  assert.deepEqual(methods, ['status']);
  assert.equal(await probeExistingWorker({ async request() {
    throw new Error('instrument worker is not running');
  } }), null);
  await assert.rejects(probeExistingWorker({ async request() {
    throw new Error('device outcome is unknown');
  } }), /unknown/);
});

test('typed failure preserves the attempted request and backend phase and context', async () => {
  const context = { session_id: 's', connection_id: 'c', epoch: 3 };
  let sent;
  const client = createClient(async (_command, args) => {
    sent = args.request;
    return { v: 2, id: sent.id, ok: false, phase: 'failed_after_call_started', context,
      error: { type: 'DeviceFault', message: 'write outcome unknown' } };
  });
  await assert.rejects(client.request('action', { role: 'voltage', name: 'set_all' }, context),
    error => error.phase === 'failed_after_call_started' && error.requestId === sent.id &&
      error.context === context);
  assert.deepEqual(sent.context, context);
});

test('host timeout carries request correlation even when native IPC rejects with a string', async () => {
  let sent;
  const client = createClient(async (_command, args) => { sent = args.request; throw 'action timed out; unknown'; });
  await assert.rejects(client.request('status'), error =>
    error.requestId === sent.id && error.transportUnknown === true && /unknown/.test(error.message));
});

test('native string rejection for an absent worker leaves a fresh launch idle', async () => {
  const calls = [];
  const client = createClient(async (command, args) => {
    calls.push({ command, method: args.request.method });
    throw 'instrument worker is not running'; // Rust Result::Err(String) over Tauri IPC.
  });
  assert.equal(await probeExistingWorker(client), null);
  assert.deepEqual(calls, [{ command: 'worker_request', method: 'status' }]);

  const fault = createClient(async () => { throw 'Python worker stopped; outcome unknown'; });
  await assert.rejects(probeExistingWorker(fault), (error) => error.message === 'Python worker stopped; outcome unknown' && error.transportUnknown === true);
});

test('idle close authorizes the Tauri fallback without invoking worker cleanup', async () => {
  const capability = JSON.parse(await readFile(
    new URL('../src-tauri/capabilities/default.json', import.meta.url), 'utf8'));
  let requestClose;
  const commands = [];
  // Characterize Tauri 2's native boundary: its close listener destroys the
  // window after the application callback unless preventDefault was called.
  // Keep our real guard and shipped capability; fake only the native command.
  await installCloseGuard({
    async onCloseRequested(callback) {
      requestClose = async () => {
        const event = { prevented: false, preventDefault() { this.prevented = true; } };
        await callback(event);
        if (!event.prevented) {
          assert.ok(capability.windows.includes('main'));
          assert.ok(capability.permissions.includes('core:window:allow-destroy'),
            'idle close is denied by the shipped main-window capability');
          commands.push('destroy');
        }
      };
      return async () => { commands.push('unlisten'); };
    },
    async close() { commands.push('close'); },
  }, {
    active: () => false,
    confirm: () => assert.fail('an idle window must not confirm cleanup'),
    stop: () => assert.fail('an idle window must not stop a worker'),
    report: () => assert.fail('an idle window has no cleanup report'),
    error: (error) => { throw error; },
  });
  await requestClose();
  assert.deepEqual(commands, ['destroy']);
});

test('window close waits for cleanup and stays open when evidence is incomplete', async () => {
  let handler, closeCalls = 0, removed = 0, stopCalls = 0;
  const windowHandle = {
    async onCloseRequested(callback) { handler = callback; return async () => { removed += 1; }; },
    async close() { closeCalls += 1; },
  };
  const reports = [];
  let active = true;
  await installCloseGuard(windowHandle, {
    active: () => active,
    confirm: () => true,
    async stop() { stopCalls += 1; return {
      steps: [{ role: 'voltage', action: 'zero', ok: false, error: 'no telemetry' }],
      unreleased: [], voltage_zero: null,
    }; },
    report: (value, warning) => { reports.push({ value, warning }); active = false; },
    error: (error) => { throw error; },
  });
  const event = { prevented: false, preventDefault() { this.prevented = true; } };
  await handler(event);
  assert.equal(event.prevented, true);
  assert.equal(stopCalls, 1);
  assert.equal(closeCalls, 0);
  assert.equal(removed, 0);
  assert.match(reports[0].warning, /no telemetry/);
});

test('window close proceeds only after a clean report and cancelled close does not stop', async () => {
  let handler, closeCalls = 0, removed = 0, stopCalls = 0, allowed = false;
  const windowHandle = {
    async onCloseRequested(callback) { handler = callback; return async () => { removed += 1; }; },
    async close() { closeCalls += 1; },
  };
  await installCloseGuard(windowHandle, {
    active: () => true,
    confirm: () => allowed,
    async stop() { stopCalls += 1; return { steps: [], unreleased: [], voltage_zero: null }; },
    report: () => {}, error: (error) => { throw error; },
  });
  const event = { prevented: false, preventDefault() { this.prevented = true; } };
  await handler(event);
  assert.equal(event.prevented, true);
  assert.equal(stopCalls, 0);
  allowed = true;
  await handler(event);
  assert.equal(stopCalls, 1);
  assert.equal(removed, 1);
  assert.equal(closeCalls, 1);
});

test('a shutdown timeout keeps the window open and preserves the unknown outcome', async () => {
  let handler, closed = false, reportedError = null;
  await installCloseGuard({
    async onCloseRequested(callback) { handler = callback; return async () => {}; },
    async close() { closed = true; },
  }, {
    active: () => true, confirm: () => true,
    async stop() { throw new Error('shutdown timed out; output unknown'); },
    report: () => { throw new Error('no report expected'); },
    error: (error) => { reportedError = error.message; },
  });
  const event = { prevented: false, preventDefault() { this.prevented = true; } };
  await handler(event);
  assert.equal(event.prevented, true);
  assert.equal(closed, false);
  assert.match(reportedError, /output unknown/);
});

test('failed window close retains immutable evidence and a second explicit close retries', async () => {
  let handler, stops = 0, closed = 0;
  const errors = [], reports = [];
  const evidence = { attempts: [{ id: 'first', phase: 'failed_after_call_started', error: { message: 'release failed' } }],
    received_cleanup_report: null, process_exit: null };
  await installCloseGuard({ onCloseRequested: async callback => { handler = callback; return () => {}; },
    close: async () => { closed++; } }, {
    active: () => true, confirm: () => true,
    stop: async () => { if (++stops === 1) throw `shutdown evidence: ${JSON.stringify(evidence)}`;
      return { steps: [], unreleased: [], voltage_zero: null }; },
    error: error => errors.push(error), report: report => reports.push(report),
  });
  await handler({ preventDefault() {} });
  assert.equal(closed, 0);
  assert.deepEqual(errors[0].shutdownEvidence, evidence);
  await handler({ preventDefault() {} });
  assert.equal(stops, 2);
  assert.equal(closed, 1);
  assert.equal(reports.length, 1);
  assert.deepEqual(errors[0].shutdownEvidence, evidence);
});
