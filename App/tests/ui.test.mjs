import assert from 'node:assert/strict';
import test from 'node:test';

import { appendTelemetry, describeStage, osaCursorIndex, previewStageMove, voltageRows, gainSummary } from '../web/view-model.js';
import * as stageViews from '../web/view-model.js';
import { fiber, osa, voltage, gain, overview } from '../web/panels.js';
import {settings} from './legacy-settings.js';

test('Gain unknown evidence is never Off and retains an explicit safety Off action', () => {
  const html = gain({roles:{gain:{confirmed:true,mode:'READY',context:{session_id:'s1',connection_id:'c1'}}}, status: { devices: { gain: { connected: true, state: 'READY', fields: {} } } } });
  assert.match(html, /Unknown/);
  assert.doesNotMatch(html, />Off</);
  assert.match(html, /data-op="gain-enable-current" disabled/);
  assert.match(html, /data-op="gain-disable-current" >/);
  assert.match(html, /data-op="gain-disable-tec" >/);
});

test('Gain evidence retains false as Off and gates stale fields without age labels', () => {
  const fields = Object.fromEntries(Object.entries({ temperature_c: 24, target_c: 24,
    current_ma: 0, tec_enabled: true, current_enabled: false }).map(([key, value]) =>
    [key, { value, quality: 'fresh', observed_age_s: 1, connection_id: 'c1', revision: 1 }]));
  const state = { roles: { gain: { context: { session_id: 's1', connection_id: 'c1', epoch: 0 },
    revision: 1, mode: 'READY', confirmed: true } },
    status: { devices: { gain: { connected: true, state:'READY', fields, timing: { roundTripMs: 4000,
      receivedAtMs: 10000 } } } }, nowMs: 11000 };
  const html = gain(state);
  assert.match(html, /Unknown/);
  assert.match(html, /stale/);
  assert.doesNotMatch(html, /s ago/);
  assert.match(html, /data-op="gain-enable-current" disabled/);
  state.nowMs = 10000;
  assert.match(gain(state), /Off/);
  assert.match(gain(state), /data-op="gain-enable-current" >/);
  fields.tec_enabled.value = false;
  assert.match(gain(state), /data-op="gain-enable-current" disabled/);
});

test('Gain temperature history uses field identity and observation revision', () => {
  const sample = { fields: { temperature_c: { value: 24, quality: 'fresh', observed_age_s: 0,
    connection_id: 'c1', revision: 2 } } };
  const first = appendTelemetry([], sample, 'gain');
  assert.equal(first.length, 1);
  assert.equal(appendTelemetry(first, sample, 'gain').length, 1);
  assert.equal(appendTelemetry(first, { fields: { temperature_c: {
    ...sample.fields.temperature_c, revision: 1 } } }, 'gain').length, 1);
  assert.equal(appendTelemetry(first, { fields: { temperature_c: {
    ...sample.fields.temperature_c, revision: 3, quality: 'error' } } }, 'gain').length, 1);
});

test('retained connection without a device card still exposes disconnect under a host restriction', () => {
  const state = { client: {}, mode: 'real', status: { devices: {} }, roles: {
    osa: { context: { session_id: 's1', connection_id: 'connecting-1', epoch: 0 },
      mode: 'UNKNOWN', hostRestricted: true, normalPending: {} } } };
  const html = settings(state);
  assert.match(html, /data-op="disconnect" data-role="osa">/);
});

test('a new safety request cannot borrow the previous attempt final-completion label', () => {
  const control = { context: { session_id: 's1', connection_id: 'c1', epoch: 2 }, mode: 'STOPPING',
    safetyPending: { intent: 'tec_off', sentEpoch: 2 }, safetyEpoch: 2,
    safety: { state: 'STOP_HELD', effective_intent: 'current_off', attempt_id: 'old-stop' } };
  const state = { client: {}, roles: { gain: control }, status: { devices: { gain: { connected: true, fields: {} } } } };
  assert.match(gain(state), /Pending/);
  assert.doesNotMatch(gain(state), /Completed/);
  control.safetyEpoch = 3;
  control.safety = { state: 'WAITING_OLD', effective_intent: 'tec_off', attempt_id: 'new-stop' };
  assert.match(gain(state), /Waiting for prior call/);
});

