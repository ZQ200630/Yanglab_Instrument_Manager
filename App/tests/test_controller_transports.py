"""Controller safety with real drivers and bounded serial transport doubles.

These are hardware-free boundary tests, never an App backend or data source.
"""
import threading
import unittest
from copy import deepcopy

from App.worker.controller import ConsoleController
from App.worker.contracts import Request
from Code.Utils.voltage import VoltageSource, encode_voltages
from Code.Debugs.test_voltage_evidence import EvidenceSerial
from Code.Utils.gain import GainDriver
from Code.Debugs.test_drivers import FakeGainSerial


class ControllerTransportTests(unittest.TestCase):
    def test_real_voltage_failed_close_keeps_immutable_attempt_and_can_retry(self):
        drivers, transports = [], []
        class RetainedVoltage(VoltageSource):
            fail_close = True
            def close(self):
                if self.fail_close:
                    raise OSError('Injected close failure before resource release')
                return super().close()
        def factory(port):
            serial = EvidenceSerial()
            driver = RetainedVoltage(port=port, serial_factory=lambda **_: serial,
                io_timeout=.01, startup_timeout=.5)
            drivers.append(driver); transports.append(serial)
            return driver
        controller = ConsoleController(factories={'voltage': factory}, port_enumerator=lambda: ())
        try:
            controller.handle('connect', {'role': 'voltage', 'resource': 'COM990',
                'acknowledge_lifecycle': True})
            driver, serial = drivers[-1], transports[-1]
            first_context = controller.context('voltage')
            failed = controller.submit(Request('close-first', 'disconnect', {'role': 'voltage'},
                controller.context('voltage'))).result(2)
            self.assertEqual(failed.phase, 'failed_after_call_started')
            history = controller.cached_status()['role_cleanup_attempts']
            self.assertEqual(len(history), 1)
            original = deepcopy(history[0])
            self.assertEqual(original['unreleased'], ['voltage'])
            self.assertEqual([step['action'] for step in original['steps']], ['zero', 'close'])
            self.assertTrue(original['steps'][0]['ok'])
            self.assertFalse(original['steps'][1]['ok'])
            zero = original['voltage_zero']
            # An asynchronous telemetry frame may or may not have arrived when
            # the attempt is frozen. A completed write alone is not readback.
            self.assertIn(zero['state'], ('command_sent', 'measured_zero'))
            if zero['state'] == 'measured_zero':
                self.assertGreaterEqual(zero['observed_at'], zero['sent_at'])
            else:
                self.assertIsNone(zero['observed_at'])
            self.assertTrue(serial.is_open)
            history[0]['steps'].clear(); history[0]['unreleased'].clear()
            history[0]['voltage_zero']['state'] = 'caller changed copy'
            driver.fail_close = False
            retry = controller.submit(Request('close-retry', 'disconnect', {'role': 'voltage'},
                controller.context('voltage'))).result(2)
            self.assertEqual(retry.phase, 'completed')
            history = controller.cached_status()['role_cleanup_attempts']
            self.assertEqual(history[0], original)
            self.assertNotEqual(history[1]['attempt_id'], original['attempt_id'])
            self.assertEqual(history[1]['unreleased'], [])
            self.assertFalse(serial.is_open)
            controller.handle('connect', {'role': 'voltage', 'resource': 'COM990',
                'acknowledge_lifecycle': True})
            self.assertNotEqual(controller.context('voltage').connection_id, first_context.connection_id)
            self.assertEqual(controller.cached_status()['role_cleanup_attempts'], history)
        finally:
            for driver in drivers: driver.fail_close = False
            for serial in transports: serial.release_read.set()
            self.assertEqual(controller.close()['unreleased'], [])

    def test_real_gain_cleanup_disables_current_before_tec_and_releases_serial(self):
        fields = [b'READY;T=22.000\r\n', b'READY;E=22.000\r\n',
                  b'READY;R=0\r\n', b'READY;C=0.000\r\n', b'READY;Q=0\r\n']
        serial = FakeGainSerial(fields * 4)
        def factory(port):
            return GainDriver(port=port, serial_factory=lambda **_: serial,
                start_watchdog=False)
        controller = ConsoleController(factories={'gain': factory}, port_enumerator=lambda: ())
        try:
            controller.handle('connect', {'role': 'gain', 'resource': 'COM991',
                'acknowledge_lifecycle': True})
            serial.writes.clear()
            report = controller.close()
            self.assertEqual(report['unreleased'], [])
            current_off = b'STQA000000\r\n'
            tec_off = b'STRA000000\r\n'
            self.assertIn(current_off, serial.writes)
            self.assertIn(tec_off, serial.writes)
            self.assertLess(serial.writes.index(current_off), serial.writes.index(tec_off))
            self.assertFalse(serial.is_open)
            self.assertFalse(controller.handle('ping', {})['connected'])
        finally:
            controller.close()

    def test_voltage_lifecycle_consent_precedes_real_driver_construction(self):
        constructed = []
        def forbidden(**kwargs):
            constructed.append(kwargs)
            raise AssertionError('No driver construction without lifecycle consent')
        controller = ConsoleController(factories={'voltage': forbidden}, port_enumerator=lambda: ())
        try:
            outcome = controller.submit(Request('no-consent', 'connect',
                {'role': 'voltage', 'resource': 'COM990'}, controller.context('voltage'))).result(2)
            self.assertEqual(outcome.phase, 'rejected_before_call')
            self.assertEqual(constructed, [])
        finally:
            controller.close()

    def test_real_voltage_final_zero_follows_older_entered_command(self):
        entered, release, first_zero = (threading.Event() for _ in range(3))
        serial = EvidenceSerial()
        calls = []
        class PausedVoltage(VoltageSource):
            def set_channel(self, channel, voltage):
                entered.set()
                if not release.wait(3):
                    raise TimeoutError('Unit-test command barrier not released')
                calls.append('old')
                return super().set_channel(channel, voltage)
            def zero(self, emergency=False):
                calls.append('zero')
                result = super().zero(emergency=emergency)
                first_zero.set()
                return result
        def factory(port):
            return PausedVoltage(port=port, serial_factory=lambda **_: serial,
                io_timeout=.01, startup_timeout=.5)
        controller = ConsoleController(factories={'voltage': factory}, port_enumerator=lambda: ())
        try:
            controller.handle('connect', {'role': 'voltage', 'resource': 'COM990',
                'acknowledge_lifecycle': True})
            calls.clear(); serial.writes.clear(); first_zero.clear()
            older = controller.submit(Request('older', 'action', {'role': 'voltage',
                'name': 'set_channel', 'channel': 1, 'voltage': .1}, controller.context('voltage')))
            self.assertTrue(entered.wait(2))
            stopped = controller.submit(Request('zero', 'action',
                {'role': 'voltage', 'name': 'zero'}, controller.context('voltage')))
            self.assertTrue(first_zero.wait(1))
            self.assertFalse(stopped.done(), 'Early zero must not be mistaken for the final barrier')
            release.set()
            older.result(2)
            self.assertEqual(stopped.result(2).phase, 'completed')
            self.assertEqual(calls, ['zero', 'old', 'zero'])
            self.assertEqual(serial.writes[-1], encode_voltages([0.] * 8))
            self.assertEqual(controller._requested_voltage, [0.] * 8)
        finally:
            release.set(); serial.release_read.set(); serial.close_error = None
            report = controller.close()
            self.assertEqual(report['unreleased'], [])
            self.assertFalse(serial.is_open)


if __name__ == '__main__':
    unittest.main()
