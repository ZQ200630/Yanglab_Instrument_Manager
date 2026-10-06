"""Real-only startup contracts. Never open a serial or VISA resource."""
import contextlib
import io
import unittest
import importlib
from unittest.mock import patch
from App.tests.domain_fixture import config
from App.tests.driver_fixture import WireOSA
from App.worker.controller import DomainController, ConsoleError, _json_value
from App.worker.contracts_v3 import RequestV3
from Code.Debugs.test_fiber_coupling import make_mdt_status
from unittest.mock import patch

from App.worker.main import _parser
from App.worker.controller import ConsoleController, DomainController
from Code.Utils import AQ6370, GainDriver, PM400, VoltageSource


class RealOnlyTests(unittest.TestCase):
    def test_obsolete_instrument_backend_is_not_importable(self):
        with self.assertRaises(ModuleNotFoundError):
            importlib.import_module('App.worker.simulation')

    def test_explicit_probe_transport_is_used_without_external_driver_construction(self):
        controller = DomainController(session_id='a' * 32, host_verification=True,
                                      factories={'osa': WireOSA}, port_enumerator=lambda: ())
        cfg = config(21, 'osa')
        controller.configure(cfg)
        consent = {'accepted': True, 'stage': 'readonly', 'supervised': False,
                   'retain_session': False, 'binding': {'mode': 'real',
                   'domain': {'kind': 'device', 'id': cfg.domain.id}, 'config_rev': 1,
                   'model_id': 'aq6370', 'profile_id': 'gpib-visa', 'config_digest': 'd' * 64}}
        try:
            with patch('App.worker.verification.AQ6370',
                       side_effect=AssertionError('External driver construction forbidden')):
                out = controller.submit(RequestV3('check', 'check_online',
                    {'authorization': consent}, controller.context(cfg.domain))).result(2)
            self.assertEqual(out.phase, 'completed', out)
            self.assertEqual(out.result['identity']['serial'], 'UNITTEST')
            self.assertTrue(out.result['release_confirmed'])
        finally:
            controller.close()

    def test_mdt_public_status_has_json_safe_capabilities_and_held_voltage(self):
        status = make_mdt_status('UNITTEST')
        try:
            encoded = _json_value(status)
        except ConsoleError as error:
            self.fail(f'Public MDT metadata cannot reach the client: {error}')
        self.assertEqual(encoded['serial_number'], 'UNITTEST')
        self.assertEqual(encoded['supported_commands'], [])
        self.assertEqual(encoded['axes']['x']['actual_v'], 0.)
    def test_obsolete_backend_flag_is_rejected_before_activation(self):
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as rejected:
                _parser().parse_args(['--simulate'])
        self.assertEqual(rejected.exception.code, 2)

    def test_default_controller_selects_real_drivers_without_opening(self):
        controller = None
        with patch('pyvisa.ResourceManager', side_effect=AssertionError('VISA must not open')), \
                patch('serial.Serial', side_effect=AssertionError('Serial must not open')):
            try:
                try:
                    controller = ConsoleController()
                except TypeError as error:
                    self.fail(f'Default construction must choose real drivers: {error}')
                self.assertIs(controller._factory('osa'), AQ6370)
                self.assertIs(controller._factory('voltage'), VoltageSource)
                self.assertIs(controller._factory('gain'), GainDriver)
                self.assertIs(controller._factory('pm400'), PM400)
                self.assertEqual(controller.cached_status()['mode'], 'real')
            finally:
                if controller is not None:
                    controller.close()

    def test_domain_controller_boot_does_not_construct_instruments(self):
        controller = None
        with patch('pyvisa.ResourceManager', side_effect=AssertionError('VISA must not open')), \
                patch('serial.Serial', side_effect=AssertionError('Serial must not open')):
            try:
                try:
                    controller = DomainController(session_id='a' * 32, host_verification=True)
                except TypeError as error:
                    self.fail(f'Domain startup must be real-only and disconnected: {error}')
                self.assertEqual(controller.cached_status()['mode'], 'real')
                self.assertEqual(controller.registry.states(), ())
            finally:
                if controller is not None:
                    controller.close()


if __name__ == '__main__':
    unittest.main()
