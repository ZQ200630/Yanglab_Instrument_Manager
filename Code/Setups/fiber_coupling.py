"""Immutable configuration contracts for the registered dual-fiber stages."""

from __future__ import annotations

import json
import inspect
import math
import threading
from collections.abc import Callable, Iterable
from collections.abc import Mapping
from dataclasses import dataclass
from enum import Enum
from numbers import Real
from pathlib import Path
from types import MappingProxyType

from serial.tools import list_ports

from Code.Utils.common import (
    DeviceFault,
    DriverError,
    DriverState,
    InstrumentConnectionError,
    InstrumentSafetyError,
)
from Code.Utils.mdt693b import (
    AXIS_BASELINE_ATTESTATION_EVIDENCE,
    Axis,
    AxisState,
    MDT693B,
    MDTStatus,
)


class StageSide(Enum):
    LEFT = "left"
    RIGHT = "right"


class LogicalAxis(Enum):
    X = "x"
    Y = "y"
    Z = "z"


def _finite_motion_float(value: object, field: str) -> float:
    if isinstance(value, bool) or not isinstance(value, Real):
        raise InstrumentSafetyError(f"{field} must be a finite number")
    result = float(value)
    if not math.isfinite(result):
        raise InstrumentSafetyError(f"{field} must be a finite number")
    return result


def _immutable_mapping(mapping: Mapping[object, object]) -> MappingProxyType:
    if not isinstance(mapping, Mapping):
        raise InstrumentSafetyError("mapping field must be a mapping")
    return MappingProxyType(dict(mapping))


@dataclass(frozen=True)
class Vector3Um:
    x: float
    y: float
    z: float

    def __post_init__(self) -> None:
        object.__setattr__(self, "x", _finite_motion_float(self.x, "x"))
        object.__setattr__(self, "y", _finite_motion_float(self.y, "y"))
        object.__setattr__(self, "z", _finite_motion_float(self.z, "z"))

    def for_axis(self, axis: LogicalAxis) -> float:
        return {LogicalAxis.X: self.x, LogicalAxis.Y: self.y, LogicalAxis.Z: self.z}[axis]


@dataclass(frozen=True)
class CalibrationCoefficient:
    um_per_v: float
    source: str
    date: str | None
    note: str


@dataclass(frozen=True)
class AxisCalibration:
    positive: CalibrationCoefficient
    negative: CalibrationCoefficient


@dataclass(frozen=True)
class StageDefinition:
    side: StageSide
    serial_number: str
    toward_chip_sign: int
    axis_map: Mapping[LogicalAxis, Axis]
    polarity: Mapping[LogicalAxis, int]
    calibration: Mapping[LogicalAxis, AxisCalibration]

    def __post_init__(self) -> None:
        object.__setattr__(self, "axis_map", _immutable_mapping(self.axis_map))
        object.__setattr__(self, "polarity", _immutable_mapping(self.polarity))
        object.__setattr__(self, "calibration", _immutable_mapping(self.calibration))


@dataclass(frozen=True)
class FiberCouplingConfig:
    model: str
    stages: Mapping[StageSide, StageDefinition]
    toward_chip_limit_um: float
    other_limit_um: float

    def __post_init__(self) -> None:
        object.__setattr__(self, "stages", _immutable_mapping(self.stages))


@dataclass(frozen=True)
class SerialDeviceInfo:
    resource: str
    serial_number: str
    description: str


@dataclass(frozen=True)
class FiberDiscovery:
    registered: Mapping[StageSide, SerialDeviceInfo]
    missing_sides: frozenset[StageSide]
    unknown_devices: tuple[SerialDeviceInfo, ...]

    def __post_init__(self) -> None:
        object.__setattr__(self, "registered", _immutable_mapping(self.registered))
        object.__setattr__(self, "missing_sides", frozenset(self.missing_sides))
        object.__setattr__(self, "unknown_devices", tuple(self.unknown_devices))


@dataclass(frozen=True)
class StageStatus:
    side: StageSide
    available: bool
    serial_number: str
    resource: str | None
    toward_chip_sign: int
    toward_chip_limit_um: float
    other_limit_um: float
    baseline_known: bool
    estimated_position_um: Vector3Um | None
    observed_voltage_v: Mapping[LogicalAxis, float] | None
    calibration: Mapping[LogicalAxis, AxisCalibration]
    nominal_authorized: bool
    restricted: bool
    fault: str | None

    def __post_init__(self) -> None:
        if self.observed_voltage_v is not None:
            object.__setattr__(self, "observed_voltage_v", _immutable_mapping(self.observed_voltage_v))
        object.__setattr__(self, "calibration", _immutable_mapping(self.calibration))


@dataclass(frozen=True)
class MoveResult:
    side: StageSide
    requested_um: Vector3Um
    voltage_delta_v: Mapping[LogicalAxis, float]
    calibration_used: Mapping[LogicalAxis, CalibrationCoefficient]
    estimated_before_um: Vector3Um
    estimated_after_um: Vector3Um
    observed_voltage_v: Mapping[LogicalAxis, float]
    confirmed: bool

    def __post_init__(self) -> None:
        object.__setattr__(self, "voltage_delta_v", _immutable_mapping(self.voltage_delta_v))
        object.__setattr__(self, "calibration_used", _immutable_mapping(self.calibration_used))
        object.__setattr__(self, "observed_voltage_v", _immutable_mapping(self.observed_voltage_v))


@dataclass(frozen=True)
class _AxisMove:
    logical_axis: LogicalAxis
    controller_axis: Axis
    requested_um: float
    coefficient: CalibrationCoefficient
    delta_v: float
    start_v: float
    target_v: float


@dataclass(frozen=True)
class _MovePlan:
    requested_um: Vector3Um
    moves: tuple[_AxisMove, ...]
    voltage_delta_v: Mapping[LogicalAxis, float]
    calibration_used: Mapping[LogicalAxis, CalibrationCoefficient]

    def __post_init__(self) -> None:
        object.__setattr__(self, "moves", tuple(self.moves))
        object.__setattr__(self, "voltage_delta_v", _immutable_mapping(self.voltage_delta_v))
        object.__setattr__(self, "calibration_used", _immutable_mapping(self.calibration_used))


@dataclass(frozen=True)
class _StageMotionEvidence:
    requested_um: Vector3Um
    completed_moves: tuple[_AxisMove, ...]
    last_observed_voltage_v: Mapping[LogicalAxis, float] | None

    def __post_init__(self) -> None:
        object.__setattr__(self, "completed_moves", tuple(self.completed_moves))
        if self.last_observed_voltage_v is not None:
            object.__setattr__(
                self,
                "last_observed_voltage_v",
                _immutable_mapping(self.last_observed_voltage_v),
            )


class StageUnavailableError(InstrumentConnectionError):
    """The requested registered setup side is not connected."""


