import copy
import math
import unittest
from pathlib import Path

import numpy as np

from Code.Experiments.sil_repeat.config import (
    RepeatRunConfig,
    RepeatScanConfig,
    load_repeat_config,
)
from Code.Experiments.sil_repeat.scan import (
    build_repeat_schedule,
    build_v2_grid,
    cycle_steps,
)


APPROVED_CONFIG = (
    Path(__file__).resolve().parents[1]
    / "Experiments"
    / "sil_repeat"
    / "repeat_24C_100mA.json"
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
            "v2_min": 40.0,
            "v2_max": 90.0,
            "points_per_branch": 40,
            "cycles": 100,
            "settle_time_s": 0.1,
        },
        "gain": {"temperature_c": 24.0, "current_ma": 100.0},
        "temperature_timeout_s": 300.0,
        "ld_settle_s": 5.0,
        "current_ramp_step_ma": 1.0,
        "current_ramp_interval_s": 0.1,
        "analysis": {
            "signal_window_db": 30.0,
            "display_floor_dbm": None,
            "thresholds": None,
        },
        "output_root": "Result/sil_repeat",
        "display_latest": True,
    }


class RepeatConfigTests(unittest.TestCase):
    def test_mapping_normalizes_approved_operating_point(self):
        config = RepeatRunConfig.from_mapping(valid_payload())
        self.assertEqual(config.operation_point.temperature_c, 24.0)
        self.assertEqual(config.operation_point.current_ma, 100.0)
        self.assertEqual(config.scan, RepeatScanConfig())
        self.assertEqual(config.output_root, Path("Result/sil_repeat"))

    def test_load_approved_config(self):
        config = load_repeat_config(APPROVED_CONFIG)
        self.assertEqual(config.scan.cycles, 100)
        self.assertEqual(config.scan.points_per_branch, 40)

    def test_rejects_invalid_counts_and_voltage_squared_limits(self):
        cases = (
            ("cycles", 0),
            ("cycles", True),
            ("points_per_branch", 1),
            ("points_per_branch", True),
            ("v2_min", -0.1),
            ("v2_max", 196.1),
            ("v2_min", float("nan")),
            ("settle_time_s", -0.1),
        )
        for field, value in cases:
            with self.subTest(field=field, value=value):
                payload = valid_payload()
                payload["scan"][field] = value
                with self.assertRaises(ValueError):
                    RepeatRunConfig.from_mapping(payload)
        payload = valid_payload()
        payload["scan"].update(v2_min=90.0, v2_max=40.0)
        with self.assertRaises(ValueError):
            RepeatRunConfig.from_mapping(payload)

    def test_rejects_unsafe_gain_or_ramp_values(self):
        mutations = (
            lambda p: p["gain"].update(temperature_c=14.9),
            lambda p: p["gain"].update(current_ma=200.1),
            lambda p: p.update(current_ramp_step_ma=1.1),
            lambda p: p.update(current_ramp_interval_s=0.049),
        )
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                payload = copy.deepcopy(valid_payload())
                mutate(payload)
                with self.assertRaises(ValueError):
                    RepeatRunConfig.from_mapping(payload)


class RepeatScheduleTests(unittest.TestCase):
    def test_approved_schedule_has_exact_branch_counts_and_endpoints(self):
        steps = build_repeat_schedule(RepeatScanConfig())
        self.assertEqual(len(steps), 8000)
        self.assertEqual([step.direction for step in steps[:40]], ["forward"] * 40)
        self.assertEqual([step.direction for step in steps[40:80]], ["reverse"] * 40)
        self.assertEqual(steps[0].voltage_squared_v2, 40.0)
        self.assertEqual(steps[39].voltage_squared_v2, 90.0)
        self.assertEqual(steps[40].voltage_squared_v2, 90.0)
        self.assertEqual(steps[79].voltage_squared_v2, 40.0)
        self.assertEqual(steps[0].cycle_index, 0)
        self.assertEqual(steps[-1].cycle_index, 99)
        self.assertEqual([step.sequence_index for step in steps], list(range(8000)))

    def test_v2_grid_is_uniform_and_voltage_is_square_root(self):
        scan = RepeatScanConfig(cycles=1)
        grid = build_v2_grid(scan)
        np.testing.assert_allclose(grid, np.linspace(40.0, 90.0, 40))
        steps = build_repeat_schedule(scan)
        self.assertAlmostEqual(steps[0].voltage_v, math.sqrt(40.0))
        self.assertAlmostEqual(steps[39].voltage_v, math.sqrt(90.0))

    def test_pair_indices_align_reverse_to_forward_grid(self):
        scan = RepeatScanConfig(v2_min=4.0, v2_max=9.0, points_per_branch=3, cycles=2)
        first = cycle_steps(build_repeat_schedule(scan), 0)
        self.assertEqual([step.pair_index for step in first], [0, 1, 2, 2, 1, 0])
        self.assertEqual([step.branch_index for step in first], [0, 1, 2, 0, 1, 2])
        self.assertEqual(len(cycle_steps(build_repeat_schedule(scan), 1)), 6)


if __name__ == "__main__":
    unittest.main()
