from __future__ import annotations

import copy
import ast
import contextlib
import io
import json
import math
import tempfile
import threading
import unittest
from dataclasses import FrozenInstanceError, dataclass, replace
from pathlib import Path
from types import MappingProxyType
from typing import Mapping
from unittest.mock import patch

from Code.Debugs import check_fiber_stages
from Code.Debugs import test_mdt693b as mdt_test_support

from Code.Setups.fiber_coupling import (
    DEFAULT_CONFIG_PATH,
    AxisCalibration,
    BaselineUnknownError,
    CalibrationRequiredError,
    CalibrationCoefficient,
    FiberCouplingConfig,
    FiberCouplingSetup,
    InstrumentSafetyError,
    LogicalAxis,
    MoveResult,
    SerialDeviceInfo,
    FiberDiscovery,
    StageStatus,
    StageConnectionError,
    StageMotionError,
    StageMotionLimitError,
    StageDefinition,
    StageSide,
    StageUnavailableError,
    Vector3Um,
    load_fiber_coupling_config,
)
from Code.Utils.mdt693b import Axis, AxisState, MDTStatus, RotaryMode, VoltageLimit


_AXIS_BASELINE_ATTESTATION_EVIDENCE = (
    "MDT axis-command baseline adopted by explicit operator attestation; "
    "software cannot verify absence of external/manual contributions"
)


def _coefficient() -> dict[str, object]:
    return {
        "um_per_v": 0.2666666666666667,
        "source": "nominal_MAX312D",
        "date": None,
        "note": "MAX312D nominal 20 um over 75 V",
    }


def _valid_config() -> dict[str, object]:
    calibration = {
        axis: {"positive": _coefficient(), "negative": _coefficient()}
        for axis in ("x", "y", "z")
    }
    return {
        "model": "MAX312D",
        "operator_limits_um": {"toward_chip": 0.2, "other": 1.0},
        "stages": {
            "left": {
                "serial_number": "2110148249-10",
                "toward_chip_sign": 1,
                "axis_map": {"x": "Y", "y": "X", "z": "Z"},
                "polarity": {"x": 1, "y": 1, "z": 1},
                "calibration": copy.deepcopy(calibration),
            },
            "right": {
                "serial_number": "160721175410",
                "toward_chip_sign": -1,
                "axis_map": {"x": "X", "y": "Y", "z": "Z"},
                "polarity": {"x": 1, "y": 1, "z": 1},
                "calibration": copy.deepcopy(calibration),
            },
        },
    }


class FiberConfigTests(unittest.TestCase):
    def _load(self, raw: object):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fiber_coupling.json"
            path.write_text(json.dumps(raw), encoding="utf-8")
            return load_fiber_coupling_config(path)

    def _assert_rejected(self, raw: object) -> None:
        with self.assertRaises(InstrumentSafetyError):
            self._load(raw)

    def _load_text(self, text: str):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fiber_coupling.json"
            path.write_text(text, encoding="utf-8")
            return load_fiber_coupling_config(path)

    def test_default_config_has_exact_registered_topology(self):
        config = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)
        left = config.stages[StageSide.LEFT]
        right = config.stages[StageSide.RIGHT]

        self.assertEqual(left.serial_number, "2110148249-10")
        self.assertEqual(right.serial_number, "160721175410")
        self.assertEqual(left.axis_map[LogicalAxis.X], Axis.Y)
        self.assertEqual(left.axis_map[LogicalAxis.Y], Axis.X)
        self.assertEqual(left.axis_map[LogicalAxis.Z], Axis.Z)
        self.assertEqual(right.axis_map[LogicalAxis.X], Axis.X)
        self.assertEqual(right.axis_map[LogicalAxis.Y], Axis.Y)
        self.assertEqual(right.axis_map[LogicalAxis.Z], Axis.Z)
        self.assertEqual(left.toward_chip_sign, 1)
        self.assertEqual(right.toward_chip_sign, -1)

    def test_default_config_has_twelve_nominal_directional_coefficients(self):
        config = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)
        coefficients = [
            coefficient
            for stage in config.stages.values()
            for axis in stage.calibration.values()
            for coefficient in (axis.positive, axis.negative)
        ]
        self.assertEqual(len(coefficients), 12)
        for coefficient in coefficients:
            self.assertEqual(coefficient.um_per_v, 0.2666666666666667)
            self.assertEqual(coefficient.source, "nominal_MAX312D")

    def test_vector_rejects_bool_nan_infinity_and_non_numbers(self):
        for bad in (True, float("nan"), float("inf"), "0.1", None):
            with self.subTest(bad=bad):
                with self.assertRaises(InstrumentSafetyError):
                    Vector3Um(bad, 0.0, 0.0)

    def test_loader_rejects_missing_or_extra_stage_sides(self):
        missing = _valid_config()
        del missing["stages"]["right"]
        extra = _valid_config()
        extra["stages"]["spare"] = copy.deepcopy(extra["stages"]["left"])
        self._assert_rejected(missing)
        self._assert_rejected(extra)

    def test_loader_rejects_duplicate_stage_serial_numbers(self):
        raw = _valid_config()
        raw["stages"]["right"]["serial_number"] = "2110148249-10"
        self._assert_rejected(raw)

    def test_loader_rejects_incomplete_or_duplicate_axis_permutations(self):
        incomplete = _valid_config()
        del incomplete["stages"]["left"]["axis_map"]["z"]
        duplicate = _valid_config()
        duplicate["stages"]["left"]["axis_map"]["y"] = "Y"
        self._assert_rejected(incomplete)
        self._assert_rejected(duplicate)

    def test_loader_rejects_invalid_polarity_and_model(self):
        for bad in (0, -2, True, "1"):
            with self.subTest(polarity=bad):
                raw = _valid_config()
                raw["stages"]["left"]["polarity"]["x"] = bad
                self._assert_rejected(raw)
        raw = _valid_config()
        raw["model"] = "MAX313D"
        self._assert_rejected(raw)

    def test_loader_rejects_missing_directional_calibration(self):
        for direction in ("positive", "negative"):
            with self.subTest(direction=direction):
                raw = _valid_config()
                del raw["stages"]["left"]["calibration"]["x"][direction]
                self._assert_rejected(raw)

    def test_loader_rejects_invalid_calibration_coefficients_and_sources(self):
        for bad in (True, "0.1", float("nan"), float("inf"), 0.0, -0.1, None):
            with self.subTest(coefficient=bad):
                raw = _valid_config()
                raw["stages"]["left"]["calibration"]["x"]["positive"]["um_per_v"] = bad
                self._assert_rejected(raw)
        raw = _valid_config()
        raw["stages"]["left"]["calibration"]["x"]["positive"]["source"] = "estimated"
        self._assert_rejected(raw)

    def test_loader_rejects_limit_violations_and_negative_limits(self):
        for field, bad in (
            ("toward_chip", 0.2000001), ("other", 1.0000001),
            ("toward_chip", -0.01), ("other", -0.01),
        ):
            with self.subTest(field=field, limit=bad):
                raw = _valid_config()
                raw["operator_limits_um"][field] = bad
                self._assert_rejected(raw)

    def test_loader_accepts_zero_operator_limits(self):
        raw = _valid_config()
        raw["operator_limits_um"] = {"toward_chip": 0.0, "other": 0.0}
        config = self._load(raw)
        self.assertEqual(config.toward_chip_limit_um, 0.0)
        self.assertEqual(config.other_limit_um, 0.0)

    def test_loader_rejects_nonfinite_and_nonnumeric_operator_limits(self):
        for field, bad in (
            ("toward_chip", True), ("other", "0.1"),
            ("toward_chip", float("nan")), ("other", float("inf")),
        ):
            with self.subTest(field=field, limit=bad):
                raw = _valid_config()
                raw["operator_limits_um"][field] = bad
                self._assert_rejected(raw)

    def test_loader_rejects_duplicate_top_level_json_name_with_cause(self):
        with self.assertRaises(InstrumentSafetyError) as caught:
            self._load_text('{"model": "MAX312D", "model": "MAX312D"}')
        self.assertIn("$", str(caught.exception))
        self.assertIn("model", str(caught.exception))
        self.assertIsInstance(caught.exception.__cause__, ValueError)

    def test_loader_rejects_duplicate_nested_limit_json_name_with_cause(self):
        with self.assertRaises(InstrumentSafetyError) as caught:
            self._load_text(
                '{"model":"MAX312D","operator_limits_um":'
                '{"toward_chip":0.2,"toward_chip":0.1,"other":1.0},"stages":{}}'
            )
        self.assertIn("$.operator_limits_um", str(caught.exception))
        self.assertIn("toward_chip", str(caught.exception))
        self.assertIsInstance(caught.exception.__cause__, ValueError)

    def test_loader_rejects_unexpected_keys_and_json_constants(self):
        raw = _valid_config()
        raw["stages"]["left"]["calibration"]["x"]["positive"]["extra"] = "unsafe"
        self._assert_rejected(raw)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "nan.json"
            path.write_text('{"model": NaN}', encoding="utf-8")
            with self.assertRaises(InstrumentSafetyError):
                load_fiber_coupling_config(path)

    def test_loaded_and_constructed_mappings_are_defensively_immutable(self):
        config = self._load(_valid_config())
        stage = config.stages[StageSide.LEFT]
        for mapping in (config.stages, stage.axis_map, stage.polarity, stage.calibration):
            with self.subTest(mapping=mapping):
                self.assertIsInstance(mapping, MappingProxyType)
                with self.assertRaises(TypeError):
                    mapping[object()] = object()

        axis_map = {LogicalAxis.X: Axis.X, LogicalAxis.Y: Axis.Y, LogicalAxis.Z: Axis.Z}
        polarity = {LogicalAxis.X: 1, LogicalAxis.Y: 1, LogicalAxis.Z: 1}
        calibration = AxisCalibration(_coefficient_object(), _coefficient_object())
        calibrations = {axis: calibration for axis in LogicalAxis}
        definition = StageDefinition(StageSide.LEFT, "serial", 1, axis_map, polarity, calibrations)
        complete = FiberCouplingConfig("MAX312D", {StageSide.LEFT: definition}, 0.2, 1.0)
        axis_map[LogicalAxis.X] = Axis.Y
        self.assertEqual(definition.axis_map[LogicalAxis.X], Axis.X)
        with self.assertRaises(TypeError):
            complete.stages[StageSide.RIGHT] = definition


def _coefficient_object() -> CalibrationCoefficient:
    return CalibrationCoefficient(0.2666666666666667, "nominal_MAX312D", None, "nominal")


@dataclass(frozen=True)
class FakePort:
    device: str
    serial_number: str | None
    description: str = "Thorlabs MDT693B"
    manufacturer: str = "Thorlabs"
    product: str = "MDT693B"


def make_mdt_status(
    serial: str,
    *,
    axis_command_known: bool = False,
    restricted: bool = False,
    fault_evidence: str | None = None,
    actual_v: Mapping[Axis, float] | None = None,
) -> MDTStatus:
    voltages = actual_v or {axis: 0.0 for axis in Axis}
    return MDTStatus(
        product="MDT693B",
        firmware="1.0",
        serial_number=serial,
        friendly_name="fake",
        echo_enabled=True,
        hardware_limit=VoltageLimit.V75,
        display_intensity=1,
        master_scan_enabled=False,
        master_scan_voltage_v=0.0,
        axes={axis: AxisState(voltages[axis], 0.0, 75.0) for axis in Axis},
        dac_step=1,
        compatibility_enabled=False,
        rotary_mode=RotaryMode.DEFAULT,
        push_to_adjust_disabled=False,
        supported_commands=frozenset(),
        restricted=restricted,
        fault_evidence=fault_evidence,
        observed_at=0.0,
        axis_command_known=axis_command_known,
    )


_NO_OVERRIDE = object()


class FakeSetupMDT:
    def __init__(
        self,
        port,
        *,
        status,
        connect_error=None,
        close_error=None,
        retain_open=False,
        retain_fault=False,
        omit_is_open=False,
        omit_state=False,
        status_error=None,
        probe_errors=None,
        adopt_error=None,
        adopt_entered=None,
        adopt_continue=None,
        close_entered=None,
        close_continue=None,
        adopt_return_status=_NO_OVERRIDE,
        post_adopt_status=_NO_OVERRIDE,
        post_adopt_status_error=_NO_OVERRIDE,
        setter_gate=None,
        setter_entered=None,
        set_errors=None,
        setter_mutation=None,
        setter_return_mutations=None,
        setter_current_mutations=None,
        setter_current_errors=None,
        setter_event_log=None,
        get_all_error=None,
        get_all_errors=None,
    ):
        self.port = port
        self._status = status
        self.status_error = status_error
        self.application_limits_v = MappingProxyType({axis: 75.0 for axis in Axis})
        self.connect_error = connect_error
        self.close_error = close_error
        self.retain_open = retain_open
        self.retain_fault = retain_fault
        self.probe_errors = dict(probe_errors or {})
        self.adopt_error = adopt_error
        self.adopt_entered = adopt_entered
        self.adopt_continue = adopt_continue
        self.close_entered = close_entered
        self.close_continue = close_continue
        self.adopt_return_status = adopt_return_status
        self.post_adopt_status = post_adopt_status
        self.post_adopt_status_error = post_adopt_status_error
        self.setter_gate = setter_gate
        self.setter_entered = setter_entered or threading.Event()
        self.set_errors = dict(set_errors or {})
        self.setter_mutation = setter_mutation
        self.setter_return_mutations = dict(setter_return_mutations or {})
        self.setter_current_mutations = dict(setter_current_mutations or {})
        self.setter_current_errors = dict(setter_current_errors or {})
        self.setter_event_log = setter_event_log
        self.get_all_error = get_all_error
        self.get_all_errors = dict(get_all_errors or {})
        self.connect_calls = 0
        self.close_calls = 0
        self.status_reads = 0
        self.set_calls = []
        self.adopt_calls = []
        self.get_all_calls = 0
        if not omit_is_open:
            self.is_open = True
        if not omit_state:
            self.state = "READY"

    def __getattribute__(self, name):
        if name in {"is_open", "state", "status"}:
            try:
                probe_errors = object.__getattribute__(self, "probe_errors")
            except AttributeError:
                probe_errors = {}
            error = probe_errors.get(name)
            if error is not None:
                raise error
        return object.__getattribute__(self, name)

    @property
    def status(self):
        self.status_reads += 1
        if self.status_error is not None:
            raise self.status_error
        return self._status

    def connect(self):
        self.connect_calls += 1
        if self.connect_error is not None:
            raise self.connect_error
        return self

    def adopt_current_axis_baseline(self, *, confirm=False):
        self.adopt_calls.append(confirm)
        if self.adopt_error is not None:
            raise self.adopt_error
        self._status = replace(self._status, axis_command_known=True)
        adopted = self._status
        if self.adopt_entered is not None:
            self.adopt_entered.set()
        if self.adopt_continue is not None:
            self.adopt_continue.wait(timeout=2.0)
        if self.post_adopt_status is not _NO_OVERRIDE:
            self._status = self.post_adopt_status
        if self.post_adopt_status_error is not _NO_OVERRIDE:
            self.status_error = self.post_adopt_status_error
        return adopted if self.adopt_return_status is _NO_OVERRIDE else self.adopt_return_status

    def get_all_voltages(self):
        self.get_all_calls += 1
        error = self.get_all_errors.get(self.get_all_calls, self.get_all_error)
        if error is not None:
            raise error
        observed = MappingProxyType({axis: state.actual_v for axis, state in self.status.axes.items()})
        mutation = getattr(self, "get_all_mutation", None)
        if mutation is not None:
            mutation(self)
        return observed

    def set_axis_voltage(self, axis, target_v):
        self.set_calls.append((axis, target_v))
        if self.setter_event_log is not None:
            self.setter_event_log.append((threading.current_thread().name, axis, target_v))
        if self.setter_gate is not None:
            self.setter_entered.set()
            self.setter_gate.wait(timeout=2.0)
        error = self.set_errors.get(len(self.set_calls))
        if error is not None:
            raise error
        axes = dict(self._status.axes)
        axes[axis] = replace(axes[axis], actual_v=target_v)
        self._status = replace(self._status, axes=axes, axis_command_known=True)
        returned = self._status
        return_mutation = self.setter_return_mutations.get(len(self.set_calls))
        if return_mutation is not None:
            returned = return_mutation(self, returned)
        if self.setter_mutation is not None:
            self.setter_mutation(self, axis, target_v)
        current_mutation = self.setter_current_mutations.get(len(self.set_calls))
        if current_mutation is not None:
            current_mutation(self, axis, target_v)
        current_error = self.setter_current_errors.get(len(self.set_calls))
        if current_error is not None:
            self.status_error = current_error
        return returned

    def close(self):
        self.close_calls += 1
        if self.close_error is not None:
            raise self.close_error
        if self.close_entered is not None:
            self.close_entered.set()
        if self.close_continue is not None:
            self.close_continue.wait(timeout=2.0)
        if not self.retain_open and "is_open" in self.__dict__:
            self.__dict__["is_open"] = False
        if "state" in self.__dict__:
            self.__dict__["state"] = "FAULT" if self.retain_fault else "DISCONNECTED"


class DescriptorProbeMDT(FakeSetupMDT):
    def __init__(self, *args, **kwargs):
        self.descriptor_errors = {}
        super().__init__(*args, **kwargs)

    @property
    def is_open(self):
        error = self.descriptor_errors.get("is_open")
        if error is not None:
            raise error
        return self.__dict__.get("_descriptor_is_open", True)

    @is_open.setter
    def is_open(self, value):
        self.__dict__["_descriptor_is_open"] = value

    @property
    def state(self):
        error = self.descriptor_errors.get("state")
        if error is not None:
            raise error
        return self.__dict__.get("_descriptor_state", "READY")

    @state.setter
    def state(self, value):
        self.__dict__["_descriptor_state"] = value

    def close(self):
        super().close()
        if not self.retain_open:
            self.__dict__["_descriptor_is_open"] = False
        self.__dict__["_descriptor_state"] = "FAULT" if self.retain_fault else "DISCONNECTED"


class FakeMDTFactory:
    def __init__(self, definitions):
        self.definitions = definitions
        self.instances = {}

    def __call__(self, port):
        definition = self.definitions[port]
        instance = FakeSetupMDT(port, **definition)
        self.instances[port] = instance
        return instance


class UnconfirmedBaselineFakeMDT(FakeSetupMDT):
    def adopt_current_axis_baseline(self, *, confirm=False):
        self.adopt_calls.append(confirm)
        return self._status


class PublicationGateLock:
    """Deterministically pause one publication transaction for race tests."""

    def __init__(self, *, pause_before_acquire):
        self._lock = threading.RLock()
        self.pause_before_acquire = pause_before_acquire
        self.entered = threading.Event()
        self.contender_entered = threading.Event()
        self.release = threading.Event()
        self._entries = 0
        self._entries_lock = threading.Lock()

    def __enter__(self):
        with self._entries_lock:
            self._entries += 1
            entry = self._entries
        if entry == 1 and self.pause_before_acquire:
            self.entered.set()
            self.release.wait(timeout=2.0)
        if entry > 1:
            self.contender_entered.set()
        self._lock.acquire()
        if entry == 1 and not self.pause_before_acquire:
            self.entered.set()
            self.release.wait(timeout=2.0)
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        self._lock.release()
        return False


