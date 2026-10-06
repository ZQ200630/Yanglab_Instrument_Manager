import assert from 'node:assert/strict';
import test from 'node:test';

import { buildPmSettingAction, parsePmValue, recordPmSample, renderPm400 } from '../web/pm400.js';
import { pm400 as shellPm400 } from '../web/panels.js';

const fixture = {
  status: { devices: { pm400: {
    connected: true, state: 'READY',
    instrument: { manufacturer: 'THORLABS', model: 'PM400' },
    sensor: { name: 'S120C', serial_number: 'S123',
      capabilities: { power: true, energy: false } },
    catalog: {
      measurements: [
        { key: 'power', label: 'Optical power', unit: 'W', supported: true },
        { key: 'energy', label: 'Energy', unit: 'J', supported: false,
          requirements: ['energy'] },
      ],
      settings: [
        { key: 'sense.wavelength_nm', section: 'sense', label: 'Wavelength · nm', kind: 'float',
          readable: true, writable: true, read_supported: true, write_supported: false,
          write_requirements: ['wavelength_settable'], selectors: ['minimum','maximum'] },
      ],
      commands: [],
    },
  } } },
  pmTab: 'measurement', pmKind: 'power', pmMeasurement: null,
  pmReadings: {}, pmHistory: [], busy: false,
};

test('PM400 measurement panel disables sensor-unsupported measurement kinds', () => {
  const html = renderPm400(fixture, () => '');
  assert.match(html, /value="power"/);
  assert.match(html, /value="energy"[^>]*disabled/);
  assert.match(html, /S120C/);
});

test('PM400 trend does not mix incompatible measurement kinds', () => {
  const html = renderPm400({ ...fixture, pmHistory: [
    { kind: 'power', value: 1 }, { kind: 'energy', value: 5000 },
    { kind: 'power', value: 2 },
  ] }, () => '');
  const line = html.match(/<polyline points="([^"]+)"/);
  assert.ok(line, 'expected a trend polyline');
  assert.equal(line[1].split(' ').length, 2);
});

test('PM400 history records the returned measurement kind, not a stale selector', () => {
  const history = recordPmSample([], { kind: 'energy', value: 0.002, unit: 'J' }, 'power');
  assert.deepEqual(history, [{ kind: 'energy', value: 0.002 }]);
  assert.deepEqual(recordPmSample(history, { value: Infinity }, 'power'), history);
});

test('the main console panel presents PM400 measurement and advanced tabs', () => {
  const html = shellPm400(fixture);
  assert.match(html, /data-pm-tab="measurement"/);
  assert.match(html, /data-pm-tab="advanced"/);
});

test('PM400 sensor panel can read supported wavelength but cannot write without capability', () => {
  const html = renderPm400({ ...fixture, pmTab: 'sensor' }, () => '');
  assert.match(html, /data-op="pm-read" data-setting="sense\.wavelength_nm"/);
  assert.match(html, /data-op="pm-write" data-setting="sense\.wavelength_nm"[^>]*disabled/);
  assert.match(html, /wavelength_settable/);
});

test('PM400 system panel shows cached preexisting errors without querying or trusting device HTML', () => {
  const device = fixture.status.devices.pm400;
  const html = renderPm400({ ...fixture, pmTab: 'system', status: { devices: { pm400: {
    ...device, last_preexisting_errors: [
      { code: -200, message: '<old device error>', raw: '-200,"<old device error>"' },
    ],
  } } } }, () => '');
  assert.match(html, /Errors present before the last checked operation/);
  assert.match(html, /-200/);
  assert.match(html, /&lt;old device error&gt;/);
  assert.doesNotMatch(html, /<old device error>/);
});

test('PM400 input parser returns typed values and rejects malformed numbers', () => {
  assert.equal(parsePmValue('bool', 'true'), true);
  assert.equal(parsePmValue('register16', '65535'), 65535);
  assert.equal(parsePmValue('power_unit', 'DBM'), 'DBM');
  assert.throws(() => parsePmValue('float', 'Infinity'), /finite/i);
  assert.throws(() => parsePmValue('register8', '256'), /range/i);
  assert.throws(() => parsePmValue('bool', 'yes'), /boolean/i);
});

