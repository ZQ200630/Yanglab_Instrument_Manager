"""Newport 5.x WinUSB ABI, metadata discovery and process-local USB ownership.

No DLL loads or instrument opens at import time. The SDK opens every matching
PID and has a global close API: one serialized bus owns it until the last
transport releases. USB addresses are deliberately never used as identities.
"""
from __future__ import annotations

import ctypes as ct
import logging
import os
import re
import struct
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from threading import RLock

from .common import InstrumentConnectionError, InstrumentProtocolError

VID, PID = 0x104D, 0x100A
DOWNLOAD_URL = 'https://download.newport.com/#/Software/Newport_USB_Driver/'
# Matches Newport's C++ ASCII example. Larger reads can wait for additional
# packets; all supported scalar replies fit in one 64-byte packet.
MAX_DEVICES, MAX_REPLY = 32, 64


class _SDKReadFailure(InstrumentConnectionError):
    """The SDK returned its general read error (-1); no output result implied."""


def device_key(value):
    if type(value) is not str or not re.fullmatch(r'6700 SN[0-9]{1,16}', value):
        raise InstrumentConnectionError('Select the exact controller key: 6700 SN<serial>')
    return value


def validate_dll(path, *, bits=None):
    bits = bits or struct.calcsize('P') * 8
    with Path(path).open('rb') as stream:
        header = stream.read(64)
        if len(header) != 64 or header[:2] != b'MZ':
            raise InstrumentConnectionError('Newport DLL has an invalid PE header')
        offset = int.from_bytes(header[60:64], 'little')
        if offset > 16 * 1024 * 1024:
            raise InstrumentConnectionError('Newport DLL has an invalid PE offset')
        stream.seek(offset)
        pe = stream.read(6)
    if len(pe) != 6 or pe[:4] != b'PE\0\0' or int.from_bytes(pe[4:], 'little') != {32: 0x14c, 64: 0x8664}[bits]:
        raise InstrumentConnectionError(f'Newport DLL does not match the {bits}-bit Python worker')
    return Path(path).resolve()


def dll_path():
    if sys.platform != 'win32':
        raise InstrumentConnectionError('Newport USB requires Windows and the vendor WinUSB driver')
    bits = struct.calcsize('P') * 8
    folders = []
    for name in ('ProgramW6432', 'ProgramFiles', 'ProgramFiles(x86)'):
        root = os.environ.get(name)
        if root:
            base = Path(root) / 'Newport' / 'Newport USB Driver' / 'Bin'
            folders.extend((base / 'usbdll.dll', base / ('x64' if bits == 64 else 'x86') / 'usbdll.dll'))
    incompatible = []
    for candidate in dict.fromkeys(folders):
        if candidate.is_file():
            try:
                return validate_dll(candidate, bits=bits)
            except InstrumentConnectionError as error:
                incompatible.append(str(error))
    reason = '; '.join(incompatible) or 'Newport USB SDK is missing'
    raise InstrumentConnectionError(reason + '. Install the official Newport USB Driver; NI-VISA is not required.')


def sdk_status():
    try:
        path = dll_path()
        return {'state': 'ready', 'path': str(path), 'bits': struct.calcsize('P') * 8,
                'message': 'Compatible SDK file found; communication has not been tested.'}
    except (OSError, InstrumentConnectionError) as error:
        return {'state': 'missing' if 'missing' in str(error).lower() else 'unavailable', 'message': str(error)}


