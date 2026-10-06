"""Validated, immutable configuration for SIL hysteresis experiments."""

from __future__ import annotations

import json
import math
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any


def _finite_number(value: Any, name: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{name} must be a finite number")
    number = float(value)
    if not math.isfinite(number):
        raise ValueError(f"{name} must be a finite number")
    return number


def _bounded_number(value: Any, name: str, minimum: float, maximum: float) -> float:
    number = _finite_number(value, name)
    if not minimum <= number <= maximum:
        raise ValueError(f"{name} must be between {minimum} and {maximum}")
    return number


def _nonnegative_number(value: Any, name: str) -> float:
    number = _finite_number(value, name)
    if number < 0:
        raise ValueError(f"{name} must be non-negative")
    return number


def _mapping(value: Any, name: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ValueError(f"{name} must be a mapping")
    return value


def _optional_text(value: Any, name: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{name} must be a non-empty string or null")
    return value.strip()


@dataclass(frozen=True)
class OperationPoint:
    temperature_c: float
    current_ma: float

    def __post_init__(self) -> None:
        object.__setattr__(self, "temperature_c", _bounded_number(self.temperature_c, "temperature", 15.0, 40.0))
        object.__setattr__(self, "current_ma", _bounded_number(self.current_ma, "current", 0.0, 200.0))


@dataclass(frozen=True)
class DeviceConfig:
    osa_resource: str = "GPIB0::4::INSTR"
    osa_trace: str = "A"
    voltage_port: str | None = None
    gain_port: str | None = None
    gain_serial_number: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.osa_resource, str) or not self.osa_resource.strip():
            raise ValueError("OSA resource must be a non-empty string")
        trace = self.osa_trace.strip().upper() if isinstance(self.osa_trace, str) else ""
        if len(trace) != 1 or trace not in "ABCDEFG":
            raise ValueError("OSA trace must identify AQ6370 trace A through G")
        object.__setattr__(self, "osa_resource", self.osa_resource.strip())
        object.__setattr__(self, "osa_trace", trace)
        object.__setattr__(self, "voltage_port", _optional_text(self.voltage_port, "voltage_port"))
        object.__setattr__(self, "gain_port", _optional_text(self.gain_port, "gain_port"))
        object.__setattr__(self, "gain_serial_number", _optional_text(self.gain_serial_number, "gain_serial_number"))


@dataclass(frozen=True)
class ScanConfig:
    channel: int = 1
    v_min: float = 0.0
    v_max: float = 13.0
    points: int = 100
    spacing: str = "v_squared"
    settle_time_s: float = 0.1

    def __post_init__(self) -> None:
        if isinstance(self.channel, bool) or not isinstance(self.channel, int) or not 1 <= self.channel <= 8:
            raise ValueError("channel must be an integer between 1 and 8")
        v_min = _bounded_number(self.v_min, "voltage", 0.0, 14.0)
        v_max = _bounded_number(self.v_max, "voltage", 0.0, 14.0)
        if v_min >= v_max:
            raise ValueError("v_min must be less than v_max")
        if isinstance(self.points, bool) or not isinstance(self.points, int) or self.points < 2:
            raise ValueError("points must be an integer of at least two")
        if self.spacing != "v_squared":
            raise ValueError("spacing must be v_squared")
        object.__setattr__(self, "v_min", v_min)
        object.__setattr__(self, "v_max", v_max)
        object.__setattr__(self, "settle_time_s", _nonnegative_number(self.settle_time_s, "settle_time_s"))


@dataclass(frozen=True)
class AnalysisConfig:
    signal_window_db: float = 30.0
    display_floor_dbm: float | None = None
    thresholds: Mapping[str, float] | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "signal_window_db", _finite_number(self.signal_window_db, "signal_window_db"))
        if self.display_floor_dbm is not None:
            object.__setattr__(
                self,
                "display_floor_dbm",
                _finite_number(self.display_floor_dbm, "display_floor_dbm"),
            )
        if self.thresholds is not None:
            if not isinstance(self.thresholds, Mapping):
                raise ValueError("thresholds must be a mapping or null")
            checked: dict[str, float] = {}
            for name, value in self.thresholds.items():
                if not isinstance(name, str) or not name:
                    raise ValueError("thresholds keys must be non-empty strings")
                checked[name] = _finite_number(value, "thresholds")
            object.__setattr__(self, "thresholds", MappingProxyType(checked))


@dataclass(frozen=True)
class RunConfig:
    devices: DeviceConfig
    scan: ScanConfig
    operation_points: tuple[OperationPoint, ...]
    temperature_timeout_s: float = 300.0
    ld_settle_s: float = 5.0
    current_ramp_step_ma: float = 1.0
    current_ramp_interval_s: float = 0.1
    analysis: AnalysisConfig = AnalysisConfig()
    output_root: Path = Path("Result/sil_hysteresis")
    display_latest: bool = True

    def __post_init__(self) -> None:
        if not isinstance(self.devices, DeviceConfig):
            raise ValueError("devices must be a DeviceConfig")
        if not isinstance(self.scan, ScanConfig):
            raise ValueError("scan must be a ScanConfig")
        points = tuple(self.operation_points)
        if not points:
            raise ValueError("operation point collection must not be empty")
        if not all(isinstance(point, OperationPoint) for point in points):
            raise ValueError("operation_points must contain OperationPoint values")
        if len(set(points)) != len(points):
            raise ValueError("duplicate operation point pair")
        if not isinstance(self.analysis, AnalysisConfig):
            raise ValueError("analysis must be an AnalysisConfig")
        if not isinstance(self.output_root, (str, Path)) or not str(self.output_root).strip():
            raise ValueError("output_root must be a non-empty path")
        if not isinstance(self.display_latest, bool):
            raise ValueError("display_latest must be a boolean")
        object.__setattr__(self, "operation_points", points)
        object.__setattr__(
            self,
            "temperature_timeout_s",
            _nonnegative_number(self.temperature_timeout_s, "temperature_timeout_s"),
        )
        object.__setattr__(self, "ld_settle_s", _nonnegative_number(self.ld_settle_s, "ld_settle_s"))
        ramp_step = _finite_number(self.current_ramp_step_ma, "current_ramp_step_ma")
        if not 0.0 < ramp_step <= 1.0:
            raise ValueError("current_ramp_step_ma must be greater than 0 and no greater than 1.0")
        ramp_interval = _finite_number(
            self.current_ramp_interval_s, "current_ramp_interval_s"
        )
        if ramp_interval < 0.05:
            raise ValueError("current_ramp_interval_s must be at least 0.05")
        object.__setattr__(self, "current_ramp_step_ma", ramp_step)
        object.__setattr__(self, "current_ramp_interval_s", ramp_interval)
        object.__setattr__(self, "output_root", Path(self.output_root))

    @classmethod
    def from_mapping(cls, payload: Mapping[str, Any]) -> "RunConfig":
        data = _mapping(payload, "configuration")
        devices_data = _mapping(data.get("devices", {}), "devices")
        scan_data = _mapping(data.get("scan", {}), "scan")
        gain_data = _mapping(data.get("gain", {}), "gain")
        analysis_data = _mapping(data.get("analysis", {}), "analysis")

        devices = DeviceConfig(
            osa_resource=devices_data.get("osa_resource", "GPIB0::4::INSTR"),
            osa_trace=devices_data.get("osa_trace", "A"),
            voltage_port=devices_data.get("voltage_port"),
            gain_port=devices_data.get("gain_port"),
            gain_serial_number=devices_data.get("gain_serial_number"),
        )
        scan = ScanConfig(
            channel=scan_data.get("channel", 1),
            v_min=scan_data.get("v_min", 0.0),
            v_max=scan_data.get("v_max", 13.0),
            points=scan_data.get("points", 100),
            spacing=scan_data.get("spacing", "v_squared"),
            settle_time_s=scan_data.get("settle_time_s", 0.1),
        )
        analysis = AnalysisConfig(
            signal_window_db=analysis_data.get("signal_window_db", 30.0),
            display_floor_dbm=analysis_data.get("display_floor_dbm"),
            thresholds=analysis_data.get("thresholds"),
        )
        operation_points = _parse_operation_points(gain_data)
        return cls(
            devices=devices,
            scan=scan,
            operation_points=operation_points,
            temperature_timeout_s=data.get("temperature_timeout_s", 300.0),
            ld_settle_s=data.get("ld_settle_s", 5.0),
            current_ramp_step_ma=data.get("current_ramp_step_ma", 1.0),
            current_ramp_interval_s=data.get("current_ramp_interval_s", 0.1),
            analysis=analysis,
            output_root=data.get("output_root", Path("Result/sil_hysteresis")),
            display_latest=data.get("display_latest", True),
        )


def _parse_operation_points(gain_data: Mapping[str, Any]) -> tuple[OperationPoint, ...]:
    has_explicit = "operation_points" in gain_data
    has_grid = "grid" in gain_data
    if has_explicit == has_grid:
        raise ValueError("gain must define exactly one of operation_points or grid")
    if has_explicit:
        raw_points = gain_data["operation_points"]
        if isinstance(raw_points, (str, bytes)) or not isinstance(raw_points, (list, tuple)):
            raise ValueError("operation_points must be a collection")
        if not raw_points:
            raise ValueError("operation point collection must not be empty")
        return tuple(_operation_point_from_mapping(item) for item in raw_points)

    grid = _mapping(gain_data["grid"], "gain grid")
    temperatures = grid.get("temperature_c")
    currents = grid.get("current_ma")
    if isinstance(temperatures, (str, bytes)) or not isinstance(temperatures, (list, tuple)) or not temperatures:
        raise ValueError("gain grid temperature_c must be a non-empty collection")
    if isinstance(currents, (str, bytes)) or not isinstance(currents, (list, tuple)) or not currents:
        raise ValueError("gain grid current_ma must be a non-empty collection")
    return tuple(
        OperationPoint(temperature_c, current_ma)
        for temperature_c in temperatures
        for current_ma in currents
    )


def _operation_point_from_mapping(value: Any) -> OperationPoint:
    point = _mapping(value, "operation point")
    if "temperature_c" not in point or "current_ma" not in point:
        raise ValueError("operation point requires temperature_c and current_ma")
    return OperationPoint(point["temperature_c"], point["current_ma"])


def load_config(path: Path) -> RunConfig:
    with Path(path).open("r", encoding="utf-8") as handle:
        payload = json.load(handle)
    return RunConfig.from_mapping(payload)
