"""Command-line entry point for safe SIL hysteresis experiments."""

from __future__ import annotations

import argparse
import json
import logging
import math
import sys
import time
from collections.abc import Callable, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, TextIO

from .config import RunConfig, load_config
from .experiment import ExperimentRunner, RunSummary
from .scan import build_scan_steps
from .storage import RunStore


DEFAULT_CONFIG = Path(__file__).with_name("default_config.json")
_SHUTDOWN_TEXT = (
    "voltage emergency zero; Gain current disable; Gain TEC disable; "
    "Gain close; Voltage Source close; OSA close"
)


@dataclass(frozen=True)
class DriverFactories:
    """Injectable constructors for the three public hardware drivers."""

    osa: Callable[..., Any]
    voltage: Callable[..., Any]
    gain: Callable[..., Any]

    def __post_init__(self) -> None:
        if not all(callable(factory) for factory in (self.osa, self.voltage, self.gain)):
            raise TypeError("all driver factories must be callable")


@dataclass(frozen=True)
class _InstrumentBundle:
    osa: Any
    voltage: Any
    gain: Any


def _default_driver_factories() -> DriverFactories:
    """Import real drivers only after the operator has authorized construction."""

    from Code.Utils import AQ6370, GainDriver, VoltageSource

    return DriverFactories(osa=AQ6370, voltage=VoltageSource, gain=GainDriver)


def make_real_bundle(config: RunConfig, factories: DriverFactories) -> _InstrumentBundle:
    """Construct unconnected public drivers using only their supported keywords."""

    if not isinstance(config, RunConfig):
        raise TypeError("config must be a RunConfig")
    if not isinstance(factories, DriverFactories):
        raise TypeError("factories must be DriverFactories")
    gain_kwargs: dict[str, Any] = {"port": config.devices.gain_port}
    if config.devices.gain_serial_number is not None:
        gain_kwargs["usb_serial"] = config.devices.gain_serial_number
    return _InstrumentBundle(
        osa=factories.osa(resource_name=config.devices.osa_resource),
        voltage=factories.voltage(port=config.devices.voltage_port),
        gain=factories.gain(**gain_kwargs),
    )


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run the SIL hysteresis experiment")
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--config", type=Path)
    source.add_argument("--resume", type=Path, metavar="RUN_DIRECTORY")
    parser.add_argument("--plan", action="store_true")
    coupling = parser.add_mutually_exclusive_group()
    coupling.add_argument(
        "--wait-for-coupling",
        action="store_true",
        help="hold stable TEC/current at zero phase voltage until exact COUPLED confirmation",
    )
    coupling.add_argument(
        "--wait-for-first-coupling",
        action="store_true",
        help="require exact COUPLED only at the first pending operation",
    )
    parser.add_argument("--no-display", action="store_true")
    parser.add_argument(
        "--log-level",
        choices=("DEBUG", "INFO", "WARNING", "ERROR"),
        default="INFO",
    )
    return parser


def _minimum_ramp_time(config: RunConfig) -> float:
    """Return the driver's unavoidable inter-step delay for the full schedule."""

    total_s = 0.0
    for _ in config.operation_points:
        previous = 0.0
        for step in build_scan_steps(config.scan):
            delta = abs(step.voltage_v - previous)
            ramp_steps = max(1, math.ceil(delta / 0.1))
            total_s += max(0, ramp_steps - 1) * 0.05
            previous = step.voltage_v
    return total_s


