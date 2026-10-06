"""Behavioral tests for instrument ownership and the console action boundary."""

from __future__ import annotations

import unittest
import time
import threading
import subprocess
import sys
import textwrap
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch
from datetime import datetime, timezone
from types import SimpleNamespace

from App.worker.controller import ConsoleController, ConsoleError, _json_value
from App.worker.contracts import Context, Request, encode_v2
from App.tests.driver_fixture import factories as wire_factories, fiber_factory
from Code.Debugs.test_fiber_coupling import FakePort
from Code.Debugs import test_drivers as driver_tests
from App.worker.discovery import discover
from Code.Utils import MeasurementKind
from Code.Setups.fiber_coupling import LogicalAxis, StageSide, StageMotionLimitError


class FakeVoltage:
    instances: list[FakeVoltage] = []

    def __init__(self, port: str):
        self.port = port
        self.state = SimpleNamespace(name="DISCONNECTED")
        self.connected = False
        self.closed = False
        self.commands: list[tuple[int, float]] = []
        self.zero_evidence = SimpleNamespace(state=SimpleNamespace(value="unknown"))
        self.instances.append(self)

    def connect(self):
        self.connected = True
        self.state = SimpleNamespace(name="READY")
        return self

    def read_status(self):
        return SimpleNamespace(voltage_v=(0.0,) * 8, current_ma=(0.0,) * 8,
                               received_at=10.0)

    def set_channel(self, channel: int, voltage: float):
        self.commands.append((channel, voltage))

    def zero(self, emergency: bool = False):
        self.commands.append((0, 0.0))

    def close(self):
        self.closed = True
        self.state = SimpleNamespace(name="DISCONNECTED")


class FakeVisa:
    instances: list[FakeVisa] = []

    def __init__(self, resource_name: str):
        self.resource_name = resource_name
        self.state = SimpleNamespace(name="DISCONNECTED")
        self.identity = "YOKOGAWA,AQ6370E,TEST,1.0"
        self.instances.append(self)

    def connect(self):
        self.state = SimpleNamespace(name="READY")
        return self

    def close(self):
        self.state = SimpleNamespace(name="DISCONNECTED")


