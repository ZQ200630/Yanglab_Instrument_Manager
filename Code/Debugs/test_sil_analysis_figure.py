from __future__ import annotations

import csv
import json
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path

import numpy as np
from matplotlib.backends.backend_agg import FigureCanvasAgg
from matplotlib.figure import Figure
from matplotlib.text import Text

from Code.Experiments.sil_hysteresis.analysis import compare_attempt, write_metrics_csv
from Code.Experiments.sil_hysteresis.config import AnalysisConfig, ScanConfig
from Code.Experiments.sil_hysteresis.figure import (
    LatestResultWindow,
    OperationMetadata,
    build_comparison_figure,
    save_comparison_figure,
)
from Code.Experiments.sil_hysteresis.scan import build_scan_steps
from Code.Experiments.sil_hysteresis.storage import CompletedAttemptData, StoredAcquisition


def synthetic_attempt(points: int = 4, reverse_offset_db: float = 0.0) -> CompletedAttemptData:
    """Build a validated-shaped, on-disk triangular attempt without hardware."""
    root = Path(tempfile.mkdtemp())
    schedule = build_scan_steps(ScanConfig(v_max=4.0, points=points))
    wavelength_nm = np.array([1550.0, 1550.1, 1550.2, 1550.3])
    acquisitions: list[StoredAcquisition] = []
    for step in schedule:
        forward_power_dbm = np.array([-45.0, -20.0, -25.0, -50.0]) + step.voltage_v
        power_dbm = (
            forward_power_dbm
            if step.direction == "forward"
            else forward_power_dbm + reverse_offset_db
        )
        path = root / f"{step.sequence_index:03d}.npz"
        np.savez(path, wavelength_nm=wavelength_nm, power_dbm=power_dbm)
        acquisitions.append(
            StoredAcquisition(path, step, "2026-08-20T00:00:00+00:00", 1.0, "A", "synthetic")
        )
    return CompletedAttemptData(root, 0, 1, tuple(acquisitions))


def dense_attempt(reverse_offset_db=0.0):
    """Dense plotting fixture from fixed four-bin inputs, not an instrument model."""
    return synthetic_attempt(points=100, reverse_offset_db=reverse_offset_db)


def with_step_changes(
    data: CompletedAttemptData, changes: dict[int, dict[str, object]]
) -> CompletedAttemptData:
    """Return the same stored observations with deliberately altered schedule metadata."""
    acquisitions = list(data.acquisitions)
    for index, fields in changes.items():
        acquisitions[index] = replace(acquisitions[index], step=replace(acquisitions[index].step, **fields))
    return replace(data, acquisitions=tuple(acquisitions))


def comparison_result():
    """Produce a complete, real comparison result for figure behavior tests."""
    data = synthetic_attempt(points=4, reverse_offset_db=1.5)
    result = compare_attempt(data, AnalysisConfig(signal_window_db=30.0))
    return result, data.path


def metadata(operation_index: int = 0, run_mode: str = "real") -> OperationMetadata:
    return OperationMetadata(
        temperature_c=22.0,
        current_ma=80.0,
        operation_index=operation_index,
        run_mode=run_mode,
    )


class FakeCanvas(FigureCanvasAgg):
    def __init__(self, figure: Figure) -> None:
        super().__init__(figure)
        self.draw_idle_calls = 0
        self.flush_events_calls = 0

    def draw_idle(self) -> None:
        self.draw_idle_calls += 1

    def flush_events(self) -> None:
        self.flush_events_calls += 1


class FakeFigure(Figure):
    def __init__(self) -> None:
        self.clear_calls = 0
        super().__init__()
        self.canvas = FakeCanvas(self)
        self.clear_calls = 0

    def clear(self) -> None:
        self.clear_calls += 1
        super().clear()


class FakePyplot:
    def __init__(self) -> None:
        self.figure_calls = 0
        self.show_calls: list[bool] = []

    def figure(self):
        self.figure_calls += 1
        return FakeFigure()

    def show(self, *, block: bool) -> None:
        self.show_calls.append(block)


def fake_pyplot() -> FakePyplot:
    return FakePyplot()


class BrokenShowPyplot(FakePyplot):
    def show(self, *, block: bool) -> None:
        self.show_calls.append(block)
        raise RuntimeError("GUI unavailable")