class FailingPublicationLock:
    """Inject a final publication transaction failure after assignments."""

    def __init__(self, error, *, fail_on_entry=1):
        self.error = error
        self.fail_on_entry = fail_on_entry
        self.entries = 0

    def __enter__(self):
        self.entries += 1
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        if self.entries == self.fail_on_entry:
            raise self.error
        return False


class InstrumentedOperationLock:
    """RLock-compatible test wrapper recording attempted and entered owners."""

    def __init__(self, lock):
        self._lock = lock
        self.attempted = {}
        self.entered = {}
        self.events = []
        self._events_lock = threading.Lock()

    def acquire(self, *args, **kwargs):
        name = threading.current_thread().name
        with self._events_lock:
            self.attempted.setdefault(name, threading.Event()).set()
            self.events.append(("attempt", name))
        acquired = self._lock.acquire(*args, **kwargs)
        if acquired:
            with self._events_lock:
                self.entered.setdefault(name, threading.Event()).set()
                self.events.append(("enter", name))
        return acquired

    def release(self):
        name = threading.current_thread().name
        with self._events_lock:
            self.events.append(("exit", name))
        return self._lock.release()

    def __enter__(self):
        self.acquire()
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        self.release()
        return False

    def wait_attempt(self, name):
        with self._events_lock:
            event = self.attempted.setdefault(name, threading.Event())
        return event.wait(timeout=1.0)

    def has_entered(self, name):
        with self._events_lock:
            event = self.entered.get(name)
            return event is not None and event.is_set()


class FiberBaselineTests(unittest.TestCase):
    def setUp(self):
        self.config = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)
        self.left_serial = self.config.stages[StageSide.LEFT].serial_number
        self.right_serial = self.config.stages[StageSide.RIGHT].serial_number

    def _setup(self, *, config=None, side=StageSide.LEFT, driver=None):
        active_config = config or self.config
        active_driver = driver or FakeSetupMDT(
            "COM6" if side is StageSide.LEFT else "COM7",
            status=make_mdt_status(active_config.stages[side].serial_number),
        )
        resource = "COM6" if side is StageSide.LEFT else "COM7"
        setup = FiberCouplingSetup(
            active_config,
            FiberCouplingSetup.enumerate(
                active_config,
                port_enumerator=lambda: [FakePort(resource, active_config.stages[side].serial_number)],
            ),
            {side: active_driver},
        )
        self.addCleanup(setup.close)
        return setup, active_driver

    def _fully_measured_config(self):
        stages = {}
        for side, definition in self.config.stages.items():
            calibration = {
                axis: AxisCalibration(
                    replace(axis_calibration.positive, source="measured"),
                    replace(axis_calibration.negative, source="measured"),
                )
                for axis, axis_calibration in definition.calibration.items()
            }
            stages[side] = replace(definition, calibration=calibration)
        return replace(self.config, stages=stages)

    def _mixed_calibration_config(self):
        config = self._fully_measured_config()
        definition = config.stages[StageSide.LEFT]
        calibration = dict(definition.calibration)
        coefficient = calibration[LogicalAxis.Z]
        calibration[LogicalAxis.Z] = AxisCalibration(
            replace(coefficient.positive, source="nominal_MAX312D"), coefficient.negative,
        )
        return replace(config, stages={
            **config.stages,
            StageSide.LEFT: replace(definition, calibration=calibration),
        })

    def _assert_zero_setters(self, *drivers):
        for driver in drivers:
            self.assertEqual(driver.set_calls, [])

    def _replace_actual(self, driver, axis, actual_v):
        snapshot = driver.status
        driver._status = replace(
            snapshot,
            axes={**snapshot.axes, axis: replace(snapshot.axes[axis], actual_v=actual_v)},
        )

    def _status_with_actual(self, snapshot, axis, actual_v):
        return replace(
            snapshot,
            axes={**snapshot.axes, axis: replace(snapshot.axes[axis], actual_v=actual_v)},
        )

    def test_unavailable_and_precondition_rejections_perform_no_driver_calls(self):
        setup, left = self._setup()
        with self.assertRaises(StageUnavailableError):
            setup.right.adopt_baseline(confirm=True, allow_nominal=True)
        self.assertEqual(left.adopt_calls, [])
        self._assert_zero_setters(left)

        cases = (
            {},
            {"confirm": 1, "allow_nominal": True},
            {"confirm": True, "allow_nominal": 1},
            {"confirm": True},
        )
        for kwargs in cases:
            with self.subTest(kwargs=kwargs):
                guarded_setup, driver = self._setup()
                with self.assertRaises(InstrumentSafetyError):
                    guarded_setup.left.adopt_baseline(**kwargs)
                self.assertEqual(driver.adopt_calls, [])
                self.assertEqual(driver.get_all_calls, 0)
                self._assert_zero_setters(driver)

    def test_mixed_nominal_calibration_requires_session_authorization(self):
        setup, driver = self._setup(config=self._mixed_calibration_config())
        with self.assertRaises(CalibrationRequiredError):
            setup.left.adopt_baseline(confirm=True, allow_nominal=False)
        self.assertEqual(driver.adopt_calls, [])
        self._assert_zero_setters(driver)

    def test_fully_measured_calibration_adopts_without_nominal_authorization(self):
        setup, driver = self._setup(config=self._fully_measured_config())
        status = setup.left.adopt_baseline(confirm=True, allow_nominal=False)
        self.assertEqual(driver.adopt_calls, [True])
        self.assertTrue(status.baseline_known)
        self.assertFalse(status.nominal_authorized)
        self.assertEqual(status.estimated_position_um, Vector3Um(0.0, 0.0, 0.0))
        self._assert_zero_setters(driver)

    def test_adoption_publishes_logical_left_and_right_voltages(self):
        left = FakeSetupMDT(
            "COM6", status=make_mdt_status(
                self.left_serial, actual_v={Axis.X: 1.0, Axis.Y: 2.0, Axis.Z: 3.0},
            )
        )
        right = FakeSetupMDT(
            "COM7", status=make_mdt_status(
                self.right_serial, actual_v={Axis.X: 4.0, Axis.Y: 5.0, Axis.Z: 6.0},
            )
        )
        discovery = FiberCouplingSetup.enumerate(
            self.config,
            port_enumerator=lambda: [FakePort("COM6", self.left_serial), FakePort("COM7", self.right_serial)],
        )
        setup = FiberCouplingSetup(self.config, discovery, {StageSide.LEFT: left, StageSide.RIGHT: right})
        self.addCleanup(setup.close)

        left_status = setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        right_status = setup.right.adopt_baseline(confirm=True, allow_nominal=True)
        self.assertEqual(left.adopt_calls, [True])
        self.assertEqual(right.adopt_calls, [True])
        self.assertEqual(
            left_status.observed_voltage_v,
            {LogicalAxis.X: 2.0, LogicalAxis.Y: 1.0, LogicalAxis.Z: 3.0},
        )
        self.assertEqual(
            right_status.observed_voltage_v,
            {LogicalAxis.X: 4.0, LogicalAxis.Y: 5.0, LogicalAxis.Z: 6.0},
        )
        self.assertEqual(left_status.estimated_position_um, Vector3Um(0.0, 0.0, 0.0))
        self.assertTrue(left_status.baseline_known)
        self.assertTrue(left_status.nominal_authorized)
        self.assertEqual(left.get_all_calls, 0)
        self.assertEqual(right.get_all_calls, 0)
        self._assert_zero_setters(left, right)

    def test_exact_driver_attestation_is_informational_through_all_setup_validators(self):
        driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(
                self.left_serial,
                fault_evidence=_AXIS_BASELINE_ATTESTATION_EVIDENCE,
                actual_v={axis: 50.0 for axis in Axis},
            ),
        )
        setup, _ = self._setup(driver=driver)

        adopted = setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        self.assertTrue(adopted.baseline_known)
        self.assertEqual(adopted.fault, _AXIS_BASELINE_ATTESTATION_EVIDENCE)
        self.assertTrue(setup.left.status.baseline_known)

        moved = setup.left.move_by_um(0.1, -0.1, 0.1)

        self.assertTrue(moved.confirmed)
        self.assertEqual(moved.requested_um, Vector3Um(0.1, -0.1, 0.1))
        self.assertTrue(setup.left.status.baseline_known)

    def test_exact_attestation_is_hazardous_without_known_unrestricted_authority(self):
        for changes in (
            {"axis_command_known": False},
            {"restricted": True},
        ):
            setup, driver = self._setup()
            setup.left.adopt_baseline(confirm=True, allow_nominal=True)
            driver._status = replace(
                driver.status,
                fault_evidence=_AXIS_BASELINE_ATTESTATION_EVIDENCE,
                **changes,
            )
            with self.subTest(changes=changes):
                self.assertFalse(setup.left.status.baseline_known)

    def test_arbitrary_fault_evidence_remains_rejected_at_every_setup_boundary(self):
        arbitrary = _AXIS_BASELINE_ATTESTATION_EVIDENCE + " altered"

        adoption_setup, adoption_driver = self._setup(driver=FakeSetupMDT(
            "COM6",
            status=make_mdt_status(self.left_serial, fault_evidence=arbitrary),
        ))
        with self.assertRaises(BaselineUnknownError):
            adoption_setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        self.assertEqual(adoption_driver.set_calls, [])

        status_setup, status_driver = self._setup()
        status_setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        status_driver._status = replace(status_driver.status, fault_evidence=arbitrary)
        self.assertFalse(status_setup.left.status.baseline_known)

        planning_setup, planning_driver = self._setup()
        planning_setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        planning_driver._status = replace(planning_driver.status, fault_evidence=arbitrary)
        with self.assertRaises(BaselineUnknownError):
            planning_setup.left.move_by_um(0.1, 0.0, 0.0)
        self.assertEqual(planning_driver.set_calls, [])

        def returned_fault(_, snapshot):
            return replace(snapshot, fault_evidence=arbitrary)

        execution_driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(
                self.left_serial, actual_v={axis: 50.0 for axis in Axis},
            ),
            setter_return_mutations={1: returned_fault},
        )
        execution_setup, _ = self._setup(driver=execution_driver)
        execution_setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        with self.assertRaises(StageMotionError):
            execution_setup.left.move_by_um(0.1, 0.0, 0.0)
        self.assertEqual(len(execution_driver.set_calls), 1)

        final_driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(
                self.left_serial, actual_v={axis: 50.0 for axis in Axis},
            ),
        )
        final_setup, _ = self._setup(driver=final_driver)
        final_setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        final_driver.get_all_mutation = lambda fake: (
            setattr(fake, "_status", replace(fake.status, fault_evidence=arbitrary))
            if fake.get_all_calls == 2 else None
        )
        with self.assertRaises(StageMotionError):
            final_setup.left.move_by_um(0.1, 0.0, 0.0)
        self.assertEqual(len(final_driver.set_calls), 1)


    def test_adoption_requires_current_public_status_to_be_healthy_and_coherent(self):
        returned = make_mdt_status(
            self.left_serial,
            axis_command_known=True,
            actual_v={Axis.X: 0.0, Axis.Y: 0.0, Axis.Z: 0.0},
        )
        invalid_current = (
            None,
            replace(returned, axis_command_known=False),
            replace(returned, restricted=True),
            replace(returned, fault_evidence="current fault"),
            self._status_with_actual(returned, Axis.Y, 1.000001e-6),
        )
        for current in invalid_current:
            with self.subTest(current=current):
                setup, driver = self._setup(driver=FakeSetupMDT(
                    "COM6",
                    status=make_mdt_status(self.left_serial),
                    adopt_return_status=returned,
                    post_adopt_status=current,
                ))
                with self.assertRaises(BaselineUnknownError):
                    setup.left.adopt_baseline(confirm=True, allow_nominal=True)
                self.assertEqual(driver.adopt_calls, [True])
                self.assertEqual(driver.status_reads, 1)
                if current is not None:
                    self.assertFalse(setup.left.status.baseline_known)
                self._assert_zero_setters(driver)

        exact_current = self._status_with_actual(returned, Axis.Y, 1e-6)
        setup, driver = self._setup(driver=FakeSetupMDT(
            "COM6",
            status=make_mdt_status(self.left_serial),
            adopt_return_status=returned,
            post_adopt_status=exact_current,
        ))
        status = setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        self.assertEqual(driver.adopt_calls, [True])
        self.assertEqual(driver.status_reads, 1)
        self.assertTrue(status.baseline_known)
        self.assertEqual(status.observed_voltage_v[LogicalAxis.X], 1e-6)
        self._assert_zero_setters(driver)

    def test_adoption_current_status_baseexceptions_are_typed_and_clear_authority(self):
        for error_type in (Exception, KeyboardInterrupt, SystemExit):
            with self.subTest(error=error_type.__name__):
                original = error_type("current status failed")
                setup, driver = self._setup(driver=FakeSetupMDT(
                    "COM6",
                    status=make_mdt_status(self.left_serial),
                    post_adopt_status_error=original,
                ))
                with self.assertRaises(BaselineUnknownError) as caught:
                    setup.left.adopt_baseline(confirm=True, allow_nominal=True)
                self.assertIs(caught.exception.__cause__, original)
                self.assertEqual(driver.adopt_calls, [True])
                self.assertEqual(driver.status_reads, 1)
                driver.status_error = None
                self.assertFalse(setup.left.status.baseline_known)
                self._assert_zero_setters(driver)

    def test_status_keeps_baseline_at_exact_tolerance_and_invalidates_just_over(self):
        setup, driver = self._setup()
        setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        baseline = driver.status.axes[Axis.Y].actual_v
        self._replace_actual(driver, Axis.Y, baseline + 1e-6)
        self.assertTrue(setup.left.status.baseline_known)
        self._replace_actual(driver, Axis.Y, baseline + 1.000001e-6)
        status = setup.left.status
        self.assertFalse(status.baseline_known)
        self.assertIsNone(status.estimated_position_um)
        self.assertFalse(status.nominal_authorized)
        self._assert_zero_setters(driver)

    def test_status_invalidates_on_unknown_authority_restriction_fault_or_actual_change(self):
        cases = (
            {"axis_command_known": False},
            {"restricted": True},
            {"fault_evidence": "live fault"},
            {},
        )
        for changes in cases:
            with self.subTest(changes=changes):
                setup, driver = self._setup()
                setup.left.adopt_baseline(confirm=True, allow_nominal=True)
                if changes:
                    driver._status = replace(driver.status, **changes)
                else:
                    self._replace_actual(driver, Axis.Y, driver.status.axes[Axis.Y].actual_v + 0.01)
                status = setup.left.status
                self.assertFalse(status.baseline_known)
                self.assertIsNone(status.estimated_position_um)
                self.assertFalse(status.nominal_authorized)
                self.assertEqual(status.restricted, changes.get("restricted", False))
                self.assertEqual(status.fault, changes.get("fault_evidence"))
                self._assert_zero_setters(driver)

    def test_missing_status_invalidates_before_typed_status_failure(self):
        setup, driver = self._setup()
        setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        driver._status = None
        with self.assertRaises(StageConnectionError):
            setup.left.status
        driver._status = make_mdt_status(self.left_serial, axis_command_known=True)
        self.assertFalse(setup.left.status.baseline_known)
        self._assert_zero_setters(driver)

    def test_adoption_unconfirmed_or_baseexception_failure_leaves_no_baseline(self):
        unconfirmed = UnconfirmedBaselineFakeMDT(
            "COM6", status=make_mdt_status(self.left_serial),
        )
        setup, _ = self._setup(driver=unconfirmed)
        with self.assertRaises(BaselineUnknownError):
            setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        self.assertEqual(unconfirmed.adopt_calls, [True])
        self.assertFalse(setup.left.status.baseline_known)
        self._assert_zero_setters(unconfirmed)

        for changes in ({"restricted": True}, {"fault_evidence": "adoption fault"}):
            with self.subTest(changes=changes):
                setup, driver = self._setup(driver=FakeSetupMDT(
                    "COM6", status=make_mdt_status(self.left_serial, **changes),
                ))
                with self.assertRaises(BaselineUnknownError):
                    setup.left.adopt_baseline(confirm=True, allow_nominal=True)
                self.assertEqual(driver.adopt_calls, [True])
                published = setup.left.status
                self.assertFalse(published.baseline_known)
                self.assertEqual(published.restricted, changes.get("restricted", False))
                self.assertEqual(published.fault, changes.get("fault_evidence"))
                self._assert_zero_setters(driver)

        original = BaseException("baseline publication failed")
        setup, driver = self._setup(driver=FakeSetupMDT(
            "COM6", status=make_mdt_status(self.left_serial), adopt_error=original,
        ))
        with self.assertRaises(BaselineUnknownError) as caught:
            setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        self.assertIs(caught.exception.__cause__, original)
        self.assertFalse(setup.left.status.baseline_known)
        self._assert_zero_setters(driver)

    def test_reconnect_and_close_clear_session_baseline_and_status_is_immutable(self):
        setup, driver = self._setup()
        setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        status = setup.left.status
        with self.assertRaises(TypeError):
            status.observed_voltage_v[LogicalAxis.X] = 1.0
        with self.assertRaises(TypeError):
            status.calibration[LogicalAxis.X] = object()
        setup.close()
        closed = setup.left.status
        self.assertFalse(closed.available)
        self.assertFalse(closed.baseline_known)
        self.assertIsNone(closed.estimated_position_um)

        reconnected, replacement = self._setup(driver=FakeSetupMDT(
            "COM6", status=make_mdt_status(self.left_serial, axis_command_known=True),
        ))
        observed = reconnected.left.status
        self.assertFalse(observed.baseline_known)
        self.assertFalse(observed.nominal_authorized)
        self._assert_zero_setters(driver, replacement)

    def test_close_intent_and_adoption_are_serialized_without_setter_calls(self):
        close_entered = threading.Event()
        close_continue = threading.Event()
        driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(self.left_serial),
            close_entered=close_entered,
            close_continue=close_continue,
        )
        setup, _ = self._setup(driver=driver)
        close_errors = []
        closer = threading.Thread(target=lambda: _record_error(setup.close, close_errors))
        closer.start()
        self.assertTrue(close_entered.wait(timeout=1.0))
        with self.assertRaises(StageConnectionError):
            setup.left.adopt_baseline(confirm=True, allow_nominal=True)
        self.assertEqual(driver.adopt_calls, [])
        close_continue.set()
        closer.join(timeout=1.0)
        self.assertFalse(closer.is_alive())
        self.assertEqual(close_errors, [])
        self._assert_zero_setters(driver)

    def test_root_close_intent_cancels_adoption_before_publication(self):
        adopt_entered = threading.Event()
        adopt_continue = threading.Event()
        driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(self.left_serial),
            adopt_entered=adopt_entered,
            adopt_continue=adopt_continue,
        )
        setup, _ = self._setup(driver=driver)
        adoption_errors = []
        close_errors = []
        adopter = threading.Thread(
            target=lambda: _record_error(
                lambda: setup.left.adopt_baseline(confirm=True, allow_nominal=True), adoption_errors,
            )
        )
        adopter.start()
        self.assertTrue(adopt_entered.wait(timeout=1.0))
        closer = threading.Thread(target=lambda: _record_error(setup.close, close_errors))
        closer.start()
        self.assertTrue(setup._close_intent.wait(timeout=1.0))
        adopt_continue.set()
        adopter.join(timeout=1.0)
        closer.join(timeout=1.0)
        self.assertFalse(adopter.is_alive())
        self.assertFalse(closer.is_alive())
        self.assertEqual(close_errors, [])
        self.assertEqual(len(adoption_errors), 1)
        self.assertIsInstance(adoption_errors[0], StageConnectionError)
        self.assertEqual(driver.adopt_calls, [True])
        self.assertFalse(setup.left.status.baseline_known)
        self.assertIsNone(setup.left.status.estimated_position_um)
        self.assertFalse(setup.left.status.nominal_authorized)
        self._assert_zero_setters(driver)

    def test_publication_transaction_linearizes_close_before_or_after_baseline_publish(self):
        for pause_before_acquire, expected_close_wins in ((True, True), (False, False)):
            with self.subTest(close_wins=expected_close_wins):
                setup, driver = self._setup()
                gate = PublicationGateLock(pause_before_acquire=pause_before_acquire)
                setup._publication_lock = gate
                setup.left._publication_lock = gate
                setup.right._publication_lock = gate
                publications = []
                original_publish = setup.left._status_from_snapshot_unlocked

                def observe_publication(snapshot, observed):
                    publications.append((
                        setup._close_intent.is_set(),
                        setup.left._command_baseline_v is not None,
                        setup.left._estimated_position_um,
                        setup.left._nominal_authorized,
                    ))
                    return original_publish(snapshot, observed)

                setup.left._status_from_snapshot_unlocked = observe_publication
                adoption_errors = []
                close_errors = []
                adopter = threading.Thread(
                    target=lambda: _record_error(
                        lambda: setup.left.adopt_baseline(confirm=True, allow_nominal=True),
                        adoption_errors,
                    )
                )
                adopter.start()
                self.assertTrue(gate.entered.wait(timeout=1.0))
                closer = threading.Thread(target=lambda: _record_error(setup.close, close_errors))
                closer.start()
                self.assertTrue(gate.contender_entered.wait(timeout=1.0))
                if expected_close_wins:
                    self.assertTrue(setup._close_intent.wait(timeout=1.0))
                else:
                    self.assertFalse(setup._close_intent.is_set())
                gate.release.set()
                adopter.join(timeout=1.0)
                closer.join(timeout=1.0)
                self.assertFalse(adopter.is_alive())
                self.assertFalse(closer.is_alive())
                self.assertEqual(close_errors, [])
                self.assertEqual(driver.adopt_calls, [True])
                if expected_close_wins:
                    self.assertEqual(len(adoption_errors), 1)
                    self.assertIsInstance(adoption_errors[0], StageConnectionError)
                    self.assertEqual(publications, [])
                else:
                    self.assertEqual(adoption_errors, [])
                    self.assertEqual(
                        publications,
                        [(False, True, Vector3Um(0.0, 0.0, 0.0), True)],
                    )
                self.assertFalse(setup.left.status.baseline_known)
                self.assertIsNone(setup.left.status.estimated_position_um)
                self.assertFalse(setup.left.status.nominal_authorized)
                self._assert_zero_setters(driver)