class BaselineUnknownError(InstrumentSafetyError):
    """Relative motion lacks a trustworthy session command baseline."""


class CalibrationRequiredError(InstrumentSafetyError):
    """The active nominal calibration lacks explicit authorization."""


class StageMotionLimitError(InstrumentSafetyError):
    """A requested displacement or derived voltage exceeds a limit."""


class StageConnectionError(InstrumentConnectionError):
    """Setup discovery, identity verification, or cleanup failed."""


class StageMotionError(DeviceFault):
    """An attempted stage motion could not be confirmed safely."""


DEFAULT_CONFIG_PATH = Path(__file__).resolve().parents[2] / "Config" / "fiber_coupling.json"
_HARD_TOWARD_CHIP_LIMIT_UM = 0.2
_HARD_OTHER_LIMIT_UM = 1.0


class FiberStage:
    """One stable registered-side handle owned by a fiber-coupling setup."""

    def __init__(
        self,
        definition: StageDefinition,
        *,
        toward_chip_limit_um: float,
        other_limit_um: float,
        driver: MDT693B | None = None,
        resource: str | None = None,
        close_intent: threading.Event | None = None,
        publication_lock: threading.RLock | None = None,
    ) -> None:
        self._definition = definition
        self._toward_chip_limit_um = toward_chip_limit_um
        self._other_limit_um = other_limit_um
        self._driver = driver
        self._resource = resource
        self._close_requested = False
        self._setup_close_intent = (
            close_intent if close_intent is not None else threading.Event()
        )
        self._publication_lock = (
            publication_lock if publication_lock is not None else threading.RLock()
        )
        self._lock = threading.RLock()
        self._operation_lock = self._lock
        self._command_baseline_v: Mapping[LogicalAxis, float] | None = None
        self._estimated_position_um: Vector3Um | None = None
        self._nominal_authorized = False

    @property
    def status(self) -> StageStatus:
        with self._lock:
            driver = self._driver
            if driver is None:
                self._invalidate_baseline_unlocked()
                return self._unavailable_status_unlocked()
            try:
                snapshot = driver.status
            except BaseException:
                self._invalidate_baseline_unlocked()
                raise
            if snapshot is None:
                self._invalidate_baseline_unlocked()
                raise StageConnectionError(
                    f"{self._definition.side.value} MDT driver has no published status"
                )
            try:
                observed = self._logical_actual_voltages(snapshot)
            except BaseException:
                self._invalidate_baseline_unlocked()
                raise
            if (
                not snapshot.axis_command_known
                or snapshot.restricted
                or _has_hazardous_fault_evidence(snapshot)
                or (
                    self._command_baseline_v is not None
                    and any(
                        not math.isclose(
                            observed[axis], command_v, rel_tol=0.0, abs_tol=1e-6,
                        )
                        for axis, command_v in self._command_baseline_v.items()
                    )
                )
            ):
                self._invalidate_baseline_unlocked()
            return self._status_from_snapshot_unlocked(snapshot, observed)

    def adopt_baseline(
        self, *, confirm: bool = False, allow_nominal: bool = False,
    ) -> StageStatus:
        """Adopt the present public MDT outputs as this session's zero point."""
        with self._lock:
            if type(confirm) is not bool or type(allow_nominal) is not bool:
                raise InstrumentSafetyError("baseline confirmation options must be exact booleans")
            driver = self._driver
            if driver is None:
                raise StageUnavailableError(
                    f"{self._definition.side.value} stage is unavailable"
                )
            if self._close_requested or self._setup_close_intent.is_set():
                raise StageConnectionError(
                    f"{self._definition.side.value} stage is closing"
                )
            if not confirm:
                raise InstrumentSafetyError("baseline adoption requires confirm=True")
            needs_nominal = self._uses_nominal_calibration()
            if needs_nominal and not allow_nominal:
                raise CalibrationRequiredError(
                    "nominal calibration requires allow_nominal=True during baseline adoption"
                )
            self._invalidate_baseline_unlocked()
            try:
                snapshot = driver.adopt_current_axis_baseline(confirm=True)
            except BaseException as error:
                self._invalidate_baseline_unlocked()
                translated = BaselineUnknownError(
                    f"{self._definition.side.value} MDT baseline adoption failed"
                )
                translated.__cause__ = error
                raise translated
            try:
                returned_observed = self._validated_adoption_observation_unlocked(
                    snapshot, "returned",
                )
                current_snapshot = driver.status
                current_observed = self._validated_adoption_observation_unlocked(
                    current_snapshot, "current",
                )
                if any(
                    not math.isclose(
                        returned_observed[axis], current_observed[axis],
                        rel_tol=0.0, abs_tol=1e-6,
                    )
                    for axis in LogicalAxis
                ):
                    raise InstrumentSafetyError(
                        "MDT returned and current adoption voltages disagree"
                    )
            except BaseException as error:
                self._invalidate_baseline_unlocked()
                translated = BaselineUnknownError(
                    f"{self._definition.side.value} MDT adoption status could not be confirmed"
                )
                translated.__cause__ = error
                raise translated
            with self._publication_lock:
                if self._close_requested or self._setup_close_intent.is_set():
                    self._invalidate_baseline_unlocked()
                    raise StageConnectionError(
                        f"{self._definition.side.value} stage closed during baseline adoption"
                    )
                self._command_baseline_v = MappingProxyType(dict(current_observed))
                self._estimated_position_um = Vector3Um(0.0, 0.0, 0.0)
                self._nominal_authorized = needs_nominal and allow_nominal
                return self._status_from_snapshot_unlocked(current_snapshot, current_observed)

    def _logical_actual_voltages(self, snapshot: MDTStatus) -> dict[LogicalAxis, float]:
        return {
            logical: snapshot.axes[controller_axis].actual_v
            for logical, controller_axis in self._definition.axis_map.items()
        }

    def _validated_adoption_observation_unlocked(
        self, snapshot: MDTStatus | None, source: str,
    ) -> dict[LogicalAxis, float]:
        if snapshot is None:
            raise InstrumentSafetyError(f"MDT {source} adoption status is absent")
        observed = self._logical_actual_voltages(snapshot)
        if not snapshot.axis_command_known:
            raise InstrumentSafetyError(
                f"MDT {source} adoption status lacks axis-command authority"
            )
        if snapshot.restricted or _has_hazardous_fault_evidence(snapshot):
            raise InstrumentSafetyError(
                f"MDT {source} adoption status is restricted or faulted"
            )
        return observed

    def _uses_nominal_calibration(self) -> bool:
        return any(
            coefficient.source == "nominal_MAX312D"
            for calibration in self._definition.calibration.values()
            for coefficient in (calibration.positive, calibration.negative)
        )

    def _plan_move_unlocked(self, requested: Vector3Um) -> _MovePlan:
        """Build a complete read-only relative-motion plan under ``_operation_lock``."""
        if not isinstance(requested, Vector3Um):
            raise StageMotionLimitError("requested motion must be Vector3Um")
        driver = self._driver
        if driver is None:
            self._invalidate_baseline_unlocked()
            raise StageUnavailableError(f"{self._definition.side.value} stage is unavailable")
        if self._close_requested or self._setup_close_intent.is_set():
            self._invalidate_baseline_unlocked()
            raise StageConnectionError(f"{self._definition.side.value} stage is closing")
        try:
            baseline = self._validated_command_baseline_unlocked()
            self._validated_nominal_authorization_unlocked()
        except BaseException as error:
            self._invalidate_baseline_unlocked()
            if self._close_requested or self._setup_close_intent.is_set():
                translated = StageConnectionError(
                    f"{self._definition.side.value} stage closed during planning preflight"
                )
                translated.__cause__ = error
                raise translated
            if isinstance(error, BaselineUnknownError):
                raise
            translated = BaselineUnknownError(
                f"{self._definition.side.value} stage command baseline is invalid"
            )
            translated.__cause__ = error
            raise translated
        try:
            fresh_values = driver.get_all_voltages()
            snapshot = driver.status
            fresh_actual = self._validated_fresh_voltages_unlocked(fresh_values)
            self._validated_planning_limits_unlocked(driver, snapshot)
            observed = self._validated_planning_observation_unlocked(snapshot, baseline)
            self._validated_fresh_coherence_unlocked(fresh_actual, observed, baseline)
            self._validated_nominal_authorization_unlocked()
        except BaseException as error:
            self._invalidate_baseline_unlocked()
            if self._close_requested or self._setup_close_intent.is_set():
                translated = StageConnectionError(
                    f"{self._definition.side.value} stage closed during planning preflight"
                )
                translated.__cause__ = error
                raise translated
            if isinstance(error, BaselineUnknownError):
                raise
            translated = BaselineUnknownError(
                f"{self._definition.side.value} stage freshness or authority is uncertain"
            )
            translated.__cause__ = error
            raise translated

        candidates: dict[LogicalAxis, tuple[Axis, float, CalibrationCoefficient, float, float, float]] = {}
        try:
            for logical_axis in LogicalAxis:
                displacement = requested.for_axis(logical_axis)
                self._validate_requested_displacement_unlocked(logical_axis, displacement)
                if displacement == 0.0:
                    continue
                controller_axis = self._definition.axis_map[logical_axis]
                polarity = self._validated_polarity_unlocked(logical_axis)
                calibration = self._directional_calibration_unlocked(logical_axis, displacement)
                coefficient = _finite_motion_float(
                    calibration.um_per_v, f"{logical_axis.value} calibration um_per_v",
                )
                if coefficient <= 0.0:
                    raise InstrumentSafetyError("calibration coefficient must be positive")
                delta_v = displacement / coefficient / polarity
                target_v = baseline[logical_axis] + delta_v
                _finite_motion_float(delta_v, f"{logical_axis.value} voltage delta")
                _finite_motion_float(target_v, f"{logical_axis.value} target voltage")
                self._validate_target_voltage_unlocked(driver, snapshot, controller_axis, target_v)
                candidates[logical_axis] = (
                    controller_axis, displacement, calibration, delta_v,
                    baseline[logical_axis], target_v,
                )
        except BaseException as error:
            if isinstance(error, StageMotionLimitError):
                raise
            translated = StageMotionLimitError(
                f"{self._definition.side.value} requested motion is unsafe"
            )
            translated.__cause__ = error
            raise translated

        toward = requested.x * self._definition.toward_chip_sign > 0.0
        away = requested.x * self._definition.toward_chip_sign < 0.0
        order = (
            (LogicalAxis.Y, LogicalAxis.Z, LogicalAxis.X) if toward
            else (LogicalAxis.X, LogicalAxis.Y, LogicalAxis.Z) if away
            else (LogicalAxis.Y, LogicalAxis.Z)
        )
        with self._publication_lock:
            if self._close_requested or self._setup_close_intent.is_set():
                self._invalidate_baseline_unlocked()
                raise StageConnectionError(f"{self._definition.side.value} stage is closing")
            moves = tuple(
                _AxisMove(axis, *candidates[axis]) for axis in order if axis in candidates
            )
            return _MovePlan(
                requested,
                moves,
                {move.logical_axis: move.delta_v for move in moves},
                {move.logical_axis: move.coefficient for move in moves},
            )

    def move_by_um(self, dx: float = 0.0, dy: float = 0.0, dz: float = 0.0) -> MoveResult:
        """Execute one fully confirmed logical relative-motion vector.

        The MDT owns every physical ramp.  This layer only publishes a new
        session estimate after all planned controller targets are confirmed.
        """
        requested = Vector3Um(dx, dy, dz)
        if self._close_requested or self._setup_close_intent.is_set():
            with self._operation_lock:
                self._invalidate_baseline_unlocked()
            raise StageConnectionError(f"{self._definition.side.value} stage is closing")
        with self._operation_lock:
            if self._close_requested or self._setup_close_intent.is_set():
                self._invalidate_baseline_unlocked()
                raise StageConnectionError(f"{self._definition.side.value} stage is closing")
            plan = self._plan_move_unlocked(requested)
            completed_moves: list[_AxisMove] = []
            last_observed: Mapping[LogicalAxis, float] | None = None
            try:
                driver = self._driver
                if driver is None:  # Defensive: planner already checks this boundary.
                    raise StageUnavailableError(
                        f"{self._definition.side.value} stage is unavailable"
                    )
                before = self._validated_estimate_unlocked()
                working_baseline = dict(self._validated_command_baseline_unlocked())
                last_observed = dict(working_baseline)
                for move in plan.moves:
                    self._raise_if_closing_after_plan_unlocked()
                    returned = driver.set_axis_voltage(move.controller_axis, move.target_v)
                    expected = dict(working_baseline)
                    expected[move.logical_axis] = move.target_v
                    returned_observed = self._validated_execution_observation_unlocked(
                        returned, expected, "returned",
                    )
                    last_observed = dict(returned_observed)
                    current = driver.status
                    current_observed = self._validated_execution_observation_unlocked(
                        current, expected, "current",
                    )
                    last_observed = dict(current_observed)
                    self._validated_execution_coherence_unlocked(
                        returned_observed, current_observed, expected,
                    )
                    working_baseline = expected
                    completed_moves.append(move)
                    self._raise_if_closing_after_plan_unlocked()

                final_values = driver.get_all_voltages()
                final_snapshot = driver.status
                final_observed = self._validated_final_observation_unlocked(
                    final_values, final_snapshot, working_baseline,
                )
                last_observed = dict(final_observed)
                after = Vector3Um(
                    before.x + requested.x,
                    before.y + requested.y,
                    before.z + requested.z,
                )
                with self._publication_lock:
                    self._raise_if_closing_after_plan_unlocked()
                    self._command_baseline_v = MappingProxyType(dict(final_observed))
                    self._estimated_position_um = after
                    return MoveResult(
                        side=self._definition.side,
                        requested_um=requested,
                        voltage_delta_v=plan.voltage_delta_v,
                        calibration_used=plan.calibration_used,
                        estimated_before_um=before,
                        estimated_after_um=after,
                        observed_voltage_v=final_observed,
                        confirmed=True,
                    )
            except BaseException as error:
                evidence = _StageMotionEvidence(
                    requested,
                    tuple(completed_moves),
                    last_observed,
                )
                self._invalidate_baseline_unlocked()
                if isinstance(error, (KeyboardInterrupt, SystemExit)):
                    self._attach_motion_fault_evidence(error, evidence)
                    raise
                translated = StageMotionError(
                    f"{self._definition.side.value} stage motion could not be confirmed"
                )
                translated.__cause__ = error
                self._attach_motion_fault_evidence(translated, evidence)
                raise translated

    def _validated_estimate_unlocked(self) -> Vector3Um:
        estimate = self._estimated_position_um
        if estimate is None:
            raise BaselineUnknownError("no estimated stage position is available")
        return estimate

    def _raise_if_closing_after_plan_unlocked(self) -> None:
        if self._close_requested or self._setup_close_intent.is_set():
            raise StageConnectionError(f"{self._definition.side.value} stage closed during motion")

    def _validated_execution_observation_unlocked(
        self,
        snapshot: MDTStatus | None,
        expected: Mapping[LogicalAxis, float],
        source: str,
    ) -> Mapping[LogicalAxis, float]:
        if snapshot is None:
            raise InstrumentSafetyError(f"MDT {source} motion status is absent")
        observed = self._validated_status_actual_voltages_unlocked(snapshot, source)
        if not snapshot.axis_command_known:
            raise InstrumentSafetyError(f"MDT {source} motion status lacks axis-command authority")
        if snapshot.restricted or _has_hazardous_fault_evidence(snapshot):
            raise InstrumentSafetyError(f"MDT {source} motion status is restricted or faulted")
        for axis in LogicalAxis:
            if not math.isclose(
                observed[axis], expected[axis], rel_tol=0.0, abs_tol=1e-6,
            ):
                raise InstrumentSafetyError(
                    f"MDT {source} motion voltage differs from the expected command"
                )
        return observed

    def _validated_status_actual_voltages_unlocked(
        self, snapshot: MDTStatus, source: str,
    ) -> Mapping[LogicalAxis, float]:
        if not isinstance(snapshot.axes, Mapping) or set(snapshot.axes) != set(Axis):
            raise InstrumentSafetyError(f"MDT {source} motion status axis mapping is incomplete")
        controller_actual = {
            axis: _finite_motion_float(snapshot.axes[axis].actual_v, f"{axis.name} {source} voltage")
            for axis in Axis
        }
        return {
            logical: controller_actual[controller]
            for logical, controller in self._definition.axis_map.items()
        }

    def _validated_execution_coherence_unlocked(
        self,
        returned: Mapping[LogicalAxis, float],
        current: Mapping[LogicalAxis, float],
        expected: Mapping[LogicalAxis, float],
    ) -> None:
        for axis in LogicalAxis:
            if not math.isclose(
                returned[axis], current[axis], rel_tol=0.0, abs_tol=1e-6,
            ):
                raise InstrumentSafetyError("MDT returned and current motion voltages disagree")
            if not math.isclose(
                returned[axis], expected[axis], rel_tol=0.0, abs_tol=1e-6,
            ):
                raise InstrumentSafetyError("MDT returned motion voltage differs from expected command")

    def _validated_final_observation_unlocked(
        self,
        final_values: Mapping[Axis, float],
        snapshot: MDTStatus | None,
        expected: Mapping[LogicalAxis, float],
    ) -> Mapping[LogicalAxis, float]:
        fresh = self._validated_fresh_voltages_unlocked(final_values)
        if snapshot is None:
            raise InstrumentSafetyError("MDT final motion status is absent")
        current = self._validated_status_actual_voltages_unlocked(snapshot, "final")
        if not snapshot.axis_command_known:
            raise InstrumentSafetyError("MDT final motion status lacks axis-command authority")
        if snapshot.restricted or _has_hazardous_fault_evidence(snapshot):
            raise InstrumentSafetyError("MDT final motion status is restricted or faulted")
        for axis in LogicalAxis:
            if not math.isclose(fresh[axis], current[axis], rel_tol=0.0, abs_tol=1e-6):
                raise InstrumentSafetyError("MDT final fresh and current voltages disagree")
            if not math.isclose(fresh[axis], expected[axis], rel_tol=0.0, abs_tol=1e-6):
                raise InstrumentSafetyError("MDT final voltage differs from expected command")
        return fresh

    def _attach_motion_fault_evidence(
        self, error: BaseException, evidence: _StageMotionEvidence,
    ) -> None:
        try:
            setattr(
                error,
                "stage_fault_evidence",
                f"{self._definition.side.value} stage motion authority invalidated",
            )
            setattr(error, "stage_motion_evidence", evidence)
        except BaseException:
            pass

    def _validated_command_baseline_unlocked(self) -> Mapping[LogicalAxis, float]:
        baseline = self._command_baseline_v
        if baseline is None or set(baseline) != set(LogicalAxis):
            raise BaselineUnknownError("no complete setup command baseline is available")
        for axis in LogicalAxis:
            _finite_motion_float(baseline[axis], f"{axis.value} command baseline")
        return baseline

    def _validated_nominal_authorization_unlocked(self) -> None:
        if self._uses_nominal_calibration() and self._nominal_authorized is not True:
            raise InstrumentSafetyError("nominal calibration authorization is unavailable")

    def _validated_fresh_voltages_unlocked(
        self, values: Mapping[Axis, float],
    ) -> Mapping[LogicalAxis, float]:
        if not isinstance(values, Mapping) or set(values) != set(Axis):
            raise InstrumentSafetyError("MDT fresh voltage mapping is incomplete")
        controller_actual = {
            axis: _finite_motion_float(values[axis], f"{axis.name} fresh voltage")
            for axis in Axis
        }
        return {
            logical_axis: controller_actual[controller_axis]
            for logical_axis, controller_axis in self._definition.axis_map.items()
        }

    def _validated_fresh_coherence_unlocked(
        self,
        fresh_actual: Mapping[LogicalAxis, float],
        observed: Mapping[LogicalAxis, float],
        baseline: Mapping[LogicalAxis, float],
    ) -> None:
        for axis in LogicalAxis:
            if not math.isclose(fresh_actual[axis], observed[axis], rel_tol=0.0, abs_tol=1e-6):
                raise InstrumentSafetyError("MDT fresh and current actual voltages disagree")
            if not math.isclose(fresh_actual[axis], baseline[axis], rel_tol=0.0, abs_tol=1e-6):
                raise InstrumentSafetyError("MDT fresh voltage changed from command baseline")

    def _validated_planning_observation_unlocked(
        self, snapshot: MDTStatus | None, baseline: Mapping[LogicalAxis, float],
    ) -> Mapping[LogicalAxis, float]:
        if snapshot is None:
            raise InstrumentSafetyError("MDT has no current public status")
        observed = self._logical_actual_voltages(snapshot)
        if not snapshot.axis_command_known:
            raise InstrumentSafetyError("MDT status lacks axis-command authority")
        if snapshot.restricted or _has_hazardous_fault_evidence(snapshot):
            raise InstrumentSafetyError("MDT status is restricted or faulted")
        if any(
            not math.isclose(observed[axis], baseline[axis], rel_tol=0.0, abs_tol=1e-6)
            for axis in LogicalAxis
        ):
            raise InstrumentSafetyError("MDT actual voltage changed from command baseline")
        return observed

    def _validated_planning_limits_unlocked(self, driver: MDT693B, snapshot: MDTStatus) -> None:
        if not isinstance(snapshot.axes, Mapping) or set(snapshot.axes) != set(Axis):
            raise InstrumentSafetyError("MDT status axis limits are incomplete")
        _finite_motion_float(snapshot.hardware_limit.volts, "MDT hardware voltage limit")
        application_limits = driver.application_limits_v
        if not isinstance(application_limits, Mapping) or set(application_limits) != set(Axis):
            raise InstrumentSafetyError("MDT application voltage limits are incomplete")
        for axis in Axis:
            state = snapshot.axes[axis]
            minimum = _finite_motion_float(state.minimum_v, f"{axis.name} minimum voltage")
            maximum = _finite_motion_float(state.maximum_v, f"{axis.name} maximum voltage")
            _finite_motion_float(state.actual_v, f"{axis.name} actual voltage")
            _finite_motion_float(application_limits[axis], f"{axis.name} application voltage limit")
            if minimum > maximum:
                raise InstrumentSafetyError(f"{axis.name} voltage limits are inverted")

    def _validate_requested_displacement_unlocked(
        self, axis: LogicalAxis, displacement: float,
    ) -> None:
        _finite_motion_float(displacement, f"{axis.value} requested displacement")
        if axis is LogicalAxis.X and displacement * self._definition.toward_chip_sign > 0.0:
            limit = min(_HARD_TOWARD_CHIP_LIMIT_UM, self._toward_chip_limit_um)
        else:
            limit = min(_HARD_OTHER_LIMIT_UM, self._other_limit_um)
        limit = _finite_motion_float(limit, f"{axis.value} requested displacement limit")
        if limit < 0.0 or abs(displacement) > limit:
            raise StageMotionLimitError(
                f"{axis.value} requested displacement exceeds its exact safety limit"
            )

    def _validated_polarity_unlocked(self, axis: LogicalAxis) -> int:
        polarity = self._definition.polarity.get(axis)
        if type(polarity) is not int or polarity not in {-1, 1}:
            raise InstrumentSafetyError(f"{axis.value} polarity is invalid")
        return polarity

    def _directional_calibration_unlocked(
        self, axis: LogicalAxis, displacement: float,
    ) -> CalibrationCoefficient:
        calibration = self._definition.calibration.get(axis)
        if not isinstance(calibration, AxisCalibration):
            raise InstrumentSafetyError(f"{axis.value} calibration is unavailable")
        coefficient = calibration.positive if displacement > 0.0 else calibration.negative
        if not isinstance(coefficient, CalibrationCoefficient):
            raise InstrumentSafetyError(f"{axis.value} directional calibration is invalid")
        return coefficient

    def _validate_target_voltage_unlocked(
        self, driver: MDT693B, snapshot: MDTStatus, controller_axis: Axis, target_v: float,
    ) -> None:
        state = snapshot.axes[controller_axis]
        upper = min(
            75.0,
            _finite_motion_float(snapshot.hardware_limit.volts, "MDT hardware voltage limit"),
            _finite_motion_float(
                driver.application_limits_v[controller_axis],
                f"{controller_axis.name} application voltage limit",
            ),
            _finite_motion_float(state.maximum_v, f"{controller_axis.name} maximum voltage"),
        )
        lower = _finite_motion_float(state.minimum_v, f"{controller_axis.name} minimum voltage")
        if target_v < 0.0 or target_v > 75.0 or target_v < lower or target_v > upper:
            raise StageMotionLimitError(
                f"{controller_axis.name} target voltage is outside its exact public limits"
            )

    def _invalidate_baseline_unlocked(self) -> None:
        self._command_baseline_v = None
        self._estimated_position_um = None
        self._nominal_authorized = False

    def _unavailable_status_unlocked(self) -> StageStatus:
        return StageStatus(
            side=self._definition.side,
            available=False,
            serial_number=self._definition.serial_number,
            resource=None,
            toward_chip_sign=self._definition.toward_chip_sign,
            toward_chip_limit_um=self._toward_chip_limit_um,
            other_limit_um=self._other_limit_um,
            baseline_known=False,
            estimated_position_um=None,
            observed_voltage_v=None,
            calibration=self._definition.calibration,
            nominal_authorized=False,
            restricted=False,
            fault=None,
        )

    def _status_from_snapshot_unlocked(
        self, snapshot: MDTStatus, observed: Mapping[LogicalAxis, float],
    ) -> StageStatus:
        baseline_known = self._command_baseline_v is not None
        return StageStatus(
            side=self._definition.side,
            available=True,
            serial_number=self._definition.serial_number,
            resource=self._resource,
            toward_chip_sign=self._definition.toward_chip_sign,
            toward_chip_limit_um=self._toward_chip_limit_um,
            other_limit_um=self._other_limit_um,
            baseline_known=baseline_known,
            estimated_position_um=(self._estimated_position_um if baseline_known else None),
            observed_voltage_v=observed,
            calibration=self._definition.calibration,
            nominal_authorized=self._nominal_authorized if baseline_known else False,
            restricted=snapshot.restricted,
            fault=snapshot.fault_evidence,
        )

    def _request_close(self) -> None:
        with self._lock:
            self._close_requested = True
            self._invalidate_baseline_unlocked()

    def _release_driver(self) -> None:
        with self._lock:
            self._driver = None
            self._resource = None
            self._invalidate_baseline_unlocked()


