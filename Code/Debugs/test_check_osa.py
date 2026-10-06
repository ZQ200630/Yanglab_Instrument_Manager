"""Diagnostic routing exercises the real OSA driver with an injected VISA wire."""
import io
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from contextlib import redirect_stdout
from unittest.mock import Mock, patch

import numpy as np

from Code.Debugs import check_osa
from Code.Debugs.test_pm400 import FakeResourceManager, FakeVisaResource
from Code.Debugs.test_osa_read import TraceWire, SweepBoundary
from Code.Utils.osa import AQ6370
from Code.Utils.common import InstrumentConnectionError


class OSADiagnosticTests(unittest.TestCase):
    def setUp(self):
        # A CLI test must not install root INFO logging for unrelated suites.
        logging_patch = patch.object(check_osa.logging, 'basicConfig')
        logging_patch.start()
        self.addCleanup(logging_patch.stop)

    def test_identity_route_only_queries_identity_and_confirms_release(self):
        resource = FakeVisaResource({'*IDN?': 'YOKOGAWA,AQ6370E,UNITTEST,1.0'})
        manager = FakeResourceManager(resource)
        def forbidden(_):
            raise AssertionError('Identity check must not construct a sweep instrument')
        driver = AQ6370('GPIB0::29::INSTR', resource_manager_factory=lambda: manager,
                        instrument_factory=forbidden)
        output = io.StringIO()
        try:
            with patch('sys.argv', ['check_osa', '--identity', '--resource', 'GPIB0::29::INSTR']), \
                    patch.object(check_osa, 'AQ6370', return_value=driver), redirect_stdout(output):
                try:
                    result = check_osa.main()
                except SystemExit as error:
                    self.fail(f'Identity-only routing missing: exit {error.code}')
            self.assertEqual(result, 0)
            record = json.loads(output.getvalue())
            self.assertEqual(record['identity']['serial'], 'UNITTEST')
            self.assertTrue(record['release_confirmed'])
            self.assertFalse(driver.has_resource_responsibility)
            self.assertTrue(resource.closed)
            self.assertEqual(resource.transactions, ['*IDN?'])
            self.assertEqual(resource.mutating_commands, [])
        finally:
            driver.close()

    def test_identity_failure_still_releases_without_scan_or_abort(self):
        resource = FakeVisaResource({'*IDN?': 'UNEXPECTED,MODEL,SN,1'})
        manager = FakeResourceManager(resource)
        driver = AQ6370('GPIB0::29::INSTR', resource_manager_factory=lambda: manager)
        try:
            with patch('sys.argv', ['check_osa', '--identity']), \
                    patch.object(check_osa, 'AQ6370', return_value=driver):
                with self.assertRaises(InstrumentConnectionError):
                    check_osa.main()
            self.assertFalse(driver.has_resource_responsibility)
            self.assertTrue(resource.closed)
            self.assertEqual(resource.mutating_commands, [])
        finally:
            driver.close()

    def test_default_is_identity_only_and_never_constructs_sweep(self):
        resource = FakeVisaResource({'*IDN?': 'YOKOGAWA,AQ6370E,UNITTEST,1.0'})
        manager = FakeResourceManager(resource)
        driver = AQ6370('GPIB0::29::INSTR', resource_manager_factory=lambda: manager,
                       instrument_factory=lambda _: (_ for _ in ()).throw(AssertionError('default started sweep path')))
        with patch('sys.argv', ['check_osa']), patch.object(check_osa, 'AQ6370', return_value=driver), redirect_stdout(io.StringIO()):
            try:
                self.assertEqual(check_osa.main(), 0)
            except (AssertionError, InstrumentConnectionError) as error:
                self.fail(str(error))
        self.assertEqual(resource.transactions, ['*IDN?'])
        self.assertFalse(driver.has_resource_responsibility)

    def test_existing_trace_saves_native_data_and_truthful_release_without_acquire(self):
        wire, manager = TraceWire(), Mock()
        manager.open_resource.return_value = wire
        driver = AQ6370('GPIB0::29::INSTR', resource_manager_factory=lambda: manager,
                       instrument_factory=SweepBoundary)
        with TemporaryDirectory() as root:
            out = Path(root) / 'trace'
            with patch('sys.argv', ['check_osa', '--read-trace', 'A', '--out', str(out)]), \
                    patch.object(check_osa, 'AQ6370', return_value=driver), \
                    patch.object(driver, 'acquire', side_effect=AssertionError('read started sweep')), redirect_stdout(io.StringIO()):
                try:
                    result = check_osa.main()
                except SystemExit as error:
                    self.fail(f'Existing-trace route missing: {error.code}')
            self.assertEqual(result, 0)
            with np.load(out / 'trace.npz', allow_pickle=False) as arrays:
                np.testing.assert_array_equal(arrays['native_values'], [-40., -30.])
                np.testing.assert_array_equal(arrays['wavelength_nm'], [953.67431640625, 1430.511474609375])
            metadata = json.loads((out / 'trace.json').read_text(encoding='utf-8'))
            self.assertEqual(metadata['native_unit'], 'dBm')
            self.assertEqual(metadata['context_before']['sweep_mode'], 2)
            self.assertEqual(metadata['sample_count'], 2)
            self.assertEqual(metadata['consistency'], 'unproven')
            self.assertTrue(metadata['release_confirmed'])
            self.assertEqual(metadata['physical_state'], 'not_measured')
            self.assertNotIn('measurement_time', metadata)
        self.assertEqual(wire.actions, [])
        self.assertTrue(wire.closed)

    def test_existing_output_is_rejected_before_any_instrument_open(self):
        with TemporaryDirectory() as root:
            with patch('sys.argv', ['check_osa', '--read-trace', 'A', '--out', root]), \
                    patch.object(check_osa, 'AQ6370') as factory:
                with self.assertRaises((FileExistsError, SystemExit)):
                    check_osa.main()
                factory.assert_not_called()

    def test_action_modes_are_explicit_and_mutually_exclusive(self):
        for arguments in (['--identity', '--sweep'], ['--read-trace', 'A', '--sweep'],
                          ['--read-trace', 'A'], ['--out', 'somewhere']):
            with self.subTest(arguments=arguments), patch('sys.argv', ['check_osa', *arguments]), \
                    patch.object(check_osa, 'AQ6370') as factory:
                with self.assertRaises(SystemExit):
                    check_osa.main()
                factory.assert_not_called()

    def test_failed_and_interrupted_reads_release_and_never_publish_data(self):
        for error in (KeyboardInterrupt('stop'), InstrumentConnectionError('wire failed')):
            with self.subTest(error=type(error).__name__), TemporaryDirectory() as root:
                class BrokenWire(TraceWire):
                    def read(self, session, count):
                        if self.pending.startswith(':TRACe:Y?'):
                            raise error
                        return super().read(session, count)
                wire, manager = BrokenWire(), Mock()
                manager.open_resource.return_value = wire
                driver = AQ6370('GPIB0::29::INSTR', resource_manager_factory=lambda: manager,
                               instrument_factory=SweepBoundary)
                out = Path(root) / 'failed'
                with patch('sys.argv', ['check_osa', '--read-trace', 'A', '--out', str(out)]), \
                        patch.object(check_osa, 'AQ6370', return_value=driver), redirect_stdout(io.StringIO()):
                    try:
                        with self.assertRaises(type(error)) as caught:
                            check_osa.main()
                    except SystemExit as missing:
                        self.fail(f'Read route missing: {missing.code}')
                self.assertIs(caught.exception, error)
                self.assertFalse((out / 'trace.npz').exists())
                metadata = json.loads((out / 'trace.json').read_text(encoding='utf-8'))
                self.assertEqual(metadata['status'], 'failed')
                self.assertTrue(metadata['release_confirmed'])
                self.assertTrue(wire.closed)
                self.assertEqual(wire.actions, [])


if __name__ == '__main__':
    unittest.main()
