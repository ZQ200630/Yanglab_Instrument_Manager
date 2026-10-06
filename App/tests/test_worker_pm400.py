"""The console exposes typed PM400 methods without raw SCPI or bypassing capabilities."""

from __future__ import annotations

import datetime
import unittest
import threading
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

from Code.Utils import AdapterType, MeasurementKind, PM400, PowerUnit, StatusGroup
from App.worker.controller import ConsoleController, ConsoleError
from App.worker.contracts import Request
from App.worker.pm400_ops import catalog, execute


class RecordingFacade:
    def __init__(self) -> None:
        self.calls: list[tuple[str, object]] = []

    def set_power_unit(self, value: PowerUnit) -> PowerUnit:
        self.calls.append(("set_power_unit", value))
        return value

    def get_power_unit(self) -> PowerUnit:
        self.calls.append(("get_power_unit", None))
        return PowerUnit.WATTS

    def set_wavelength_nm(self, value: float) -> float:
        self.calls.append(("set_wavelength_nm", value))
        return value

    def set_photodiode_response_a_per_w(self, value: float, *, confirm: bool) -> float:
        self.calls.append(("set_photodiode_response_a_per_w", (value, confirm)))
        return value

    def set_adapter_type(self, value: AdapterType, *, confirm: bool) -> AdapterType:
        self.calls.append(("set_adapter_type", (value, confirm)))
        return value

    def set_date(self, value: datetime.date) -> datetime.date:
        self.calls.append(("set_date", value))
        return value

    def read_condition(self, group: StatusGroup) -> int:
        self.calls.append(("read_condition", group))
        return 12

    def set_enable(self, group: StatusGroup, value: int) -> int:
        self.calls.append(("set_enable", (group, value)))
        return value

    def measure(self, kind: MeasurementKind):
        self.calls.append(("measure", kind))
        return SimpleNamespace(kind=kind, value=0.001, unit=kind.default_unit)

    def reset(self, *, confirm: bool) -> None:
        self.calls.append(("reset", confirm))


class FakePM400:
    def __init__(self, **caps: bool) -> None:
        capabilities = {
            "power": True, "energy": False, "response_settable": False,
            "wavelength_settable": False, "tau_settable": False,
            "temperature_sensor": False,
        }
        capabilities.update(caps)
        self.sensor_info = SimpleNamespace(capabilities=SimpleNamespace(**capabilities))
        self.sense = RecordingFacade()
        self.input = RecordingFacade()
        self.system = RecordingFacade()
        self.status = RecordingFacade()
        self.measurement = RecordingFacade()

    def reset(self, *, confirm: bool) -> None:
        self.system.reset(confirm=confirm)