class FiberCouplingSetup:
    """Discover registered MDTs by serial and own their connection lifecycle."""

    def __init__(
        self,
        config: FiberCouplingConfig,
        discovery: FiberDiscovery,
        drivers: Mapping[StageSide, MDT693B],
    ) -> None:
        self.config = config
        self.discovery = discovery
        self._lock = threading.RLock()
        self._close_requested = False
        self._close_intent = threading.Event()
        self._publication_lock = threading.RLock()
        self._drivers = dict(drivers)
        self.left = FiberStage(
            config.stages[StageSide.LEFT],
            toward_chip_limit_um=config.toward_chip_limit_um,
            other_limit_um=config.other_limit_um,
            driver=self._drivers.get(StageSide.LEFT),
            resource=(discovery.registered.get(StageSide.LEFT).resource
                      if StageSide.LEFT in discovery.registered else None),
            close_intent=self._close_intent,
            publication_lock=self._publication_lock,
        )
        self.right = FiberStage(
            config.stages[StageSide.RIGHT],
            toward_chip_limit_um=config.toward_chip_limit_um,
            other_limit_um=config.other_limit_um,
            driver=self._drivers.get(StageSide.RIGHT),
            resource=(discovery.registered.get(StageSide.RIGHT).resource
                      if StageSide.RIGHT in discovery.registered else None),
            close_intent=self._close_intent,
            publication_lock=self._publication_lock,
        )

    @classmethod
    def enumerate(
        cls,
        config: FiberCouplingConfig | None = None,
        *,
        config_path: str | Path = DEFAULT_CONFIG_PATH,
        port_enumerator: Callable[[], Iterable[object]] = list_ports.comports,
    ) -> FiberDiscovery:
        active_config = config or load_fiber_coupling_config(config_path)
        serial_to_side = {
            definition.serial_number: side for side, definition in active_config.stages.items()
        }
        registered: dict[StageSide, SerialDeviceInfo] = {}
        registered_resources: dict[str, tuple[StageSide, str]] = {}
        unknown: list[SerialDeviceInfo] = []
        for port in port_enumerator():
            resource = str(getattr(port, "device", ""))
            serial_value = getattr(port, "serial_number", None)
            serial = serial_value if isinstance(serial_value, str) else ""
            description_value = getattr(port, "description", "")
            description = description_value if isinstance(description_value, str) else ""
            manufacturer = getattr(port, "manufacturer", "")
            product = getattr(port, "product", "")
            if serial in serial_to_side:
                side = serial_to_side[serial]
                if side in registered:
                    raise StageConnectionError(
                        f"Duplicate registered {side.value} serial metadata: {serial}"
                    )
                canonical_resource = _canonical_windows_resource(resource)
                previous = registered_resources.get(canonical_resource)
                if previous is not None:
                    previous_side, previous_resource = previous
                    raise StageConnectionError(
                        "Registered fiber stages share one physical resource: "
                        f"{previous_side.value}={previous_resource!r}, "
                        f"{side.value}={resource!r}"
                    )
                registered_resources[canonical_resource] = (side, resource)
                registered[side] = SerialDeviceInfo(resource, serial, description)
            elif serial and _is_mdt693b_candidate(manufacturer, product, description):
                unknown.append(SerialDeviceInfo(resource, serial, description))
        return FiberDiscovery(
            registered=registered,
            missing_sides=frozenset(set(StageSide).difference(registered)),
            unknown_devices=tuple(sorted(unknown, key=lambda item: (item.serial_number, item.resource))),
        )

    @classmethod
    def connect(
        cls,
        config: FiberCouplingConfig | None = None,
        *,
        config_path: str | Path = DEFAULT_CONFIG_PATH,
        port_enumerator: Callable[[], Iterable[object]] = list_ports.comports,
        driver_factory: Callable[..., MDT693B] = MDT693B,
        member_serials: tuple[str, ...] | None = None,
    ) -> FiberCouplingSetup:
        active_config = config or load_fiber_coupling_config(config_path)
        selected = None
        if member_serials is not None:
            fixed = frozenset(("2110148249-10", "160721175410"))
            if (type(member_serials) is not tuple or not member_serials
                    or any(type(value) is not str or value not in fixed for value in member_serials)
                    or len(set(member_serials)) != len(member_serials)):
                raise StageConnectionError("Select one or both registered fiber serial numbers")
            selected = frozenset(member_serials)
        discovery = cls.enumerate(
            active_config, config_path=config_path, port_enumerator=port_enumerator
        )
        if selected is not None:
            registered = {side: info for side, info in discovery.registered.items()
                          if active_config.stages[side].serial_number in selected}
            if {active_config.stages[side].serial_number for side in registered} != selected:
                raise StageConnectionError("An explicitly selected fiber member is not present")
            discovery = FiberDiscovery(registered,
                frozenset(set(StageSide).difference(registered)), discovery.unknown_devices)
        if not discovery.registered:
            raise StageConnectionError("No registered fiber stage was discovered")
        drivers: dict[StageSide, MDT693B] = {}
        primary: BaseException | None = None
        for side in StageSide:
            info = discovery.registered.get(side)
            if info is None:
                continue
            try:
                driver = driver_factory(info.resource)
                drivers[side] = driver
                driver.connect()
                status = driver.status
                if status is None:
                    raise StageConnectionError(
                        f"{side.value} MDT did not publish status during connect"
                    )
                expected_serial = active_config.stages[side].serial_number
                if status.serial_number != expected_serial:
                    raise StageConnectionError(
                        f"{side.value} MDT serial mismatch: expected {expected_serial}, "
                        f"observed {status.serial_number}"
                    )
            except BaseException as error:
                primary = _stage_connection_error(
                    error, f"Failed to connect {side.value} fiber stage"
                )
                break
        if primary is not None:
            _close_drivers(drivers, primary)
            raise primary
        return cls(active_config, discovery, drivers)

    @classmethod
    def for_members(cls, serials: tuple[str, ...], **kwargs) -> FiberCouplingSetup:
        """Connect only explicitly selected, serial-bound members; preserve outputs."""
        if "member_serials" in kwargs:
            raise StageConnectionError("Member selection may not be overridden")
        return cls.connect(member_serials=serials, **kwargs)

    @property
    def state(self) -> DriverState | None:
        """Aggregate cached lifecycle only, independent of motion baselines.

        A missing side is valid. A retained but disconnected child is a fault;
        unsupported child state remains unknown. No sensor or motion I/O occurs.
        """
        with self._lock:
            if not self._drivers:
                return DriverState.DISCONNECTED
            if self._close_requested or self._close_intent.is_set():
                return DriverState.CLOSING
            states = tuple(getattr(driver, "state", None) for driver in self._drivers.values())
            if any(state in (DriverState.FAULT, DriverState.DISCONNECTED) for state in states):
                return DriverState.FAULT
            if any(state is DriverState.CLOSING for state in states):
                return DriverState.CLOSING
            if any(not isinstance(state, DriverState) for state in states):
                return None
            if any(state is DriverState.CONNECTING for state in states):
                return DriverState.CONNECTING
            return DriverState.ACTIVE if DriverState.ACTIVE in states else DriverState.READY

    @property
    def available_sides(self) -> frozenset[StageSide]:
        with self._lock:
            return frozenset(self._drivers)

    @property
    def missing_sides(self) -> frozenset[StageSide]:
        with self._lock:
            return frozenset(set(StageSide).difference(self._drivers))

    @property
    def unknown_devices(self) -> tuple[SerialDeviceInfo, ...]:
        return self.discovery.unknown_devices

    def close(self) -> None:
        with self._publication_lock:
            self._close_intent.set()
        with self._lock:
            self._close_requested = True
            self.left._request_close()
            self.right._request_close()
            owned = tuple(self._drivers.items())
        primary: BaseException | None = None
        closed: list[StageSide] = []
        for side, driver in owned:
            try:
                driver.close()
                if _driver_remains_open(driver):
                    raise StageConnectionError(
                        f"{side.value} MDT close returned while its resource remains open"
                    )
                closed.append(side)
            except BaseException as error:
                failure = _stage_connection_error(error, f"Failed to close {side.value} fiber stage")
                if primary is None:
                    primary = failure
                else:
                    _attach_cleanup_error(primary, failure)
        with self._lock:
            for side in closed:
                self._drivers.pop(side, None)
                (self.left if side is StageSide.LEFT else self.right)._release_driver()
        if primary is not None:
            raise primary

    def __enter__(self) -> FiberCouplingSetup:
        return self

    def __exit__(self, exc_type, exc_value, traceback) -> bool:
        try:
            self.close()
        except BaseException as cleanup_error:
            if exc_type is None:
                raise
            if isinstance(exc_value, BaseException):
                _attach_cleanup_error(exc_value, cleanup_error)
        return False