def windows_devices():
    """SetupAPI metadata only, including devices with no installed driver."""
    if sys.platform != 'win32':
        return []
    from ctypes import wintypes as wt

    class Info(ct.Structure):
        _fields_ = [('cbSize', wt.DWORD), ('ClassGuid', ct.c_byte * 16),
                    ('DevInst', wt.DWORD), ('Reserved', ct.c_size_t)]

    api = ct.WinDLL('setupapi', use_last_error=True)
    cm = ct.WinDLL('cfgmgr32', use_last_error=True)
    api.SetupDiGetClassDevsW.argtypes = [ct.c_void_p, wt.LPCWSTR, wt.HWND, wt.DWORD]
    api.SetupDiGetClassDevsW.restype = wt.HANDLE
    api.SetupDiEnumDeviceInfo.argtypes = [wt.HANDLE, wt.DWORD, ct.POINTER(Info)]
    api.SetupDiEnumDeviceInfo.restype = wt.BOOL
    api.SetupDiGetDeviceRegistryPropertyW.argtypes = [wt.HANDLE, ct.POINTER(Info), wt.DWORD,
        ct.POINTER(wt.DWORD), ct.c_void_p, wt.DWORD, ct.POINTER(wt.DWORD)]
    api.SetupDiGetDeviceRegistryPropertyW.restype = wt.BOOL
    api.SetupDiGetDeviceInstanceIdW.argtypes = [wt.HANDLE, ct.POINTER(Info), wt.LPWSTR, wt.DWORD, ct.POINTER(wt.DWORD)]
    api.SetupDiGetDeviceInstanceIdW.restype = wt.BOOL
    api.SetupDiDestroyDeviceInfoList.argtypes = [wt.HANDLE]
    api.SetupDiDestroyDeviceInfoList.restype = wt.BOOL
    cm.CM_Get_DevNode_Status.argtypes = [ct.POINTER(wt.ULONG), ct.POINTER(wt.ULONG), wt.DWORD, wt.ULONG]
    cm.CM_Get_DevNode_Status.restype = wt.DWORD
    handle = api.SetupDiGetClassDevsW(None, 'USB', None, 6)
    if handle == wt.HANDLE(-1).value:
        raise ct.WinError(ct.get_last_error())
    records = []
    try:
        def prop(info, number):
            buffer = ct.create_unicode_buffer(2048)
            kind, needed = wt.DWORD(), wt.DWORD()
            if not api.SetupDiGetDeviceRegistryPropertyW(handle, ct.byref(info), number, ct.byref(kind), buffer, ct.sizeof(buffer), ct.byref(needed)):
                return ''
            return buffer.value
        for index in range(4096):
            info = Info(); info.cbSize = ct.sizeof(Info)
            if not api.SetupDiEnumDeviceInfo(handle, index, ct.byref(info)):
                error = ct.get_last_error()
                if error == 259:
                    break
                raise ct.WinError(error)
            instance = ct.create_unicode_buffer(512)
            if not api.SetupDiGetDeviceInstanceIdW(handle, ct.byref(info), instance, len(instance), None):
                raise ct.WinError(ct.get_last_error())
            if 'VID_104D&PID_100A' not in instance.value.upper():
                continue
            flags, problem = wt.ULONG(), wt.ULONG()
            result = cm.CM_Get_DevNode_Status(ct.byref(flags), ct.byref(problem), info.DevInst, 0)
            records.append({'instance_id': instance.value, 'description': prop(info, 12) or prop(info, 0),
                'service': prop(info, 4), 'problem_code': int(problem.value) if result == 0 else None})
        else:
            raise InstrumentConnectionError('USB metadata enumeration capacity exceeded')
    finally:
        if not api.SetupDiDestroyDeviceInfoList(handle):
            raise ct.WinError(ct.get_last_error())
    return records


def inventory(*, _records=None, _sdk_status=None):
    records = windows_devices() if _records is None else _records
    devices = []
    for record in records:
        code, service = record.get('problem_code'), record.get('service', '')
        state = 'ready' if code == 0 and service.upper() == 'WINUSB' else 'missing' if code == 28 else 'unavailable'
        devices.append({**record, 'driver_state': state})
    return {'devices': devices, 'sdk': sdk_status() if _sdk_status is None else _sdk_status,
            'download_url': DOWNLOAD_URL,
            'install_note': 'Close instrument applications and disconnect/power off Newport USB instruments before vendor installation. Installation requires administrator consent.'}