class FiberMDTAttestationIntegrationTests(unittest.TestCase):
    """Real public MDT authority behavior crossing into the setup layer."""

    def test_real_query_only_adoption_supports_status_planning_and_motion(self):
        config = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)
        left = replace(config.stages[StageSide.LEFT], serial_number="SN123")
        config = replace(config, stages={**config.stages, StageSide.LEFT: left})
        driver, device, fake_time = mdt_test_support.MDTAxisCommandAuthorityTests._driver()
        discovery = FiberCouplingSetup.enumerate(
            config,
            port_enumerator=lambda: [FakePort("COM_TEST", "SN123")],
        )
        setup = FiberCouplingSetup(config, discovery, {StageSide.LEFT: driver})
        self.addCleanup(setup.close)

        adopted = setup.left.adopt_baseline(confirm=True, allow_nominal=True)

        self.assertEqual(driver.status.fault_evidence, _AXIS_BASELINE_ATTESTATION_EVIDENCE)
        self.assertTrue(adopted.baseline_known)
        self.assertEqual(adopted.fault, _AXIS_BASELINE_ATTESTATION_EVIDENCE)
        self.assertTrue(setup.left.status.baseline_known)

        fake_time.permit(10)
        moved = setup.left.move_by_um(0.02, 0.0, 0.0)

        self.assertTrue(moved.confirmed)
        self.assertEqual(moved.requested_um, Vector3Um(0.02, 0.0, 0.0))
        self.assertTrue(setup.left.status.baseline_known)
        self.assertTrue(any("=" in command for command in device.commands))


class FiberMotionPlanningTests(unittest.TestCase):
    """Pure planning boundaries; no assertion here permits a voltage setter."""

    def setUp(self):
        self.base_config = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)

    def _calibrated_config(self, *, toward=0.2, other=1.0):
        stages = {}
        number = 1
        for side, definition in self.base_config.stages.items():
            calibration = {}
            for axis in LogicalAxis:
                positive = CalibrationCoefficient(float(number), "measured", None, "positive")
                number += 1
                negative = CalibrationCoefficient(float(number), "measured", None, "negative")
                number += 1
                calibration[axis] = AxisCalibration(positive, negative)
            stages[side] = replace(definition, calibration=calibration)
        return replace(
            self.base_config, stages=stages,
            toward_chip_limit_um=toward, other_limit_um=other,
        )

    def _setup(self, *, config=None, side=StageSide.LEFT, driver=None):
        config = config or self._calibrated_config()
        definition = config.stages[side]
        driver = driver or FakeSetupMDT(
            "COM6" if side is StageSide.LEFT else "COM7",
            status=make_mdt_status(
                definition.serial_number, actual_v={axis: 50.0 for axis in Axis},
            ),
        )
        resource = "COM6" if side is StageSide.LEFT else "COM7"
        setup = FiberCouplingSetup(
            config,
            FiberCouplingSetup.enumerate(
                config, port_enumerator=lambda: [FakePort(resource, definition.serial_number)],
            ),
            {side: driver},
        )
        self.addCleanup(setup.close)
        setup_stage = setup.left if side is StageSide.LEFT else setup.right
        setup_stage.adopt_baseline(confirm=True, allow_nominal=True)
        return setup, setup_stage, driver

    def _plan(self, stage, request):
        with stage._operation_lock:
            return stage._plan_move_unlocked(request)

    def _assert_no_setters(self, *drivers):
        for driver in drivers:
            self.assertEqual(driver.set_calls, [])

    def _replace_axis(self, driver, axis, **changes):
        status = driver.status
        driver._status = replace(
            status, axes={**status.axes, axis: replace(status.axes[axis], **changes)},
        )

    def test_directional_coefficients_mapping_order_and_immutable_plan_are_exact(self):
        config = self._calibrated_config()
        for side in StageSide:
            setup, stage, driver = self._setup(config=config, side=side)
            definition = config.stages[side]
            for logical in LogicalAxis:
                for request_sign, coefficient in (
                    (0.1, definition.calibration[logical].positive),
                    (-0.1, definition.calibration[logical].negative),
                ):
                    request = Vector3Um(
                        request_sign if logical is LogicalAxis.X else 0.0,
                        request_sign if logical is LogicalAxis.Y else 0.0,
                        request_sign if logical is LogicalAxis.Z else 0.0,
                    )
                    plan = self._plan(stage, request)
                    move = plan.moves[0]
                    with self.subTest(side=side, axis=logical, sign=request_sign):
                        self.assertEqual(move.logical_axis, logical)
                        self.assertEqual(move.controller_axis, definition.axis_map[logical])
                        self.assertIs(move.coefficient, coefficient)
                        self.assertEqual(move.delta_v, request_sign / coefficient.um_per_v)
                        self.assertEqual(move.start_v, 50.0)
                        self.assertEqual(move.target_v, 50.0 + request_sign / coefficient.um_per_v)
            plan = self._plan(stage, Vector3Um(0.1, -0.2, 0.3))
            expected = (LogicalAxis.Y, LogicalAxis.Z, LogicalAxis.X) if side is StageSide.LEFT else (LogicalAxis.X, LogicalAxis.Y, LogicalAxis.Z)
            self.assertEqual(tuple(move.logical_axis for move in plan.moves), expected)
            with self.assertRaises(TypeError):
                plan.voltage_delta_v[LogicalAxis.X] = 1.0
            with self.assertRaises(TypeError):
                plan.calibration_used[LogicalAxis.X] = object()
            self._assert_no_setters(driver)

    def test_exact_directional_and_other_displacement_boundaries(self):
        cases = (
            (StageSide.LEFT, Vector3Um(0.2, 0.0, 0.0), True),
            (StageSide.LEFT, Vector3Um(math.nextafter(0.2, math.inf), 0.0, 0.0), False),
            (StageSide.LEFT, Vector3Um(-1.0, 0.0, 0.0), True),
            (StageSide.RIGHT, Vector3Um(-0.2, 0.0, 0.0), True),
            (StageSide.RIGHT, Vector3Um(math.nextafter(-0.2, -math.inf), 0.0, 0.0), False),
            (StageSide.RIGHT, Vector3Um(1.0, 0.0, 0.0), True),
        )
        for side, request, accepted in cases:
            with self.subTest(side=side, request=request):
                setup, stage, driver = self._setup(side=side)
                if accepted:
                    self._plan(stage, request)
                else:
                    with self.assertRaises(StageMotionLimitError):
                        self._plan(stage, request)
                self._assert_no_setters(driver)
        for side in StageSide:
            for axis in (LogicalAxis.Y, LogicalAxis.Z):
                for sign in (-1.0, 1.0):
                    request = Vector3Um(0.0, sign if axis is LogicalAxis.Y else 0.0, sign if axis is LogicalAxis.Z else 0.0)
                    setup, stage, driver = self._setup(side=side)
                    self._plan(stage, request)
                    too_far = Vector3Um(0.0, math.nextafter(sign, math.copysign(math.inf, sign)) if axis is LogicalAxis.Y else 0.0, math.nextafter(sign, math.copysign(math.inf, sign)) if axis is LogicalAxis.Z else 0.0)
                    with self.assertRaises(StageMotionLimitError):
                        self._plan(stage, too_far)
                    self._assert_no_setters(driver)

    def test_configuration_can_reduce_but_not_raise_requested_limits(self):
        setup, stage, driver = self._setup(config=self._calibrated_config(toward=0.1, other=0.5))
        self._plan(stage, Vector3Um(0.1, 0.0, 0.0))
        self._plan(stage, Vector3Um(-0.5, 0.0, 0.0))
        for request in (Vector3Um(math.nextafter(0.1, math.inf), 0.0, 0.0), Vector3Um(-math.nextafter(0.5, math.inf), 0.0, 0.0)):
            with self.assertRaises(StageMotionLimitError):
                self._plan(stage, request)
        self._assert_no_setters(driver)

    def test_voltage_target_boundaries_and_each_public_limit_bind_before_plan_exists(self):
        config = self._calibrated_config()
        definition = config.stages[StageSide.LEFT]
        coefficient = definition.calibration[LogicalAxis.X].positive.um_per_v
        for request, start, target, accepted in (
            (Vector3Um(-0.2, 0.0, 0.0), 0.1, 0.0, True),
            (Vector3Um(0.1, 0.0, 0.0), 74.9, 75.0, True),
            (Vector3Um(-0.2, 0.0, 0.0), math.nextafter(0.1, -math.inf), math.nextafter(0.1, -math.inf) - 0.1, False),
            (Vector3Um(0.1, 0.0, 0.0), math.nextafter(74.9, math.inf), math.nextafter(75.0, math.inf), False),
        ):
            driver = FakeSetupMDT(
                "COM6", status=make_mdt_status(
                    definition.serial_number,
                    actual_v={Axis.X: 0.0, Axis.Y: start, Axis.Z: 0.0},
                ),
            )
            setup, stage, driver = self._setup(config=config, driver=driver)
            with self.subTest(target=target):
                if accepted:
                    self.assertEqual(self._plan(stage, request).moves[0].target_v, target)
                else:
                    with self.assertRaises(StageMotionLimitError):
                        self._plan(stage, request)
                self._assert_no_setters(driver)
        limit_cases = (
            ("hardware", lambda d: object.__setattr__(d._status, "hardware_limit", type("Limit", (), {"volts": 0.05})())),
            ("application", lambda d: setattr(d, "application_limits_v", MappingProxyType({Axis.X: 75.0, Axis.Y: 0.05, Axis.Z: 75.0}))),
            ("minimum", lambda d: self._replace_axis(d, Axis.Y, actual_v=0.1, minimum_v=0.05)),
            ("maximum", lambda d: self._replace_axis(d, Axis.Y, maximum_v=0.05)),
        )
        for name, constrain in limit_cases:
            with self.subTest(limit=name):
                actual = {Axis.X: 0.0, Axis.Y: 0.1 if name == "minimum" else 0.0, Axis.Z: 0.0}
                setup, stage, driver = self._setup(
                    config=config,
                    driver=FakeSetupMDT("COM6", status=make_mdt_status(definition.serial_number, actual_v=actual)),
                )
                constrain(driver)
                if name == "minimum":
                    request = Vector3Um(-0.2, 0.0, 0.0)
                else:
                    request = Vector3Um(0.1, 0.0, 0.0) if name == "hardware" else Vector3Um(0.1, 0.0, 0.0)
                with self.assertRaises(StageMotionLimitError):
                    self._plan(stage, request)
                self._assert_no_setters(driver)

    def test_bad_derived_values_and_bad_later_axis_leave_zero_setters(self):
        config = self._calibrated_config()
        setup, stage, driver = self._setup(config=config)
        bad_calibration = dict(stage._definition.calibration)
        original = bad_calibration[LogicalAxis.X]
        bad_calibration[LogicalAxis.X] = AxisCalibration(replace(original.positive, um_per_v=float("nan")), original.negative)
        stage._definition = replace(stage._definition, calibration=bad_calibration)
        with self.assertRaises(StageMotionLimitError):
            self._plan(stage, Vector3Um(0.1, 0.0, 0.0))
        self._assert_no_setters(driver)
        setup, stage, driver = self._setup(config=config)
        with self.assertRaises(StageMotionLimitError):
            self._plan(stage, Vector3Um(-0.1, 0.1, math.nextafter(1.0, math.inf)))
        self._assert_no_setters(driver)

    def test_zero_vector_still_performs_fresh_authority_preflight_without_writes(self):
        setup, stage, driver = self._setup()
        before = driver.get_all_calls
        plan = self._plan(stage, Vector3Um(0.0, 0.0, 0.0))
        self.assertEqual(driver.get_all_calls, before + 1)
        self.assertEqual(plan.moves, ())
        self.assertEqual(dict(plan.voltage_delta_v), {})
        self._assert_no_setters(driver)
        unavailable = FiberCouplingSetup(self._calibrated_config(), FiberCouplingSetup.enumerate(self._calibrated_config(), port_enumerator=lambda: []), {})
        self.addCleanup(unavailable.close)
        with unavailable.left._operation_lock:
            with self.assertRaises(StageUnavailableError):
                unavailable.left._plan_move_unlocked(Vector3Um(0.0, 0.0, 0.0))
        unadopted_driver = FakeSetupMDT(
            "COM6", status=make_mdt_status(self._calibrated_config().stages[StageSide.LEFT].serial_number),
        )
        unadopted = FiberCouplingSetup(
            self._calibrated_config(),
            FiberCouplingSetup.enumerate(
                self._calibrated_config(),
                port_enumerator=lambda: [FakePort("COM6", self._calibrated_config().stages[StageSide.LEFT].serial_number)],
            ),
            {StageSide.LEFT: unadopted_driver},
        )
        self.addCleanup(unadopted.close)
        with unadopted.left._operation_lock:
            with self.assertRaises(BaselineUnknownError):
                unadopted.left._plan_move_unlocked(Vector3Um(0.0, 0.0, 0.0))
        self._assert_no_setters(unadopted_driver)

    def test_fresh_query_or_status_authority_uncertainty_invalidates_baseline_truthfully(self):
        for mutation in (
            lambda d: self._replace_axis(d, Axis.Y, actual_v=0.01),
            lambda d: setattr(d, "_status", replace(d.status, axis_command_known=False)),
            lambda d: setattr(d, "_status", replace(d.status, restricted=True)),
            lambda d: setattr(d, "_status", replace(d.status, fault_evidence="fresh fault")),
        ):
            setup, stage, driver = self._setup()
            driver.get_all_mutation = mutation
            with self.subTest(mutation=mutation):
                with self.assertRaises(BaselineUnknownError):
                    self._plan(stage, Vector3Um(0.0, 0.0, 0.0))
                self.assertIsNone(stage._command_baseline_v)
                self.assertIsNone(stage._estimated_position_um)
                self.assertFalse(stage._nominal_authorized)
                self._assert_no_setters(driver)
        for error_type in (Exception, KeyboardInterrupt, SystemExit):
            setup, stage, driver = self._setup()
            original = error_type("fresh read failed")
            driver.get_all_voltages = lambda: (_ for _ in ()).throw(original)
            with self.subTest(error=error_type.__name__):
                with self.assertRaises(BaselineUnknownError) as caught:
                    self._plan(stage, Vector3Um(0.0, 0.0, 0.0))
                self.assertIs(caught.exception.__cause__, original)
                self.assertIsNone(stage._command_baseline_v)
                self._assert_no_setters(driver)
        setup, stage, driver = self._setup()
        original = RuntimeError("fresh status failed")
        driver.status_error = original
        with self.assertRaises(BaselineUnknownError) as caught:
            self._plan(stage, Vector3Um(0.0, 0.0, 0.0))
        self.assertIs(caught.exception.__cause__, original)
        self.assertIsNone(stage._command_baseline_v)
        self._assert_no_setters(driver)

    def test_fresh_return_must_be_complete_finite_and_coherent_with_status_and_baseline(self):
        bad_returns = (
            {Axis.X: 50.0, Axis.Y: 50.0},
            {Axis.X: 50.0, Axis.Y: 50.0, Axis.Z: 50.0, "extra": 50.0},
            {Axis.X: True, Axis.Y: 50.0, Axis.Z: 50.0},
            {Axis.X: float("nan"), Axis.Y: 50.0, Axis.Z: 50.0},
            {Axis.X: "50", Axis.Y: 50.0, Axis.Z: 50.0},
            {Axis.X: 50.0, Axis.Y: 49.0, Axis.Z: 50.0},
        )
        for returned in bad_returns:
            setup, stage, driver = self._setup()
            events = []
            driver.get_all_voltages = lambda returned=returned: (events.append("return"), returned)[1]
            with self.subTest(returned=returned):
                with self.assertRaises(BaselineUnknownError):
                    self._plan(stage, Vector3Um(0.0, 0.0, 0.0))
                self.assertEqual(events, ["return"])
                self.assertGreaterEqual(driver.status_reads, 2)
                self.assertIsNone(stage._command_baseline_v)
                self.assertIsNone(stage._estimated_position_um)
                self.assertFalse(stage._nominal_authorized)
                self._assert_no_setters(driver)

    def test_fresh_return_and_following_status_baseexceptions_are_typed_and_clear_authority(self):
        for phase in ("return", "status"):
            for error_type in (Exception, KeyboardInterrupt, SystemExit):
                setup, stage, driver = self._setup()
                original = error_type(f"fresh {phase} failed")
                if phase == "return":
                    driver.get_all_voltages = lambda original=original: (_ for _ in ()).throw(original)
                else:
                    driver.get_all_voltages = lambda: {axis: 50.0 for axis in Axis}
                    driver.status_error = original
                with self.subTest(phase=phase, error=error_type.__name__):
                    with self.assertRaises(BaselineUnknownError) as caught:
                        self._plan(stage, Vector3Um(0.0, 0.0, 0.0))
                    self.assertIs(caught.exception.__cause__, original)
                    self.assertIsNone(stage._command_baseline_v)
                    self.assertIsNone(stage._estimated_position_um)
                    self.assertFalse(stage._nominal_authorized)
                    self._assert_no_setters(driver)

    def test_nominal_authorization_is_rechecked_before_and_after_fresh_read(self):
        config = self._calibrated_config()
        definition = config.stages[StageSide.LEFT]
        calibration = dict(definition.calibration)
        x = calibration[LogicalAxis.X]
        calibration[LogicalAxis.X] = AxisCalibration(
            replace(x.positive, source="nominal_MAX312D"), x.negative,
        )
        config = replace(config, stages={**config.stages, StageSide.LEFT: replace(definition, calibration=calibration)})
        setup, stage, driver = self._setup(config=config)
        stage._nominal_authorized = False
        before = driver.get_all_calls
        with self.assertRaises(BaselineUnknownError):
            self._plan(stage, Vector3Um(0.1, 0.0, 0.0))
        self.assertEqual(driver.get_all_calls, before)
        self.assertIsNone(stage._command_baseline_v)
        self._assert_no_setters(driver)

        setup, stage, driver = self._setup(config=config)
        driver.get_all_mutation = lambda _: setattr(stage, "_nominal_authorized", False)
        with self.assertRaises(BaselineUnknownError):
            self._plan(stage, Vector3Um(0.1, 0.0, 0.0))
        self.assertEqual(driver.get_all_calls, 1)
        self.assertIsNone(stage._command_baseline_v)
        self._assert_no_setters(driver)

    def test_negative_polarity_uses_signed_delta_and_target_for_all_mappings(self):
        config = self._calibrated_config()
        stages = {
            side: replace(definition, polarity={axis: -1 for axis in LogicalAxis})
            for side, definition in config.stages.items()
        }
        config = replace(config, stages=stages)
        for side in StageSide:
            setup, stage, driver = self._setup(config=config, side=side)
            definition = config.stages[side]
            for logical_axis in LogicalAxis:
                for requested_um, coefficient in (
                    (0.1, definition.calibration[logical_axis].positive),
                    (-0.1, definition.calibration[logical_axis].negative),
                ):
                    request = Vector3Um(
                        requested_um if logical_axis is LogicalAxis.X else 0.0,
                        requested_um if logical_axis is LogicalAxis.Y else 0.0,
                        requested_um if logical_axis is LogicalAxis.Z else 0.0,
                    )
                    move = self._plan(stage, request).moves[0]
                    with self.subTest(side=side, logical_axis=logical_axis, requested_um=requested_um):
                        self.assertEqual(move.controller_axis, definition.axis_map[logical_axis])
                        self.assertEqual(move.delta_v, requested_um / coefficient.um_per_v / -1)
                        self.assertEqual(move.target_v, 50.0 + requested_um / coefficient.um_per_v / -1)
            self._assert_no_setters(driver)

    def test_malformed_public_limits_fail_closed_and_invalidate_authority(self):
        malformed = (
            ("application missing", lambda d: setattr(d, "application_limits_v", {Axis.X: 75.0, Axis.Y: 75.0})),
            ("application nonmapping", lambda d: setattr(d, "application_limits_v", object())),
            ("application bool", lambda d: setattr(d, "application_limits_v", {axis: (True if axis is Axis.Y else 75.0) for axis in Axis})),
            ("application nan", lambda d: setattr(d, "application_limits_v", {axis: (float("nan") if axis is Axis.Y else 75.0) for axis in Axis})),
            ("application text", lambda d: setattr(d, "application_limits_v", {axis: ("75" if axis is Axis.Y else 75.0) for axis in Axis})),
            ("hardware missing", lambda d: object.__setattr__(d._status, "hardware_limit", None)),
            ("hardware malformed", lambda d: object.__setattr__(d._status, "hardware_limit", object())),
            ("hardware bool", lambda d: object.__setattr__(d._status, "hardware_limit", type("Limit", (), {"volts": True})())),
            ("hardware nan", lambda d: object.__setattr__(d._status, "hardware_limit", type("Limit", (), {"volts": float("nan")})())),
            ("axis minimum text", lambda d: object.__setattr__(d._status.axes[Axis.Y], "minimum_v", "0")),
            ("axis maximum nan", lambda d: object.__setattr__(d._status.axes[Axis.Y], "maximum_v", float("nan"))),
        )
        for name, corrupt in malformed:
            setup, stage, driver = self._setup()
            corrupt(driver)
            with self.subTest(name=name):
                with self.assertRaises(BaselineUnknownError):
                    self._plan(stage, Vector3Um(0.0, 0.0, 0.0))
                self.assertIsNone(stage._command_baseline_v)
                self.assertIsNone(stage._estimated_position_um)
                self.assertFalse(stage._nominal_authorized)
                self._assert_no_setters(driver)