def _stage_connection_error(error: BaseException, message: str) -> StageConnectionError:
    if isinstance(error, StageConnectionError):
        return error
    translated = StageConnectionError(message)
    translated.__cause__ = error
    return translated


def _driver_remains_open(driver: MDT693B) -> bool:
    is_open = _read_optional_public_member(driver, "is_open")
    if is_open is not _MISSING_MEMBER and bool(is_open):
        return True
    state = _read_optional_public_member(driver, "state")
    if state is not _MISSING_MEMBER:
        state_name = state.name if isinstance(state, Enum) else state
        if state_name == "DISCONNECTED":
            return False
        return True
    status = driver.status
    return status is not None and status.fault_evidence is not None


def _has_hazardous_fault_evidence(snapshot: MDTStatus) -> bool:
    evidence = snapshot.fault_evidence
    return evidence is not None and not (
        evidence == AXIS_BASELINE_ATTESTATION_EVIDENCE
        and snapshot.axis_command_known is True
        and snapshot.restricted is False
    )


def _canonical_windows_resource(resource: str) -> str:
    canonical = resource.strip().casefold()
    device_namespace_prefix = "\\\\.\\"
    candidate = (
        canonical[len(device_namespace_prefix):]
        if canonical.startswith(device_namespace_prefix)
        else canonical
    )
    port_number = candidate[3:] if candidate.startswith("com") else ""
    if port_number and port_number.isascii() and port_number.isdecimal():
        return candidate
    return resource


