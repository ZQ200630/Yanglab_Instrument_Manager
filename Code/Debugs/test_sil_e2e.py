"""Default real-path CLI storage/analysis contracts with fixed test inputs; no hardware acceptance."""

from __future__ import annotations

import csv
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import matplotlib.pyplot as plt
import numpy as np

from Code.Experiments.sil_hysteresis.analysis import compare_attempt
from Code.Experiments.sil_hysteresis.config import load_config
from Code.Experiments.sil_hysteresis.figure import (
    OperationMetadata,
    build_comparison_figure,
)
from Code.Experiments.sil_hysteresis.run import DEFAULT_CONFIG, main
from Code.Experiments.sil_hysteresis.storage import load_complete_attempt


class EndToEndStorageTests(unittest.TestCase):
    """Catch incomplete triangles, provenance loss, and raw/product drift."""

    def test_default_cli_fixed_inputs_preserve_traceability_and_separate_runs(self):
        from functools import partial
        from Code.Debugs.test_sil_experiment_cli import ManualClock
        from Code.Debugs.runner_fixture import runner_factories
        from Code.Experiments.sil_hysteresis.experiment import ExperimentRunner
        with tempfile.TemporaryDirectory() as root:
            payload = json.loads(DEFAULT_CONFIG.read_text(encoding="utf-8"))
            payload["output_root"] = str(Path(root) / "results")
            config_path = Path(root) / "default_with_temporary_output.json"
            config_path.write_text(json.dumps(payload), encoding="utf-8")
            clock = ManualClock()
            with patch("Code.Experiments.sil_hysteresis.run.ExperimentRunner",
                       partial(ExperimentRunner, clock=clock, sleep=clock.sleep)), patch(
                "Code.Experiments.sil_hysteresis.run._default_driver_factories",
                side_effect=AssertionError("Uninjected hardware construction")):
                for _ in range(2):
                    self.assertEqual(main(["--config", str(config_path), "--no-display",
                        "--log-level", "ERROR"], input_fn=lambda _: "RUN",
                        factories=runner_factories(clock), output=io.StringIO()), 0)
                runs = tuple((Path(root) / "results").glob("run_*"))
                self.assertEqual(len(runs), 2)
                self.assertNotEqual(runs[0].name, runs[1].name)
                for run_path in runs:
                    self._audit_run(run_path, "real", config_path)

    def _audit_run(self, run_path: Path, mode: str, config_path: Path):
        manifest = json.loads((run_path / "run.json").read_text(encoding="utf-8"))
        self.assertEqual(manifest["run_mode"], mode)
        self.assertEqual(
            manifest["provenance"],
            {"label": "REAL", "run_mode": mode, "simulated": False},
        )
        self.assertEqual(len(manifest["schedule"]), 199)
        self.assertEqual(manifest["schedule"][99]["voltage_v"], 13.0)
        self.assertIsNone(manifest["schedule"][99]["comparison_index"])

        attempt_path = next(run_path.glob("op_*/attempt_*"))
        status = json.loads((attempt_path / "status.json").read_text(encoding="utf-8"))
        self.assertEqual(status["state"], "complete")
        self.assertEqual(len(status["acquisitions"]), 199)
        self.assertEqual(len(tuple(attempt_path.glob("forward/*.npz"))), 100)
        self.assertEqual(len(tuple(attempt_path.glob("reverse/*.npz"))), 99)
        for name in (
            "acquisitions.csv",
            "metrics.csv",
            "summary.json",
            "comparison.svg",
            "comparison.pdf",
            "comparison.png",
        ):
            self.assertTrue((attempt_path / name).is_file(), name)

        data = load_complete_attempt(attempt_path)
        result = compare_attempt(data, load_config(config_path).analysis)
        self.assertEqual(len(data.acquisitions), 199)
        self.assertEqual(result.run_mode, mode)
        self.assertEqual(result.classification, "OBSERVATION_REQUIRED")
        self.assertEqual(result.classification_scope, "observation")
        self.assertEqual(result.excluded_turning_points, 1)
        self.assertEqual(result.excluded_turning_point_voltage_v, 13.0)
        self.assertEqual(result.voltage_v.size, 99)
        self.assertLess(float(np.max(result.voltage_v)), 13.0)
        all_power_dbm = np.concatenate((result.forward_dbm.ravel(), result.reverse_dbm.ravel()))
        self.assertGreaterEqual(float(np.min(all_power_dbm)), -82.2)
        self.assertLessEqual(float(np.max(all_power_dbm)), -19.9)
        self.assertLessEqual(float(np.max(np.abs(result.difference_db))), 60.0)

        with (attempt_path / "metrics.csv").open(newline="", encoding="utf-8") as handle:
            metric_rows = list(csv.DictReader(handle))
        self.assertEqual(len(metric_rows), 99)
        np.testing.assert_array_equal(
            np.asarray([float(row["voltage_v"]) for row in metric_rows]),
            result.voltage_v,
        )
        np.testing.assert_array_equal(
            np.asarray([float(row["signal_mae_db"]) for row in metric_rows]),
            result.signal_mae_db,
        )
        np.testing.assert_array_equal(
            np.asarray([float(row["peak_shift_nm"]) for row in metric_rows]),
            result.peak_shift_nm,
        )
        self.assertTrue(all(row["run_mode"] == mode for row in metric_rows))
        self.assertTrue(all(row["provenance_label"] == "REAL" for row in metric_rows))

        figure = build_comparison_figure(
            result,
            OperationMetadata(22.0, 80.0, 0, run_mode=mode),
        )
        self.addCleanup(plt.close, figure)
        forward_axis = next(axis for axis in figure.axes if axis.get_title() == "Forward sweep")
        reverse_axis = next(axis for axis in figure.axes if axis.get_title() == "Reverse sweep")
        difference_axis = next(
            axis for axis in figure.axes if axis.get_title() == "Forward − reverse power"
        )
        metric_axis = next(axis for axis in figure.axes if axis.get_title() == "Signal discrepancy")
        shift_axis = next(axis for axis in figure.axes if axis.get_ylabel() == "|Peak shift| (nm)")
        np.testing.assert_array_equal(forward_axis.images[0].get_array(), result.forward_dbm.T)
        np.testing.assert_array_equal(reverse_axis.images[0].get_array(), result.reverse_dbm.T)
        self.assertIs(forward_axis.images[0].norm, reverse_axis.images[0].norm)
        np.testing.assert_array_equal(difference_axis.images[0].get_array(), result.difference_db.T)
        self.assertEqual(difference_axis.images[0].norm.vcenter, 0.0)
        self.assertAlmostEqual(
            difference_axis.images[0].norm.vmin,
            -difference_axis.images[0].norm.vmax,
        )
        np.testing.assert_array_equal(metric_axis.lines[0].get_xdata(), result.voltage_v**2)
        np.testing.assert_array_equal(metric_axis.lines[0].get_ydata(), result.signal_mae_db)
        np.testing.assert_array_equal(shift_axis.lines[0].get_ydata(), np.abs(result.peak_shift_nm))
        self.assertEqual(figure._sil_provenance, "REAL run")
        self.assertIn("OBSERVATION_REQUIRED", figure._suptitle.get_text())
        return result


if __name__ == "__main__":
    unittest.main()