class FiberMotionExecutionTests(unittest.TestCase):
    """Public confirmed-motion behavior, including failure and lock boundaries."""

    def setUp(self):
        base = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)
        stages = {}
        number = 1
        for side, definition in base.stages.items():
            calibration = {}
            for axis in LogicalAxis:
                calibration[axis] = AxisCalibration(
                    CalibrationCoefficient(float(number), "measured", None, "positive"),
                    CalibrationCoefficient(float(number + 1), "measured", None, "negative"),
                )
                number += 2
            stages[side] = replace(definition, calibration=calibration)
        self.config = replace(base, stages=stages)

    def _setup(self, *, config=None, sides=(StageSide.LEFT,), drivers=None):
        config = config or self.config
        drivers = dict(drivers or {})
        ports = []
        for side in sides:
            definition = config.stages[side]
            resource = "COM6" if side is StageSide.LEFT else "COM7"
            ports.append(FakePort(resource, definition.serial_number))
            drivers.setdefault(
                side,
                FakeSetupMDT(resource, status=make_mdt_status(
                    definition.serial_number, actual_v={axis: 50.0 for axis in Axis},
                )),
            )
        setup = FiberCouplingSetup(
            config,
            FiberCouplingSetup.enumerate(config, port_enumerator=lambda: ports),
            drivers,
        )
        self.addCleanup(setup.close)
        for side in sides:
            (setup.left if side is StageSide.LEFT else setup.right).adopt_baseline(
                confirm=True, allow_nominal=True,
            )
        return setup, drivers

    def _stage(self, setup, side):
        return setup.left if side is StageSide.LEFT else setup.right

    def _assert_invalid(self, stage):
        status = stage.status
        self.assertFalse(status.baseline_known)
        self.assertIsNone(status.estimated_position_um)
        self.assertFalse(status.nominal_authorized)

    def _assert_motion_evidence(self, error, requested, completed, observed):
        evidence = error.stage_motion_evidence
        self.assertEqual(evidence.requested_um, requested)
        self.assertIsInstance(evidence.completed_moves, tuple)
        self.assertEqual(
            tuple(
                (
                    move.logical_axis,
                    move.controller_axis,
                    move.requested_um,
                    move.coefficient,
                    move.delta_v,
                    move.start_v,
                    move.target_v,
                )
                for move in evidence.completed_moves
            ),
            completed,
        )
        if observed is None:
            self.assertIsNone(evidence.last_observed_voltage_v)
        else:
            self.assertIsInstance(evidence.last_observed_voltage_v, MappingProxyType)
            self.assertEqual(dict(evidence.last_observed_voltage_v), observed)
            with self.assertRaises(TypeError):
                evidence.last_observed_voltage_v[LogicalAxis.X] = 0.0
        with self.assertRaises(FrozenInstanceError):
            evidence.requested_um = Vector3Um(0.0, 0.0, 0.0)

    def test_instrumented_operation_lock_wraps_and_forwards_reentrant_api(self):
        inner = threading.RLock()
        operation_lock = InstrumentedOperationLock(inner)
        self.assertIs(operation_lock._lock, inner)

        self.assertTrue(operation_lock.acquire())
        self.assertTrue(operation_lock.acquire(blocking=False))
        operation_lock.release()
        operation_lock.release()
        self.assertEqual(
            operation_lock.events,
            [
                ("attempt", threading.current_thread().name),
                ("enter", threading.current_thread().name),
                ("attempt", threading.current_thread().name),
                ("enter", threading.current_thread().name),
                ("exit", threading.current_thread().name),
                ("exit", threading.current_thread().name),
            ],
        )

        with operation_lock as entered:
            self.assertIs(entered, operation_lock)

    def test_public_move_confirms_exact_order_evidence_and_accumulates(self):
        setup, drivers = self._setup(sides=(StageSide.LEFT, StageSide.RIGHT))
        for side, requested, order in (
            (StageSide.LEFT, Vector3Um(0.1, -0.2, 0.3), (Axis.X, Axis.Z, Axis.Y)),
            (StageSide.RIGHT, Vector3Um(-0.1, 0.2, 0.3), (Axis.Y, Axis.Z, Axis.X)),
        ):
            stage = self._stage(setup, side)
            driver = drivers[side]
            result = stage.move_by_um(requested.x, requested.y, requested.z)
            with self.subTest(side=side):
                self.assertEqual(tuple(axis for axis, _ in driver.set_calls), order)
                self.assertEqual(driver.get_all_calls, 2)  # plan + final confirmation
                self.assertEqual(result.requested_um, requested)
                self.assertEqual(result.estimated_before_um, Vector3Um(0.0, 0.0, 0.0))
                self.assertEqual(result.estimated_after_um, requested)
                self.assertTrue(result.confirmed)
                self.assertIsInstance(result, MoveResult)
                with self.assertRaises(TypeError):
                    result.observed_voltage_v[LogicalAxis.X] = 0.0
                self.assertEqual(stage.status.estimated_position_um, requested)
            follow = stage.move_by_um(0.0, 0.1, 0.0)
            self.assertEqual(follow.estimated_before_um, requested)
            self.assertEqual(
                follow.estimated_after_um,
                Vector3Um(requested.x, requested.y + 0.1, requested.z),
            )

    def test_zero_vector_confirms_read_only_without_setter_or_estimate_change(self):
        setup, drivers = self._setup()
        stage, driver = setup.left, drivers[StageSide.LEFT]
        before_reads = driver.get_all_calls
        result = stage.move_by_um()
        self.assertEqual(driver.set_calls, [])
        self.assertEqual(driver.get_all_calls, before_reads + 2)
        self.assertEqual(result.estimated_before_um, Vector3Um(0.0, 0.0, 0.0))
        self.assertEqual(result.estimated_after_um, Vector3Um(0.0, 0.0, 0.0))

    def test_public_whole_vector_preflight_rejects_bad_later_axis_without_setter(self):
        setup, drivers = self._setup()
        stage, driver = setup.left, drivers[StageSide.LEFT]
        with self.assertRaises(StageMotionLimitError):
            stage.move_by_um(-0.1, 0.1, math.nextafter(1.0, math.inf))
        self.assertEqual(driver.set_calls, [])
        self.assertTrue(stage.status.baseline_known)

    def test_partial_setter_failure_has_no_rollback_and_invalidates_only_local_side(self):
        original = RuntimeError("second axis failed")
        left = FakeSetupMDT(
            "COM6", status=make_mdt_status(self.config.stages[StageSide.LEFT].serial_number,
                                               actual_v={axis: 50.0 for axis in Axis}),
            set_errors={2: original},
        )
        setup, drivers = self._setup(
            sides=(StageSide.LEFT, StageSide.RIGHT), drivers={StageSide.LEFT: left},
        )
        with self.assertRaises(StageMotionError) as caught:
            setup.left.move_by_um(0.1, -0.1, 0.1)
        self.assertIs(caught.exception.__cause__, original)
        self.assertEqual(len(left.set_calls), 2)
        y_negative = self.config.stages[StageSide.LEFT].calibration[LogicalAxis.Y].negative
        self._assert_motion_evidence(
            caught.exception,
            Vector3Um(0.1, -0.1, 0.1),
            ((
                LogicalAxis.Y, Axis.X, -0.1, y_negative,
                -0.025, 50.0, 49.975,
            ),),
            {LogicalAxis.X: 50.0, LogicalAxis.Y: 49.975, LogicalAxis.Z: 50.0},
        )
        with self.assertRaises(FrozenInstanceError):
            caught.exception.stage_motion_evidence.completed_moves[0].target_v = 0.0
        with self.assertRaises(TypeError):
            caught.exception.stage_motion_evidence.completed_moves[0] = object()
        self._assert_invalid(setup.left)
        self.assertTrue(setup.right.status.baseline_known)
        self.assertEqual(drivers[StageSide.RIGHT].set_calls, [])

    def test_first_setter_failure_preserves_request_and_pre_attempt_observation(self):
        original = RuntimeError("first axis failed")
        driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(
                self.config.stages[StageSide.LEFT].serial_number,
                actual_v={axis: 50.0 for axis in Axis},
            ),
            set_errors={1: original},
        )
        setup, _ = self._setup(drivers={StageSide.LEFT: driver})

        with self.assertRaises(StageMotionError) as caught:
            setup.left.move_by_um(0.1, -0.1, 0.1)

        self.assertIs(caught.exception.__cause__, original)
        self.assertFalse(hasattr(original, "stage_motion_evidence"))
        self._assert_motion_evidence(
            caught.exception,
            Vector3Um(0.1, -0.1, 0.1),
            (),
            {LogicalAxis.X: 50.0, LogicalAxis.Y: 50.0, LogicalAxis.Z: 50.0},
        )

    def test_final_read_failure_preserves_all_confirmed_axes_and_last_observation(self):
        original = RuntimeError("final read failed")
        driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(
                self.config.stages[StageSide.LEFT].serial_number,
                actual_v={axis: 50.0 for axis in Axis},
            ),
            get_all_errors={2: original},
        )
        setup, _ = self._setup(drivers={StageSide.LEFT: driver})

        with self.assertRaises(StageMotionError) as caught:
            setup.left.move_by_um(0.1, -0.1, 0.1)

        definition = self.config.stages[StageSide.LEFT]
        self.assertIs(caught.exception.__cause__, original)
        self._assert_motion_evidence(
            caught.exception,
            Vector3Um(0.1, -0.1, 0.1),
            (
                (
                    LogicalAxis.Y, Axis.X, -0.1,
                    definition.calibration[LogicalAxis.Y].negative,
                    -0.025, 50.0, 49.975,
                ),
                (
                    LogicalAxis.Z, Axis.Z, 0.1,
                    definition.calibration[LogicalAxis.Z].positive,
                    0.02, 50.0, 50.02,
                ),
                (
                    LogicalAxis.X, Axis.Y, 0.1,
                    definition.calibration[LogicalAxis.X].positive,
                    0.1, 50.0, 50.1,
                ),
            ),
            {LogicalAxis.X: 50.1, LogicalAxis.Y: 49.975, LogicalAxis.Z: 50.02},
        )

    def test_keyboard_interrupt_and_system_exit_keep_identity_with_partial_evidence(self):
        definition = self.config.stages[StageSide.LEFT]
        for error_type in (KeyboardInterrupt, SystemExit):
            original = error_type("second axis interrupted")
            driver = FakeSetupMDT(
                "COM6",
                status=make_mdt_status(
                    definition.serial_number,
                    actual_v={axis: 50.0 for axis in Axis},
                ),
                set_errors={2: original},
            )
            setup, _ = self._setup(drivers={StageSide.LEFT: driver})
            with self.subTest(error=error_type.__name__):
                with self.assertRaises(error_type) as caught:
                    setup.left.move_by_um(0.1, -0.1, 0.1)
                self.assertIs(caught.exception, original)
                self._assert_motion_evidence(
                    original,
                    Vector3Um(0.1, -0.1, 0.1),
                    ((
                        LogicalAxis.Y, Axis.X, -0.1,
                        definition.calibration[LogicalAxis.Y].negative,
                        -0.025, 50.0, 49.975,
                    ),),
                    {
                        LogicalAxis.X: 50.0,
                        LogicalAxis.Y: 49.975,
                        LogicalAxis.Z: 50.0,
                    },
                )

    def test_each_setter_requires_full_returned_and_current_working_vector(self):
        def returned_later_axis_drift(_, snapshot):
            return replace(
                snapshot,
                axes={
                    **snapshot.axes,
                    Axis.Z: replace(snapshot.axes[Axis.Z], actual_v=49.0),
                },
            )

        def current_later_axis_drift(fake, *_):
            fake._status = replace(
                fake._status,
                axes={
                    **fake._status.axes,
                    Axis.Z: replace(fake._status.axes[Axis.Z], actual_v=49.0),
                },
            )

        for source, kwargs in (
            ("returned", {"setter_return_mutations": {1: returned_later_axis_drift}}),
            ("current", {"setter_current_mutations": {1: current_later_axis_drift}}),
        ):
            driver = FakeSetupMDT(
                "COM6",
                status=make_mdt_status(
                    self.config.stages[StageSide.LEFT].serial_number,
                    actual_v={axis: 50.0 for axis in Axis},
                ),
                **kwargs,
            )
            setup, _ = self._setup(drivers={StageSide.LEFT: driver})
            with self.subTest(source=source):
                with self.assertRaises(StageMotionError):
                    setup.left.move_by_um(0.1, -0.1, 0.1)
                self.assertEqual(len(driver.set_calls), 1)
                self._assert_invalid(setup.left)

    def test_underlying_stage_motion_error_is_wrapped_as_this_operation_cause(self):
        original = StageMotionError("underlying motion evidence")
        driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(
                self.config.stages[StageSide.LEFT].serial_number,
                actual_v={axis: 50.0 for axis in Axis},
            ),
            set_errors={1: original},
        )
        setup, _ = self._setup(drivers={StageSide.LEFT: driver})
        with self.assertRaises(StageMotionError) as caught:
            setup.left.move_by_um(0.1, 0.0, 0.0)
        self.assertIsNot(caught.exception, original)
        self.assertIs(caught.exception.__cause__, original)
        self._assert_invalid(setup.left)

    def test_returned_or_current_health_and_mapping_fail_closed_before_next_setter(self):
        def malformed_return(_, snapshot):
            return replace(snapshot, axes={Axis.X: snapshot.axes[Axis.X]})

        def nonfinite_return(_, snapshot):
            bad_axis = type("BadAxis", (), {"actual_v": float("nan")})()
            return replace(snapshot, axes={**snapshot.axes, Axis.Z: bad_axis})

        def current_restricted(fake, *_):
            fake._status = replace(fake._status, restricted=True)

        def current_faulted(fake, *_):
            fake._status = replace(fake._status, fault_evidence="post-set fault")

        def current_unknown(fake, *_):
            fake._status = replace(fake._status, axis_command_known=False)

        cases = (
            ("returned incomplete", {"setter_return_mutations": {1: malformed_return}}),
            ("returned nonfinite", {"setter_return_mutations": {1: nonfinite_return}}),
            ("current restricted", {"setter_current_mutations": {1: current_restricted}}),
            ("current fault", {"setter_current_mutations": {1: current_faulted}}),
            ("current authority", {"setter_current_mutations": {1: current_unknown}}),
        )
        for name, kwargs in cases:
            driver = FakeSetupMDT(
                "COM6", status=make_mdt_status(
                    self.config.stages[StageSide.LEFT].serial_number,
                    actual_v={axis: 50.0 for axis in Axis},
                ), **kwargs,
            )
            setup, _ = self._setup(drivers={StageSide.LEFT: driver})
            with self.subTest(name=name):
                with self.assertRaises(StageMotionError):
                    setup.left.move_by_um(0.1, -0.1, 0.1)
                self.assertEqual(len(driver.set_calls), 1)
                driver.status_error = None
                self._assert_invalid(setup.left)

    def test_returned_and_current_status_failure_matrix_is_symmetric(self):
        def returned_restricted(_, snapshot):
            return replace(snapshot, restricted=True)

        def returned_faulted(_, snapshot):
            return replace(snapshot, fault_evidence="returned fault")

        def returned_unknown(_, snapshot):
            return replace(snapshot, axis_command_known=False)

        def current_incomplete(fake, *_):
            fake._status = replace(fake._status, axes={Axis.X: fake._status.axes[Axis.X]})

        def current_nonfinite(fake, *_):
            bad_axis = type("BadAxis", (), {"actual_v": float("nan")})()
            fake._status = replace(fake._status, axes={**fake._status.axes, Axis.Z: bad_axis})

        cases = (
            ("returned restricted", {"setter_return_mutations": {1: returned_restricted}}),
            ("returned fault", {"setter_return_mutations": {1: returned_faulted}}),
            ("returned authority", {"setter_return_mutations": {1: returned_unknown}}),
            ("current incomplete", {"setter_current_mutations": {1: current_incomplete}}),
            ("current nonfinite", {"setter_current_mutations": {1: current_nonfinite}}),
        )
        for name, kwargs in cases:
            driver = FakeSetupMDT(
                "COM6", status=make_mdt_status(
                    self.config.stages[StageSide.LEFT].serial_number,
                    actual_v={axis: 50.0 for axis in Axis},
                ), **kwargs,
            )
            setup, _ = self._setup(drivers={StageSide.LEFT: driver})
            with self.subTest(name=name):
                with self.assertRaises(StageMotionError):
                    setup.left.move_by_um(0.1, -0.1, 0.1)
                self.assertEqual(len(driver.set_calls), 1)
                self.assertIsNone(setup.left._command_baseline_v)
                self.assertIsNone(setup.left._estimated_position_um)
                self.assertFalse(setup.left._nominal_authorized)

    def test_post_setter_status_baseexceptions_keep_identity_and_invalidate(self):
        for error_type in (Exception, KeyboardInterrupt, SystemExit):
            original = error_type("current status failed")
            driver = FakeSetupMDT(
                "COM6", status=make_mdt_status(
                    self.config.stages[StageSide.LEFT].serial_number,
                    actual_v={axis: 50.0 for axis in Axis},
                ), setter_current_errors={1: original},
            )
            setup, _ = self._setup(drivers={StageSide.LEFT: driver})
            expected = error_type if error_type in (KeyboardInterrupt, SystemExit) else StageMotionError
            with self.subTest(error=error_type.__name__):
                with self.assertRaises(expected) as caught:
                    setup.left.move_by_um(0.1, 0.0, 0.0)
                if expected is StageMotionError:
                    self.assertIs(caught.exception.__cause__, original)
                else:
                    self.assertIs(caught.exception, original)
                    self.assertTrue(hasattr(original, "stage_fault_evidence"))
                self._assert_motion_evidence(
                    caught.exception,
                    Vector3Um(0.1, 0.0, 0.0),
                    (),
                    {
                        LogicalAxis.X: 50.1,
                        LogicalAxis.Y: 50.0,
                        LogicalAxis.Z: 50.0,
                    },
                )
                driver.status_error = None
                self._assert_invalid(setup.left)

    def test_invalid_current_status_retains_valid_returned_observation(self):
        def drift_current(fake, axis, target):
            fake._status = replace(
                fake._status,
                axes={
                    **fake._status.axes,
                    axis: replace(fake._status.axes[axis], actual_v=target + 0.01),
                },
            )

        driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(
                self.config.stages[StageSide.LEFT].serial_number,
                actual_v={axis: 50.0 for axis in Axis},
            ),
            setter_current_mutations={1: drift_current},
        )
        setup, _ = self._setup(drivers={StageSide.LEFT: driver})

        with self.assertRaises(StageMotionError) as caught:
            setup.left.move_by_um(0.1, 0.0, 0.0)

        self.assertEqual(len(driver.set_calls), 1)
        self._assert_motion_evidence(
            caught.exception,
            Vector3Um(0.1, 0.0, 0.0),
            (),
            {
                LogicalAxis.X: 50.1,
                LogicalAxis.Y: 50.0,
                LogicalAxis.Z: 50.0,
            },
        )
        self._assert_invalid(setup.left)

    def test_coherence_failure_retains_latest_valid_current_observation(self):
        returned_x = 50.1 - 0.9e-6
        current_x = 50.1 + 0.9e-6

        def offset_returned(_, snapshot):
            return replace(
                snapshot,
                axes={
                    **snapshot.axes,
                    Axis.Y: replace(snapshot.axes[Axis.Y], actual_v=returned_x),
                },
            )

        def offset_current(fake, axis, _):
            fake._status = replace(
                fake._status,
                axes={
                    **fake._status.axes,
                    axis: replace(fake._status.axes[axis], actual_v=current_x),
                },
            )

        driver = FakeSetupMDT(
            "COM6",
            status=make_mdt_status(
                self.config.stages[StageSide.LEFT].serial_number,
                actual_v={axis: 50.0 for axis in Axis},
            ),
            setter_return_mutations={1: offset_returned},
            setter_current_mutations={1: offset_current},
        )
        setup, _ = self._setup(drivers={StageSide.LEFT: driver})

        with self.assertRaises(StageMotionError) as caught:
            setup.left.move_by_um(0.1, 0.0, 0.0)

        self.assertEqual(len(driver.set_calls), 1)
        self._assert_motion_evidence(
            caught.exception,
            Vector3Um(0.1, 0.0, 0.0),
            (),
            {
                LogicalAxis.X: current_x,
                LogicalAxis.Y: 50.0,
                LogicalAxis.Z: 50.0,
            },
        )
        self._assert_invalid(setup.left)

    def test_timeout_ignored_and_clamped_setters_are_unconfirmed(self):
        def replace_current(fake, axis, value):
            fake._status = replace(
                fake._status,
                axes={
                    **fake._status.axes,
                    axis: replace(fake._status.axes[axis], actual_v=value),
                },
            )

        cases = (
            ("timeout", {"set_errors": {1: TimeoutError("setter timed out")}}),
            ("ignored", {"setter_mutation": lambda fake, axis, _: replace_current(fake, axis, 50.0)}),
            ("clamped", {"setter_mutation": lambda fake, axis, target: replace_current(fake, axis, target - 0.01)}),
        )
        for name, kwargs in cases:
            driver = FakeSetupMDT(
                "COM6", status=make_mdt_status(
                    self.config.stages[StageSide.LEFT].serial_number,
                    actual_v={axis: 50.0 for axis in Axis},
                ), **kwargs,
            )
            setup, _ = self._setup(drivers={StageSide.LEFT: driver})
            with self.subTest(name=name):
                with self.assertRaises(StageMotionError):
                    setup.left.move_by_um(0.1, 0.0, 0.0)
                self.assertEqual(len(driver.set_calls), 1)
                self._assert_invalid(setup.left)

    def test_final_query_and_status_baseexceptions_keep_identity(self):
        for phase in ("query", "status"):
            for error_type in (KeyboardInterrupt, SystemExit):
                original = error_type(f"final {phase} interrupted")
                driver = FakeSetupMDT(
                    "COM6", status=make_mdt_status(
                        self.config.stages[StageSide.LEFT].serial_number,
                        actual_v={axis: 50.0 for axis in Axis},
                    ),
                )
                if phase == "query":
                    driver.get_all_errors = {2: original}
                else:
                    driver.get_all_mutation = lambda fake, original=original: (
                        setattr(fake, "status_error", original)
                        if fake.get_all_calls == 2 else None
                    )
                setup, _ = self._setup(drivers={StageSide.LEFT: driver})
                with self.subTest(phase=phase, error=error_type.__name__):
                    with self.assertRaises(error_type) as caught:
                        setup.left.move_by_um(0.1, 0.0, 0.0)
                    self.assertIs(caught.exception, original)
                    self.assertTrue(hasattr(original, "stage_fault_evidence"))
                    driver.status_error = None
                    self._assert_invalid(setup.left)

    def test_uncertain_final_confirmation_and_baseexceptions_invalidate_without_replacement(self):
        cases = (
            ("final read", lambda d, error: setattr(d, "get_all_errors", {2: error}), RuntimeError),
            ("setter keyboard", lambda d, error: setattr(d, "set_errors", {1: error}), KeyboardInterrupt),
            ("setter exit", lambda d, error: setattr(d, "set_errors", {1: error}), SystemExit),
            ("authority", lambda d, error: setattr(
                d, "setter_mutation", lambda fake, *_: setattr(
                    fake, "_status", replace(fake.status, axis_command_known=False),
                )
            ), None),
        )
        for name, configure, error_type in cases:
            setup, drivers = self._setup()
            stage, driver = setup.left, drivers[StageSide.LEFT]
            original = error_type(name) if error_type is not None else None
            configure(driver, original)
            with self.subTest(name=name):
                expected = error_type if error_type in (KeyboardInterrupt, SystemExit) else StageMotionError
                with self.assertRaises(expected) as caught:
                    stage.move_by_um(0.1, 0.0, 0.0)
                if original is not None and expected is StageMotionError:
                    self.assertIs(caught.exception.__cause__, original)
                if original is not None and expected is not StageMotionError:
                    self.assertIs(caught.exception, original)
                self.assertGreaterEqual(len(driver.set_calls), 1)
                self._assert_invalid(stage)

    def test_mismatched_final_mapping_invalidates_after_attempt(self):
        setup, drivers = self._setup()
        stage, driver = setup.left, drivers[StageSide.LEFT]
        driver.get_all_mutation = lambda fake: (
            setattr(
                fake, "_status", replace(
                    fake.status,
                    axes={**fake.status.axes, Axis.Y: replace(fake.status.axes[Axis.Y], actual_v=49.0)},
                ),
            ) if fake.get_all_calls == 2 else None
        )
        with self.assertRaises(StageMotionError):
            stage.move_by_um(0.1, 0.0, 0.0)
        self.assertEqual(len(driver.set_calls), 1)
        self._assert_invalid(stage)

    def test_close_intent_after_first_axis_prevents_later_axes(self):
        gate, entered = threading.Event(), threading.Event()
        driver = FakeSetupMDT(
            "COM6", status=make_mdt_status(self.config.stages[StageSide.LEFT].serial_number,
                                               actual_v={axis: 50.0 for axis in Axis}),
            setter_gate=gate, setter_entered=entered,
        )
        setup, _ = self._setup(drivers={StageSide.LEFT: driver})
        errors = []
        worker = threading.Thread(
            target=lambda: _record_error(lambda: setup.left.move_by_um(0.1, -0.1, 0.1), errors),
        )
        worker.start()
        self.assertTrue(entered.wait(timeout=1.0))
        closer = threading.Thread(target=lambda: _record_error(setup.close, []))
        closer.start()
        self.assertTrue(setup._close_intent.wait(timeout=1.0))
        gate.set()
        worker.join(timeout=1.0)
        closer.join(timeout=1.0)
        self.assertFalse(worker.is_alive())
        self.assertFalse(closer.is_alive())
        self.assertEqual(len(driver.set_calls), 1)
        self.assertIsInstance(errors[0], StageMotionError)
        self._assert_invalid(setup.left)

    def test_same_side_operations_serialize_while_independent_sides_can_enter(self):
        gate, entered = threading.Event(), threading.Event()
        left = FakeSetupMDT(
            "COM6", status=make_mdt_status(self.config.stages[StageSide.LEFT].serial_number,
                                               actual_v={axis: 50.0 for axis in Axis}),
            setter_gate=gate, setter_entered=entered,
        )
        right_gate, right_entered = threading.Event(), threading.Event()
        right = FakeSetupMDT(
            "COM7", status=make_mdt_status(self.config.stages[StageSide.RIGHT].serial_number,
                                               actual_v={axis: 50.0 for axis in Axis}),
            setter_gate=right_gate, setter_entered=right_entered,
        )
        setup, _ = self._setup(sides=(StageSide.LEFT, StageSide.RIGHT), drivers={
            StageSide.LEFT: left, StageSide.RIGHT: right,
        })
        first = threading.Thread(target=lambda: setup.left.move_by_um(-0.1, 0.0, 0.0))
        queued = threading.Thread(target=lambda: setup.left.move_by_um(-0.1, 0.0, 0.0))
        independent = threading.Thread(target=lambda: setup.right.move_by_um(0.1, 0.0, 0.0))
        first.start()
        self.assertTrue(entered.wait(timeout=1.0))
        queued.start()
        independent.start()
        self.assertTrue(right_entered.wait(timeout=1.0))
        self.assertEqual(len(left.set_calls), 1)
        gate.set()
        right_gate.set()
        for thread in (first, queued, independent):
            thread.join(timeout=1.0)
            self.assertFalse(thread.is_alive())
        self.assertEqual(len(left.set_calls), 2)

    def test_two_distinguishable_three_axis_same_side_moves_do_not_interleave(self):
        gate, entered, events = threading.Event(), threading.Event(), []
        driver = FakeSetupMDT(
            "COM6", status=make_mdt_status(
                self.config.stages[StageSide.LEFT].serial_number,
                actual_v={axis: 50.0 for axis in Axis},
            ), setter_gate=gate, setter_entered=entered, setter_event_log=events,
        )
        setup, _ = self._setup(drivers={StageSide.LEFT: driver})
        original_lock = setup.left._operation_lock
        operation_lock = InstrumentedOperationLock(original_lock)
        self.assertIs(operation_lock._lock, original_lock)
        setup.left._operation_lock = operation_lock
        first_errors, second_errors = [], []
        first = threading.Thread(
            name="first-move",
            target=lambda: _record_error(
                lambda: setup.left.move_by_um(0.1, -0.1, 0.1), first_errors,
            ),
        )
        second = threading.Thread(
            name="second-move",
            target=lambda: _record_error(
                lambda: setup.left.move_by_um(-0.1, 0.1, -0.1), second_errors,
            ),
        )
        first.start()
        self.assertTrue(entered.wait(timeout=1.0))
        second.start()
        self.assertTrue(operation_lock.wait_attempt("second-move"))
        self.assertFalse(operation_lock.has_entered("second-move"))
        self.assertEqual(len(events), 1)
        gate.set()
        for worker in (first, second):
            worker.join(timeout=1.0)
            self.assertFalse(worker.is_alive())
        self.assertEqual(first_errors, [])
        self.assertEqual(second_errors, [])
        self.assertEqual(
            [entry[0] for entry in events],
            ["first-move"] * 3 + ["second-move"] * 3,
        )
        self.assertEqual(
            [(axis, target) for _, axis, target in events],
            [
                (Axis.X, 49.975), (Axis.Z, 50.02), (Axis.Y, 50.1),
                (Axis.Y, 50.1 - 0.1 / 2), (Axis.X, 49.975 + 0.1 / 3),
                (Axis.Z, 50.02 - 0.1 / 6),
            ],
        )

    def test_active_and_queued_moves_close_truthfully_without_queued_io(self):
        for name in ("success", "close error", "retained open"):
            injected_close_error = RuntimeError("close failed") if name == "close error" else None
            driver_kwargs = {
                "close_error": injected_close_error,
                "retain_open": name == "retained open",
            }
            gate, entered = threading.Event(), threading.Event()
            close_lock_attempted = threading.Event()
            close_lock_acquired = threading.Event()
            driver = FakeSetupMDT(
                "COM6", status=make_mdt_status(
                    self.config.stages[StageSide.LEFT].serial_number,
                    actual_v={axis: 50.0 for axis in Axis},
                ), setter_gate=gate, setter_entered=entered, **driver_kwargs,
            )
            setup, _ = self._setup(drivers={StageSide.LEFT: driver})
            original_request_close = setup.left._request_close

            def instrumented_request_close():
                close_lock_attempted.set()
                original_request_close()
                close_lock_acquired.set()

            setup.left._request_close = instrumented_request_close
            original_lock = setup.left._operation_lock
            operation_lock = InstrumentedOperationLock(original_lock)
            self.assertIs(operation_lock._lock, original_lock)
            setup.left._operation_lock = operation_lock
            active_errors, queued_errors, close_errors = [], [], []
            active = threading.Thread(name="active-move", target=lambda: _record_error(
                lambda: setup.left.move_by_um(0.1, -0.1, 0.1), active_errors,
            ))
            queued = threading.Thread(name="queued-move", target=lambda: _record_error(
                lambda: setup.left.move_by_um(-0.1, 0.1, -0.1), queued_errors,
            ))
            active.start()
            self.assertTrue(entered.wait(timeout=1.0))
            queued.start()
            self.assertTrue(operation_lock.wait_attempt("queued-move"))
            self.assertFalse(operation_lock.has_entered("queued-move"))
            closer = threading.Thread(target=lambda: _record_error(setup.close, close_errors))
            closer.start()
            self.assertTrue(setup._close_intent.wait(timeout=1.0))
            self.assertTrue(close_lock_attempted.wait(timeout=1.0))
            self.assertFalse(close_lock_acquired.is_set())
            self.assertEqual(driver.close_calls, 0)
            gate.set()
            self.assertTrue(close_lock_acquired.wait(timeout=1.0))
            for worker in (active, queued, closer):
                worker.join(timeout=1.0)
                self.assertFalse(worker.is_alive())
            with self.subTest(name=name):
                self.assertEqual(len(driver.set_calls), 1)
                self.assertEqual(len(active_errors), 1)
                self.assertIsInstance(active_errors[0], StageMotionError)
                self.assertIsInstance(active_errors[0].__cause__, StageConnectionError)
                self.assertEqual(len(queued_errors), 1)
                self.assertIsInstance(queued_errors[0], StageConnectionError)
                self._assert_invalid(setup.left)
                self.assertEqual(driver.close_calls, 1)
                if name == "success":
                    self.assertEqual(close_errors, [])
                    self.assertEqual(setup.available_sides, frozenset())
                    self.assertFalse(setup.left.status.available)
                    self.assertFalse(driver.is_open)
                    self.assertEqual(driver.state, "DISCONNECTED")
                else:
                    self.assertEqual(len(close_errors), 1)
                    self.assertIsInstance(close_errors[0], StageConnectionError)
                    if injected_close_error is not None:
                        self.assertIs(close_errors[0].__cause__, injected_close_error)
                        self.assertEqual(str(injected_close_error), "close failed")
                    self.assertEqual(setup.available_sides, frozenset({StageSide.LEFT}))
                    self.assertTrue(setup.left.status.available)
                    self.assertTrue(driver.is_open)
                    driver.close_error = None
                    driver.retain_open = False
                    setup.close()
                    self.assertEqual(driver.close_calls, 2)
                    self.assertEqual(setup.available_sides, frozenset())
                    self.assertFalse(setup.left.status.available)
                    self.assertFalse(driver.is_open)
                    self.assertEqual(driver.state, "DISCONNECTED")

    def test_exact_move_result_evidence_and_nominal_authority_are_preserved_on_success(self):
        setup, drivers = self._setup()
        stage, driver = setup.left, drivers[StageSide.LEFT]
        result = stage.move_by_um(0.1, -0.1, 0.1)
        calibration = self.config.stages[StageSide.LEFT].calibration
        self.assertEqual(
            dict(result.voltage_delta_v),
            {LogicalAxis.Y: -0.025, LogicalAxis.Z: 0.02, LogicalAxis.X: 0.1},
        )
        self.assertEqual(
            dict(result.calibration_used),
            {
                LogicalAxis.Y: calibration[LogicalAxis.Y].negative,
                LogicalAxis.Z: calibration[LogicalAxis.Z].positive,
                LogicalAxis.X: calibration[LogicalAxis.X].positive,
            },
        )
        self.assertEqual(
            dict(result.observed_voltage_v),
            {LogicalAxis.X: 50.1, LogicalAxis.Y: 49.975, LogicalAxis.Z: 50.02},
        )
        self.assertEqual(result.estimated_before_um, Vector3Um(0.0, 0.0, 0.0))
        self.assertEqual(result.estimated_after_um, Vector3Um(0.1, -0.1, 0.1))
        self.assertTrue(result.confirmed)
        with self.assertRaises(TypeError):
            result.voltage_delta_v[LogicalAxis.X] = 0.0
        with self.assertRaises(TypeError):
            result.calibration_used[LogicalAxis.X] = calibration[LogicalAxis.X].positive
        with self.assertRaises(TypeError):
            result.observed_voltage_v[LogicalAxis.X] = 0.0
        with self.assertRaises(FrozenInstanceError):
            result.confirmed = False
        follow = stage.move_by_um(-0.1, 0.1, -0.1)
        self.assertEqual(follow.estimated_before_um, result.estimated_after_um)
        self.assertEqual(follow.estimated_after_um, Vector3Um(0.0, 0.0, 0.0))

        nominal = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)
        nominal_driver = FakeSetupMDT(
            "COM6", status=make_mdt_status(
                nominal.stages[StageSide.LEFT].serial_number,
                actual_v={axis: 50.0 for axis in Axis},
            ),
        )
        nominal_setup, _ = self._setup(config=nominal, drivers={StageSide.LEFT: nominal_driver})
        nominal_setup.left.move_by_um(0.1, 0.0, 0.0)
        self.assertTrue(nominal_setup.left.status.nominal_authorized)

    def test_right_toward_and_away_moves_use_exact_identity_controller_targets(self):
        setup, drivers = self._setup(sides=(StageSide.RIGHT,))
        stage, driver = setup.right, drivers[StageSide.RIGHT]
        definition = self.config.stages[StageSide.RIGHT]
        first = stage.move_by_um(-0.1, 0.1, -0.1)  # Toward: Y/Z/X.
        second = stage.move_by_um(0.1, -0.1, 0.1)  # Away: X/Y/Z.
        x, y, z = definition.calibration[LogicalAxis.X], definition.calibration[LogicalAxis.Y], definition.calibration[LogicalAxis.Z]
        expected = (
            (Axis.Y, 50.0 + 0.1 / y.positive.um_per_v),
            (Axis.Z, 50.0 - 0.1 / z.negative.um_per_v),
            (Axis.X, 50.0 - 0.1 / x.negative.um_per_v),
            (Axis.X, 50.0 - 0.1 / x.negative.um_per_v + 0.1 / x.positive.um_per_v),
            (Axis.Y, 50.0 + 0.1 / y.positive.um_per_v - 0.1 / y.negative.um_per_v),
            (Axis.Z, 50.0 - 0.1 / z.negative.um_per_v + 0.1 / z.positive.um_per_v),
        )
        self.assertEqual(tuple(driver.set_calls), expected)
        self.assertEqual(first.requested_um, Vector3Um(-0.1, 0.1, -0.1))
        self.assertEqual(second.requested_um, Vector3Um(0.1, -0.1, 0.1))

    def test_publication_failure_invalidates_even_after_final_assignments(self):
        for error_type in (Exception, KeyboardInterrupt, SystemExit):
            original = error_type("publication failed")
            setup, _ = self._setup()
            stage = setup.left
            stage._publication_lock = FailingPublicationLock(original, fail_on_entry=2)
            expected = error_type if error_type in (KeyboardInterrupt, SystemExit) else StageMotionError
            with self.subTest(error=error_type.__name__):
                with self.assertRaises(expected) as caught:
                    stage.move_by_um(0.1, 0.0, 0.0)
                if expected is StageMotionError:
                    self.assertIs(caught.exception.__cause__, original)
                    evidence_owner = caught.exception
                else:
                    self.assertIs(caught.exception, original)
                    evidence_owner = original
                self.assertEqual(
                    getattr(evidence_owner, "stage_fault_evidence", None),
                    "left stage motion authority invalidated",
                )
                self._assert_invalid(stage)

    def test_exception_evidence_is_local_without_mutating_underlying_causes(self):
        plain = RuntimeError("plain underlying")
        nested = ValueError("nested underlying")
        seeded = StageMotionError("seeded underlying")
        seeded.__cause__ = nested
        seeded.stage_fault_evidence = "underlying evidence"
        seeded.stage_motion_evidence = "underlying motion evidence"
        for original in (plain, seeded):
            driver = FakeSetupMDT(
                "COM6", status=make_mdt_status(
                    self.config.stages[StageSide.LEFT].serial_number,
                    actual_v={axis: 50.0 for axis in Axis},
                ), set_errors={1: original},
            )
            setup, _ = self._setup(drivers={StageSide.LEFT: driver})
            with self.subTest(original=type(original).__name__):
                with self.assertRaises(StageMotionError) as caught:
                    setup.left.move_by_um(0.1, 0.0, 0.0)
                self.assertIs(caught.exception.__cause__, original)
                self.assertEqual(
                    caught.exception.stage_fault_evidence,
                    "left stage motion authority invalidated",
                )
                if original is plain:
                    self.assertFalse(hasattr(plain, "stage_fault_evidence"))
                    self.assertFalse(hasattr(plain, "stage_motion_evidence"))
                else:
                    self.assertEqual(seeded.stage_fault_evidence, "underlying evidence")
                    self.assertEqual(seeded.stage_motion_evidence, "underlying motion evidence")
                    self.assertIs(seeded.__cause__, nested)


