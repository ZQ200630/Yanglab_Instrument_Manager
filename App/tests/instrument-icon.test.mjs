import test from 'node:test';
import assert from 'node:assert/strict';
import {renderConsole, mountConsole} from '../web/console-ui.js';
import {renderOverview} from '../web/overview.js';
import {createDeviceStore} from '../web/device-store.js';

const hostId = 'a'.repeat(32), deviceId = 'b'.repeat(32), setupId = 'c'.repeat(32);
function fixture(modelId, {setup = false, connected = false, name = 'Bench instrument'} = {}) {
  const device = {device_id: deviceId, model_id: modelId, name, params: {}, config_rev: 1};
  const host = {host_id: hostId, connected, mode: 'real', control: {}, registry: {
    devices: [device], setups: setup ? [{setup_id: setupId, name: 'Fiber coupling', members: [deviceId]}] : [],
  }};
  return {host, store: createDeviceStore(), route: `#/host/${hostId}/${setup ? 'setup/' + setupId : 'device/' + deviceId}`};
}
const svg = html => html.match(/<svg\b[^>]*data-instrument-icon="[^"]+"[\s\S]*?<\/svg>/)?.[0];

// Catches a missing/wrong model mapping, or replacing the identity icon with an interactive viewer.
for (const modelId of ['aq6370', 'voltage', 'gain', 'pm400', 'mdt693b']) {
  test(`${modelId} instrument opens with a compact noninteractive identity icon`, () => {
    const f = fixture(modelId), html = renderConsole(f.route, f.host, f.store), icon = svg(html);
    assert.ok(icon, 'the production instrument header must contain an icon');
    assert.match(icon, new RegExp(`data-instrument-icon="${modelId}"`));
    assert.match(icon, /width="48"/); assert.match(icon, /height="48"/);
    assert.match(icon, /aria-hidden="true"/); assert.match(icon, /focusable="false"/);
    assert.ok(html.indexOf(icon) < html.indexOf('Bench instrument'), 'icon belongs left of the instrument name');
    assert.doesNotMatch(icon, /tabindex|on\w+=|<canvas|<animate|<script/);
    assert.match(html, /data-op="connect"[^>]*disabled/);
    assert.match(html, /Host offline/);
  });
}

test('Fiber page identifies the stage while its physical controller card identifies the MDT', () => {
  const f = fixture('mdt693b', {setup: true});
  const page = renderConsole(f.route, f.host, f.store), overview = renderOverview(f.host, f.store);
  assert.match(svg(page) || '', /data-instrument-icon="fiber-coupling"/);
  assert.match(svg(overview) || '', /data-instrument-icon="mdt693b"/);
  assert.match(overview, new RegExp(`/setup/${setupId}`), 'existing setup navigation is preserved');
});

test('Overview uses the same identity icon without treating connectivity as an appearance or reading', () => {
  for (const modelId of ['aq6370', 'voltage', 'gain', 'pm400', 'mdt693b']) {
    const f = fixture(modelId), overview = renderOverview(f.host, f.store), page = renderConsole(f.route, f.host, f.store);
    assert.ok(svg(overview), 'each physical instrument card has an identity icon');
    assert.equal(svg(overview), svg(page));
    assert.equal(svg(renderOverview({...f.host, connected: true}, f.store)), svg(overview));
    assert.match(overview, /LOCAL/); assert.match(overview, /OFFLINE/);
    assert.match(overview, /Session closed/);
    assert.ok(overview.indexOf(svg(overview)) < overview.indexOf('Bench instrument'));
  }
});

test('supported instrument families have distinguishable geometry, not one recolored generic box', () => {
  const shapes = ['aq6370', 'voltage', 'gain', 'pm400', 'mdt693b'].map(modelId => {
    const f = fixture(modelId), icon = svg(renderOverview(f.host, f.store));
    assert.ok(icon);
    return icon.replace(/<svg[^>]*>|<title>[\s\S]*?<\/title>/g, '').replace(/\s(?:fill|stroke|stroke-width)="[^"]*"/g, '');
  });
  const f = fixture('mdt693b', {setup: true});
  shapes.push((svg(renderConsole(f.route, f.host, f.store)) || '').replace(/<svg[^>]*>|<title>[\s\S]*?<\/title>/g, '').replace(/\s(?:fill|stroke|stroke-width)="[^"]*"/g, ''));
  assert.equal(new Set(shapes).size, 6);
});

test('unknown model metadata cannot inject SVG or impersonate a known device icon', () => {
  for (const modelId of ['unsupported', '__proto__', '<script>alert(1)</script>']) {
    const f = fixture(modelId, {name: '<Unsafe name>'}), html = renderOverview(f.host, f.store);
    assert.match(svg(html) || '', /data-instrument-icon="instrument"/);
    assert.doesNotMatch(html, /<script>|<Unsafe name>/);
    assert.match(html, /&lt;Unsafe name&gt;/);
  }
});

test('production mounting no longer creates a large preview or GPU viewer', async () => {
  const keys = ['document', 'window', 'location', 'confirm', 'setInterval', 'clearInterval'];
  const previous = new Map(keys.map(key => [key, {exists: Object.hasOwn(globalThis, key), value: globalThis[key]}]));
  const elements = new Map(), windowListeners = new Map();
  const element = () => ({innerHTML: '', textContent: '', hidden: false, querySelectorAll: () => [],
    querySelector: () => element(), addEventListener() {}, removeEventListener() {}});
  const node = selector => {if (!elements.has(selector)) elements.set(selector, element()); return elements.get(selector);};
  const f = fixture('aq6370');
  f.store.apply({type: 'snapshot', host_id: hostId, boot_id: 'd'.repeat(32), seq: 1, data: f.host});
  try {
    globalThis.document = {activeElement: null, querySelector: node, getElementById: id => node('#' + id)};
    globalThis.window = {addEventListener: (name, listener) => windowListeners.set(name, listener)};
    globalThis.location = {hash: f.route};
    globalThis.confirm = () => {throw new Error('Icon rendering must not request an instrument action');};
    globalThis.setInterval = () => 1; globalThis.clearInterval = () => {};
    const client = {preferences: async () => {throw new Error('Offline visual fixture');}, disconnect() {}};
    const session = {client, clientFor:()=>client, clients:()=>[{hostId,client}], store: f.store, hostId, heartbeat() {}, navigate() {}, offline() {}};
    const ui = mountConsole(session, {event: {listen() {}}}); await ui.ready;
    assert.doesNotMatch(node('#model-panel').innerHTML, /data-scene-host|Reset view|Displacement scale/);
    assert.match(node('#content').innerHTML, /data-instrument-icon="aq6370"/);
    windowListeners.get('beforeunload')();
  } finally {
    for (const [key, item] of previous) {if (item.exists) globalThis[key] = item.value; else delete globalThis[key];}
  }
});
