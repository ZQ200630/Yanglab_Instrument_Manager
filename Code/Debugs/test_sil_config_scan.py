import copy
import math
import unittest
from pathlib import Path

import numpy as np

from Code.Experiments.sil_hysteresis.config import (
    AnalysisConfig,
    DeviceConfig,
    OperationPoint,
    RunConfig,
    ScanConfig,
    load_config,
)
from Code.Experiments.sil_hysteresis.scan import (
    TimingTracker,
    build_scan_steps,
    build_voltage_grid,
)


DEFAULT_CONFIG = (
    Path(__file__).resolve().parents[1]
    / "Experiments"
    / "sil_hysteresis"
    / "default_config.json"
)


def valid_payload():
    return {
        "devices": {
            "osa_resource": "GPIB0::4::INSTR",
            "osa_trace": "A",
            "voltage_port": None,
            "gain_port": None,
            "gain_serial_number": None,
        },
        "scan": {
            "channel": 1,
            "v_min": 0.0,
            "v_max": 13.0,
            "points": 100,
            "spacing": "v_squared",
            "settle_time_s": 0.1,
        },
        "gain": {"operation_points": [{"temperature_c": 22.0, "current_ma": 80.0}]},
        "temperature_timeout_s": 300.0,
        "ld_settle_s": 5.0,
        "current_ramp_step_ma": 1.0,
        "current_ramp_interval_s": 0.1,
        "analysis": {
            "signal_window_db": 30.0,
            "display_floor_dbm": None,
            "thresholds": None,
        },
        "output_root": "Result/sil_hysteresis",
        "display_latest": True,
    }


class ConfigTests(unittest.TestCase):
    def test_default_config_matches_approved_single_point(self):
        config = load_config(DEFAULT_CONFIG)
        self.assertEqual(config.operation_points, (OperationPoint(22.0, 80.0),))
        self.assertEqual(config.scan.channel, 1)
        self.assertEqual(config.scan.v_min, 0.0)
        self.assertEqual(config.scan.v_max, 13.0)
        self.assertEqual(config.scan.points, 100)
        self.assertEqual(config.scan.spacing, "v_squared")
        self.assertEqual(config.scan.settle_time_s, 0.1)
        self.assertEqual(config.current_ramp_step_ma, 1.0)
        self.assertEqual(config.current_ramp_interval_s, 0.1)

    def test_grid_and_explicit_points_are_mutually_exclusive(self):
        payload = valid_payload()
        payload["gain"]["grid"] = {"temperature_c": [21, 22], "current_ma": [70, 80]}
        with self.assertRaisesRegex(ValueError, "exactly one"):
            RunConfig.from_mapping(payload)

    def test_grid_expands_temperature_major_then_current(self):
        payload = valid_payload()
        payload["gain"] = {
            "grid": {"temperature_c": [21.0, 22.0], "current_ma": [70.0, 80.0]}
        }
        config = RunConfig.from_mapping(payload)
        self.assertEqual(
            config.operation_points,
            (
                OperationPoint(21.0, 70.0),
                OperationPoint(21.0, 80.0),
                OperationPoint(22.0, 70.0),
                OperationPoint(22.0, 80.0),
            ),
        )

    def test_config_is_immutable(self):
        config = RunConfig.from_mapping(valid_payload())
        with self.assertRaises((AttributeError, TypeError)):
            config.scan.v_max = 12.0

    def test_rejects_unsafe_and_malformed_configuration_values(self):
        cases = (
            (lambda p: p["gain"]["operation_points"][0].update(temperature_c=True), "temperature"),
            (lambda p: p["gain"]["operation_points"][0].update(temperature_c=float("nan")), "temperature"),
            (lambda p: p["gain"]["operation_points"][0].update(temperature_c=14.999), "temperature"),
            (lambda p: p["gain"]["operation_points"][0].update(temperature_c=40.001), "temperature"),
            (lambda p: p["gain"]["operation_points"][0].update(current_ma=True), "current"),
            (lambda p: p["gain"]["operation_points"][0].update(current_ma=float("inf")), "current"),
            (lambda p: p["gain"]["operation_points"][0].update(current_ma=-0.001), "current"),
            (lambda p: p["gain"]["operation_points"][0].update(current_ma=200.001), "current"),
            (lambda p: p["scan"].update(channel=True), "channel"),
            (lambda p: p["scan"].update(channel=0), "channel"),
            (lambda p: p["scan"].update(channel=9), "channel"),
            (lambda p: p["scan"].update(v_min=-0.001), "voltage"),
            (lambda p: p["scan"].update(v_max=14.001), "voltage"),
            (lambda p: p["scan"].update(v_min=13.0), "v_min"),
            (lambda p: p["scan"].update(points=1), "points"),
            (lambda p: p["scan"].update(points=True), "points"),
            (lambda p: p["scan"].update(spacing="linear"), "spacing"),
            (lambda p: p["scan"].update(settle_time_s=-0.1), "settle"),
            (lambda p: p.update(temperature_timeout_s=-0.1), "temperature_timeout"),
            (lambda p: p.update(ld_settle_s=-0.1), "ld_settle"),
            (lambda p: p.update(current_ramp_step_ma=0.0), "current_ramp_step"),
            (lambda p: p.update(current_ramp_step_ma=1.001), "current_ramp_step"),
            (lambda p: p.update(current_ramp_interval_s=0.049), "current_ramp_interval"),
            (lambda p: p["devices"].update(osa_trace="H"), "OSA trace"),
        )
        for mutate, message in cases:
            with self.subTest(message=message):
                payload = valid_payload()
                mutate(payload)
                with self.assertRaisesRegex(ValueError, message):
                    RunConfig.from_mapping(payload)

    def test_rejects_invalid_operation_point_definitions(self):
        missing = valid_payload()
        missing["gain"] = {}
        empty = valid_payload()
        empty["gain"]["operation_points"] = []
        duplicate = valid_payload()
        duplicate["gain"]["operation_points"].append({"temperature_c": 22.0, "current_ma": 80.0})
        for payload, message in ((missing, "exactly one"), (empty, "empty"), (duplicate, "duplicate")):
            with self.subTest(message=message):
                with self.assertRaisesRegex(ValueError, message):
                    RunConfig.from_mapping(payload)

    def test_analysis_values_must_be_finite_numbers_and_thresholds_mapping(self):
        cases = (
            ("signal_window_db", True, "signal_window"),
            ("signal_window_db", float("nan"), "signal_window"),
            ("display_floor_dbm", True, "display_floor"),
            ("display_floor_dbm", float("inf"), "display_floor"),
            ("thresholds", [], "thresholds"),
            ("thresholds", {"mae": True}, "thresholds"),
            ("thresholds", {"mae": float("nan")}, "thresholds"),
        )
        for field, value, message in cases:
            with self.subTest(field=field, value=value):
                payload = valid_payload()
                payload["analysis"][field] = value
                with self.assertRaisesRegex(ValueError, message):
                    RunConfig.from_mapping(payload)


