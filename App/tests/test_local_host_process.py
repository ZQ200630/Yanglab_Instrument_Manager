"""Windows Host contract tests, real drivers with explicit finite transports.

Not GUI, hardware or remote-network acceptance. Uses the production global
Host guard; an existing lab Host is never stopped or displaced by this suite.
"""
import ctypes
from ctypes import wintypes
import json
import hashlib
from pathlib import Path
import queue
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid
from App.tests.host_wire_fixture import stage_worker
from App.tests.host_build_fixture import native_host_binary

ROOT = Path(__file__).resolve().parents[2]
BINARY = native_host_binary()
kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.CreateFileW.argtypes = (wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
    wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE)
kernel.CreateFileW.restype = wintypes.HANDLE
kernel.ReadFile.argtypes = (wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD,
    ctypes.POINTER(wintypes.DWORD), wintypes.LPVOID)
kernel.WriteFile.argtypes = kernel.ReadFile.argtypes
kernel.CloseHandle.argtypes = (wintypes.HANDLE,)

class Pipe:
    def __init__(self, endpoint):
        self.handle = kernel.CreateFileW(endpoint, 0x00120183, 0, None, 3, 0x00110000, None)
        if self.handle == wintypes.HANDLE(-1).value:
            raise ctypes.WinError(ctypes.get_last_error())
        self.sequence = 0

    def close(self):
        if self.handle is not None:
            kernel.CloseHandle(self.handle)
            self.handle = None

    def write(self, data):
        count = wintypes.DWORD()
        if not kernel.WriteFile(self.handle, data, len(data), ctypes.byref(count), None):
            raise ctypes.WinError(ctypes.get_last_error())
        if count.value != len(data):
            raise RuntimeError('Partial test pipe write')

    def read(self, length):
        result = bytearray()
        while len(result) < length:
            buffer = ctypes.create_string_buffer(length - len(result))
            count = wintypes.DWORD()
            if not kernel.ReadFile(self.handle, buffer, len(buffer), ctypes.byref(count), None):
                raise ctypes.WinError(ctypes.get_last_error())
            if not count.value:
                raise EOFError('Pipe closed')
            result.extend(buffer.raw[:count.value])
        return bytes(result)

    def receive(self):
        length, = struct.unpack('<I', self.read(4))
        if length > 65536:
            raise AssertionError('Unbounded Host frame')
        return json.loads(self.read(length))

    def call(self, method, params=None):
        self.sequence += 1
        request = dict(v=1, id=str(self.sequence), method=method, params=params or {})
        data = json.dumps(request).encode()
        self.write(struct.pack('<I', len(data)) + data)
        return self.receive()

    def receive_event(self):
        data = bytearray()
        binding = None
        while True:
            chunk = self.receive()
            expected = (chunk['seq'], chunk['size'], chunk['checksum'])
            if not chunk.get('event_chunk') or chunk['offset'] != len(data) or (binding and binding != expected):
                raise AssertionError('Mismatched test event frame')
            binding = expected
            if not 0 < chunk['size'] <= 1024*1024:
                raise AssertionError('Unbounded test event')
            data.extend(bytes.fromhex(chunk['data_hex']))
            if len(data) == chunk['size']:
                if hashlib.sha256(data).hexdigest() != chunk['checksum']:
                    raise AssertionError('Test event checksum mismatch')
                return json.loads(data)
            if len(data) > chunk['size']:
                raise AssertionError('Test event overflow')


class HostPreflightTests(unittest.TestCase):
    def test_an_existing_executable_outside_visa_never_creates_an_intent(self):
        with tempfile.TemporaryDirectory(prefix='yang-preflight-environment-') as directory:
            root = Path(directory)
            candidate = root / 'python.exe'
            candidate.write_bytes(b'not a launched interpreter')
            child = subprocess.run([str(BINARY), '--root', str(ROOT), '--record-dir', str(root / 'records'),
                '--python', str(candidate), '--real'], capture_output=True, text=True, timeout=15)
            self.assertEqual(child.returncode, 2, child.stderr)
            self.assertEqual(json.loads(child.stderr)['code'], 'WorkerStartup')
            self.assertFalse((root / 'records/worker.json').exists())

    def test_unavailable_paths_never_create_an_unresolved_worker_intent(self):
        for missing in ('python', 'root'):
            with self.subTest(missing=missing), tempfile.TemporaryDirectory(prefix='yang-preflight-') as directory:
                record = Path(directory) / 'records'
                child = subprocess.run([str(BINARY), '--root', str(ROOT if missing != 'root' else Path(directory) / 'absent'),
                    '--record-dir', str(record), '--python', str(sys.executable if missing != 'python' else Path(directory) / 'absent/python.exe'), '--real'],
                    capture_output=True, text=True, timeout=15)
                self.assertEqual(child.returncode, 2, child.stderr)
                self.assertEqual(json.loads(child.stderr)['code'], 'WorkerStartup')
                self.assertFalse((record / 'worker.json').exists(), 'pre-spawn validation failure left an ownership intent')


class LocalHostProcessTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), f'Build the native debug Host first: {BINARY}')
        self.directory = tempfile.TemporaryDirectory(prefix='yang-host-contract-')
        self.addCleanup(self.directory.cleanup)
        self.worker_root = stage_worker(Path(self.directory.name) / 'worker-root')
        self.children = []
        self.clients = []
        self.addCleanup(self.cleanup_owned_transports)
        self.seed_registry(self.directory.name)
        self.child = self.launch()
        output = queue.Queue()
        threading.Thread(target=lambda: output.put(self.child.stdout.readline()), daemon=True).start()
        try:
            self.endpoint = json.loads(output.get(timeout=12))['endpoint']
        except Exception:
            try:
                self.child.wait(1)
                diagnostic = self.child.stderr.read()
            except subprocess.TimeoutExpired:
                diagnostic = 'Child still running'
            self.fail('Contract Host startup failed; no existing Host may be displaced: ' + diagnostic)
        self.client = self.connect()
        self.assertTrue(self.client.call('ping')['ok'])

    def seed_registry(self, directory):
        pass

    def launch(self, directory=None):
        child = subprocess.Popen([str(BINARY), '--real', '--root', str(self.worker_root),
            '--record-dir', directory or self.directory.name, '--python', sys.executable,
            ], stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.children.append(child)
        return child

    def connect(self):
        deadline = time.monotonic() + 3
        while True:
            try:
                client = Pipe(self.endpoint)
                self.clients.append(client)
                return client
            except OSError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.02)

    def cleanup_owned_transports(self):
        # Only these Popen objects and explicitly staged transport-injected
        # roots are owned here. Never discover/terminate another lab Host.
        try:
            for client in self.clients:
                if client is not getattr(self, 'client', None):
                    client.close()
            if hasattr(self, 'endpoint') and self.child.poll() is None:
                current = getattr(self, 'client', None)
                if current is None or current.handle is None:
                    current = self.connect()
                try:
                    response = current.call('stop', {'confirm': True})
                except OSError:
                    current = self.connect()
                    response = current.call('stop', {'confirm': True})
                self.assertTrue(response['ok'], response)
        finally:
            for client in self.clients:
                client.close()
            for child in self.children:
                try:
                    self.assertIsNotNone(child.wait(8))
                finally:
                    if child.poll() is None:
                        child.terminate()  # Test-created no-hardware peer only.
                        child.wait(5)
                    child.stdout.close()
                    child.stderr.close()

    def test_real_driver_transport_start_stop_and_pipe_reconnect_preserve_host(self):
        ping = self.client.call('ping')
        self.assertTrue(ping['ok'], ping)
        self.assertEqual(ping['result']['mode'], 'real')
        self.assertEqual(ping['result']['worker_protocol'], 3)
        record = json.loads((Path(self.directory.name) / 'worker.json').read_text())
        self.assertEqual(record['phase'], 'identified')
        self.assertIsInstance(record['pid'], int)
        self.assertEqual(len(record['creation_time']), 16)
        host_id = self.client.call('snapshot')['result']['host_id']
        self.client.close()
        self.client = self.connect()
        self.assertEqual(self.client.call('snapshot')['result']['host_id'], host_id)
        self.assertFalse(self.client.call('stop')['ok'])

    def test_surviving_channels_can_query_and_close_but_not_restore_authority(self):
        primary = self.connect()
        hello = primary.call('ping')['result']
        results = self.connect()
        self.assertTrue(results.call('ping', dict(attach_token=hello['attach_token'], channel='results'))['ok'])
        safety = self.connect()
        self.assertTrue(safety.call('ping', dict(attach_token=hello['attach_token'], channel='safety'))['ok'])
        events = self.connect()
        self.assertTrue(events.call('ping', dict(attach_token=hello['attach_token'], channel='events'))['ok'])
        self.assertTrue(events.call('subscribe')['ok'])
        self.assertEqual(events.receive_event()['type'], 'snapshot')
        # Snapshot requests are read-only and must not depend on the ordinary channel.
        requested = results.call('request_snapshot')
        self.assertTrue(requested['ok'], requested)
        primary.close()
        time.sleep(.1)
        original = results.call('operation', dict(request_id='lost-receipt'))
        self.assertEqual(original['error']['code'], 'OperationUnknown', original)
        self.assertEqual(results.call('acquire_control', dict(domain=dict(kind='device', id='a'*32)))['error']['code'], 'SessionRevoked')
        closed = safety.call('close_client')
        self.assertTrue(closed['ok'], closed)
        self.assertTrue(closed['result']['released'])
        self.assertFalse(self.connect().call('ping', dict(attach_token=hello['attach_token'], channel='heartbeat'))['ok'])

    def test_async_channels_authenticate_once_and_status_cannot_command_hardware(self):
        primary = self.connect()
        hello = primary.call('ping')['result']
        status, background = self.connect(), self.connect()
        for channel, name in ((status, 'status'), (background, 'background')):
            attached = channel.call('ping', dict(attach_token=hello['attach_token'], channel=name))
            self.assertTrue(attached['ok'], attached)
            self.assertEqual(attached['result']['client_session_id'], hello['client_session_id'])
        self.assertTrue(status.call('snapshot')['ok'])
        self.assertTrue(status.call('catalog')['ok'])
        self.assertTrue(status.call('worker_status')['ok'])
        for method in ('execute', 'acquire_control', 'install_driver'):
            rejected = status.call(method)
            self.assertFalse(rejected['ok'], rejected)
            self.assertEqual(rejected['error']['code'], 'SessionRevoked')
        for method in ('execute', 'snapshot'):
            self.assertEqual(background.call(method)['error']['code'], 'SessionRevoked')
        self.assertTrue(background.call('driver_status')['ok'])
        self.assertTrue(self.client.call('snapshot')['ok'])

    def test_losing_status_channel_preserves_control_but_losing_background_revokes_it(self):
        primary = self.connect()
        hello = primary.call('ping')['result']
        status, background = self.connect(), self.connect()
        for channel, name in ((status, 'status'), (background, 'background')):
            self.assertTrue(channel.call('ping', dict(attach_token=hello['attach_token'], channel=name))['ok'])
        status.close()
        time.sleep(.05)
        replacement = self.connect().call('ping', dict(attach_token=hello['attach_token'], channel='status'))
        self.assertTrue(replacement['ok'], replacement)
        background.close()
        time.sleep(.05)
        rejected = self.connect().call('ping', dict(attach_token=hello['attach_token'], channel='background'))
        self.assertFalse(rejected['ok'], rejected)

    def test_second_host_does_not_replace_worker_record_or_spawn(self):
        original = (Path(self.directory.name) / 'worker.json').read_bytes()
        second = self.launch()
        self.assertEqual(second.wait(5), 2)
        error = json.loads(second.stderr.readline())
        self.assertEqual(error['code'], 'HostAlreadyRunning')
        self.assertEqual((Path(self.directory.name) / 'worker.json').read_bytes(), original)
        self.assertTrue(self.client.call('ping')['ok'])

    def test_unresolved_intent_and_pid_reuse_block_new_spawn(self):
        # Ownership checks must run after our first guarded Host releases.
        # Production no longer has a test-tag bypass for a second physical Host.
        self.assertTrue(self.client.call('stop', {'confirm': True})['ok'])
        self.assertEqual(self.child.wait(8), 0)
        with tempfile.TemporaryDirectory(prefix='yang-host-orphan-contract-') as directory:
            for phase in ('intent', 'identified'):
                record = dict(version=1, phase=phase, nonce=uuid.uuid4().hex,
                    pid=self.child.pid if phase == 'identified' else None,
                    creation_time='00000000000000ff' if phase == 'identified' else None)
                path = Path(directory) / 'worker.json'
                path.write_text(json.dumps(record))
                original = path.read_bytes()
                orphan_start = self.launch(directory)
                self.assertEqual(orphan_start.wait(5), 2)
                self.assertEqual(json.loads(orphan_start.stderr.readline())['code'], 'OwnershipUnknown')
                self.assertEqual(path.read_bytes(), original)
                self.assertFalse((Path(directory) / 'devices.json').exists())

    def test_sixteen_clients_then_explicit_capacity_and_reconnect(self):
        for _ in range(15):
            self.assertTrue(self.connect().call('ping')['ok'])
        seventeenth = self.connect()
        reply = seventeenth.call('ping')
        self.assertFalse(reply['ok'])
        self.assertEqual(reply['error']['code'], 'ClientCapacity')
        seventeenth.close()
        self.clients[1].close()
        time.sleep(.1)
        self.assertTrue(self.connect().call('ping')['ok'])

    def test_oversized_duplicate_and_truncated_clients_cannot_dispatch(self):
        invalid = self.connect()
        invalid.write(struct.pack('<I', 65537))
        with self.assertRaises((OSError, EOFError)):
            invalid.receive()
        invalid.close()
        duplicate = self.connect()
        data = b'{"v":1,"v":1,"id":"x","method":"stop","params":{"confirm":true}}'
        duplicate.write(struct.pack('<I', len(data)) + data)
        with self.assertRaises((OSError, EOFError)):
            duplicate.receive()
        duplicate.close()
        truncated = self.connect()
        truncated.write(struct.pack('<I', 400) + b'{}')
        truncated.close()
        self.assertTrue(self.client.call('ping')['ok'])
        self.assertFalse(self.client.call('snapshot', {'local': True})['ok'])


if __name__ == '__main__':
    unittest.main()
