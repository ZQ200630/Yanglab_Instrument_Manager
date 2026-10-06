"""Offline subprocess acceptance for the isolated native close-failure peer."""

from __future__ import annotations

import ast
import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


FIXTURE = Path(__file__).with_name("native_close_fixture.py")


def request(method, **params):
    return {"v": 2, "id": method, "method": method, "params": params,
            "context": {"session_id": "native-close-fixture", "connection_id": None, "epoch": 0}}


class NativeCloseFixtureTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(FIXTURE.is_file(), "native close-failure fixture is missing")
        temporary = tempfile.TemporaryDirectory(prefix="sil-native-close-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        (self.root / "App/worker").mkdir(parents=True)
        (self.root / "Code/Utils").mkdir(parents=True)
        for path in ("App/__init__.py", "App/worker/__init__.py", "Code/Utils/osa.py"):
            (self.root / path).write_text("# Isolated pipe-only fixture.\n", encoding="utf-8")
        shutil.copyfile(FIXTURE, self.root / "App/worker/main.py")
        # Fail before any real driver import, even if a dependency is added later.
        trap = "raise AssertionError('hardware import attempted by pipe fixture')\n"
        for path in ("Code/__init__.py", "serial.py", "pyvisa.py", "visa.py"):
            (self.root / path).write_text(trap, encoding="utf-8")

    def run_peer(self, requests=(), arguments=("--real", "--protocol", "2")):
        environment = os.environ.copy()
        environment["PYTHONPATH"] = str(self.root)
        environment["PYTHONDONTWRITEBYTECODE"] = "1"
        # -S removes third-party site packages; the native host uses the same
        # VISA interpreter and module entry point without this extra test guard.
        completed = subprocess.run(
            [sys.executable, "-S", "-u", "-B", "-m", "App.worker.main", *arguments],
            cwd=self.root, env=environment, text=True, capture_output=True,
            input="".join(json.dumps(item) + "\n" for item in requests),
            timeout=10, check=False,
        )
        replies = [json.loads(line) for line in completed.stdout.splitlines()]
        for reply in replies:
            self.assertEqual(reply["v"], 2)
            self.assertIs(type(reply["ok"]), bool)
            self.assertEqual(set(reply), {"v", "id", "ok", "phase", "context", "result" if reply["ok"] else "error"})
        return completed, replies

    def test_native_startup_identity_settings_and_disconnected_status(self):
        completed, replies = self.run_peer(map(request, ("ping", "settings_get", "status")))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stderr, "")
        self.assertEqual([reply["id"] for reply in replies], ["ping", "settings_get", "status"])
        self.assertTrue(all(reply["ok"] for reply in replies))
        identity = replies[0]["result"]
        self.assertEqual(identity["mode"], "real")
        self.assertIs(identity["connected"], False)
        self.assertEqual(identity["protocol_version"], 2)
        self.assertEqual(Path(identity["python_executable"]).resolve(), Path(sys.executable).resolve())
        self.assertEqual(Path(identity["project_root"]).resolve(), self.root)
        self.assertEqual(identity["environment_name"].lower(), "visa")
        self.assertEqual(replies[1]["result"], {
            "version": 1, "python_path": identity["python_executable"],
            "bindings": {"osa": None, "pm400": None, "voltage": None, "gain": None},
        })
        status = replies[2]["result"]
        self.assertEqual(status["mode"], "real")
        self.assertEqual(status["devices"], {})
        self.assertIsNone(status["last_cleanup"])
        self.assertIsNotNone(datetime.datetime.fromisoformat(status["observed_at"]).tzinfo)

    def test_shutdown_flushes_failed_fixture_report_then_exits_19(self):
        completed, replies = self.run_peer([request("shutdown"), request("status")])
        self.assertEqual(completed.returncode, 19, completed.stderr)
        self.assertEqual(completed.stderr, "")
        self.assertEqual(len(replies), 1, "no requests may run after shutdown")
        self.assertEqual(replies[0]["id"], "shutdown")
        self.assertTrue(replies[0]["ok"])
        report = replies[0]["result"]
        self.assertEqual(report["unreleased"], ["pipe_fixture"])
        self.assertIsNone(report["voltage_zero"])
        self.assertEqual(len(report["steps"]), 1)
        self.assertEqual(report["steps"][0]["role"], "pipe_fixture")
        self.assertEqual(report["steps"][0]["action"], "close")
        self.assertIs(report["steps"][0]["ok"], False)
        self.assertIn("fixture", report["steps"][0]["error"].lower())

    def test_only_exact_real_protocol_argument_is_allowed(self):
        for arguments in (("--sim", "--protocol", "2"), ("--real", "--protocol", "1"), (), ("--real",), ("--real", "--real"),
                          ("--real", "--real"), ("--real", "--unknown")):
            with self.subTest(arguments=arguments):
                completed, replies = self.run_peer([request("ping")], arguments)
                self.assertNotEqual(completed.returncode, 0)
                self.assertEqual(replies, [])
                self.assertIn("--real", completed.stderr)

    def test_all_device_methods_and_settings_writes_are_denied(self):
        methods = ("inventory", "connect", "disconnect", "action", "settings_save", "unknown")
        completed, replies = self.run_peer(
            [request(method, role="voltage", resource="COM1") for method in methods]
            + [request("status")]
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stderr, "")
        self.assertEqual([reply["id"] for reply in replies], [*methods, "status"])
        for reply in replies[:-1]:
            self.assertFalse(reply["ok"])
            self.assertIsInstance(reply["error"]["type"], str)
            self.assertIsInstance(reply["error"]["message"], str)
        self.assertEqual(replies[-1]["result"]["devices"], {})
        self.assertFalse((self.root / "settings.json").exists())

    def test_eof_exits_cleanly_without_fabricating_cleanup(self):
        completed, replies = self.run_peer()
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stderr, "")
        self.assertEqual(replies, [])

    def test_fixture_import_boundary_is_stdlib_only_without_dynamic_execution(self):
        tree = ast.parse(FIXTURE.read_text(encoding="utf-8"))
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                self.assertTrue(all(alias.name in {"sys", "json", "datetime"} for alias in node.names))
            elif isinstance(node, ast.ImportFrom):
                self.assertEqual(node.level, 0)
                self.assertEqual(node.module, "pathlib")
                self.assertEqual([alias.name for alias in node.names], ["Path"])
            elif isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
                self.assertNotIn(node.func.id, {"eval", "exec", "compile", "__import__"})


if __name__ == "__main__":
    unittest.main()