class ScanTests(unittest.TestCase):
    def test_voltage_grid_is_uniform_in_squared_voltage(self):
        grid = build_voltage_grid(ScanConfig(points=100))
        np.testing.assert_allclose(grid**2, np.linspace(0.0, 169.0, 100))
        self.assertEqual(grid[0], 0.0)
        self.assertEqual(grid[-1], 13.0)

    def test_schedule_is_continuous_and_has_199_acquisitions(self):
        steps = build_scan_steps(ScanConfig(points=100))
        self.assertEqual(len(steps), 199)
        self.assertEqual([step.direction for step in steps[:100]], ["forward"] * 100)
        self.assertEqual(steps[99].voltage_v, 13.0)
        self.assertLess(steps[100].voltage_v, 13.0)
        self.assertEqual(steps[-1].voltage_v, 0.0)

    def test_comparison_indices_pair_reverse_rows_to_ascending_rows(self):
        steps = build_scan_steps(ScanConfig(points=4, v_max=4.0))
        self.assertEqual([step.comparison_index for step in steps], [0, 1, 2, None, 2, 1, 0])
        self.assertEqual([step.direction_index for step in steps], [0, 1, 2, 3, 0, 1, 2])
        self.assertEqual([step.sequence_index for step in steps], list(range(7)))

    def test_timing_tracker_returns_none_until_acquisition_then_estimates_total_remaining_time(self):
        tracker = TimingTracker()
        self.assertIsNone(tracker.estimate_remaining(3, 0.3, 1.2))
        tracker.record_acquisition(2.0)
        tracker.record_acquisition(4.0)
        self.assertEqual(tracker.count, 2)
        self.assertEqual(tracker.total_s, 6.0)
        self.assertEqual(tracker.mean_s, 3.0)
        self.assertEqual(tracker.estimate_remaining(3, 0.3, 1.2), 10.5)

    def test_timing_tracker_rejects_nonfinite_or_negative_acquisition_durations(self):
        tracker = TimingTracker()
        for seconds in (True, -0.1, math.nan, math.inf):
            with self.subTest(seconds=seconds):
                with self.assertRaises(ValueError):
                    tracker.record_acquisition(seconds)


if __name__ == "__main__":
    unittest.main()
