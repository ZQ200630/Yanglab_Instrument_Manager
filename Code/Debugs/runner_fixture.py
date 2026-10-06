"""Narrow runner-interface fixtures; never a driver backend or hardware proof.

Reuses repeat-runner test doubles and one literal three-point observation.
No spectrum generator, discovery, connection configuration, or mode selector.
Driver interlock/ramp/transport tests use actual Code/Utils drivers separately.
"""
from dataclasses import replace
from types import SimpleNamespace

from Code.Debugs.test_sil_repeat_experiment import FakeOSA, FakeVoltage, FakeGain
from Code.Utils.voltage import VoltageStatus


class _Events:
    def __init__(self, destination):
        self.destination = destination

    def append(self, event):
        exact = {
            "osa.connect": "osa_connected", "osa.close": "osa_closed",
            "voltage.connect": "voltage_connected", "voltage.close": "voltage_closed",
            "voltage.zero.emergency": "emergency_zero", "voltage.zero": "voltage_zero",
            "gain.connect": "gain_connected", "gain.close": "gain_closed",
            "gain.enable_tec": "tec_enabled", "gain.disable_tec": "tec_disabled",
            "gain.enable_current": "current_enabled",
            "gain.disable_current": "current_disabled",
            "gain.read_temperature": "temperature_read",
        }
        prefixes = {
            "gain.set_temperature.": "temperature_set",
            "gain.wait_stable.": "stability_wait",
            "gain.ramp_current.": "current_set",
            "osa.acquire.": "spectrum_acquired",
            "voltage.set.": "voltage_step",
        }
        if event in exact:
            self.destination.append(exact[event])
        else:
            for prefix, label in prefixes.items():
                if event.startswith(prefix):
                    self.destination.append(label)
                    break


class _Voltage(FakeVoltage):
    """Tests runner target-confirmation ordering, not instrument telemetry."""
    def __init__(self, events, clock):
        super().__init__(events)
        self.clock = clock
        self.channel = 1

    def set_channel(self, channel, voltage):
        self.channel = channel
        return super().set_channel(channel, voltage)

    def read_status(self):
        values = [0.] * 8
        values[self.channel - 1] = self.current
        return VoltageStatus(tuple(values), (0.,) * 8, self.clock())

    def wait_for_status(self, **kwargs):
        return self.read_status()


class _OSA(FakeOSA):
    def acquire(self, trace="A", timeout=None):
        return super().acquire(trace=trace)


class _Gain(FakeGain):
    def __init__(self, events, clock):
        super().__init__(events)
        self.clock = clock

    def read_temperature(self):
        self.events.append("gain.read_temperature")
        self.clock.sleep(.01)
        return 22.

    def read_status(self):
        return replace(super().read_status(), temperature_c=22.,
                       received_at=self.clock())


def runner_bundle(clock, events=None):
    destination = [] if events is None else events
    sink = _Events(destination)
    return SimpleNamespace(osa=_OSA(sink), voltage=_Voltage(sink, clock),
                           gain=_Gain(sink, clock), events=destination)


def runner_factories(clock, events=None):
    from Code.Experiments.sil_hysteresis.run import DriverFactories
    bundle = runner_bundle(clock, events)
    return DriverFactories(osa=lambda **_: bundle.osa,
        voltage=lambda **_: bundle.voltage, gain=lambda **_: bundle.gain)
