"""Explicit transport injection for native Host contract tests.

Not an App backend, simulator, GUI preview, or hardware acceptance. The Host
and worker admission/lease/cleanup implementation remain real. Only finite
driver transport scripts are supplied; unknown addresses/commands fail.
"""
from functools import partial
from pathlib import Path
import shutil
import textwrap

from App.tests.driver_fixture import WireOSA, WireGain, WireVoltage, WireMeter, FakePort
from Code.Debugs.test_mdt693b import FakeMDTSerial
from Code.Setups.fiber_coupling import FiberCouplingSetup
from Code.Utils.mdt693b import MDT693B
from Code.Debugs.test_tlb6700 import Script as NewportScript
from Code.Utils.tlb6700 import TLB6700

ROOT = Path(__file__).resolve().parents[2]
OSA_IDENTITIES = {
    'GPIB0::1::INSTR': 'HOST-OSA-1',
    'GPIB0::2::INSTR': 'HOST-OSA-2',
    'GPIB0::4::INSTR': 'HOST-OSA-4',
    'GPIB0::5::INSTR': 'HOST-OSA-5',
    'GPIB0::6::INSTR': 'HOST-OSA-6',
    'GPIB0::7::INSTR': 'HOST-OSA-7',
    'GPIB0::8::INSTR': 'HOST-OSA-8',
    'GPIB0::9::INSTR': 'HOST-OSA-9',
}
PORTS = (FakePort('COM991', '2110148249-10'),
         FakePort('COM992', '160721175410'))
MDT_IDENTITIES = {port.device: port.serial_number for port in PORTS}


class HostOSA(WireOSA):
    def __init__(self, resource_name, **kwargs):
        serial = OSA_IDENTITIES[resource_name]
        super().__init__(resource_name, **kwargs)
        self.test_wire.replies['*IDN?'] = f'YOKOGAWA,AQ6370E,{serial},1.0'


class ReadonlyMDTWire(FakeMDTSerial):
    """Literal read-only transcript, no setter or motion state model."""
    def __init__(self, serial):
        super().__init__()
        values = {
            'id?': 'MDT693B,1.23', 'serial?': serial,
            'friendly?': 'Contract test', 'echo?': '0', 'vlimit?': '0',
            'intensity?': '7', 'msenable?': '0', 'msvoltage?': '0',
            'xvoltage?': '20', 'yvoltage?': '30', 'zvoltage?': '40',
            'xmin?': '0', 'ymin?': '0', 'zmin?': '0',
            'xmax?': '75', 'ymax?': '75', 'zmax?': '75',
            'dacstep?': '100', 'cm?': '0', 'rotarymode?': '0',
            'pushdisable?': '0',
        }
        self.replies = {key.encode() + b'\r\n': (value + '\r\n*\r\n').encode()
                        for key, value in values.items()}
        self.replies[b'?\r\n'] = ('\r\n'.join(('?', *values)) + '\r\n*\r\n').encode()

    def write(self, data):
        reply = self.replies[data]  # A setter/arrow/unknown query cannot succeed.
        self.writes.append(data)
        self.queue(reply)
        return len(data)


class HostMDT(MDT693B):
    def __init__(self, port, **kwargs):
        self.test_wire = ReadonlyMDTWire(MDT_IDENTITIES[port])
        super().__init__(port, serial_factory=lambda **_: self.test_wire, **kwargs)


class ReadonlyNewportWire(NewportScript):
    def query(self, command):
        if command not in self.replies:
            raise AssertionError('Setter or unknown Newport query forbidden in native Host fixture')
        return super().query(command)


class HostLaser(TLB6700):
    def __init__(self, device_key, **kwargs):
        identities = {'6700 SN1012':'1012', '6700 SN1020':'1020'}
        self.test_wire = ReadonlyNewportWire(identities[device_key], head='6722-P')
        super().__init__(device_key=device_key, _transport=self.test_wire, **kwargs)


def host_factories():
    def fiber(member_serials):
        return FiberCouplingSetup.for_members(member_serials,
            port_enumerator=lambda: PORTS, driver_factory=HostMDT)
    return {'osa': HostOSA, 'gain': WireGain, 'voltage': WireVoltage,
            'pm400': WireMeter, 'mdt': HostMDT, 'fiber': fiber, 'laser':HostLaser}


def stage_worker(directory, *, capture_fault=None, hold_inventory=False):
    """Stage production Python code, injecting transport factories only in tests."""
    if capture_fault not in (None, 'staging', 'worker_exit'):
        raise ValueError('Unknown finite delivery fault')
    if type(hold_inventory) is not bool:
        raise ValueError('Inventory hold must be an explicit test boundary')
    root = Path(directory)
    app = root / 'App'
    worker = app / 'worker'
    shutil.copytree(ROOT / 'App/worker', worker,
                    ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
    shutil.copytree(ROOT / 'App/catalog', app / 'catalog')
    (app / '__init__.py').write_text(
        f"__path__.append({str(ROOT / 'App')!r})\n", encoding='utf-8')
    (worker / 'main.py').rename(worker / '_production_main.py')
    # This entry exists in a temporary test directory only, never in packaging.
    (worker / 'main.py').write_text(textwrap.dedent(f"""
        import sys
        from functools import partial
        sys.path.append({str(ROOT)!r})
        from App.worker import controller, verification
        from App.tests.host_wire_fixture import host_factories, PORTS
        def forbidden(*args, **kwargs):
            raise AssertionError('External hardware construction forbidden in Host contract test')
        for module in (controller, verification):
            for name in ('AQ6370', 'VoltageSource', 'GainDriver', 'PM400', 'MDT693B', 'TLB6700'):
                if hasattr(module, name):
                    setattr(module, name, forbidden)
        original = controller.DomainController
        controller.DomainController = partial(original,
            factories=host_factories(), port_enumerator=lambda: PORTS)
        if {hold_inventory!r}:
            import time
            from pathlib import Path
            def finite_inventory():
                gate = Path({str(root)!r})
                (gate/'inventory-entered').write_text('entered')
                deadline = time.monotonic()+5
                while not (gate/'inventory-release').exists():
                    if time.monotonic() >= deadline:
                        raise TimeoutError('Finite inventory gate expired')
                    time.sleep(.01)
                return {{'fixture_inventory':True}}
            controller.discover = finite_inventory
        if {capture_fault!r} is not None:
            from App.worker import captures
            def fail_delivery(*args, **kwargs):
                if {capture_fault!r} == 'worker_exit':
                    import os
                    os._exit(91)  # Only this owned, finite-transport test worker.
                raise OSError('finite staging write failure')
            captures._write_file = fail_delivery
        from App.worker._production_main import main
        raise SystemExit(main())
    """), encoding='utf-8')
    return root
