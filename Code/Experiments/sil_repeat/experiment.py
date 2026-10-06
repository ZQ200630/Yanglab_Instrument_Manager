"""Fail-safe orchestration for repeated SIL forward/reverse cycles."""

from __future__ import annotations

import csv
import json
import logging
import math
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from Code.Experiments.sil_hysteresis.scan import TimingTracker
from Code.Setups.session import InstrumentSession, safe_shutdown
from Code.Utils.common import InstrumentSafetyError

from .config import RepeatRunConfig
from .postprocess import PostprocessRequest
from .scan import RepeatStep, cycle_steps
from .storage import CycleData, RepeatRunStore


LOGGER = logging.getLogger(__name__)
_PRODUCT_NAMES = ("metrics.csv", "summary.json", "comparison.svg", "comparison.pdf", "comparison.png")
_METRICS_FIELDS = (
    "provenance_label", "run_mode", "simulated", "voltage_v",
    "voltage_squared_v2", "full_mae_db", "full_rms_db", "full_correlation",
    "signal_mae_db", "signal_rms_db", "signal_correlation",
    "full_wavelength_bin_count", "signal_wavelength_bin_count",
    "forward_peak_nm", "reverse_peak_nm", "peak_shift_nm",
)
_SUMMARY_FIELDS = (
    "cycle_index", "p95_signal_mae_db", "maximum_absolute_peak_shift_nm",
    "largest_discrepancy_v2", "mean_osa_duration_s", "started_at",
    "completed_at", "classification", "run_mode",
)
_CYCLE_SUMMARY_KEYS = {
    "provenance", "aggregation_definitions", "classification", "thresholds",
    "excluded_turning_points", "excluded_turning_point_voltage_v",
    "summary_metrics", "discrepancy_ranking",
}
_SUMMARY_METRIC_KEYS = {
    "median_absolute_difference_db", "p95_absolute_difference_db",
    "median_correlation", "maximum_absolute_peak_shift_nm", "p95_full_mae_db",
    "p95_signal_mae_db",
}


def _finite_number(value: Any) -> bool:
    return (
        not isinstance(value, bool)
        and isinstance(value, (int, float))
        and math.isfinite(float(value))
    )


def _valid_cycle_summary(
    summary: Any,
    data: CycleData,
    config: RepeatRunConfig,
    expected_label: str,
    expected_v2: list[float],
) -> bool:
    if not isinstance(summary, dict) or set(summary) != _CYCLE_SUMMARY_KEYS:
        return False
    if summary["provenance"] != {
        "label": expected_label,
        "run_mode": data.run_mode,
        "simulated": data.run_mode != "real",
    }:
        return False
    definitions = summary["aggregation_definitions"]
    if (
        not isinstance(definitions, dict)
        or set(definitions) != {
            "difference_db", "full_metrics", "signal_mask_rule", "p95_signal_mae_db"
        }
        or not all(isinstance(value, str) and value for value in definitions.values())
    ):
        return False
    expected_thresholds = (
        dict(config.analysis.thresholds) if config.analysis.thresholds is not None else None
    )
    if summary["thresholds"] != expected_thresholds:
        return False
    classification = summary["classification"]
    if not isinstance(classification, dict) or set(classification) != {"state", "scope"}:
        return False
    if expected_thresholds is None:
        if classification != {"state": "OBSERVATION_REQUIRED", "scope": "observation"}:
            return False
    elif (
        classification["scope"] != "exploratory"
        or classification["state"]
        not in {"HIGH_CONSISTENCY", "VISIBLE_DIFFERENCE", "REVIEW_RECOMMENDED"}
    ):
        return False
    if summary["excluded_turning_points"] != 1 or not _finite_number(
        summary["excluded_turning_point_voltage_v"]
    ):
        return False
    if not math.isclose(
        float(summary["excluded_turning_point_voltage_v"]),
        math.sqrt(config.scan.v2_max),
        rel_tol=0.0,
        abs_tol=1e-9,
    ):
        return False
    metrics = summary["summary_metrics"]
    if not isinstance(metrics, dict) or set(metrics) != _SUMMARY_METRIC_KEYS:
        return False
    for name, value in metrics.items():
        if value is None and name == "median_correlation":
            continue
        if not _finite_number(value):
            return False
    ranking = summary["discrepancy_ranking"]
    if not isinstance(ranking, list) or len(ranking) != min(5, len(expected_v2)):
        return False
    ranking_v2: list[float] = []
    ranking_mae: list[float] = []
    for point in ranking:
        if not isinstance(point, dict) or set(point) != {
            "voltage_v", "voltage_squared_v2", "signal_mae_db"
        }:
            return False
        if not all(_finite_number(point[name]) for name in point):
            return False
        voltage = float(point["voltage_v"])
        voltage_squared = float(point["voltage_squared_v2"])
        signal_mae = float(point["signal_mae_db"])
        if signal_mae < 0.0 or not math.isclose(
            voltage * voltage, voltage_squared, rel_tol=0.0, abs_tol=1e-9
        ):
            return False
        if not any(
            math.isclose(voltage_squared, expected, rel_tol=0.0, abs_tol=1e-9)
            for expected in expected_v2
        ):
            return False
        ranking_v2.append(voltage_squared)
        ranking_mae.append(signal_mae)
    if len({round(value, 9) for value in ranking_v2}) != len(ranking_v2):
        return False
    return ranking_mae == sorted(ranking_mae, reverse=True)


