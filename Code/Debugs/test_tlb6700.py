"""Finite Newport transport boundaries; no USB device or alternate app backend."""
import importlib.util
import unittest

from Code.Utils.common import DriverState, InstrumentProtocolError, InstrumentSafetyError


class Script:
    def __init__(self, serial='1012', head='TLB-6721'):
        self.key = '6700 SN' + serial
        self.commands = []
        self.opened = False
        self.close_error = False
        self.replies = {
            '*IDN?': 'New_Focus 6700 v2.4 03/19/14 SN' + serial,
            'SYST:LAS:MODEL?': head, 'SYST:LAS:SN?': 'P1001',
            'OUTP:STAT?': '0', 'OUTP:TRAC?': '1', 'SYST:MCONT?': 'LOC',
            'SOUR:CPOW?': '0', 'SOUR:WAVE?': '1060.00',
            'SOUR:CURR:DIODE?': '20', 'SOUR:POW:DIODE?': '10',
            'SOUR:VOLT:PIEZ?': '50', '*OPC?': '1', '*STB?': '0',
            'SENS:WAVE': '1060.01', 'SENS:CURR:DIODE': '0',
            'SENS:POW:DIODE': '0',
        }

    def open(self, key):
        if key != self.key:
            raise RuntimeError('wrong device')
        self.opened = True

    def query(self, command):
        if not self.opened:
            raise RuntimeError('closed')
        self.commands.append(command)
        if command in self.replies:
            return self.replies[command]
        parts = command.split(' ')
        if len(parts) == 2 and parts[0] + '?' in self.replies:
            self.replies[parts[0] + '?'] = parts[1]
            return 'OK'
        raise AssertionError('unreviewed command ' + command)

    def close(self):
        if self.close_error:
            raise OSError('retained native handle')
        self.opened = False


