"""Finite metadata fixtures: no SetupAPI calls, ports or vendor installers."""
import importlib
import unittest


class SerialUSBDriverTests(unittest.TestCase):
    def inventory(self, records):
        self.assertIsNotNone(importlib.util.find_spec('Code.Utils.usb_drivers'),
                             'USB serial driver metadata inventory is required')
        return importlib.import_module('Code.Utils.usb_drivers').inventory(_records=records)

    def test_unplugged_device_is_not_a_missing_driver(self):
        result = self.inventory([])
        self.assertEqual(result['ch340']['state'], 'not_detected')
        self.assertEqual(result['cp210x']['state'], 'not_detected')

    def test_only_matching_no_driver_problem_offers_installation(self):
        records = [
            {'instance_id': r'USB\VID_1A86&PID_7523\A', 'problem_code': 28, 'service': ''},
            {'instance_id': r'USB\VID_10C4&PID_EA60\B', 'problem_code': 0, 'service': 'silabser'},
            {'instance_id': r'USB\VID_FFFF&PID_FFFF\C', 'problem_code': 28, 'service': ''},
        ]
        result = self.inventory(records)
        self.assertEqual(result['ch340']['state'], 'missing')
        self.assertEqual(result['cp210x']['state'], 'ready')
        self.assertEqual(len(result['ch340']['devices']), 1)

    def test_device_fault_and_unknown_status_are_not_missing(self):
        for code, service in [(10, 'silabser'), (22, 'silabser'), (None, ''), (0, '')]:
            with self.subTest(code=code, service=service):
                result = self.inventory([{'instance_id': r'USB\VID_10C4&PID_EA60\A',
                                         'problem_code': code, 'service': service}])
                self.assertEqual(result['cp210x']['state'], 'unavailable')

    def test_one_ready_adapter_does_not_hide_another_missing_adapter(self):
        result = self.inventory([
            {'instance_id': r'USB\VID_1A86&PID_7523\A', 'problem_code': 0, 'service': 'CH341SER_A'},
            {'instance_id': r'USB\VID_1A86&PID_7523\B', 'problem_code': 28, 'service': ''},
        ])
        self.assertEqual(result['ch340']['state'], 'missing')
        self.assertEqual([d['driver_state'] for d in result['ch340']['devices']], ['ready', 'missing'])

    def test_current_and_legacy_wch_service_names_are_ready(self):
        for service in ['CH341SER_A64', 'CH341SER_A', 'CH341SER']:
            result = self.inventory([{'instance_id': r'USB\VID_1A86&PID_7523\A',
                                     'problem_code': 0, 'service': service}])
            self.assertEqual(result['ch340']['state'], 'ready')

    def test_healthy_composite_parent_does_not_hide_ready_cp210x_interfaces(self):
        result = self.inventory([
            {'instance_id': r'USB\VID_10C4&PID_EA70\A', 'problem_code': 0, 'service': 'usbccgp'},
            {'instance_id': r'USB\VID_10C4&PID_EA70&MI_00\A', 'problem_code': 0, 'service': 'silabser'},
            {'instance_id': r'USB\VID_10C4&PID_EA70&MI_01\A', 'problem_code': 0, 'service': 'silabser'},
        ])
        self.assertEqual(result['cp210x']['state'], 'ready')
        self.assertEqual(len(result['cp210x']['devices']), 2)


if __name__ == '__main__':
    unittest.main()
