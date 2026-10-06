"""Crash-safe, offline persistence for SIL hysteresis acquisitions.

This module deliberately accepts already-confirmed driver snapshots.  It never
constructs a driver, opens a VISA resource, or opens a serial port.
"""

from __future__ import annotations

import csv
import json
import math
import os
import re
import tempfile
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import numpy as np

from .config import RunConfig
from .scan import ScanStep, build_scan_steps


_ATTEMPT_RE = re.compile(r"attempt_(\d{3,})$")
_RUN_MODES = frozenset(("real", "similar", "hysteretic"))


def _validate_run_mode(run_mode: str) -> str:
    if run_mode not in _RUN_MODES:
        raise ValueError("run_mode must be real, similar, or hysteretic")
    return run_mode


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


def _timestamp() -> str:
    return datetime.now(timezone.utc).isoformat()


def _write_json_atomic(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", dir=path.parent, suffix=".tmp", delete=False
        ) as handle:
            temp_path = Path(handle.name)
            json.dump(payload, handle, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp_path, path)
    except BaseException:
        if temp_path is not None:
            temp_path.unlink(missing_ok=True)
        raise


def _write_npz_atomic(path: Path, **payload: Any) -> None:
    """Write an NPZ through a same-directory binary handle, then replace."""
    path.parent.mkdir(parents=True, exist_ok=True)
    temp_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(mode="w+b", dir=path.parent, suffix=".tmp", delete=False) as handle:
            temp_path = Path(handle.name)
            # Passing the handle prevents NumPy from appending a second .npz suffix.
            np.savez_compressed(handle, **payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp_path, path)
    except BaseException:
        if temp_path is not None:
            temp_path.unlink(missing_ok=True)
        raise


def _append_csv(path: Path, record: dict[str, Any]) -> None:
    """Durably append scalar acquisition metadata after its NPZ is durable."""
    path.parent.mkdir(parents=True, exist_ok=True)
    exists = path.exists()
    with path.open("a", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=tuple(record))
        if not exists:
            writer.writeheader()
        writer.writerow(record)
        handle.flush()
        os.fsync(handle.fileno())


