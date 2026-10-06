"""Static preview rejects instrument control; injected tests preserve worker contracts."""

from __future__ import annotations

import unittest
from copy import deepcopy
import json
import threading
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict
from unittest.mock import patch
from App.tests.driver_fixture import WireOSA, factories, fiber_factory, FakePort
from App.worker.controller import ConsoleController

from App.tests.preview_server import PreviewBridge, make_server


class PreviewBridgeTests(unittest.TestCase):
    def bridge(self):
        ports = [FakePort('COM992', '2110148249-10')]
        sources = factories()
        sources['fiber'] = lambda: fiber_factory(ports)(('2110148249-10',))
        return PreviewBridge(controller_factory=lambda: ConsoleController(
            factories=sources, port_enumerator=lambda: ports))

    def test_default_browser_preview_cannot_start_instrument_control(self):
        bridge = PreviewBridge()
        with self.assertRaisesRegex(ValueError, 'Native App'):
            bridge.invoke('worker_start', {'config': {}})
        self.assertIsNone(bridge._controller)

    def test_same_resource_reconnect_rejects_old_wire_context(self):
        bridge = self.bridge()
        bridge.invoke('worker_start', {'config': {}})
        owner = bridge._controller
        def request(request_id, method, context, **params):
            return bridge.invoke('worker_request', {'request': dict(v=2, id=request_id,
                method=method, params=params, context=asdict(context))})
        try:
            self.assertTrue(request('first-connect', 'connect', owner.context('voltage'),
                role='voltage', resource='COM990', acknowledge_lifecycle=True)['ok'])
            old = owner.context('voltage')
            self.assertTrue(request('disconnect', 'disconnect', old, role='voltage')['ok'])
            self.assertTrue(request('reconnect', 'connect', owner.context('voltage'),
                role='voltage', resource='COM990', acknowledge_lifecycle=True)['ok'])
            current = owner.context('voltage')
            self.assertNotEqual(old.connection_id, current.connection_id)
            for request_id, name in (('old-write', 'set_channel'), ('old-zero', 'zero')):
                params = dict(role='voltage', name=name)
                if name == 'set_channel': params.update(channel=1, voltage=0.1)
                denied = request(request_id, 'action', old, **params)
                self.assertEqual(denied['phase'], 'rejected_before_call')
                self.assertFalse(denied['ok'])
                self.assertEqual(denied['id'], request_id)
            self.assertEqual(owner.context('voltage'), current)
            self.assertTrue(request('current-write', 'action', current, role='voltage',
                name='set_channel', channel=1, voltage=0.1)['ok'])
            self.assertEqual(owner.cached_status()['devices']['voltage']['requested_voltage_v'][0], .1)
            from Code.Utils.voltage import encode_voltages
            self.assertEqual(owner._devices['voltage'].test_wire.writes[-1], encode_voltages([.1] + [0.] * 7))
        finally:
            bridge.shutdown()

    def test_backend_selector_is_rejected_and_injected_controller_starts_disconnected(self) -> None:
        bridge = self.bridge()
        with self.assertRaisesRegex(ValueError, "no backend configuration"):
            bridge.invoke("worker_start", {"config": {"mode": "real", "pythonPath": "ignored"}})
        bridge.invoke("worker_start", {"config": {}})
        status = bridge.invoke("worker_request", {"request": {
            "v": 2, "id": "status-1", "method": "status", "params": {}, "context": None,
        }})
        self.assertEqual(status["result"]["devices"], {})
        bridge.shutdown()

    def test_injected_bridge_preserves_controller_safety_boundary(self) -> None:
        bridge = self.bridge()
        bridge.invoke("worker_start", {"config": {}})
        context = bridge._controller.context("fiber")
        from dataclasses import asdict
        connected = bridge.invoke("worker_request", {"request": {
            "v": 2, "id": "connect-1", "method": "connect", "params": {"role": "fiber"}, "context": asdict(context),
        }})
        self.assertTrue(connected["ok"])
        self.assertEqual(connected["result"]["status"]["left"]["serial_number"], "2110148249-10")
        denied = bridge.invoke("worker_request", {"request": {
            "v": 2, "id": "move-1", "method": "action", "context": connected["context"],
            "params": {"role": "fiber", "name": "move", "side": "left", "dx": 0.1},
        }})
        self.assertFalse(denied["ok"])
        self.assertEqual(denied["id"], "move-1")
        report = bridge.invoke("worker_stop", {})
        self.assertEqual(report["unreleased"], [])

    def test_preview_waits_outside_owner_lock_and_retains_failed_cleanup_owner(self):
        entered, release = threading.Event(), threading.Event()
        class BlockingOSA(WireOSA):
            fail_close = True
            def acquire(self, trace="A"):
                entered.set()
                if not release.wait(3): raise TimeoutError("test release missing")
                return super().acquire(trace)
            def close(self):
                if self.fail_close: raise OSError("fixture retained resource")
                super().close()
        bridge = self.bridge()
        with patch("App.worker.controller.ConsoleController._factory",
                   side_effect=AssertionError("startup must not resolve any factory")):
            identity = bridge.invoke("worker_start", {"config": {}})
        self.assertFalse(identity["connected"])
        owner = bridge._controller
        owner._factories["osa"] = BlockingOSA
        serial = 0
        def request(method, **params):
            nonlocal serial
            serial += 1
            role = params.get("role")
            context = asdict(owner.context(role)) if role else {
                "session_id": identity["session_id"], "connection_id": None, "epoch": 0}
            return bridge.invoke("worker_request", {"request": {
                "v": 2, "id": str(serial), "method": method, "params": params, "context": context}})
        try:
            self.assertTrue(request("connect", role="osa", resource="GPIB0::29::INSTR", acknowledge_lifecycle=True)["ok"])
            with ThreadPoolExecutor(max_workers=2) as executor:
                acquiring = executor.submit(request, "action", role="osa", name="acquire")
                self.assertTrue(entered.wait(1))
                status = executor.submit(request, "status").result(1)
                self.assertTrue(status["ok"])
                self.assertFalse(acquiring.done())
                release.set()
                self.assertTrue(acquiring.result(2)["ok"])
            failed = bridge.invoke("worker_stop", {})
            first_report = deepcopy(failed)
            self.assertEqual(failed["unreleased"], ["osa"])
            self.assertIs(bridge._controller, owner)
            with self.assertRaisesRegex(ValueError, "already running"):
                bridge.invoke("worker_start", {"config": {}})
            self.assertEqual(request("status")["result"]["last_cleanup"], failed)
            BlockingOSA.fail_close = False
            self.assertEqual(bridge.invoke("worker_stop", {})["unreleased"], [])
            self.assertIsNone(bridge._controller)
            self.assertEqual(failed["unreleased"], ["osa"])
        finally:
            release.set()
            BlockingOSA.fail_close = False
            bridge.shutdown()
        self.assertEqual(failed, first_report)

    def test_http_preview_serves_frontend_and_bounded_injected_controller(self) -> None:
        server = make_server("127.0.0.1", 0)
        server.bridge = self.bridge()
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        base = f"http://127.0.0.1:{server.server_port}"

        def invoke(command, args):
            request = urllib.request.Request(
                base + "/__preview_invoke", method="POST",
                data=json.dumps({"command": command, "args": args}).encode("utf-8"),
                headers={"Content-Type": "application/json"},
            )
            with urllib.request.urlopen(request, timeout=3) as response:
                return json.load(response)

        try:
            with urllib.request.urlopen(base + "/", timeout=3) as response:
                html = response.read().decode("utf-8")
            self.assertLess(html.index("preview-bridge.js"), html.index('src="./main.js"'))
            with urllib.request.urlopen(base + "/preview-bridge.js", timeout=3) as response:
                self.assertIn("__TAURI__", response.read().decode("utf-8"))
            invoke("worker_start", {"config": {}})
            status = invoke("worker_request", {"request": {
                "v": 2, "id": "status-http", "method": "status", "params": {}, "context": None,
            }})
            self.assertEqual(status["result"]["devices"], {})
            invoke("worker_stop", {})
        finally:
            server.shutdown()
            server.server_close()
            server.bridge.shutdown()
            thread.join(timeout=3)


if __name__ == "__main__":
    unittest.main()