class PM400ConsoleTests(unittest.TestCase):
    def test_pm400_validation_and_invocation_have_distinct_dispatch_phases(self):
        class ConnectedPM(FakePM400):
            def __init__(self, resource_name):
                super().__init__()
                self.state = SimpleNamespace(name="DISCONNECTED")
            def connect(self):
                self.state = SimpleNamespace(name="READY")
            def close(self):
                self.state = SimpleNamespace(name="DISCONNECTED")
        device = ConnectedPM("unused")
        controller = ConsoleController(factories={"pm400": lambda **kwargs: device},
                                       port_enumerator=lambda: ())
        self.addCleanup(controller.close)
        controller.handle("connect", {"role": "pm400", "resource": "USB0::0x1313::0x8078::TEST::INSTR"})
        rejected = controller.submit(Request("capability", "action", {
            "role": "pm400", "name": "write", "setting": "sense.wavelength_nm", "value": 1550,
        }, controller.context("pm400"))).result(2)
        self.assertEqual(rejected.phase, "rejected_before_call")
        self.assertEqual(device.sense.calls, [])
        self.assertEqual(controller.cached_status()["roles"]["pm400"]["state"], "READY")
        def fail_after_write(value):
            device.sense.calls.append(("set_power_unit", value))
            raise ValueError("reply malformed after public write")
        device.sense.set_power_unit = fail_after_write
        failed = controller.submit(Request("write", "action", {
            "role": "pm400", "name": "write", "setting": "sense.power_unit", "value": "DBM",
        }, controller.context("pm400"))).result(2)
        self.assertEqual(failed.phase, "failed_after_call_started")
        self.assertEqual(device.sense.calls, [("set_power_unit", PowerUnit.DBM)])
        self.assertEqual(controller.cached_status()["roles"]["pm400"]["state"], "FAULT")

    def test_blocking_pm400_capabilities_do_not_block_cached_status_or_other_role(self):
        entered, release = threading.Event(), threading.Event()
        class SlowCapabilities:
            def __init__(self, resource_name):
                self.state = SimpleNamespace(name="DISCONNECTED")
            def connect(self):
                self.state = SimpleNamespace(name="READY")
            @property
            def sensor_info(self):
                entered.set()
                if not release.wait(4):
                    raise TimeoutError("test release not received")
                return None
            def close(self):
                self.state = SimpleNamespace(name="DISCONNECTED")
        from App.tests.test_worker_controller import FakeVisa
        controller = ConsoleController(factories={"pm400": SlowCapabilities, "osa": FakeVisa}, port_enumerator=lambda: ())
        try:
            pending = controller.submit(Request("connect", "connect", {"role": "pm400", "resource": "USB0::0x1313::0x8078::TEST::INSTR"},
                                                controller.context("pm400")))
            self.assertTrue(entered.wait(2))
            with ThreadPoolExecutor(max_workers=1) as pool:
                snapshot = pool.submit(controller.handle, "status", {}).result(1)
                self.assertEqual(snapshot["roles"]["pm400"]["state"], "CONNECTING")
                connected = pool.submit(controller.handle, "connect", {
                    "role": "osa", "resource": "GPIB0::4::INSTR", "acknowledge_lifecycle": True}).result(1)
                self.assertTrue(connected["connected"])
            self.assertFalse(pending.done())
        finally:
            release.set()
            controller.close()

    def test_catalog_marks_unsupported_measurements_and_settings_before_write(self) -> None:
        listing = catalog(FakePM400())
        measurements = {item["key"]: item for item in listing["measurements"]}
        self.assertEqual(len(measurements), 9)
        self.assertTrue(measurements["power"]["supported"])
        self.assertFalse(measurements["energy"]["supported"])
        settings = {item["key"]: item for item in listing["settings"]}
        self.assertFalse(settings["sense.wavelength_nm"]["supported"])
        self.assertFalse(settings["sense.photodiode_response_a_per_w"]["supported"])

    def test_unsupported_energy_measurement_fails_before_driver_call(self) -> None:
        device = FakePM400()
        with self.assertRaisesRegex(ValueError, "capability"):
            execute(device, "measure", {"kind": "energy"})
        self.assertEqual(device.measurement.calls, [])

    def test_measurement_configuration_rejects_unsupported_sensor_kind_before_write(self) -> None:
        device = FakePM400()
        setting = next(item for item in catalog(device)["settings"]
                       if item["key"] == "measurement.configuration")
        options = {item["key"]: item for item in setting["choices"]}
        self.assertFalse(options["energy"]["supported"])
        with self.assertRaisesRegex(ValueError, "capability"):
            execute(device, "write", {
                "setting": "measurement.configuration", "value": "energy",
            })
        self.assertEqual(device.measurement.calls, [])

    def test_unsupported_wavelength_write_fails_before_driver_call(self) -> None:
        device = FakePM400()
        with self.assertRaisesRegex(ValueError, "capability"):
            execute(device, "write", {"setting": "sense.wavelength_nm", "value": 1550.0})
        self.assertEqual(device.sense.calls, [])

    def test_power_unit_and_measurement_use_typed_driver_arguments(self) -> None:
        device = FakePM400()
        self.assertIs(execute(device, "write", {"setting": "sense.power_unit", "value": "DBM"}),
                      PowerUnit.DBM)
        self.assertEqual(device.sense.calls, [("set_power_unit", PowerUnit.DBM)])
        result = execute(device, "measure", {"kind": "power"})
        self.assertEqual(result.kind, MeasurementKind.POWER)
        self.assertEqual(device.measurement.calls, [("measure", MeasurementKind.POWER)])

    def test_sensitive_changes_require_server_side_confirmation(self) -> None:
        device = FakePM400(response_settable=True)
        with self.assertRaisesRegex(ValueError, "confirm"):
            execute(device, "write", {
                "setting": "sense.photodiode_response_a_per_w", "value": 0.12,
            })
        self.assertEqual(device.sense.calls, [])
        self.assertEqual(execute(device, "write", {
            "setting": "sense.photodiode_response_a_per_w", "value": 0.12,
            "confirm": True,
        }), 0.12)
        self.assertEqual(device.sense.calls,
                         [("set_photodiode_response_a_per_w", (0.12, True))])
        with self.assertRaisesRegex(ValueError, "confirm"):
            execute(device, "command", {"command": "root.reset"})
        self.assertEqual(device.system.calls, [])

    def test_status_group_and_system_date_are_not_raw_strings(self) -> None:
        device = FakePM400()
        self.assertEqual(execute(device, "read", {
            "setting": "status.condition", "group": "operation",
        }), 12)
        self.assertEqual(device.status.calls, [("read_condition", StatusGroup.OPERATION)])
        self.assertEqual(execute(device, "write", {
            "setting": "system.date", "value": "2026-09-25",
        }), datetime.date(2026, 9, 25))
        self.assertEqual(device.system.calls,
                         [("set_date", datetime.date(2026, 9, 25))])

    def test_unknown_action_cannot_reach_private_or_raw_driver_api(self) -> None:
        device = FakePM400()
        with self.assertRaises(ValueError):
            execute(device, "write", {"setting": "_write_action", "value": "*RST"})
        self.assertEqual(device.sense.calls, [])

    def test_malformed_json_value_is_rejected_before_a_pm400_write(self) -> None:
        device = FakePM400()
        with self.assertRaisesRegex(ValueError, "power unit"):
            execute(device, "write", {
                "setting": "sense.power_unit", "value": ["W"],
            })
        self.assertEqual(device.sense.calls, [])

    def test_every_declared_pm400_setting_and_command_maps_to_public_driver_method(self) -> None:
        device = PM400("USB0::0x1313::0x8078::TEST::INSTR")
        listing = catalog(device)
        for setting in listing["settings"]:
            owner = device if setting["section"] == "root" else getattr(device, setting["section"])
            if setting["readable"]:
                self.assertTrue(callable(getattr(owner, setting["reader"])), setting["key"])
            if setting["writable"]:
                self.assertTrue(callable(getattr(owner, setting["writer"])), setting["key"])
        for command in listing["commands"]:
            owner = device if command["section"] == "root" else getattr(device, command["section"])
            self.assertTrue(callable(getattr(owner, command["method"])), command["key"])

    def test_console_routes_pm400_actions_and_catalog_through_the_typed_registry(self) -> None:
        class ConnectedPM(FakePM400):
            def __init__(self, resource_name: str):
                super().__init__()
                self.state = SimpleNamespace(name="DISCONNECTED")
                self.instrument_info = SimpleNamespace(manufacturer="THORLABS", model="PM400")
                self.last_preexisting_errors = (
                    SimpleNamespace(code=-200, message="Execution error", raw='-200,"Execution error"'),
                )

            def connect(self):
                self.state = SimpleNamespace(name="READY")
                return self

            def close(self):
                self.state = SimpleNamespace(name="DISCONNECTED")

        controller = ConsoleController(factories={"pm400": ConnectedPM})
        self.addCleanup(controller.close)
        connected = controller.handle("connect", {
            "role": "pm400", "resource": "USB0::0x1313::0x8078::TEST::INSTR",
        })
        self.assertEqual(len(connected["status"]["catalog"]["measurements"]), 9)
        self.assertEqual(connected["status"].get("last_preexisting_errors"), [
            {"code": -200, "message": "Execution error", "raw": '-200,"Execution error"'},
        ])
        result = controller.handle("action", {
            "role": "pm400", "name": "measure", "kind": "power",
        })
        self.assertEqual(result["result"]["value"], 0.001)
        with self.assertRaises(ConsoleError):
            controller.handle("action", {
                "role": "pm400", "name": "write", "setting": "sense.wavelength_nm",
                "value": 1550.0,
            })
        controller.close()

    def test_pm400_driver_value_error_after_write_is_not_reported_as_invalid_input(self) -> None:
        class FaultingFacade(RecordingFacade):
            def set_power_unit(self, value: PowerUnit) -> PowerUnit:
                super().set_power_unit(value)
                raise ValueError("reply invalid after unit write")

        class ConnectedPM(FakePM400):
            instances: list[ConnectedPM] = []

            def __init__(self, resource_name: str):
                super().__init__()
                self.sense = FaultingFacade()
                self.state = SimpleNamespace(name="DISCONNECTED")
                self.instrument_info = SimpleNamespace(manufacturer="THORLABS", model="PM400")
                self.instances.append(self)

            def connect(self):
                self.state = SimpleNamespace(name="READY")
                return self

            def close(self):
                self.state = SimpleNamespace(name="DISCONNECTED")

        controller = ConsoleController(factories={"pm400": ConnectedPM})
        self.addCleanup(controller.close)
        controller.handle("connect", {
            "role": "pm400", "resource": "USB0::0x1313::0x8078::TEST::INSTR",
        })
        with self.assertRaisesRegex(ConsoleError, "outcome is unknown"):
            controller.handle("action", {
                "role": "pm400", "name": "write", "setting": "sense.power_unit", "value": "DBM",
            })
        self.assertEqual(ConnectedPM.instances[-1].sense.calls,
                         [("set_power_unit", PowerUnit.DBM)])
        controller.close()


if __name__ == "__main__":
    unittest.main()
