"""Validated immutable configuration for repeated SIL cycle scans."""

from __future__ import annotations

import json
import math
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from Code.Experiments.sil_hysteresis.config import (
    AnalysisConfig,
    DeviceConfig,
    OperationPoint,
)


def _finite(value: Any, name: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{name} must be a finite number")
    number = float(value)
    if not math.isfinite(number):
        raise ValueError(f"{name} must be a finite number")
    return number


def _nonnegative(value: Any, name: str) -> float:
    number = _finite(value, name)
    if number < 0.0:
        raise ValueError(f"{name} must be non-negative")
    return number


def _mapping(value: Any, name: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ValueError(f"{name} must be a mapping")
    return value


@dataclass(frozen=True)
class RepeatScanConfig:
    channel: int = 1
    v2_min: float = 40.0
    v2_max: float = 90.0
    points_per_branch: int = 40
    cycles: int = 100
    settle_time_s: float = 0.1

    def __post_init__(self) -> None:
        if isinstance(self.channel, bool) or not isinstance(self.channel, int) or not 1 <= self.channel <= 8:
            raise ValueError("channel must be an integer between 1 and 8")
        if (
            isinstance(self.points_per_branch, bool)
            or not isinstance(self.points_per_branch, int)
            or self.points_per_branch < 2
        ):
            raise ValueError("points_per_branch must be an integer of at least two")
        if isinstance(self.cycles, bool) or not isinstance(self.cycles, int) or self.cycles < 1:
            raise ValueError("cycles must be a positive integer")
        v2_min = _finite(self.v2_min, "v2_min")
        v2_max = _finite(self.v2_max, "v2_max")
        if not 0.0 <= v2_min < v2_max <= 14.0**2:
            raise ValueError("V squared bounds must satisfy 0 <= v2_min < v2_max <= 196")
        object.__setattr__(self, "v2_min", v2_min)
        object.__setattr__(self, "v2_max", v2_max)
        object.__setattr__(self, "settle_time_s", _nonnegative(self.settle_time_s, "settle_time_s"))


@dataclass(frozen=True)
class RepeatRunConfig:
    devices: DeviceConfig
    scan: RepeatScanConfig
    operation_point: OperationPoint
    temperature_timeout_s: float = 300.0
    ld_settle_s: float = 5.0
    current_ramp_step_ma: float = 1.0
    current_ramp_interval_s: float = 0.1
    analysis: AnalysisConfig = AnalysisConfig()
    output_root: Path = Path("Result/sil_repeat")
    display_latest: bool = True

    def __post_init__(self) -> None:
        if not isinstance(self.devices, DeviceConfig):
            raise ValueError("devices must be a DeviceConfig")
        if not isinstance(self.scan, RepeatScanConfig):
            raise ValueError("scan must be a RepeatScanConfig")
        if not isinstance(self.operation_point, OperationPoint):
            raise ValueError("operation_point must be an OperationPoint")
        if not isinstance(self.analysis, AnalysisConfig):
            raise ValueError("analysis must be an AnalysisConfig")
        if not isinstance(self.output_root, (str, Path)) or not str(self.output_root).strip():
            raise ValueError("output_root must be a non-empty path")
        if not isinstance(self.display_latest, bool):
            raise ValueError("display_latest must be a boolean")
        ramp_step = _finite(self.current_ramp_step_ma, "current_ramp_step_ma")
        if not 0.0 < ramp_step <= 1.0:
            raise ValueError("current_ramp_step_ma must be greater than 0 and no greater than 1.0")
        ramp_interval = _finite(self.current_ramp_interval_s, "current_ramp_interval_s")
        if ramp_interval < 0.05:
            raise ValueError("current_ramp_interval_s must be at least 0.05")
        object.__setattr__(self, "temperature_timeout_s", _nonnegative(self.temperature_timeout_s, "temperature_timeout_s"))
        object.__setattr__(self, "ld_settle_s", _nonnegative(self.ld_settle_s, "ld_settle_s"))
        object.__setattr__(self, "current_ramp_step_ma", ramp_step)
        object.__setattr__(self, "current_ramp_interval_s", ramp_interval)
        object.__setattr__(self, "output_root", Path(self.output_root))

    @classmethod
    def from_mapping(cls, payload: Mapping[str, Any]) -> "RepeatRunConfig":
        data = _mapping(payload, "configuration")
        devices_data = _mapping(data.get("devices", {}), "devices")
        scan_data = _mapping(data.get("scan", {}), "scan")
        gain_data = _mapping(data.get("gain", {}), "gain")
        analysis_data = _mapping(data.get("analysis", {}), "analysis")
        if "temperature_c" not in gain_data or "current_ma" not in gain_data:
            raise ValueError("gain requires temperature_c and current_ma")
        return cls(
            devices=DeviceConfig(
                osa_resource=devices_data.get("osa_resource", "GPIB0::4::INSTR"),
                osa_trace=devices_data.get("osa_trace", "A"),
                voltage_port=devices_data.get("voltage_port"),
                gain_port=devices_data.get("gain_port"),
                gain_serial_number=devices_data.get("gain_serial_number"),
            ),
            scan=RepeatScanConfig(
                channel=scan_data.get("channel", 1),
                v2_min=scan_data.get("v2_min", 40.0),
                v2_max=scan_data.get("v2_max", 90.0),
                points_per_branch=scan_data.get("points_per_branch", 40),
                cycles=scan_data.get("cycles", 100),
                settle_time_s=scan_data.get("settle_time_s", 0.1),
            ),
            operation_point=OperationPoint(gain_data["temperature_c"], gain_data["current_ma"]),
            temperature_timeout_s=data.get("temperature_timeout_s", 300.0),
            ld_settle_s=data.get("ld_settle_s", 5.0),
            current_ramp_step_ma=data.get("current_ramp_step_ma", 1.0),
            current_ramp_interval_s=data.get("current_ramp_interval_s", 0.1),
            analysis=AnalysisConfig(
                signal_window_db=analysis_data.get("signal_window_db", 30.0),
                display_floor_dbm=analysis_data.get("display_floor_dbm"),
                thresholds=analysis_data.get("thresholds"),
            ),
            output_root=data.get("output_root", "Result/sil_repeat"),
            display_latest=data.get("display_latest", True),
        )


def load_repeat_config(path: Path) -> RepeatRunConfig:
    with Path(path).open("r", encoding="utf-8") as handle:
        return RepeatRunConfig.from_mapping(json.load(handle))