test('global closing progress follows each accepted role attempt rather than its request label', () => {
  const state = { closing: true, client: {}, roles: { gain: {
    context: { session_id: 's1', connection_id: 'c1', epoch: 3 }, mode: 'CLOSING',
    shutdownEpoch: 2, safetyEpoch: 3, progress: 'Shutdown pending',
    safety: { state: 'WAITING_OLD', effective_intent: 'disconnect', attempt_id: 'closing-gain' },
  } }, status: { devices: { gain: { connected: true } } } };
  assert.match(overview(state), /Waiting for prior call/);
  assert.match(overview(state), /closing-gain/);
});

test('stage model preserves measured voltage while the position is unknown', () => {
  const left = describeStage({
    side: 'left', serial: '2110148249-10', available: true,
    observed_voltage_v: { x: 12, y: 9, z: 4 },
    baseline_known: false, estimated_position_um: null,
    toward_chip_sign: 1, restricted: false, fault: null,
  });
  assert.deepEqual(left.voltage, ['12.000 V', '9.000 V', '4.000 V']);
  assert.equal(left.position, 'Unknown');
  assert.equal(left.towardChip, '+X');
  assert.equal(left.canMove, false);
});

test('toward-chip preview applies each side’s physical direction and limit', () => {
  assert.equal(previewStageMove('left', { x: 0.3, y: 0, z: 0 }).allowed, false);
  assert.equal(previewStageMove('right', { x: -0.3, y: 0, z: 0 }).allowed, false);
  assert.equal(previewStageMove('left', { x: -0.3, y: 0, z: 0 }).allowed, true);
  assert.equal(previewStageMove('right', { x: 0.3, y: 0, z: 0 }).allowed, true);
  assert.equal(previewStageMove('left', { x: 0, y: 1.2, z: 0 }).allowed, false);
});

test('voltage rows show all eight observed currents and host zero evidence', () => {
  const rows = voltageRows({
    voltage_v: Array.from({ length: 8 }, (_, i) => i),
    current_ma: Array.from({ length: 8 }, (_, i) => i / 10),
    requested_voltage_v: [1.25, ...Array(7).fill(0)],
    zero_evidence: { state: 'unknown' },
  });
  assert.equal(rows.length, 8);
  assert.equal(rows[0].requested, '1.250 V');
  assert.equal(rows[7].observed, '7.000 V');
  assert.equal(rows[7].current, '0.700 mA');
  assert.equal(rows[0].zeroEvidence, 'Unknown');
  assert.equal(voltageRows({ voltage_v: Array(8).fill(0), current_ma: Array(8).fill(0) })[0].requested,
    'Unknown');
});

test('gain summary never claims driver interlock ready from temperature alone', () => {
  const summary = gainSummary({ fields: { temperature_c: { value: 24 },
    target_c: { value: 24 }, tec_enabled: { value: true }, current_ma: { value: 60 },
    current_enabled: { value: false } } });
  assert.equal(summary.temperature, '24.000 °C');
  assert.equal(summary.current, '60.000 mA');
  assert.equal(summary.interlock, 'Driver verification required');
});

test('OSA panel renders a large trace without overflowing the JS call stack or DOM', () => {
  const wavelength_nm = Array.from({ length: 200_000 }, (_, index) => 1500 + index / 1000);
  const power_dbm = wavelength_nm.map((_, index) => index === 100_001 ? 0 : -60);
  const html = osa({
    status: { devices: { osa: { connected: true, state: 'ready', identity: 'AQ6370' } } },
    trace: { trace: 'A', wavelength_nm, power_dbm }, busy: false,
  });
  assert.match(html, /OSA spectrum/);
  assert.ok(html.length < 100_000, 'SVG should be bounded even for a large trace');
  assert.match(html, /,47\.0/, 'narrow peaks must survive downsampling');
});

test('OSA cursor selects the nearest original point and exposes its numeric readout', () => {
  const trace = { trace: 'A', wavelength_nm: [1530, 1540, 1550, 1560],
    power_dbm: [-60, -40, -20, -50] };
  assert.equal(osaCursorIndex(trace, 0.68), 2);
  assert.equal(osaCursorIndex(trace, -10), 0);
  assert.equal(osaCursorIndex(trace, 10), 3);
  assert.equal(osaCursorIndex({ wavelength_nm: [], power_dbm: [] }, 0.5), null);
  const html = osa({ status: { devices: { osa: { connected: true, state: 'READY' } } },
    trace, cursor: 2 });
  assert.match(html, /data-osa-plot/);
  assert.match(html, /1550\.000 nm/);
  assert.match(html, /−20\.000 dBm/);
});

