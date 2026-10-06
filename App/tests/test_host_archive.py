"""Persistent archive contracts over native pipes and literal VISA bytes.

These tests do not touch physical instruments, native GUI or a lab network.
"""
import hashlib
import json
from pathlib import Path
import time
from App.tests import test_local_host_process as f


class HostArchiveTests(f.LocalHostProcessTests):
    def restart_with_delivery_fault(self, fault):
        self.assertTrue(self.client.call('stop', {'confirm': True})['ok'])
        self.child.wait(8)
        for client in self.clients:
            client.close()
        self.worker_root = f.stage_worker(Path(self.directory.name) / 'fault-worker-root', capture_fault=fault)
        self.child = self.launch()
        output = f.queue.Queue()
        f.threading.Thread(target=lambda: output.put(self.child.stdout.readline()), daemon=True).start()
        self.endpoint = json.loads(output.get(timeout=12))['endpoint']
        self.client = self.connect()
        self.assertTrue(self.client.call('ping')['ok'])

    def setup_osa(self):
        draft = self.client.call('create_draft', dict(model_id='aq6370', profile_id='gpib-visa',
            params={'resource': 'GPIB0::4::INSTR'}, name='OSA', expected_rev=0))['result']
        domain = dict(kind='device', id=draft['device_id'])
        lease = self.client.call('acquire_control', dict(domain=domain))['result']
        consent = dict(accepted=True, mode='real', config_digest=draft['config_digest'],
            open_effects=[], supervised=False, retain_session=False)
        proof = self.client.call('test_connection', dict(draft_id=domain['id'], expected_rev=1,
            consent=consent, lease_token=lease['token'], control_epoch=lease['control_epoch'],
            request_id='verify', sequence=1))['result']
        self.assertTrue(self.client.call('save_device', dict(draft_id=domain['id'],
            proof_id=proof['proof_id'], expected_rev=1))['ok'])
        self.run_operation(domain, lease, 'open', 'connect', {'acknowledge_lifecycle': True}, 2)
        return domain, lease

    def run_operation(self, domain, lease, request_id, method, params, sequence, expected_phase='completed'):
        context = self.client.call('snapshot')['result']['domains']['device:' + domain['id']]['context']
        intent = dict(domain=domain, lease_token=lease['token'], control_epoch=lease['control_epoch'],
            config_rev=1, context=context, method=method, params=params, sequence=sequence, confirmation=None)
        prepared = self.client.call('prepare', dict(intent=intent))
        self.assertTrue(prepared['ok'], prepared)
        intent['confirmation'] = prepared['result']['token']
        admitted = self.client.call('execute', dict(request_id=request_id, intent=intent))
        self.assertTrue(admitted['ok'], admitted)
        deadline = time.monotonic() + 10
        while True:
            operation = self.client.call('operation', dict(request_id=request_id))['result']
            if operation['status'] != 'Accepted':
                self.assertEqual(operation['phase'], expected_phase, operation)
                return operation
            self.assertLess(time.monotonic(), deadline)
            time.sleep(.02)

    def test_trace_import_once_two_observers_and_restart_history_without_authority(self):
        domain, lease = self.setup_osa()
        before = self.client.call('ping')['result']['boot_id']
        operation = self.run_operation(domain, lease, 'trace', 'action', {'name': 'read_trace', 'args': {'trace': 'A'}}, 3)
        reference = operation['result']['result']['archive_ref']
        self.assertTrue(operation['result']['result']['staging_release_confirmed'])
        self.assertEqual(reference['id'], operation['operation_id'])
        self.assertEqual(reference['sample_count'], 2)
        other = self.connect()
        self.assertTrue(other.call('ping')['ok'])
        params = dict(domain=domain, offset=0, limit=4)
        first = self.client.call('list_archives', params)
        second = other.call('list_archives', params)
        self.assertTrue(first['ok'], first)
        self.assertEqual(first['result'], second['result'])
        self.assertEqual(len(first['result']['entries']), 1)
        access = dict(domain=domain, name=reference['name'], id=reference['id'])
        chunk = other.call('read_archive', dict(access, offset=0, length=reference['byte_count']))
        self.assertTrue(chunk['ok'], chunk)
        payload = bytes.fromhex(chunk['result']['data_hex'])
        self.assertEqual(hashlib.sha256(payload).hexdigest(), reference['sha256'])
        manifest = other.call('archive_manifest', access)
        self.assertTrue(manifest['ok'], manifest)
        self.assertEqual(manifest['result']['origin']['operation_id'], operation['operation_id'])
        self.assertEqual(manifest['result']['descriptor']['metadata'], reference['metadata'])
        for key in ('lease_token', 'boot_id', 'ownership_nonce', 'proof_id'):
            self.assertNotIn(key, json.dumps(manifest['result']))
        self.assertEqual(other.call('acquire_control', dict(domain=domain))['ok'], False)
        files = list((self.worker_root / 'Result' / 'osa').glob('*/manifest.json'))
        self.assertEqual(len(files), 1)
        original = files[0].read_bytes()
        self.assertTrue(self.client.call('stop', {'confirm': True})['ok'])
        self.child.wait(8)
        for client in self.clients:
            client.close()
        self.child = self.launch()
        self.endpoint = json.loads(self.child.stdout.readline())['endpoint']
        self.client = self.connect()
        self.assertTrue(self.client.call('ping')['ok'])
        self.assertNotEqual(self.client.call('ping')['result']['boot_id'], before)
        self.assertEqual(self.client.call('archive_manifest', access)['result'], manifest['result'])
        self.assertEqual(files[0].read_bytes(), original)
        snapshot = self.client.call('snapshot')['result']
        self.assertEqual(snapshot['control']['device:' + domain['id']]['state'], 'AVAILABLE')
        self.assertIsNone(snapshot['domains']['device:' + domain['id']]['context']['connection_id'])
        wrong = dict(access, domain=dict(kind='device', id='d' * 32))
        self.assertFalse(self.client.call('read_archive', dict(wrong, offset=0, length=16))['ok'])
        self.assertFalse(self.client.call('read_archive', dict(access, offset=0, length=16385))['ok'])
        self.assertFalse(self.client.call('archive_manifest', dict(access, path='../outside'))['ok'])
        self.assertFalse(self.client.call('ack_capture', {})['ok'])
        self.assertFalse(self.client.call('read_capture_chunk', {})['ok'])

    def test_failed_storage_reports_completed_read_without_output_unknown_or_retry(self):
        domain, lease = self.setup_osa()
        (self.worker_root / 'Result' / 'osa').write_bytes(b'pre-existing operator file')
        operation = self.run_operation(domain, lease, 'trace-full-disk', 'action',
            {'name': 'read_trace', 'args': {'trace': 'A'}}, 3, 'completed_readback_failed')
        result = operation['result']['result']
        self.assertEqual(result['storage_state'], 'incomplete')
        self.assertTrue(result['hardware_read_completed'])
        self.assertFalse(result['retry_hardware'])
        self.assertNotIn('output_state', result)
        self.assertNotIn('archive_ref', result)
        self.assertEqual((self.worker_root / 'Result' / 'osa').read_bytes(), b'pre-existing operator file')
        observer = self.connect()
        self.assertTrue(observer.call('ping')['ok'])
        self.assertTrue(observer.call('snapshot')['ok'])
        # This only closes the test's literal transport; no physical OSA opens.
        self.assertTrue(self.client.call('close_client')['result']['released'])

    def assert_partial_delivery_history(self, fault, phase, read_completed):
        self.restart_with_delivery_fault(fault)
        domain, lease = self.setup_osa()
        operation = self.run_operation(domain, lease, 'trace-failed-delivery', 'action',
            {'name': 'read_trace', 'args': {'trace': 'A'}}, 3, phase)
        page = self.client.call('list_archives', dict(domain=domain, offset=0, limit=4))
        self.assertTrue(page['ok'], page)
        self.assertEqual(len(page['result']['entries']), 1, page)
        entry = page['result']['entries'][0]
        self.assertEqual(entry['id'], operation['operation_id'])
        self.assertEqual(entry['state'], 'partial')
        self.assertIsNone(entry['reference'])
        directory = self.worker_root / 'Result' / 'osa' / entry['id']
        attempt = json.loads((directory / 'attempt.json').read_text())
        failure = json.loads((directory / 'failure.json').read_text())
        self.assertEqual(attempt['origin']['operation_id'], operation['operation_id'])
        self.assertEqual(attempt['origin']['domain'], domain)
        self.assertEqual(failure['phase'], phase)
        self.assertEqual(failure['hardware_read_completed'], read_completed)
        self.assertTrue(failure['primary_error'])
        if fault == 'staging':
            self.assertIn('finite staging write failure', failure['primary_error'])
        self.assertTrue(operation['result']['partial_history_confirmed'])
        self.assertFalse((directory / 'complete.json').exists())
        self.assertFalse((directory / 'native.bin').exists())
        self.assertNotIn('archive_ref', json.dumps(operation['result']))
        stopped = self.client.call('stop', {'confirm': True})
        if fault == 'worker_exit':
            # A deliberately crashed finite-transport worker cannot attest
            # release. Keep the production refusal; end only this owned test
            # Host to exercise restart, never an actual lab Host or resource.
            self.assertFalse(stopped['ok'], stopped)
            self.assertEqual(stopped['error']['code'], 'WorkerRetained')
            self.child.terminate()
        else:
            self.assertTrue(stopped['ok'], stopped)
        self.child.wait(8)
        for client in self.clients:
            client.close()
        if fault == 'worker_exit':
            # Do not bypass the retained startup/cleanup record merely to launch
            # another hardware-capable Host. ArchiveStore's independent restart
            # test covers data-only recovery; this process case checks durable
            # files and the truthful refusal of clean lifecycle evidence.
            self.assertEqual(json.loads((directory / 'attempt.json').read_text()), attempt)
            self.assertEqual(json.loads((directory / 'failure.json').read_text()), failure)
            return
        self.child = self.launch()
        output = f.queue.Queue()
        f.threading.Thread(target=lambda: output.put(self.child.stdout.readline()), daemon=True).start()
        self.endpoint = json.loads(output.get(timeout=12))['endpoint']
        self.client = self.connect()
        self.assertTrue(self.client.call('ping')['ok'])
        restored = self.client.call('list_archives', dict(domain=domain, offset=0, limit=4))
        self.assertEqual(restored['result'], page['result'])
        snapshot = self.client.call('snapshot')['result']
        self.assertIsNone(snapshot['domains']['device:' + domain['id']]['context']['connection_id'])

    def test_staging_failure_before_descriptor_is_durable_partial_after_restart(self):
        self.assert_partial_delivery_history('staging', 'completed_readback_failed', True)

    def test_worker_exit_before_descriptor_retains_partial_without_clean_stop_evidence(self):
        self.assert_partial_delivery_history('worker_exit', 'timed_out_unknown', False)

    def test_recording_name_is_validated_then_bound_to_original_operation_once(self):
        domain, lease = self.setup_osa()
        context = self.client.call('snapshot')['result']['domains']['device:' + domain['id']]['context']
        intent = dict(domain=domain, lease_token=lease['token'], control_epoch=lease['control_epoch'],
            config_rev=1, context=context, method='action',
            params={'name': 'read_trace', 'args': {'trace': 'A', 'archive_name': '../outside'}},
            sequence=3, confirmation=None)
        self.assertFalse(self.client.call('prepare', dict(intent=intent))['ok'])
        operation = self.run_operation(domain, lease, 'named-trace', 'action',
            {'name': 'read_trace', 'args': {'trace': 'A', 'archive_name': 'Run_7'}}, 3)
        reference = operation['result']['result']['archive_ref']
        self.assertEqual(reference['name'], 'Run_7')
        self.assertEqual(reference['id'], operation['operation_id'])
        files = list((self.worker_root / 'Result' / 'Run_7').glob('*/manifest.json'))
        self.assertEqual(len(files), 1)
        original = files[0].read_bytes()
        intent['params'] = operation['command']['params']
        same = self.client.call('execute', dict(request_id='named-trace', intent=intent))
        self.assertTrue(same['ok'], same)
        self.assertEqual(same['result']['operation_id'], operation['operation_id'])
        intent['params']['args']['archive_name'] = 'Different'
        self.assertFalse(self.client.call('execute', dict(request_id='named-trace', intent=intent))['ok'])
        self.assertFalse((self.worker_root / 'Result' / 'Different').exists())
        self.assertEqual(files[0].read_bytes(), original)

    def test_selected_archive_root_is_persisted_and_applied_only_on_restart(self):
        root = Path(self.directory.name) / 'selected-result'
        settings = dict(python_path=str(f.sys.executable), host_name='Archive Host', data_root=str(root))
        result = self.client.call('save_settings', dict(expected_rev=0, settings=settings))
        self.assertTrue(result['ok'], result)
        self.assertTrue(result['result']['restart_required'])
        self.assertEqual(self.client.call('snapshot')['result']['registry']['settings']['data_root'], str(root))
        invalid = dict(settings, data_root='../outside')
        self.assertFalse(self.client.call('save_settings', dict(expected_rev=1, settings=invalid))['ok'])
        self.assertNotEqual(Path(self.client.call('snapshot')['result']['archive']['active_root']), root)
        self.assertTrue(self.client.call('stop', {'confirm': True})['ok'])
        self.child.wait(8)
        for client in self.clients:
            client.close()
        self.child = self.launch()
        self.endpoint = json.loads(self.child.stdout.readline())['endpoint']
        self.client = self.connect()
        self.assertTrue(self.client.call('ping')['ok'])
        archive = self.client.call('snapshot')['result']['archive']
        self.assertEqual(Path(archive['active_root']), root)
        self.assertTrue(archive['available'], archive)
        self.assertTrue(root.is_dir())
