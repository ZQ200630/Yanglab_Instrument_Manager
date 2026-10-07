"""Actual native Host/worker laser routes with a finite read-only transport."""
from pathlib import Path
import tempfile
import unittest
import time
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch

from App.tests.host_build_fixture import native_host_binary
from App.tests.host_wire_fixture import stage_worker
from Code.Debugs.check_laser_host import qualify, execute_read
from Code.Debugs.check_osa_host import HostChild


class NativeLaserTests(unittest.TestCase):
    def test_failed_terminal_metadata_stays_unknown_until_a_successful_query(self):
        with tempfile.TemporaryDirectory(prefix='yang-laser-metadata-') as directory:
            root = stage_worker(Path(directory)/'worker', metadata_fault=True)
            host = HostChild(native_host_binary(), root, Path(directory)/'records')
            try:
                host.start()
                with host.link() as client:
                    hello = client.call('ping')
                    def fail_after_warm(client, lease, domain, rev, action, sequence):
                        if action == 'connect':
                            return execute_read(client, lease, domain, rev, action, sequence)
                        # Warm the cache just before the existing sampler wakes.
                        first = float((root/'first-status-at').read_text())
                        while time.monotonic() < first+2.1: time.sleep(.01)
                        execute_read(client, lease, domain, rev, 'read_status', sequence)
                        (root/'metadata-fail').write_text('fail')
                        receipt = execute_read(client, lease, domain, rev, 'read_status', sequence+1)
                        while time.monotonic() < first+3.0: time.sleep(.01)
                        status = client.call('snapshot')['domains']['device:'+domain['id']]
                        self.assertEqual(status.get('communication'), 'UNKNOWN')
                        self.assertIsNone(status['host_sample_ms'])
                        self.assertGreaterEqual((root/'metadata-replies').read_text().count('failed'), 2)
                        (root/'metadata-fail').unlink()
                        deadline = time.monotonic()+3
                        while client.call('snapshot')['domains']['device:'+domain['id']].get('communication') == 'UNKNOWN':
                            self.assertLess(time.monotonic(), deadline)
                            time.sleep(.02)
                        return receipt
                    with patch('Code.Debugs.check_laser_host.execute_read', fail_after_warm):
                        qualify(client, host.endpoint, '6700 SN1012', 2, hello=hello)
                    host.stop(client)
            finally:
                (root/'metadata-fail').unlink(missing_ok=True)
                if host.child is not None and host.child.poll() is None and not host.stop_attempted and host.endpoint is not None:
                    with host.link() as cleanup:
                        cleanup.call('ping'); host.stop(cleanup)
                host.close_streams()

    def test_completed_read_receipt_does_not_wait_for_inventory(self):
        with tempfile.TemporaryDirectory(prefix='yang-laser-overlap-') as directory:
            root = stage_worker(Path(directory)/'worker', hold_inventory=True, hold_laser_read=True)
            host = HostChild(native_host_binary(), root, Path(directory)/'records')
            try:
                host.start()
                with host.link() as client:
                    hello = client.call('ping')
                    def read_during_inventory(client, lease, domain, rev, action, sequence):
                        if action == 'connect':
                            return execute_read(client, lease, domain, rev, action, sequence)
                        def wait_entered(name, pending):
                            deadline = time.monotonic()+2
                            while not (root/name).exists():
                                self.assertFalse(pending.done())
                                self.assertLess(time.monotonic(), deadline)
                                time.sleep(.01)
                        def inventory():
                            with host.link() as observer:
                                observer.call('ping')
                                return observer.call('driver_status')
                        with ThreadPoolExecutor(max_workers=2) as pool:
                            (root/'laser-hold').write_text('hold')
                            read = pool.submit(execute_read, client, lease, domain, rev, action, sequence)
                            try:
                                wait_entered('laser-entered', read)
                                query = pool.submit(inventory)
                                wait_entered('inventory-entered', query)
                                (root/'laser-release').write_text('release')
                                receipt = read.result(1)
                                self.assertEqual(receipt['phase'], 'completed')
                                self.assertFalse(query.done(), 'Inventory must still own its finite gate')
                                self.assertIsNone(client.call('snapshot')['domains']['device:'+domain['id']]['host_sample_ms'])
                            finally:
                                (root/'laser-release').write_text('release')
                                (root/'inventory-release').write_text('release')
                            self.assertEqual(query.result(3), {'fixture_inventory':True})
                        deadline = time.monotonic()+3
                        while client.call('snapshot')['domains']['device:'+domain['id']].get('communication') == 'UNKNOWN':
                            self.assertLess(time.monotonic(), deadline)
                            time.sleep(.02)
                        return receipt
                    with patch('Code.Debugs.check_laser_host.execute_read', read_during_inventory):
                        qualify(client, host.endpoint, '6700 SN1012', 2, hello=hello)
                    host.stop(client)
            finally:
                (root/'laser-release').write_text('release')
                (root/'inventory-release').write_text('release')
                if host.child is not None and host.child.poll() is None and not host.stop_attempted and host.endpoint is not None:
                    with host.link() as cleanup:
                        cleanup.call('ping'); host.stop(cleanup)
                host.close_streams()

    def test_terminal_laser_operation_publishes_fresh_snapshot_without_cache_wait(self):
        with tempfile.TemporaryDirectory(prefix='yang-laser-publish-') as directory:
            root = stage_worker(Path(directory)/'worker')
            host = HostChild(native_host_binary(), root, Path(directory)/'records')
            def immediate_sample(client, domain, previous=None):
                # Observe the real Host immediately after its terminal receipt.
                # No polling the old 2.5-second cache or replacing a production backend.
                value = client.call('snapshot')['domains']['device:'+domain['id']]
                self.assertEqual(value['state'], 'READY')
                device = value.get('device') or {}
                self.assertTrue(device.get('connected'))
                self.assertNotEqual(device['laser']['received_at'], previous)
                return device
            try:
                host.start()
                with host.link() as client:
                    hello = client.call('ping')
                    with patch('Code.Debugs.check_laser_host.await_sample', immediate_sample):
                        result = qualify(client, host.endpoint, '6700 SN1012', 3, hello=hello)
                    self.assertEqual(len(result['samples']), 3)
                    self.assertEqual([item['action'] for item in result['timings']],
                                     ['connect', 'read_status', 'read_status'])
                    for item in result['timings']:
                        self.assertGreater(item['terminal_elapsed_s'], 0)
                        self.assertGreaterEqual(item['total_elapsed_s'], item['terminal_elapsed_s'])
                        self.assertLess(item['sample_publication_after_terminal_s'], 1)
                    host.stop(client)
            finally:
                if host.child is not None and host.child.poll() is None and not host.stop_attempted and host.endpoint is not None:
                    with host.link() as cleanup:
                        cleanup.call('ping')
                        host.stop(cleanup)
                host.close_streams()

    def test_driver_inventory_queries_share_the_reserved_query_lane(self):
        with tempfile.TemporaryDirectory(prefix='yang-inventory-host-') as directory:
            root = stage_worker(Path(directory)/'worker', hold_inventory=True)
            host = HostChild(native_host_binary(), root, Path(directory)/'records')
            try:
                host.start()
                with host.link() as warm:
                    warm.call('ping')
                    warm.call('snapshot')
                def inventory():
                    with host.link() as client:
                        client.call('ping')
                        return client.call('driver_status')
                with ThreadPoolExecutor(max_workers=2) as pool:
                    first = pool.submit(inventory)
                    try:
                        deadline = time.monotonic()+3
                        while not (root/'inventory-entered').exists():
                            self.assertFalse(first.done(), 'First inventory did not reach its finite gate')
                            self.assertLess(time.monotonic(), deadline)
                            time.sleep(.01)
                        second = pool.submit(inventory)
                        time.sleep(.1)
                    finally:
                        (root/'inventory-release').write_text('release')
                    self.assertEqual(first.result(3), {'fixture_inventory':True})
                    self.assertEqual(second.result(3), {'fixture_inventory':True})
            finally:
                (root/'inventory-release').write_text('release')
                if host.child is not None and host.child.poll() is None and host.endpoint is not None:
                    with host.link() as cleanup:
                        cleanup.call('ping')
                        host.stop(cleanup)
                host.close_streams()

    def test_identity_proof_head_persistence_three_samples_and_preserving_release(self):
        binary = native_host_binary()
        self.assertTrue(binary.is_file(), f'Build the debug Host first: {binary}')
        with tempfile.TemporaryDirectory(prefix='yang-laser-host-') as directory:
            root = stage_worker(Path(directory)/'worker', hold_inventory=True)
            (root/'inventory-release').write_text('release')
            host = HostChild(binary, root, Path(directory)/'records')
            try:
                try:
                    host.start()
                except Exception:
                    self.fail('Owned fixture Host startup failed: '+repr(list(host.stderr)))
                with host.link() as client:
                    hello = client.call('ping')
                    self.assertEqual(client.call('driver_status'), {'fixture_inventory':True})
                    result = qualify(client, host.endpoint, '6700 SN1012', 3, hello=hello)
                    identity = result['record']['expected_identity']
                    self.assertEqual(identity['serial'], '1012')
                    self.assertEqual(identity['head_model'], '6722-P')
                    self.assertEqual(identity['head_serial'], 'P1001')
                    self.assertEqual(len(result['samples']), 3)
                    for sample in result['samples']:
                        self.assertFalse(sample['laser']['output_enabled'])
                        self.assertEqual(sample['wavelength_range_nm'], [1045.0, 1085.0])
                        self.assertEqual(sample['max_scan_speed_nm_s'], 10.0)
                        self.assertEqual(sample['laser']['wavelength_nm'], 1060.01)
                    with host.link() as editor:
                        editor.call('ping')
                        record=result['record'];domain={'kind':'device','id':record['device_id']}
                        self.assertTrue(editor.call('safe_stop',{'domain':domain})['accepted'])
                        deadline=time.monotonic()+5
                        while True:
                            snapshot=editor.call('snapshot');status=snapshot['domains']['device:'+domain['id']]
                            if status['state']=='DISCONNECTED' and status['context']['connection_id'] is None:break
                            self.assertLess(time.monotonic(),deadline);time.sleep(.02)
                        saved=editor.call('save_laser_limits',dict(device_id=domain['id'],config_rev=record['config_rev'],
                            expected_rev=snapshot['registry']['registry_rev'],limits={'min_nm':1050.,'max_nm':1080.,'max_speed_nm_s':1.}))
                        configured=next(d for d in saved['devices'] if d['device_id']==domain['id'])
                        self.assertEqual(configured['params']['device_key'],'6700 SN1012')
                        self.assertEqual(configured['expected_identity'],identity)
                        lease=editor.call('acquire_control',{'domain':domain})
                        execute_read(editor,lease,domain,configured['config_rev'],'connect',100)
                        status=editor.call('snapshot')['domains']['device:'+domain['id']]['device']
                        self.assertEqual(status['operating_range_nm'],[1050.,1080.])
                        self.assertEqual(status['operating_max_speed_nm_s'],1.)
                    stop = host.stop(client)
                    self.assertTrue(stop['resource_released'])
                    self.assertTrue(stop['process_exit']['confirmed'])
                    self.assertEqual(stop['host_exit_code'], 0)
            finally:
                if host.child is not None and host.child.poll() is None and not host.stop_attempted and host.endpoint is not None:
                    with host.link() as cleanup:
                        cleanup.call('ping')
                        host.stop(cleanup)
                host.close_streams()


if __name__ == '__main__': unittest.main()
