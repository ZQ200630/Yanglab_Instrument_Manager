/** Visual-only fixture: runs the production renderer, never mounts a client/Host. */
import {renderConsole} from '../web/console-ui.js';
import {createDeviceStore} from '../web/device-store.js';
import {routeFor} from '../web/routes.js';
const hostId = 'a'.repeat(32), setupId = 'f'.repeat(32);
const definitions = [
  ['aq6370', 'OSA AQ6370'], ['voltage', '8-channel voltage source'],
  ['gain', 'Gain Chip Driver'], ['pm400', 'PM400'],
  ['mdt693b', 'Left MDT693B'], ['mdt693b', 'Right MDT693B'],
];
const devices = definitions.map(([model_id, name], index) => ({
  device_id: String(index + 1).repeat(32), model_id, name, config_rev: 1,
  params: {}, expected_identity: {},
}));
const host = {host_id: hostId, host_name: 'Static fixture', mode: 'real', connected: false, control: {},
  registry: {devices, drafts: [], setups: [{setup_id: setupId, name: 'Fiber coupling', config_rev: 1,
    members: devices.filter(d => d.model_id === 'mdt693b').map(d => d.device_id)}]},
};
const routes = [
  ['#overview', 'Overview'], ...devices.map(d => [routeFor(hostId, {kind: 'device', id: d.device_id}), d.name]),
  [routeFor(hostId, {kind: 'setup', id: setupId}), 'Fiber coupling'], ['#devices', 'Device setup'], ['#settings', 'Settings'],
];
const store = createDeviceStore();
document.querySelector('#navigation').innerHTML = routes.map(([href, label]) => `<a href="${href}">${label}</a>`).join('');
function render() {document.querySelector('#content').innerHTML = renderConsole(location.hash || '#overview', host, store);}
window.addEventListener('hashchange', render); render();
