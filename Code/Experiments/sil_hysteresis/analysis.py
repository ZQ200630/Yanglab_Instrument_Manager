"""Offline paired analysis for completed SIL hysteresis attempts."""

from __future__ import annotations

import csv
import json
import math
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

import numpy as np

from .config import AnalysisConfig
from .storage import CompletedAttemptData, StoredAcquisition


@dataclass(frozen=True)
class ComparisonResult:
    """Forward/reverse spectra paired at each independently comparable voltage."""

    voltage_v: np.ndarray
    wavelength_nm: np.ndarray
    forward_dbm: np.ndarray
    reverse_dbm: np.ndarray
    difference_db: np.ndarray
    full_mae_db: np.ndarray
    full_rms_db: np.ndarray
    full_correlation: np.ndarray
    signal_mask: np.ndarray
    signal_mae_db: np.ndarray
    signal_rms_db: np.ndarray
    signal_correlation: np.ndarray
    full_wavelength_bin_count: int
    signal_wavelength_bin_count: np.ndarray
    signal_mask_rule: str
    forward_peak_nm: np.ndarray
    reverse_peak_nm: np.ndarray
    peak_shift_nm: np.ndarray
    excluded_turning_points: int
    excluded_turning_point_voltage_v: float
    excluded_turning_point_power_dbm: np.ndarray
    summary: "SummaryMetrics"
    classification: str
    classification_scope: str
    thresholds: Mapping[str, float] | None
    run_mode: str = "real"


@dataclass(frozen=True)
class DiscrepancyPoint:
    """One voltage's rank in the signal-region MAE discrepancy list."""

    voltage_v: float
    voltage_squared_v2: float
    signal_mae_db: float


@dataclass(frozen=True)
class SummaryMetrics:
    """Unrounded aggregates calculated from the per-voltage raw-dBm metrics."""

    median_absolute_difference_db: float
    p95_absolute_difference_db: float
    median_correlation: float
    maximum_absolute_peak_shift_nm: float
    p95_full_mae_db: float
    p95_signal_mae_db: float
    largest_signal_mae_voltage_points: tuple[DiscrepancyPoint, ...]


def _load_spectrum(acquisition: StoredAcquisition) -> tuple[np.ndarray, np.ndarray]:
    try:
        with np.load(acquisition.path, allow_pickle=False) as payload:
            wavelength_nm = np.asarray(payload["wavelength_nm"], dtype=float)
            power_dbm = np.asarray(payload["power_dbm"], dtype=float)
    except (OSError, KeyError, TypeError, ValueError) as error:
        raise ValueError(f"unreadable spectrum: {acquisition.path}") from error
    if wavelength_nm.ndim != 1 or power_dbm.ndim != 1 or wavelength_nm.shape != power_dbm.shape:
        raise ValueError("spectrum arrays are invalid")
    if not np.all(np.isfinite(wavelength_nm)) or not np.all(np.isfinite(power_dbm)):
        raise ValueError("spectrum arrays must be finite")
    return wavelength_nm, power_dbm


def _correlation(first: np.ndarray, second: np.ndarray) -> float:
    """Return an undefined correlation as NaN without NumPy runtime warnings."""
    if len(first) < 2 or np.std(first) == 0.0 or np.std(second) == 0.0:
        return float("nan")
    return float(np.corrcoef(first, second)[0, 1])


def _median_or_nan(values: np.ndarray) -> float:
    finite = values[np.isfinite(values)]
    return float(np.median(finite)) if finite.size else float("nan")


def _validate_thresholds(thresholds: Mapping[str, float]) -> dict[str, float]:
    required = {
        "high_max_p95_mae_db",
        "high_min_median_correlation",
        "different_min_p95_mae_db",
        "different_max_median_correlation",
    }
    if set(thresholds) != required:
        raise ValueError("thresholds must contain exactly the named exploratory threshold keys")
    checked: dict[str, float] = {}
    for name in required:
        value = thresholds[name]
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(float(value)):
            raise ValueError(f"threshold {name} must be a finite number")
        checked[name] = float(value)
    if checked["high_max_p95_mae_db"] < 0.0 or checked["different_min_p95_mae_db"] < 0.0:
        raise ValueError("exploratory MAE thresholds must be non-negative")
    if not -1.0 <= checked["high_min_median_correlation"] <= 1.0:
        raise ValueError("high_min_median_correlation must be between -1 and 1")
    if not -1.0 <= checked["different_max_median_correlation"] <= 1.0:
        raise ValueError("different_max_median_correlation must be between -1 and 1")
    return checked


