"""Actual native Host/worker laser routes with a finite read-only transport."""
from pathlib import Path
import tempfile
import unittest
import time
from concurrent.futures import ThreadPoolExecutor

from App.tests.host_build_fixture import native_host_binary
from App.tests.host_wire_fixture import stage_worker
from Code.Debugs.check_laser_host import qualify
from Code.Debugs.check_osa_host import HostChild


class NativeLaserTests(unittest.TestCase):
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
                        self.assertIsNone(sample['wavelength_range_nm'])
                        self.assertEqual(sample['laser']['wavelength_nm'], 1060.01)
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