def _record_error(operation, errors):
    try:
        operation()
    except BaseException as error:
        errors.append(error)


class FiberDiscoveryTests(unittest.TestCase):
    def setUp(self):
        self.config = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)
        self.left_serial = self.config.stages[StageSide.LEFT].serial_number
        self.right_serial = self.config.stages[StageSide.RIGHT].serial_number

    def _factory(self, **definitions):
        return FakeMDTFactory(definitions)

    def _connect(self, *, factory, ports):
        setup = FiberCouplingSetup.connect(
            config=self.config,
            port_enumerator=lambda: ports,
            driver_factory=factory,
        )
        self.addCleanup(self._assert_factory_closed, factory)
        self.addCleanup(setup.close)
        return setup

    def _assert_factory_closed(self, factory):
        for instance in factory.instances.values():
            self.assertFalse(getattr(instance, "is_open", False))
            self.assertEqual(getattr(instance, "state", "DISCONNECTED"), "DISCONNECTED")

    def test_left_only_binds_left_by_exact_serial_and_right_is_unavailable(self):
        factory = self._factory(COM6={"status": make_mdt_status(self.left_serial)})
        setup = self._connect(factory=factory, ports=[FakePort("COM6", self.left_serial)])
        self.assertTrue(setup.left.status.available)
        self.assertFalse(setup.right.status.available)
        self.assertEqual(setup.available_sides, frozenset({StageSide.LEFT}))
        with self.assertRaises(StageUnavailableError):
            setup.right.adopt_baseline(confirm=True, allow_nominal=True)
        self.assertEqual(factory.instances["COM6"].set_calls, [])

    def test_right_only_binds_right_by_exact_serial(self):
        factory = self._factory(COM7={"status": make_mdt_status(self.right_serial)})
        setup = self._connect(factory=factory, ports=[FakePort("COM7", self.right_serial)])
        self.assertFalse(setup.left.status.available)
        self.assertTrue(setup.right.status.available)
        self.assertEqual(setup.missing_sides, frozenset({StageSide.LEFT}))

    def test_both_present_bind_by_serial_independent_of_com_order(self):
        factory = self._factory(
            COM2={"status": make_mdt_status(self.right_serial)},
            COM9={"status": make_mdt_status(self.left_serial)},
        )
        setup = self._connect(
            factory=factory,
            ports=[FakePort("COM2", self.right_serial), FakePort("COM9", self.left_serial)],
        )
        self.assertEqual(setup.left.status.resource, "COM9")
        self.assertEqual(setup.right.status.resource, "COM2")

    def test_neither_present_rejects_connect_without_constructing_driver(self):
        factory = self._factory()
        with self.assertRaises(StageConnectionError):
            FiberCouplingSetup.connect(
                config=self.config, port_enumerator=lambda: [], driver_factory=factory
            )
        self.assertEqual(factory.instances, {})

    def test_unknown_alongside_known_is_sorted_and_never_opened(self):
        factory = self._factory(COM6={"status": make_mdt_status(self.left_serial)})
        setup = self._connect(
            factory=factory,
            ports=[
                FakePort("COM8", "ZZZ"), FakePort("COM6", self.left_serial), FakePort("COM3", "AAA"),
            ],
        )
        self.assertEqual(
            setup.unknown_devices,
            (
                SerialDeviceInfo("COM3", "AAA", "Thorlabs MDT693B"),
                SerialDeviceInfo("COM8", "ZZZ", "Thorlabs MDT693B"),
            ),
        )
        self.assertEqual(set(factory.instances), {"COM6"})

    def test_unknown_only_rejects_without_constructing_driver(self):
        factory = self._factory()
        with self.assertRaises(StageConnectionError):
            FiberCouplingSetup.connect(
                config=self.config,
                port_enumerator=lambda: [FakePort("COM8", "unknown")],
                driver_factory=factory,
            )
        self.assertEqual(factory.instances, {})

    def test_duplicate_registered_serial_metadata_rejects_before_construction(self):
        factory = self._factory()
        with self.assertRaises(StageConnectionError):
            FiberCouplingSetup.connect(
                config=self.config,
                port_enumerator=lambda: [FakePort("COM4", self.left_serial), FakePort("COM6", self.left_serial)],
                driver_factory=factory,
            )
        self.assertEqual(factory.instances, {})

    def test_enumerate_rejects_registered_sides_sharing_one_canonical_resource(self):
        for alias in (
            "COM10",
            "com10",
            "  CoM10  ",
            r"\\.\COM10",
            r"  \\.\cOm10  ",
        ):
            with self.subTest(alias=alias):
                with self.assertRaises(StageConnectionError):
                    FiberCouplingSetup.enumerate(
                        self.config,
                        port_enumerator=lambda alias=alias: [
                            FakePort("COM10", self.left_serial),
                            FakePort(alias, self.right_serial),
                        ],
                    )

    def test_connect_rejects_canonical_resource_aliases_before_driver_construction(self):
        for alias in (
            "COM10",
            "com10",
            " COM10 ",
            r"\\.\COM10",
            r"  \\.\cOm10  ",
        ):
            constructed = []

            def factory(resource):
                constructed.append(resource)
                raise AssertionError("driver construction must not occur")

            with self.subTest(alias=alias):
                with self.assertRaises(StageConnectionError):
                    FiberCouplingSetup.connect(
                        config=self.config,
                        port_enumerator=lambda alias=alias: [
                            FakePort("COM10", self.left_serial),
                            FakePort(alias, self.right_serial),
                        ],
                        driver_factory=factory,
                    )
                self.assertEqual(constructed, [])

    def test_non_com_windows_namespace_resources_remain_distinct(self):
        for direct, namespaced in (
            ("GPIB0", r"\\.\GPIB0"),
            ("COMX", r"\\.\COMX"),
            ("COM10-extra", r"\\.\COM10-extra"),
            ("GPIB0", "gpib0"),
            ("USB0", " USB0 "),
        ):
            ports = [
                FakePort(direct, self.left_serial),
                FakePort(namespaced, self.right_serial),
            ]
            with self.subTest(direct=direct, namespaced=namespaced):
                discovery = FiberCouplingSetup.enumerate(
                    self.config, port_enumerator=lambda ports=ports: ports,
                )
                self.assertEqual(discovery.registered[StageSide.LEFT].resource, direct)
                self.assertEqual(discovery.registered[StageSide.RIGHT].resource, namespaced)

                factory = FakeMDTFactory({
                    direct: {"status": make_mdt_status(self.left_serial)},
                    namespaced: {"status": make_mdt_status(self.right_serial)},
                })
                setup = FiberCouplingSetup.connect(
                    config=self.config,
                    port_enumerator=lambda ports=ports: ports,
                    driver_factory=factory,
                )
                try:
                    self.assertEqual(set(factory.instances), {direct, namespaced})
                finally:
                    setup.close()

    def test_identity_mismatch_after_connect_closes_created_driver(self):
        factory = self._factory(COM6={"status": make_mdt_status(self.right_serial)})
        with self.assertRaises(StageConnectionError):
            FiberCouplingSetup.connect(
                config=self.config,
                port_enumerator=lambda: [FakePort("COM6", self.left_serial)],
                driver_factory=factory,
            )
        self.assertEqual(factory.instances["COM6"].close_calls, 1)

    def test_connection_failure_closes_earlier_registered_side(self):
        factory = self._factory(
            COM6={"status": make_mdt_status(self.left_serial)},
            COM7={"status": make_mdt_status(self.right_serial), "connect_error": RuntimeError("connect")},
        )
        with self.assertRaises(StageConnectionError):
            FiberCouplingSetup.connect(
                config=self.config,
                port_enumerator=lambda: [FakePort("COM6", self.left_serial), FakePort("COM7", self.right_serial)],
                driver_factory=factory,
            )
        self.assertEqual(factory.instances["COM6"].close_calls, 1)
        self.assertEqual(factory.instances["COM7"].close_calls, 1)

    def test_context_body_exception_stays_primary_when_both_closes_fail(self):
        factory = self._factory(
            COM6={"status": make_mdt_status(self.left_serial), "close_error": RuntimeError("left close")},
            COM7={"status": make_mdt_status(self.right_serial), "close_error": RuntimeError("right close")},
        )
        body_error = KeyboardInterrupt("body")
        with self.assertRaises(KeyboardInterrupt) as caught:
            with FiberCouplingSetup.connect(
                config=self.config,
                port_enumerator=lambda: [FakePort("COM6", self.left_serial), FakePort("COM7", self.right_serial)],
                driver_factory=factory,
            ) as setup:
                def complete_intentional_cleanup():
                    for instance in factory.instances.values():
                        instance.close_error = None
                    setup.close()

                self.addCleanup(complete_intentional_cleanup)
                raise body_error
        self.assertIs(caught.exception, body_error)
        self.assertEqual(factory.instances["COM6"].close_calls, 1)
        self.assertEqual(factory.instances["COM7"].close_calls, 1)
        self.assertTrue(hasattr(body_error, "stage_cleanup_error"))

    def test_close_is_idempotent_after_confirmed_cleanup(self):
        factory = self._factory(COM6={"status": make_mdt_status(self.left_serial)})
        setup = FiberCouplingSetup.connect(
            config=self.config, port_enumerator=lambda: [FakePort("COM6", self.left_serial)], driver_factory=factory
        )
        setup.close()
        setup.close()
        self.assertEqual(factory.instances["COM6"].close_calls, 1)

    def test_close_remains_retryable_when_fake_retains_open_resource(self):
        factory = self._factory(COM6={"status": make_mdt_status(self.left_serial), "retain_open": True})
        setup = FiberCouplingSetup.connect(
            config=self.config, port_enumerator=lambda: [FakePort("COM6", self.left_serial)], driver_factory=factory
        )
        with self.assertRaises(StageConnectionError):
            setup.close()
        self.assertTrue(setup.left.status.available)
        with self.assertRaises(StageConnectionError):
            setup.close()
        self.assertEqual(factory.instances["COM6"].close_calls, 2)
        factory.instances["COM6"].retain_open = False
        setup.close()

    def test_close_retains_faulted_driver_without_is_open_until_later_cleanup(self):
        factory = self._factory(
            COM6={
                "status": make_mdt_status(self.left_serial),
                "retain_fault": True,
                "omit_is_open": True,
            }
        )
        setup = FiberCouplingSetup.connect(
            config=self.config, port_enumerator=lambda: [FakePort("COM6", self.left_serial)], driver_factory=factory
        )
        with self.assertRaises(StageConnectionError):
            setup.close()
        self.assertEqual(setup.available_sides, frozenset({StageSide.LEFT}))
        instance = factory.instances["COM6"]
        instance.retain_fault = False
        setup.close()
        self.assertEqual(instance.close_calls, 2)
        self.assertEqual(setup.available_sides, frozenset())

    def test_close_releases_disconnected_driver_with_historical_fault_status(self):
        factory = self._factory(
            COM6={"status": make_mdt_status(self.left_serial, fault_evidence="historical fault")}
        )
        setup = FiberCouplingSetup.connect(
            config=self.config, port_enumerator=lambda: [FakePort("COM6", self.left_serial)], driver_factory=factory
        )
        setup.close()
        setup.close()
        self.assertEqual(factory.instances["COM6"].close_calls, 1)
        self.assertEqual(setup.available_sides, frozenset())

    def test_close_probe_failures_are_typed_retained_and_retryable(self):
        error_types = (Exception, KeyboardInterrupt, SystemExit)
        for phase in ("is_open", "state", "status"):
            for error_type in error_types:
                with self.subTest(phase=phase, error=error_type.__name__):
                    original = error_type(f"{phase} probe")
                    factory = self._factory(
                        COM6={
                            "status": make_mdt_status(self.left_serial),
                            "omit_is_open": phase == "status",
                            "omit_state": phase == "status",
                        }
                    )
                    setup = FiberCouplingSetup.connect(
                        config=self.config,
                        port_enumerator=lambda: [FakePort("COM6", self.left_serial)],
                        driver_factory=factory,
                    )
                    instance = factory.instances["COM6"]
                    instance.probe_errors[phase] = original
                    with self.assertRaises(StageConnectionError) as caught:
                        setup.close()
                    self.assertIs(caught.exception.__cause__, original)
                    self.assertEqual(setup.available_sides, frozenset({StageSide.LEFT}))
                    instance.probe_errors.clear()
                    setup.close()
                    self.assertEqual(instance.close_calls, 2)
                    self.assertEqual(setup.available_sides, frozenset())

    def test_close_descriptor_attributeerrors_are_typed_retained_and_retryable(self):
        for phase in ("is_open", "state", "status"):
            with self.subTest(phase=phase):
                original = AttributeError(f"{phase} descriptor")
                if phase == "status":
                    instance = FakeSetupMDT(
                        "COM6",
                        status=make_mdt_status(self.left_serial),
                        omit_is_open=True,
                        omit_state=True,
                    )
                else:
                    instance = DescriptorProbeMDT("COM6", status=make_mdt_status(self.left_serial))

                setup = FiberCouplingSetup.connect(
                    config=self.config,
                    port_enumerator=lambda: [FakePort("COM6", self.left_serial)],
                    driver_factory=lambda port: instance,
                )
                if phase == "status":
                    instance.status_error = original
                else:
                    instance.descriptor_errors[phase] = original
                with self.assertRaises(StageConnectionError) as caught:
                    setup.close()
                self.assertIs(caught.exception.__cause__, original)
                self.assertEqual(setup.available_sides, frozenset({StageSide.LEFT}))
                if phase == "status":
                    instance.status_error = None
                else:
                    instance.descriptor_errors.clear()
                setup.close()
                self.assertEqual(instance.close_calls, 2)
                self.assertEqual(setup.available_sides, frozenset())

    def test_close_probe_failure_remains_primary_over_later_close_failure(self):
        probe_error = AttributeError("left state descriptor")
        left = DescriptorProbeMDT("COM6", status=make_mdt_status(self.left_serial))
        right = FakeSetupMDT(
            "COM7", status=make_mdt_status(self.right_serial), close_error=RuntimeError("right close")
        )
        setup = FiberCouplingSetup.connect(
            config=self.config,
            port_enumerator=lambda: [FakePort("COM6", self.left_serial), FakePort("COM7", self.right_serial)],
            driver_factory=lambda port: {"COM6": left, "COM7": right}[port],
        )
        left.descriptor_errors["state"] = probe_error
        with self.assertRaises(StageConnectionError) as caught:
            setup.close()
        self.assertIs(caught.exception.__cause__, probe_error)
        self.assertTrue(hasattr(caught.exception, "stage_cleanup_error"))
        left.descriptor_errors.clear()
        right.close_error = None
        setup.close()
        self.assertEqual(setup.available_sides, frozenset())

    def test_connection_baseexceptions_become_typed_and_cleanup_all_created_drivers(self):
        cases = (
            ("factory", BaseException("factory")),
            ("connect", BaseException("connect")),
            ("status", BaseException("status")),
        )
        for phase, original in cases:
            with self.subTest(phase=phase):
                left = FakeSetupMDT(
                    "COM6", status=make_mdt_status(self.left_serial), close_error=RuntimeError("left close")
                )
                right = FakeSetupMDT(
                    "COM7",
                    status=make_mdt_status(self.right_serial),
                    connect_error=original if phase == "connect" else None,
                    status_error=original if phase == "status" else None,
                    close_error=RuntimeError("right close"),
                )

                def factory(port):
                    if phase == "factory" and port == "COM7":
                        raise original
                    return {"COM6": left, "COM7": right}[port]

                with self.assertRaises(StageConnectionError) as caught:
                    FiberCouplingSetup.connect(
                        config=self.config,
                        port_enumerator=lambda: [
                            FakePort("COM6", self.left_serial), FakePort("COM7", self.right_serial),
                        ],
                        driver_factory=factory,
                    )
                self.assertIs(caught.exception.__cause__, original)
                self.assertTrue(hasattr(caught.exception, "stage_cleanup_error"))
                self.assertEqual(left.close_calls, 1)
                self.assertEqual(right.close_calls, 0 if phase == "factory" else 1)

    def test_enumerate_once_reports_all_missing_and_freezes_registered_mapping(self):
        calls = 0

        def enumerate_once():
            nonlocal calls
            calls += 1
            return []

        discovery = FiberCouplingSetup.enumerate(config=self.config, port_enumerator=enumerate_once)
        self.assertEqual(calls, 1)
        self.assertEqual(discovery.registered, MappingProxyType({}))
        self.assertEqual(discovery.missing_sides, frozenset({StageSide.LEFT, StageSide.RIGHT}))
        with self.assertRaises(TypeError):
            discovery.registered[StageSide.LEFT] = object()

    def test_enumerate_rejects_lookalike_unknown_metadata_without_opening_it(self):
        discovery = FiberCouplingSetup.enumerate(
            config=self.config,
            port_enumerator=lambda: [
                FakePort(
                    "COM8", "candidate", "MDT693B-compatible", "NotThorlabs", "MDT693B-compatible"
                )
            ],
        )
        self.assertEqual(discovery.unknown_devices, ())


