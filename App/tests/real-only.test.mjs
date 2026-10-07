import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {connectLocalHost} from '../web/host-client.js';
import {createClient} from './legacy-api.js';
import {settings} from './legacy-settings.js';

test('production assets expose neither the obsolete console entry nor GPU preview modules', async () => {
  for (const path of ['legacy-main.js', 'scene/viewer.js', 'vendor/three/three.module.js']) {
    await assert.rejects(readFile(new URL('../web/' + path, import.meta.url)), {code: 'ENOENT'});
  }
});

test('legacy wire client rejects obsolete selection before native invocation', () => {
  let invoked = 0;
  const client = createClient(() => { invoked++; });
  assert.throws(() => client.start({mode: 'simulate', pythonPath: 'VISA/python.exe'}), /real/i);
  assert.equal(invoked, 0);
});

test('shared device settings never offers an instrument backend selector', () => {
  const html = settings({client: null, mode: null, bindings: {}, status: {devices: {}}});
  assert.doesNotMatch(html, /worker-mode|Offline simulation|value="simulate"/);
  assert.match(html, /Python executable/);
});

test('saved real settings cannot attach actions to an obsolete backend', async () => {
  let starts = 0;
  let disconnects = 0;
  const client = {
    connect: async () => ({connected: true, mode: 'simulate', worker_protocol: 3}),
    startHost: async () => { starts++; },
    disconnect: async () => { disconnects++; },
  };
  await assert.rejects(connectLocalHost(client, {pythonPath: 'VISA/python.exe'}),
    error => error.code === 'HostIncompatible');
  assert.equal(starts, 0);
  assert.equal(disconnects, 1);
});

test('an unstamped older bridge does not grant a hardware connection', async () => {
  const client = {connect: async () => ({connected: true}), disconnect: async () => {}};
  await assert.rejects(connectLocalHost(client, {pythonPath: 'VISA/python.exe'}),
    error => error.code === 'HostIncompatible');
});

test('real compatible attachment neither starts nor operates instruments', async () => {
  let starts = 0;
  const client = {
    connect: async () => ({connected: true, mode: 'real', worker_protocol: 3}),
    startHost: async () => { starts++; },
  };
  const result = await connectLocalHost(client, {pythonPath: 'VISA/python.exe'});
  assert.equal(result.mode, 'real');
  assert.equal(starts, 0);
});