_MISSING_MEMBER = object()


def _read_optional_public_member(driver: MDT693B, name: str) -> object:
    """Read an optional public member once without hiding descriptor failures."""
    if inspect.getattr_static(driver, name, _MISSING_MEMBER) is _MISSING_MEMBER:
        return _MISSING_MEMBER
    return getattr(driver, name)


def _is_mdt693b_candidate(
    manufacturer: object, product: object, description: str,
) -> bool:
    manufacturer_name = manufacturer.strip().casefold() if isinstance(manufacturer, str) else ""
    identity_fields = (product, description)
    has_model = any(_has_metadata_token(value, "mdt693b") for value in identity_fields)
    has_thorlabs = manufacturer_name == "thorlabs" or _has_metadata_token(description, "thorlabs")
    return has_thorlabs and has_model


def _has_metadata_token(value: object, token: str) -> bool:
    if not isinstance(value, str):
        return False
    return token in value.casefold().replace("_", " ").split()


def _close_drivers(drivers: Mapping[StageSide, MDT693B], primary: BaseException) -> None:
    for side, driver in drivers.items():
        try:
            driver.close()
            if _driver_remains_open(driver):
                raise StageConnectionError(
                    f"{side.value} MDT cleanup returned while its resource remains open"
                )
        except BaseException as error:
            _attach_cleanup_error(
                primary, _stage_connection_error(error, f"Failed to clean up {side.value} fiber stage")
            )


