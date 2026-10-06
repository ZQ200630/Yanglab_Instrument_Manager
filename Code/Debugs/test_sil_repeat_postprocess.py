from __future__ import annotations

import csv
import tempfile
import unittest
from pathlib import Path

from Code.Debugs.test_sil_repeat_analysis_figure import (
    config_for,
    gain_status,
    spectrum_for,
    voltage_status,
)
from Code.Experiments.sil_repeat.postprocess import (
    InlinePostprocessor,
    PostprocessRequest,
    ProcessPostprocessor,
)
from Code.Experiments.sil_repeat.analysis import CycleSummaryRow, write_repeat_summary
from Code.Experiments.sil_repeat.scan import cycle_steps
from Code.Experiments.sil_repeat.storage import RepeatRunStore


class InlinePostprocessorTests(unittest.TestCase):
    def test_processes_requests_in_submission_order(self):
        calls = []
        worker = InlinePostprocessor(lambda request: calls.append(request.cycle_index))
        worker.submit(PostprocessRequest(0, Path("cycle_000/attempt_001")))
        worker.submit(PostprocessRequest(1, Path("cycle_001/attempt_001")))
        worker.close_and_drain()
        self.assertEqual(calls, [0, 1])

    def test_reports_cycle_identity_with_handler_failure(self):
        def fail(_request):
            raise RuntimeError("plot failed")

        worker = InlinePostprocessor(fail)
        worker.submit(PostprocessRequest(7, Path("cycle_007/attempt_001")))
        with self.assertRaisesRegex(RuntimeError, "cycle 7.*plot failed"):
            worker.check()
        with self.assertRaisesRegex(RuntimeError, "cycle 7.*plot failed"):
            worker.close_and_drain()

    def test_final_drain_lists_every_failed_cycle(self):
        worker = InlinePostprocessor(
            lambda request: (_ for _ in ()).throw(
                RuntimeError(f"plot failed {request.cycle_index}")
            )
        )
        worker.submit(PostprocessRequest(2, Path("cycle_002/attempt_001")))
        worker.submit(PostprocessRequest(5, Path("cycle_005/attempt_001")))
        with self.assertRaisesRegex(RuntimeError, "cycle 2.*cycle 5"):
            worker.close_and_drain()


class ProcessPostprocessorTests(unittest.TestCase):
    def test_spawn_worker_processes_two_small_cycles_and_aggregates(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=2, points=2)
            run = RepeatRunStore.create(config, run_mode="real")
            paths = []
            for cycle_index in range(2):
                attempt = run.start_cycle_attempt(cycle_index)
                for step in cycle_steps(run.schedule, cycle_index):
                    attempt.save_acquisition(
                        step=step,
                        spectrum=spectrum_for(step.direction, step.pair_index),
                        voltage_status=voltage_status(step.voltage_v),
                        gain_status=gain_status(),
                        osa_duration_s=1.25,
                    )
                attempt.mark_acquisition_complete()
                paths.append(attempt.path)
            worker = ProcessPostprocessor(config, run.path, display_latest=False, join_timeout_s=20.0)
            write_repeat_summary(
                run.path,
                (
                    CycleSummaryRow(
                        99,
                        1.0,
                        0.1,
                        40.0,
                        1.0,
                        "stale-start",
                        "stale-end",
                        "OBSERVATION_REQUIRED",
                        "real",
                    ),
                ),
            )
            for cycle_index, path in enumerate(paths):
                worker.submit(
                    PostprocessRequest(
                        cycle_index,
                        path,
                        reset_aggregate=cycle_index == 0,
                    )
                )
            worker.close_and_drain()
            self.assertEqual(worker.process_exitcode, 0)
            self.assertTrue(all(path.joinpath("summary.json").is_file() for path in paths))
            self.assertTrue(all(path.joinpath("comparison.png").is_file() for path in paths))
            with run.path.joinpath("repeat_summary.csv").open(newline="", encoding="utf-8") as handle:
                rows = list(csv.DictReader(handle))
            self.assertEqual([int(row["cycle_index"]) for row in rows], [0, 1])
            for suffix in (".svg", ".pdf", ".png"):
                self.assertTrue(run.path.joinpath("repeat_summary" + suffix).is_file())


if __name__ == "__main__":
    unittest.main()
