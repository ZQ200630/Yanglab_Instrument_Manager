"""Native Host leases, actual drivers with explicit bounded transports."""
import json
from pathlib import Path
import sys
import time
import unittest
import uuid
import dataclasses
import threading
from App.worker.controller import DomainController
from App.worker.contracts_v3 import RequestV3,DomainConfig,DomainRef
from App.tests.driver_fixture import WireVoltage, WireGain, factories, fiber_factory, FakePort
from Code.Utils.voltage import encode_voltages
from App.tests.domain_fixture import config
from App.worker.catalog import load_catalog
from App.tests.test_local_host_process import LocalHostProcessTests

class LocalLeaseTests(LocalHostProcessTests):
    def test_closed_client_cannot_reacquire_until_new_authenticated_session(self):
        domain=dict(kind='device',id=f'{1:032x}')
        self.assertTrue(self.client.call('acquire_control',dict(domain=domain))['ok'])
        self.assertTrue(self.client.call('close_client')['result']['released'])
        denied=self.client.call('acquire_control',dict(domain=domain))
        self.assertFalse(denied['ok'],denied)
        self.assertEqual(denied['error']['code'],'ClientClosing')
        self.assertTrue(self.client.call('ping')['ok'])
        fresh=self.connect();self.assertTrue(fresh.call('acquire_control',dict(domain=domain))['ok'])
    def seed_registry(self, directory):
        catalog = load_catalog()
        devices = []
        for index in (1, 2):
            model = catalog.model('aq6370')
            profile = catalog.profile('aq6370', 'gpib-visa')
            params = profile.validate({'resource': f'GPIB0::{index}::INSTR'})
            devices.append(dict(device_id=f'{index:032x}', name=f'OSA {index}',
                category=model.category, model_id=model.id, driver_id=model.driver_id,
                profile_id=profile.id, params=dict(params),
                expected_identity={'model': 'AQ6370E', 'serial': f'HOST-OSA-{index}'},
                identity_strength='operator_bound', verified_mode='real', config_rev=1,
                check_policy=dict(interval_s=30, enumeration=None, readonly=None)))
        registry = dict(version=2, host_id=uuid.uuid4().hex, registry_rev=1,
            settings=dict(python_path=sys.executable, host_name='Host contract fixture'),
            devices=devices, setups=[], drafts=[], tombstones=[])
        (Path(directory) / 'devices.json').write_text(json.dumps(registry))

    def test_two_clients_one_controller_and_gui_close_revokes_only_own_domain(self):
        one = dict(kind='device', id=f'{1:032x}')
        two = dict(kind='device', id=f'{2:032x}')
        other = self.connect()
        first = self.client.call('acquire_control', {'domain': one})
        self.assertTrue(first['ok'], first)
        self.assertEqual(other.call('acquire_control', {'domain': one})['error']['code'], 'ControlOwned')
        second = other.call('acquire_control', {'domain': two})
        self.assertTrue(second['ok'], second)
        self.assertFalse(other.call('renew_control', {'token': first['result']['token']})['ok'])
        old_epoch = first['result']['control_epoch']
        self.client.close()
        self.client = self.connect()
        deadline = time.monotonic() + 3
        while True:
            control = self.client.call('snapshot')['result']['control']
            if control['device:' + one['id']]['state'] == 'AVAILABLE':
                break
            self.assertLess(time.monotonic(), deadline)
            time.sleep(.025)
        self.assertGreater(control['device:' + one['id']]['control_epoch'], old_epoch)
        self.assertEqual(control['device:' + two['id']]['state'], 'CONTROLLED')
        self.assertFalse(self.client.call('renew_control', {'token': first['result']['token']})['ok'])
        self.assertTrue(self.client.call('acquire_control', {'domain': one})['ok'])

    def test_release_uses_controller_identity_and_safe_stop_does_not_stop_host(self):
        one = dict(kind='device', id=f'{1:032x}')
        first = self.client.call('acquire_control', {'domain': one})['result']
        other = self.connect()
        params = dict(domain=one, token=first['token'], control_epoch=first['control_epoch'])
        self.assertFalse(other.call('release_control', params)['ok'])
        safe = other.call('safe_stop', {'domain': one})
        self.assertTrue(safe['ok'], safe)
        self.assertFalse(safe['result']['physical_stop_confirmed'])
        self.assertTrue(self.client.call('ping')['ok'])
        self.assertFalse(self.client.call('release_control', params)['ok'])

    def test_expiry_is_ten_seconds_even_while_gui_keeps_reading(self):
        one = dict(kind='device', id=f'{1:032x}')
        two = dict(kind='device', id=f'{2:032x}')
        first = self.client.call('acquire_control', {'domain': one})['result']
        other = self.connect()
        second = other.call('acquire_control', {'domain': two})['result']
        start = time.monotonic()
        while time.monotonic() - start < 10.5:
            self.assertTrue(self.client.call('snapshot')['ok'])
            self.assertTrue(other.call('renew_control', {'token': second['token']})['ok'])
            time.sleep(.5)
        control = self.client.call('snapshot')['result']['control']
        self.assertEqual(control['device:' + one['id']]['state'], 'AVAILABLE')
        self.assertEqual(control['device:' + two['id']]['state'], 'CONTROLLED')
        self.assertFalse(self.client.call('renew_control', {'token': first['token']})['ok'])


