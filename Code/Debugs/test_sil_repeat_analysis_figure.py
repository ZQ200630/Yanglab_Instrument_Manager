from __future__ import annotations

import csv
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

from Code.Experiments.sil_hysteresis.config import AnalysisConfig, DeviceConfig, OperationPoint
from Code.Experiments.sil_repeat.analysis import (
    CycleSummaryRow,
    compare_cycle,
    save_repeat_summary_figure,
    write_cycle_products,
    write_repeat_summary,
)
from Code.Experiments.sil_repeat.config import RepeatRunConfig, RepeatScanConfig
from Code.Experiments.sil_repeat.scan import cycle_steps
from Code.Experiments.sil_repeat.storage import RepeatRunStore, load_complete_cycle
from Code.Utils.gain import GainStatus
from Code.Utils.osa import Spectrum
from Code.Utils.voltage import VoltageStatus


def config_for(root: Path, *, cycles: int = 1, points: int = 4) -> RepeatRunConfig:
    return RepeatRunConfig(
        devices=DeviceConfig(),
        scan=RepeatScanConfig(points_per_branch=points, cycles=cycles),
        operation_point=OperationPoint(24.0, 100.0),
        analysis=AnalysisConfig(signal_window_db=30.0),
        output_root=root,
        display_latest=False,
    )


def voltage_status(voltage: float) -> VoltageStatus:
    return VoltageStatus(
        voltage_v=(voltage, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        current_ma=(1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        received_at=42.0,
    )


def gain_status() -> GainStatus:
    return GainStatus(24.0, 24.0, True, 100.0, True, 43.0)


def spectrum_for(direction: str, pair_index: int) -> Spectrum:
    wavelength = np.linspace(1050.0, 1050.4, 5)
    base = np.array([-80.0, -50.0, -20.0, -55.0, -85.0])
    delta = 0.0 if direction == "forward" else float(pair_index + 1)
    return Spectrum(
        wavelength_nm=wavelength,
        power_dbm=base - delta,
        trace="A",
        acquired_at=datetime(2026, 8, 21, 12, 0, pair_index, tzinfo=timezone.utc),
        identity="YOKOGAWA,AQ6370E",
    )


def completed_cycle(root: Path, *, points: int = 4):
    config = config_for(root, points=points)
    run = RepeatRunStore.create(config, run_mode="real")
    attempt = run.start_cycle_attempt(0)
    for step in cycle_steps(run.schedule, 0):
        attempt.save_acquisition(
            step=step,
            spectrum=spectrum_for(step.direction, step.pair_index),
            voltage_status=voltage_status(step.voltage_v),
            gain_status=gain_status(),
            osa_duration_s=1.25,
        )
    attempt.mark_acquisition_complete()
    return config, run, attempt, load_complete_cycle(attempt.path)


class RepeatAnalysisTests(unittest.TestCase):
    def test_compare_cycle_stores_four_per_branch_but_compares_three_pairs(self):
        with tempfile.TemporaryDirectory() as root:
            config, _run, _attempt, data = completed_cycle(Path(root), points=4)
            result = compare_cycle(data, config.analysis)
            self.assertEqual(len([a for a in data.acquisitions if a.step.direction == "forward"]), 4)
            self.assertEqual(len([a for a in data.acquisitions if a.step.direction == "reverse"]), 4)
            self.assertEqual(result.voltage_v.shape, (3,))
            self.assertAlmostEqual(result.voltage_v[0] ** 2, 40.0)
            self.assertLess(result.voltage_v[-1] ** 2, 90.0)
            self.assertEqual(result.excluded_turning_points, 1)

    def test_cycle_products_include_metrics_summary_and_three_figures(self):
        with tempfile.TemporaryDirectory() as root:
            config, _run, attempt, data = completed_cycle(Path(root), points=4)
            products = write_cycle_products(data, config)
            self.assertEqual(products.summary_row.cycle_index, 0)
            self.assertTrue((attempt.path / "metrics.csv").is_file())
            self.assertTrue((attempt.path / "summary.json").is_file())
            self.assertEqual({path.suffix for path in products.figure_paths}, {".svg", ".pdf", ".png"})
            self.assertTrue(all(path.stat().st_size > 0 for path in products.figure_paths))

    def test_repeat_summary_is_sorted_and_aggregate_figure_has_three_formats(self):
        with tempfile.TemporaryDirectory() as root:
            run_path = Path(root)
            rows = (
                CycleSummaryRow(1, 3.0, 0.2, 70.0, 1.5, "b", "c", "OBSERVATION_REQUIRED", "real"),
                CycleSummaryRow(0, 2.0, 0.1, 60.0, 1.4, "a", "b", "OBSERVATION_REQUIRED", "real"),
            )
            summary_path = write_repeat_summary(run_path, rows)
            with summary_path.open(newline="", encoding="utf-8") as handle:
                saved = list(csv.DictReader(handle))
            self.assertEqual([int(row["cycle_index"]) for row in saved], [0, 1])
            figure_paths = save_repeat_summary_figure(rows, run_path / "repeat_summary")
            self.assertEqual({path.suffix for path in figure_paths}, {".svg", ".pdf", ".png"})
            self.assertTrue(all(path.stat().st_size > 0 for path in figure_paths))


if __name__ == "__main__":
    unittest.main()