@dataclass(frozen=True)
class RepeatCycleSummary:
    cycle_index: int
    acquisitions: int
    attempt_path: Path


@dataclass(frozen=True)
class RepeatRunSummary:
    run_path: Path
    acquisitions: int
    completed_cycles: int
    skipped_cycles: int
    postprocessed_only_cycles: int
    cycles: tuple[RepeatCycleSummary, ...]


def _remaining_voltage_ramp_s(current_voltage_v: float, remaining: tuple[RepeatStep, ...]) -> float:
    previous = float(current_voltage_v)
    total_steps = 0
    for step in remaining:
        total_steps += math.ceil(abs(step.voltage_v - previous) / 0.1)
        previous = step.voltage_v
    return total_steps * 0.05


class RepeatExperimentRunner:
    def __init__(
        self,
        config: RepeatRunConfig,
        store: RepeatRunStore,
        instrument_bundle_factory: Callable[[], Any],
        *,
        postprocessor: Any,
        coupling_gate: Callable[[Any], None] | None = None,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], None] = time.sleep,
        log: logging.Logger = LOGGER,
    ) -> None:
        if not isinstance(config, RepeatRunConfig):
            raise TypeError("config must be a RepeatRunConfig")
        if not isinstance(store, RepeatRunStore):
            raise TypeError("store must be a RepeatRunStore")
        if not callable(instrument_bundle_factory):
            raise TypeError("instrument_bundle_factory must be callable")
        if coupling_gate is not None and not callable(coupling_gate):
            raise TypeError("coupling_gate must be callable or None")
        for name in ("submit", "check", "close_and_drain"):
            if not callable(getattr(postprocessor, name, None)):
                raise TypeError("postprocessor does not implement the required interface")
        self.config = config
        self.store = store
        self.instrument_bundle_factory = instrument_bundle_factory
        self.postprocessor = postprocessor
        self.coupling_gate = coupling_gate
        self.clock = clock
        self.sleep = sleep
        self.log = log
        self._osa: Any = None
        self._voltage: Any = None
        self._gain: Any = None
        self._session: InstrumentSession | None = None
        self._session_connected = False
        self._cleanup_reports: tuple[Any, ...] = ()

    @staticmethod
    def _valid_graphics(base: Path) -> bool:
        return (
            base.with_suffix(".pdf").read_bytes().startswith(b"%PDF")
            and base.with_suffix(".png").read_bytes().startswith(b"\x89PNG\r\n\x1a\n")
            and "<svg" in base.with_suffix(".svg").read_text(encoding="utf-8")[:4096]
        )

    @staticmethod
    def _products_complete(data: CycleData, config: RepeatRunConfig) -> bool:
        path = data.path
        products = {name: path / name for name in _PRODUCT_NAMES}
        if not all(item.is_file() and item.stat().st_size > 0 for item in products.values()):
            return False
        try:
            with products["summary.json"].open(encoding="utf-8") as handle:
                summary = json.load(handle)
            with products["metrics.csv"].open(newline="", encoding="utf-8") as handle:
                reader = csv.DictReader(handle)
                metrics = list(reader)
            expected_label = "REAL" if data.run_mode == "real" else "SIMULATED"
            expected_v2 = sorted(
                acquisition.step.voltage_squared_v2
                for acquisition in data.acquisitions
                if acquisition.step.direction == "forward"
                and acquisition.step.pair_index < config.scan.points_per_branch - 1
            )
            return (
                tuple(reader.fieldnames or ()) == _METRICS_FIELDS
                and len(metrics) == config.scan.points_per_branch - 1
                and all(row["run_mode"] == data.run_mode for row in metrics)
                and all(row["provenance_label"] == expected_label for row in metrics)
                and all(row["simulated"] == str(data.run_mode != "real") for row in metrics)
                and all(
                    math.isclose(actual, expected, rel_tol=0.0, abs_tol=1e-9)
                    for actual, expected in zip(
                        sorted(float(row["voltage_squared_v2"]) for row in metrics),
                        expected_v2,
                        strict=True,
                    )
                )
                and _valid_cycle_summary(
                    summary, data, config, expected_label, expected_v2
                )
                and RepeatExperimentRunner._valid_graphics(path / "comparison")
            )
        except (OSError, KeyError, TypeError, ValueError, UnicodeError, json.JSONDecodeError):
            return False

    def _run_products_complete(self, expected_cycles: set[int]) -> bool:
        if not expected_cycles:
            return True
        paths = (
            self.store.path / "repeat_summary.csv",
            self.store.path / "repeat_summary.svg",
            self.store.path / "repeat_summary.pdf",
            self.store.path / "repeat_summary.png",
        )
        if not all(path.is_file() and path.stat().st_size > 0 for path in paths):
            return False
        try:
            with paths[0].open(newline="", encoding="utf-8") as handle:
                reader = csv.DictReader(handle)
                rows = list(reader)
            indices = [int(row["cycle_index"]) for row in rows]
            numeric_fields = _SUMMARY_FIELDS[1:5]
            return (
                tuple(reader.fieldnames or ()) == _SUMMARY_FIELDS
                and len(rows) == len(expected_cycles)
                and len(indices) == len(set(indices))
                and set(indices) == expected_cycles
                and all(row["run_mode"] == self.store.run_mode for row in rows)
                and all(row["classification"] for row in rows)
                and all(
                    math.isfinite(float(row[field]))
                    for row in rows
                    for field in numeric_fields
                )
                and self._valid_graphics(self.store.path / "repeat_summary")
            )
        except (OSError, KeyError, TypeError, ValueError, UnicodeError):
            return False

    def _submit_existing_missing_products(self) -> int:
        submitted = 0
        pending = set(self.store.pending_cycle_indices())
        completed = set(range(self.config.scan.cycles)) - pending
        rebuild_run_products = not self._run_products_complete(completed)
        for cycle_index in range(self.config.scan.cycles):
            if cycle_index in pending:
                continue
            data = self.store.completed_cycle_data(cycle_index)
            if (
                rebuild_run_products
                or data.state == "acquired"
                or not self._products_complete(data, self.config)
            ):
                self.postprocessor.submit(
                    PostprocessRequest(
                        cycle_index,
                        data.path,
                        reset_aggregate=rebuild_run_products and submitted == 0,
                    )
                )
                submitted += 1
        return submitted

    def _validate_final_products(self) -> None:
        problems: list[str] = []
        expected = set(range(self.config.scan.cycles))
        for cycle_index in sorted(expected):
            try:
                data = self.store.completed_cycle_data(cycle_index)
            except ValueError:
                problems.append(f"cycle {cycle_index} acquisition incomplete")
                continue
            if data.state != "complete":
                problems.append(f"cycle {cycle_index} state={data.state}")
            if not self._products_complete(data, self.config):
                problems.append(f"cycle {cycle_index} products invalid")
        if not self._run_products_complete(expected):
            problems.append("aggregate products invalid")
        if problems:
            raise RuntimeError("final result validation failed: " + "; ".join(problems))

    def _connect_and_prepare(self) -> None:
        if self._session is not None:
            raise InstrumentSafetyError(
                "runner retains an unreleased instrument session; close it explicitly "
                "before creating a new runner"
            )
        bundle = self.instrument_bundle_factory()
        self._osa = bundle.osa
        self._voltage = bundle.voltage
        self._gain = bundle.gain
        for device in (self._osa, self._voltage, self._gain):
            if hasattr(device, "log"):
                device.log = self.log
        self._session = InstrumentSession(
            osa=self._osa,
            voltage=self._voltage,
            gain=self._gain,
            log=self.log,
        )
        self._session.connect()
        self._session_connected = True
        self._session.check_health()
        self._gain.read_status()
        self._session.check_health()
        self._voltage.zero(emergency=True)
        self._session.check_health()
        self._gain.disable_current()
        point = self.config.operation_point
        self._session.check_health()
        self._gain.set_temperature(point.temperature_c)
        status = self._gain.read_status()
        if not status.tec_enabled:
            self._session.check_health()
            self._gain.enable_tec()
        self._gain.wait_stable(self.config.temperature_timeout_s)
        self._session.check_health()
        self._gain.disable_current()
        self._session.check_health()
        self._gain.enable_current()
        self._session.check_health()
        self._gain.ramp_current(
            point.current_ma,
            step_ma=self.config.current_ramp_step_ma,
            interval_s=self.config.current_ramp_interval_s,
        )
        self.sleep(self.config.ld_settle_s)
        self._session.check_health()
        self._voltage.zero(emergency=True)
        if self.coupling_gate is not None:
            self.coupling_gate(point)

    def _run_cycle(
        self,
        cycle_index: int,
        tracker: TimingTracker,
        run_steps: tuple[RepeatStep, ...],
    ) -> RepeatCycleSummary:
        attempt = self.store.start_cycle_attempt(cycle_index)
        steps = cycle_steps(self.store.schedule, cycle_index)
        if self._session is None:
            raise RuntimeError("instrument session is not available")
        try:
            for step in steps:
                self.log.info(
                    "Repeat scan command cycle=%d direction=%s point=%d sequence=%d channel=%d requested_v=%.6f v2=%.6f",
                    cycle_index,
                    step.direction,
                    step.branch_index,
                    step.sequence_index,
                    self.config.scan.channel,
                    step.voltage_v,
                    step.voltage_squared_v2,
                )
                self._session.check_health()
                self._voltage.set_channel(self.config.scan.channel, step.voltage_v)
                voltage_status = self._voltage.wait_for_channel(
                    self.config.scan.channel,
                    step.voltage_v,
                )
                self.sleep(self.config.scan.settle_time_s)
                started = self.clock()
                spectrum = self._osa.acquire(trace=self.config.devices.osa_trace)
                self._session.check_health()
                osa_duration_s = self.clock() - started
                self._gain.read_temperature()
                gain_status = self._gain.read_status()
                attempt.save_acquisition(
                    step=step,
                    spectrum=spectrum,
                    voltage_status=voltage_status,
                    gain_status=gain_status,
                    osa_duration_s=osa_duration_s,
                )
                tracker.record_acquisition(osa_duration_s)
                remaining_global = len(run_steps) - tracker.count
                measured_voltage = voltage_status.voltage_v[self.config.scan.channel - 1]
                remaining_steps = run_steps[tracker.count :]
                eta = tracker.estimate_remaining(
                    remaining_global,
                    remaining_global * self.config.scan.settle_time_s,
                    _remaining_voltage_ramp_s(measured_voltage, remaining_steps),
                )
                self.log.info(
                    "Repeat acquisition cycle=%d direction=%s point=%d measured_v=%.6f temperature_c=%.3f osa_s=%.3f completed=%d/%d eta_s=%.3f",
                    cycle_index,
                    step.direction,
                    step.branch_index,
                    measured_voltage,
                    gain_status.temperature_c,
                    osa_duration_s,
                    tracker.count,
                    len(run_steps),
                    eta,
                )
            attempt.mark_acquisition_complete()
        except BaseException as error:
            try:
                attempt.mark_failed(error)
            except BaseException as mark_error:
                try:
                    self.log.error("Could not mark repeat cycle failed: %s", mark_error)
                except BaseException:
                    pass
            raise
        self.postprocessor.submit(PostprocessRequest(cycle_index, attempt.path))
        return RepeatCycleSummary(cycle_index, len(steps), attempt.path)

    def run(self) -> RepeatRunSummary:
        postprocessed_only = 0
        pending: tuple[int, ...] = ()
        skipped = 0
        cycle_summaries: list[RepeatCycleSummary] = []
        acquisitions = 0
        primary_error: BaseException | None = None
        try:
            postprocessed_only = self._submit_existing_missing_products()
            pending = self.store.pending_cycle_indices()
            skipped = self.config.scan.cycles - len(pending)
            if pending:
                self._connect_and_prepare()
                tracker = TimingTracker()
                run_steps = tuple(
                    step
                    for cycle_index in pending
                    for step in cycle_steps(self.store.schedule, cycle_index)
                )
                for cycle_index in pending:
                    summary = self._run_cycle(cycle_index, tracker, run_steps)
                    cycle_summaries.append(summary)
                    acquisitions += summary.acquisitions
                    try:
                        self.postprocessor.check()
                    except BaseException as error:
                        self.log.error("Postprocessing failed while acquisition continues: %s", error)
        except BaseException as error:
            primary_error = error
            report = getattr(error, "cleanup_report", None)
            if report is not None and (
                not self._cleanup_reports or self._cleanup_reports[-1] is not report
            ):
                self._cleanup_reports += (report,)
            raise
        finally:
            cleanup_error_to_raise: BaseException | None = None
            postprocess_error_to_raise: BaseException | None = None
            if self._session is not None and self._session_connected:
                cleanup_report = None
                try:
                    cleanup_report = self._session.close()
                except BaseException as cleanup_error:
                    cleanup_report = getattr(cleanup_error, "cleanup_report", None)
                    if primary_error is None:
                        cleanup_error_to_raise = cleanup_error
                    else:
                        try:
                            self.log.error("Cleanup failure suppressed to preserve primary error: %s", cleanup_error)
                        except BaseException:
                            pass
                if cleanup_report is not None:
                    if (
                        not self._cleanup_reports
                        or self._cleanup_reports[-1] is not cleanup_report
                    ):
                        self._cleanup_reports += (cleanup_report,)
                    if primary_error is not None:
                        try:
                            primary_error.cleanup_report = cleanup_report
                        except BaseException:
                            pass
                self._session_connected = False
            try:
                self.postprocessor.close_and_drain()
            except BaseException as postprocess_error:
                if primary_error is None and cleanup_error_to_raise is None:
                    postprocess_error_to_raise = postprocess_error
                else:
                    try:
                        self.log.error("Postprocessing failure suppressed to preserve primary error: %s", postprocess_error)
                    except BaseException:
                        pass
            if cleanup_error_to_raise is not None:
                raise cleanup_error_to_raise
            if postprocess_error_to_raise is not None:
                raise postprocess_error_to_raise
        if getattr(self.postprocessor, "validate_products", True):
            self._validate_final_products()
        if (
            self._session is not None
            and self._cleanup_reports
            and not self._cleanup_reports[-1].unreleased
        ):
            self._session = None
            self._osa = self._voltage = self._gain = None
        return RepeatRunSummary(
            run_path=self.store.path,
            acquisitions=acquisitions,
            completed_cycles=len(cycle_summaries),
            skipped_cycles=skipped,
            postprocessed_only_cycles=postprocessed_only,
            cycles=tuple(cycle_summaries),
        )