class DomainRevocationRulesTests(unittest.TestCase):
    def setUp(self):
        ports = [FakePort('COM1', '2110148249-10'), FakePort('COM2', '160721175410')]
        sources = factories()
        sources['fiber'] = fiber_factory(ports)
        self.controller = DomainController(session_id='a' * 32, factories=sources,
                                           port_enumerator=lambda: ())
        self.addCleanup(self.controller.close)
        self.sequence = 0

    def call(self, configuration, method='connect', params=None, wait=True):
        self.sequence += 1
        future = self.controller.submit(RequestV3(str(self.sequence), method, params or {},
            self.controller.context(configuration.domain)))
        return future.result(4) if wait else future

    def test_started_voltage_call_has_final_zero_and_other_instance_is_untouched(self):
        entered, release, zeroed, closed = (threading.Event() for _ in range(4))
        self.addCleanup(release.set)
        class Voltage(WireVoltage):
            def zero(self, emergency=False):
                result = super().zero(emergency=emergency)
                if self.port == 'COM1':
                    zeroed.set()
                return result
            def close(self):
                if self.port == 'COM1':
                    closed.set()
                return super().close()
            def set_channel(self, channel, voltage):
                entered.set()
                if not release.wait(3):
                    raise TimeoutError('Offline barrier not released')
                super().set_channel(channel, voltage)
        self.controller._factories['voltage'] = Voltage
        one, two = config(1, 'voltage'), config(2, 'voltage')
        for value in (one, two):
            self.controller.configure(value)
            self.assertEqual(self.call(value, params={'acknowledge_lifecycle': True}).phase, 'completed')
        driver, other = self.controller.device(one.domain), self.controller.device(two.domain)
        zeroed.clear()
        action = self.call(one, 'action', {'name': 'set_channel', 'args': {'channel': 1, 'voltage': 1}}, False)
        self.assertTrue(entered.wait(1))
        stopped = self.call(one, 'disconnect', wait=False)
        self.assertTrue(zeroed.wait(1), 'Initial emergency zero must not wait for the held call')
        self.assertFalse(closed.is_set(), 'Resource release must wait for the final pass')
        self.assertFalse(stopped.done())
        self.assertIsNotNone(self.controller.context(one.domain).connection_id)
        self.assertEqual(other.state.name, 'READY')
        release.set()
        self.assertEqual(stopped.result(4).phase, 'completed')
        action.result(4)
        self.assertEqual(driver.test_wire.writes[-1], encode_voltages([0.] * 8))
        self.assertFalse(driver.test_wire.is_open)
        self.assertEqual(other.state.name, 'READY')

    def test_gain_cleanup_disables_current_before_tec(self):
        calls = []
        class Gain(WireGain):
            def disable_current(self):
                calls.append('current-off')
                return super().disable_current()
            def disable_tec(self):
                calls.append('tec-off')
                return super().disable_tec()
        self.controller._factories['gain'] = Gain
        one, two = config(1, 'gain'), config(2, 'gain')
        for value in (one, two):
            self.controller.configure(value)
            self.assertEqual(self.call(value, params={'acknowledge_lifecycle': True}).phase, 'completed')
        other = self.controller.device(two.domain)
        calls.clear()
        self.assertEqual(self.call(one, 'disconnect').phase, 'completed')
        self.assertIn('current-off', calls)
        self.assertLess(calls.index('current-off'), calls.index('tec-off'))
        self.assertEqual(other.state.name, 'READY')

    def test_fiber_cleanup_holds_voltage_and_invalidates_baseline(self):
        self.controller._port_enumerator = lambda: [FakePort('COM1', '2110148249-10')]
        left = dataclasses.replace(config(1, 'mdt'), expected_identity={'transport_serial': '2110148249-10'})
        setup = DomainConfig(DomainRef('setup', 'f' * 32), 1, 'fiber', 'fiber-coupling', None, {}, {}, (left,))
        self.controller.configure(setup)
        self.assertEqual(self.call(setup).phase, 'completed')
        self.assertEqual(self.call(setup, 'action', {'name': 'adopt_baseline', 'args': {
            'side': 'left', 'confirm': True, 'allow_nominal': True}}).phase, 'completed')
        self.assertEqual(self.call(setup, 'action', {'name': 'move', 'args': {
            'side': 'left', 'dx': .1, 'dy': 0.0, 'dz': 0.0}}).phase, 'completed')
        stage = self.controller.device(setup.domain).left
        child = self.controller.device(setup.domain)._drivers[stage.status.side]
        before = dict(child.status.axes)
        self.assertEqual(self.call(setup, 'disconnect').phase, 'completed')
        self.assertEqual(dict(child.status.axes), before, 'Close must hold, not zero the piezo')
        self.assertIsNone(stage.status.observed_voltage_v, 'Disconnected is not a fresh voltage observation')
        self.assertFalse(stage.status.baseline_known)

if __name__ == '__main__':
    unittest.main()
