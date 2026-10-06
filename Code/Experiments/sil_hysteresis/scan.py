"""Deterministic voltage scheduling and rolling acquisition-time estimates."""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

from .config import ScanConfig


@dataclass(frozen=True)
class ScanStep:
    sequence_index: int
    direction: str
    direction_index: int
    voltage_v: float
    comparison_index: int | None


def build_voltage_grid(scan: ScanConfig) -> np.ndarray:
    """Build an inclusive grid that is uniform in squared voltage."""
    if not isinstance(scan, ScanConfig):
        raise TypeError("scan must be a ScanConfig")
    squared_grid = np.linspace(scan.v_min**2, scan.v_max**2, scan.points)
    grid = np.sqrt(squared_grid)
    grid[0] = scan.v_min
    grid[-1] = scan.v_max
    return grid


def build_scan_steps(scan: ScanConfig) -> tuple[ScanStep, ...]:
    """Create one continuous forward/reverse scan without duplicate turnaround."""
    grid = build_voltage_grid(scan)
    forward = tuple(
        ScanStep(
            sequence_index=index,
            direction="forward",
            direction_index=index,
            voltage_v=float(voltage),
            comparison_index=index if index < len(grid) - 1 else None,
        )
        for index, voltage in enumerate(grid)
    )
    reverse = tuple(
        ScanStep(
            sequence_index=len(forward) + direction_index,
            direction="reverse",
            direction_index=direction_index,
            voltage_v=float(voltage),
            comparison_index=len(grid) - 2 - direction_index,
        )
        for direction_index, voltage in enumerate(grid[-2::-1])
    )
    return forward + reverse


@dataclass
class TimingTracker:
    """Accumulates OSA timing and estimates the remaining scheduled duration."""

    count: int = 0
    total_s: float = 0.0

    @property
    def mean_s(self) -> float:
        return self.total_s / self.count if self.count else 0.0

    def record_acquisition(self, seconds: float) -> None:
        if isinstance(seconds, bool) or not isinstance(seconds, (int, float)):
            raise ValueError("acquisition duration must be a non-negative finite number")
        seconds = float(seconds)
        if not math.isfinite(seconds) or seconds < 0:
            raise ValueError("acquisition duration must be a non-negative finite number")
        self.count += 1
        self.total_s += seconds

    def estimate_remaining(
        self,
        remaining_acquisitions: int,
        remaining_settle_s: float,
        remaining_voltage_ramp_s: float,
    ) -> float | None:
        if self.count == 0:
            return None
        if isinstance(remaining_acquisitions, bool) or not isinstance(remaining_acquisitions, int):
            raise ValueError("remaining_acquisitions must be a non-negative integer")
        if remaining_acquisitions < 0:
            raise ValueError("remaining_acquisitions must be a non-negative integer")
        for value, name in (
            (remaining_settle_s, "remaining_settle_s"),
            (remaining_voltage_ramp_s, "remaining_voltage_ramp_s"),
        ):
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                raise ValueError(f"{name} must be a non-negative finite number")
            if not math.isfinite(float(value)) or value < 0:
                raise ValueError(f"{name} must be a non-negative finite number")
        return self.mean_s * remaining_acquisitions + float(remaining_settle_s) + float(remaining_voltage_ramp_s)