def _classify(
    summary: SummaryMetrics, thresholds: Mapping[str, float] | None
) -> tuple[str, str, Mapping[str, float] | None]:
    if thresholds is None:
        return "OBSERVATION_REQUIRED", "observation", None
    checked = _validate_thresholds(thresholds)
    if (
        summary.p95_signal_mae_db <= checked["high_max_p95_mae_db"]
        and summary.median_correlation >= checked["high_min_median_correlation"]
    ):
        return "HIGH_CONSISTENCY", "exploratory", checked
    if (
        summary.p95_signal_mae_db >= checked["different_min_p95_mae_db"]
        and summary.median_correlation <= checked["different_max_median_correlation"]
    ):
        return "VISIBLE_DIFFERENCE", "exploratory", checked
    return "REVIEW_RECOMMENDED", "exploratory", checked


def compare_attempt(data: CompletedAttemptData, config: AnalysisConfig) -> ComparisonResult:
    """Align completed forward and reverse observations without resampling them."""
    if not isinstance(data, CompletedAttemptData):
        raise TypeError("data must be a CompletedAttemptData")
    if not isinstance(config, AnalysisConfig):
        raise TypeError("config must be an AnalysisConfig")

    branches: dict[str, dict[int, StoredAcquisition]] = {"forward": {}, "reverse": {}}
    excluded_turning_points = 0
    turning_acquisition: StoredAcquisition | None = None
    for acquisition in data.acquisitions:
        step = acquisition.step
        if step.direction not in branches:
            raise ValueError("acquisition direction must be forward or reverse")
        if step.comparison_index is None:
            excluded_turning_points += 1
            turning_acquisition = acquisition
            continue
        if step.comparison_index in branches[step.direction]:
            raise ValueError("duplicate comparison index")
        branches[step.direction][step.comparison_index] = acquisition

    if excluded_turning_points != 1:
        raise ValueError("attempt must contain exactly one excluded turning point")
    if turning_acquisition is None or turning_acquisition.step.direction != "forward":
        raise ValueError("excluded observation must be the forward turning point")
    if not branches["forward"] or set(branches["forward"]) != set(branches["reverse"]):
        raise ValueError("forward and reverse branches do not share comparison indices")

    comparison_indices = sorted(branches["forward"])
    wavelength_nm: np.ndarray | None = None
    forward_rows: list[np.ndarray] = []
    reverse_rows: list[np.ndarray] = []
    voltage_v: list[float] = []
    for index in comparison_indices:
        forward_acquisition = branches["forward"][index]
        reverse_acquisition = branches["reverse"][index]
        if forward_acquisition.step.voltage_v != reverse_acquisition.step.voltage_v:
            raise ValueError("forward and reverse pair voltage differs")
        forward_wavelength, forward_power = _load_spectrum(forward_acquisition)
        reverse_wavelength, reverse_power = _load_spectrum(reverse_acquisition)
        if not np.array_equal(forward_wavelength, reverse_wavelength):
            raise ValueError("forward and reverse wavelength grid differs")
        if wavelength_nm is None:
            wavelength_nm = forward_wavelength
        elif not np.array_equal(wavelength_nm, forward_wavelength):
            raise ValueError("attempt wavelength grid differs across comparable voltages")
        forward_rows.append(forward_power)
        reverse_rows.append(reverse_power)
        voltage_v.append(forward_acquisition.step.voltage_v)

    if any(second <= first for first, second in zip(voltage_v, voltage_v[1:])):
        raise ValueError("comparable voltages must be strictly ascending")
    if turning_acquisition.step.voltage_v <= voltage_v[-1]:
        raise ValueError("excluded forward turning point must be the maximum voltage")

    forward_dbm = np.vstack(forward_rows)
    reverse_dbm = np.vstack(reverse_rows)
    if turning_acquisition is None or wavelength_nm is None:
        raise ValueError("attempt is missing its turning-point spectrum")
    turning_wavelength, turning_power = _load_spectrum(turning_acquisition)
    if not np.array_equal(wavelength_nm, turning_wavelength):
        raise ValueError("attempt wavelength grid differs at the turning point")

    difference_db = forward_dbm - reverse_dbm
    full_mae_db = np.mean(np.abs(difference_db), axis=1)
    full_rms_db = np.sqrt(np.mean(difference_db**2, axis=1))
    full_correlation = np.asarray(
        [_correlation(forward, reverse) for forward, reverse in zip(forward_dbm, reverse_dbm)]
    )
    forward_peak_nm = wavelength_nm[np.argmax(forward_dbm, axis=1)]
    reverse_peak_nm = wavelength_nm[np.argmax(reverse_dbm, axis=1)]
    peak_shift_nm = forward_peak_nm - reverse_peak_nm

    forward_signal = forward_dbm >= (np.max(forward_dbm, axis=1, keepdims=True) - config.signal_window_db)
    reverse_signal = reverse_dbm >= (np.max(reverse_dbm, axis=1, keepdims=True) - config.signal_window_db)
    signal_mask = np.logical_or(forward_signal, reverse_signal)
    signal_wavelength_bin_count = np.sum(signal_mask, axis=1, dtype=int)
    if np.any(signal_wavelength_bin_count == 0):
        raise ValueError("signal mask is empty for one or more comparable voltages")
    signal_mae_db = np.asarray(
        [np.mean(np.abs(row[mask])) for row, mask in zip(difference_db, signal_mask)]
    )
    signal_rms_db = np.asarray(
        [np.sqrt(np.mean(row[mask] ** 2)) for row, mask in zip(difference_db, signal_mask)]
    )
    signal_correlation = np.asarray(
        [_correlation(forward[mask], reverse[mask]) for forward, reverse, mask in zip(forward_dbm, reverse_dbm, signal_mask)]
    )
    ranking = tuple(
        DiscrepancyPoint(float(voltage_v[index]), float(voltage_v[index] ** 2), float(signal_mae_db[index]))
        for index in np.argsort(-signal_mae_db, kind="stable")[:5]
    )
    summary = SummaryMetrics(
        median_absolute_difference_db=float(np.median(np.abs(difference_db))),
        p95_absolute_difference_db=float(np.percentile(np.abs(difference_db), 95.0)),
        median_correlation=_median_or_nan(full_correlation),
        maximum_absolute_peak_shift_nm=float(np.max(np.abs(peak_shift_nm))),
        p95_full_mae_db=float(np.percentile(full_mae_db, 95.0)),
        p95_signal_mae_db=float(np.percentile(signal_mae_db, 95.0)),
        largest_signal_mae_voltage_points=ranking,
    )
    classification, classification_scope, checked_thresholds = _classify(summary, config.thresholds)
    return ComparisonResult(
        voltage_v=np.asarray(voltage_v),
        wavelength_nm=wavelength_nm,
        forward_dbm=forward_dbm,
        reverse_dbm=reverse_dbm,
        difference_db=difference_db,
        full_mae_db=full_mae_db,
        full_rms_db=full_rms_db,
        full_correlation=full_correlation,
        signal_mask=signal_mask,
        signal_mae_db=signal_mae_db,
        signal_rms_db=signal_rms_db,
        signal_correlation=signal_correlation,
        full_wavelength_bin_count=int(wavelength_nm.size),
        signal_wavelength_bin_count=signal_wavelength_bin_count,
        signal_mask_rule=(
            f"(forward_dbm >= max(forward_dbm) - {config.signal_window_db!r} dB) OR "
            f"(reverse_dbm >= max(reverse_dbm) - {config.signal_window_db!r} dB)"
        ),
        forward_peak_nm=forward_peak_nm,
        reverse_peak_nm=reverse_peak_nm,
        peak_shift_nm=peak_shift_nm,
        excluded_turning_points=excluded_turning_points,
        excluded_turning_point_voltage_v=float(turning_acquisition.step.voltage_v),
        excluded_turning_point_power_dbm=turning_power,
        summary=summary,
        classification=classification,
        classification_scope=classification_scope,
        thresholds=checked_thresholds,
        run_mode=data.run_mode,
    )