class _DiagnosticStage:
    def __init__(self, status: StageStatus, *, after_adoption: StageStatus | None = None,
                 move_result: MoveResult | None = None, move_error: BaseException | None = None):
        self._statuses = [status]
        if after_adoption is not None:
            self._statuses.append(after_adoption)
        self._index = 0
        self.adopt_calls = []
        self.move_calls = []
        self.move_result = move_result
        self.move_error = move_error

    @property
    def status(self):
        return self._statuses[min(self._index, len(self._statuses) - 1)]

    def adopt_baseline(self, *, confirm, allow_nominal):
        self.adopt_calls.append((confirm, allow_nominal))
        self._index = 1
        return self.status

    def move_by_um(self, dx=0.0, dy=0.0, dz=0.0):
        self.move_calls.append((dx, dy, dz))
        if self.move_error is not None:
            raise self.move_error
        return self.move_result


class _DiagnosticSetup:
    def __init__(self, left: _DiagnosticStage, right: _DiagnosticStage, *, close_error=None, config=None):
        self.left = left
        self.right = right
        self.close_calls = 0
        self.close_error = close_error
        self.config = config

    def close(self):
        self.close_calls += 1
        if self.close_error is not None:
            raise self.close_error


def _cli_import_violations(source: str) -> list[str]:
    """Static-only import gate; mutation snippets are never imported."""
    def forbidden(label: str) -> bool:
        normalized = label.lower().replace("_", "").replace(".", "")
        return (
            normalized in {"mdt", "axis", "serial", "visa", "driver"}
            or any(token in normalized for token in ("mdt693b", "serial", "pyvisa"))
        )

    violations = []
    for node in ast.walk(ast.parse(source)):
        if isinstance(node, ast.Import):
            for alias in node.names:
                labels = (alias.name, alias.asname or "")
                if alias.name.startswith("Code.") and (
                    alias.name != "Code.Setups" or alias.asname is not None
                ):
                    violations.append(f"import {alias.name}")
                if any(forbidden(label) for label in labels):
                    violations.append(f"import {alias.name}")
        elif isinstance(node, ast.ImportFrom):
            module = node.module or ""
            if module.startswith("Code.") and (
                module != "Code.Setups" or any(alias.asname is not None for alias in node.names)
            ):
                violations.append(f"from {module}")
            for alias in node.names:
                labels = (module, alias.name, alias.asname or "")
                if any(forbidden(label) for label in labels):
                    violations.append(f"from {module} import {alias.name}")
    return violations


