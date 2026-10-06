import assert from 'node:assert/strict';
import test from 'node:test';

import { baselineConfirmations, cleanupWarning, connectRequest, fiberMoveRequest, markStatusUnknown, numericSetting, workerSubtitle } from '../web/operations.js';
import { overview, settings, voltage } from '../web/panels.js';

test('OSA and output-device connection requests acknowledge their lifecycle effects', () => {
  assert.deepEqual(connectRequest('osa', 'GPIB0::4::INSTR'), {
    role: 'osa', resource: 'GPIB0::4::INSTR', acknowledge_lifecycle: true,
  });
  assert.deepEqual(connectRequest('voltage', 'COM4'), {
    role: 'voltage', resource: 'COM4', acknowledge_lifecycle: true,
  });
  assert.deepEqual(connectRequest('fiber', null), {
    role: 'fiber', resource: null, acknowledge_lifecycle: false,
  });
  assert.throws(() => connectRequest('pm400', ''), /resource/i);
});

test('fiber movement is expressed only in logical laboratory axes', () => {
  assert.deepEqual(fiberMoveRequest('left', { x: 0.1, y: -0.3, z: 0 }), {
    role: 'fiber', name: 'move', side: 'left', dx: 0.1, dy: -0.3, dz: 0,
  });
  assert.throws(() => fiberMoveRequest('right', { x: -0.21, y: 0, z: 0 }), /toward-chip/i);
  assert.throws(() => fiberMoveRequest('left', { x: 0, y: 0, z: 0 }), /nonzero/i);
});

test('numeric input rejects invalid or out-of-range output targets', () => {
  assert.equal(numericSetting('14', 0, 14, 'voltage'), 14);
  assert.throws(() => numericSetting('14.1', 0, 14, 'voltage'), /range/i);
  assert.throws(() => numericSetting('', 0, 14, 'voltage'), /finite/i);
  assert.throws(() => numericSetting('Infinity', 0, 14, 'voltage'), /finite/i);
});

test('cleanup warning distinguishes failed actions from released resources', () => {
  assert.equal(cleanupWarning({ steps: [], unreleased: [], voltage_zero: null }), null);
  assert.match(cleanupWarning({
    steps: [{ role: 'voltage', action: 'zero_evidence', ok: false, error: 'timed out' }],
    unreleased: [], voltage_zero: null,
  }), /zero_evidence.*timed out/);
  assert.match(cleanupWarning({
    steps: [], unreleased: ['gain'], voltage_zero: null,
  }), /Unreleased.*gain/);
  assert.match(cleanupWarning({
    steps: [{ role: 'voltage', action: 'zero', ok: true, error: null }],
    unreleased: [], voltage_zero: { state: 'command_sent' },
  }), /zero not confirmed by telemetry/);
  assert.match(cleanupWarning(null), /Cleanup report missing/);
});

test('baseline adoption and nominal conversion require distinct attestations', () => {
  const messages = baselineConfirmations('left');
  assert.equal(messages.length, 2);
  assert.match(messages[0], /voltage readbacks.*baseline/);
  assert.match(messages[1], /MAX312D nominal conversion/);
  assert.throws(() => baselineConfirmations('middle'), /side/);
});

test('lost worker status makes old connected data stale and disables output controls', () => {
  const oldDevice = { connected: true, state: 'READY', resource: 'COM4',
    voltage_v: Array(8).fill(1), current_ma: Array(8).fill(0.1) };
  const state = {
    client: {}, mode: 'real', busy: false, inventory: null,
    bindings: { voltage: 'COM4' }, lastKnown: { voltage: oldDevice },
    lastKnownAt: { voltage: '2026-09-25T12:00:00Z' },
    status: { mode: 'real', devices: { voltage: oldDevice },
      observed_at: '2026-09-25T12:00:00Z', last_cleanup: null },
  };
  markStatusUnknown(state);
  assert.equal(state.mode, 'unknown');
  assert.deepEqual(state.status.devices, {});
  assert.equal(state.client !== null, true, 'retain the handle for orderly shutdown');
  assert.match(overview(state), /STALE/);
  assert.doesNotMatch(voltage(state), /data-op="voltage-apply"/);
  const configuration = settings(state);
  const workerCard = configuration.split('Resource bindings')[0];
  assert.match(workerCard, /<span class="badge warn">UNKNOWN<\/span>/);
  assert.doesNotMatch(workerCard, /RUNNING/);
  assert.match(workerCard, /Retry worker shutdown/);
  assert.match(configuration, /data-op="connect" data-role="voltage" disabled/);
  assert.match(configuration, /data-op="refresh" disabled/);
  assert.match(configuration, /data-op="save-settings" disabled/);
});

test('unknown worker status does not report zero open devices as fact', () => {
  assert.equal(workerSubtitle({ client: {}, mode: 'unknown', status: { devices: {} } }),
    'Connection status unknown');
  assert.equal(workerSubtitle({ client: {}, mode: 'real',
    status: { devices: { voltage: {} } } }), '1 instrument types open');
  assert.equal(workerSubtitle({ client: null }), 'Waiting for worker');
});
