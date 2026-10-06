"""Fail-fast orchestration for SIL hysteresis experiments.

The runner depends only on injected instrument objects and the public experiment
storage/analysis interfaces.  It never constructs a transport or a hardware
driver directly.
"""

from __future__ import annotations

import logging
import math
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from Code.Setups.session import InstrumentSession, safe_shutdown
from Code.Utils.common import InstrumentSafetyError

from .analysis import ComparisonResult, compare_attempt, write_metrics_csv
from .config import OperationPoint, RunConfig
from .figure import (
    LatestResultWindow,
    OperationMetadata,
    build_comparison_figure,
    save_comparison_figure,
)
from .scan import ScanStep, TimingTracker
from .storage import CompletedAttemptData, RunStore, load_complete_attempt


LOGGER = logging.getLogger(__name__)
_PRODUCT_NAMES = (
    "metrics.csv",
    "summary.json",
    "comparison.svg",
    "comparison.pdf",
    "comparison.png",
)


def _best_effort_log_error(log: logging.Logger, message: str, *args: Any) -> None:
    """Report a secondary failure without letting logging affect safety flow."""

    try:
        log.error(message, *args)
    except BaseException:
        pass


def _remaining_voltage_ramp_s(
    current_voltage_v: float, remaining_steps: tuple[ScanStep, ...]
) -> float:
    """Conservatively include one 50 ms interval per required <=0.1 V step."""

    previous = float(current_voltage_v)
    total_steps = 0
    for step in remaining_steps:
        delta = abs(step.voltage_v - previous)
        total_steps += math.ceil(delta / 0.1)
        previous = step.voltage_v
    return total_steps * 0.05


@dataclass(frozen=True)
class OperationSummary:
    """Outcome of one newly acquired or postprocessed operation point."""

    operation_index: int
    point: OperationPoint
    acquisitions: int
    attempt_path: Path
    classification: str
    figure_paths: tuple[Path, ...]
    postprocessed_only: bool = False


@dataclass(frozen=True)
class RunSummary:
    """Counts and operation outcomes produced by one runner invocation."""

    run_path: Path
    acquisitions: int
    completed_operations: int
    skipped_operations: int
    postprocessed_operations: int
    operations: tuple[OperationSummary, ...]