test('fiber control cards retain side identity and disarm motion without a baseline', () => {
  const left = { side: 'left', serial: '2110148249-10', available: true,
    observed_voltage_v: { x: 2, y: 3, z: 4 }, baseline_known: false };
  const html = fiber({ status: { devices: { fiber: { left, right: null } } },
    stageView: { left: { yaw: 12, pitch: -10 } } });
  assert.match(html, /Left stage → Chip/);
  assert.match(html, /Chip ← Right stage/);
  assert.match(html, /data-op="fiber-move" data-side="left" disabled/);
  assert.match(html, /data-op="fiber-move" data-side="right" disabled/);
  assert.doesNotMatch(html, /<canvas|data-scene-host/);
});

test('fiber baseline action stays unavailable for a faulted or restricted stage', () => {
  const base = { side: 'left', serial: '2110148249-10', available: true,
    observed_voltage_v: { x: 12, y: 9, z: 4 }, baseline_known: false,
    fault: null, restricted: false };
  for (const unsafe of [{ ...base, fault: 'monitor failed' },
    { ...base, restricted: true }]) {
    const html = fiber({ status: { devices: { fiber: { left: unsafe, right: null } } } });
    assert.match(html, /data-op="fiber-adopt" data-side="left" disabled/);
  }
  const healthy = fiber({ status: { devices: { fiber: { left: base, right: null } } },baselineConsent:{left:{baseline:true,nominal:true}} });
  assert.match(healthy, /data-op="fiber-adopt" data-side="left" >/);
});

