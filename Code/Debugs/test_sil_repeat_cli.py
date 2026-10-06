from __future__ import annotations

import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

from Code.Debugs.test_sil_repeat_config_scan import valid_payload
from Code.Debugs.test_sil_repeat_experiment import fake_bundle
from Code.Experiments.sil_repeat.postprocess import InlinePostprocessor
from Code.Experiments.sil_repeat.run import DriverFactories, main


def write_config(root: Path, *, cycles: int = 2, points: int = 2) -> Path:
    payload = valid_payload()
    payload["scan"]["cycles"] = cycles
    payload["scan"]["points_per_branch"] = points
    payload["display_latest"] = False
    payload["output_root"] = str(root / "results")
    path = root / "repeat.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def recording_factories(calls):
    return DriverFactories(
        osa=lambda **_kwargs: calls.append("osa"),
        voltage=lambda **_kwargs: calls.append("voltage"),
        gain=lambda **_kwargs: calls.append("gain"),
    )


class RepeatCliTests(unittest.TestCase):
    def test_approved_plan_reports_exact_run_without_constructing_factories(self):
        output = io.StringIO()
        calls = []
        code = main(
            ["--config", "Code/Experiments/sil_repeat/repeat_24C_100mA.json", "--plan", "--wait-for-coupling"],
            output=output,
            factories=recording_factories(calls),
        )
        text = output.getvalue()
        self.assertEqual(code, 0)
        self.assertEqual(calls, [])
        for expected in (
            "24.0 degC",
            "100.0 mA",
            "V^2 range=40.0-90.0",
            "voltage=6.3246-9.4868 V",
            "Points per branch: 40",
            "Cycles: 100",
            "Total acquisitions: 8000",
            "Result\\sil_repeat",
            "Coupling gate: enabled",
        ):
            self.assertIn(expected, text)

    def test_wrong_case_run_cancels_before_store_or_driver_construction(self):
        with tempfile.TemporaryDirectory() as root:
            config_path = write_config(Path(root))
            output = io.StringIO()
            calls = []
            with patch(
                "Code.Experiments.sil_repeat.run.REAL_OUTPUT_ROOT",
                Path(root, "results"),
            ):
                code = main(
                    ["--config", str(config_path)],
                    input_fn=lambda _prompt: "run",
                    output=output,
                    factories=recording_factories(calls),
                )
            self.assertEqual(code, 0)
            self.assertEqual(calls, [])
            self.assertFalse(Path(root, "results").exists())
            self.assertIn("Cancelled", output.getvalue())

    def test_real_run_rejects_output_outside_canonical_result_root(self):
        with tempfile.TemporaryDirectory() as root:
            root_path = Path(root)
            config_path = write_config(root_path)
            allowed = root_path / "allowed" / "Result" / "sil_repeat"
            with patch(
                "Code.Experiments.sil_repeat.run.REAL_OUTPUT_ROOT", allowed, create=True
            ):
                with self.assertRaisesRegex(ValueError, "real output_root"):
                    main(
                        ["--config", str(config_path), "--plan"],
                        output=io.StringIO(),
                        factories=recording_factories([]),
                    )

    def test_real_run_always_requires_exact_coupled_before_voltage_scan(self):
        with tempfile.TemporaryDirectory() as root:
            config_path = write_config(Path(root), cycles=1, points=2)
            events = []
            bundle = fake_bundle(events)
            factories = DriverFactories(
                osa=lambda **_kwargs: bundle.osa,
                voltage=lambda **_kwargs: bundle.voltage,
                gain=lambda **_kwargs: bundle.gain,
            )
            answers = iter(("RUN", "coupled"))
            prompts = []

            def answer(prompt):
                prompts.append(prompt)
                return next(answers)

            with patch(
                "Code.Experiments.sil_repeat.run.ProcessPostprocessor",
                return_value=InlinePostprocessor(lambda _request: None),
            ), patch(
                "Code.Experiments.sil_repeat.run.REAL_OUTPUT_ROOT",
                Path(root, "results"),
            ):
                with self.assertRaisesRegex(RuntimeError, "coupling gate was not confirmed"):
                    main(
                        ["--config", str(config_path)],
                        input_fn=answer,
                        output=io.StringIO(),
                        factories=factories,
                    )
            self.assertFalse(any(event.startswith("voltage.set.") for event in events))
            self.assertIn("begin 1 cycle;", "".join(prompts))

    def test_two_cycle_real_path_fixed_inputs_create_complete_artifacts(self):
        with tempfile.TemporaryDirectory() as root:
            root_path = Path(root)
            config_path = write_config(root_path)
            calls = []
            output = io.StringIO()
            bundle = fake_bundle([])
            factories = DriverFactories(osa=lambda **_: bundle.osa,
                voltage=lambda **_: bundle.voltage, gain=lambda **_: bundle.gain)
            answers = iter(("RUN", "COUPLED"))
            with patch("Code.Experiments.sil_repeat.run.REAL_OUTPUT_ROOT", root_path / "results"):
                code = main(["--config", str(config_path), "--no-display"],
                    input_fn=lambda _: next(answers), output=output, factories=factories)
            self.assertEqual(code, 0)
            self.assertEqual(calls, [])
            runs = tuple(root_path.joinpath("results").glob("run_*"))
            self.assertEqual(len(runs), 1)
            run = runs[0]
            self.assertFalse((run / "SIMULATED").exists())
            self.assertEqual(len(tuple(run.glob("cycle_*/attempt_*/*.npz"))), 0)
            self.assertEqual(len(tuple(run.glob("cycle_*/attempt_*/*/*.npz"))), 8)
            statuses = tuple(run.glob("cycle_*/attempt_*/status.json"))
            self.assertEqual(len(statuses), 2)
            self.assertEqual(
                [json.loads(path.read_text(encoding="utf-8"))["state"] for path in statuses],
                ["complete", "complete"],
            )
            self.assertEqual(len(tuple(run.glob("cycle_*/attempt_*/comparison.png"))), 2)
            self.assertTrue((run / "repeat_summary.csv").is_file())
            for suffix in (".svg", ".pdf", ".png"):
                self.assertTrue(run.joinpath("repeat_summary" + suffix).is_file())

    def test_obsolete_modes_fail_before_configuration_or_hardware(self):
        for mode in ("similar", "hysteretic"):
            with self.subTest(mode=mode), patch("sys.stderr", io.StringIO()):
                with self.assertRaises(SystemExit) as rejected:
                    main(["--config", "absent.json", "--simulate", mode],
                        input_fn=lambda _: self.fail("Obsolete mode prompted"),
                        factories=recording_factories([]), output=io.StringIO())
                self.assertEqual(rejected.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