def _json_number(value: float) -> float | None:
    return float(value) if math.isfinite(float(value)) else None


def write_metrics_csv(result: ComparisonResult, path: Path) -> None:
    """Write exact per-voltage source data and an adjacent explanatory summary."""
    if not isinstance(result, ComparisonResult):
        raise TypeError("result must be a ComparisonResult")
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fields = (
        "provenance_label",
        "run_mode",
        "simulated",
        "voltage_v",
        "voltage_squared_v2",
        "full_mae_db",
        "full_rms_db",
        "full_correlation",
        "signal_mae_db",
        "signal_rms_db",
        "signal_correlation",
        "full_wavelength_bin_count",
        "signal_wavelength_bin_count",
        "forward_peak_nm",
        "reverse_peak_nm",
        "peak_shift_nm",
    )
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        for index, voltage in enumerate(result.voltage_v):
            simulated = result.run_mode != "real"
            writer.writerow(
                {
                    "provenance_label": "SIMULATED" if simulated else "REAL",
                    "run_mode": result.run_mode,
                    "simulated": simulated,
                    "voltage_v": float(voltage),
                    "voltage_squared_v2": float(voltage**2),
                    "full_mae_db": float(result.full_mae_db[index]),
                    "full_rms_db": float(result.full_rms_db[index]),
                    "full_correlation": float(result.full_correlation[index]),
                    "signal_mae_db": float(result.signal_mae_db[index]),
                    "signal_rms_db": float(result.signal_rms_db[index]),
                    "signal_correlation": float(result.signal_correlation[index]),
                    "full_wavelength_bin_count": result.full_wavelength_bin_count,
                    "signal_wavelength_bin_count": int(result.signal_wavelength_bin_count[index]),
                    "forward_peak_nm": float(result.forward_peak_nm[index]),
                    "reverse_peak_nm": float(result.reverse_peak_nm[index]),
                    "peak_shift_nm": float(result.peak_shift_nm[index]),
                }
            )
    summary = result.summary
    payload: dict[str, Any] = {
        "provenance": {
            "label": "SIMULATED" if result.run_mode != "real" else "REAL",
            "run_mode": result.run_mode,
            "simulated": result.run_mode != "real",
        },
        "aggregation_definitions": {
            "difference_db": "forward_dbm - reverse_dbm, evaluated without resampling or rounding",
            "full_metrics": "per-voltage metrics use every wavelength bin",
            "signal_mask_rule": result.signal_mask_rule,
            "p95_signal_mae_db": "95th percentile of per-voltage signal-region MAE values",
        },
        "classification": {"state": result.classification, "scope": result.classification_scope},
        "thresholds": dict(result.thresholds) if result.thresholds is not None else None,
        "excluded_turning_points": result.excluded_turning_points,
        "excluded_turning_point_voltage_v": result.excluded_turning_point_voltage_v,
        "summary_metrics": {
            "median_absolute_difference_db": _json_number(summary.median_absolute_difference_db),
            "p95_absolute_difference_db": _json_number(summary.p95_absolute_difference_db),
            "median_correlation": _json_number(summary.median_correlation),
            "maximum_absolute_peak_shift_nm": _json_number(summary.maximum_absolute_peak_shift_nm),
            "p95_full_mae_db": _json_number(summary.p95_full_mae_db),
            "p95_signal_mae_db": _json_number(summary.p95_signal_mae_db),
        },
        "discrepancy_ranking": [
            {
                "voltage_v": point.voltage_v,
                "voltage_squared_v2": point.voltage_squared_v2,
                "signal_mae_db": point.signal_mae_db,
            }
            for point in summary.largest_signal_mae_voltage_points
        ],
    }
    with path.with_name("summary.json").open("w", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2, allow_nan=False)
        handle.write("\n")