class FiberDiagnosticTests(unittest.TestCase):
    """Offline contract tests for the staged public setup diagnostic."""

    def setUp(self):
        self.config = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)

    def _status(self, side, *, available=True, nominal=True, baseline=False,
                observed=None, fault=None):
        definition = self.config.stages[side]
        return StageStatus(
            side=side, available=available, serial_number=definition.serial_number,
            resource="COM6" if available else None,
            toward_chip_sign=definition.toward_chip_sign,
            toward_chip_limit_um=self.config.toward_chip_limit_um,
            other_limit_um=self.config.other_limit_um,
            baseline_known=baseline,
            estimated_position_um=Vector3Um(0.0, 0.0, 0.0) if baseline else None,
            observed_voltage_v=(observed if observed is not None else {
                LogicalAxis.X: 1.0, LogicalAxis.Y: 2.0, LogicalAxis.Z: 3.0,
            }) if available else None,
            calibration=definition.calibration,
            nominal_authorized=nominal and baseline,
            restricted=False, fault=fault,
        )

    def _result(self, side, *, axis=LogicalAxis.X, displacement=0.1):
        requested = Vector3Um(
            displacement if axis is LogicalAxis.X else 0.0,
            displacement if axis is LogicalAxis.Y else 0.0,
            displacement if axis is LogicalAxis.Z else 0.0,
        )
        coefficient = self.config.stages[side].calibration[axis].positive
        return MoveResult(
            side=side, requested_um=requested,
            voltage_delta_v={axis: displacement / coefficient.um_per_v},
            calibration_used={axis: coefficient},
            estimated_before_um=Vector3Um(0.0, 0.0, 0.0),
            estimated_after_um=requested,
            observed_voltage_v={LogicalAxis.X: 1.1, LogicalAxis.Y: 2.0, LogicalAxis.Z: 3.0},
            confirmed=True,
        )

    def _setup(self, *, left=None, right=None, close_error=None, config=None):
        left = left or _DiagnosticStage(self._status(StageSide.LEFT))
        right = right or _DiagnosticStage(self._status(StageSide.RIGHT))
        return _DiagnosticSetup(left, right, close_error=close_error, config=config)

    def _run(self, argv, *, setup=None, discovery=None, replies=()):
        stdout, stderr = io.StringIO(), io.StringIO()
        replies = iter(replies)
        prompts = []
        if setup is not None:
            setup.input_prompts = prompts
        def fake_input(_prompt):
            prompts.append(_prompt)
            reply = next(replies)
            if isinstance(reply, BaseException):
                raise reply
            return reply
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr), \
             patch.object(check_fiber_stages.FiberCouplingSetup, "connect", return_value=setup) as connect, \
             patch.object(check_fiber_stages.FiberCouplingSetup, "enumerate", return_value=discovery) as enumerate_, \
             patch("builtins.input", side_effect=fake_input):
            code = check_fiber_stages.main(argv)
        return code, stdout.getvalue(), stderr.getvalue(), connect, enumerate_

    def test_help_exits_without_setup_api(self):
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(SystemExit) as caught, \
             patch.object(check_fiber_stages.FiberCouplingSetup, "connect") as connect, \
             patch.object(check_fiber_stages.FiberCouplingSetup, "enumerate") as enumerate_:
            check_fiber_stages.main(["--help"])
        self.assertEqual(caught.exception.code, 0)
        connect.assert_not_called()
        enumerate_.assert_not_called()

    def test_parser_requires_exactly_one_mode_and_complete_move_arguments(self):
        for argv in ([], ["--enumerate", "--read-only"], ["--move"],
                     ["--move", "--side", "left", "--axis", "x"],
                     ["--move", "--side", "left", "--um", "0.1"]):
            with self.subTest(argv=argv), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as caught:
                check_fiber_stages.main(argv)
            self.assertEqual(caught.exception.code, 2)

    def test_parser_rejects_nonfinite_bool_like_and_zero_displacement(self):
        for value in ("nan", "NaN", "inf", "-inf", "True", "false", "0", "-0.0"):
            with self.subTest(value=value), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as caught:
                check_fiber_stages.main(["--move", "--side", "left", "--axis", "x", "--um", value])
            self.assertEqual(caught.exception.code, 2)

    def test_enumeration_prints_identity_without_connection(self):
        known = SerialDeviceInfo("COM6", self.config.stages[StageSide.LEFT].serial_number, "fake")
        unknown = SerialDeviceInfo("COM8", "other", "unknown")
        discovery = FiberDiscovery({StageSide.LEFT: known}, {StageSide.RIGHT}, (unknown,))
        code, out, _err, connect, enumerate_ = self._run(["--enumerate"], discovery=discovery)
        self.assertEqual(code, 0)
        enumerate_.assert_called_once_with()
        connect.assert_not_called()
        self.assertIn("LEFT", out)
        self.assertIn("RIGHT", out)
        self.assertIn("COM8", out)

    def test_read_only_uses_logical_status_and_closes(self):
        setup = self._setup()
        code, out, _err, connect, _enumerate = self._run(["--read-only"], setup=setup)
        self.assertEqual(code, 0)
        connect.assert_called_once_with()
        self.assertEqual(setup.close_calls, 1)
        self.assertIn("logical X=1", out)
        self.assertIn("Y=2", out)
        self.assertIn("baseline=unknown", out)
        self.assertIn("nominal_MAX312D", out)
        self.assertNotIn("MDT X", out)

    def test_unavailable_side_fails_before_adoption(self):
        left = _DiagnosticStage(self._status(StageSide.LEFT, available=False))
        setup = self._setup(left=left)
        code, out, _err, _connect, _enumerate = self._run(
            ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"], setup=setup,
        )
        self.assertNotEqual(code, 0)
        self.assertEqual(left.adopt_calls, [])
        self.assertEqual(left.move_calls, [])
        self.assertIn("unavailable", out.lower())

    def test_nominal_move_without_flag_has_zero_adoption_and_setter_calls(self):
        left = _DiagnosticStage(self._status(StageSide.LEFT, nominal=True))
        setup = self._setup(left=left)
        code, out, _err, _connect, _enumerate = self._run(
            ["--move", "--side", "left", "--axis", "x", "--um", "0.1"], setup=setup,
        )
        self.assertNotEqual(code, 0)
        self.assertEqual(left.adopt_calls, [])
        self.assertEqual(left.move_calls, [])
        self.assertIn("--allow-nominal", out)

    def test_eof_and_one_character_mismatch_never_move(self):
        cases = ((EOFError(),), ("ADOPT LEF",))
        for replies in cases:
            with self.subTest(replies=replies):
                left = _DiagnosticStage(self._status(StageSide.LEFT))
                setup = self._setup(left=left)
                code, _out, _err, _connect, _enumerate = self._run(
                    ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"],
                    setup=setup, replies=replies,
                )
                self.assertNotEqual(code, 0)
                self.assertEqual(left.move_calls, [])

    def test_exact_confirmations_adopt_once_and_move_exact_vector(self):
        left = _DiagnosticStage(
            self._status(StageSide.LEFT), move_result=self._result(StageSide.LEFT),
        )
        setup = self._setup(left=left)
        code, out, _err, _connect, _enumerate = self._run(
            ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"],
            setup=setup, replies=("ADOPT LEFT", "MOVE LEFT X 0.1 UM"),
        )
        self.assertEqual(code, 0)
        self.assertEqual(left.adopt_calls, [(True, True)])
        self.assertEqual(left.move_calls, [(0.1, 0.0, 0.0)])
        self.assertIn("toward chip", out)
        self.assertIn("delta", out)
        self.assertIn("No automatic return or rollback", out)
        self.assertIn("completed", out)

    def test_state_change_between_confirmations_is_reported_without_move(self):
        left = _DiagnosticStage(
            self._status(StageSide.LEFT), after_adoption=self._status(StageSide.LEFT, available=False),
        )
        setup = self._setup(left=left)
        code, out, _err, _connect, _enumerate = self._run(
            ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"],
            setup=setup, replies=("ADOPT LEFT",),
        )
        self.assertNotEqual(code, 0)
        self.assertEqual(left.move_calls, [])
        self.assertIn("unavailable", out.lower())

    def test_direction_disclosure_uses_both_side_signs_and_measured_calibration(self):
        measured = replace(
            self.config.stages[StageSide.RIGHT].calibration[LogicalAxis.X].positive,
            source="measured",
        )
        right_definition = self.config.stages[StageSide.RIGHT]
        right_calibration = dict(right_definition.calibration)
        right_calibration[LogicalAxis.X] = AxisCalibration(measured, measured)
        status = replace(self._status(StageSide.RIGHT, nominal=False), calibration=right_calibration)
        right = _DiagnosticStage(status)
        setup = self._setup(right=right)
        code, out, _err, _connect, _enumerate = self._run(
            ["--move", "--side", "right", "--axis", "x", "--um", "0.1", "--allow-nominal"], setup=setup,
            replies=("ADOPT RIGHT",),
        )
        self.assertNotEqual(code, 0)
        self.assertEqual(right.adopt_calls, [(True, True)])
        self.assertIn("away from chip", out)
        self.assertIn("measured", out)

    def test_left_away_and_right_toward_use_their_respective_x_signs(self):
        for side, displacement, expected in (
            (StageSide.LEFT, -0.1, "away from chip"),
            (StageSide.RIGHT, -0.1, "toward chip"),
        ):
            with self.subTest(side=side, displacement=displacement):
                stage = _DiagnosticStage(self._status(side))
                setup = self._setup(
                    left=stage if side is StageSide.LEFT else None,
                    right=stage if side is StageSide.RIGHT else None,
                )
                code, out, _err, _connect, _enumerate = self._run(
                    ["--move", "--side", side.value, "--axis", "x", "--um", repr(displacement), "--allow-nominal"],
                    setup=setup, replies=("no",),
                )
                self.assertNotEqual(code, 0)
                self.assertIn(expected, out)
                self.assertEqual(stage.move_calls, [])

    def test_motion_failure_and_close_error_never_print_success(self):
        error = RuntimeError("motion failed")
        left = _DiagnosticStage(self._status(StageSide.LEFT), move_error=error)
        setup = self._setup(left=left, close_error=RuntimeError("close failed"))
        code, out, _err, _connect, _enumerate = self._run(
            ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"],
            setup=setup, replies=("ADOPT LEFT", "MOVE LEFT X 0.1 UM"),
        )
        self.assertNotEqual(code, 0)
        self.assertNotIn("completed", out)
        self.assertIn("motion failed", out)
        self.assertIn("close failed", out)

    def test_unconfirmed_public_move_result_never_prints_success(self):
        left = _DiagnosticStage(
            self._status(StageSide.LEFT),
            move_result=replace(self._result(StageSide.LEFT), confirmed=False),
        )
        setup = self._setup(left=left)
        code, out, _err, _connect, _enumerate = self._run(
            ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"],
            setup=setup, replies=("ADOPT LEFT", "MOVE LEFT X 0.1 UM"),
        )
        self.assertNotEqual(code, 0)
        self.assertNotIn("Move completed", out)
        self.assertIn("not confirmed", out)

    def test_keyboard_interrupt_remains_primary_when_close_also_fails(self):
        interrupted = KeyboardInterrupt("stop")
        left = _DiagnosticStage(self._status(StageSide.LEFT), move_error=interrupted)
        setup = self._setup(left=left, close_error=RuntimeError("close failed"))
        with self.assertRaises(KeyboardInterrupt) as caught:
            self._run(
                ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"],
                setup=setup, replies=("ADOPT LEFT", "MOVE LEFT X 0.1 UM"),
            )
        self.assertIs(caught.exception, interrupted)
        self.assertEqual(str(caught.exception.fiber_diagnostic_close_error), "close failed")
        self.assertEqual(setup.close_calls, 1)

    def test_confirmation_interrupts_preserve_identity_and_close_once(self):
        cases = (
            ("first", KeyboardInterrupt("first interrupt"), (KeyboardInterrupt,)),
            ("second", KeyboardInterrupt("second interrupt"), ("ADOPT LEFT",)),
            ("first", SystemExit("first exit"), (SystemExit,)),
            ("second", SystemExit("second exit"), ("ADOPT LEFT",)),
        )
        for phase, original, replies in cases:
            for close_error in (None, RuntimeError("close failed")):
                with self.subTest(phase=phase, error=type(original).__name__, close_error=close_error is not None):
                    left = _DiagnosticStage(self._status(StageSide.LEFT))
                    setup = self._setup(left=left, close_error=close_error)
                    prompts = replies[:-1] + (original,) if phase == "first" else replies + (original,)
                    with self.assertRaises(type(original)) as caught:
                        self._run(
                            ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"],
                            setup=setup, replies=prompts,
                        )
                    self.assertIs(caught.exception, original)
                    self.assertEqual(left.move_calls, [])
                    self.assertEqual(setup.close_calls, 1)
                    if close_error is None:
                        self.assertFalse(hasattr(original, "fiber_diagnostic_close_error"))
                    else:
                        self.assertIs(original.fiber_diagnostic_close_error, close_error)

    def test_second_confirmation_eof_or_mismatch_has_zero_move_calls(self):
        for reply in (EOFError(), "MOVE LEFT X 0.10 UM"):
            with self.subTest(reply=reply):
                left = _DiagnosticStage(self._status(StageSide.LEFT))
                setup = self._setup(left=left)
                code, _out, _err, _connect, _enumerate = self._run(
                    ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"],
                    setup=setup, replies=("ADOPT LEFT", reply),
                )
                self.assertNotEqual(code, 0)
                self.assertEqual(left.adopt_calls, [(True, True)])
                self.assertEqual(left.move_calls, [])

    def test_close_only_failure_is_nonzero_without_success_text(self):
        setup = self._setup(close_error=RuntimeError("close only"))
        code, out, _err, _connect, _enumerate = self._run(["--read-only"], setup=setup)
        self.assertNotEqual(code, 0)
        self.assertIn("close only", out)
        self.assertNotIn("Move completed", out)

    def test_preview_uses_logical_sign_before_public_polarity_for_both_directions(self):
        base_definition = self.config.stages[StageSide.LEFT]
        positive = CalibrationCoefficient(0.5, "measured", "2026-08-20", "positive logical")
        negative = CalibrationCoefficient(0.25, "nominal_MAX312D", None, "negative logical")
        calibration = dict(base_definition.calibration)
        calibration[LogicalAxis.X] = AxisCalibration(positive, negative)
        for polarity in (-1, 1):
            definition = replace(
                base_definition,
                polarity={**base_definition.polarity, LogicalAxis.X: polarity},
                calibration=calibration,
            )
            config = replace(self.config, stages={**self.config.stages, StageSide.LEFT: definition})
            status = replace(self._status(StageSide.LEFT), calibration=calibration)
            setup = self._setup(left=_DiagnosticStage(status), config=config)
            for requested, coefficient, expected_delta in (
                (0.1, positive, 0.2 * polarity), (-0.1, negative, -0.4 * polarity),
            ):
                with self.subTest(polarity=polarity, requested=requested):
                    selected, delta, exact = check_fiber_stages._preview(
                        setup, status, LogicalAxis.X, requested,
                    )
                    self.assertIs(selected, coefficient)
                    self.assertEqual(delta, expected_delta)
                    self.assertTrue(exact)
                    out = io.StringIO()
                    with contextlib.redirect_stdout(out):
                        check_fiber_stages._print_plan(
                            setup, status, LogicalAxis.X, requested, repr(requested),
                        )
                    text = out.getvalue()
                    self.assertIn(f"calibration={coefficient.source}", text)
                    self.assertIn(f"coefficient={coefficient.um_per_v:g} um/V", text)
                    self.assertIn(f"estimated voltage delta={expected_delta:g} V", text)

    def test_preview_without_public_polarity_labels_magnitude_not_signed_estimate(self):
        status = self._status(StageSide.LEFT)
        setup = self._setup(left=_DiagnosticStage(status), config=None)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            check_fiber_stages._print_plan(setup, status, LogicalAxis.X, -0.1, "-0.1")
        self.assertIn("conservative estimated voltage-delta magnitude=", out.getvalue())

    def test_nominal_gate_covers_every_direction_even_with_negative_polarity(self):
        definition = self.config.stages[StageSide.LEFT]
        calibration = dict(definition.calibration)
        calibration[LogicalAxis.X] = AxisCalibration(
            CalibrationCoefficient(0.5, "measured", None, "positive logical"),
            CalibrationCoefficient(0.25, "nominal_MAX312D", None, "negative logical"),
        )
        config = replace(
            self.config,
            stages={
                **self.config.stages,
                StageSide.LEFT: replace(
                    definition,
                    polarity={**definition.polarity, LogicalAxis.X: -1},
                    calibration=calibration,
                ),
            },
        )
        for requested, expected_adoptions in ((0.1, []), (-0.1, [])):
            with self.subTest(requested=requested):
                status = replace(self._status(StageSide.LEFT), calibration=calibration)
                left = _DiagnosticStage(status)
                setup = self._setup(left=left, config=config)
                code, _out, _err, _connect, _enumerate = self._run(
                    ["--move", "--side", "left", "--axis", "x", "--um", repr(requested)],
                    setup=setup, replies=("ADOPT LEFT", "decline"),
                )
                self.assertNotEqual(code, 0)
                self.assertEqual(left.adopt_calls, expected_adoptions)

    def test_any_nominal_stage_direction_blocks_adoption_before_prompt(self):
        definition = self.config.stages[StageSide.LEFT]
        calibration = dict(definition.calibration)
        x = calibration[LogicalAxis.X]
        calibration[LogicalAxis.X] = AxisCalibration(
            replace(x.positive, source="measured"), x.negative,
        )
        config = replace(
            self.config,
            stages={
                **self.config.stages,
                StageSide.LEFT: replace(
                    definition,
                    polarity={**definition.polarity, LogicalAxis.X: -1},
                    calibration=calibration,
                ),
            },
        )
        status = replace(self._status(StageSide.LEFT), calibration=calibration)
        left = _DiagnosticStage(status)
        setup = self._setup(left=left, config=config)
        code, out, _err, _connect, _enumerate = self._run(
            ["--move", "--side", "left", "--axis", "x", "--um", "0.1"], setup=setup,
        )
        self.assertNotEqual(code, 0)
        self.assertEqual(setup.input_prompts, [])
        self.assertEqual(left.adopt_calls, [])
        self.assertEqual(left.move_calls, [])
        self.assertIn("calibration=measured", out)
        self.assertIn("Nominal calibration is active", out)

    def test_whole_stage_measured_or_allow_nominal_permits_adoption_prompt(self):
        definition = self.config.stages[StageSide.LEFT]
        measured = {
            axis: AxisCalibration(
                replace(pair.positive, source="measured"),
                replace(pair.negative, source="measured"),
            )
            for axis, pair in definition.calibration.items()
        }
        measured_config = replace(
            self.config,
            stages={**self.config.stages, StageSide.LEFT: replace(definition, calibration=measured)},
        )
        for config, allow_nominal in ((measured_config, False), (self.config, True)):
            with self.subTest(allow_nominal=allow_nominal):
                status = replace(self._status(StageSide.LEFT), calibration=config.stages[StageSide.LEFT].calibration)
                left = _DiagnosticStage(status)
                setup = self._setup(left=left, config=config)
                argv = ["--move", "--side", "left", "--axis", "x", "--um", "0.1"]
                if allow_nominal:
                    argv.append("--allow-nominal")
                code, _out, _err, _connect, _enumerate = self._run(
                    argv, setup=setup, replies=("decline",),
                )
                self.assertNotEqual(code, 0)
                self.assertEqual(setup.input_prompts, ["Confirmation: "])
                self.assertEqual(left.adopt_calls, [])

    def test_eof_declines_at_both_prompts_and_keeps_decline_result_through_close_error(self):
        for phase in ("first", "second"):
            for close_error in (None, RuntimeError("close after EOF")):
                with self.subTest(phase=phase, close_error=close_error is not None):
                    left = _DiagnosticStage(self._status(StageSide.LEFT))
                    setup = self._setup(left=left, close_error=close_error)
                    replies = (EOFError(),) if phase == "first" else ("ADOPT LEFT", EOFError())
                    code, out, _err, _connect, _enumerate = self._run(
                        ["--move", "--side", "left", "--axis", "x", "--um", "0.1", "--allow-nominal"],
                        setup=setup, replies=replies,
                    )
                    self.assertEqual(code, 2)
                    self.assertEqual(setup.close_calls, 1)
                    self.assertEqual(left.move_calls, [])
                    self.assertNotIn("Move completed", out)
                    if close_error is not None:
                        self.assertIn("Close also failed", out)

    def test_cli_imports_only_setup_public_surface_and_stdlib(self):
        source = Path(check_fiber_stages.__file__).read_text(encoding="utf-8")
        self.assertEqual(_cli_import_violations(source), [])
        mutations = (
            "from Code.Utils.mdt693b import MDT693B\n",
            "from Code.Utils import Axis as LogicalAxis\n",
            "import Code.Utils as safe\n",
            "import Code.Setups.fiber_coupling\n",
            "from Code.Setups.fiber_coupling import FiberCouplingSetup\n",
            "from Code.Setups import LogicalAxis as safe\n",
            "import serial.tools as ports\n",
            "import pyvisa as visa\n",
            "from Code.Setups import LogicalAxis as Axis\n",
        )
        for source in mutations:
            with self.subTest(source=source):
                self.assertTrue(_cli_import_violations(source))