class _NativeUSB:
    def __init__(self):
        path = dll_path()
        logging.getLogger(__name__).debug('Loading Newport SDK: %s', path)
        # Absolute, architecture-checked file; dependency lookup stays in its folder.
        with os.add_dll_directory(str(path.parent)):
            self.dll = ct.WinDLL(str(path), winmode=0x1100)
            logging.getLogger(__name__).debug('Newport SDK loaded')
        declarations = {
            'newp_usb_open_devices': ([ct.c_int, ct.c_bool, ct.POINTER(ct.c_int)], ct.c_long),
            'newp_usb_get_device_info': ([ct.c_void_p], ct.c_long),
            'newp_usb_send_ascii': ([ct.c_long, ct.c_void_p, ct.c_ulong], ct.c_long),
            'newp_usb_get_ascii': ([ct.c_long, ct.c_void_p, ct.c_ulong, ct.POINTER(ct.c_ulong)], ct.c_long),
            'newp_usb_uninit_system': ([], None),
        }
        for name, (arguments, result) in declarations.items():
            function = getattr(self.dll, name)
            function.argtypes, function.restype = arguments, result
        if logging.getLogger(__name__).isEnabledFor(logging.DEBUG):
            for name in ('newp_usb_SetLogging', 'newp_usb_SetTraceLog'):
                try:
                    function = getattr(self.dll, name)
                    function.argtypes, function.restype = [ct.c_bool], None
                    function(True)
                except AttributeError:
                    pass

    @staticmethod
    def _check(result, operation):
        if result == -1 and operation == 'read':
            raise _SDKReadFailure('Newport SDK read failed (-1); check USB connection and other applications')
        if result != 0:
            raise InstrumentConnectionError(f'Newport SDK {operation} failed ({result}); check driver binding and other applications')

    def open(self):
        count = ct.c_int()
        logging.getLogger(__name__).debug('Entering Newport open_devices (PID=%04X, index addressing)', PID)
        self._check(self.dll.newp_usb_open_devices(PID, False, ct.byref(count)), 'open')
        logging.getLogger(__name__).debug('Newport open_devices returned count=%s', count.value)
        if not 0 <= count.value <= MAX_DEVICES:
            raise InstrumentProtocolError('Newport SDK returned an invalid device count')
        info = ct.create_string_buffer(8192)
        self._check(self.dll.newp_usb_get_device_info(info), 'identity enumeration')
        logging.getLogger(__name__).debug('Newport device info: %r', info.value)
        self._ids = {}
        for index in range(count.value):
            # SDK discovery can consume an old queued reply as its *IDN?
            # response. Quiesce the input without modifying settings, then bind
            # each session-local index from a fresh controller identity only.
            self._drain(index)
            identity = self._query_index(index, '*IDN?')
            match = re.fullmatch(r'New_Focus\s+6700\s+v\S+\s+\S+\s+SN([0-9]{1,16})', identity, re.IGNORECASE)
            if not match:
                raise InstrumentProtocolError('Invalid Newport controller enumeration')
            key = device_key('6700 SN' + match[1])
            if key in self._ids:
                raise InstrumentProtocolError('Duplicate Newport controller enumeration')
            self._ids[key] = index
        values = tuple(self._ids)
        if len(values) != count.value:
            raise InstrumentProtocolError('Newport SDK returned missing or duplicate controller keys')
        return values

    def query(self, key, command):
        return self._query_index(self._ids[key], command)

    def _query_index(self, index, command):
        readonly = {'*IDN?', '*OPC?', '*STB?', 'SYST:LAS:MODEL?', 'SYST:LAS:SN?',
                    'OUTP:STAT?', 'OUTP:TRAC?', 'SYST:MCONT?', 'SOUR:CPOW?',
                    'SENS:WAVE', 'SOUR:WAVE?', 'SENS:POW:DIODE', 'SOUR:POW:DIODE?',
                    'SENS:CURR:DIODE', 'SOUR:CURR:DIODE?', 'SOUR:VOLT:PIEZ?'}
        try:
            value = self._transaction(index, command)
        except _SDKReadFailure:
            if command not in readonly:
                raise
            logging.getLogger(__name__).warning('Read-only query %s failed; synchronizing input once', command)
            self._drain(index)
            return self._transaction(index, command)
        if command in readonly and value in {'COMMAND NOT VALID', 'NO PARAMETER SPECIFIED'}:
            logging.getLogger(__name__).warning('Rejected read-only query %s; synchronizing input once', command)
            self._drain(index)
            return self._transaction(index, command)
        return value

    def _drain(self, index):
        for _ in range(16):
            buffer, read = ct.create_string_buffer(MAX_REPLY), ct.c_ulong()
            result = self.dll.newp_usb_get_ascii(index, buffer, MAX_REPLY, ct.byref(read))
            if result == -1:
                return  # SDK general error, normally its fixed 2 s empty-input timeout.
            self._check(result, 'input synchronization')
        raise InstrumentProtocolError('Newport input did not become quiescent')

    def _transaction(self, index, command):
        # Pace scalar transactions for the legacy controller firmware.
        time.sleep(0.200)
        # Follow Newport's C++ example: mutable 64-byte backing, ASCII text,
        # and strlen excluding NUL. The SDK handles the USB terminator.
        words = {'SYST':'SYSTem', 'LAS':'LASer', 'OUTP':'OUTPut', 'STAT':'STATe', 'TRAC':'TRACk',
                 'SOUR':'SOURce', 'WAVE':'WAVElength', 'CPOW':'CPOWer', 'VOLT':'VOLTage',
                 'PIEZ':'PIEZo', 'CURR':'CURRent', 'SENS':'SENSe', 'POW':'POWer', 'DIODE':'DIODe'}
        header, separator, argument = command.partition(' ')
        header = ':'.join(words.get(word.rstrip('?'), word.rstrip('?')) + ('?' if word.endswith('?') else '')
                          for word in header.split(':'))
        encoded = (header + separator + argument).encode('ascii')
        if len(encoded) >= MAX_REPLY:
            raise InstrumentProtocolError('Newport scalar command exceeds the 64-byte packet')
        writable = ct.create_string_buffer(encoded, MAX_REPLY)
        self._check(self.dll.newp_usb_send_ascii(index, writable, len(encoded)), 'write')
        buffer, count = ct.create_string_buffer(MAX_REPLY), ct.c_ulong()
        self._check(self.dll.newp_usb_get_ascii(index, buffer, MAX_REPLY, ct.byref(count)), 'read')
        logging.getLogger(__name__).debug('Newport %s read count=%s, packet=%r', command, count.value, buffer.raw)
        if not 0 < count.value <= MAX_REPLY:
            raise InstrumentProtocolError('Newport SDK response is empty or exceeds the reply capacity')
        try:
            # Firmware 2.4 returns all 64 bytes, leaving bytes from the previous
            # longer reply behind the first CRLF. They are packet padding, not
            # another response. Bound scalar exchanges to this one packet.
            raw = buffer.raw[:count.value]
            logging.getLogger(__name__).debug('Newport %s reply bytes: %r', command, raw)
            frame, terminator, padding = raw.partition(b'\r\n')
            if not terminator:
                raise InstrumentProtocolError('TLB response is not terminated by CRLF')
            value = frame.decode('ascii')
        except UnicodeError as error:
            raise InstrumentProtocolError('Newport response is not ASCII') from error
        if not value or '\r' in value or '\n' in value or not value.isprintable():
            raise InstrumentProtocolError('Newport response contains an invalid frame')
        return value

    def close(self):
        self.dll.newp_usb_uninit_system()


