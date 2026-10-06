"""Diagnostic pipe against the actual Host, with finite transport fixtures only."""
from Code.Debugs import check_osa_host as d
from Code.Debugs.check_osa_host import HostLink, NativePipe
import struct
import tempfile
from pathlib import Path
from App.tests import test_local_host_process as f


class DiagnosticPipeTests(f.LocalHostProcessTests):
    def test_diagnostic_reader_binds_real_frames_and_releases_its_handle(self):
        wire = NativePipe(self.endpoint, timeout=2)
        with HostLink(wire) as client:
            ping = client.call('ping')
            self.assertEqual(ping['mode'], 'real')
            snapshot = client.call('snapshot')
            self.assertEqual(snapshot['boot_id'], ping['boot_id'])
            self.assertEqual(snapshot['host_id'], self.client.call('snapshot')['result']['host_id'])
        self.assertIsNone(wire.handle)
        self.assertTrue(self.client.call('ping')['ok'])

    def test_empty_pipe_read_has_a_deadline_without_replaying_or_stopping_host(self):
        wire = NativePipe(self.endpoint, timeout=.05)
        with HostLink(wire):
            with self.assertRaises(TimeoutError):
                wire.read(4)
        self.assertIsNone(wire.handle)
        self.assertTrue(self.client.call('ping')['ok'])

    def test_diagnostic_operation_and_native_archive_use_actual_host_routes(self):
        with HostLink(NativePipe(self.endpoint, timeout=5)) as client:
            hello = client.call('ping')
            draft = client.call('create_draft', dict(model_id='aq6370',profile_id='gpib-visa',params={'resource':'GPIB0::4::INSTR'},name='OSA',expected_rev=0))
            domain = dict(kind='device',id=draft['device_id'])
            lease = client.call('acquire_control', {'domain':domain})
            with d.LeasePulse(self.endpoint,hello['attach_token'],lease) as pulse:
                pulse.check()
                self.assertGreaterEqual(pulse.renewals,1)
                proof = client.call('test_connection', dict(draft_id=domain['id'],expected_rev=1,consent=dict(accepted=True,mode='real',config_digest=draft['config_digest'],open_effects=[],supervised=False,retain_session=False),lease_token=lease['token'],control_epoch=lease['control_epoch'],request_id='verify',sequence=1))
                client.call('save_device',dict(draft_id=domain['id'],proof_id=proof['proof_id'],expected_rev=1))
                d.execute_read(client,lease,domain,1,'connect',2,timeout=5)
                operation = d.execute_read(client,lease,domain,1,'read_trace',3,timeout=5)
                ref = operation['result']['result']['archive_ref']
                host_id = client.call('snapshot')['host_id']
                manifest, raw, capture = d.fetch_archive(client,ref,host_id,domain)
                self.assertEqual(raw,struct.pack('<dddd',1550.,-30.,1551.,-31.))
                self.assertEqual(capture.native_unit,'dBm')
                self.assertGreater(len(manifest),0)
            self.assertTrue(client.call('close_client')['released'])


class DiagnosticOwnedHostTests(f.unittest.TestCase):
    def test_owns_only_its_child_and_records_release_before_exit(self):
        with tempfile.TemporaryDirectory(prefix='yang-diag-test-') as directory:
            root = f.stage_worker(Path(directory)/'root')
            owned = d.HostChild(f.BINARY,root,Path(directory)/'records')
            try:
                owned.start()
                with owned.link() as client:
                    self.assertEqual(client.call('ping')['mode'],'real')
                    report = owned.stop(client)
                self.assertIs(report['resource_released'],True)
                self.assertEqual(report['host_exit_code'],0)
                self.assertEqual(owned.child.poll(),0)
            finally:
                if owned.child is not None and owned.child.poll() is None:
                    with owned.link() as cleanup:
                        cleanup.call('ping')
                        owned.stop(cleanup)