_MDT_MODULE = "Code.Utils.mdt693b"


def _is_mdt_module(module: str) -> bool:
    return module == _MDT_MODULE or module.startswith(f"{_MDT_MODULE}.")


def _dotted_name(node: ast.expr) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        parent = _dotted_name(node.value)
        return f"{parent}.{node.attr}" if parent is not None else None
    return None


class _MDTBoundaryVisitor(ast.NodeVisitor):
    """Test-only AST guard for direct experiment-level MDT access."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.violations: list[str] = []
        self._constructor_aliases = {"MDT693B"}
        self._import_module_aliases: set[str] = set()
        self._builtin_import_aliases = {"__import__"}
        self._importlib_modules: set[str] = set()
        self._static_strings: dict[str, str] = {}

    def _report(self, node: ast.AST, reason: str) -> None:
        self.violations.append(f"{self.path.as_posix()}:{node.lineno}: {reason}")

    def visit_Import(self, node: ast.Import) -> None:
        for imported in node.names:
            if _is_mdt_module(imported.name):
                self._report(node, f"forbidden import {imported.name}")
            if imported.name == "importlib":
                self._importlib_modules.add(imported.asname or "importlib")
        self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        module = node.module or ""
        if _is_mdt_module(module):
            self._report(node, f"forbidden import from {module}")
        for imported in node.names:
            if module == "Code.Utils" and imported.name == "MDT693B":
                self._report(node, "forbidden MDT693B import from Code.Utils")
                self._constructor_aliases.add(imported.asname or imported.name)
            if module == "Code.Utils" and _is_mdt_module(
                f"Code.Utils.{imported.name}"
            ):
                self._report(node, "forbidden MDT module import from Code.Utils")
            if module == "importlib" and imported.name == "import_module":
                self._import_module_aliases.add(imported.asname or imported.name)
        self.generic_visit(node)

    def visit_Assign(self, node: ast.Assign) -> None:
        self.visit(node.value)
        self._assign_aliases(node.targets, node.value)
        for target in node.targets:
            self.visit(target)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        if node.value is not None:
            self.visit(node.value)
            self._assign_aliases((node.target,), node.value)
        self.visit(node.target)
        self.visit(node.annotation)

    def visit_Call(self, node: ast.Call) -> None:
        if self._is_constructor(node.func):
            self._report(node, "forbidden MDT693B constructor call")
        if self._is_obvious_dynamic_mdt_import(node):
            self._report(node, "forbidden dynamic MDT module import")
        self.generic_visit(node)

    def _is_constructor(self, node: ast.expr) -> bool:
        if isinstance(node, ast.Name):
            return node.id in self._constructor_aliases
        return isinstance(node, ast.Attribute) and node.attr == "MDT693B"

    def _is_obvious_dynamic_mdt_import(self, node: ast.Call) -> bool:
        module = self._static_module_argument(node)
        if module is None or not _is_mdt_module(module):
            return False
        return (
            self._is_builtin_import_loader(node.func)
            or self._is_import_module_loader(node.func)
        )

    def _assign_aliases(self, targets: tuple[ast.expr, ...] | list[ast.expr], value: ast.expr) -> None:
        constructor = self._is_constructor(value)
        importlib_module = self._is_importlib_module(value)
        import_module_loader = self._is_import_module_loader(value)
        builtin_import_loader = self._is_builtin_import_loader(value)
        static_string = self._static_string(value)
        for target in targets:
            if not isinstance(target, ast.Name):
                continue
            self._clear_aliases(target.id)
            if constructor:
                self._constructor_aliases.add(target.id)
            if importlib_module:
                self._importlib_modules.add(target.id)
            if import_module_loader:
                self._import_module_aliases.add(target.id)
            if builtin_import_loader:
                self._builtin_import_aliases.add(target.id)
            if static_string is not None:
                self._static_strings[target.id] = static_string

    def _clear_aliases(self, name: str) -> None:
        if name != "MDT693B":
            self._constructor_aliases.discard(name)
        if name != "__import__":
            self._builtin_import_aliases.discard(name)
        self._import_module_aliases.discard(name)
        self._importlib_modules.discard(name)
        self._static_strings.pop(name, None)

    def _static_module_argument(self, node: ast.Call) -> str | None:
        if node.args:
            return self._static_string(node.args[0])
        for keyword in node.keywords:
            if keyword.arg in {"name", "module"}:
                return self._static_string(keyword.value)
        return None

    def _static_string(self, node: ast.expr) -> str | None:
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.Name):
            return self._static_strings.get(node.id)
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = self._static_string(node.left)
            right = self._static_string(node.right)
            if left is not None and right is not None:
                return left + right
        return None

    def _is_builtin_import_loader(self, node: ast.expr) -> bool:
        return isinstance(node, ast.Name) and node.id in self._builtin_import_aliases

    def _is_importlib_module(self, node: ast.expr) -> bool:
        return isinstance(node, ast.Name) and node.id in self._importlib_modules

    def _is_import_module_loader(self, node: ast.expr) -> bool:
        if isinstance(node, ast.Name):
            return node.id in self._import_module_aliases
        return (
            isinstance(node, ast.Attribute)
            and node.attr == "import_module"
            and _dotted_name(node.value) in self._importlib_modules
        )


def _scan_mdt_source(source: str, path: Path) -> list[str]:
    tree = ast.parse(source, filename=str(path))
    visitor = _MDTBoundaryVisitor(path)
    visitor.visit(tree)
    return visitor.violations


def scan_experiment_mdt_imports(root: Path) -> list[str]:
    violations: list[str] = []
    for path in sorted(root.rglob("*.py"), key=lambda candidate: candidate.as_posix()):
        violations.extend(_scan_mdt_source(path.read_text(encoding="utf-8"), path))
    return violations


class FiberWorkspaceBoundaryTests(unittest.TestCase):
    def test_experiments_do_not_bypass_fiber_setup_with_mdt693b(self):
        violations = scan_experiment_mdt_imports(Path("Code/Experiments"))
        self.assertEqual(violations, [])

    def test_scanner_catches_synthetic_ast_mutations(self):
        mutations = (
            "import Code.Utils.mdt693b\n",
            "import Code.Utils.mdt693b as mdt\nmdt.MDT693B('COM1')\n",
            "from Code.Utils.mdt693b import MDT693B as Controller\nController('COM1')\n",
            "from Code.Utils import MDT693B\nMDT693B('COM1')\n",
            "from Code.Utils import mdt693b as mdt\n",
            "import Code.Utils as utils\nutils.MDT693B('COM1')\n",
            "from importlib import import_module as load\nload('Code.Utils.mdt693b')\n",
            "import importlib\nload = importlib.import_module\nload('Code.Utils.mdt693b')\n",
            "import importlib as il\nil.import_module('Code.Utils.mdt693b')\n",
            "import importlib as il\nloader = il\nloader.import_module(name='Code.Utils.mdt693b')\n",
            "import importlib\nloader = importlib\nloader.import_module(module='Code.Utils.mdt693b')\n",
            "loader = __import__\nloader('Code.Utils.mdt693b')\n",
            "import importlib as il\nloader = il.import_module\nloader(name='Code.Utils.mdt693b')\n",
            "import importlib\nloader = importlib.import_module\nloader('Code.Utils.' + 'mdt693b')\n",
            "name = 'Code.Utils.' + 'mdt693b'\nloader = __import__\nloader(name=name)\n",
            "__import__('Code.Utils.mdt693b')\n",
            "Driver = MDT693B\nDriver('COM1')\n",
        )
        for source in mutations:
            with self.subTest(source=source):
                self.assertTrue(_scan_mdt_source(source, Path("synthetic.py")))

        self.assertEqual(
            _scan_mdt_source("note = 'MDT693B and Code.Utils.mdt693b'\n", Path("safe.py")),
            [],
        )
        safe_sources = (
            "import importlib\nimportlib.import_module(runtime_name)\n",
            "import importlib as il\nloader = il\nloader.import_module('Code.Utils.other')\n",
            "import importlib\nloader = importlib.import_module\nloader(name='unrelated.module')\n",
            "name = runtime_name\nloader = __import__\nloader(module=name)\n",
        )
        for source in safe_sources:
            with self.subTest(safe_source=source):
                self.assertEqual(_scan_mdt_source(source, Path("safe.py")), [])

    def test_public_setup_surface_excludes_raw_mdt_access(self):
        import Code.Setups as public_setups

        exported = set(public_setups.__all__)
        self.assertTrue(exported)
        self.assertFalse({"MDT693B", "Axis"} & exported)
        self.assertFalse({"MDT693B", "Axis"} & set(vars(public_setups)))
        raw_driver_getters = {
            name for name in exported
            if "driver" in name.casefold() and name.casefold().startswith(("get", "raw"))
        }
        self.assertEqual(raw_driver_getters, set())


if __name__ == "__main__":
    unittest.main()