class ExperimentRunner:
    """Run complete operation points with immediate persistence and shutdown."""

    def __init__(
        self,
        config: RunConfig,
        store: RunStore,
        instrument_bundle_factory: Callable[[], Any],
        *,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], None] = time.sleep,
        log: logging.Logger = LOGGER,
        latest_window: LatestResultWindow | None = None,
        analysis_fn: Callable[[CompletedAttemptData, Any], ComparisonResult] = compare_attempt,
        metrics_writer: Callable[[ComparisonResult, Path], None] = write_metrics_csv,
        figure_builder: Callable[[ComparisonResult, OperationMetadata], Any] = build_comparison_figure,
        figure_saver: Callable[[Any, Path], tuple[Path, ...]] = save_comparison_figure,
        coupling_gate: Callable[[int, OperationPoint], None] | None = None,
    ) -> None:
        if not isinstance(config, RunConfig):
            raise TypeError("config must be a RunConfig")
        if not callable(instrument_bundle_factory):
            raise TypeError("instrument_bundle_factory must be callable")
        self.config = config
        self.store = store
        self.instrument_bundle_factory = instrument_bundle_factory
        self.clock = clock
        self.sleep = sleep
        self.log = log
        self.latest_window = (
            latest_window
            if latest_window is not None
            else (LatestResultWindow() if config.display_latest else None)
        )
        self.analysis_fn = analysis_fn
        self.metrics_writer = metrics_writer
        self.figure_builder = figure_builder
        self.figure_saver = figure_saver
        if coupling_gate is not None and not callable(coupling_gate):
            raise TypeError("coupling_gate must be callable or None")
        self.coupling_gate = coupling_gate
        self._osa: Any = None
        self._voltage: Any = None
        self._gain: Any = None
        self._session: InstrumentSession | None = None
        self._cleanup_reports: tuple[Any, ...] = ()

    def run(self) -> RunSummary:
        """Postprocess complete data, then acquire all still-pending operations."""

        operation_summaries: list[OperationSummary] = []
        skipped_operations = 0
        postprocessed_operations = 0
        initially_complete: set[int] = set()

        for index, point in enumerate(self.config.operation_points):
            if not self.store.operation_complete(index):
                continue
            initially_complete.add(index)
            skipped_operations += 1
            data = self._find_complete_attempt(index)
            if not self._products_complete(data.path):
                operation_summaries.append(
                    self._postprocess(data, index, point, acquisitions=0, postprocessed_only=True)
                )
                postprocessed_operations += 1

        pending = tuple(
            index
            for index in self.store.pending_operation_indices()
            if index not in initially_complete
        )
        if not pending:
            return RunSummary(
                run_path=self.store.path,
                acquisitions=0,
                completed_operations=0,
                skipped_operations=skipped_operations,
                postprocessed_operations=postprocessed_operations,
                operations=tuple(operation_summaries),
            )

        primary_error: BaseException | None = None
        session_connected = False
        acquisitions = 0
        completed_operations = 0
        bundle: Any = None
        try:
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

            # Connection order is safety-significant: OSA identity, voltage
            # startup zero, then the Gain read-only status snapshot.
            self._session = InstrumentSession(
                osa=self._osa,
                voltage=self._voltage,
                gain=self._gain,
                log=self.log,
            )
            self._session.connect()
            session_connected = True
            self._session.check_health()
            self._gain.read_status()

            # Normalize the connected baseline before any setpoint change.
            self._session.check_health()
            self._voltage.zero(emergency=True)
            self._session.check_health()
            self._gain.disable_current()

            for index in pending:
                point = self.config.operation_points[index]
                operation = self.run_operation(index, point)
                operation_summaries.append(operation)
                acquisitions += operation.acquisitions
                completed_operations += 1
        except BaseException as error:
            primary_error = error
            report = getattr(error, "cleanup_report", None)
            if report is not None and (
                not self._cleanup_reports or self._cleanup_reports[-1] is not report
            ):
                self._cleanup_reports += (report,)
            raise
        finally:
            if self._session is not None and session_connected:
                cleanup_report = None
                cleanup_error = None
                try:
                    cleanup_report = self._session.close()
                except BaseException as error:
                    cleanup_error = error
                    cleanup_report = getattr(error, "cleanup_report", None)
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
                if cleanup_error is not None:
                    if primary_error is None:
                        raise cleanup_error
                    _best_effort_log_error(
                        self.log,
                        "Cleanup failure suppressed to preserve primary error: error=%s",
                        cleanup_error,
                    )

        if (
            self._session is not None
            and self._cleanup_reports
            and not self._cleanup_reports[-1].unreleased
        ):
            self._session = None
            self._osa = self._voltage = self._gain = None

        return RunSummary(
            run_path=self.store.path,
            acquisitions=acquisitions,
            completed_operations=completed_operations,
            skipped_operations=skipped_operations,
            postprocessed_operations=postprocessed_operations,
            operations=tuple(operation_summaries),
        )

    def run_operation(self, index: int, point: OperationPoint) -> OperationSummary:
        """Acquire one complete triangular scan and postprocess only after safe-off."""

        if self._osa is None or self._voltage is None or self._gain is None:
            raise RuntimeError("instruments are not connected")
        if self._session is None:
            raise RuntimeError("instrument session is not available")
        if not isinstance(index, int) or isinstance(index, bool):
            raise ValueError("operation index must be an integer")
        if not 0 <= index < len(self.config.operation_points):
            raise ValueError("operation index is outside the configured operation points")
        if point != self.config.operation_points[index]:
            raise ValueError("operation point does not match the configured index")

        attempt = self.store.start_attempt(index)
        tracker = TimingTracker()
        schedule = self.store.schedule
        try:
            # Every attempt, including a resumed incomplete operation, begins
            # from an explicit all-channel zero with current disabled.
            self._session.check_health()
            self._voltage.zero(emergency=True)
            self._session.check_health()
            self._gain.disable_current()
            self._session.check_health()
            self._gain.set_temperature(point.temperature_c)
            gain_status = self._gain.read_status()
            if not gain_status.tec_enabled:
                self._session.check_health()
                self._gain.enable_tec()
            self._gain.wait_stable(self.config.temperature_timeout_s)

            # The hardware resets its setpoint to 3 mA when output is enabled;
            # confirm that state first, then ramp the live output to the target.
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
            if self.coupling_gate is not None:
                # Reassert the optical phase-shifter baseline immediately
                # before yielding control for manual coupling.  The same
                # instrument session and Gain outputs remain active.
                self._session.check_health()
                self._voltage.zero(emergency=True)
                self.coupling_gate(index, point)

            for step in schedule:
                self.log.info(
                    "Voltage scan command op=%d direction=%s point=%d sequence=%d "
                    "channel=%d requested_v=%.6f",
                    index,
                    step.direction,
                    step.direction_index,
                    step.sequence_index,
                    self.config.scan.channel,
                    step.voltage_v,
                )
                self._session.check_health()
                self._voltage.set_channel(self.config.scan.channel, step.voltage_v)
                # Do not acquire from an arbitrary fresh frame: telemetry must
                # confirm that this scan channel reached the commanded target.
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
                remaining = len(schedule) - tracker.count
                measured_voltage = voltage_status.voltage_v[self.config.scan.channel - 1]
                eta = tracker.estimate_remaining(
                    remaining,
                    remaining * self.config.scan.settle_time_s,
                    _remaining_voltage_ramp_s(
                        measured_voltage,
                        schedule[tracker.count :],
                    ),
                )
                self.log.info(
                    "Acquisition op=%d direction=%s point=%d requested_v=%.6f "
                    "measured_v=%.6f temperature_c=%.3f osa_s=%.3f completed=%d/%d "
                    "mean_osa_s=%.3f eta_s=%.3f",
                    index,
                    step.direction,
                    step.direction_index,
                    step.voltage_v,
                    measured_voltage,
                    gain_status.temperature_c,
                    osa_duration_s,
                    tracker.count,
                    len(schedule),
                    tracker.mean_s,
                    eta,
                )

            self._session.check_health()
            self._voltage.zero(emergency=True)
            self._session.check_health()
            self._gain.disable_current()
            attempt.mark_complete()
        except BaseException as error:
            try:
                attempt.mark_failed(error)
            except BaseException as mark_error:
                _best_effort_log_error(
                    self.log,
                    "Could not mark failed attempt without replacing primary error: error=%s",
                    mark_error,
                )
            raise

        data = load_complete_attempt(attempt.path)
        return self._postprocess(
            data,
            index,
            point,
            acquisitions=len(schedule),
            postprocessed_only=False,
        )

    def _find_complete_attempt(self, operation_index: int) -> CompletedAttemptData:
        candidates = sorted(
            self.store.path.glob(f"op_{operation_index:03d}_*/attempt_*")
        )
        for path in candidates:
            try:
                return load_complete_attempt(path)
            except (OSError, TypeError, ValueError):
                continue
        raise ValueError(f"operation {operation_index} has no valid complete attempt")

    @staticmethod
    def _products_complete(attempt_path: Path) -> bool:
        return all((attempt_path / name).is_file() for name in _PRODUCT_NAMES)

    def _postprocess(
        self,
        data: CompletedAttemptData,
        index: int,
        point: OperationPoint,
        *,
        acquisitions: int,
        postprocessed_only: bool,
    ) -> OperationSummary:
        result = self.analysis_fn(data, self.config.analysis)
        self.metrics_writer(result, data.path / "metrics.csv")
        metadata = OperationMetadata(
            point.temperature_c,
            point.current_ma,
            index,
            getattr(self.store, "run_mode", "real"),
        )
        figure = self.figure_builder(result, metadata)
        figure_paths = tuple(self.figure_saver(figure, data.path / "comparison"))
        if self.latest_window is not None:
            self.latest_window.update(result, metadata)
        self.log.info(
            "Operation complete: op=%d classification=%s figures=%s",
            index,
            result.classification,
            ",".join(str(path) for path in figure_paths),
        )
        return OperationSummary(
            operation_index=index,
            point=point,
            acquisitions=acquisitions,
            attempt_path=data.path,
            classification=result.classification,
            figure_paths=figure_paths,
            postprocessed_only=postprocessed_only,
        )