def _attach_cleanup_error(primary: BaseException, secondary: BaseException) -> None:
    try:
        if getattr(primary, "stage_cleanup_error", None) is None:
            setattr(primary, "stage_cleanup_error", secondary)
        else:
            setattr(primary, "stage_additional_cleanup_error", secondary)
    except BaseException:
        pass


_SIDES = frozenset(side.value for side in StageSide)
_LOGICAL_AXES = frozenset(axis.value for axis in LogicalAxis)
_PHYSICAL_AXES = frozenset(axis.name for axis in Axis)
_CALIBRATION_SOURCES = frozenset({"nominal_MAX312D", "measured"})


class _StrictJsonObject(dict[str, object]):
    """A parsed object that retains names repeated in its JSON source."""

    def __init__(self, pairs: list[tuple[str, object]]) -> None:
        super().__init__()
        duplicates: list[str] = []
        for name, value in pairs:
            if name in self:
                duplicates.append(name)
            self[name] = value
        self.duplicate_keys = tuple(duplicates)
        self.duplicate_error = (
            ValueError(f"duplicate JSON key(s): {sorted(set(duplicates))}")
            if duplicates
            else None
        )


def load_fiber_coupling_config(
    path: str | Path = DEFAULT_CONFIG_PATH,
) -> FiberCouplingConfig:
    raw = _read_strict_json(Path(path))
    return _parse_fiber_coupling_config(raw)


