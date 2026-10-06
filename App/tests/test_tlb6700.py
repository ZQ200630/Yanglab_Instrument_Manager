"""Real v3 pipeline with a finite, injected Newport transport boundary."""
import dataclasses
import threading
import time
import unittest
import uuid

from App.worker.catalog import load_catalog, CatalogError
from App.worker.controller import DomainController
from App.worker.contracts_v3 import domain_config, RequestV3
from Code.Debugs.test_tlb6700 import Script


def config(number=1, **changes):
    value = dict(domain={'kind':'device', 'id':format(number, '032x')}, config_rev=1,
        driver_kind='laser', model_id='tlb6700', profile_id='newport-usb',
        params={'device_key':'6700 SN1012'}, expected_identity={}, members=[])
    value.update(changes)
    return domain_config(value)


class LaserAppTests(unittest.TestCase):
    def test_cached_laser_age_advances_while_next_observation_is_blocked(self):
        controller, cfg = self.controller(), config()
        controller.configure(cfg)
        self.assertEqual(self.call(controller, cfg, 'connect', {}).phase, 'completed')
        driver = controller.device(cfg.domain)
        original = driver._transport.query
        entered, release = threading.Event(), threading.Event()
        def gated(command):
            if command == 'SENS:WAVE':
                entered.set()
                if not release.wait(3):
                    raise AssertionError('finite observation gate timed out')
            return original(command)
        driver._transport.query = gated
        pending = controller.submit(RequestV3(uuid.uuid4().hex, 'action',
            {'name':'read_status', 'args':{}}, controller.context(cfg.domain)))
        try:
            self.assertTrue(entered.wait(2))
            key = 'device:' + cfg.domain.id
            before = controller.cached_status()['devices'][key]
            commands = list(driver._transport.commands)
            time.sleep(0.08)
            after = controller.cached_status()['devices'][key]
            self.assertEqual(before['laser']['received_at'], after['laser']['received_at'])
            self.assertGreaterEqual(after['sample_age_s'] - before['sample_age_s'], 0.05)
            self.assertEqual(commands, driver._transport.commands,
                             'Publishing cache age must not acquire hardware')
        finally:
            release.set()
            self.assertEqual(pending.result(5).phase, 'completed')

    def test_catalog_selects_newport_without_visa_or_serial_backend(self):
        model = load_catalog().model('tlb6700')
        self.assertEqual(model.category, 'Laser')
        self.assertEqual(model.profiles['newport-usb'].access, 'newport')
        self.assertNotIn('enable_current', model.operations)
        for params in ({'device_key':'6700 SN1012\n*RST'}, {'device_key':'COM4'},
                       {'device_key':'6700 SN1012', 'dll_path':'user.dll'}):
            with self.assertRaises(CatalogError):
                model.profiles['newport-usb'].validate(params)

    def controller(self):
        from Code.Utils.tlb6700 import TLB6700
        def factory(device_key):
            return TLB6700(device_key=device_key, _transport=Script(device_key[7:]))
        controller = DomainController(session_id='a'*32, port_enumerator=lambda:(),
            factories={'laser':factory})
        self.addCleanup(controller.close)
        return controller

    def call(self, controller, cfg, method, params):
        return controller.submit(RequestV3(uuid.uuid4().hex, method, params, controller.context(cfg.domain))).result(5)

    def test_connect_observe_typed_action_and_preserving_disconnect(self):
        controller, cfg = self.controller(), config()
        controller.configure(cfg)
        result = self.call(controller, cfg, 'connect', {})
        self.assertEqual(result.phase, 'completed', result)
        driver = controller.device(cfg.domain)
        self.assertFalse(controller.cached_status()['devices']['device:' + cfg.domain.id]['laser']['output_enabled'])
        before = len(driver._transport.commands)
        result = self.call(controller, cfg, 'action', {'name':'read_status', 'args':{}})
        self.assertEqual(result.phase, 'completed', result)
        self.assertEqual(len(driver._transport.commands) - before, 13, 'One refresh must acquire exactly one status sample')
        denied = self.call(controller, cfg, 'action', {'name':'write', 'args':{'command':'*RST'}})
        self.assertEqual(denied.phase, 'rejected_before_call')
        result = self.call(controller, cfg, 'disconnect', {})
        self.assertEqual(result.phase, 'completed', result)
        self.assertTrue(driver.resources_released)
        self.assertTrue(all(' ' not in command for command in driver._transport.commands))

    def test_changed_head_identity_cannot_gain_session_control(self):
        controller = self.controller()
        cfg = config(expected_identity={'model':'TLB-6700','serial':'1012','head_model':'TLB-6721','head_serial':'OTHER'})
        controller.configure(cfg)
        result = self.call(controller, cfg, 'connect', {})
        self.assertNotEqual(result.phase, 'completed')
        self.assertIsNone(controller.device(cfg.domain))

    def test_distinct_lasers_have_independent_domains_and_duplicate_key_is_busy(self):
        controller = self.controller()
        first, second = config(), config(2, params={'device_key':'6700 SN1020'})
        duplicate = config(3)
        for cfg in (first, second, duplicate):
            controller.configure(cfg)
        self.assertEqual(self.call(controller, first, 'connect', {}).phase, 'completed')
        self.assertEqual(self.call(controller, second, 'connect', {}).phase, 'completed')
        self.assertNotEqual(self.call(controller, duplicate, 'connect', {}).phase, 'completed')
        self.assertEqual(self.call(controller, first, 'disconnect', {}).phase, 'completed')
        self.assertEqual(self.call(controller, second, 'action', {'name':'read_status','args':{}}).phase, 'completed')


if __name__ == '__main__':
    unittest.main()
