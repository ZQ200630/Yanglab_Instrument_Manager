"""Out-of-process analysis so figure export never delays hardware cadence."""

from __future__ import annotations

import csv
import json
import multiprocessing
import queue
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from Code.Experiments.sil_hysteresis.figure import LatestResultWindow, OperationMetadata

from .analysis import (
    CycleSummaryRow,
    save_repeat_summary_figure,
    write_cycle_products,
    write_repeat_summary,
)
from .config import RepeatRunConfig
from .storage import CycleAttemptStore, RepeatRunStore, load_complete_cycle


@dataclass(frozen=True)
class PostprocessRequest:
    cycle_index: int
    attempt_path: Path
    reset_aggregate: bool = False

    def __post_init__(self) -> None:
        if isinstance(self.cycle_index, bool) or not isinstance(self.cycle_index, int) or self.cycle_index < 0:
            raise ValueError("cycle_index must be a non-negative integer")
        if not isinstance(self.reset_aggregate, bool):
            raise ValueError("reset_aggregate must be boolean")
        object.__setattr__(self, "attempt_path", Path(self.attempt_path))


@dataclass(frozen=True)
class _PostprocessResult:
    cycle_index: int
    error_type: str | None
    error: str | None


def _failure_message(failures: list[_PostprocessResult]) -> str:
    return "postprocessing failures: " + "; ".join(
        f"cycle {failure.cycle_index}: {failure.error_type}: {failure.error}"
        for failure in failures
    )


def _read_summary_rows(path: Path) -> dict[int, CycleSummaryRow]:
    if not path.is_file():
        return {}
    rows: dict[int, CycleSummaryRow] = {}
    with path.open(newline="", encoding="utf-8") as handle:
        for raw in csv.DictReader(handle):
            row = CycleSummaryRow(
                cycle_index=int(raw["cycle_index"]),
                p95_signal_mae_db=float(raw["p95_signal_mae_db"]),
                maximum_absolute_peak_shift_nm=float(raw["maximum_absolute_peak_shift_nm"]),
                largest_discrepancy_v2=float(raw["largest_discrepancy_v2"]),
                mean_osa_duration_s=float(raw["mean_osa_duration_s"]),
                started_at=raw["started_at"],
                completed_at=raw["completed_at"],
                classification=raw["classification"],
                run_mode=raw["run_mode"],
            )
            rows[row.cycle_index] = row
    return rows


def _mark_postprocessed(attempt_path: Path) -> None:
    status = json.loads((attempt_path / "status.json").read_text(encoding="utf-8"))
    run = RepeatRunStore.open_existing(attempt_path.parents[1])
    attempt = CycleAttemptStore(
        run,
        attempt_path,
        int(status["cycle_index"]),
        int(status["attempt_number"]),
    )
    if status.get("state") == "acquired":
        attempt.mark_postprocessed()


def _worker_main(
    config: RepeatRunConfig,
    run_path: Path,
    request_queue: Any,
    result_queue: Any,
    display_latest: bool,
) -> None:
    rows = _read_summary_rows(Path(run_path) / "repeat_summary.csv")
    latest_window = LatestResultWindow() if display_latest else None
    while True:
        request = request_queue.get()
        if request is None:
            return
        try:
            if request.reset_aggregate:
                rows.clear()
            data = load_complete_cycle(request.attempt_path)
            if data.cycle_index != request.cycle_index:
                raise ValueError("postprocess request cycle does not match stored cycle")
            products = write_cycle_products(data, config)
            rows[request.cycle_index] = products.summary_row
            ordered = tuple(rows[index] for index in sorted(rows))
            write_repeat_summary(run_path, ordered)
            save_repeat_summary_figure(ordered, Path(run_path) / "repeat_summary")
            _mark_postprocessed(request.attempt_path)
            if latest_window is not None:
                latest_window.update(
                    products.result,
                    OperationMetadata(
                        config.operation_point.temperature_c,
                        config.operation_point.current_ma,
                        request.cycle_index,
                        data.run_mode,
                    ),
                )
            result_queue.put(_PostprocessResult(request.cycle_index, None, None))
        except BaseException as error:
            result_queue.put(
                _PostprocessResult(request.cycle_index, type(error).__name__, str(error))
            )


