"""Native ABI/lifecycle tests with finite DLL boundary inputs."""
import importlib.util
import ctypes as ct
from types import SimpleNamespace
import tempfile
import unittest
import threading
from pathlib import Path


class Native:
    def __init__(self):
        self.open_calls = 0
        self.close_calls = 0
        self.commands = []
    def open(self):
        self.open_calls += 1
        return ('6700 SN1012', '6700 SN1020')
    def query(self, key, command):
        self.commands.append((key, command))
        return key
    def close(self):
        self.close_calls += 1


class NewportTests(unittest.TestCase):
    def test_read_failure_retries_only_reviewed_getters_and_only_once(self):
        api = self.api()
        for command in ('*IDN?', 'OUTP:STAT 1'):
            native = api._NativeUSB.__new__(api._NativeUSB)
            calls, drains = [], []
            def transaction(index, cmd):
                calls.append(cmd)
                if len(calls) == 1:
                    raise api._SDKReadFailure('read failed (-1)')
                return 'fresh'
            native._transaction = transaction
            native._drain = lambda index: drains.append(index)
            if command == '*IDN?':
                self.assertEqual(native._query_index(0, command), 'fresh')
                self.assertEqual(len(calls), 2)
                self.assertEqual(drains, [0])
            else:
                with self.assertRaises(api._SDKReadFailure): native._query_index(0, command)
                self.assertEqual(len(calls), 1)
                self.assertEqual(drains, [])
        native._transaction = lambda *args: (_ for _ in ()).throw(api._SDKReadFailure('still failed'))
        with self.assertRaises(api._SDKReadFailure): native._query_index(0, '*IDN?')

    def test_sdk_lifetime_stays_on_one_thread_across_callers(self):
        api = self.api()
        threads = []
        native = Native()
        original_open, original_query, original_close = native.open, native.query, native.close
        def record(function):
            def invoke(*args):
                threads.append(threading.get_ident())
                return function(*args)
            return invoke
        def factory():
            threads.append(threading.get_ident())
            return native
        native.open, native.query, native.close = map(record, (original_open, original_query, original_close))
        wire = api.NewportTransport(_bus=api.NewportBus(_native_factory=factory))
        wire.open('6700 SN1012')
        reader = threading.Thread(target=lambda: wire.query('*IDN?'))
        reader.start(); reader.join(2)
        self.assertFalse(reader.is_alive())
        wire.close()
        self.assertEqual(len(set(threads)), 1, 'SDK construction, open, query and close must share a thread')
        self.assertNotEqual(threads[0], threading.get_ident())

    def test_readonly_error_resynchronizes_once_but_write_is_never_replayed(self):
        api = self.api()
        for command, replies, expected in [('*IDN?', ['COMMAND NOT VALID', 'fresh'], 'fresh'),
                                           ('OUTP:STAT 1', ['COMMAND NOT VALID'], 'COMMAND NOT VALID')]:
            native = api._NativeUSB.__new__(api._NativeUSB)
            native._ids = {'6700 SN1012':0}
            calls = []
            def transaction(index, cmd):
                calls.append(cmd)
                return replies.pop(0)
            native._transaction = transaction
            native._drain = lambda index: None
            self.assertEqual(native.query('6700 SN1012',command),expected)
            self.assertEqual(len(calls),2 if command=='*IDN?' else 1)
    def test_missing_key_with_failed_cleanup_remains_owned(self):
        api = self.api()
        native = Native()
        native.close = lambda: (_ for _ in ()).throw(OSError('retained'))
        bus = api.NewportBus(_native_factory=lambda: native)
        wire = api.NewportTransport(_bus=bus)
        with self.assertRaises(OSError): wire.open('6700 SN9999')
        self.assertTrue(wire.is_open)
        with self.assertRaises(Exception): api.NewportTransport(_bus=bus).open('6700 SN1012')
        native.close = lambda: None
        wire.close()
        self.assertFalse(wire.is_open)
    def api(self):
        self.assertIsNotNone(importlib.util.find_spec('Code.Utils.newport_usb'), 'Newport transport missing')
        from Code.Utils import newport_usb
        return newport_usb

    def test_duplicate_claim_cannot_close_or_command_another_controller(self):
        api = self.api()
        native = Native()
        bus = api.NewportBus(_native_factory=lambda: native)
        first = api.NewportTransport(_bus=bus)
        duplicate = api.NewportTransport(_bus=bus)
        second = api.NewportTransport(_bus=bus)
        first.open('6700 SN1012')
        with self.assertRaises(Exception):
            duplicate.open('6700 SN1012')
        duplicate.close()
        second.open('6700 SN1020')
        first.close()
        self.assertEqual(second.query('*IDN?'), '6700 SN1020')
        self.assertEqual(native.close_calls, 0)
        second.close()
        self.assertEqual(native.open_calls, 1)
        self.assertEqual(native.close_calls, 1)

    def test_failed_native_release_stays_owned_until_retry(self):
        api = self.api()
        native = Native()
        def failed():
            raise OSError('cannot release')
        native.close = failed
        bus = api.NewportBus(_native_factory=lambda: native)
        wire = api.NewportTransport(_bus=bus)
        wire.open('6700 SN1012')
        self.assertFalse(bus.resources_released)
        with self.assertRaises(OSError):
            wire.close()
        other = api.NewportTransport(_bus=bus)
        with self.assertRaises(Exception):
            other.open('6700 SN1012')
        native.close = lambda: None
        wire.close()
        self.assertTrue(bus.resources_released)
        other.open('6700 SN1012')
        other.close()

    def test_dll_selection_rejects_wrong_architecture_before_loading(self):
        api = self.api()
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'usbdll.dll'
            content = bytearray(256)
            content[:2] = b'MZ'
            content[60:64] = (128).to_bytes(4, 'little')
            content[128:132] = b'PE\0\0'
            content[132:134] = (0x14c).to_bytes(2, 'little')
            path.write_bytes(content)
            with self.assertRaises(Exception):
                api.validate_dll(path, bits=64)

    def test_metadata_inventory_does_not_open_sdk_or_query_instrument(self):
        api = self.api()
        result = api.inventory(_records=[{'instance_id': 'USB\\VID_104D&PID_100A\\TEST',
            'description': 'Tunable Laser Controller', 'service': 'WINUSB', 'problem_code': 0}],
            _sdk_status={'state': 'missing', 'message': 'SDK not installed'})
        self.assertEqual(len(result['devices']), 1)
        self.assertEqual(result['devices'][0]['driver_state'], 'ready')
        self.assertEqual(result['sdk']['state'], 'missing')

    def test_fresh_enumeration_rejects_owned_bus_without_closing_sessions(self):
        api = self.api()
        native = Native()
        bus = api.NewportBus(_native_factory=lambda: native)
        wire = api.NewportTransport(_bus=bus)
        wire.open('6700 SN1012')
        with self.assertRaisesRegex(api.InstrumentConnectionError, 'Disconnect'):
            bus.controllers()
        self.assertEqual(wire.query('*IDN?'), '6700 SN1012')
        self.assertEqual(native.close_calls, 0)
        wire.close()
        self.assertEqual(bus.controllers(), ('6700 SN1012', '6700 SN1020'))
        self.assertEqual(native.close_calls, 2)

    def test_native_enumeration_binds_serial_to_session_local_index(self):
        api = self.api()
        native = api._NativeUSB.__new__(api._NativeUSB)
        def open_devices(pid, address, count):
            self.assertEqual(pid, 4106)
            self.assertFalse(address)
            ct.cast(count, ct.POINTER(ct.c_int))[0] = 1
            return 0
        def keys(buffer):
            value = b'0,STALE CACHED IDENTITY;\0'
            ct.memmove(buffer, value, len(value))
            return 0
        replies = iter((None, b'New_Focus 6700 v2.4 03/19/14 SN1012\r\n'))
        def read(index, buffer, length, count):
            reply = next(replies)
            if reply is None: return -1
            ct.memmove(buffer, reply, len(reply))
            ct.cast(count, ct.POINTER(ct.c_ulong))[0] = 64
            return 0
        native.dll = SimpleNamespace(newp_usb_open_devices=open_devices,
            newp_usb_get_device_info=keys,newp_usb_get_ascii=read,newp_usb_send_ascii=lambda *a:0)
        self.assertEqual(native.open(), ('6700 SN1012',))

    def test_ascii_write_uses_vendor_strlen_and_mutable_packet_capacity(self):
        api = self.api()
        native = api._NativeUSB.__new__(api._NativeUSB)
        native._ids = {'6700 SN1012': 0}
        def write(key, command, length):
            self.assertEqual(key, 0)
            self.assertIsInstance(command, ct.Array)
            self.assertEqual(ct.sizeof(command), 64, 'Newport command packet must be fully backed by memory')
            self.assertEqual(command.value, b'*IDN?')
            self.assertEqual(length, 5)
            self.assertEqual(command.raw[5:], bytes(59))
            return 0
        def read(key, buffer, length, count):
            self.assertEqual(length, 64, 'Use the vendor single-packet ASCII read capacity')
            ct.memmove(buffer, b'New_Focus\r\nold reply\r\n', 22)
            ct.cast(count, ct.POINTER(ct.c_ulong))[0] = 64
            return 0
        native.dll = SimpleNamespace(newp_usb_send_ascii=write, newp_usb_get_ascii=read)
        self.assertEqual(native.query('6700 SN1012', '*IDN?'), 'New_Focus')


if __name__ == '__main__':
    unittest.main()