def _config_payload(config: RunConfig) -> dict[str, Any]:
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
            "v_min": config.scan.v_min,
            "v_max": config.scan.v_max,
            "points": config.scan.points,
            "spacing": config.scan.spacing,
            "settle_time_s": config.scan.settle_time_s,
        },
        "gain": {
            "operation_points": [
                {"temperature_c": point.temperature_c, "current_ma": point.current_ma}
                for point in config.operation_points
            ]
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


def _step_payload(step: ScanStep) -> dict[str, Any]:
    return {
        "sequence_index": step.sequence_index,
        "direction": step.direction,
        "direction_index": step.direction_index,
        "voltage_v": step.voltage_v,
        "comparison_index": step.comparison_index,
    }


def _step_from_payload(payload: dict[str, Any]) -> ScanStep:
    return ScanStep(
        sequence_index=int(payload["sequence_index"]),
        direction=str(payload["direction"]),
        direction_index=int(payload["direction_index"]),
        voltage_v=float(payload["voltage_v"]),
        comparison_index=(None if payload["comparison_index"] is None else int(payload["comparison_index"])),
    )


def _op_directory(index: int, config: RunConfig) -> str:
    point = config.operation_points[index]
    return f"op_{index:03d}_T{point.temperature_c:.3f}_I{point.current_ma:.3f}"


def _attempt_path_for(run_path: Path, index: int, config: RunConfig, attempt_number: int) -> Path:
    return run_path / _op_directory(index, config) / f"attempt_{attempt_number:03d}"


@dataclass(frozen=True)
class StoredAcquisition:
    """Durable metadata for one stored spectrum."""

    path: Path
    step: ScanStep
    acquired_at: str
    osa_duration_s: float
    trace: str
    identity: str


@dataclass(frozen=True)
class CompletedAttemptData:
    """A fully revalidated completed attempt, ready for offline analysis."""

    path: Path
    operation_index: int
    attempt_number: int
    acquisitions: tuple[StoredAcquisition, ...]
    run_mode: str = "real"


class RunStore:
    """Owns a run directory and creates independently terminal attempts."""

    def __init__(self, path: Path, config: RunConfig, run_mode: str = "real") -> None:
        self.path = Path(path)
        self.config = config
        self.run_mode = _validate_run_mode(run_mode)
        self.schedule = build_scan_steps(config.scan)

    @classmethod
    def create(cls, config: RunConfig, run_mode: str = "real") -> "RunStore":
        if not isinstance(config, RunConfig):
            raise TypeError("config must be a RunConfig")
        run_mode = _validate_run_mode(run_mode)
        _require_real_acquisition(run_mode)
        config.output_root.mkdir(parents=True, exist_ok=True)
        run_id = f"run_{datetime.now(timezone.utc):%Y%m%dT%H%M%S%fZ}_{uuid.uuid4().hex[:8]}"
        path = config.output_root / run_id
        path.mkdir()
        manifest = {
            "version": 1,
            "created_at": _timestamp(),
            "run_mode": run_mode,
            "provenance": _provenance(run_mode),
            "config": _config_payload(config),
            "schedule": [_step_payload(step) for step in build_scan_steps(config.scan)],
        }
        _write_json_atomic(path / "run.json", manifest)
        return cls(path, config, run_mode)

    @classmethod
    def open_existing(cls, path: Path) -> "RunStore":
        path = Path(path)
        try:
            with (path / "run.json").open("r", encoding="utf-8") as handle:
                manifest = json.load(handle)
            run_mode = _validate_run_mode(manifest["run_mode"])
            config = RunConfig.from_mapping(manifest["config"])
            stored_schedule = tuple(_step_from_payload(step) for step in manifest["schedule"])
        except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
            raise ValueError(f"invalid run manifest: {path}") from error
        if manifest.get("provenance") != _provenance(run_mode):
            raise ValueError("run manifest provenance does not match run_mode")
        run = cls(path, config, run_mode)
        if stored_schedule != run.schedule:
            raise ValueError("run manifest schedule does not match its configuration")
        # A complete marker is only advisory.  Force validation while reopening.
        for index in range(len(config.operation_points)):
            run.operation_complete(index)
        return run

    def _attempt_directories(self, operation_index: int) -> tuple[tuple[int, Path], ...]:
        operation_path = self.path / _op_directory(operation_index, self.config)
        if not operation_path.exists():
            return ()
        attempts: list[tuple[int, Path]] = []
        for child in operation_path.iterdir():
            match = _ATTEMPT_RE.fullmatch(child.name)
            if child.is_dir() and match:
                attempts.append((int(match.group(1)), child))
        return tuple(sorted(attempts))

    def start_attempt(self, operation_index: int) -> "AttemptStore":
        _require_real_acquisition(self.run_mode)
        if not isinstance(operation_index, int) or isinstance(operation_index, bool):
            raise ValueError("operation_index must be an integer")
        if not 0 <= operation_index < len(self.config.operation_points):
            raise ValueError("operation_index is outside the configured operation points")
        if self.operation_complete(operation_index):
            raise ValueError("operation is already complete")
        attempts = self._attempt_directories(operation_index)
        attempt_number = (attempts[-1][0] if attempts else 0) + 1
        path = _attempt_path_for(self.path, operation_index, self.config, attempt_number)
        path.mkdir(parents=True, exist_ok=False)
        _write_json_atomic(
            path / "status.json",
            {
                "state": "running",
                "operation_index": operation_index,
                "attempt_number": attempt_number,
                "run_mode": self.run_mode,
                "provenance": _provenance(self.run_mode),
                "started_at": _timestamp(),
                "acquisitions": [],
            },
        )
        return AttemptStore(self, path, operation_index, attempt_number)

    def operation_complete(self, operation_index: int) -> bool:
        if not isinstance(operation_index, int) or isinstance(operation_index, bool):
            raise ValueError("operation_index must be an integer")
        if not 0 <= operation_index < len(self.config.operation_points):
            raise ValueError("operation_index is outside the configured operation points")
        for attempt_number, path in self._attempt_directories(operation_index):
            try:
                AttemptStore(self, path, operation_index, attempt_number)._completed_data()
            except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError):
                continue
            return True
        return False

    def pending_operation_indices(self) -> tuple[int, ...]:
        return tuple(
            index
            for index in range(len(self.config.operation_points))
            if not self.operation_complete(index)
        )


