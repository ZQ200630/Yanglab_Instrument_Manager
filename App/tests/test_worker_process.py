"""Subprocess checks for the persistent local JSON-line worker."""

from __future__ import annotations

import json
import subprocess
import sys
import time
import unittest
from pathlib import Path
import tempfile
import threading
import queue
import textwrap
from copy import deepcopy


class WorkerProcessTests(unittest.TestCase):
    def test_blocked_stdout_starts_independent_cleanup_without_false_release(self):
        from unittest.mock import patch
        from App.worker.controller import ConsoleController
        from App.worker.main import run
        from App.tests.driver_fixture import WireGain, WireOSA, WireVoltage, RESOURCES

        writing, output_release, osa_closing, osa_release, voltage_closed, gain_closed = (
            threading.Event() for _ in range(6))
        incoming, frames, codes = queue.Queue(), [], []
        class Input:
            def __iter__(self):
                return iter(incoming.get, None)
        class Output:
            def write(self, line):
                writing.set()
                if not output_release.wait(5): raise TimeoutError('stdout barrier not released')
                frames.append(json.loads(line))
            def flush(self):
                pass
        class OSA(WireOSA):
            def close(self):
                osa_closing.set()
                if not osa_release.wait(5): raise TimeoutError('close barrier not released')
                super().close()
        class Voltage(WireVoltage):
            def close(self):
                super().close()
                voltage_closed.set()
        class Gain(WireGain):
            def close(self):
                super().close()
                gain_closed.set()
        controller = ConsoleController(port_enumerator=lambda: (), factories={
            'osa': OSA, 'voltage': Voltage, 'gain': Gain})
        server = None
        try:
            for role in ('osa', 'voltage', 'gain'):
                controller.handle('connect', dict(role=role, resource=RESOURCES[role],
                    acknowledge_lifecycle=True))
            workers = tuple(controller._scheduler._threads)
            incoming.put(json.dumps(dict(v=2, id='blocked-ping', method='ping',
                                         params={}, context=None)) + '\n')
            with tempfile.TemporaryDirectory() as directory, patch(
                    'App.worker.main.OUTPUT_STALL_SECONDS', 0):
                server = threading.Thread(target=lambda: codes.append(run(controller,
                    input_stream=Input(), output_stream=Output(),
                    settings_path=Path(directory) / 'settings.json')))
                server.start()
                self.assertTrue(writing.wait(2))
                for event in (osa_closing, voltage_closed, gain_closed):
                    self.assertTrue(event.wait(2), 'cleanup must not wait for stdout or OSA')
                self.assertEqual(frames, [], 'no response has been delivered through held stdout')
                self.assertIn('osa', controller.cached_status()['devices'])
                self.assertEqual(tuple(controller._scheduler._threads), workers)
                self.assertTrue(server.is_alive())
                osa_release.set()
                output_release.set()
                server.join(3)
                self.assertFalse(server.is_alive())
                self.assertEqual(codes, [2], 'stdout stall remains a transport fault')
                self.assertEqual(controller.cached_status()['last_cleanup']['unreleased'], [])
                self.assertEqual([frame['id'] for frame in frames], ['blocked-ping'])
        finally:
            osa_release.set()
            output_release.set()
            incoming.put(None)
            if server: server.join(3)
            controller.close()
            self.assertTrue(controller._scheduler.join(3))

    def _launch(self):
        folder = tempfile.TemporaryDirectory(prefix='yang-worker-wire-')
        self.addCleanup(folder.cleanup)
        # Test process only: real controller and real drivers, injected at the
        # transport boundary. No CLI/App path selects this fixture.
        source = textwrap.dedent("""
            import sys
            from pathlib import Path
            from App.worker.main import run
            from App.worker.controller import ConsoleController
            from App.tests.driver_fixture import factories, fiber_factory, FakePort
            ports = [FakePort('COM992', '2110148249-10'),
                     FakePort('COM993', '160721175410')]
            sources = factories()
            sources['fiber'] = lambda: fiber_factory(ports)(
                ('2110148249-10', '160721175410'))
            output = sys.stdout
            sys.stdout = sys.stderr
            controller = ConsoleController(factories=sources, port_enumerator=lambda: ports)
            raise SystemExit(run(controller, input_stream=sys.stdin, output_stream=output,
                settings_path=Path(sys.argv[1]) / 'settings.json'))
        """)
        return subprocess.Popen([sys.executable, '-B', '-u', '-c', source, folder.name],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1)

    def _request(self, child, request):
        assert child.stdin is not None and child.stdout is not None
        if not hasattr(child, "contexts"):
            child.contexts = {}
            child.session_context = None
        role = request.get("params", {}).get("role")
        if request["method"] not in {"ping", "status"} and child.session_context is None:
            self._request(child, {"v": 2, "id": "bootstrap", "method": "ping", "params": {}})
        request["context"] = child.contexts.get(role) if role else child.session_context
        child.stdin.write(json.dumps(request) + "\n")
        child.stdin.flush()
        reply = json.loads(child.stdout.readline())
        if reply.get("context"):
            if role:
                child.contexts[role] = reply["context"]
            else:
                child.session_context = reply["context"]
        for name, value in (reply.get("result", {}).get("roles", {}) if request["method"] in {"ping", "status"} else {}).items():
            child.contexts[name] = {key: value[key] for key in ("session_id", "connection_id", "epoch")}
        return reply

    def test_acquire_zero_status_are_correlated_before_osa_barrier_releases(self):
        # A peer-owned file barrier controls a transport-injected public acquire method.
        # The production protocol has no test or release commands.
        with tempfile.TemporaryDirectory() as directory:
            entered, release = Path(directory) / "entered", Path(directory) / "release"
            source = textwrap.dedent("""
                import sys, time
                from pathlib import Path
                from App.worker.main import run
                from App.worker.controller import ConsoleController
                from App.tests.driver_fixture import WireOSA, WireVoltage
                class BarrierOSA(WireOSA):
                    def acquire(self, trace="A"):
                        Path(sys.argv[1]).write_text("entered")
                        deadline = time.monotonic() + 10
                        while not Path(sys.argv[2]).exists():
                            if time.monotonic() > deadline:
                                raise TimeoutError("test release missing")
                            time.sleep(.01)
                        return super().acquire(trace)
                output = sys.stdout
                sys.stdout = sys.stderr
                controller = ConsoleController(port_enumerator=lambda: (), factories={"osa": BarrierOSA, "voltage": WireVoltage})
                raise SystemExit(run(controller, input_stream=sys.stdin, output_stream=output,
                    settings_path=Path(sys.argv[1]).with_name("settings.json")))
            """)
            child = subprocess.Popen([sys.executable, "-B", "-u", "-c", source, str(entered), str(release)],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
            reader = None
            try:
                for role in ("osa", "voltage"):
                    connected = self._request(child, dict(v=2, id="connect-" + role, method="connect",
                        params=dict(role=role, resource={"osa": "GPIB0::29::INSTR", "voltage": "COM990"}[role], acknowledge_lifecycle=True)))
                    self.assertTrue(connected["ok"], connected)
                frames = queue.Queue()
                reader = threading.Thread(target=lambda: [frames.put(json.loads(line)) for line in child.stdout])
                reader.start()
                requests = [
                    dict(v=2, id="acquire", method="action", params=dict(role="osa", name="acquire"), context=child.contexts["osa"]),
                    dict(v=2, id="zero", method="action", params=dict(role="voltage", name="zero"), context=child.contexts["voltage"]),
                    dict(v=2, id="status", method="status", params={}, context=child.session_context),
                ]
                child.stdin.write("".join(json.dumps(item) + "\n" for item in requests))
                child.stdin.flush()
                deadline = time.monotonic() + 2
                while not entered.exists() and time.monotonic() < deadline:
                    time.sleep(.01)
                self.assertTrue(entered.exists(), "transport-injected acquisition reached barrier")
                early = [frames.get(timeout=2), frames.get(timeout=2)]
                self.assertEqual({frame["id"] for frame in early}, {"zero", "status"})
                self.assertTrue(all(frame["ok"] for frame in early), early)
                self.assertFalse(release.exists())
                release.write_text("release")
                self.assertEqual(frames.get(timeout=2)["id"], "acquire")
                child.stdin.write(json.dumps(dict(v=2, id="shutdown", method="shutdown", params={},
                    context=child.session_context)) + "\n")
                child.stdin.flush()
                self.assertEqual(frames.get(timeout=2)["result"]["unreleased"], [])
                self.assertEqual(child.wait(timeout=3), 0, "stdin remains open")
            finally:
                release.write_text("release")
                child.stdin.close()
                child.wait(timeout=5)
                if reader: reader.join(3)
                child.stdout.close()
                child.stderr.close()

    def _cleanup_peer(self, directory):
        source = textwrap.dedent("""
            import sys
            from pathlib import Path
            from App.worker.main import run, _Replies
            from concurrent.futures import Future
            from unittest.mock import patch
            import time
            from App.worker.controller import ConsoleController
            from App.tests.driver_fixture import WireOSA, WireVoltage
            folder = Path(sys.argv[1])
            original_reserve = _Replies.reserve
            def reserve(replies, request):
                if request.id == "peer-fault":
                    raise RuntimeError("offline ingress fault")
                return original_reserve(replies, request)
            _Replies.reserve = reserve
            original_publish = _Replies.publish
            def publish(replies, request, completed):
                if request.id == "callback-fault":
                    completed = Future()
                    completed.set_exception(SystemExit("offline callback fault"))
                if request.id in {"publication-fault", "shutdown-publication-fault"}:
                    with patch.object(replies.queue, "put_nowait", side_effect=RuntimeError("offline publication fault")):
                        return original_publish(replies, request, completed)
                return original_publish(replies, request, completed)
            _Replies.publish = publish
            class FailingOSA(WireOSA):
                def close(self):
                    with (folder / "attempts").open("a") as log:
                        log.write("close\\n")
                    if (folder / "fail").exists():
                        raise OSError("offline close failure")
                    if (folder / "hold-close").exists():
                        (folder / "closing").touch()
                        while (folder / "hold-close").exists():
                            time.sleep(.01)
                    return super().close()
            output = sys.stdout
            sys.stdout = sys.stderr
            controller = ConsoleController(port_enumerator=lambda: (), factories={"osa": FailingOSA})
            original_cleanup = controller._close_compat
            def cleanup():
                report = original_cleanup()
                (folder / "cleanup-ready").touch()
                return report
            controller._close_compat = cleanup
            raise SystemExit(run(controller, input_stream=sys.stdin, output_stream=output,
                settings_path=folder / "settings.json"))
        """)
        return subprocess.Popen([sys.executable, "-B", "-u", "-c", source, directory],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)

    def test_failed_shutdown_retains_status_and_explicit_retry_creates_new_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            failure = Path(directory) / "fail"
            failure.touch()
            child = self._cleanup_peer(directory)
            try:
                self.assertTrue(self._request(child, dict(v=2, id="connect", method="connect",
                    params=dict(role="osa", resource="GPIB0::29::INSTR", acknowledge_lifecycle=True)))["ok"])
                first = self._request(child, dict(v=2, id="close1", method="shutdown", params={}))
                first_report = deepcopy(first['result'])
                self.assertEqual(first["result"]["unreleased"], ["osa"])
                self.assertIsNone(child.poll())
                status = self._request(child, dict(v=2, id="status", method="status", params={}))
                self.assertEqual(status["result"]["last_cleanup"], first["result"])
                failure.unlink()
                second = self._request(child, dict(v=2, id="close2", method="shutdown", params={}))
                self.assertEqual(second["result"]["unreleased"], [])
                self.assertNotEqual(second["result"]["attempt_id"], first["result"]["attempt_id"])
                self.assertEqual(first["result"]["unreleased"], ["osa"])
                self.assertEqual(child.wait(timeout=3), 0)
                self.assertEqual(first['result'], first_report)
            finally:
                child.stdin.close()
                child.wait(timeout=5)
                child.stdout.close()
                child.stderr.close()

    def test_retry_after_ingress_fault_keeps_latest_attempt_through_eof(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            failure, hold = folder / "fail", folder / "hold-close"
            failure.touch()
            child = self._cleanup_peer(directory)
            try:
                self.assertTrue(self._request(child, dict(v=2, id="connect", method="connect",
                    params=dict(role="osa", resource="GPIB0::29::INSTR", acknowledge_lifecycle=True)))["ok"])
                child.stdin.write(json.dumps(dict(v=2, id="peer-fault", method="status",
                    params={}, context=None)) + "\n")
                child.stdin.flush()
                self.assertIn("offline ingress fault", child.stderr.readline())
                deadline = time.monotonic() + 2
                while not (folder / "cleanup-ready").exists():
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.01)
                status = self._request(child, dict(v=2, id="status-after-fault", method="status", params={}))
                self.assertEqual(status["result"]["last_cleanup"]["unreleased"], ["osa"])
                hold.touch()
                failure.unlink()
                child.stdin.write(json.dumps(dict(v=2, id="retry", method="shutdown",
                    params={}, context=child.session_context)) + "\n")
                child.stdin.flush()
                child.stdin.close()
                deadline = time.monotonic() + 2
                while not (folder / "closing").exists():
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.01)
                hold.unlink()
                reply = json.loads(child.stdout.readline())
                self.assertEqual(reply["id"], "retry")
                self.assertEqual(reply["result"]["unreleased"], [])
                self.assertEqual(child.wait(timeout=2), 2, "original ingress fault remains diagnostic")
            finally:
                if hold.exists(): hold.unlink()
                if child.poll() is None:
                    child.terminate()  # This test-created transport-injected peer only.
                    child.wait(timeout=3)
                if not child.stdin.closed: child.stdin.close()
                child.stdout.close()
                child.stderr.close()

    def test_duplicate_fault_failed_cleanup_keeps_status_and_explicit_retry_channel(self):
        self._fault_cleanup_channel("connect")

    def test_callback_fault_failed_cleanup_keeps_status_and_explicit_retry_channel(self):
        self._fault_cleanup_channel("callback-fault")

    def test_publication_fault_failed_cleanup_keeps_status_and_explicit_retry_channel(self):
        self._fault_cleanup_channel("publication-fault")

    def _fault_cleanup_channel(self, fault_id):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            failure = folder / "fail"
            failure.touch()
            child = self._cleanup_peer(directory)
            reader = None
            try:
                self.assertTrue(self._request(child, dict(v=2, id="connect", method="connect",
                    params=dict(role="osa", resource="GPIB0::29::INSTR", acknowledge_lifecycle=True)))["ok"])
                frames = queue.Queue()
                reader = threading.Thread(target=lambda: [frames.put(json.loads(line)) for line in child.stdout])
                reader.start()
                def send(id, method):
                    child.stdin.write(json.dumps(dict(v=2, id=id, method=method, params={},
                        context=child.session_context)) + "\n")
                    child.stdin.flush()
                send(fault_id, "status")  # "connect" reuses a recent ID; no second terminal is legal.
                if fault_id != "connect":
                    terminal = frames.get(timeout=2)
                    self.assertEqual(terminal["id"], fault_id)
                    self.assertEqual(terminal["phase"], "completed_readback_failed" if
                                     fault_id == "callback-fault" else "completed")
                deadline = time.monotonic() + 2
                while not (folder / "cleanup-ready").exists():
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.01)
                send("diagnostic", "status")
                try:
                    status = frames.get(timeout=2)
                except queue.Empty:
                    self.fail("healthy pipes lost diagnostic channel after protocol fault")
                self.assertEqual(status["id"], "diagnostic")
                self.assertTrue(status["ok"], "healthy pipes must retain status: " + str(status))
                first = status["result"]["last_cleanup"]
                self.assertEqual(first["unreleased"], ["osa"])
                self.assertEqual((folder / "attempts").read_text().splitlines(), ["close"])
                failure.unlink()
                retry_id = "shutdown-publication-fault" if fault_id == "publication-fault" else "explicit-retry"
                send(retry_id, "shutdown")
                retry = frames.get(timeout=2)
                self.assertEqual(retry["id"], retry_id)
                self.assertEqual(retry["result"]["unreleased"], [])
                self.assertNotEqual(first["attempt_id"], retry["result"]["attempt_id"])
                self.assertEqual(child.wait(timeout=3), 2, "fault remains diagnostic, stdin stays open")
                self.assertEqual((folder / "attempts").read_text().splitlines(), ["close", "close"])
                self.assertNotIn("input unavailable", child.stderr.read())
            finally:
                if child.poll() is None:
                    child.terminate()  # Only this test-created transport-injected peer.
                    child.wait(timeout=3)
                child.stdin.close()
                if reader: reader.join(3)
                child.stdout.close()
                child.stderr.close()

    def test_rejected_stale_shutdown_does_not_replace_eof_cleanup_responsibility(self):
        child = self._launch()
        try:
            self.assertTrue(self._request(child, dict(v=2, id="connect", method="connect",
                params=dict(role="voltage", resource="COM990", acknowledge_lifecycle=True)))["ok"])
            child.stdin.write(json.dumps(dict(v=2, id="stale-stop", method="shutdown", params={},
                context=dict(session_id="retired-session", connection_id=None, epoch=0))) + "\n")
            child.stdin.flush()
            reply = json.loads(child.stdout.readline())
            self.assertEqual(reply["phase"], "rejected_before_call")
            child.stdin.close()
            self.assertEqual(child.wait(timeout=2), 0, "EOF still owns a fresh coordinated cleanup")
        finally:
            if child.poll() is None:
                child.terminate()  # This test-created transport-injected worker only.
                child.wait(timeout=3)
            if not child.stdin.closed: child.stdin.close()
            child.stdout.close()
            child.stderr.close()

    def test_eof_after_failed_shutdown_retains_without_automatic_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "fail").touch()
            child = self._cleanup_peer(directory)
            try:
                self.assertTrue(self._request(child, dict(v=2, id="connect", method="connect",
                    params=dict(role="osa", resource="GPIB0::29::INSTR", acknowledge_lifecycle=True)))["ok"])
                first = self._request(child, dict(v=2, id="close1", method="shutdown", params={}))
                self.assertEqual(first["result"]["unreleased"], ["osa"])
                before = (Path(directory) / "attempts").read_text()
                child.stdin.close()
                diagnostic = child.stderr.readline()
                self.assertIn("retained wait", diagnostic)
                self.assertIn("input unavailable", diagnostic)
                self.assertIsNone(child.poll())
                self.assertEqual((Path(directory) / "attempts").read_text(), before,
                    "EOF must not automatically retry a failed explicit attempt")
            finally:
                # This test-created transport-injected peer deliberately retains ownership.
                # Termination here tests EOF diagnostics, never touches a production process.
                child.terminate()
                child.wait(timeout=5)
                if not child.stdin.closed: child.stdin.close()
                child.stdout.close()
                child.stderr.close()

    def test_startup_is_disconnected_and_actions_are_correlated(self) -> None:
        child = self._launch()
        try:
            ping = self._request(child, {"v": 2, "id": "1", "method": "ping", "params": {}})
            self.assertEqual(ping["id"], "1")
            self.assertTrue(ping["ok"])
            self.assertIn("python_executable", ping["result"])
            status = self._request(child, {"v": 2, "id": "2", "method": "status", "params": {}})
            self.assertEqual(status["result"]["devices"], {})
            connected = self._request(child, {"v": 2, "id": "3", "method": "connect",
                                              "params": {"role": "fiber"}})
            self.assertEqual(connected["result"]["status"]["right"]["serial_number"], "160721175410")
            denied = self._request(child, {"v": 2, "id": "4", "method": "action", "params": {
                "role": "fiber", "name": "move", "side": "left", "dx": 0.1,
            }})
            self.assertFalse(denied["ok"])
            self.assertEqual(denied["id"], "4")
            stopped = self._request(child, {"v": 2, "id": "5", "method": "shutdown", "params": {}})
            self.assertTrue(stopped["ok"])
            self.assertEqual(stopped["result"]["unreleased"], [])
            self.assertEqual(child.wait(timeout=5), 0, "successful shutdown must exit with stdin open")
        finally:
            if child.stdin:
                child.stdin.close()
            child.wait(timeout=5)
            if child.stdout:
                child.stdout.close()
            if child.stderr:
                child.stderr.close()
        self.assertEqual(child.returncode, 0)

    def test_malformed_request_gets_error_and_next_request_still_works(self) -> None:
        child = self._launch()
        try:
            assert child.stdin is not None and child.stdout is not None
            child.stdin.write('{"v":1,"id":"broken","method":\n')
            child.stdin.flush()
            bad = json.loads(child.stdout.readline())
            self.assertEqual((bad["id"], bad["ok"]), ("invalid", False))
            good = self._request(child, {"v": 2, "id": "good", "method": "status", "params": {}})
            self.assertEqual(good["id"], "good")
            self.assertTrue(good["ok"])
        finally:
            if child.stdin:
                child.stdin.close()
            child.wait(timeout=5)
            if child.stdout:
                child.stdout.close()
            if child.stderr:
                child.stderr.close()
        self.assertEqual(child.returncode, 0)

    def test_eof_runs_orderly_cleanup_after_transport_injected_output_connection(self) -> None:
        child = self._launch()
        try:
            connected = self._request(child, {
                "v": 2, "id": "connect-voltage", "method": "connect", "params": {
                    "role": "voltage", "resource": "COM990",
                    "acknowledge_lifecycle": True,
                },
            })
            self.assertTrue(connected["ok"])
            assert child.stdin is not None
            child.stdin.close()  # EOF takes the worker through its finally cleanup.
            self.assertEqual(child.wait(timeout=5), 0)
            assert child.stdout is not None and child.stderr is not None
            self.assertEqual(child.stdout.read(), "", "EOF must not invent a cleanup response")
            self.assertEqual(child.stderr.read(), "")
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
            if child.stdin and not child.stdin.closed:
                child.stdin.close()
            if child.stdout:
                child.stdout.close()
            if child.stderr:
                child.stderr.close()

    def test_forced_transport_injected_child_exit_during_pending_call_has_no_success_reply(self) -> None:
        child = self._launch()
        try:
            connected = self._request(child, {
                "v": 2, "id": "connect-gain", "method": "connect", "params": {
                    "role": "gain", "resource": "COM991", "acknowledge_lifecycle": True,
                },
            })
            self.assertTrue(connected["ok"])
            assert child.stdin is not None
            child.stdin.write(json.dumps({
                "v": 2, "id": "pending", "method": "action", "context": child.contexts["gain"], "params": {
                    "role": "gain", "name": "wait_stable", "timeout_s": 60,
                },
            }) + "\n")
            child.stdin.flush()
            time.sleep(0.15)  # TEC-off cannot satisfy the real stability interlock.
            self.assertIsNone(child.poll())
            child.terminate()  # Only the transport-injected subprocess created by this test.
            child.wait(timeout=5)
            assert child.stdout is not None
            replies = [json.loads(line) for line in child.stdout.read().splitlines()]
            self.assertFalse(any(reply.get("id") == "pending" and reply.get("ok")
                                 for reply in replies))
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
            if child.stdin:
                child.stdin.close()
            if child.stdout:
                child.stdout.close()
            if child.stderr:
                child.stderr.close()


if __name__ == "__main__":
    unittest.main()
