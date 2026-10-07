"""Hardware-free admission and receipt checks for the laser Host diagnostic."""
import unittest
from unittest.mock import patch, MagicMock
from Code.Debugs import check_laser_host as diagnostic
from Code.Debugs.check_laser_host import read_intent, execute_read, require_laser


class LaserHostDiagnosticTests(unittest.TestCase):
    def test_later_failure_preserves_identity_and_completed_sample_evidence(self):
        snapshot, lease, domain = self.authority(False)
        snapshot.update(startup_error=None, registry={'devices':[], 'registry_rev':0})
        record = dict(model_id='tlb6700', profile_id='newport-usb', params={'device_key':'6700 SN1012'},
                      verified_mode='real', config_rev=1, expected_identity={'model':'TLB-6700', 'serial':'1012',
                      'head_model':'6722-P', 'head_serial':'0953'})
        sample = {'identity':record['expected_identity'], 'laser':{'received_at':1.0}}
        class Client:
            def call(self, method, params=None):
                return {'ping':{'boot_id':snapshot['boot_id'], 'attach_token':'f'*32}, 'snapshot':snapshot,
                        'create_draft':{'device_id':domain['id'], 'config_digest':'0'*64},
                        'acquire_control':lease, 'test_connection':{'proof_id':'1'*32}, 'save_device':record}[method]
        evidence = {}
        with patch.object(diagnostic, 'LeasePulse', return_value=MagicMock()), \
             patch.object(diagnostic, 'execute_read', side_effect=[{'phase':'completed'}, RuntimeError('readback failed')]), \
             patch.object(diagnostic, 'await_sample', return_value=sample):
            with self.assertRaisesRegex(RuntimeError, 'readback failed'):
                diagnostic.qualify(Client(), 'unused finite endpoint', '6700 SN1012', 3, evidence=evidence)
        self.assertEqual(evidence['record'], record)
        self.assertEqual(evidence['samples'], [sample])
        self.assertEqual(len(evidence['operations']), 1)

    def authority(self, connected=True):
        domain = {'kind':'device', 'id':'a'*32}
        context = dict(domain=domain, session_id='b'*32, connection_id='c'*32 if connected else None, epoch=1)
        snapshot = dict(boot_id='d'*32, domains={'device:'+domain['id']:dict(context=context)})
        lease = dict(boot_id='d'*32, token='e'*32, control_epoch=1)
        return snapshot, lease, domain

    def test_only_connect_and_read_status_can_be_prepared(self):
        snapshot, lease, domain = self.authority()
        self.assertEqual(read_intent(snapshot, lease, domain, 1, 'read_motion', 2)['params'],
                         {'name':'read_motion', 'args':{}})
        self.assertEqual(read_intent(snapshot, lease, domain, 1, 'read_status', 2)['params'],
                         {'name':'read_status', 'args':{}})
        for action in ('set_wavelength', 'set_output', 'set_remote', 'write', 'disconnect'):
            with self.assertRaises(ValueError): read_intent(snapshot, lease, domain, 1, action, 2)
        disconnected, lease, domain = self.authority(False)
        self.assertEqual(read_intent(disconnected, lease, domain, 1, 'connect', 1)['params'],
                         {'acknowledge_lifecycle':True})
        with self.assertRaises(ValueError): read_intent(disconnected, lease, domain, 1, 'read_status', 2)
        lease['boot_id'] = 'f'*32
        with self.assertRaises(ValueError): read_intent(disconnected, lease, domain, 1, 'connect', 2)

    def test_exact_controller_and_head_identity_are_required(self):
        record = dict(model_id='tlb6700', profile_id='newport-usb', params={'device_key':'6700 SN1012'},
                      verified_mode='real', expected_identity={'model':'TLB-6700', 'serial':'1012',
                      'head_model':'6722-P', 'head_serial':'0953'})
        require_laser(record, '6700 SN1012')
        with self.assertRaises(ValueError): require_laser(record, '6700 SN1020')
        record['expected_identity']['head_serial'] = ''
        with self.assertRaises(ValueError): require_laser(record, '6700 SN1012')

    def test_failed_or_mismatched_receipt_never_reexecutes(self):
        snapshot, lease, domain = self.authority()
        for receipt in ({'operation_id':'f'*32, 'status':'Terminal', 'phase':'failed'},
                        {'operation_id':'0'*32, 'status':'Terminal', 'phase':'completed'}):
            class Client:
                calls = []
                def call(self, method, params=None):
                    self.calls.append(method)
                    return {'snapshot':snapshot, 'prepare':{'token':'confirmation'},
                            'execute':{'operation_id':'f'*32}, 'operation':receipt}[method]
            client = Client()
            with self.assertRaises((ValueError, RuntimeError)):
                execute_read(client, lease, domain, 1, 'read_status', 2)
            self.assertEqual(client.calls.count('execute'), 1)


if __name__ == '__main__': unittest.main()