def _plan_lines(
    config: RunConfig,
    output_directory: Path,
    *,
    wait_for_coupling: bool = False,
    wait_for_first_coupling: bool = False,
) -> tuple[str, ...]:
    per_operation = len(build_scan_steps(config.scan))
    operations = len(config.operation_points)
    total_acquisitions = per_operation * operations
    total_settle_s = total_acquisitions * config.scan.settle_time_s
    total_current_ramp_s = sum(
        math.ceil(abs(point.current_ma - 3.0) / config.current_ramp_step_ma)
        * config.current_ramp_interval_s
        for point in config.operation_points
    )
    lines = ["Validated SIL hysteresis plan"]
    lines.extend(
        f"Operation {index + 1}/{operations}: temperature={point.temperature_c:.1f} degC, "
        f"current={point.current_ma:.1f} mA"
        for index, point in enumerate(config.operation_points)
    )
    lines.extend(
        (
            f"OSA: resource={config.devices.osa_resource}, trace={config.devices.osa_trace}",
            "Voltage Source port: " + str(config.devices.voltage_port),
            "Gain Driver port: " + str(config.devices.gain_port),
            "Gain USB serial: "
            + (
                config.devices.gain_serial_number
                if config.devices.gain_serial_number is not None
                else "driver default"
            ),
            f"Voltage scan: channel={config.scan.channel}, "
            f"range={config.scan.v_min:.1f}-{config.scan.v_max:.1f} V, "
            f"spacing={config.scan.spacing}, points={config.scan.points}",
            f"Acquisitions per operation: {per_operation}",
            f"Total acquisitions: {total_acquisitions}",
            f"Settle time per acquisition: {config.scan.settle_time_s:.1f} s",
            f"Total configured acquisition-settle wait: {total_settle_s:.3f} s",
            f"Minimum cumulative voltage-ramp time: {_minimum_ramp_time(config):.3f} s",
            f"Temperature stability timeout: {config.temperature_timeout_s:.1f} s per operation",
            f"LD settle wait per operation: {config.ld_settle_s:.1f} s",
            f"Total configured LD settle wait: {operations * config.ld_settle_s:.3f} s",
            f"Current ramp: max_step={config.current_ramp_step_ma:.3f} mA, "
            f"interval={config.current_ramp_interval_s:.3f} s",
            f"Total estimated current-ramp time from 3 mA: {total_current_ramp_s:.3f} s",
            "OSA acquisition time: unknown (rolling mean and ETA begin after acquisition 1)",
            "Coupling gate: "
            + (
                "first pending operation only after stable TEC/current with Voltage Source held at zero"
                if wait_for_first_coupling
                else "enabled after stable TEC/current with Voltage Source held at zero"
                if wait_for_coupling
                else "disabled"
            ),
            f"Output directory: {output_directory}",
            f"Shutdown: {_SHUTDOWN_TEXT}",
        )
    )
    return tuple(lines)


def _write_plan(
    config: RunConfig,
    output_directory: Path,
    output: TextIO,
    *,
    wait_for_coupling: bool = False,
    wait_for_first_coupling: bool = False,
) -> None:
    label = "REAL"
    for line in _plan_lines(
        config,
        output_directory,
        wait_for_coupling=wait_for_coupling,
        wait_for_first_coupling=wait_for_first_coupling,
    ):
        print(f"[{label}] {line}", file=output)
    output.flush()


class _RunLabelFilter(logging.Filter):
    def __init__(self, label: str) -> None:
        super().__init__()
        self.label = label

    def filter(self, record: logging.LogRecord) -> bool:
        record.run_label = self.label
        return True


class _UTCFormatter(logging.Formatter):
    converter = time.gmtime


@contextmanager
def _configured_logger(
    run_path: Path,
    output: TextIO,
    level_name: str,
):
    label = "REAL"
    logger = logging.getLogger(f"sil_hysteresis.run.{id(output)}.{time.monotonic_ns()}")
    logger.setLevel(logging.DEBUG)
    logger.propagate = False
    formatter = _UTCFormatter(
        "%(asctime)sZ %(levelname)s [%(run_label)s] %(message)s",
        datefmt="%Y-%m-%dT%H:%M:%S",
    )
    label_filter = _RunLabelFilter(label)
    stream_handler = logging.StreamHandler(output)
    file_handler = logging.FileHandler(run_path / "experiment.log", encoding="utf-8")
    stream_handler.setLevel(getattr(logging, level_name))
    file_handler.setLevel(logging.INFO)
    for handler in (stream_handler, file_handler):
        handler.addFilter(label_filter)
        handler.setFormatter(formatter)
        logger.addHandler(handler)
    try:
        yield logger
    finally:
        for handler in tuple(logger.handlers):
            logger.removeHandler(handler)
            handler.flush()
            handler.close()