class InlinePostprocessor:
    """Synchronous implementation used by deterministic orchestration tests."""

    def __init__(
        self,
        handler: Callable[[PostprocessRequest], Any],
        *,
        validate_products: bool = False,
    ) -> None:
        if not callable(handler):
            raise TypeError("handler must be callable")
        if not isinstance(validate_products, bool):
            raise TypeError("validate_products must be boolean")
        self._handler = handler
        self.validate_products = validate_products
        self._failures: list[_PostprocessResult] = []
        self._reported_failures = 0
        self._closed = False

    def submit(self, request: PostprocessRequest) -> None:
        if self._closed:
            raise RuntimeError("postprocessor is closed")
        try:
            self._handler(request)
        except BaseException as error:
            self._failures.append(
                _PostprocessResult(request.cycle_index, type(error).__name__, str(error))
            )

    def check(self) -> None:
        if self._reported_failures < len(self._failures):
            failure = self._failures[self._reported_failures]
            self._reported_failures += 1
            raise RuntimeError(
                f"postprocessing cycle {failure.cycle_index} failed: "
                f"{failure.error_type}: {failure.error}"
            )

    def close_and_drain(self) -> None:
        self._closed = True
        if self._failures:
            raise RuntimeError(_failure_message(self._failures))


class ProcessPostprocessor:
    validate_products = True

    def __init__(
        self,
        config: RepeatRunConfig,
        run_path: Path,
        *,
        display_latest: bool,
        join_timeout_s: float = 300.0,
    ) -> None:
        if not isinstance(config, RepeatRunConfig):
            raise TypeError("config must be a RepeatRunConfig")
        if isinstance(join_timeout_s, bool) or not isinstance(join_timeout_s, (int, float)) or join_timeout_s <= 0:
            raise ValueError("join_timeout_s must be positive")
        context = multiprocessing.get_context("spawn")
        self._requests = context.Queue()
        self._results = context.Queue()
        self._process = context.Process(
            target=_worker_main,
            args=(config, Path(run_path), self._requests, self._results, bool(display_latest)),
            name="sil-repeat-postprocess",
        )
        self._process.start()
        self._join_timeout_s = float(join_timeout_s)
        self._submitted: set[int] = set()
        self._completed: set[int] = set()
        self._failures: list[_PostprocessResult] = []
        self._reported_failures = 0
        self._closed = False

    @property
    def process_exitcode(self) -> int | None:
        return self._process.exitcode

    def submit(self, request: PostprocessRequest) -> None:
        if self._closed:
            raise RuntimeError("postprocessor is closed")
        if request.cycle_index in self._submitted:
            raise ValueError("cycle has already been submitted")
        self._submitted.add(request.cycle_index)
        self._requests.put(request)

    def _collect(self) -> None:
        while True:
            try:
                result = self._results.get_nowait()
            except queue.Empty:
                return
            if result.error is None:
                self._completed.add(result.cycle_index)
            else:
                self._failures.append(result)

    def check(self) -> None:
        self._collect()
        if self._reported_failures < len(self._failures):
            failure = self._failures[self._reported_failures]
            self._reported_failures += 1
            raise RuntimeError(
                f"postprocessing cycle {failure.cycle_index} failed: "
                f"{failure.error_type}: {failure.error}"
            )
        if self._process.exitcode not in {None, 0}:
            raise RuntimeError(f"postprocessor exited with code {self._process.exitcode}")

    def close_and_drain(self) -> None:
        if self._closed:
            self.check()
            return
        self._closed = True
        self._requests.put(None)
        self._process.join(self._join_timeout_s)
        if self._process.is_alive():
            self._process.terminate()
            self._process.join(5.0)
            raise RuntimeError("postprocessor did not drain before timeout")
        self._collect()
        if self._failures:
            raise RuntimeError(_failure_message(self._failures))
        if self._process.exitcode not in {None, 0}:
            raise RuntimeError(f"postprocessor exited with code {self._process.exitcode}")
        missing = self._submitted - self._completed
        if missing:
            raise RuntimeError(
                "postprocessor returned no result for cycles "
                + ",".join(str(index) for index in sorted(missing))
            )