class TLBTests(unittest.TestCase):
    def test_actual_suffixed_head_is_not_given_standard_model_control_limits(self):
        device, wire = self.driver(Script(head='6722-P'))
        device.connect()
        self.assertEqual(device.identity['head_model'], '6722-P')
        self.assertIsNone(device.wavelength_range_nm)
        self.assertFalse(device.read_status().output_enabled)
        before = list(wire.commands)
        with self.assertRaises(InstrumentSafetyError): device.set_wavelength(1060, confirm=True)
        self.assertEqual(wire.commands, before)
        device.close()

    def test_explicit_output_off_is_available_during_tuning(self):
        device, wire = self.driver()
        device.connect()
        wire.replies['SYST:MCONT?'] = 'REM'
        wire.replies['*OPC?'] = '0'
        device.set_output(False, confirm=True)
        self.assertIn('OUTP:STAT 0', wire.commands)
        with self.assertRaises(InstrumentSafetyError):
            device.set_output(True, confirm=True)
        device.close()
    def driver(self, script=None):
        self.assertIsNotNone(importlib.util.find_spec('Code.Utils.tlb6700'), 'TLB6700 driver missing')
        from Code.Utils.tlb6700 import TLB6700
        script = script or Script()
        return TLB6700(device_key=script.key, _transport=script), script

    def test_connect_and_close_preserve_output_and_front_panel(self):
        device, wire = self.driver()
        device.connect()
        self.assertEqual(device.identity['serial'], '1012')
        self.assertEqual(device.identity['head_serial'], 'P1001')
        self.assertEqual(device.wavelength_range_nm, (1030.0, 1070.0))
        device.close()
        self.assertEqual(wire.commands, ['*IDN?', 'SYST:LAS:MODEL?', 'SYST:LAS:SN?'])
        self.assertTrue(device.resources_released)

    def test_probe_releases_and_binds_both_controller_and_head(self):
        device, wire = self.driver()
        report = device.probe_identity()
        self.assertTrue(report.release_confirmed)
        self.assertEqual(report.identity['model'], 'TLB-6700')
        self.assertEqual(report.identity['head_model'], 'TLB-6721')
        self.assertFalse(wire.opened)

    def test_identity_mismatch_releases_without_writes(self):
        device, wire = self.driver()
        wire.replies['*IDN?'] = 'New_Focus 6700 v2.4 03/19/14 SN9999'
        with self.assertRaises(Exception):
            device.connect()
        self.assertTrue(device.resources_released)
        self.assertEqual(wire.commands, ['*IDN?'])

    def test_unknown_head_remains_readable_but_wavelength_write_is_blocked(self):
        device, wire = self.driver(Script(head='6999-X'))
        device.connect()
        self.assertIsNone(device.wavelength_range_nm)
        before = list(wire.commands)
        with self.assertRaises(InstrumentSafetyError):
            device.set_wavelength(1060, confirm=True)
        for action in (lambda:device.set_remote(True, confirm=True), lambda:device.set_output(True, confirm=True),
                       lambda:device.set_tracking(True, confirm=True), lambda:device.set_piezo(50, confirm=True)):
            with self.assertRaises(InstrumentSafetyError):
                action()
        self.assertEqual(wire.commands, before)
        device.close()

    def test_readback_separates_setpoint_from_reported_wavelength(self):
        device, wire = self.driver()
        device.connect()
        sample = device.read_status()
        self.assertEqual(sample.wavelength_nm, 1060.01)
        self.assertEqual(sample.wavelength_setpoint_nm, 1060.0)
        self.assertFalse(sample.output_enabled)
        self.assertFalse(sample.remote)
        self.assertTrue(sample.tracking)
        self.assertIn('SENS:WAVE', wire.commands)
        self.assertTrue(all(' ' not in command for command in wire.commands))
        device.close()

    def test_malformed_read_latches_fault_without_safety_writes(self):
        device, wire = self.driver()
        device.connect()
        wire.replies['OUTP:STAT?'] = 'unknown'
        with self.assertRaises(InstrumentProtocolError):
            device.read_status()
        self.assertIs(device.state, DriverState.FAULT)
        self.assertTrue(all(' ' not in command for command in wire.commands))
        device.close()

    def test_nan_and_out_of_range_are_rejected_before_any_command(self):
        device, wire = self.driver()
        device.connect()
        before = list(wire.commands)
        for value in (True, float('nan'), 1029.9, 1070.1):
            with self.subTest(value=value), self.assertRaises(InstrumentSafetyError):
                device.set_wavelength(value, confirm=True)
        self.assertEqual(wire.commands, before)
        device.close()

    def test_control_requires_confirmation_and_explicit_remote_mode(self):
        device, wire = self.driver()
        device.connect()
        with self.assertRaises(InstrumentSafetyError):
            device.set_wavelength(1061)
        with self.assertRaises(InstrumentSafetyError):
            device.set_wavelength(1061, confirm=True)
        self.assertNotIn('SOUR:WAVE 1061', wire.commands)
        device.set_remote(True, confirm=True)
        device.set_wavelength(1061, confirm=True)
        self.assertIn('SOUR:WAVE 1061', wire.commands)
        device.close()

    def test_rejected_device_command_is_not_retried_or_reported_success(self):
        device, wire = self.driver()
        device.connect()
        wire.replies['SYST:MCONT?'] = 'REM'
        wire.replies['SOUR:WAVE 1061'] = 'VALUE OUT OF RANGE'
        with self.assertRaises(InstrumentProtocolError):
            device.set_wavelength(1061, confirm=True)
        self.assertEqual(wire.commands.count('SOUR:WAVE 1061'), 1)
        self.assertIs(device.state, DriverState.FAULT)
        device.close()

    def test_close_failure_retains_responsibility_for_explicit_retry(self):
        device, wire = self.driver()
        device.connect()
        wire.close_error = True
        with self.assertRaises(OSError):
            device.close()
        self.assertTrue(device.has_resource_responsibility)
        wire.close_error = False
        device.close()
        self.assertTrue(device.resources_released)


if __name__ == '__main__':
    unittest.main()
