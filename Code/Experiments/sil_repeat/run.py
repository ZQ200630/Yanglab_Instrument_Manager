"""Command-line entry point for safe repeated SIL cycle scans."""

from __future__ import annotations

import argparse
import logging
import math
import sys
import time
from collections.abc import Callable, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, TextIO

from .config import RepeatRunConfig, load_repeat_config
from .experiment import RepeatExperimentRunner, RepeatRunSummary
from .postprocess import ProcessPostprocessor
from .scan import build_repeat_schedule
from .storage import RepeatRunStore


DEFAULT_CONFIG = Path(__file__).with_name("repeat_24C_100mA.json")
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
REAL_OUTPUT_ROOT = (REPOSITORY_ROOT / "Result" / "sil_repeat").resolve()
_SHUTDOWN_TEXT = (
    "voltage emergency zero; Gain current disable; Gain TEC disable; "
    "Gain close; Voltage Source close; OSA close"
)


@dataclass(frozen=True)
class DriverFactories:
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
    from Code.Utils import AQ6370, GainDriver, VoltageSource

    return DriverFactories(osa=AQ6370, voltage=VoltageSource, gain=GainDriver)


def make_real_bundle(config: RepeatRunConfig, factories: DriverFactories) -> _InstrumentBundle:
    if not isinstance(config, RepeatRunConfig):
        raise TypeError("config must be a RepeatRunConfig")
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
    parser = argparse.ArgumentParser(description="Run repeated SIL forward/reverse cycles")
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--config", type=Path)
    source.add_argument("--resume", type=Path, metavar="RUN_DIRECTORY")
    parser.add_argument("--plan", action="store_true")
    parser.add_argument(
        "--wait-for-coupling",
        action="store_true",
        help="compatibility flag; real runs always require exact COUPLED",
    )
    parser.add_argument("--no-display", action="store_true")
    parser.add_argument(
        "--log-level",
        choices=("DEBUG", "INFO", "WARNING", "ERROR"),
        default="INFO",
    )
    return parser


def _minimum_ramp_time(config: RepeatRunConfig) -> float:
    previous = 0.0
    total = 0.0
    for step in build_repeat_schedule(config.scan):
        ramp_steps = math.ceil(abs(step.voltage_v - previous) / 0.1)
        total += max(0, ramp_steps - 1) * 0.05
        previous = step.voltage_v
    return total


def _validated_real_output_root(path: Path) -> Path:
    candidate = Path(path)
    if not candidate.is_absolute():
        candidate = REPOSITORY_ROOT / candidate
    candidate = candidate.resolve()
    allowed = Path(REAL_OUTPUT_ROOT).resolve()
    if candidate != allowed and allowed not in candidate.parents:
        raise ValueError(f"real output_root must be within {allowed}")
    return candidate


def _plan_lines(
    config: RepeatRunConfig,
    output_directory: Path,
    *,
    wait_for_coupling: bool,
) -> tuple[str, ...]:
    acquisitions = len(build_repeat_schedule(config.scan))
    settle_s = acquisitions * config.scan.settle_time_s
    current_ramp_s = (
        math.ceil(abs(config.operation_point.current_ma - 3.0) / config.current_ramp_step_ma)
        * config.current_ramp_interval_s
    )
    ramp_s = _minimum_ramp_time(config)
    observed_estimate_h = (
        acquisitions * 1.46 + settle_s + ramp_s + current_ramp_s + config.ld_settle_s
    ) / 3600.0
    return (
        "Validated repeated SIL cycle plan",
        f"Operating point: temperature={config.operation_point.temperature_c:.1f} degC, current={config.operation_point.current_ma:.1f} mA",
        f"OSA: resource={config.devices.osa_resource}, trace={config.devices.osa_trace}",
        f"Voltage Source port: {config.devices.voltage_port}",
        f"Gain Driver port: {config.devices.gain_port}",
        "Gain USB serial: " + (config.devices.gain_serial_number or "driver default"),
        f"Voltage scan: channel={config.scan.channel}, V^2 range={config.scan.v2_min:.1f}-{config.scan.v2_max:.1f}, voltage={math.sqrt(config.scan.v2_min):.4f}-{math.sqrt(config.scan.v2_max):.4f} V",
        f"Points per branch: {config.scan.points_per_branch}",
        f"Cycles: {config.scan.cycles}",
        f"Acquisitions per cycle: {2 * config.scan.points_per_branch}",
        f"Total acquisitions: {acquisitions}",
        f"Primary comparable voltage pairs per cycle: {config.scan.points_per_branch - 1}",
        f"Settle time per acquisition: {config.scan.settle_time_s:.1f} s",
        f"Total configured acquisition-settle wait: {settle_s:.1f} s",
        f"Minimum cumulative voltage-ramp time: {ramp_s:.1f} s",
        f"Temperature stability timeout: {config.temperature_timeout_s:.1f} s",
        f"LD settle wait: {config.ld_settle_s:.1f} s",
        f"Current ramp: max_step={config.current_ramp_step_ma:.3f} mA, interval={config.current_ramp_interval_s:.3f} s",
        f"Estimated current ramp from 3 mA: {current_ramp_s:.1f} s",
        f"Observed-duration estimate at 1.46 s/OSA sweep: {observed_estimate_h:.2f} h",
        "Capacity warning: 8000 spectra may consume several hundred megabytes" if acquisitions == 8000 else "Capacity depends on OSA wavelength-bin count",
        "Coupling gate: enabled after stable TEC/current with Voltage Source held at zero",
        f"Output directory: {output_directory}",
        f"Shutdown: {_SHUTDOWN_TEXT}",
    )


