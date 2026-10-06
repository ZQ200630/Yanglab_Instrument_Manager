"""Real-only experiment entry/storage gates; no devices or experiments run."""
import contextlib
import io
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from Code.Experiments.sil_hysteresis import run as hysteresis_cli
from Code.Experiments.sil_hysteresis.config import RunConfig, DeviceConfig, ScanConfig, OperationPoint
from Code.Experiments.sil_hysteresis.storage import RunStore
from Code.Experiments.sil_repeat import run as repeat_cli
from Code.Experiments.sil_repeat.config import load_repeat_config
from Code.Experiments.sil_repeat.storage import RepeatRunStore
from dataclasses import replace


def hysteresis_config(root):
    return RunConfig(devices=DeviceConfig(), scan=ScanConfig(points=2),
        operation_points=(OperationPoint(22., 60.),), output_root=Path(root))


def repeat_config(root):
    return replace(load_repeat_config(repeat_cli.DEFAULT_CONFIG), output_root=Path(root))


class RealExperimentTests(unittest.TestCase):
    def test_retired_instrument_generators_are_not_importable(self):
        for name in ("Code.Experiments.sil_hysteresis.simulate",
                     "Code.Experiments.sil_repeat.simulate"):
            with self.subTest(module=name):
                self.assertIsNone(importlib.util.find_spec(name))

    def test_obsolete_flags_fail_before_an_execution_path_exists(self):
        for cli in (hysteresis_cli, repeat_cli):
            for mode in ('similar', 'hysteretic'):
                with self.subTest(cli=cli.__name__, mode=mode), contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as rejected:
                        cli._build_parser().parse_args(['--simulate', mode])
                    self.assertEqual(rejected.exception.code, 2)

    def test_non_real_new_store_is_rejected_before_directory_creation(self):
        for store_type, config in ((RunStore, hysteresis_config), (RepeatRunStore, repeat_config)):
            for mode in ('similar', 'hysteretic'):
                with self.subTest(store=store_type.__name__, mode=mode), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp) / 'not-created'
                    with self.assertRaisesRegex(ValueError, 'real|historical|Historical'):
                        store_type.create(config(root), run_mode=mode)
                    self.assertFalse(root.exists())

    def test_historical_manifest_is_readable_but_cannot_start_another_attempt(self):
        for store_type, config, start in (
                (RunStore, hysteresis_config, 'start_attempt'),
                (RepeatRunStore, repeat_config, 'start_cycle_attempt')):
            for mode in ('similar', 'hysteretic'):
                with self.subTest(store=store_type.__name__, mode=mode), tempfile.TemporaryDirectory() as tmp:
                    store = store_type.create(config(tmp), run_mode='real')
                    path = store.path / 'run.json'
                    record = json.loads(path.read_text())
                    # A literal archived manifest fixture, not generated instrument data.
                    record.update(run_mode=mode, provenance={
                        'label': 'SIMULATED', 'run_mode': mode, 'simulated': True})
                    path.write_text(json.dumps(record))
                    original = path.read_bytes()
                    opened = store_type.open_existing(store.path)
                    self.assertEqual(opened.run_mode, mode)
                    before = {p.relative_to(store.path) for p in store.path.rglob('*')}
                    with self.assertRaisesRegex(ValueError, 'real|historical|Historical'):
                        getattr(opened, start)(0)
                    self.assertEqual(path.read_bytes(), original)
                    self.assertEqual({p.relative_to(store.path) for p in store.path.rglob('*')}, before)

    def test_historical_cli_resume_never_prompts_or_constructs_hardware(self):
        for cli, store_type, config in (
                (hysteresis_cli, RunStore, hysteresis_config),
                (repeat_cli, RepeatRunStore, repeat_config)):
            with self.subTest(cli=cli.__name__), tempfile.TemporaryDirectory() as tmp:
                store = store_type.create(config(tmp), run_mode='real')
                manifest = store.path / 'run.json'
                record = json.loads(manifest.read_text())
                record.update(run_mode='similar', provenance={
                    'label': 'SIMULATED', 'run_mode': 'similar', 'simulated': True})
                manifest.write_text(json.dumps(record))
                def forbidden(*args, **kwargs):
                    self.fail('Historical data must not authorize hardware or prompt for RUN')
                with (
                    patch.object(cli, '_default_driver_factories', forbidden),
                    patch.object(repeat_cli, 'REAL_OUTPUT_ROOT', Path(tmp)),
                ):
                    with self.assertRaisesRegex(ValueError, 'mode mismatch|historical|Historical'):
                        cli.main(['--resume', str(store.path), '--plan'],
                                 input_fn=forbidden, output=io.StringIO())


if __name__ == '__main__':
    unittest.main()