def _summary_payload(attempt_path: Path) -> tuple[dict[str, Any], list[float]]:
    with (attempt_path / "summary.json").open("r", encoding="utf-8") as handle:
        payload = json.load(handle)
    metrics = payload.get("summary_metrics", {})
    discrepancies = [
        float(item["voltage_v"])
        for item in payload.get("discrepancy_ranking", ())
        if isinstance(item, dict) and "voltage_v" in item
    ]
    return metrics, discrepancies


def _log_run_summary(log: logging.Logger, summary: RunSummary) -> None:
    for operation in summary.operations:
        try:
            metrics, discrepancies = _summary_payload(operation.attempt_path)
        except (OSError, TypeError, ValueError, json.JSONDecodeError) as error:
            log.warning(
                "Operation artifact summary unavailable: op=%d attempt=%s error=%s",
                operation.operation_index,
                operation.attempt_path,
                error,
            )
            continue
        log.info(
            "Operation artifacts: op=%d attempt=%s figures=%s metrics=%s "
            "discrepancy_voltages=%s classification=%s postprocessed_only=%s",
            operation.operation_index,
            operation.attempt_path,
            ",".join(str(path) for path in operation.figure_paths),
            json.dumps(metrics, sort_keys=True, separators=(",", ":")),
            json.dumps(discrepancies, separators=(",", ":")),
            operation.classification,
            operation.postprocessed_only,
        )
    log.info(
        "Run complete: path=%s acquisitions=%d completed_operations=%d "
        "skipped=%d postprocessed=%d",
        summary.run_path,
        summary.acquisitions,
        summary.completed_operations,
        summary.skipped_operations,
        summary.postprocessed_operations,
    )


def main(
    argv: Sequence[str] | None = None,
    *,
    input_fn: Callable[[str], str] = input,
    factories: DriverFactories | None = None,
    output: TextIO | None = None,
) -> int:
    """Validate, authorize, and run one CLI invocation."""

    args = _build_parser().parse_args(argv)
    output = sys.stdout if output is None else output
    requested_mode = "real"

    if args.resume is not None:
        store = RunStore.open_existing(args.resume)
        if store.run_mode != requested_mode:
            raise ValueError(
                f"resume mode mismatch: stored={store.run_mode}, requested={requested_mode}"
            )
        config = store.config
        output_directory = store.path
    else:
        store = None
        config = load_config(args.config if args.config is not None else DEFAULT_CONFIG)
        output_directory = config.output_root
    if args.no_display:
        config = replace(config, display_latest=False)

    _write_plan(
        config,
        output_directory,
        output,
        wait_for_coupling=args.wait_for_coupling,
        wait_for_first_coupling=args.wait_for_first_coupling,
    )
    if args.plan:
        return 0

    answer = input_fn("Type exactly RUN to construct real instruments: ")
    if answer != "RUN":
        print("[REAL] Cancelled; no instrument factories were constructed.", file=output)
        output.flush()
        return 0

    if store is None:
        store = RunStore.create(config, run_mode=requested_mode)

    with _configured_logger(
        store.path,
        output,
        args.log_level,
    ) as log:
        log.info("Run starting: path=%s mode=real", store.path)
        selected_factories = factories if factories is not None else _default_driver_factories()
        first_coupling_confirmed = False

        def coupling_gate(index, point):
            nonlocal first_coupling_confirmed
            if args.wait_for_first_coupling and first_coupling_confirmed:
                return
            answer = input_fn(
                f"Operation {index + 1}: TEC={point.temperature_c:.1f} degC and "
                f"LD current={point.current_ma:.1f} mA are active with voltage at zero. "
                "Type exactly COUPLED to begin scanning; any other input shuts down: "
            )
            if answer != "COUPLED":
                raise RuntimeError("coupling gate was not confirmed")
            first_coupling_confirmed = True

        runner = ExperimentRunner(
            config,
            store,
            lambda: make_real_bundle(config, selected_factories),
            log=log,
            coupling_gate=(
                coupling_gate
                if args.wait_for_coupling or args.wait_for_first_coupling
                else None
            ),
        )
        summary = runner.run()
        _log_run_summary(log, summary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