def _read_strict_json(path: Path) -> object:
    def reject_constant(value: str) -> object:
        raise ValueError(f"JSON constant {value} is not permitted")

    try:
        with path.open("r", encoding="utf-8") as handle:
            return json.load(
                handle,
                object_pairs_hook=_StrictJsonObject,
                parse_constant=reject_constant,
            )
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        raise InstrumentSafetyError(f"{path}: invalid fiber coupling JSON") from exc


def _parse_fiber_coupling_config(raw: object) -> FiberCouplingConfig:
    root = _object(raw, "$")
    _exact_keys(root, {"model", "operator_limits_um", "stages"}, "$")
    model = _text(root["model"], "$.model", nonempty=True)
    if model != "MAX312D":
        raise InstrumentSafetyError("$.model must equal MAX312D")
    limits = _object(root["operator_limits_um"], "$.operator_limits_um")
    _exact_keys(limits, {"toward_chip", "other"}, "$.operator_limits_um")
    toward_limit = _nonnegative_finite(limits["toward_chip"], "$.operator_limits_um.toward_chip")
    other_limit = _nonnegative_finite(limits["other"], "$.operator_limits_um.other")
    if toward_limit > _HARD_TOWARD_CHIP_LIMIT_UM:
        raise InstrumentSafetyError("$.operator_limits_um.toward_chip exceeds the hard limit")
    if other_limit > _HARD_OTHER_LIMIT_UM:
        raise InstrumentSafetyError("$.operator_limits_um.other exceeds the hard limit")

    raw_stages = _object(root["stages"], "$.stages")
    _exact_keys(raw_stages, _SIDES, "$.stages")
    stages = {
        side: _parse_stage(raw_stages[side.value], side)
        for side in StageSide
    }
    serial_numbers = [definition.serial_number for definition in stages.values()]
    if len(set(serial_numbers)) != len(serial_numbers):
        raise InstrumentSafetyError("$.stages: serial_number values must be unique")
    return FiberCouplingConfig(model, stages, toward_limit, other_limit)


