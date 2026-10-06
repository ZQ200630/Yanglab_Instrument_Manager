"""Cycle comparison and aggregate metrics for repeated SIL scans."""

from __future__ import annotations

import csv
import uuid
from dataclasses import dataclass
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np

from Code.Experiments.sil_hysteresis.analysis import (
    ComparisonResult,
    compare_attempt,
    write_metrics_csv,
)
from Code.Experiments.sil_hysteresis.config import AnalysisConfig
from Code.Experiments.sil_hysteresis.figure import (
    OperationMetadata,
    build_comparison_figure,
    save_comparison_figure,
)
from Code.Experiments.sil_hysteresis.scan import ScanStep
from Code.Experiments.sil_hysteresis.storage import (
    CompletedAttemptData,
    StoredAcquisition,
)

from .config import RepeatRunConfig
from .figure import annotate_cycle, save_repeat_summary_figure
from .storage import CycleData, RepeatStoredAcquisition


@dataclass(frozen=True)
class CycleSummaryRow:
    cycle_index: int
    p95_signal_mae_db: float
    maximum_absolute_peak_shift_nm: float
    largest_discrepancy_v2: float
    mean_osa_duration_s: float
    started_at: str
    completed_at: str
    classification: str
    run_mode: str


@dataclass(frozen=True)
class CycleProducts:
    result: ComparisonResult
    summary_row: CycleSummaryRow
    figure_paths: tuple[Path, ...]


def _adapt_acquisition(
    acquisition: RepeatStoredAcquisition,
    *,
    direction_index: int,
    comparison_index: int | None,
) -> StoredAcquisition:
    step = ScanStep(
        sequence_index=acquisition.step.sequence_index,
        direction=acquisition.step.direction,
        direction_index=direction_index,
        voltage_v=acquisition.step.voltage_v,
        comparison_index=comparison_index,
    )
    return StoredAcquisition(
        path=acquisition.path,
        step=step,
        acquired_at=acquisition.acquired_at,
        osa_duration_s=acquisition.osa_duration_s,
        trace=acquisition.trace,
        identity=acquisition.identity,
    )


def compare_cycle(data: CycleData, config: AnalysisConfig) -> ComparisonResult:
    if not isinstance(data, CycleData):
        raise TypeError("data must be CycleData")
    if not isinstance(config, AnalysisConfig):
        raise TypeError("config must be AnalysisConfig")
    forward = sorted(
        (item for item in data.acquisitions if item.step.direction == "forward"),
        key=lambda item: item.step.branch_index,
    )
    reverse = sorted(
        (item for item in data.acquisitions if item.step.direction == "reverse"),
        key=lambda item: item.step.branch_index,
    )
    if len(forward) != len(reverse) or len(forward) < 2:
        raise ValueError("cycle must contain equal complete branches")
    count = len(forward)
    if {item.step.pair_index for item in forward} != set(range(count)):
        raise ValueError("forward pair indices are incomplete")
    if {item.step.pair_index for item in reverse} != set(range(count)):
        raise ValueError("reverse pair indices are incomplete")
    adapted: list[StoredAcquisition] = []
    for item in forward:
        comparison_index = None if item.step.pair_index == count - 1 else item.step.pair_index
        adapted.append(
            _adapt_acquisition(
                item,
                direction_index=item.step.branch_index,
                comparison_index=comparison_index,
            )
        )
    reverse_comparable = [item for item in reverse if item.step.pair_index != count - 1]
    for direction_index, item in enumerate(reverse_comparable):
        adapted.append(
            _adapt_acquisition(
                item,
                direction_index=direction_index,
                comparison_index=item.step.pair_index,
            )
        )
    return compare_attempt(
        CompletedAttemptData(
            path=data.path,
            operation_index=data.cycle_index,
            attempt_number=data.attempt_number,
            acquisitions=tuple(adapted),
            run_mode=data.run_mode,
        ),
        config,
    )


def _summary_row(data: CycleData, result: ComparisonResult) -> CycleSummaryRow:
    largest = result.summary.largest_signal_mae_voltage_points[0]
    return CycleSummaryRow(
        cycle_index=data.cycle_index,
        p95_signal_mae_db=result.summary.p95_signal_mae_db,
        maximum_absolute_peak_shift_nm=result.summary.maximum_absolute_peak_shift_nm,
        largest_discrepancy_v2=largest.voltage_squared_v2,
        mean_osa_duration_s=float(np.mean([item.osa_duration_s for item in data.acquisitions])),
        started_at=data.acquisitions[0].acquired_at,
        completed_at=data.acquisitions[-1].acquired_at,
        classification=result.classification,
        run_mode=data.run_mode,
    )


def write_cycle_products(data: CycleData, config: RepeatRunConfig) -> CycleProducts:
    result = compare_cycle(data, config.analysis)
    write_metrics_csv(result, data.path / "metrics.csv")
    metadata = OperationMetadata(
        config.operation_point.temperature_c,
        config.operation_point.current_ma,
        data.cycle_index,
        data.run_mode,
    )
    figure = annotate_cycle(
        build_comparison_figure(result, metadata),
        data.cycle_index,
        config.scan.cycles,
    )
    try:
        figure_paths = save_comparison_figure(figure, data.path / "comparison")
    finally:
        plt.close(figure)
    return CycleProducts(result, _summary_row(data, result), tuple(figure_paths))


def _summary_record(row: CycleSummaryRow) -> dict[str, object]:
    return {
        "cycle_index": row.cycle_index,
        "p95_signal_mae_db": row.p95_signal_mae_db,
        "maximum_absolute_peak_shift_nm": row.maximum_absolute_peak_shift_nm,
        "largest_discrepancy_v2": row.largest_discrepancy_v2,
        "mean_osa_duration_s": row.mean_osa_duration_s,
        "started_at": row.started_at,
        "completed_at": row.completed_at,
        "classification": row.classification,
        "run_mode": row.run_mode,
    }


def write_repeat_summary(run_path: Path, rows: tuple[CycleSummaryRow, ...]) -> Path:
    ordered = sorted(rows, key=lambda row: row.cycle_index)
    if len({row.cycle_index for row in ordered}) != len(ordered):
        raise ValueError("repeat summary contains duplicate cycle indices")
    path = Path(run_path) / "repeat_summary.csv"
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    try:
        with temporary.open("w", newline="", encoding="utf-8") as handle:
            fields = tuple(_summary_record(ordered[0])) if ordered else (
                "cycle_index",
                "p95_signal_mae_db",
                "maximum_absolute_peak_shift_nm",
                "largest_discrepancy_v2",
                "mean_osa_duration_s",
                "started_at",
                "completed_at",
                "classification",
                "run_mode",
            )
            writer = csv.DictWriter(handle, fieldnames=fields)
            writer.writeheader()
            writer.writerows(_summary_record(row) for row in ordered)
        temporary.replace(path)
    finally:
        if temporary.exists():
            temporary.unlink()
    return path


__all__ = [
    "CycleProducts",
    "CycleSummaryRow",
    "compare_cycle",
    "save_repeat_summary_figure",
    "write_cycle_products",
    "write_repeat_summary",
]
