"""Cycle-scoped atomic persistence for long repeated SIL runs."""

from __future__ import annotations

import csv
import json
import math
import re
import tempfile
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import numpy as np

from .config import RepeatRunConfig
from .scan import RepeatStep, build_repeat_schedule, cycle_steps


_ATTEMPT_RE = re.compile(r"attempt_(\d+)")
_PRODUCT_NAMES = ("metrics.csv", "summary.json", "comparison.svg", "comparison.pdf", "comparison.png")


def _timestamp() -> str:
    return datetime.now(timezone.utc).isoformat()


def _validate_run_mode(value: str) -> str:
    if value not in {"real", "similar", "hysteretic"}:
        raise ValueError("run_mode must be real, similar, or hysteretic")
    return value


def _require_real_acquisition(run_mode: str) -> None:
    if run_mode != "real":
        raise ValueError("Historical non-real runs are read-only; acquisition requires real provenance")


def _provenance(run_mode: str) -> dict[str, Any]:
    simulated = run_mode != "real"
    return {
        "label": "SIMULATED" if simulated else "REAL",
        "run_mode": run_mode,
        "simulated": simulated,
    }


def _write_json_atomic(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    try:
        with temporary.open("w", encoding="utf-8", newline="\n") as handle:
            json.dump(payload, handle, indent=2, sort_keys=True)
            handle.write("\n")
        temporary.replace(path)
    finally:
        if temporary.exists():
            temporary.unlink()


def _write_npz_atomic(path: Path, **payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    try:
        with temporary.open("wb") as handle:
            np.savez_compressed(handle, **payload)
        temporary.replace(path)
    finally:
        if temporary.exists():
            temporary.unlink()


def _write_csv_atomic(path: Path, records: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    try:
        if records:
            with temporary.open("w", newline="", encoding="utf-8") as handle:
                writer = csv.DictWriter(handle, fieldnames=tuple(records[0]))
                writer.writeheader()
                writer.writerows(records)
        else:
            temporary.write_text("", encoding="utf-8")
        temporary.replace(path)
    finally:
        if temporary.exists():
            temporary.unlink()


def _config_payload(config: RepeatRunConfig) -> dict[str, Any]:
    return {
        "devices": {
            "osa_resource": config.devices.osa_resource,
            "osa_trace": config.devices.osa_trace,
            "voltage_port": config.devices.voltage_port,
            "gain_port": config.devices.gain_port,
            "gain_serial_number": config.devices.gain_serial_number,
        },
        "scan": {
            "channel": config.scan.channel,
            "v2_min": config.scan.v2_min,
            "v2_max": config.scan.v2_max,
            "points_per_branch": config.scan.points_per_branch,
            "cycles": config.scan.cycles,
            "settle_time_s": config.scan.settle_time_s,
        },
        "gain": {
            "temperature_c": config.operation_point.temperature_c,
            "current_ma": config.operation_point.current_ma,
        },
        "temperature_timeout_s": config.temperature_timeout_s,
        "ld_settle_s": config.ld_settle_s,
        "current_ramp_step_ma": config.current_ramp_step_ma,
        "current_ramp_interval_s": config.current_ramp_interval_s,
        "analysis": {
            "signal_window_db": config.analysis.signal_window_db,
            "display_floor_dbm": config.analysis.display_floor_dbm,
            "thresholds": dict(config.analysis.thresholds) if config.analysis.thresholds else None,
        },
        "output_root": str(config.output_root),
        "display_latest": config.display_latest,
    }


def _step_payload(step: RepeatStep) -> dict[str, Any]:
    return {
        "sequence_index": step.sequence_index,
        "cycle_index": step.cycle_index,
        "direction": step.direction,
        "branch_index": step.branch_index,
        "pair_index": step.pair_index,
        "voltage_squared_v2": step.voltage_squared_v2,
        "voltage_v": step.voltage_v,
        "requested_voltage_v": step.voltage_v,
    }


def _relative_path(step: RepeatStep) -> Path:
    return Path(step.direction) / f"{step.branch_index:03d}_V2_{step.voltage_squared_v2:.6f}.npz"


@dataclass(frozen=True)
class RepeatStoredAcquisition:
    path: Path
    step: RepeatStep
    acquired_at: str
    osa_duration_s: float
    trace: str
    identity: str


@dataclass(frozen=True)
class CycleData:
    path: Path
    cycle_index: int
    attempt_number: int
    acquisitions: tuple[RepeatStoredAcquisition, ...]
    run_mode: str
    state: str


class RepeatRunStore:
    def __init__(self, path: Path, config: RepeatRunConfig, run_mode: str) -> None:
        self.path = Path(path)
        self.config = config
        self.run_mode = _validate_run_mode(run_mode)
        self.schedule = build_repeat_schedule(config.scan)

    @classmethod
    def create(cls, config: RepeatRunConfig, run_mode: str = "real") -> "RepeatRunStore":
        if not isinstance(config, RepeatRunConfig):
            raise TypeError("config must be a RepeatRunConfig")
        run_mode = _validate_run_mode(run_mode)
        _require_real_acquisition(run_mode)
        config.output_root.mkdir(parents=True, exist_ok=True)
        path = config.output_root / f"run_{datetime.now(timezone.utc):%Y%m%dT%H%M%S%fZ}_{uuid.uuid4().hex[:8]}"
        path.mkdir()
        _write_json_atomic(
            path / "run.json",
            {
                "version": 1,
                "created_at": _timestamp(),
                "run_mode": run_mode,
                "provenance": _provenance(run_mode),
                "config": _config_payload(config),
                "schedule_length": len(build_repeat_schedule(config.scan)),
            },
        )
        run = cls(path, config, run_mode)
        run._refresh_cycles_csv()
        return run

    @classmethod
    def open_existing(cls, path: Path) -> "RepeatRunStore":
        path = Path(path)
        try:
            manifest = json.loads((path / "run.json").read_text(encoding="utf-8"))
            run_mode = _validate_run_mode(manifest["run_mode"])
            config = RepeatRunConfig.from_mapping(manifest["config"])
        except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
            raise ValueError(f"invalid repeat run manifest: {path}") from error
        run = cls(path, config, run_mode)
        if manifest.get("provenance") != _provenance(run_mode):
            raise ValueError("run manifest provenance does not match run_mode")
        if manifest.get("schedule_length") != len(run.schedule):
            raise ValueError("run manifest schedule length does not match configuration")
        return run

    def _cycle_path(self, cycle_index: int) -> Path:
        self._validate_cycle_index(cycle_index)
        return self.path / f"cycle_{cycle_index:03d}"

    def _validate_cycle_index(self, cycle_index: int) -> None:
        if (
            isinstance(cycle_index, bool)
            or not isinstance(cycle_index, int)
            or not 0 <= cycle_index < self.config.scan.cycles
        ):
            raise ValueError("cycle_index is outside the configured cycles")

    def _attempt_directories(self, cycle_index: int) -> tuple[tuple[int, Path], ...]:
        cycle_path = self._cycle_path(cycle_index)
        if not cycle_path.exists():
            return ()
        found: list[tuple[int, Path]] = []
        for child in cycle_path.iterdir():
            match = _ATTEMPT_RE.fullmatch(child.name)
            if child.is_dir() and match:
                found.append((int(match.group(1)), child))
        return tuple(sorted(found))

    def _valid_cycle_data(self, cycle_index: int) -> CycleData | None:
        for attempt_number, path in reversed(self._attempt_directories(cycle_index)):
            try:
                return CycleAttemptStore(self, path, cycle_index, attempt_number).completed_data()
            except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError):
                continue
        return None

    def pending_cycle_indices(self) -> tuple[int, ...]:
        return tuple(
            index
            for index in range(self.config.scan.cycles)
            if self._valid_cycle_data(index) is None
        )

    def completed_cycle_data(self, cycle_index: int) -> CycleData:
        self._validate_cycle_index(cycle_index)
        data = self._valid_cycle_data(cycle_index)
        if data is None:
            raise ValueError("cycle acquisition is not complete")
        return data

    def start_cycle_attempt(self, cycle_index: int) -> "CycleAttemptStore":
        _require_real_acquisition(self.run_mode)
        self._validate_cycle_index(cycle_index)
        if self._valid_cycle_data(cycle_index) is not None:
            raise ValueError("cycle acquisition is already complete")
        attempts = self._attempt_directories(cycle_index)
        number = (attempts[-1][0] if attempts else 0) + 1
        path = self._cycle_path(cycle_index) / f"attempt_{number:03d}"
        path.mkdir(parents=True, exist_ok=False)
        _write_json_atomic(
            path / "status.json",
            {
                "state": "running",
                "cycle_index": cycle_index,
                "attempt_number": number,
                "run_mode": self.run_mode,
                "provenance": _provenance(self.run_mode),
                "started_at": _timestamp(),
                "acquisitions": [],
            },
        )
        self._refresh_cycles_csv()
        return CycleAttemptStore(self, path, cycle_index, number)

    def _refresh_cycles_csv(self) -> None:
        rows: list[dict[str, Any]] = []
        for cycle_index in range(self.config.scan.cycles):
            attempts = self._attempt_directories(cycle_index)
            if not attempts:
                continue
            attempt_number, path = attempts[-1]
            try:
                status = json.loads((path / "status.json").read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            rows.append(
                {
                    "cycle_index": cycle_index,
                    "attempt_number": attempt_number,
                    "state": status.get("state", "invalid"),
                    "acquisitions": len(status.get("acquisitions", [])),
                    "started_at": status.get("started_at", ""),
                    "terminal_at": status.get("completed_at", status.get("failed_at", "")),
                    "relative_path": path.relative_to(self.path).as_posix(),
                }
            )
        _write_csv_atomic(self.path / "cycles.csv", rows)


class CycleAttemptStore:
    def __init__(
        self,
        run: RepeatRunStore,
        path: Path,
        cycle_index: int,
        attempt_number: int,
    ) -> None:
        self._run = run
        self.path = Path(path)
        self.cycle_index = cycle_index
        self.attempt_number = attempt_number

    @property
    def _status_path(self) -> Path:
        return self.path / "status.json"

    def _status(self) -> dict[str, Any]:
        try:
            status = json.loads(self._status_path.read_text(encoding="utf-8"))
        except (OSError, TypeError, json.JSONDecodeError) as error:
            raise ValueError(f"invalid cycle status: {self.path}") from error
        if (
            status.get("cycle_index") != self.cycle_index
            or status.get("attempt_number") != self.attempt_number
            or status.get("run_mode") != self._run.run_mode
            or status.get("provenance") != _provenance(self._run.run_mode)
        ):
            raise ValueError("cycle attempt identity or provenance does not match directory")
        return status

    def _require_state(self, state: str) -> dict[str, Any]:
        status = self._status()
        if status.get("state") != state:
            raise ValueError(f"cycle attempt must be {state}")
        return status

    def save_acquisition(
        self,
        *,
        step: RepeatStep,
        spectrum: Any,
        voltage_status: Any,
        gain_status: Any,
        osa_duration_s: float,
    ) -> RepeatStoredAcquisition:
        status = self._require_state("running")
        expected_steps = cycle_steps(self._run.schedule, self.cycle_index)
        if not isinstance(step, RepeatStep) or step not in expected_steps:
            raise ValueError("step is not in this cycle schedule")
        if any(record.get("sequence_index") == step.sequence_index for record in status["acquisitions"]):
            raise ValueError("step has already been stored")
        if (
            isinstance(osa_duration_s, bool)
            or not isinstance(osa_duration_s, (int, float))
            or not math.isfinite(float(osa_duration_s))
            or osa_duration_s < 0.0
        ):
            raise ValueError("osa_duration_s must be a non-negative finite number")
        try:
            wavelength_nm = np.asarray(spectrum.wavelength_nm, dtype=float)
            power_dbm = np.asarray(spectrum.power_dbm, dtype=float)
            measured_voltage_v = np.asarray(voltage_status.voltage_v, dtype=float)
            measured_current_ma = np.asarray(voltage_status.current_ma, dtype=float)
        except (AttributeError, TypeError, ValueError) as error:
            raise ValueError("invalid acquisition snapshot") from error
        if wavelength_nm.ndim != 1 or power_dbm.shape != wavelength_nm.shape:
            raise ValueError("invalid spectrum arrays")
        if measured_voltage_v.shape != (8,) or measured_current_ma.shape != (8,):
            raise ValueError("voltage telemetry must contain eight channels")
        relative = _relative_path(step)
        final_path = self.path / relative
        if final_path.exists():
            raise ValueError("spectrum path already exists")
        acquired_at = spectrum.acquired_at.isoformat()
        provenance = _provenance(self._run.run_mode)
        payload: dict[str, Any] = {
            "wavelength_nm": wavelength_nm,
            "power_dbm": power_dbm,
            "requested_voltage_v": step.voltage_v,
            "voltage_squared_v2": step.voltage_squared_v2,
            "measured_voltage_v": measured_voltage_v,
            "measured_current_ma": measured_current_ma,
            "cycle_index": step.cycle_index,
            "direction": step.direction,
            "branch_index": step.branch_index,
            "pair_index": step.pair_index,
            "sequence_index": step.sequence_index,
            "acquired_at": acquired_at,
            "osa_duration_s": float(osa_duration_s),
            "trace": str(spectrum.trace),
            "identity": str(spectrum.identity),
            "measured_temperature_c": float(gain_status.temperature_c),
            "target_temperature_c": float(gain_status.target_c),
            "tec_enabled": bool(gain_status.tec_enabled),
            "current_setpoint_ma": float(gain_status.current_ma),
            "current_output_enabled": bool(gain_status.current_enabled),
            "voltage_status_received_at": float(voltage_status.received_at),
            "gain_status_received_at": float(gain_status.received_at),
            "provenance_label": provenance["label"],
            "run_mode": provenance["run_mode"],
            "simulated": provenance["simulated"],
        }
        for channel in range(8):
            payload[f"measured_voltage_{channel + 1}_v"] = float(measured_voltage_v[channel])
            payload[f"measured_current_{channel + 1}_ma"] = float(measured_current_ma[channel])
        _write_npz_atomic(final_path, **payload)
        record = {
            "relative_path": relative.as_posix(),
            **_step_payload(step),
            "acquired_at": acquired_at,
            "osa_duration_s": float(osa_duration_s),
            "trace": str(spectrum.trace),
            "identity": str(spectrum.identity),
            "measured_temperature_c": float(gain_status.temperature_c),
            "target_temperature_c": float(gain_status.target_c),
            "tec_enabled": bool(gain_status.tec_enabled),
            "current_setpoint_ma": float(gain_status.current_ma),
            "current_output_enabled": bool(gain_status.current_enabled),
            "voltage_status_received_at": float(voltage_status.received_at),
            "gain_status_received_at": float(gain_status.received_at),
            "provenance_label": provenance["label"],
            "run_mode": provenance["run_mode"],
            "simulated": provenance["simulated"],
        }
        for channel in range(8):
            record[f"measured_voltage_{channel + 1}_v"] = float(measured_voltage_v[channel])
            record[f"measured_current_{channel + 1}_ma"] = float(measured_current_ma[channel])
        status["acquisitions"].append(record)
        _write_csv_atomic(self.path / "acquisitions.csv", status["acquisitions"])
        _write_json_atomic(self._status_path, status)
        return RepeatStoredAcquisition(final_path, step, acquired_at, float(osa_duration_s), str(spectrum.trace), str(spectrum.identity))

    def mark_acquisition_complete(self) -> None:
        status = self._require_state("running")
        self._validate_records(status)
        status["state"] = "acquired"
        status["completed_at"] = _timestamp()
        _write_json_atomic(self._status_path, status)
        self._run._refresh_cycles_csv()

    def mark_postprocessed(self) -> None:
        status = self._require_state("acquired")
        missing = [name for name in _PRODUCT_NAMES if not (self.path / name).is_file()]
        if missing:
            raise ValueError(f"cycle product files are missing: {','.join(missing)}")
        status["state"] = "complete"
        status["postprocessed_at"] = _timestamp()
        _write_json_atomic(self._status_path, status)
        self._run._refresh_cycles_csv()

    def mark_failed(self, error: BaseException) -> None:
        status = self._require_state("running")
        status["state"] = "failed"
        status["failed_at"] = _timestamp()
        status["error_type"] = type(error).__name__
        status["error"] = str(error)
        _write_json_atomic(self._status_path, status)
        self._run._refresh_cycles_csv()

    def _validate_records(self, status: dict[str, Any]) -> tuple[RepeatStoredAcquisition, ...]:
        records = status.get("acquisitions")
        expected_steps = cycle_steps(self._run.schedule, self.cycle_index)
        if not isinstance(records, list) or len(records) != len(expected_steps):
            raise ValueError("cycle attempt does not contain its configured schedule")
        stored: list[RepeatStoredAcquisition] = []
        wavelength_grid: np.ndarray | None = None
        for expected, record in zip(expected_steps, records):
            relative = _relative_path(expected)
            if (
                not isinstance(record, dict)
                or record.get("relative_path") != relative.as_posix()
                or record.get("sequence_index") != expected.sequence_index
                or record.get("cycle_index") != expected.cycle_index
                or record.get("direction") != expected.direction
                or record.get("branch_index") != expected.branch_index
                or record.get("pair_index") != expected.pair_index
                or float(record.get("voltage_squared_v2")) != expected.voltage_squared_v2
                or float(record.get("voltage_v")) != expected.voltage_v
                or float(record.get("requested_voltage_v")) != expected.voltage_v
                or record.get("run_mode") != self._run.run_mode
                or record.get("simulated") is not _provenance(self._run.run_mode)["simulated"]
            ):
                raise ValueError("cycle acquisition records do not match the configured schedule")
            path = self.path / relative
            if not path.is_file():
                raise ValueError("cycle attempt references a missing spectrum")
            try:
                with np.load(path, allow_pickle=False) as payload:
                    wavelength = np.asarray(payload["wavelength_nm"], dtype=float)
                    power = np.asarray(payload["power_dbm"], dtype=float)
                    if (
                        int(payload["sequence_index"]) != expected.sequence_index
                        or int(payload["cycle_index"]) != expected.cycle_index
                        or str(payload["direction"]) != expected.direction
                        or int(payload["branch_index"]) != expected.branch_index
                        or int(payload["pair_index"]) != expected.pair_index
                        or float(payload["voltage_squared_v2"]) != expected.voltage_squared_v2
                        or float(payload["requested_voltage_v"]) != expected.voltage_v
                        or str(payload["run_mode"]) != self._run.run_mode
                    ):
                        raise ValueError("spectrum metadata does not match the configured schedule")
            except (OSError, KeyError, TypeError, ValueError) as error:
                if isinstance(error, ValueError) and str(error).startswith("spectrum metadata"):
                    raise
                raise ValueError("cycle attempt references an unreadable spectrum") from error
            if wavelength.ndim != 1 or power.shape != wavelength.shape:
                raise ValueError("spectrum arrays are invalid")
            if wavelength_grid is None:
                wavelength_grid = wavelength
            elif not np.array_equal(wavelength_grid, wavelength):
                raise ValueError("cycle spectra do not share one wavelength grid")
            stored.append(
                RepeatStoredAcquisition(
                    path=path,
                    step=expected,
                    acquired_at=str(record["acquired_at"]),
                    osa_duration_s=float(record["osa_duration_s"]),
                    trace=str(record["trace"]),
                    identity=str(record["identity"]),
                )
            )
        return tuple(stored)

    def completed_data(self) -> CycleData:
        status = self._status()
        if status.get("state") not in {"acquired", "complete"}:
            raise ValueError("cycle acquisition is not complete")
        return CycleData(
            path=self.path,
            cycle_index=self.cycle_index,
            attempt_number=self.attempt_number,
            acquisitions=self._validate_records(status),
            run_mode=self._run.run_mode,
            state=str(status["state"]),
        )


def load_complete_cycle(path: Path) -> CycleData:
    path = Path(path)
    try:
        status = json.loads((path / "status.json").read_text(encoding="utf-8"))
        cycle_index = int(status["cycle_index"])
        attempt_number = int(status["attempt_number"])
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid cycle status: {path}") from error
    run = RepeatRunStore.open_existing(path.parents[1])
    return CycleAttemptStore(run, path, cycle_index, attempt_number).completed_data()
