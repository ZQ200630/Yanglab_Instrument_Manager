"""Native staging through the actual domain scheduler and AQ driver.

Only the finite VISA byte transport is replaced; no backend or GUI samples.
"""
import dataclasses
import hashlib
import json
import os
from pathlib import Path
import struct
import tempfile
import threading
import unittest
import uuid
from unittest.mock import patch

from App.tests.domain_fixture import config
from App.tests.driver_fixture import WireOSA
from App.worker.captures import CaptureSpool
from App.worker.contracts_v3 import ContextV3, RequestV3, encode_v3, parse_v3
from App.worker.controller import DomainController

SESSION, NONCE = 'a' * 32, 'b' * 32


class CaptureRouteTests(unittest.TestCase):
    def test_legacy_codec_does_not_gain_an_unbounded_native_trace_route(self):
        from App.worker.controller import ConsoleController
        from App.worker.contracts import Request
        c = ConsoleController(factories={'osa': WireOSA}, port_enumerator=lambda: ())
        self.addCleanup(c.close)
        c.handle('connect', {'role': 'osa', 'resource': 'GPIB0::29::INSTR', 'acknowledge_lifecycle': True})
        out = c.submit(Request('read', 'action', {'role': 'osa', 'name': 'read_trace'}, c.context('osa'))).result(3)
        self.assertEqual(out.phase, 'rejected_before_call', out)
        self.assertEqual(c._devices['osa'].test_wire.visalib.counts, {})

    def make(self, *, points=2, watts=False, spool=True):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name) / NONCE
        root.mkdir()
        staging = CaptureSpool(root, NONCE)

        class ScriptOSA(WireOSA):
            def __init__(self, **kwargs):
                super().__init__(**kwargs)
                wire = self.test_wire.visalib
                import numpy as np
                wire.x = np.linspace(1550e-9, 1551e-9, points)
                wire.y = np.full(points, .001 if watts else -30.)
                wire.meta[':TRACe:DATA:SNUMber? TRA'] = str(points)
                if watts:
                    wire.y[0] = 0.
                    wire.meta[':DISPlay:TRACe:Y1:SCALe:SPACing?'] = '1'
                    wire.meta[':DISPlay:TRACe:Y1:SCALe:UNIT?'] = '1'

        options = dict(session_id=SESSION, factories={'osa': ScriptOSA}, port_enumerator=lambda: ())
        if spool:
            options.update(capture_spool=staging, ownership_nonce=NONCE)
        controller = DomainController(**options)
        self.addCleanup(controller.close)
        cfg = config(1, 'osa')
        controller.configure(cfg)
        result = controller.submit(RequestV3('open', 'connect', {'acknowledge_lifecycle': True}, controller.context(cfg.domain))).result(3)
        self.assertEqual(result.phase, 'completed', result)
        return controller, cfg.domain, staging, root

    def action(self, controller, ref, name='read_trace', args=None, id='capture'):
        return controller.submit(RequestV3(id, 'action', {'name': name, 'args': args or {}}, controller.context(ref))).result(8)

    def private(self, controller, method, params, *, session=SESSION):
        return controller.submit(RequestV3(uuid.uuid4().hex, method, params, ContextV3(session, None, None, 0))).result(3)

    def test_maximum_trace_terminal_is_small_and_staging_preserves_all_samples(self):
        # Catches converting the native capture to oversized array JSON.
        c, ref, staging, _ = self.make(points=200001)
        out = self.action(c, ref)
        self.assertEqual(out.phase, 'completed', out)
        self.assertLess(len(encode_v3('capture', out).encode()), 65536)
        descriptor = out.result['result']
        self.assertEqual((descriptor['point_count'], descriptor['byte_count']), (200001, 3200016))
        chunk = self.private(c, 'read_capture_chunk', dict(ownership_nonce=NONCE, capture_id=descriptor['capture_id'], offset=3200000, length=16))
        self.assertEqual(chunk.phase, 'completed', chunk)
        self.assertEqual(bytes.fromhex(chunk.result['data_hex']), struct.pack('<dd', 1551., -30.))
        self.assertLess(len(encode_v3('chunk', chunk).encode()), 65536)
        first = self.private(c, 'read_capture_chunk', dict(ownership_nonce=NONCE, capture_id=descriptor['capture_id'], offset=0, length=16384))
        self.assertEqual(first.phase, 'completed', first)
        self.assertEqual(len(first.result['data_hex']), 32768)
        self.assertLess(len(encode_v3('first', first).encode()), 65536)
        self.assertEqual(c.device(ref).test_wire.visalib.actions, [])

    def test_native_zero_watt_sweep_has_original_context_and_one_transfer(self):
        c, ref, staging, _ = self.make(watts=True)
        out = self.action(c, ref, 'acquire')
        self.assertEqual(out.phase, 'completed', out)
        descriptor = out.result['result']
        self.assertEqual(descriptor['metadata']['native_unit'], 'W')
        self.assertEqual(descriptor['metadata']['context_before']['sweep_mode'], 1)
        self.assertEqual(descriptor['metadata']['consistency'], 'unproven')
        payload = bytes.fromhex(staging.read(descriptor['capture_id'], 0, 32)['data_hex'])
        self.assertEqual(payload, struct.pack('<dddd', 1550., 0., 1551., .001))
        self.assertEqual(descriptor['sha256'], hashlib.sha256(payload).hexdigest())
        wire = c.device(ref).test_wire.visalib
        self.assertEqual(wire.actions, ['INIT'])
        self.assertEqual(wire.counts[':TRACe:X? TRA,1,2'], 1)
        self.assertEqual(wire.counts[':TRACe:Y? TRA,1,2'], 1)

    def test_unconfigured_spool_rejects_before_acquisition(self):
        c, ref, _, _ = self.make(spool=False)
        out = self.action(c, ref, 'acquire')
        self.assertEqual(out.phase, 'rejected_before_call', out)
        self.assertEqual(c.device(ref).test_wire.visalib.actions, [])

    def test_stage_disk_failure_is_readback_failure_not_a_repeated_device_call(self):
        c, ref, _, root = self.make()
        with patch('App.worker.captures.os.write', side_effect=OSError('disk full')):
            out = self.action(c, ref)
        self.assertEqual(out.phase, 'completed_readback_failed', out)
        self.assertIsNone(out.result)
        self.assertNotIn('outcome is unknown', out.error['message'])
        self.assertEqual(c.device(ref).test_wire.visalib.actions, [])
        self.assertEqual(c.device(ref).test_wire.visalib.counts[':TRACe:Y? TRA,1,2'], 1)
        self.assertEqual(len(list(root.glob('*.bin'))), 1)

    def test_status_and_stop_admission_are_responsive_during_blocked_disk_write(self):
        c, ref, _, root = self.make()
        entered, release = threading.Event(), threading.Event()
        original = os.write
        def blocked(handle, content):
            entered.set()
            if not release.wait(3):
                raise OSError('test write gate expired')
            return original(handle, content)
        with patch('App.worker.captures.os.write', side_effect=blocked):
            pending = c.submit(RequestV3('capture', 'action', {'name': 'read_trace', 'args': {}}, c.context(ref)))
            try:
                self.assertTrue(entered.wait(2), 'staging was never entered')
                status = c.submit(RequestV3('status', 'status', {}, ContextV3(SESSION, None, None, 0))).result(.5)
                self.assertEqual(status.phase, 'completed', status)
                before = c.context(ref)
                stop = c.submit(RequestV3('stop', 'disconnect', {}, before))
                self.assertNotEqual(c.context(ref), before)
                self.assertFalse(stop.done())  # Release evidence still waits for the active callback.
                self.assertEqual(c.cached_status()['domains']['device:' + ref.id]['state'], 'CLOSING')
            finally:
                release.set()
            out = pending.result(3)
            self.assertEqual(stop.result(3).phase, 'completed')
        self.assertNotEqual(out.phase, 'completed', out)
        self.assertIsNone(out.result)
        self.assertEqual(list(root.glob('*.json')), [])

    def test_interrupted_staging_reports_completed_readback_failure(self):
        c, ref, _, _ = self.make()
        with patch('App.worker.captures.os.write', side_effect=KeyboardInterrupt('interrupted file write')):
            out = self.action(c, ref)
        self.assertEqual(out.phase, 'completed_readback_failed', out)
        self.assertIsNone(out.result)
        self.assertIn('KeyboardInterrupt', out.error['message'])
        self.assertEqual(c.device(ref).test_wire.visalib.actions, [])

    def test_other_domain_safety_completes_while_osa_disk_is_blocked(self):
        from App.tests.driver_fixture import WireVoltage
        c, ref, _, _ = self.make()
        c._factories['voltage'] = WireVoltage
        cfg = config(2, 'voltage')
        c.configure(cfg)
        opened = c.submit(RequestV3('open-voltage', 'connect', {'acknowledge_lifecycle': True}, c.context(cfg.domain))).result(3)
        self.assertEqual(opened.phase, 'completed', opened)
        entered, release = threading.Event(), threading.Event()
        original = os.write
        def blocked(handle, content):
            entered.set()
            if not release.wait(3):
                raise OSError('test write gate expired')
            return original(handle, content)
        with patch('App.worker.captures.os.write', side_effect=blocked):
            pending = c.submit(RequestV3('capture', 'action', {'name': 'read_trace', 'args': {}}, c.context(ref)))
            try:
                self.assertTrue(entered.wait(2))
                zero = c.submit(RequestV3('zero', 'action', {'name': 'zero', 'args': {}}, c.context(cfg.domain))).result(.5)
                self.assertEqual(zero.phase, 'completed', zero)
                self.assertEqual(zero.result['status']['requested_voltage_v'], [0.] * 8)
            finally:
                release.set()
            self.assertEqual(pending.result(3).phase, 'completed')

    def test_bootstrap_installs_only_its_nonce_bound_spool_after_activation(self):
        from App.worker.main import _bootstrap_v3
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name) / NONCE
        root.mkdir()
        bootstrap = _bootstrap_v3(ownership_nonce=NONCE, capture_spool=root)
        context = bootstrap.global_context()
        params = dict(ownership_nonce=NONCE, capture_id='e' * 32, offset=0, length=16)
        denied = bootstrap.submit(RequestV3('before', 'read_capture_chunk', params, context)).result(1)
        self.assertEqual(denied.phase, 'rejected_before_call')
        activated = bootstrap.submit(RequestV3('activate', 'activate', {'ownership_nonce': NONCE}, context)).result(1)
        self.assertEqual(activated.phase, 'completed', activated)
        self.addCleanup(bootstrap.runtime.close)
        self.assertEqual(bootstrap.runtime._capture_spool.root, root.absolute())
        self.assertEqual(bootstrap.runtime.cached_status()['devices'], {})

    def test_bootstrap_rejects_wrong_spool_without_consuming_activation(self):
        from App.worker.main import _bootstrap_v3
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        wrong = Path(temporary.name) / ('c' * 32)
        wrong.mkdir()
        bootstrap = _bootstrap_v3(ownership_nonce=NONCE, capture_spool=wrong)
        out = bootstrap.submit(RequestV3('activate', 'activate', {'ownership_nonce': NONCE}, bootstrap.global_context())).result(1)
        self.assertEqual(out.phase, 'rejected_before_call', out)
        self.assertFalse(bootstrap.activated)
        self.assertIsNone(bootstrap.runtime)

    def test_private_chunks_require_current_owner_and_matching_complete_ack(self):
        c, ref, _, root = self.make()
        out = self.action(c, ref)
        self.assertEqual(out.phase, 'completed', out)
        descriptor = out.result['result']
        params = dict(ownership_nonce='c' * 32, capture_id=descriptor['capture_id'], offset=0, length=32)
        denied = self.private(c, 'read_capture_chunk', params)
        self.assertEqual(denied.phase, 'rejected_before_call', denied)
        params['ownership_nonce'] = NONCE
        stale = self.private(c, 'read_capture_chunk', params, session='d' * 32)
        self.assertEqual(stale.phase, 'rejected_before_call', stale)
        ack = self.private(c, 'ack_capture', dict(ownership_nonce=NONCE, capture_id=descriptor['capture_id'], sha256=descriptor['sha256']))
        self.assertEqual(ack.phase, 'completed', ack)
        self.assertEqual(list(root.iterdir()), [])

    def test_private_wire_shapes_are_global_bounded_and_not_arbitrary_paths(self):
        frame = dict(v=3, id='chunk', method='read_capture_chunk', params=dict(ownership_nonce=NONCE, capture_id='e' * 32, offset=0, length=16384), context=dataclasses.asdict(ContextV3(SESSION, None, None, 0)))
        self.assertEqual(parse_v3(json.dumps(frame)).method, 'read_capture_chunk')
        for field, value in [('length', 16385), ('length', True), ('offset', -1), ('capture_id', '../outside'), ('path', 'C:/outside')]:
            with self.subTest(field=field, value=value):
                bad = json.loads(json.dumps(frame))
                bad['params'][field] = value
                self.assertRaises(ValueError, parse_v3, json.dumps(bad))

    def test_typed_trace_action_rejects_transport_path_and_context_overrides(self):
        c, ref, _, _ = self.make()
        for index, args in enumerate(({'path': 'C:/outside'}, {'resource': 'GPIB0::4::INSTR'}, {'trace': 'H'}, {'ownership_nonce': NONCE})):
            with self.subTest(args=args):
                out = self.action(c, ref, args=args, id='bad-' + str(index))
                self.assertEqual(out.phase, 'rejected_before_call', out)
        self.assertEqual(c.device(ref).test_wire.visalib.actions, [])


if __name__ == '__main__':
    unittest.main()
