from __future__ import annotations

import tempfile
import unittest
import csv
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

from Code.Experiments.sil_hysteresis.config import (
    DeviceConfig,
    OperationPoint,
    RunConfig,
    ScanConfig,
)
from Code.Experiments.sil_hysteresis.scan import ScanStep
from Code.Experiments.sil_hysteresis.storage import RunStore, load_complete_attempt
from Code.Utils.gain import GainStatus
from Code.Utils.osa import Spectrum
from Code.Utils.voltage import VoltageStatus


def make_config(output_root: Path) -> RunConfig:
    return RunConfig(
        devices=DeviceConfig(),
        scan=ScanConfig(points=2),
        operation_points=(OperationPoint(22.0, 80.0),),
        output_root=output_root,
    )


def make_two_operation_config(output_root: Path) -> RunConfig:
    return RunConfig(
        devices=DeviceConfig(),
        scan=ScanConfig(points=2),
        operation_points=(OperationPoint(22.0, 80.0), OperationPoint(23.0, 81.0)),
        output_root=output_root,
    )


def fake_spectrum() -> Spectrum:
    return Spectrum(
        wavelength_nm=np.array([1550.0, 1550.1]),
        power_dbm=np.array([-40.0, -20.0]),
        trace="A",
        acquired_at=datetime(2026, 8, 20, tzinfo=timezone.utc),
        identity="YOKOGAWA,AQ6370",
    )


def fake_voltage_status() -> VoltageStatus:
    return VoltageStatus(
        voltage_v=(0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0),
        current_ma=(10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0),
        received_at=42.0,
    )


def fake_gain_status() -> GainStatus:
    return GainStatus(
        temperature_c=22.05,
        target_c=22.0,
        tec_enabled=True,
        current_ma=80.0,
        current_enabled=True,
        received_at=43.0,
    )


def complete_attempt_with_schedule(attempt, schedule) -> None:
    for step in schedule:
        attempt.save_acquisition(
            step=step,
            spectrum=fake_spectrum(),
            voltage_status=fake_voltage_status(),
            gain_status=fake_gain_status(),
            osa_duration_s=1.25,
        )
    attempt.mark_complete()


class AtomicStorageTests(unittest.TestCase):
    def test_save_acquisition_writes_arrays_and_confirmed_metadata(self):
        with tempfile.TemporaryDirectory() as root:
            run = RunStore.create(make_config(Path(root)))
            attempt = run.start_attempt(0)
            saved = attempt.save_acquisition(
                step=ScanStep(0, "forward", 0, 0.0, 0),
                spectrum=fake_spectrum(),
                voltage_status=fake_voltage_status(),
                gain_status=fake_gain_status(),
                osa_duration_s=1.25,
            )
            self.assertTrue(saved.path.exists())
            self.assertFalse(any(saved.path.parent.glob("*.tmp")))
            with np.load(saved.path) as payload:
                np.testing.assert_array_equal(payload["wavelength_nm"], [1550.0, 1550.1])
                np.testing.assert_array_equal(payload["power_dbm"], [-40.0, -20.0])
                self.assertEqual(float(payload["requested_voltage_v"]), 0.0)
                np.testing.assert_array_equal(
                    payload["measured_voltage_v"], [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]
                )
                np.testing.assert_array_equal(
                    payload["measured_current_ma"], [10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0]
                )
                self.assertEqual(str(payload["trace"]), "A")
                self.assertEqual(str(payload["identity"]), "YOKOGAWA,AQ6370")
            with (attempt.path / "acquisitions.csv").open(newline="", encoding="utf-8") as handle:
                rows = list(csv.DictReader(handle))
            self.assertEqual(len(rows), 1)
            self.assertEqual(float(rows[0]["measured_voltage_8_v"]), 7.0)
            self.assertEqual(float(rows[0]["measured_current_1_ma"]), 10.0)
            self.assertEqual(float(rows[0]["measured_temperature_c"]), 22.05)


class AttemptSemanticsTests(unittest.TestCase):
    def test_incomplete_attempt_is_never_selected_as_complete(self):
        with tempfile.TemporaryDirectory() as root:
            run = RunStore.create(make_config(Path(root)))
            first = run.start_attempt(0)
            first.mark_failed(RuntimeError("OSA timeout"))
            second = run.start_attempt(0)
            self.assertEqual(first.attempt_number, 1)
            self.assertEqual(second.attempt_number, 2)
            self.assertFalse(run.operation_complete(0))
            with self.assertRaisesRegex(ValueError, "terminal"):
                first.mark_complete()

    def test_completed_operation_is_skipped_but_partial_operation_restarts(self):
        with tempfile.TemporaryDirectory() as root:
            run = RunStore.create(make_two_operation_config(Path(root)))
            complete_attempt_with_schedule(run.start_attempt(0), run.schedule)
            run.start_attempt(1).mark_failed(RuntimeError("interrupted"))
            self.assertEqual(run.pending_operation_indices(), (1,))

    def test_start_attempt_rejects_an_already_completed_operation(self):
        with tempfile.TemporaryDirectory() as root:
            run = RunStore.create(make_config(Path(root)))
            complete_attempt_with_schedule(run.start_attempt(0), run.schedule)
            with self.assertRaisesRegex(ValueError, "already complete"):
                run.start_attempt(0)

    def test_reopening_ignores_stale_complete_marker_with_missing_spectrum(self):
        with tempfile.TemporaryDirectory() as root:
            run = RunStore.create(make_config(Path(root)))
            attempt = run.start_attempt(0)
            complete_attempt_with_schedule(attempt, run.schedule)
            attempt.path.joinpath("forward", "000_V0.000000.npz").unlink()
            reopened = RunStore.open_existing(run.path)
            self.assertFalse(reopened.operation_complete(0))
            with self.assertRaisesRegex(ValueError, "missing spectrum"):
                load_complete_attempt(attempt.path)

    def test_nonfinite_osa_duration_is_rejected_before_writing_metadata(self):
        with tempfile.TemporaryDirectory() as root:
            run = RunStore.create(make_config(Path(root)))
            attempt = run.start_attempt(0)
            with self.assertRaisesRegex(ValueError, "osa_duration_s"):
                attempt.save_acquisition(
                    step=run.schedule[0],
                    spectrum=fake_spectrum(),
                    voltage_status=fake_voltage_status(),
                    gain_status=fake_gain_status(),
                    osa_duration_s=float("nan"),
                )
            self.assertFalse(any(attempt.path.rglob("*.npz")))


if __name__ == "__main__":
    unittest.main()