test('fiber directional reference remains accessible beside each set of controls', () => {
  const left = { side: 'left', serial: '2110148249-10', available: true,
    observed_voltage_v: { x: 2, y: 3, z: 4 }, baseline_known: false };
  const html = fiber({ status: { devices: { fiber: { left, right: null } } } });
  assert.match(html, /data-stage-scene="left"[^>]*tabindex="0"/);
  assert.match(html, /aria-label="left fiber stage directions\. \+X toward chip/);
  assert.match(html, /aria-label="right fiber stage directions\. −X toward chip/);
  assert.deepEqual(stageViews.rotateStageView?.('left', { yaw: -35, pitch: -25 }, 'ArrowRight'),
    { yaw: -30, pitch: -25 });
  assert.deepEqual(stageViews.rotateStageView?.('left', { yaw: -35, pitch: -25 }, 'ArrowUp'),
    { yaw: -35, pitch: -20 });
  assert.deepEqual(stageViews.rotateStageView?.('right', { yaw: 85, pitch: 15 }, 'ArrowRight'),
    { yaw: 85, pitch: 15 });
  assert.deepEqual(stageViews.rotateStageView?.('right', { yaw: 12, pitch: 0 }, 'Home'),
    { yaw: 35, pitch: -25 });
  assert.equal(stageViews.rotateStageView?.('left', { yaw: -35, pitch: -25 }, 'Enter'), null);
});

test('voltage and gain panels keep measured output separate from editable targets', () => {
  const voltageHtml = voltage({ status: { devices: { voltage: {
    connected:true,quality:'fresh',
    voltage_v: Array(8).fill(0.5), current_ma: Array(8).fill(0.06),
    zero_evidence: { state: 'unknown' }, sample_age_s: 0.4,
  } } }, busy: false });
  assert.doesNotMatch(voltageHtml, /Telemetry sample|s ago/);
  assert.match(voltageHtml, /Target voltage/);
  assert.match(voltageHtml, /aria-label="Measured voltage">0\.500<small>V/);
  assert.match(voltageHtml, /id="voltage-1"[^>]*value="00\.000"/);
  const gainHtml = gain({ status: { devices: { gain: {
    temperature_c: 24, target_c: 24, current_ma: 0, tec_enabled: false,
    current_enabled: false, fields: { temperature_c: { value: 24, observed_age_s: 6.2, quality: 'fresh' } },
  } } } });
  assert.doesNotMatch(gainHtml, /s ago/);
  assert.match(gainHtml, /showing last readings/);
});

test('overview exposes cleanup steps and host-only zero evidence', () => {
  const html = overview({ status: { devices: {}, last_cleanup: {
    steps: [{ role: 'gain', action: 'current_off', ok: true, error: null }],
    unreleased: [], voltage_zero: { state: 'measured_zero' },
  } } });
  assert.match(html, /cleanup report/);
  assert.match(html, /gain.*current_off/);
  assert.match(html, /host telemetry/);
});

test('abnormal worker exit exposes original cleanup evidence without claiming resource release', () => {
  const received = { steps: [{ role: 'voltage', action: 'zero', ok: false, error: '<sensor unavailable>' }],
    unreleased: ['voltage'], voltage_zero: null };
  const report = { steps: [{ role: 'worker', action: 'shutdown', ok: false, error: 'abnormal exit' }],
    unreleased: ['unknown'], voltage_zero: null, resource_release_verified: false,
    process_exit: { confirmed: true, success: false, code: 19 }, received_cleanup_report: received };
  const html = overview({ status: { devices: {}, last_cleanup: report } });
  assert.match(html, /Process exited.*19/);
  assert.match(html, /Resource release unconfirmed/);
  assert.match(html, /Cleanup report received before exit/);
  assert.match(html, /&lt;sensor unavailable&gt;/);
  assert.doesNotMatch(html, /<sensor unavailable>/);
  assert.deepEqual(report.received_cleanup_report, received);
});

test('overview distinguishes connected, discovered and stale last-known devices', () => {
  const html = overview({
    status: { observed_at: '2026-09-25T12:00:00+00:00', devices: {
      osa: { connected: true, state: 'READY', resource: 'GPIB0::4::INSTR', identity: 'AQ6370E' },
    } },
    bindings: { osa: 'GPIB0::4::INSTR', voltage: 'COM4' },
    inventory: { visa: ['GPIB0::4::INSTR'], serial: [], fiber: {}, errors: {} },
    lastKnown: { voltage: { resource: 'COM4', identity: 'CH340 voltage board' } },
    lastKnownAt: { voltage: '2026-09-25T11:00:00+00:00' },
  });
  assert.match(html, /AQ6370E/);
  assert.match(html, /Found in inventory/);
  assert.match(html, /CH340 voltage board/);
  assert.match(html, /STALE/);
  assert.match(html, /Last status/);
  assert.match(html, /Bound resource not found/);
});

test('a role retained after uncertain disconnect is not presented as fully offline', () => {
  const state = {
    client: {}, mode: 'real', bindings: { osa: 'GPIB0::4::INSTR' },
    status: { observed_at: '2026-09-25T12:00:00+00:00', devices: {
      osa: { connected: false, state: 'UNKNOWN', resource: 'GPIB0::4::INSTR',
        identity: 'YOKOGAWA,AQ6370E,TEST,1.0' },
    } },
  };
  const summary = overview(state);
  assert.match(summary, /1 ROLE\(S\) RETAINED/);
  assert.match(summary, /Resource retained by worker; release unconfirmed/);
  assert.doesNotMatch(summary, /Previously identified; now disconnected/);
  const panel = osa(state);
  assert.match(panel, /data-op="disconnect" data-role="osa"[^>]*>Retry disconnect/);
  assert.doesNotMatch(panel, /data-op="connect" data-role="osa"/);
});

test('overview counts only unresolved sessions as retained roles', () => {
  const html = overview({ status: { devices: {
    osa: { connected: true, state: 'READY', resource: 'SIM-OSA' },
    voltage: { connected: false, state: 'UNKNOWN', resource: 'SIM-VOLTAGE' },
  } } });
  assert.match(html, /1 ROLE\(S\) RETAINED/);
  assert.doesNotMatch(html, /2 ROLE\(S\) RETAINED/);
});

test('telemetry trend appends only fresh samples and panels show fixed physical scales', () => {
  const voltageStatus = { received_at: 10, voltage_v: Array(8).fill(0.5),
    current_ma: Array(8).fill(0.06), sample_age_s: 0, zero_evidence: { state: 'unknown' } };
  let history = appendTelemetry([], voltageStatus, 'voltage');
  history = appendTelemetry(history, voltageStatus, 'voltage');
  assert.equal(history.length, 1, 'repeated sample timestamps must not duplicate points');
  history = appendTelemetry(history, { ...voltageStatus, received_at: 11 }, 'voltage');
  assert.equal(history.length, 2);
  const html = voltage({ status: { devices: { voltage: voltageStatus } },
    voltageHistory: history, busy: false });
  assert.match(html, /data-trend="voltage-1"/);
  assert.match(html, /aria-label="Last 2 host samples; fixed scale 0–14"/);
  assert.doesNotMatch(html, /<span class="trend-scale">/);
  const gainStatus = { fields: { temperature_c: { value: 24, observed_age_s: 0,
    quality: 'fresh', connection_id: 'c1', revision: 1 } } };
  const gainHistory = appendTelemetry([], gainStatus, 'gain');
  const gainHtml = gain({ status: { devices: { gain: gainStatus } }, gainHistory });
  assert.match(gainHtml, /data-trend="gain-temperature"/);
  assert.doesNotMatch(gainHtml, /Fixed scale 15–40 °C/);
  assert.match(gainHtml, /Temperature trend time window/);
});

test('faulted status disables acquisition and output controls before driver validation', () => {
  const fault = { connected: false, state: 'FAULT', status_error: 'telemetry stale' };
  const osaHtml = osa({ status: { devices: { osa: fault } }, busy: false });
  assert.match(osaHtml, /data-op="osa-acquire" disabled/);
  const voltageHtml = voltage({ status: { devices: { voltage: fault } }, busy: false });
  assert.match(voltageHtml, /data-op="voltage-zero" >/,
    'emergency zero remains available even when telemetry is unavailable');
  assert.match(voltageHtml, /data-op="voltage-apply" data-channel="1" disabled/);
  const gainHtml = gain({roles:{gain:{confirmed:true,mode:'FAULT',context:{session_id:'s1',connection_id:'c1'}}}, status: { devices: { gain: fault } }, busy: false });
  assert.match(gainHtml, /data-op="gain-enable-tec" disabled/);
  assert.match(gainHtml, /data-op="gain-enable-current" disabled/);
  assert.match(gainHtml, /data-op="gain-disable-current" >/);
  assert.match(gainHtml, /telemetry stale/);
});

test('device settings group USB adapter suggestions without choosing for the operator', () => {
  const html = settings({
    client: {}, pythonPath: 'D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe',
    desiredMode: 'real', bindings: { osa: 'GPIB0::4::INSTR',
      pm400: null, voltage: null, gain: 'COM9' }, status: { devices: {} },
    inventory: {
      serial: [
        { resource: 'COM4', description: 'CH340', serial: '' },
        { resource: 'COM5', description: 'CP210x', serial: '' },
        { resource: 'COM8', description: 'Other', serial: '' },
      ], visa: ['GPIB0::4::INSTR'], fiber: {}, errors: {},
      suggestions: { voltage: ['COM4'], gain: ['COM5'] },
    },
  });
  assert.match(html, /CH340 suggested/);
  assert.match(html, /CP210x suggested/);
  assert.match(html, /COM4/);
  assert.match(html, /COM5/);
  assert.match(html, /Previous binding \(not found\)/);
  assert.match(html, /COM9/);
  assert.match(html, /Other serial ports/);
  assert.doesNotMatch(html, /value="COM4" selected/,
    'USB metadata is a suggestion, not authorization to bind a port');
});

test('registered fiber serial ports are absent from Voltage and Gain choices', () => {
  const html = settings({
    client: {}, mode: 'real', pythonPath: 'D:/Anaconda/envs/VISA/python.exe',
    desiredMode: 'real', bindings: {}, status: { devices: {} },
    inventory: {
      serial: [
        { resource: 'COM6', description: 'MDT693B', serial: '2110148249-10' },
        { resource: 'COM5', description: 'CP210x', serial: 'GAIN-1' },
        { resource: 'COM8', description: 'MDT693B', serial: 'OTHER-MDT' },
      ], visa: [], errors: {},
      fiber: { left: { resource: 'COM6', serial: '2110148249-10' }, right: null,
        unknown: [{ resource: 'COM8', serial: 'OTHER-MDT' }] },
      suggestions: { voltage: [], gain: ['COM5'] },
    },
  });
  assert.doesNotMatch(html, /<option value="COM6"/);
  assert.doesNotMatch(html, /<option value="COM8"/);
  assert.match(html, /LEFT · 2110148249-10/);
  assert.match(html, /COM5/);
  for (const selected of ['COM6', String.raw`\\.\COM6`]) {
    const saved = settings({
      client: {}, mode: 'real', pythonPath: 'D:/Anaconda/envs/VISA/python.exe',
      desiredMode: 'real', bindings: { gain: selected }, status: { devices: {} },
      inventory: {
        serial: [{ resource: 'COM6', description: 'MDT693B', serial: '2110148249-10' },
          { resource: 'COM5', description: 'CP210x', serial: 'GAIN-1' }],
        visa: [], errors: {},
        fiber: { left: { resource: 'COM6', serial: '2110148249-10' }, right: null },
        suggestions: { voltage: [], gain: ['COM5'] },
      },
    });
    assert.doesNotMatch(saved, /<option value="COM6"/);
    assert.match(saved, /Saved port is reserved for a fiber stage/);
    assert.match(saved, /data-role="gain" disabled/);
  }
});
