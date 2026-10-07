"""Identity-bound TLB-6700 Velocity driver using Newport USB, not VISA.

Connect/probe/close preserve output, tracking, remote/local and front-panel
settings. No reset, recall, scan, auto-enable, error-queue consumption or replay.
Control actions require operator confirmation and explicit remote selection.
"""
from __future__ import annotations

import math
import re
import time
from dataclasses import dataclass
from threading import RLock

from .common import (DriverState, InstrumentConnectionError, InstrumentProtocolError,
                     InstrumentSafetyError, require_state, _identity_probe, _probe_connect_guard)
from .newport_usb import device_key
from .tlb_native_bridge import NativeLaser, NATIVE, validate_status

# Conservative, published standard-model envelopes; not a calibration query.
# Extended/custom/unknown heads remain readable. Never infer range from S/N.
# https://www.newport.com/mam/celum/celum_assets/resources/Velocity_Datasheet.pdf
RANGES = {'6712': (765.0, 781.0), '6721': (1030.0, 1070.0),
          '6722': (1045.0, 1085.0),
          '6724': (1270.0, 1330.0), '6728': (1520.0, 1570.0), '6730': (1550.0, 1630.0)}


@dataclass(frozen=True)
class LaserStatus:
    wavelength_nm: float
    wavelength_setpoint_nm: float
    power_mw: float
    power_setpoint_mw: float
    current_ma: float
    current_setpoint_ma: float
    piezo_percent: float
    output_enabled: bool
    tracking: bool
    remote: bool
    constant_power: bool
    operation_complete: bool
    status_byte: int
    received_at: float
    read_interval_s: float


