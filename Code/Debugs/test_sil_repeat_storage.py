from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

from Code.Experiments.sil_hysteresis.config import AnalysisConfig, DeviceConfig, OperationPoint
from Code.Experiments.sil_repeat.config import RepeatRunConfig, RepeatScanConfig
from Code.Experiments.sil_repeat.scan import cycle_steps
from Code.Experiments.sil_repeat.storage import RepeatRunStore, load_complete_cycle
from Code.Utils.gain import GainStatus
from Code.Utils.osa import Spectrum
from Code.Utils.voltage import VoltageStatus


def make_config(output_root: Path, *, cycles: int = 2, points_per_branch: int = 2) -> RepeatRunConfig:
    return RepeatRunConfig(
        devices=DeviceConfig(),
        scan=RepeatScanConfig(
            v2_min=40.0,
            v2_max=90.0,
            points_per_branch=points_per_branch,
            cycles=cycles,
        ),
        operation_point=OperationPoint(24.0, 100.0),
        analysis=AnalysisConfig(),
        output_root=output_root,
        display_latest=False,
    )


def fake_spectrum() -> Spectrum:
    return Spectrum(
        wavelength_nm=np.array([1050.0, 1050.1, 1050.2]),
        power_dbm=np.array([-80.0, -20.0, -60.0]),
        trace="A",
        acquired_at=datetime(2026, 8, 21, tzinfo=timezone.utc),
        identity="YOKOGAWA,AQ6370E",
    )


def fake_voltage_status() -> VoltageStatus:
    return VoltageStatus(
        voltage_v=(6.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        current_ma=(1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        received_at=42.0,
    )


def fake_gain_status() -> GainStatus:
    return GainStatus(
        temperature_c=24.0,
        target_c=24.0,
        tec_enabled=True,
        current_ma=100.0,
        current_enabled=True,
        received_at=43.0,
    )


def acquire_cycle(run: RepeatRunStore, cycle_index: int = 0):
    attempt = run.start_cycle_attempt(cycle_index)
    for step in cycle_steps(run.schedule, cycle_index):
        attempt.save_acquisition(
            step=step,
            spectrum=fake_spectrum(),
            voltage_status=fake_voltage_status(),
            gain_status=fake_gain_status(),
            osa_duration_s=1.25,
        )
    attempt.mark_acquisition_complete()
    return attempt


class RepeatStorageTests(unittest.TestCase):
    def test_each_cycle_has_bounded_manifest_and_unique_branch_paths(self):
        with tempfile.TemporaryDirectory() as root:
            run = RepeatRunStore.create(make_config(Path(root)), run_mode="real")
            attempt = acquire_cycle(run)
            status = json.loads((attempt.path / "status.json").read_text(encoding="utf-8"))
            self.assertEqual(status["state"], "acquired")
            self.assertEqual(len(status["acquisitions"]), 4)
            self.assertIn("requested_voltage_v", status["acquisitions"][0])
            self.assertEqual(
                status["acquisitions"][0]["requested_voltage_v"],
                run.schedule[0].voltage_v,
            )
            self.assertTrue((attempt.path / "forward" / "000_V2_40.000000.npz").is_file())
            self.assertTrue((attempt.path / "reverse" / "000_V2_90.000000.npz").is_file())
            with np.load(attempt.path / "forward" / "000_V2_40.000000.npz") as payload:
                self.assertEqual(str(payload["provenance_label"]), "REAL")
                self.assertEqual(str(payload["run_mode"]), "real")
                self.assertFalse(bool(payload["simulated"]))
            self.assertEqual(run.pending_cycle_indices(), (1,))
            loaded = load_complete_cycle(attempt.path)
            self.assertEqual(len(loaded.acquisitions), 4)
            self.assertEqual(loaded.run_mode, "real")

    def test_partial_cycle_is_preserved_and_resume_uses_new_attempt(self):
        with tempfile.TemporaryDirectory() as root:
            run = RepeatRunStore.create(make_config(Path(root), cycles=1))
            first = run.start_cycle_attempt(0)
            first.save_acquisition(
                step=cycle_steps(run.schedule, 0)[0],
                spectrum=fake_spectrum(),
                voltage_status=fake_voltage_status(),
                gain_status=fake_gain_status(),
                osa_duration_s=1.0,
            )
            first.mark_failed(RuntimeError("OSA timeout"))
            reopened = RepeatRunStore.open_existing(run.path)
            second = reopened.start_cycle_attempt(0)
            self.assertEqual(second.attempt_number, 2)
            self.assertTrue(first.path.joinpath("forward", "000_V2_40.000000.npz").is_file())

    def test_duplicate_step_is_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            run = RepeatRunStore.create(make_config(Path(root), cycles=1))
            attempt = run.start_cycle_attempt(0)
            step = cycle_steps(run.schedule, 0)[0]
            kwargs = dict(
                step=step,
                spectrum=fake_spectrum(),
                voltage_status=fake_voltage_status(),
                gain_status=fake_gain_status(),
                osa_duration_s=1.0,
            )
            attempt.save_acquisition(**kwargs)
            with self.assertRaisesRegex(ValueError, "already"):
                attempt.save_acquisition(**kwargs)

    def test_missing_spectrum_invalidates_acquired_cycle(self):
        with tempfile.TemporaryDirectory() as root:
            run = RepeatRunStore.create(make_config(Path(root), cycles=1))
            attempt = acquire_cycle(run)
            attempt.path.joinpath("forward", "000_V2_40.000000.npz").unlink()
            reopened = RepeatRunStore.open_existing(run.path)
            self.assertEqual(reopened.pending_cycle_indices(), (0,))
            with self.assertRaisesRegex(ValueError, "missing spectrum"):
                load_complete_cycle(attempt.path)

    def test_mutated_sequence_index_invalidates_acquired_cycle(self):
        with tempfile.TemporaryDirectory() as root:
            run = RepeatRunStore.create(make_config(Path(root), cycles=1))
            attempt = acquire_cycle(run)
            status_path = attempt.path / "status.json"
            status = json.loads(status_path.read_text(encoding="utf-8"))
            status["acquisitions"][0]["sequence_index"] = 999
            status_path.write_text(json.dumps(status), encoding="utf-8")
            reopened = RepeatRunStore.open_existing(run.path)
            self.assertEqual(reopened.pending_cycle_indices(), (0,))

    def test_mark_postprocessed_requires_all_product_files(self):
        with tempfile.TemporaryDirectory() as root:
            run = RepeatRunStore.create(make_config(Path(root), cycles=1))
            attempt = acquire_cycle(run)
            with self.assertRaisesRegex(ValueError, "product"):
                attempt.mark_postprocessed()
            for name in ("metrics.csv", "summary.json", "comparison.svg", "comparison.pdf", "comparison.png"):
                attempt.path.joinpath(name).write_text("fixture", encoding="utf-8")
            attempt.mark_postprocessed()
            status = json.loads(attempt.path.joinpath("status.json").read_text(encoding="utf-8"))
            self.assertEqual(status["state"], "complete")


if __name__ == "__main__":
    unittest.main()