class NewportBus:
    def __init__(self, *, _native_factory=_NativeUSB):
        self._factory, self._native = _native_factory, None
        self._lock, self._claims, self._keys = RLock(), {}, ()
        self._retained = False
        self._executor = None

    def _call(self, function, *args):
        # Host connection, action and observation lanes use different threads.
        # Keep this global-state vendor SDK's complete lifetime on one thread.
        if self._executor is None:
            self._executor = ThreadPoolExecutor(max_workers=1, thread_name_prefix='NewportUSB')
        return self._executor.submit(function, *args).result()

    def _stop_executor(self):
        if self._executor is not None:
            self._executor.shutdown(wait=True)
            self._executor = None

    def controllers(self):
        """Explicit active identity enumeration; never part of metadata inventory."""
        with self._lock:
            if self._retained:
                raise InstrumentConnectionError('Newport enumeration cleanup remains retained; restart the owner after checking it')
            if self._native is not None:
                return self._keys
            native = None
            try:
                native = self._call(self._factory)
                return self._call(native.open)
            finally:
                try:
                    if native is not None:
                        self._call(native.close)
                except BaseException:
                    self._native, self._retained = native, True
                    raise
                else:
                    self._stop_executor()

    def acquire(self, key, owner):
        device_key(key)
        with self._lock:
            if self._retained:
                raise InstrumentConnectionError('Newport SDK cleanup remains unresolved')
            if key in self._claims:
                raise InstrumentConnectionError('This Newport controller already has a session')
            if self._native is None:
                try:
                    self._native = self._call(self._factory)
                except BaseException:
                    self._stop_executor()
                    raise
                try:
                    self._keys = self._call(self._native.open)
                except BaseException:
                    try:
                        self._call(self._native.close)
                        self._native = None
                        self._stop_executor()
                    except BaseException:
                        self._retained = True
                        self._claims[key] = owner
                    raise
            if key not in self._keys:
                if not self._claims:
                    try:
                        self._call(self._native.close)
                        self._native = None
                        self._stop_executor()
                    except BaseException:
                        self._claims[key] = owner
                        self._retained = True
                        raise
                raise InstrumentConnectionError('Selected controller was not found; verify its exact serial key')
            self._claims[key] = owner

    def query(self, key, owner, command):
        with self._lock:
            if self._claims.get(key) is not owner or self._retained:
                raise InstrumentConnectionError('Newport transport has no current ownership')
            if type(command) is not str or not command or len(command) > 256 or any(ord(c) < 32 or ord(c) > 126 for c in command):
                raise InstrumentProtocolError('Invalid Newport command framing')
            return self._call(self._native.query, key, command)

    def owns(self, key, owner):
        with self._lock:
            return self._claims.get(key) is owner

    def release(self, key, owner):
        with self._lock:
            if self._claims.get(key) is not owner:
                return
            if len(self._claims) == 1:
                try:
                    self._call(self._native.close)
                except BaseException:
                    self._retained = True
                    raise
                self._native, self._keys, self._retained = None, (), False
                self._stop_executor()
            del self._claims[key]


_BUS = NewportBus()


class NewportTransport:
    def __init__(self, *, _bus=_BUS):
        self._bus, self._key, self._owner = _bus, None, object()

    def open(self, key):
        key = device_key(key)
        if self._key is not None and self.is_open:
            raise InstrumentConnectionError('Transport already owns a Newport controller')
        self._key = key
        self._bus.acquire(key, self._owner)

    @property
    def is_open(self):
        return self._key is not None and self._bus.owns(self._key, self._owner)

    def query(self, command):
        return self._bus.query(self._key, self._owner, command)

    def close(self):
        self._bus.release(self._key, self._owner)
        self._key = None