class TLB6700:
    @staticmethod
    def enumerate():
        """Explicit read-only SDK discovery, which temporarily opens matching USB devices."""
        return NATIVE.enumerate()

    @staticmethod
    def discover():
        """Read actual controller/head identities in one temporary, preserving SDK session."""
        return NATIVE.discover()

    def __init__(self, *, device_key: str, _transport=None):
        self.device_key = globals()['device_key'](device_key)
        self._native = NativeLaser() if _transport is None else None
        self._transport = self._native if self._native is not None else _transport
        self._lifecycle_lock = RLock()
        self.state = DriverState.DISCONNECTED
        self.identity = None
        self.wavelength_range_nm = None
        self._responsibility = False

    @property
    def has_resource_responsibility(self):
        return self._responsibility or getattr(self._transport, 'is_open', False)

    @property
    def resources_released(self):
        return not self.has_resource_responsibility and self.state is DriverState.DISCONNECTED

    @property
    def is_open(self):
        return self.has_resource_responsibility

    def _query(self, command):
        try:
            reply = self._transport.query(command)
            if type(reply) is not str or not reply or len(reply) > 4096 or not reply.isprintable():
                raise InstrumentProtocolError('Invalid TLB response')
            return reply.strip()
        except BaseException:
            self.state = DriverState.FAULT
            raise

    def _number(self, command, *, minimum=None, maximum=None):
        reply = ''
        try:
            reply = self._query(command)
            value = float(reply)
            if not math.isfinite(value) or minimum is not None and value < minimum or maximum is not None and value > maximum:
                raise ValueError('outside bounds')
            return value
        except (ValueError, OverflowError) as error:
            self.state = DriverState.FAULT
            raise InstrumentProtocolError(f'Invalid numeric TLB response for {command}: {reply[:80]!r}') from error

    def _switch(self, command):
        reply = self._query(command).upper()
        if reply not in {'0', '1', 'OFF', 'ON'}:
            self.state = DriverState.FAULT
            raise InstrumentProtocolError('Invalid TLB switch response for ' + command)
        return reply in {'1', 'ON'}

    def _remote(self):
        reply = self._query('SYST:MCONT?').upper()
        if reply not in {'LOC', 'REM'}:
            self.state = DriverState.FAULT
            raise InstrumentProtocolError('Invalid TLB remote/local response')
        return reply == 'REM'

    def connect(self):
        with self._lifecycle_lock:
            _probe_connect_guard(self)
            require_state(self.state, {DriverState.DISCONNECTED}, 'connect')
            self.state = DriverState.CONNECTING
            try:
                if self._native is not None:
                    self._responsibility = True
                    result = self._native.connect(self.device_key)
                    self.identity = dict(result['identity'])
                    if self.identity.get('serial') != self.device_key[7:]:
                        raise InstrumentConnectionError('Native controller identity differs from selected serial')
                    bounds = result['wavelength_range_nm']
                    self.wavelength_range_nm = tuple(bounds) if bounds is not None else None
                    self.state = DriverState.READY
                    return self
                self._transport.open(self.device_key)
                self._responsibility = True
                raw = self._query('*IDN?')
                match = re.fullmatch(r'NEW[_ ]FOCUS\s+6700\s+v(\S+)\s+\S+\s*,?\s*SN(\d+)', raw, re.IGNORECASE)
                if not match or '6700 SN' + match[2] != self.device_key:
                    raise InstrumentConnectionError('Controller identity differs from the selected Newport key')
                head = self._query('SYST:LAS:MODEL?')
                serial = self._query('SYST:LAS:SN?')
                if not re.fullmatch(r'(?:TLB-)?[0-9]{4}(?:-[A-Za-z0-9]+)*', head, re.IGNORECASE) or not re.fullmatch(r'[A-Za-z0-9_-]{1,64}', serial):
                    raise InstrumentProtocolError('Invalid laser-head identity')
                self.identity = {'manufacturer': 'New Focus', 'model': 'TLB-6700',
                    'serial': match[2], 'firmware': match[1], 'head_model': head, 'head_serial': serial}
                model = re.fullmatch(r'(?:TLB-)?(\d{4})', head, re.IGNORECASE)
                self.wavelength_range_nm = RANGES.get(model[1]) if model else None
                self.state = DriverState.READY
            except BaseException as primary:
                self.state = DriverState.FAULT
                try:
                    self.close()
                except BaseException as cleanup:
                    primary.cleanup_error = cleanup
                raise
        return self

    def probe_identity(self):
        return _identity_probe(self, self.connect, lambda: (dict(self.identity),
            {'wavelength_range_nm': self.wavelength_range_nm, 'range_source': 'published standard model envelope'}))

    def read_status(self):
        with self._lifecycle_lock:
            require_state(self.state, {DriverState.READY}, 'read status')
            started = time.monotonic()
            if self._native is not None:
                def sample_native():
                    sample = validate_status(self._native.status())
                    return LaserStatus(**sample, received_at=started)
                return self._native_call(sample_native)
            output = self._switch('OUTP:STAT?')
            tracking = self._switch('OUTP:TRAC?')
            remote = self._remote()
            cp = self._switch('SOUR:CPOW?')
            wavelength = self._number('SENS:WAVE', minimum=0)
            target = self._number('SOUR:WAVE?', minimum=0)
            power = self._number('SENS:POW:DIODE')
            power_target = self._number('SOUR:POW:DIODE?', minimum=0)
            current = self._number('SENS:CURR:DIODE')
            current_target = self._number('SOUR:CURR:DIODE?', minimum=0)
            piezo = self._number('SOUR:VOLT:PIEZ?', minimum=0, maximum=100)
            complete = self._switch('*OPC?')
            stb = self._number('*STB?', minimum=0, maximum=255)
            if not stb.is_integer():
                self.state = DriverState.FAULT
                raise InstrumentProtocolError('Invalid TLB status byte')
            ended = time.monotonic()
            return LaserStatus(wavelength, target, power, power_target, current, current_target,
                piezo, output, tracking, remote, cp, complete, int(stb), started, ended - started)

    def _native_call(self, function, *args):
        try:
            return function(*args)
        except InstrumentSafetyError:
            raise
        except BaseException:
            self.state = DriverState.FAULT
            raise

    def _native_action(self, name, value, confirm):
        require_state(self.state, {DriverState.READY}, 'control laser')
        if type(confirm) is not bool:
            raise InstrumentSafetyError('Confirmation must be boolean')
        self._native_call(self._native.action, name, value, confirm)

    def _authorize(self, confirm, *, remote=True, reviewed=True, allow_busy=False):
        require_state(self.state, {DriverState.READY}, 'control laser')
        if confirm is not True:
            raise InstrumentSafetyError('Explicit operator confirmation is required')
        if reviewed and self.wavelength_range_nm is None:
            raise InstrumentSafetyError('This laser head is read-only until its control limits are reviewed')
        if remote and not self._remote():
            raise InstrumentSafetyError('Select Remote explicitly before changing laser settings')
        if not allow_busy and not self._switch('*OPC?'):
            raise InstrumentSafetyError('Controller is busy; no control command was sent')

    def _command(self, command):
        if self._query(command).upper() != 'OK':
            self.state = DriverState.FAULT
            raise InstrumentProtocolError('TLB rejected ' + command + '; no retry was attempted')

    def set_remote(self, remote, *, confirm=False):
        with self._lifecycle_lock:
            if self._native is not None:
                if type(remote) is not bool:
                    raise InstrumentSafetyError('Invalid laser control value')
                self._native_action('remote', remote, confirm)
                return
            if type(remote) is not bool:
                raise InstrumentSafetyError('Remote selection must be boolean')
            self._authorize(confirm, remote=False)
            self._command('SYST:MCONT ' + ('REM' if remote else 'LOC'))

    def set_wavelength(self, wavelength_nm, *, confirm=False):
        with self._lifecycle_lock:
            if self._native is not None:
                if type(wavelength_nm) not in {int, float} or not math.isfinite(wavelength_nm):
                    raise InstrumentSafetyError('Invalid laser control value')
                self._native_action('wavelength', wavelength_nm, confirm)
                return
            bounds = self.wavelength_range_nm
            if (bounds is None or type(wavelength_nm) not in {int, float} or not math.isfinite(wavelength_nm)
                    or not bounds[0] <= wavelength_nm <= bounds[1]):
                raise InstrumentSafetyError('Wavelength is outside the reviewed head envelope or head is unknown')
            self._authorize(confirm)
            if not self._switch('OUTP:TRAC?'):
                raise InstrumentSafetyError('Enable wavelength tracking explicitly before setting wavelength')
            self._command('SOUR:WAVE ' + format(wavelength_nm, '.12g'))

    def set_piezo(self, percent, *, confirm=False):
        with self._lifecycle_lock:
            if self._native is not None:
                if type(percent) not in {int, float} or not math.isfinite(percent):
                    raise InstrumentSafetyError('Invalid laser control value')
                self._native_action('piezo', percent, confirm)
                return
            if type(percent) not in {int, float} or not math.isfinite(percent) or not 0 <= percent <= 100:
                raise InstrumentSafetyError('Piezo setpoint must be 0–100 percent')
            self._authorize(confirm)
            self._command('SOUR:VOLT:PIEZ ' + format(percent, '.12g'))

    def set_tracking(self, enabled, *, confirm=False):
        with self._lifecycle_lock:
            if self._native is not None:
                if type(enabled) is not bool:
                    raise InstrumentSafetyError('Invalid laser control value')
                self._native_action('tracking', enabled, confirm)
                return
            if type(enabled) is not bool:
                raise InstrumentSafetyError('Tracking selection must be boolean')
            self._authorize(confirm)
            self._command('OUTP:TRAC ' + str(int(enabled)))

    def set_output(self, enabled, *, confirm=False):
        with self._lifecycle_lock:
            if self._native is not None:
                if type(enabled) is not bool:
                    raise InstrumentSafetyError('Invalid laser control value')
                self._native_action('output', enabled, confirm)
                return
            if type(enabled) is not bool:
                raise InstrumentSafetyError('Output selection must be boolean')
            self._authorize(confirm, reviewed=enabled, allow_busy=not enabled)
            self._command('OUTP:STAT ' + str(int(enabled)))
            # Firmware key/interlock and ONDELAY remain authoritative. Host readback
            # decides the reported state; successful write is not emission proof.

    def close(self):
        with self._lifecycle_lock:
            _probe_connect_guard(self)
            self.state = DriverState.CLOSING
            try:
                self._transport.close()
            except BaseException:
                self.state = DriverState.FAULT
                raise
            self._responsibility = False
            self.state = DriverState.DISCONNECTED

    def __enter__(self):
        return self.connect()

    def __exit__(self, kind, value, traceback):
        try:
            self.close()
        except BaseException:
            if value is None:
                raise
