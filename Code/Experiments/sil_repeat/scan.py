"""Deterministic V-squared scheduling for repeated SIL scans."""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

from .config import RepeatScanConfig


@dataclass(frozen=True)
class RepeatStep:
    sequence_index: int
    cycle_index: int
    direction: str
    branch_index: int
    pair_index: int
    voltage_squared_v2: float
    voltage_v: float


def build_v2_grid(scan: RepeatScanConfig) -> np.ndarray:
    if not isinstance(scan, RepeatScanConfig):
        raise TypeError("scan must be a RepeatScanConfig")
    grid = np.linspace(scan.v2_min, scan.v2_max, scan.points_per_branch)
    grid[0] = scan.v2_min
    grid[-1] = scan.v2_max
    return grid


def build_repeat_schedule(scan: RepeatScanConfig) -> tuple[RepeatStep, ...]:
    grid = build_v2_grid(scan)
    steps: list[RepeatStep] = []
    for cycle_index in range(scan.cycles):
        for direction, branch in (("forward", grid), ("reverse", grid[::-1])):
            for branch_index, voltage_squared in enumerate(branch):
                pair_index = (
                    branch_index
                    if direction == "forward"
                    else scan.points_per_branch - 1 - branch_index
                )
                steps.append(
                    RepeatStep(
                        sequence_index=len(steps),
                        cycle_index=cycle_index,
                        direction=direction,
                        branch_index=branch_index,
                        pair_index=pair_index,
                        voltage_squared_v2=float(voltage_squared),
                        voltage_v=math.sqrt(float(voltage_squared)),
                    )
                )
    return tuple(steps)


def cycle_steps(schedule: tuple[RepeatStep, ...], cycle_index: int) -> tuple[RepeatStep, ...]:
    if isinstance(cycle_index, bool) or not isinstance(cycle_index, int) or cycle_index < 0:
        raise ValueError("cycle_index must be a non-negative integer")
    selected = tuple(step for step in schedule if step.cycle_index == cycle_index)
    if not selected:
        raise ValueError("cycle_index is outside the schedule")
    return selected