def _parse_stage(raw: object, side: StageSide) -> StageDefinition:
    path = f"$.stages.{side.value}"
    stage = _object(raw, path)
    _exact_keys(stage, {"serial_number", "toward_chip_sign", "axis_map", "polarity", "calibration"}, path)
    serial_number = _text(stage["serial_number"], f"{path}.serial_number", nonempty=True)
    toward_chip_sign = _one_of_int(stage["toward_chip_sign"], {-1, 1}, f"{path}.toward_chip_sign")
    axis_map = _parse_axis_map(stage["axis_map"], f"{path}.axis_map")
    polarity = _parse_polarity(stage["polarity"], f"{path}.polarity")
    calibration = _parse_calibration(stage["calibration"], f"{path}.calibration")
    return StageDefinition(side, serial_number, toward_chip_sign, axis_map, polarity, calibration)


def _parse_axis_map(raw: object, path: str) -> dict[LogicalAxis, Axis]:
    values = _object(raw, path)
    _exact_keys(values, _LOGICAL_AXES, path)
    result: dict[LogicalAxis, Axis] = {}
    physical_names: list[str] = []
    for logical_axis in LogicalAxis:
        field = f"{path}.{logical_axis.value}"
        physical = _text(values[logical_axis.value], field, nonempty=True)
        if physical not in _PHYSICAL_AXES:
            raise InstrumentSafetyError(f"{field} must be one of X, Y, Z")
        physical_names.append(physical)
        result[logical_axis] = Axis[physical]
    if len(set(physical_names)) != len(physical_names):
        raise InstrumentSafetyError(f"{path} must be a complete physical-axis permutation")
    return result


def _parse_polarity(raw: object, path: str) -> dict[LogicalAxis, int]:
    values = _object(raw, path)
    _exact_keys(values, _LOGICAL_AXES, path)
    return {
        axis: _one_of_int(values[axis.value], {-1, 1}, f"{path}.{axis.value}")
        for axis in LogicalAxis
    }


def _parse_calibration(raw: object, path: str) -> dict[LogicalAxis, AxisCalibration]:
    values = _object(raw, path)
    _exact_keys(values, _LOGICAL_AXES, path)
    return {
        axis: AxisCalibration(
            _parse_coefficient(values[axis.value], "positive", f"{path}.{axis.value}"),
            _parse_coefficient(values[axis.value], "negative", f"{path}.{axis.value}"),
        )
        for axis in LogicalAxis
    }


def _parse_coefficient(axis_raw: object, direction: str, axis_path: str) -> CalibrationCoefficient:
    axis = _object(axis_raw, axis_path)
    _exact_keys(axis, {"positive", "negative"}, axis_path)
    path = f"{axis_path}.{direction}"
    coefficient = _object(axis[direction], path)
    _exact_keys(coefficient, {"um_per_v", "source", "date", "note"}, path)
    um_per_v = _positive_finite(coefficient["um_per_v"], f"{path}.um_per_v")
    source = _text(coefficient["source"], f"{path}.source", nonempty=True)
    if source not in _CALIBRATION_SOURCES:
        raise InstrumentSafetyError(f"{path}.source is not recognized")
    date_raw = coefficient["date"]
    if date_raw is not None and not isinstance(date_raw, str):
        raise InstrumentSafetyError(f"{path}.date must be null or nonempty text")
    if isinstance(date_raw, str) and not date_raw.strip():
        raise InstrumentSafetyError(f"{path}.date must be null or nonempty text")
    note = _text(coefficient["note"], f"{path}.note")
    return CalibrationCoefficient(um_per_v, source, date_raw, note)


def _object(value: object, path: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise InstrumentSafetyError(f"{path} must be a JSON object")
    parse_error = getattr(value, "duplicate_error", None)
    if parse_error is not None:
        raise InstrumentSafetyError(f"{path} has {parse_error}") from parse_error
    return value


def _exact_keys(value: Mapping[str, object], expected: set[str] | frozenset[str], path: str) -> None:
    actual = set(value)
    if actual != set(expected):
        missing = sorted(set(expected) - actual)
        unexpected = sorted(actual - set(expected))
        raise InstrumentSafetyError(f"{path} has invalid keys; missing={missing}, unexpected={unexpected}")


def _text(value: object, path: str, *, nonempty: bool = False) -> str:
    if not isinstance(value, str) or (nonempty and not value.strip()):
        qualifier = " nonempty" if nonempty else ""
        raise InstrumentSafetyError(f"{path} must be{qualifier} text")
    return value


def _positive_finite(value: object, path: str) -> float:
    if isinstance(value, bool) or not isinstance(value, Real):
        raise InstrumentSafetyError(f"{path} must be a positive finite number")
    result = float(value)
    if not math.isfinite(result) or result <= 0:
        raise InstrumentSafetyError(f"{path} must be a positive finite number")
    return result


def _nonnegative_finite(value: object, path: str) -> float:
    if isinstance(value, bool) or not isinstance(value, Real):
        raise InstrumentSafetyError(f"{path} must be a nonnegative finite number")
    result = float(value)
    if not math.isfinite(result) or result < 0:
        raise InstrumentSafetyError(f"{path} must be a nonnegative finite number")
    return result


def _one_of_int(value: object, allowed: set[int], path: str) -> int:
    if type(value) is not int or value not in allowed:
        choices = " or ".join(str(item) for item in sorted(allowed))
        raise InstrumentSafetyError(f"{path} must be exactly {choices}")
    return value