test('PM400 request builder keeps read and write capability gates distinct', () => {
  const wavelength = fixture.status.devices.pm400.catalog.settings[0];
  assert.deepEqual(buildPmSettingAction(wavelength, 'read', { selector: 'minimum' }), {
    role: 'pm400', name: 'read', setting: 'sense.wavelength_nm', selector: 'minimum',
  });
  assert.throws(() => buildPmSettingAction(wavelength, 'write', { raw: '1550' }), /unsupported/i);
  const unit = { key: 'sense.power_unit', kind: 'power_unit',
    writable: true, write_supported: true, sensitive: false };
  assert.deepEqual(buildPmSettingAction(unit, 'write', { raw: 'DBM' }), {
    role: 'pm400', name: 'write', setting: 'sense.power_unit', value: 'DBM',
  });
  const response = { key: 'sense.photodiode_response_a_per_w', kind: 'float',
    writable: true, write_supported: true, sensitive: true };
  assert.throws(() => buildPmSettingAction(response, 'write', { raw: '0.12' }), /confirm/i);
  assert.equal(buildPmSettingAction(response, 'write', { raw: '0.12', confirmed: true }).confirm, true);
});

test('PM400 configuration controls disable unsupported sensor measurement kinds', () => {
  const configuration = {
    key: 'measurement.configuration', section: 'measurement', label: 'Measurement configuration',
    kind: 'measurement_kind', readable: true, writable: true,
    read_supported: true, write_supported: true,
    choices: [{ key: 'power', label: 'Optical power', supported: true },
      { key: 'energy', label: 'Energy', supported: false }],
  };
  const device = fixture.status.devices.pm400;
  const html = renderPm400({ ...fixture, pmTab: 'advanced', status: { devices: {
    pm400: { ...device, catalog: { ...device.catalog, settings: [configuration] } },
  } } }, () => '');
  assert.match(html, /value="energy"[^>]*disabled/);
  assert.throws(() => buildPmSettingAction(configuration, 'write', { raw: 'energy' }), /unsupported/i);
});

test('PM400 advanced commands show their last returned error queue without treating it as telemetry', () => {
  const device = fixture.status.devices.pm400;
  const html = renderPm400({ ...fixture, pmTab: 'advanced',
    pmCommandResults: {
      'system.drain_errors': [{ code: -200, message: '<execution error>', raw: '-200,"<execution error>"' }],
      'root.clear_status': null,
    },
    status: { devices: { pm400: { ...device, catalog: { ...device.catalog,
      commands: [
        { key: 'system.drain_errors', section: 'system', label: 'Read error queue', supported: true },
        { key: 'root.clear_status', section: 'root', label: 'Clear status registers', supported: true },
      ],
    } } } },
  }, () => '');
  assert.match(html, /Last successful result/);
  assert.match(html, /-200/);
  assert.match(html, /&lt;execution error&gt;/);
  assert.doesNotMatch(html, /<execution error>/);
  assert.match(html, /Completed \(no return value\)/);
});

test('PM400 fault status disables measurement, settings, and advanced commands', () => {
  const device = fixture.status.devices.pm400;
  const setting = { key: 'sense.average_count', section: 'sense', label: 'Average count', kind: 'int',
    readable: true, writable: true, read_supported: true, write_supported: true };
  const command = { key: 'system.drain_errors', section: 'system', label: 'Read error queue',
    supported: true };
  const failed = { ...fixture, status: { devices: { pm400: {
    ...device, connected: false, state: 'FAULT', status_error: 'readback failed',
    catalog: { ...device.catalog, settings: [setting], commands: [command] },
  } } } };
  const measure = renderPm400(failed, () => '');
  assert.match(measure, /data-op="pm-measure" disabled/);
  assert.match(measure, /id="pm-kind" disabled/);
  assert.match(measure, /readback failed/);
  const sensor = renderPm400({ ...failed, pmTab: 'sensor' }, () => '');
  assert.match(sensor, /id="pm-value-sense\.average_count"[^>]*disabled/);
  assert.match(sensor, /data-op="pm-read" data-setting="sense\.average_count" disabled/);
  assert.match(sensor, /data-op="pm-write" data-setting="sense\.average_count" disabled/);
  const advanced = renderPm400({ ...failed, pmTab: 'advanced' }, () => '');
  assert.match(advanced, /data-op="pm-command" data-command="system\.drain_errors"\s+disabled/);
});