class AttemptStore:
    """Persists spectra for one operation-point attempt and its state machine."""

    def __init__(self, run: RunStore, path: Path, operation_index: int, attempt_number: int) -> None:
        self._run = run
        self.path = Path(path)
        self.operation_index = operation_index
        self.attempt_number = attempt_number

    @property
    def _status_path(self) -> Path:
        return self.path / "status.json"

    def _status(self) -> dict[str, Any]:
        try:
            with self._status_path.open("r", encoding="utf-8") as handle:
                status = json.load(handle)
        except (OSError, TypeError, json.JSONDecodeError) as error:
            raise ValueError(f"invalid attempt status: {self.path}") from error
        if not isinstance(status, dict):
            raise ValueError(f"invalid attempt status: {self.path}")
        if status.get("operation_index") != self.operation_index or status.get("attempt_number") != self.attempt_number:
            raise ValueError("attempt status identity does not match directory")
        if status.get("run_mode") != self._run.run_mode or status.get("provenance") != _provenance(
            self._run.run_mode
        ):
            raise ValueError("attempt provenance does not match run manifest")
        return status

    def _require_running(self) -> dict[str, Any]:
        status = self._status()
        if status.get("state") != "running":
            raise ValueError("attempt is terminal")
        return status

    def save_acquisition(
        self,
        *,
        step: ScanStep,
        spectrum: Any,
        voltage_status: Any,
        gain_status: Any,
        osa_duration_s: float,
    ) -> StoredAcquisition:
        """Atomically store a confirmed acquisition, then record it in the manifest."""
        status = self._require_running()
        if not isinstance(step, ScanStep) or not 0 <= step.sequence_index < len(self._run.schedule):
            raise ValueError("step is not in the configured schedule")
        if step != self._run.schedule[step.sequence_index]:
            raise ValueError("step is not in the configured schedule")
        if (
            isinstance(osa_duration_s, bool)
            or not isinstance(osa_duration_s, (int, float))
            or not math.isfinite(float(osa_duration_s))
            or osa_duration_s < 0
        ):
            raise ValueError("osa_duration_s must be a non-negative number")
        try:
            wavelength_nm = np.asarray(spectrum.wavelength_nm, dtype=float)
            power_dbm = np.asarray(spectrum.power_dbm, dtype=float)
            measured_voltage_v = np.asarray(voltage_status.voltage_v, dtype=float)
            measured_current_ma = np.asarray(voltage_status.current_ma, dtype=float)
        except (AttributeError, TypeError, ValueError) as error:
            raise ValueError("invalid acquisition snapshot") from error
        if wavelength_nm.ndim != 1 or power_dbm.ndim != 1 or wavelength_nm.shape != power_dbm.shape:
            raise ValueError("invalid spectrum arrays")
        if measured_voltage_v.shape != (8,) or measured_current_ma.shape != (8,):
            raise ValueError("voltage telemetry must contain eight channels")
        if any(record.get("sequence_index") == step.sequence_index for record in status.get("acquisitions", [])):
            raise ValueError("step has already been stored in this attempt")
        relative_path = Path(step.direction) / f"{step.direction_index:03d}_V{step.voltage_v:.6f}.npz"
        final_path = self.path / relative_path
        if final_path.exists():
            raise ValueError("spectrum path already exists")
        acquired_at = spectrum.acquired_at.isoformat()
        provenance = _provenance(self._run.run_mode)
        npz_payload: dict[str, Any] = {
            "wavelength_nm": wavelength_nm,
            "power_dbm": power_dbm,
            "requested_voltage_v": float(step.voltage_v),
            "measured_voltage_v": measured_voltage_v,
            "measured_current_ma": measured_current_ma,
            "direction": step.direction,
            "direction_index": step.direction_index,
            "sequence_index": step.sequence_index,
            "comparison_index": -1 if step.comparison_index is None else step.comparison_index,
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
            npz_payload[f"measured_voltage_{channel + 1}_v"] = float(measured_voltage_v[channel])
            npz_payload[f"measured_current_{channel + 1}_ma"] = float(measured_current_ma[channel])
        _write_npz_atomic(final_path, **npz_payload)
        record = {
            "relative_path": relative_path.as_posix(),
            "sequence_index": step.sequence_index,
            "direction": step.direction,
            "direction_index": step.direction_index,
            "requested_voltage_v": step.voltage_v,
            "comparison_index": step.comparison_index,
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
        _append_csv(self.path / "acquisitions.csv", record)
        status["acquisitions"].append(record)
        _write_json_atomic(self._status_path, status)
        return StoredAcquisition(final_path, step, acquired_at, float(osa_duration_s), str(spectrum.trace), str(spectrum.identity))

    def mark_complete(self) -> None:
        status = self._require_running()
        self._validate_records(status)
        status["state"] = "complete"
        status["completed_at"] = _timestamp()
        _write_json_atomic(self._status_path, status)

    def mark_failed(self, error: BaseException) -> None:
        status = self._require_running()
        status["state"] = "failed"
        status["failed_at"] = _timestamp()
        status["error_type"] = type(error).__name__
        status["error"] = str(error)
        _write_json_atomic(self._status_path, status)

    def _validate_records(self, status: dict[str, Any]) -> tuple[StoredAcquisition, ...]:
        records = status.get("acquisitions")
        if not isinstance(records, list) or len(records) != len(self._run.schedule):
            raise ValueError("attempt does not contain the configured schedule")
        stored: list[StoredAcquisition] = []
        wavelength_grid: np.ndarray | None = None
        for expected, record in zip(self._run.schedule, records):
            if not isinstance(record, dict):
                raise ValueError("attempt acquisition record is invalid")
            expected_relative = Path(expected.direction) / f"{expected.direction_index:03d}_V{expected.voltage_v:.6f}.npz"
            if (
                record.get("relative_path") != expected_relative.as_posix()
                or record.get("sequence_index") != expected.sequence_index
                or record.get("direction") != expected.direction
                or record.get("direction_index") != expected.direction_index
                or record.get("comparison_index") != expected.comparison_index
                or float(record.get("requested_voltage_v")) != expected.voltage_v
                or record.get("provenance_label") != _provenance(self._run.run_mode)["label"]
                or record.get("run_mode") != self._run.run_mode
                or record.get("simulated") is not _provenance(self._run.run_mode)["simulated"]
            ):
                raise ValueError("attempt acquisition records do not match the configured schedule")
            path = self.path / expected_relative
            if not path.is_file():
                raise ValueError("attempt references a missing spectrum file")
            try:
                with np.load(path, allow_pickle=False) as payload:
                    wavelength = np.asarray(payload["wavelength_nm"], dtype=float)
                    power = np.asarray(payload["power_dbm"], dtype=float)
                    if (
                        float(payload["requested_voltage_v"]) != expected.voltage_v
                        or str(payload["direction"]) != expected.direction
                        or int(payload["direction_index"]) != expected.direction_index
                        or int(payload["sequence_index"]) != expected.sequence_index
                        or str(payload["provenance_label"]) != _provenance(self._run.run_mode)["label"]
                        or str(payload["run_mode"]) != self._run.run_mode
                        or bool(payload["simulated"]) != _provenance(self._run.run_mode)["simulated"]
                    ):
                        raise ValueError("spectrum file metadata does not match its schedule")
            except (OSError, KeyError, TypeError, ValueError) as error:
                if isinstance(error, ValueError) and str(error).startswith("spectrum file metadata"):
                    raise
                raise ValueError("attempt references an unreadable spectrum file") from error
            if wavelength.ndim != 1 or power.ndim != 1 or wavelength.shape != power.shape:
                raise ValueError("spectrum file arrays are invalid")
            if wavelength_grid is None:
                wavelength_grid = wavelength
            elif not np.array_equal(wavelength_grid, wavelength):
                raise ValueError("attempt spectra do not share one wavelength grid")
            stored.append(
                StoredAcquisition(
                    path=path,
                    step=expected,
                    acquired_at=str(record["acquired_at"]),
                    osa_duration_s=float(record["osa_duration_s"]),
                    trace=str(record["trace"]),
                    identity=str(record["identity"]),
                )
            )
        return tuple(stored)

    def _completed_data(self) -> CompletedAttemptData:
        status = self._status()
        if status.get("state") != "complete":
            raise ValueError("attempt is not complete")
        acquisitions = self._validate_records(status)
        return CompletedAttemptData(
            self.path,
            self.operation_index,
            self.attempt_number,
            acquisitions,
            self._run.run_mode,
        )


def load_complete_attempt(path: Path) -> CompletedAttemptData:
    """Open a completed attempt only after revalidating every referenced NPZ."""
    path = Path(path)
    try:
        with (path / "status.json").open("r", encoding="utf-8") as handle:
            status = json.load(handle)
        operation_index = int(status["operation_index"])
        attempt_number = int(status["attempt_number"])
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid attempt status: {path}") from error
    run = RunStore.open_existing(path.parents[1])
    attempt = AttemptStore(run, path, operation_index, attempt_number)
    return attempt._completed_data()
