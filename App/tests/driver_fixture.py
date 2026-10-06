"""Actual drivers with bounded, explicitly injected test transports.

Not a worker backend: no synthetic spectra, discovery, control model or live
instrument implementation. Unknown wire requests fail instead of guessing.
"""
from types import SimpleNamespace

from Code.Debugs.test_drivers import FakeGainSerial
from Code.Debugs.test_pm400 import FakeResourceManager, FakeVisaResource
from Code.Debugs.test_voltage_evidence import EvidenceSerial
from Code.Debugs.test_osa_read import TraceWire
from Code.Setups.fiber_coupling import FiberCouplingSetup, load_fiber_coupling_config
from Code.Debugs.test_fiber_coupling import FakeSetupMDT, make_mdt_status, FakePort
from Code.Utils.common import DriverState
from Code.Utils.gain import GainDriver
from Code.Utils.osa import AQ6370
from Code.Utils.pm400 import PM400
from Code.Utils.voltage import VoltageSource


class GainWire(FakeGainSerial):
    replies_by_request = {
        b'RDTA\r\n': b'READY;T=22.000\r\n',
        b'RDEA\r\n': b'READY;E=22.000\r\n',
        b'RDRA\r\n': b'READY;R=0\r\n',
        b'RDCA\r\n': b'READY;C=0.000\r\n',
        b'RDQA\r\n': b'READY;Q=0\r\n',
        b'STQA000000\r\n': b'READY;Q=0\r\n',
        b'STRA000000\r\n': b'READY;D=0\r\n',
    }

    def __init__(self):
        super().__init__()
        self.replies_by_request = dict(type(self).replies_by_request)
        # Finite scripts for controller tests, not a virtual instrument state.
        for value in (1, 3, 4, 5, 10, 20, 22, 50, 60, 99):
            self.replies_by_request[f'STCA{value * 1000:06d}\r\n'.encode()] = (
                f'READY;C={value:.3f}\r\n'.encode())

    def read_until(self, expected=b'\n'):
        self.read_terminators.append(expected)
        return self.replies_by_request[self.writes[-1]]


class WireGain(GainDriver):
    def __init__(self, port, **kwargs):
        self.test_wire = GainWire()
        super().__init__(port=port, serial_factory=lambda **_: self.test_wire,
                         start_watchdog=False, **kwargs)


class WireVoltage(VoltageSource):
    def __init__(self, port, **kwargs):
        self.test_wire = EvidenceSerial()
        kwargs.setdefault('io_timeout', .01)
        kwargs.setdefault('startup_timeout', .5)
        super().__init__(port=port, serial_factory=lambda **_: self.test_wire, **kwargs)


class WireOSA(AQ6370):
    def __init__(self, resource_name, **kwargs):
        self.test_wire = FakeVisaResource({'*IDN?': 'YOKOGAWA,AQ6370E,UNITTEST,1.0', '*OPC?': '1'})
        byte_wire = TraceWire()
        byte_wire.meta[':INITiate:SMODe?'] = '1'
        byte_wire.x[:] = [1550e-9, 1551e-9]
        byte_wire.y[:] = [-30., -31.]
        original_write = self.test_wire.write
        def write(command):
            byte_wire.write(command)  # Only the finite, known query/action script.
            return original_write(command)
        self.test_wire.write = write
        self.test_wire.visalib, self.test_wire.session = byte_wire, 1
        manager = FakeResourceManager(self.test_wire)
        instrument = SimpleNamespace(abort=lambda: self.test_wire.write(':ABORt'),
            initiate_sweep=lambda: self.test_wire.write(':INITiate:IMMediate'))
        super().__init__(resource_name, resource_manager_factory=lambda: manager,
                         instrument_factory=lambda _: instrument, **kwargs)


class WireMeter(PM400):
    def __init__(self, resource_name, **kwargs):
        self.test_wire = FakeVisaResource({
            '*IDN?': 'THORLABS,PM400,UNITTEST,1.0',
            'SYSTem:SENSor:IDN?': '"S130C","UNITTEST","cal",1,0,49',
            'SENSe:CORRection:WAVelength? MINimum': '200',
            'SENSe:CORRection:WAVelength? MAXimum': '2000',
            'SENSe:CORRection:WAVelength?': '780',
            'SYSTem:ERRor?': '0,"No error"',
        })
        manager = FakeResourceManager(self.test_wire)
        super().__init__(resource_name, resource_manager_factory=lambda: manager, **kwargs)


class EmptyFiber(FiberCouplingSetup):
    """Real setup with no discovered children; only used to test close routing."""
    def __init__(self):
        config = load_fiber_coupling_config()
        discovery = FiberCouplingSetup.enumerate(config, port_enumerator=lambda: ())
        super().__init__(config, discovery, {})


class MDTFacade(FakeSetupMDT):
    """Public MDT boundary for controller/setup tests; no transport is opened."""
    def __init__(self, port, *, serial='2110148249-10', **kwargs):
        super().__init__(port, status=make_mdt_status(serial))
        self.state = DriverState.DISCONNECTED
        self.is_open = False

    def connect(self):
        super().connect()
        self.state = DriverState.READY
        self.is_open = True
        return self

    def close(self):
        super().close()
        self.state = DriverState.DISCONNECTED


def fiber_factory(ports):
    """Inject explicit test USB identities into the real logical setup."""
    by_port = {port.device: port.serial_number for port in ports}
    def factory(member_serials):
        return FiberCouplingSetup.for_members(member_serials,
            port_enumerator=lambda: ports,
            driver_factory=lambda port: MDTFacade(port, serial=by_port[port]))
    return factory


def factories():
    return {'osa': WireOSA, 'gain': WireGain, 'voltage': WireVoltage,
            'pm400': WireMeter, 'mdt': MDTFacade}


RESOURCES = {'osa': 'GPIB0::29::INSTR', 'pm400': 'USB0::0x1313::0x8078::UNITTEST::INSTR',
             'gain': 'COM991', 'voltage': 'COM990'}
