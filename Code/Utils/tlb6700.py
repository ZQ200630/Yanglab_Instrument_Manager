"""Identity-bound TLB-6700 Velocity driver using Newport USB, not VISA.

Connect/probe/close preserve output, tracking, remote/local and front-panel
settings. No reset, recall, auto-enable, error-queue consumption or replay.
Typed composite controls temporarily take Remote and return the front panel after ACK.
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
from .tlb_native_bridge import NativeLaser, NATIVE, validate_status, validate_motion
from .tlb_models import RANGES, head_spec

# Conservative, published standard-model envelopes; not a calibration query.
# Extended/custom/unknown heads remain readable. Never infer range from S/N.
# https://www.newport.com/mam/celum/celum_assets/resources/Velocity_Datasheet.pdf


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

    def __init__(self, *, device_key: str, control_limits=None, _transport=None):
        self.device_key = globals()['device_key'](device_key)
        self._native = NativeLaser() if _transport is None else None
        self._transport = self._native if self._native is not None else _transport
        self._lifecycle_lock = RLock()
        self.state = DriverState.DISCONNECTED
        self.identity = None
        self.wavelength_range_nm = None
        self.control_limits = dict(control_limits) if control_limits is not None else None
        self._responsibility = False

    @property
    def max_scan_speed_nm_s(self):
        spec=head_spec(self.identity['head_model']) if self.identity else None
        return spec[2] if spec else None

    @property
    def operating_range_nm(self):
        if self.wavelength_range_nm is None:return None
        if self.control_limits is None:return self.wavelength_range_nm
        return (max(self.wavelength_range_nm[0],self.control_limits['min_nm']),
                min(self.wavelength_range_nm[1],self.control_limits['max_nm']))

    @property
    def operating_max_speed_nm_s(self):
        if self.max_scan_speed_nm_s is None:return None
        return min(self.max_scan_speed_nm_s,self.control_limits['max_speed_nm_s']) if self.control_limits else self.max_scan_speed_nm_s

    def _apply_limits(self):
        if self.control_limits is None:return
        values=self.control_limits
        if (set(values)!={'min_nm','max_nm','max_speed_nm_s'} or
            any(type(v) not in {int,float} or not math.isfinite(v) for v in values.values()) or
            self.wavelength_range_nm is None or not self.wavelength_range_nm[0]<=values['min_nm']<values['max_nm']<=self.wavelength_range_nm[1] or
            not 0.01<=values['max_speed_nm_s']<=self.max_scan_speed_nm_s):
            raise InstrumentSafetyError('Operating limits can only narrow the hardware envelope')
        if self._native is not None:self._native_call(self._native.set_limits,values)

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
                    self._apply_limits()
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
                spec = head_spec(head)
                self.wavelength_range_nm = spec[:2] if spec else None
                self._apply_limits()
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

    def read_motion(self):
        """Motion readback with completion checks bracketing the wavelength."""
        with self._lifecycle_lock:
            require_state(self.state,{DriverState.READY},'read motion')
            started=time.monotonic()
            if self._native is not None:
                sample=self._native_call(lambda:validate_motion(self._native.motion()))
            else:
                was_complete=self._switch('*OPC?')
                sample=dict(wavelength_nm=self._number('SENS:WAVE',minimum=0),
                    wavelength_setpoint_nm=self._number('SOUR:WAVE?',minimum=0),
                    tracking=self._switch('OUTP:TRAC?'))
                still_complete=self._switch('*OPC?')
                sample.update(operation_complete=was_complete and still_complete,read_interval_s=time.monotonic()-started)
            return dict(sample,received_at=started)
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
            bounds = self.operating_range_nm
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

    def _control(self, name, value=None, *, confirm=False):
        """Typed composite; never use finally to issue writes after an uncertain action."""
        with self._lifecycle_lock:
            require_state(self.state,{DriverState.READY},'control laser')
            if confirm is not True:raise InstrumentSafetyError('Explicit operator confirmation is required')
            stopping=name=='scan_stop' or name in {'output','tracking'} and value is False
            bounds=self.operating_range_nm
            def inside(v):
                return type(v) in {int,float} and math.isfinite(v) and bounds is not None and bounds[0]<=v<=bounds[1]
            if bounds is None and not stopping:raise InstrumentSafetyError('Unknown laser-head control limits')
            if name in {'output','tracking'} and type(value) is not bool:raise InstrumentSafetyError('Control selection must be boolean')
            if name in {'wavelength','target'} and not inside(value):raise InstrumentSafetyError('Wavelength is outside this head range')
            if name=='piezo' and (type(value) not in {int,float} or not math.isfinite(value) or not 0<=value<=100):
                raise InstrumentSafetyError('Piezo must be 0–100 percent')
            if name=='scan_start':
                start,stop,speed=value['start_nm'],value['stop_nm'],value['speed_nm_s']
                backward=value.get('return_speed_nm_s')
                if backward is not None and (type(backward) not in {int,float} or not math.isfinite(backward) or not 0.01<=backward<=self.operating_max_speed_nm_s):
                    raise InstrumentSafetyError('Backward velocity exceeds operating limits')
                if (not inside(start) or not inside(stop) or abs(start-stop)<0.009999 or
                    type(speed) not in {int,float} or not math.isfinite(speed) or not 0.01<=speed<=self.operating_max_speed_nm_s):
                    raise InstrumentSafetyError('Scan wavelengths or speed are outside this head limits')
            if self._native is not None:
                self._native_call(self._native.control,name,value,confirm)
                return
            self._authorize(confirm,remote=False,reviewed=not stopping,allow_busy=stopping)
            remote=self._remote()
            tracking=self._switch('OUTP:TRAC?') if name=='wavelength' else True
            if name=='scan_start':
                actual=self._number('SOUR:WAVE:MAXVEL?',minimum=0.01)
                if speed>actual or backward is not None and backward>actual:raise InstrumentSafetyError('Scan speed exceeds the controller maximum')
                cap=min(actual,self.operating_max_speed_nm_s)
                backward=cap if backward is None else backward
            if not remote:self._command('SYST:MCONT REM')
            scalar=lambda v:format(v,'.12g')
            if name=='wavelength':
                if not tracking:self._command('OUTP:TRAC 1')
                self._command('SOUR:WAVE '+scalar(value))
            elif name=='target':self._command('SOUR:WAVE '+scalar(value))
            elif name=='piezo':self._command('SOUR:VOLT:PIEZ '+scalar(value))
            elif name in {'output','tracking'}:self._command(('OUTP:STAT ' if name=='output' else 'OUTP:TRAC ')+str(int(value)))
            elif name=='scan_stop':self._command('OUTP:SCAN:STOP')
            elif name=='scan_start':
                settings=[('SOUR:WAVE:START',scalar(start)),('SOUR:WAVE:STOP',scalar(stop)),
                    ('SOUR:WAVE:SLEW:FORW',scalar(speed)),('SOUR:WAVE:SLEW:RET',scalar(backward)),
                    ('SOUR:WAVE:DESSCANS','1')]
                for command,arg in settings:self._command(command+' '+arg)
                a=self._number('SOUR:WAVE:START?');b=self._number('SOUR:WAVE:STOP?')
                forward=self._number('SOUR:WAVE:SLEW:FORW?',minimum=0.01,maximum=cap)
                ret=self._number('SOUR:WAVE:SLEW:RET?',minimum=0.01,maximum=cap)
                cycles=self._number('SOUR:WAVE:DESSCANS?',minimum=1,maximum=9999)
                if (not inside(a) or not inside(b) or abs(a-start)>0.005001 or abs(b-stop)>0.005001 or
                    abs(a-b)<0.009999 or abs(forward-speed)>0.000001 or abs(ret-backward)>0.000001 or cycles!=1):
                    self.state=DriverState.FAULT
                    raise InstrumentProtocolError('Scan setting verification failed; scanning was not started')
                self._command('OUTP:SCAN:START')
            else:raise InstrumentSafetyError('Unsupported typed laser control')
            self._command('SYST:MCONT LOC')

    def set_target_wavelength(self,wavelength_nm,*,confirm=False):self._control('target',wavelength_nm,confirm=confirm)
    def move_wavelength(self,wavelength_nm,*,confirm=False):self._control('wavelength',wavelength_nm,confirm=confirm)
    def control_piezo(self,percent,*,confirm=False):self._control('piezo',percent,confirm=confirm)
    def control_tracking(self,enabled,*,confirm=False):self._control('tracking',enabled,confirm=confirm)
    def control_output(self,enabled,*,confirm=False):self._control('output',enabled,confirm=confirm)
    def start_scan(self,start_nm,stop_nm,speed_nm_s,*,return_speed_nm_s=None,confirm=False):
        self._control('scan_start',{'start_nm':start_nm,'stop_nm':stop_nm,'speed_nm_s':speed_nm_s,'return_speed_nm_s':return_speed_nm_s},confirm=confirm)
    def stop_scan(self,*,confirm=False):self._control('scan_stop',confirm=confirm)

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