class AnalysisTests(unittest.TestCase):
    def test_reverse_branch_is_aligned_to_ascending_voltage(self):
        data = synthetic_attempt(points=4, reverse_offset_db=2.0)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))

        result = compare_attempt(data, AnalysisConfig(signal_window_db=30.0))

        np.testing.assert_allclose(result.voltage_v, [0.0, np.sqrt(16.0 / 3.0), np.sqrt(32.0 / 3.0)])
        np.testing.assert_allclose(result.difference_db, -2.0)
        self.assertEqual(result.excluded_turning_points, 1)

    def test_wavelength_mismatch_is_rejected_before_pairing(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        path = data.acquisitions[5].path
        with np.load(path) as payload:
            wavelength_nm = payload["wavelength_nm"].copy()
            power_dbm = payload["power_dbm"].copy()
        wavelength_nm[1] += 0.01
        np.savez(path, wavelength_nm=wavelength_nm, power_dbm=power_dbm)

        with self.assertRaisesRegex(ValueError, "wavelength grid"):
            compare_attempt(data, AnalysisConfig())

    def test_pair_voltage_mismatch_is_rejected_before_comparison(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        malformed = with_step_changes(
            data, {5: {"voltage_v": data.acquisitions[5].step.voltage_v + 0.01}}
        )

        with self.assertRaisesRegex(ValueError, "pair voltage"):
            compare_attempt(malformed, AnalysisConfig())

    def test_comparable_voltages_must_be_strictly_ascending(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        malformed = with_step_changes(
            data,
            {
                0: {"voltage_v": data.acquisitions[1].step.voltage_v},
                1: {"voltage_v": data.acquisitions[0].step.voltage_v},
                5: {"voltage_v": data.acquisitions[0].step.voltage_v},
                6: {"voltage_v": data.acquisitions[1].step.voltage_v},
            },
        )

        with self.assertRaisesRegex(ValueError, "ascending"):
            compare_attempt(malformed, AnalysisConfig())

    def test_excluded_observation_must_be_forward_turning_point(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        malformed = with_step_changes(data, {3: {"direction": "reverse"}})

        with self.assertRaisesRegex(ValueError, "forward turning point"):
            compare_attempt(malformed, AnalysisConfig())

    def test_excluded_turning_point_must_be_the_maximum_voltage(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        malformed = with_step_changes(data, {3: {"voltage_v": 3.0}})

        with self.assertRaisesRegex(ValueError, "maximum voltage"):
            compare_attempt(malformed, AnalysisConfig())

    def test_constant_spectra_return_nan_correlation_without_crashing(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        for acquisition in data.acquisitions:
            np.savez(
                acquisition.path,
                wavelength_nm=np.array([1550.0, 1550.1, 1550.2, 1550.3]),
                power_dbm=np.full(4, -25.0),
            )

        result = compare_attempt(data, AnalysisConfig())

        self.assertTrue(np.isnan(result.full_correlation).all())
        self.assertTrue(np.isnan(result.signal_correlation).all())

    def test_empty_signal_mask_has_an_explicit_error(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))

        with self.assertRaisesRegex(ValueError, "signal mask"):
            compare_attempt(data, AnalysisConfig(signal_window_db=-0.1))

    def test_full_spectrum_metrics_retain_every_wavelength_bin(self):
        data = synthetic_attempt(points=4, reverse_offset_db=2.0)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))

        result = compare_attempt(data, AnalysisConfig(signal_window_db=1.0))

        self.assertEqual(result.full_wavelength_bin_count, 4)
        np.testing.assert_allclose(result.full_mae_db, 2.0)
        np.testing.assert_allclose(result.full_rms_db, 2.0)

    def test_signal_metrics_use_union_of_differing_branch_peak_regions(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        wavelength_nm = np.array([1550.0, 1550.1, 1550.2, 1550.3, 1550.4])
        forward_dbm = np.array([-50.0, -10.0, -20.0, -50.0, -50.0])
        reverse_dbm = np.array([-50.0, -30.0, -40.0, -10.0, -50.0])
        for acquisition in data.acquisitions:
            np.savez(
                acquisition.path,
                wavelength_nm=wavelength_nm,
                power_dbm=forward_dbm if acquisition.step.direction == "forward" else reverse_dbm,
            )

        result = compare_attempt(data, AnalysisConfig(signal_window_db=15.0))

        expected_mask = np.array([False, True, True, True, False])
        np.testing.assert_array_equal(result.signal_mask[0], expected_mask)
        np.testing.assert_array_equal(result.signal_wavelength_bin_count, 3)
        np.testing.assert_allclose(result.signal_mae_db, 80.0 / 3.0)
        np.testing.assert_allclose(result.signal_rms_db, np.sqrt(800.0))
        np.testing.assert_allclose(result.signal_correlation, -0.8386278693775346)

    def test_null_thresholds_require_observation_even_for_identical_branches(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))

        result = compare_attempt(data, AnalysisConfig(thresholds=None))

        self.assertEqual(result.classification, "OBSERVATION_REQUIRED")

    def test_named_thresholds_produce_only_exploratory_labels(self):
        high_data = synthetic_attempt(points=4)
        different_data = synthetic_attempt(points=4, reverse_offset_db=2.0)
        review_data = synthetic_attempt(points=4, reverse_offset_db=2.0)
        self.addCleanup(lambda: __import__("shutil").rmtree(high_data.path))
        self.addCleanup(lambda: __import__("shutil").rmtree(different_data.path))
        self.addCleanup(lambda: __import__("shutil").rmtree(review_data.path))

        high = compare_attempt(
            high_data,
            AnalysisConfig(
                thresholds={
                    "high_max_p95_mae_db": 0.1,
                    "high_min_median_correlation": 0.9,
                    "different_min_p95_mae_db": 1.0,
                    "different_max_median_correlation": 0.5,
                }
            ),
        )
        different = compare_attempt(
            different_data,
            AnalysisConfig(
                thresholds={
                    "high_max_p95_mae_db": 0.1,
                    "high_min_median_correlation": 0.9,
                    "different_min_p95_mae_db": 1.0,
                    "different_max_median_correlation": 1.0,
                }
            ),
        )
        review = compare_attempt(
            review_data,
            AnalysisConfig(
                thresholds={
                    "high_max_p95_mae_db": 0.1,
                    "high_min_median_correlation": 0.9,
                    "different_min_p95_mae_db": 3.0,
                    "different_max_median_correlation": 0.5,
                }
            ),
        )

        self.assertEqual(high.classification, "HIGH_CONSISTENCY")
        self.assertEqual(different.classification, "VISIBLE_DIFFERENCE")
        self.assertEqual(review.classification, "REVIEW_RECOMMENDED")
        self.assertEqual(high.classification_scope, "exploratory")

    def test_invalid_threshold_keys_are_rejected(self):
        data = synthetic_attempt(points=4)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))

        with self.assertRaisesRegex(ValueError, "thresholds"):
            compare_attempt(data, AnalysisConfig(thresholds={"high_max_p95_mae_db": 0.1}))

    def test_metrics_csv_and_summary_json_keep_unrounded_source_values(self):
        data = synthetic_attempt(points=4, reverse_offset_db=1.234567890123)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        result = compare_attempt(data, AnalysisConfig())
        output = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: __import__("shutil").rmtree(output))

        write_metrics_csv(result, output / "metrics.csv")

        with (output / "metrics.csv").open(newline="", encoding="utf-8") as handle:
            rows = list(csv.DictReader(handle))
        self.assertEqual(len(rows), 3)
        self.assertEqual(
            tuple(rows[0]),
            (
                "provenance_label", "run_mode", "simulated",
                "voltage_v", "voltage_squared_v2", "full_mae_db", "full_rms_db", "full_correlation",
                "signal_mae_db", "signal_rms_db", "signal_correlation", "full_wavelength_bin_count",
                "signal_wavelength_bin_count", "forward_peak_nm", "reverse_peak_nm", "peak_shift_nm",
            ),
        )
        self.assertEqual(rows[0]["provenance_label"], "REAL")
        self.assertEqual(rows[0]["run_mode"], "real")
        self.assertEqual(float(rows[0]["full_mae_db"]), result.full_mae_db[0])
        with (output / "summary.json").open(encoding="utf-8") as handle:
            summary = json.load(handle)
        self.assertEqual(summary["classification"]["state"], "OBSERVATION_REQUIRED")
        self.assertEqual(
            summary["provenance"],
            {"label": "REAL", "run_mode": "real", "simulated": False},
        )
        self.assertEqual(summary["thresholds"], None)
        self.assertIn("aggregation_definitions", summary)
        self.assertEqual(len(summary["discrepancy_ranking"]), 3)


class FigureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.output = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: __import__("shutil").rmtree(self.output))

    def assert_visible_artists_within_canvas(self, figure):
        figure.canvas.draw()
        renderer = figure.canvas.get_renderer()
        canvas = figure.bbox
        tolerance_px = 1.0
        for text in figure.findobj(Text):
            if not text.get_visible() or not text.get_text().strip():
                continue
            bounds = text.get_window_extent(renderer)
            with self.subTest(text=text.get_text()):
                self.assertGreaterEqual(bounds.x0, canvas.x0 - tolerance_px)
                self.assertGreaterEqual(bounds.y0, canvas.y0 - tolerance_px)
                self.assertLessEqual(bounds.x1, canvas.x1 + tolerance_px)
                self.assertLessEqual(bounds.y1, canvas.y1 + tolerance_px)
        for axis in figure.axes:
            bounds = axis.get_tightbbox(renderer)
            with self.subTest(axis=axis.get_title() or axis.get_ylabel()):
                self.assertGreaterEqual(bounds.x0, canvas.x0 - tolerance_px)
                self.assertGreaterEqual(bounds.y0, canvas.y0 - tolerance_px)
                self.assertLessEqual(bounds.x1, canvas.x1 + tolerance_px)
                self.assertLessEqual(bounds.y1, canvas.y1 + tolerance_px)

    def test_real_figure_visible_artists_fit_inside_180_mm_canvas(self):
        result, source = comparison_result()
        self.addCleanup(lambda: __import__("shutil").rmtree(source))
        figure = build_comparison_figure(result, metadata(run_mode="real"))
        self.addCleanup(lambda: __import__("matplotlib.pyplot").pyplot.close(figure))

        self.assertIn(result.classification, figure._suptitle.get_text())
        self.assert_visible_artists_within_canvas(figure)

    def test_simulated_figure_provenance_and_classification_fit_inside_canvas(self):
        result, source = comparison_result()
        self.addCleanup(lambda: __import__("shutil").rmtree(source))
        figure = build_comparison_figure(result, metadata(run_mode="hysteretic"))
        self.addCleanup(lambda: __import__("matplotlib.pyplot").pyplot.close(figure))

        title = figure._suptitle.get_text()
        self.assertIn("SIMULATED", title)
        self.assertIn("hysteretic", title)
        self.assertIn(result.classification, title)
        self.assert_visible_artists_within_canvas(figure)

    def test_exports_editable_primary_and_preview_formats(self):
        result, source = comparison_result()
        self.addCleanup(lambda: __import__("shutil").rmtree(source))
        fig = build_comparison_figure(result, metadata())

        paths = save_comparison_figure(fig, self.output / "comparison")

        self.assertEqual({path.suffix for path in paths}, {".svg", ".pdf", ".png"})
        self.assertIn("<text", (self.output / "comparison.svg").read_text(encoding="utf-8"))

    def test_latest_window_reuses_one_figure(self):
        result, source = comparison_result()
        self.addCleanup(lambda: __import__("shutil").rmtree(source))
        window = LatestResultWindow(pyplot=fake_pyplot())

        window.update(result, metadata(operation_index=0))
        first_identity = id(window.figure)
        window.update(result, metadata(operation_index=1))

        self.assertEqual(id(window.figure), first_identity)
        self.assertEqual(window.pyplot.figure_calls, 1)
        self.assertEqual(window.pyplot.show_calls, [False])
        self.assertEqual(window.figure.clear_calls, 1)
        self.assertEqual(window.figure.canvas.draw_idle_calls, 2)
        self.assertEqual(window.figure.canvas.flush_events_calls, 2)

    def test_shared_power_colorbar_does_not_cover_reverse_panel(self):
        result, source = comparison_result()
        self.addCleanup(lambda: __import__("shutil").rmtree(source))
        figure = build_comparison_figure(result, metadata())
        self.addCleanup(lambda: __import__("matplotlib.pyplot").pyplot.close(figure))

        reverse_axis = next(axis for axis in figure.axes if axis.get_title() == "Reverse sweep")
        power_bar_axis = next(axis for axis in figure.axes if axis.get_ylabel() == "Power (dBm)")

        self.assertGreaterEqual(power_bar_axis.get_position().x0, reverse_axis.get_position().x1)

    def test_heatmap_cell_centers_match_every_stored_coordinate(self):
        result, source = comparison_result()
        self.addCleanup(lambda: __import__("shutil").rmtree(source))
        figure = build_comparison_figure(result, metadata())
        self.addCleanup(lambda: __import__("matplotlib.pyplot").pyplot.close(figure))

        expected_x = result.voltage_v**2
        expected_y = result.wavelength_nm
        for title in ("Forward sweep", "Reverse sweep", "Forward − reverse power"):
            axis = next(candidate for candidate in figure.axes if candidate.get_title() == title)
            image = axis.images[0]
            x_min, x_max, y_min, y_max = image.get_extent()
            rows, columns = image.get_array().shape
            rendered_x_centers = np.linspace(
                x_min + (x_max - x_min) / (2.0 * columns),
                x_max - (x_max - x_min) / (2.0 * columns),
                columns,
            )
            rendered_y_centers = np.linspace(
                y_min + (y_max - y_min) / (2.0 * rows),
                y_max - (y_max - y_min) / (2.0 * rows),
                rows,
            )
            with self.subTest(panel=title):
                np.testing.assert_allclose(rendered_x_centers, expected_x, atol=1e-12)
                np.testing.assert_allclose(rendered_y_centers, expected_y, atol=1e-12)

    def test_heatmaps_reject_nonmonotonic_or_nonuniform_voltage_squared_coordinates(self):
        result, source = comparison_result()
        self.addCleanup(lambda: __import__("shutil").rmtree(source))

        with self.assertRaisesRegex(ValueError, "strictly increasing"):
            build_comparison_figure(
                replace(result, voltage_v=np.array([0.0, 3.0, 2.0])),
                metadata(),
            )
        with self.assertRaisesRegex(ValueError, "uniformly spaced"):
            build_comparison_figure(
                replace(result, voltage_v=np.array([0.0, 1.0, 3.0])),
                metadata(),
            )

    def test_single_paired_voltage_is_rendered_at_its_stored_cell_center(self):
        data = synthetic_attempt(points=2, reverse_offset_db=1.0)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        result = compare_attempt(data, AnalysisConfig(signal_window_db=30.0))

        figure = build_comparison_figure(result, metadata())
        self.addCleanup(lambda: __import__("matplotlib.pyplot").pyplot.close(figure))

        self.assertEqual(result.voltage_v.size, 1)
        expected_center = float(result.voltage_v[0] ** 2)
        for title in ("Forward sweep", "Reverse sweep", "Forward − reverse power"):
            axis = next(candidate for candidate in figure.axes if candidate.get_title() == title)
            x_min, x_max, _, _ = axis.images[0].get_extent()
            with self.subTest(panel=title):
                self.assertAlmostEqual((x_min + x_max) / 2.0, expected_center)

    def test_single_paired_voltage_shows_only_its_nonnegative_v2_tick_on_all_panels(self):
        data = synthetic_attempt(points=2, reverse_offset_db=1.0)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        result = compare_attempt(data, AnalysisConfig(signal_window_db=30.0))
        figure = build_comparison_figure(result, metadata())
        self.addCleanup(lambda: __import__("matplotlib.pyplot").pyplot.close(figure))
        figure.canvas.draw()
        self.assert_visible_artists_within_canvas(figure)

        expected_tick = float(result.voltage_v[0] ** 2)
        for title in (
            "Forward sweep",
            "Reverse sweep",
            "Forward − reverse power",
            "Signal discrepancy",
        ):
            axis = next(candidate for candidate in figure.axes if candidate.get_title() == title)
            tick_labels = [label.get_text() for label in axis.get_xticklabels() if label.get_visible()]
            with self.subTest(panel=title):
                np.testing.assert_array_equal(axis.get_xticks(), [expected_tick])
                self.assertEqual(len(tick_labels), 1)
                self.assertNotIn("−", tick_labels[0])
                self.assertGreaterEqual(float(tick_labels[0]), 0.0)

    def test_dense_metric_panel_uses_direct_labels_without_connected_peak_markers(self):
        data = dense_attempt()
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        result = compare_attempt(data, AnalysisConfig(signal_window_db=30.0))
        figure = build_comparison_figure(result, metadata())
        self.addCleanup(lambda: __import__("matplotlib.pyplot").pyplot.close(figure))

        metric_axis = next(axis for axis in figure.axes if axis.get_title() == "Signal discrepancy")
        shift_axis = next(axis for axis in figure.axes if axis.get_ylabel() == "|Peak shift| (nm)")

        self.assertGreaterEqual(metric_axis.get_position().width, 0.20)
        self.assertIsNone(metric_axis.get_legend())
        self.assertEqual(shift_axis.lines[0].get_linestyle(), "None")

    def test_largest_discrepancy_annotation_stays_inside_metric_plot_area(self):
        data = dense_attempt()
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        result = compare_attempt(data, AnalysisConfig(signal_window_db=30.0))
        figure = build_comparison_figure(result, metadata())
        self.addCleanup(lambda: __import__("matplotlib.pyplot").pyplot.close(figure))
        figure.canvas.draw()
        renderer = figure.canvas.get_renderer()
        metric_axis = next(axis for axis in figure.axes if axis.get_title() == "Signal discrepancy")
        annotation = next(text for text in metric_axis.texts if text.get_text().startswith("Largest"))
        annotation_bounds = annotation.get_window_extent(renderer)
        plot_bounds = metric_axis.bbox

        self.assertGreaterEqual(annotation_bounds.x0, plot_bounds.x0)
        self.assertGreaterEqual(annotation_bounds.y0, plot_bounds.y0)
        self.assertLessEqual(annotation_bounds.x1, plot_bounds.x1)
        self.assertLessEqual(annotation_bounds.y1, plot_bounds.y1)

    def test_largest_discrepancy_label_clears_both_metric_series(self):
        data = dense_attempt(reverse_offset_db=2.0)
        self.addCleanup(lambda: __import__("shutil").rmtree(data.path))
        result = compare_attempt(data, AnalysisConfig(signal_window_db=30.0))
        figure = build_comparison_figure(result, metadata(run_mode="hysteretic"))
        self.addCleanup(lambda: __import__("matplotlib.pyplot").pyplot.close(figure))
        figure.canvas.draw()
        renderer = figure.canvas.get_renderer()
        metric_axis = next(axis for axis in figure.axes if axis.get_title() == "Signal discrepancy")
        shift_axis = next(axis for axis in figure.axes if axis.get_ylabel() == "|Peak shift| (nm)")
        annotation = next(text for text in metric_axis.texts if text.get_text().startswith("Largest"))
        # Annotation's union box includes the arrow and empty space between its
        # tip and text. That rectangle can contain nearby points even when
        # neither the label nor the arrow covers them. Check the label itself.
        bounds = Text.get_window_extent(annotation, renderer).expanded(1.04, 1.08)
        largest_index = int(np.argmax(result.signal_mae_db))

        for axis, line, excluded_index in (
            (metric_axis, metric_axis.lines[0], largest_index),
            (shift_axis, shift_axis.lines[0], None),
        ):
            points = axis.transData.transform(
                np.column_stack((line.get_xdata(), line.get_ydata()))
            )
            for index, (x, y) in enumerate(points):
                if index == excluded_index:
                    continue
                with self.subTest(axis=axis.get_ylabel(), index=index):
                    self.assertFalse(bounds.contains(x, y))

    def test_gui_failure_disables_later_display_attempts(self):
        result, source = comparison_result()
        self.addCleanup(lambda: __import__("shutil").rmtree(source))
        pyplot = BrokenShowPyplot()
        window = LatestResultWindow(pyplot=pyplot)

        with self.assertLogs("Code.Experiments.sil_hysteresis.figure", level="WARNING") as logs:
            window.update(result, metadata())
            window.update(result, metadata(operation_index=1))

        self.assertFalse(window.enabled)
        self.assertEqual(pyplot.show_calls, [False])
        self.assertEqual(len(logs.output), 1)


if __name__ == "__main__":
    unittest.main()