class ControllerTests(unittest.TestCase):
    def setUp(self) -> None:
        FakeVoltage.instances.clear()
        FakeVisa.instances.clear()
        real_controller = ConsoleController
        def tracked_controller(**kwargs):
            kwargs.setdefault("port_enumerator", lambda: ())
            controller = real_controller(**kwargs)
            self.addCleanup(controller.close)
            return controller
        self.controller_patch = patch(__name__ + ".ConsoleController", tracked_controller)
        self.controller_patch.start()
        self.addCleanup(self.controller_patch.stop)

    def test_direct_malformed_submit_preserves_terminal_pre_call_rejection(self):
        calls = []
        def forbidden_factory(*args, **kwargs):
            calls.append((args, kwargs))
            raise AssertionError("malformed input reached a driver factory")
        controller = ConsoleController(factories={
            role: forbidden_factory for role in ("osa", "gain", "voltage", "pm400", "fiber")
        })
        context = controller.context("gain")
        requests = (
            None,
            Request([], "status", {}, None),
            Request("", "connect", {"role": "gain"}, context),
            Request("bad-ctx", "action", {"role": "gain", "name": "x"}, {}),
            Request("bad-epoch", "status", {}, Context(context.session_id, None, True)),
        )
        for request in requests:
            with self.subTest(request=request):
                future = controller.submit(request)
                self.assertTrue(future.done(), "pre-call rejection must publish a terminal Future")
                outcome = future.result(timeout=1)
                self.assertEqual(outcome.phase, "rejected_before_call")
                self.assertIsNone(outcome.result)
                self.assertIsNotNone(outcome.error)
                self.assertEqual(outcome.context, Context(context.session_id, None, 0))
                encode_v2("rejection-evidence", outcome)
        self.assertEqual(calls, [])
        self.assertEqual(controller.cached_status()["devices"], {})

    def test_concurrent_canonical_alias_is_reserved_before_second_factory(self):
        for first_role, second_role, first_resource, second_resource, fake in (
            ("voltage", "gain", "COM4", r"\\.\COM4", FakeVoltage),
            ("osa", "pm400", "GPIB::4::INSTR", "GPIB0::4::INSTR", FakeVisa),
        ):
            with self.subTest(role=first_role):
                entered, release = threading.Event(), threading.Event()
                class Slow(fake):
                    def connect(self):
                        entered.set()
                        if not release.wait(4):
                            raise TimeoutError("test release not received")
                        return super().connect()
                constructions = []
                def forbidden(**kwargs):
                    constructions.append(kwargs)
                    return fake(**kwargs)
                controller = ConsoleController(factories={first_role: Slow, second_role: forbidden})
                with ThreadPoolExecutor(max_workers=2) as pool:
                    try:
                        first = pool.submit(controller.handle, "connect", {"role": first_role,
                            "resource": first_resource, "acknowledge_lifecycle": True})
                        self.assertTrue(entered.wait(2))
                        second = pool.submit(controller.handle, "connect", {"role": second_role,
                            "resource": second_resource, "acknowledge_lifecycle": True})
                        with self.assertRaises(ConsoleError):
                            second.result(1)
                        self.assertFalse(first.done())
                        self.assertEqual(constructions, [])
                    finally:
                        release.set()
                        pool.shutdown(wait=True)
                        controller.close()

    def test_stop_during_connect_never_publishes_ready(self):
        controller = ConsoleController(factories=wire_factories())
        self.assertTrue(callable(getattr(controller, "submit", None)), "controller must use scheduler")
        entered, release = threading.Event(), threading.Event()
        class Slow(FakeVisa):
            def connect(self):
                entered.set()
                if not release.wait(4):
                    raise TimeoutError("test release not received")
                return super().connect()
        controller = ConsoleController(factories={"osa": Slow})
        try:
            connect = controller.submit(Request("connect", "connect", {"role": "osa",
                "resource": "GPIB0::4::INSTR", "acknowledge_lifecycle": True}, controller.context("osa")))
            self.assertTrue(entered.wait(2))
            stop = controller.submit(Request("stop", "disconnect", {"role": "osa"}, controller.context("osa")))
            self.assertEqual(controller.cached_status()["roles"]["osa"]["state"], "CLOSING")
            release.set()
            self.assertEqual(connect.result(2).phase, "completed_readback_failed")
            self.assertEqual(stop.result(2).phase, "completed")
            self.assertNotIn("osa", controller.cached_status()["devices"])
        finally:
            release.set()
            controller.close()

    def test_observation_runs_off_caller_and_status_never_reads_driver(self):
        names = []
        class ReadThreads(FakeVoltage):
            def read_status(self):
                names.append(threading.current_thread().name)
                return super().read_status()
        controller = ConsoleController(factories={"voltage": ReadThreads})
        controller.handle("connect", {"role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True})
        self.assertTrue(names)
        self.assertTrue(all(name.startswith("observe-") for name in names), names)
        before = len(names)
        for _ in range(5):
            controller.handle("status", {})
        self.assertEqual(len(names), before)

    def test_factory_failure_retains_unknown_reservation_without_fake_device(self):
        # This deliberately unrecoverable fake has no object with a public close.
        # Isolate it so retained production workers need no unsafe escape hatch.
        script = textwrap.dedent('''
            import os, traceback
            from App.worker.controller import ConsoleController, ConsoleError
            constructed = []
            def fail(**kwargs):
                raise OSError('factory raised after resource allocation')
            def second(**kwargs):
                constructed.append(True)
                raise AssertionError('second factory must not enter')
            code = 1
            try:
                c = ConsoleController(factories={'voltage': fail, 'gain': second}, port_enumerator=lambda: ())
                for role in ('voltage', 'gain'):
                    try:
                        c.handle('connect', {'role': role, 'resource': 'COM4', 'acknowledge_lifecycle': True})
                    except ConsoleError:
                        pass
                    else:
                        raise AssertionError('connection must fail')
                assert constructed == []
                assert 'voltage' not in c._devices
                assert c.close()['unreleased'] == ['voltage']
                assert not c._scheduler.join(0.05)
                code = 0
            except BaseException:
                traceback.print_exc()
            finally:
                os._exit(code)  # test-only process contains no real I/O
        ''')
        child = subprocess.run([sys.executable, '-B', '-c', script], capture_output=True, text=True, timeout=10)
        self.assertEqual(child.returncode, 0, child.stderr)

    def test_visa_alias_resolver_reserves_canonical_resource_before_factory(self):
        class Manager:
            def resource_info(self, name):
                return SimpleNamespace(resource_name="GPIB0::4::INSTR" if name == "scope" else name)
        manager = Manager()
        controller = ConsoleController(factories={"osa": FakeVisa, "pm400": FakeVisa},
            visa_resource_resolver=lambda name: manager.resource_info(name).resource_name)
        controller.handle("connect", {"role": "osa", "resource": "scope", "acknowledge_lifecycle": True})
        with self.assertRaisesRegex(ConsoleError, "VISA resource"):
            controller.handle("connect", {"role": "pm400", "resource": "GPIB::4::INSTR"})
        self.assertEqual(len(FakeVisa.instances), 1)

    def test_visa_serial_component_case_is_not_collapsed(self):
        controller = ConsoleController(factories={"osa": FakeVisa, "pm400": FakeVisa})
        controller.handle("connect", {"role": "osa", "resource": "USB0::0x1313::0x8078::abc::INSTR",
                                      "acknowledge_lifecycle": True})
        controller.handle("connect", {"role": "pm400", "resource": "USB0::0x1313::0x8078::ABC::INSTR"})
        self.assertEqual(len(FakeVisa.instances), 2)

    def test_global_cleanup_does_not_erase_contradictory_release_evidence(self):
        class Contradictory(FakeVisa):
            resources_released = False
            is_open = False
        controller = ConsoleController(factories={"osa": Contradictory})
        controller.handle("connect", {"role": "osa", "resource": "GPIB0::4::INSTR",
                                      "acknowledge_lifecycle": True})
        try:
            first = controller.close()
            self.assertEqual(first["unreleased"], ["osa"])
            self.assertIn("osa", controller._devices)
            self.assertIn("osa", controller._resources)
            first["unreleased"].clear()
            self.assertEqual(controller.close()["unreleased"], ["osa"])
        finally:
            Contradictory.instances[-1].resources_released = True
            controller.close()

    def test_completed_cleanup_does_not_report_released_device_as_owned(self):
        controller = ConsoleController(factories=wire_factories())
        controller.handle("connect", {"role": "osa", "resource": "GPIB0::29::INSTR", "acknowledge_lifecycle": True})
        self.assertEqual(controller.close()["unreleased"], [])
        self.assertEqual(controller.handle("status", {})["devices"], {})
        self.assertFalse(controller.handle("ping", {})["connected"])

    def test_independent_connects_enter_before_first_connect_returns(self):
        entered, release, second = threading.Event(), threading.Event(), threading.Event()
        class SlowOSA(FakeVisa):
            def connect(self):
                entered.set()
                if not release.wait(4):
                    raise TimeoutError("test release not received")
                return super().connect()
        class FastVoltage(FakeVoltage):
            def connect(self):
                second.set()
                return super().connect()
        controller = ConsoleController(factories={"osa": SlowOSA, "voltage": FastVoltage}, port_enumerator=lambda: ())
        with ThreadPoolExecutor(max_workers=2) as pool:
            try:
                first = pool.submit(controller.handle, "connect", {
                    "role": "osa", "resource": "GPIB0::4::INSTR", "acknowledge_lifecycle": True})
                self.assertTrue(entered.wait(2))
                next_call = pool.submit(controller.handle, "connect", {
                    "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True})
                self.assertTrue(second.wait(0.5), "independent connect blocked by controller ownership lock")
                self.assertTrue(next_call.result(2)["connected"])
                self.assertFalse(first.done())
            finally:
                release.set()
                pool.shutdown(wait=True)
                controller.close()

    def test_startup_does_not_construct_or_connect_devices(self) -> None:
        controller = ConsoleController(
            factories={"voltage": FakeVoltage, "osa": FakeVisa},
        )
        startup = controller.handle("status", {})
        self.assertEqual(startup["devices"], {})
        self.assertIn("observed_at", startup)
        self.assertEqual(FakeVoltage.instances, [])
        self.assertEqual(FakeVisa.instances, [])

    def test_ping_reports_whether_worker_still_owns_a_device(self) -> None:
        controller = ConsoleController(factories=wire_factories())
        try:
            self.assertIs(controller.handle("ping", {})["connected"], False)
            controller.handle("connect", {
                "role": "osa", "resource": "GPIB0::29::INSTR", "acknowledge_lifecycle": True,
            })
            self.assertIs(controller.handle("ping", {})["connected"], True)
            controller.handle("disconnect", {"role": "osa"})
            self.assertIs(controller.handle("ping", {})["connected"], False)
        finally:
            controller.close()

    def test_inventory_uses_enumeration_without_constructing_devices(self) -> None:
        controller = ConsoleController(factories=wire_factories())
        class Manager:
            def list_resources(self):
                return ('GPIB0::29::INSTR',)
            def open_resource(self, *args):
                raise AssertionError('Inventory must never open an instrument')
            def close(self):
                pass
        inventory_read = lambda: discover(port_enumerator=lambda: (),
                                          resource_manager_factory=Manager)
        with patch('App.worker.controller.discover', side_effect=inventory_read):
            inventory = controller.handle('inventory', {})
        self.assertEqual(inventory['visa'], ['GPIB0::29::INSTR'])
        self.assertEqual(inventory['serial'], [])
        self.assertEqual(inventory['suggestions'], {'voltage': [], 'gain': []})
        self.assertIsNone(inventory['fiber']['left'])
        self.assertIsNone(inventory['fiber']['right'])
        self.assertEqual(controller.cached_status()['devices'], {})

    def test_voltage_connect_requires_lifecycle_acknowledgement(self) -> None:
        controller = ConsoleController(factories={"voltage": FakeVoltage})
        with self.assertRaises(ConsoleError):
            controller.handle("connect", {"role": "voltage", "resource": "COM4"})
        self.assertEqual(FakeVoltage.instances, [])
        connected = controller.handle("connect", {
            "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
        })
        self.assertTrue(connected["connected"])
        self.assertEqual(connected["status"]["voltage_v"], [0.0] * 8)
        controller.handle("action", {
            "role": "voltage", "name": "set_channel", "channel": 1, "voltage": 1.2,
        })
        self.assertEqual(FakeVoltage.instances[0].commands, [(1, 1.2)])
        controller.handle("disconnect", {"role": "voltage"})
        self.assertTrue(FakeVoltage.instances[0].closed)

    def test_voltage_requested_command_is_separate_from_observed_telemetry(self) -> None:
        controller = ConsoleController(factories={"voltage": FakeVoltage})
        connected = controller.handle("connect", {
            "role": "voltage", "resource": "COM990", "acknowledge_lifecycle": True,
        })["status"]
        self.assertEqual(connected["requested_voltage_v"], [0.0] * 8)
        changed = controller.handle("action", {
            "role": "voltage", "name": "set_channel", "channel": 2, "voltage": 1.25,
        })["status"]
        self.assertEqual(changed["requested_voltage_v"][1], 1.25)
        self.assertEqual(changed["voltage_v"][1], 0.0)
        self.assertEqual(controller.handle("status", {})["devices"]["voltage"]["requested_voltage_v"][1], 1.25)
        zero = controller.handle("action", {"role": "voltage", "name": "zero"})["status"]
        self.assertEqual(zero["requested_voltage_v"], [0.0] * 8)
        controller.close()

    def test_voltage_failed_command_marks_requested_target_unknown(self) -> None:
        class PartialVoltage(FakeVoltage):
            def set_channel(self, channel: int, voltage: float):
                super().set_channel(channel, voltage)
                raise RuntimeError("interrupted after partial write")

        controller = ConsoleController(factories={"voltage": PartialVoltage})
        controller.handle("connect", {
            "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "interrupted after partial write"):
            controller.handle("action", {
                "role": "voltage", "name": "set_channel", "channel": 1, "voltage": 1.2,
            })
        status = controller.handle("status", {})["devices"]["voltage"]
        self.assertIsNone(status["requested_voltage_v"])
        self.assertEqual(status["voltage_v"], [0.0] * 8,
                         "last host sample must not be inferred from an attempted write")
        with self.assertRaisesRegex(ConsoleError, "not ready"):
            controller.handle("action", {
                "role": "voltage", "name": "set_channel", "channel": 2, "voltage": 0.5,
            })
        self.assertEqual(PartialVoltage.instances[-1].commands, [(1, 1.2)])
        controller.close()

    def test_driver_value_error_after_write_reports_unknown_outcome(self) -> None:
        class PartialVoltage(FakeVoltage):
            def set_channel(self, channel: int, voltage: float):
                super().set_channel(channel, voltage)
                raise ValueError("reply checksum failed after write")

        controller = ConsoleController(factories={"voltage": PartialVoltage},
                                       port_enumerator=lambda: ())
        controller.handle("connect", {
            "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "outcome is unknown"):
            controller.handle("action", {
                "role": "voltage", "name": "set_channel", "channel": 1, "voltage": 1.2,
            })
        self.assertEqual(PartialVoltage.instances[-1].commands, [(1, 1.2)])
        self.assertIsNone(controller.handle("status", {})["devices"]["voltage"]["requested_voltage_v"])
        controller.close()

    def test_osa_value_error_after_acquisition_reports_unknown_outcome(self) -> None:
        class PartialOSA(FakeVisa):
            def acquire(self, trace: str = "A"):
                self.acquisitions = getattr(self, "acquisitions", 0) + 1
                raise ValueError("trace reply malformed after sweep")

        controller = ConsoleController(factories={"osa": PartialOSA})
        controller.handle("connect", {
            "role": "osa", "resource": "GPIB0::4::INSTR", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "outcome is unknown"):
            controller.handle("action", {"role": "osa", "name": "acquire"})
        self.assertEqual(PartialOSA.instances[-1].acquisitions, 1)
        controller.close()

    def test_missing_voltage_parameter_is_invalid_before_driver_call(self) -> None:
        controller = ConsoleController(factories={"voltage": FakeVoltage},
                                       port_enumerator=lambda: ())
        controller.handle("connect", {
            "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "invalid voltage action"):
            controller.handle("action", {"role": "voltage", "name": "set_channel",
                                         "channel": 1})
        self.assertEqual(FakeVoltage.instances[-1].commands, [])
        controller.close()

    def test_completed_voltage_command_with_failed_status_readback_is_not_called_unexecuted(self) -> None:
        class ReadbackFails(FakeVoltage):
            def read_status(self):
                if self.commands:
                    raise OSError("telemetry unavailable")
                return super().read_status()

        controller = ConsoleController(factories={"voltage": ReadbackFails},
                                       port_enumerator=lambda: ())
        controller.handle("connect", {
            "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "command completed.*status readback failed"):
            controller.handle("action", {
                "role": "voltage", "name": "set_channel", "channel": 1, "voltage": 1.2,
            })
        self.assertEqual(ReadbackFails.instances[-1].commands, [(1, 1.2)])
        self.assertIn("telemetry unavailable",
                      controller.handle("status", {})["devices"]["voltage"]["status_error"])
        controller.close()

    def test_connected_voltage_with_failed_first_readback_is_retained_for_cleanup(self) -> None:
        class NoReadback(FakeVoltage):
            def read_status(self):
                raise OSError("first telemetry unavailable")

        controller = ConsoleController(factories={"voltage": NoReadback},
                                       port_enumerator=lambda: ())
        with self.assertRaisesRegex(ConsoleError, "connection established.*status readback failed"):
            controller.handle("connect", {
                "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
            })
        self.assertEqual(controller.handle("status", {})["devices"]["voltage"]["resource"],
                         "COM4")
        report = controller.close()
        self.assertTrue(NoReadback.instances[-1].closed)
        self.assertEqual(report["unreleased"], [])

    def test_completed_osa_acquisition_with_invalid_result_reports_serialization_not_readback(self) -> None:
        class InvalidTrace(FakeVisa):
            acquisitions = 0

            def acquire(self, trace: str = "A"):
                self.acquisitions += 1
                return {"trace": trace, "power_dbm": [float("nan")]}

        controller = ConsoleController(factories={"osa": InvalidTrace})
        controller.handle("connect", {
            "role": "osa", "resource": "GPIB0::4::INSTR", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "command completed.*result encoding failed"):
            controller.handle("action", {"role": "osa", "name": "acquire"})
        self.assertEqual(InvalidTrace.instances[-1].acquisitions, 1)
        controller.close()

    def test_duplicate_visa_resource_is_rejected_before_second_device_construction(self) -> None:
        controller = ConsoleController(
            factories={"osa": FakeVisa, "pm400": FakeVisa},
        )
        controller.handle("connect", {
            "role": "osa", "resource": "GPIB0::4::INSTR", "acknowledge_lifecycle": True,
        })
        with self.assertRaises(ConsoleError):
            controller.handle("connect", {
                "role": "pm400", "resource": "GPIB0::4::INSTR",
            })
        self.assertEqual(len(FakeVisa.instances), 1)
        controller.close()

    def test_unknown_state_after_close_retains_device_and_visa_reservation(self) -> None:
        class AmbiguousOSA(FakeVisa):
            def __init__(self, resource_name: str):
                super().__init__(resource_name)
                self.is_open = False
                self.close_calls = 0

            def connect(self):
                super().connect()
                self.is_open = True
                return self

            def close(self):
                self.close_calls += 1
                if self.close_calls == 1:
                    self.state = SimpleNamespace(name="UNKNOWN")
                    return
                self.is_open = False
                super().close()

        controller = ConsoleController(
            factories={"osa": AmbiguousOSA, "pm400": FakeVisa},
        )
        controller.handle("connect", {
            "role": "osa", "resource": "GPIB0::4::INSTR", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "without confirmed release"):
            controller.handle("disconnect", {"role": "osa"})
        self.assertEqual(controller.handle("status", {})["devices"]["osa"]["state"], "UNKNOWN")
        self.assertTrue(AmbiguousOSA.instances[0].is_open)
        with self.assertRaisesRegex(ConsoleError, "VISA resource"):
            controller.handle("connect", {
                "role": "pm400", "resource": "GPIB0::4::INSTR",
            })
        self.assertEqual(len(FakeVisa.instances), 1)
        disconnected = controller.handle("disconnect", {"role": "osa"})
        self.assertFalse(disconnected["connected"])
        self.assertFalse(AmbiguousOSA.instances[0].is_open)
        self.assertEqual(controller.handle("status", {})["devices"], {})

    def test_voltage_close_requires_explicit_resource_release_evidence(self) -> None:
        class ReaderStillAlive(FakeVoltage):
            def __init__(self, port: str):
                super().__init__(port)
                self.resources_released = False
                self.close_calls = 0

            def close(self):
                self.close_calls += 1
                self.state = SimpleNamespace(name="DISCONNECTED")
                self.resources_released = self.close_calls > 1

        controller = ConsoleController(
            factories={"voltage": ReaderStillAlive, "gain": FakeVoltage},
            port_enumerator=lambda: (),
        )
        controller.handle("connect", {
            "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "without confirmed release"):
            controller.handle("disconnect", {"role": "voltage"})
        retained = controller.handle("status", {})["devices"]["voltage"]
        self.assertEqual(retained["resource"], "COM4")
        self.assertIsNone(retained["requested_voltage_v"],
                          "a partial shutdown cannot preserve the last completed target")
        with self.assertRaisesRegex(ConsoleError, "serial resource"):
            controller.handle("connect", {
                "role": "gain", "resource": "com4", "acknowledge_lifecycle": True,
            })
        disconnected = controller.handle("disconnect", {"role": "voltage"})
        self.assertFalse(disconnected["connected"])
        self.assertTrue(ReaderStillAlive.instances[0].resources_released)

    def test_open_port_evidence_overrides_disconnected_state(self) -> None:
        class OpenPortOSA(FakeVisa):
            def __init__(self, resource_name: str):
                super().__init__(resource_name)
                self.is_open = True
                self.close_calls = 0

            def close(self):
                self.close_calls += 1
                self.state = SimpleNamespace(name="DISCONNECTED")
                if self.close_calls > 1:
                    self.is_open = False

        controller = ConsoleController(factories={"osa": OpenPortOSA})
        controller.handle("connect", {
            "role": "osa", "resource": "GPIB0::4::INSTR", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "without confirmed release"):
            controller.handle("disconnect", {"role": "osa"})
        self.assertIn("osa", controller.handle("status", {})["devices"])
        self.assertTrue(OpenPortOSA.instances[0].is_open)
        self.assertFalse(controller.handle("disconnect", {"role": "osa"})["connected"])
        self.assertFalse(OpenPortOSA.instances[0].is_open)

    def test_release_evidence_read_failure_retains_voltage_for_retry(self) -> None:
        class UnreadableRelease(FakeVoltage):
            def __init__(self, port: str):
                super().__init__(port)
                self.release_read_fails = True

            @property
            def resources_released(self):
                if self.release_read_fails:
                    raise RuntimeError("release flag unavailable")
                return True

        controller = ConsoleController(
            factories={"voltage": UnreadableRelease},
            port_enumerator=lambda: (),
        )
        controller.handle("connect", {
            "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
        })
        self.addCleanup(setattr, UnreadableRelease.instances[0], 'release_read_fails', False)
        failure = None
        try:
            controller.handle("disconnect", {"role": "voltage"})
        except Exception as error:
            failure = error
        self.assertIsInstance(failure, ConsoleError)
        self.assertRegex(str(failure), "without confirmed release.*release flag unavailable")
        retained = controller.handle("status", {})["devices"]["voltage"]
        self.assertEqual(retained["resource"], "COM4")
        self.assertIsNone(retained["requested_voltage_v"])
        UnreadableRelease.instances[0].release_read_fails = False
        self.assertFalse(controller.handle("disconnect", {"role": "voltage"})["connected"])

    def test_voltage_and_gain_cannot_claim_the_same_serial_port(self) -> None:
        controller = ConsoleController(factories=wire_factories())
        controller.handle("connect", {
            "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
        })
        with self.assertRaisesRegex(ConsoleError, "serial resource"):
            controller.handle("connect", {
                "role": "gain", "resource": "com4", "acknowledge_lifecycle": True,
            })
        self.assertNotIn("gain", controller.handle("status", {})["devices"])
        controller.close()

    def test_known_mdt_port_is_rejected_for_gain_before_driver_construction(self) -> None:
        port = SimpleNamespace(device="COM6", vid=0x10C4, pid=0xEA60,
                               serial_number="2110148249-10", description="MDT693B",
                               manufacturer="Thorlabs")
        controller = ConsoleController(
            factories={"gain": FakeVoltage},
            port_enumerator=lambda: (port,),
        )
        with self.assertRaisesRegex(ConsoleError, "MDT693B"):
            controller.handle("connect", {
                "role": "gain", "resource": "com6", "acknowledge_lifecycle": True,
            })
        self.assertEqual(FakeVoltage.instances, [])

    def test_unregistered_mdt_candidate_is_rejected_before_serial_open(self) -> None:
        port = SimpleNamespace(device="COM8", vid=0x10C4, pid=0xEA60,
                               serial_number="OTHER-MDT", description="MDT693B",
                               manufacturer="Thorlabs", product="MDT693B")
        controller = ConsoleController(
            factories={"gain": FakeVoltage},
            port_enumerator=lambda: (port,),
        )
        with self.assertRaisesRegex(ConsoleError, "MDT693B"):
            controller.handle("connect", {
                "role": "gain", "resource": "COM8", "acknowledge_lifecycle": True,
            })
        self.assertEqual(FakeVoltage.instances, [])

    def test_mdt_port_reservation_covers_windows_namespace_alias(self) -> None:
        port = SimpleNamespace(device="COM6", vid=0x10C4, pid=0xEA60,
                               serial_number="2110148249-10", description="MDT693B",
                               manufacturer="Thorlabs")
        controller = ConsoleController(
            factories={"gain": FakeVoltage},
            port_enumerator=lambda: (port,),
        )
        with self.assertRaisesRegex(ConsoleError, "MDT693B"):
            controller.handle("connect", {
                "role": "gain", "resource": r"\\.\COM6", "acknowledge_lifecycle": True,
            })
        self.assertEqual(FakeVoltage.instances, [])

    def test_fiber_refuses_a_port_already_owned_by_serial_role(self) -> None:
        listed_ports = [()]
        port = SimpleNamespace(device="COM6", vid=0x10C4, pid=0xEA60,
                               serial_number="2110148249-10", description="MDT693B",
                               manufacturer="Thorlabs")
        fiber_constructed = []
        controller = ConsoleController(
            factories={"voltage": FakeVoltage,
                       "fiber": lambda: fiber_constructed.append(True)},
            port_enumerator=lambda: listed_ports[0],
        )
        controller.handle("connect", {
            "role": "voltage", "resource": "COM6", "acknowledge_lifecycle": True,
        })
        listed_ports[0] = (port,)
        with self.assertRaisesRegex(ConsoleError, "already assigned"):
            controller.handle("connect", {"role": "fiber"})
        self.assertEqual(fiber_constructed, [])
        controller.close()

    def test_stage_has_voltage_but_no_position_until_adoption(self) -> None:
        ports = [FakePort('COM900', '2110148249-10')]
        factory = fiber_factory(ports)
        controller = ConsoleController(factories={'fiber': lambda: factory(('2110148249-10',))},
                                       port_enumerator=lambda: ports)
        self.assertEqual(controller.handle("status", {})["devices"], {})
        connected = controller.handle("connect", {"role": "fiber"})
        left = connected["status"]["left"]
        self.assertEqual(left["serial_number"], "2110148249-10")
        self.assertEqual(left["observed_voltage_v"], {"x": 0.0, "y": 0.0, "z": 0.0})
        self.assertIsNone(left["estimated_position_um"])
        with self.assertRaises(ConsoleError):
            controller.handle("action", {
                "role": "fiber", "name": "move", "side": "left", "dx": 0.1,
            })
        with self.assertRaises(ConsoleError):
            controller.handle("action", {
                "role": "fiber", "name": "adopt_baseline", "side": "left",
                "confirm": False, "allow_nominal": True,
            })
        controller.handle("disconnect", {"role": "fiber"})
        controller.handle("connect", {"role": "fiber"})
        controller.handle("action", {
            "role": "fiber", "name": "adopt_baseline", "side": "left",
            "confirm": True, "allow_nominal": True,
        })
        moved = controller.handle("action", {
            "role": "fiber", "name": "move", "side": "left", "dx": 0.1,
        })
        self.assertAlmostEqual(moved["status"]["left"]["estimated_position_um"]["x"], 0.1)
        controller.close()

    def test_unknown_action_cannot_call_arbitrary_driver_method(self) -> None:
        controller = ConsoleController(factories={"voltage": FakeVoltage})
        controller.handle("connect", {
            "role": "voltage", "resource": "COM4", "acknowledge_lifecycle": True,
        })
        with self.assertRaises(ConsoleError):
            controller.handle("action", {
                "role": "voltage", "name": "_write_frame_locked", "frame": "unsafe",
            })
        self.assertEqual(FakeVoltage.instances[0].commands, [])
        controller.close()

    def test_fiber_factory_result_is_already_connected(self) -> None:
        class ConnectedFiber:
            state = SimpleNamespace(name="READY")
            left = SimpleNamespace(status={"side": "left", "available": True})
            right = SimpleNamespace(status={"side": "right", "available": False})
            connect_calls = 0

            def connect(self):
                self.connect_calls += 1
                raise AssertionError("FiberCouplingSetup.connect already returned a session")

            def close(self):
                self.state = SimpleNamespace(name="DISCONNECTED")

        fiber = ConnectedFiber()
        controller = ConsoleController(factories={"fiber": lambda: fiber})
        status = controller.handle("connect", {"role": "fiber"})
        self.assertTrue(status["connected"])
        self.assertEqual(fiber.connect_calls, 0)
        controller.close()

    def test_real_stage_axis_keys_serialize_as_logical_names(self) -> None:
        self.assertEqual(_json_value({LogicalAxis.X: 12.0, LogicalAxis.Z: 4.0}),
                         {"x": 12.0, "z": 4.0})

    def test_osa_acquisition_timestamp_serializes_for_frontend(self) -> None:
        measured = datetime(2026, 9, 25, 12, 0, tzinfo=timezone.utc)
        self.assertEqual(_json_value({"acquired_at": measured}),
                         {"acquired_at": "2026-09-25T12:00:00+00:00"})

    def test_pm400_measurement_kind_serializes_as_a_stable_name(self) -> None:
        self.assertEqual(_json_value(MeasurementKind.POWER_DENSITY), "power_density")

    def test_stage_rejected_target_keeps_voltage_and_estimate(self) -> None:
        factory = fiber_factory([FakePort('COM900', '2110148249-10')])
        setup = factory(('2110148249-10',))
        try:
            stage = setup.left
            stage.adopt_baseline(confirm=True, allow_nominal=True)
            before = stage.status
            with self.assertRaises(StageMotionLimitError):
                stage.move_by_um(dx=.3)
            self.assertEqual(stage.status.observed_voltage_v, before.observed_voltage_v)
            self.assertEqual(stage.status.estimated_position_um, before.estimated_position_um)
        finally:
            setup.close()

    def test_voltage_zero_evidence_keeps_truthful_driver_shape(self) -> None:
        controller = ConsoleController(factories=wire_factories())
        snapshot = controller.handle("connect", {
            "role": "voltage", "resource": "COM990", "acknowledge_lifecycle": True,
        })["status"]
        self.assertEqual(snapshot["zero_evidence"]["state"], "measured_zero")
        self.assertGreaterEqual(snapshot["sample_age_s"], 0.0)
        self.assertLess(snapshot["sample_age_s"], 1.0)
        report = controller.close()
        self.assertIn(report["voltage_zero"]["state"], ("measured_zero", "command_sent"))
        self.assertTrue(all(step["ok"] for step in report["steps"]))

    def test_gain_status_age_tracks_each_public_read(self) -> None:
        controller = ConsoleController(factories=wire_factories())
        snapshot = controller.handle("connect", {
            "role": "gain", "resource": "COM991", "acknowledge_lifecycle": True,
        })["status"]
        temperature = snapshot['fields']['temperature_c']
        self.assertGreaterEqual(temperature['observed_age_s'], 0.0)
        self.assertLess(temperature['observed_age_s'], 1.0)
        time.sleep(0.02)
        refreshed = controller.handle("status", {})["devices"]["gain"]
        self.assertGreater(refreshed['fields']['temperature_c']['observed_age_s'],
                           temperature['observed_age_s'])
        self.assertEqual(refreshed['fields']['temperature_c']['revision'], temperature['revision'],
                         'cache-only status cannot poll fresh telemetry')
        self.assertNotIn('received_at', refreshed)
        controller.close()

    def test_gain_current_requires_five_seconds_of_consecutive_stability(self) -> None:
        driver_tests.GainSafetyTests(
            'test_enable_current_requires_ready_tec_latest_temperature_and_six_samples').debug()
        driver_tests.GainSafetyTests(
            'test_six_samples_spanning_4_999_seconds_are_not_stable').debug()

    def test_pm400_supports_sensor_gated_measurement_and_typed_settings(self) -> None:
        controller = ConsoleController(factories=wire_factories())
        connected = controller.handle('connect', {
            'role': 'pm400', 'resource': 'USB0::0x1313::0x8078::UNITTEST::INSTR'})['status']
        self.assertTrue(next(item for item in connected['catalog']['measurements']
                             if item['key'] == 'power')['supported'])
        driver = controller._devices['pm400']
        driver.test_wire.replies.update({'SENSe:POWer:DC:UNIT?': 'W',
                                        'MEASure:SCALar:POWer?': '.001'})
        measured = controller.handle('action', {
            'role': 'pm400', 'name': 'measure', 'kind': 'power'})['result']
        self.assertEqual(measured['kind'], 'power')
        self.assertEqual(measured['value'], .001)
        driver.test_wire.replies['SENSe:POWer:DC:UNIT?'] = 'DBM'
        controller.handle('action', {
            'role': 'pm400', 'name': 'write', 'setting': 'sense.power_unit', 'value': 'DBM'})
        read = controller.handle('action', {
            'role': 'pm400', 'name': 'read', 'setting': 'sense.power_unit'})['result']
        self.assertEqual(read, 'DBM')
        with self.assertRaises(ConsoleError):
            controller.handle('action', {
                'role': 'pm400', 'name': 'write', 'setting': 'input.adapter_type',
                'value': 'thermal'})
        controller.close()


if __name__ == "__main__":
    unittest.main()