def _write_plan(
    config: RepeatRunConfig,
    output_directory: Path,
    output: TextIO,
    *,
    wait_for_coupling: bool,
) -> None:
    label = "REAL"
    for line in _plan_lines(config, output_directory, wait_for_coupling=wait_for_coupling):
        print(f"[{label}] {line}", file=output)
    output.flush()


class _UTCFormatter(logging.Formatter):
    converter = time.gmtime


@contextmanager
def _configured_logger(run_path: Path, output: TextIO, level_name: str):
    label = "REAL"
    logger = logging.getLogger(f"sil_repeat.run.{id(output)}.{time.monotonic_ns()}")
    logger.setLevel(logging.DEBUG)
    logger.propagate = False
    formatter = _UTCFormatter(
        f"%(asctime)sZ %(levelname)s [{label}] %(message)s",
        datefmt="%Y-%m-%dT%H:%M:%S",
    )
    stream = logging.StreamHandler(output)
    file_handler = logging.FileHandler(run_path / "experiment.log", encoding="utf-8")
    stream.setLevel(getattr(logging, level_name))
    file_handler.setLevel(logging.INFO)
    for handler in (stream, file_handler):
        handler.setFormatter(formatter)
        logger.addHandler(handler)
    try:
        yield logger
    finally:
        for handler in tuple(logger.handlers):
            logger.removeHandler(handler)
            handler.flush()
            handler.close()


def _log_summary(log: logging.Logger, summary: RepeatRunSummary) -> None:
    log.info(
        "Repeat run complete: path=%s acquisitions=%d completed_cycles=%d skipped=%d postprocessed_only=%d",
        summary.run_path,
        summary.acquisitions,
        summary.completed_cycles,
        summary.skipped_cycles,
        summary.postprocessed_only_cycles,
    )


def main(
    argv: Sequence[str] | None = None,
    *,
    input_fn: Callable[[str], str] = input,
    factories: DriverFactories | None = None,
    output: TextIO | None = None,
) -> int:
    args = _build_parser().parse_args(argv)
    output = sys.stdout if output is None else output
    requested_mode = "real"

    if args.resume is not None:
        _validated_real_output_root(args.resume)
        store = RepeatRunStore.open_existing(args.resume)
        if store.run_mode != requested_mode:
            raise ValueError(
                f"resume mode mismatch: stored={store.run_mode}, requested={requested_mode}"
            )
        config = store.config
        output_directory = store.path
    else:
        store = None
        config = load_repeat_config(args.config if args.config is not None else DEFAULT_CONFIG)
        config = replace(
            config,
            output_root=_validated_real_output_root(config.output_root),
        )
        output_directory = config.output_root
    if args.no_display:
        config = replace(config, display_latest=False)

    _write_plan(
        config,
        output_directory,
        output,
        wait_for_coupling=args.wait_for_coupling,
    )
    if args.plan:
        return 0

    answer = input_fn("Type exactly RUN to construct real instruments: ")
    if answer != "RUN":
        print("[REAL] Cancelled; no instrument factories were constructed.", file=output)
        output.flush()
        return 0

    if store is None:
        store = RepeatRunStore.create(config, run_mode=requested_mode)

    with _configured_logger(store.path, output, args.log_level) as log:
        log.info("Repeat run starting: path=%s mode=%s", store.path, requested_mode)
        postprocessor = ProcessPostprocessor(
            config,
            store.path,
            display_latest=config.display_latest,
        )
        selected = factories if factories is not None else _default_driver_factories()
        bundle_factory = lambda: make_real_bundle(config, selected)

        def coupling_gate(point):
            cycle_word = "cycle" if config.scan.cycles == 1 else "cycles"
            answer = input_fn(
                f"TEC={point.temperature_c:.1f} degC and LD current={point.current_ma:.1f} mA are active with voltage at zero. "
                f"Type exactly COUPLED to begin {config.scan.cycles} {cycle_word}; any other input shuts down: "
            )
            if answer != "COUPLED":
                raise RuntimeError("coupling gate was not confirmed")

        runner = RepeatExperimentRunner(
            config,
            store,
            bundle_factory,
            postprocessor=postprocessor,
            coupling_gate=coupling_gate,
            sleep=time.sleep,
            log=log,
        )
        summary = runner.run()
        _log_summary(log, summary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
