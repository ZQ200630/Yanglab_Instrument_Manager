"""Exercise the configured bundle resources without Tauri or real hardware."""

from __future__ import annotations

import json
import glob
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch
from pathlib import Path


TAURI_ROOT = Path(__file__).resolve().parents[1] / "src-tauri"
CONFIG = json.loads((TAURI_ROOT / "tauri.conf.json").read_text(encoding="utf-8"))
HANDSHAKE_TIMEOUT = 15


def _stage_resources(root: Path, *, omit_code: bool = False) -> None:
    for source_name, destination_name in CONFIG["bundle"]["resources"].items():
        if omit_code and destination_name.startswith("Code/"):
            continue
        if "*" in source_name:
            # Tauri ResourcePaths places each globbed basename under its map
            # destination; it does not preserve the wildcard's source prefix.
            sources = sorted(glob.glob(str(TAURI_ROOT / source_name)))
            if not sources:
                raise AssertionError(f"empty resource glob: {source_name}")
            for item in sources:
                source = Path(item)
                target = (root / destination_name / source.name).resolve()
                if not target.is_relative_to(root.resolve()) or not source.is_file():
                    raise AssertionError(f"invalid resource glob item: {source}")
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, target)
            continue
        source = (TAURI_ROOT / source_name).resolve()
        target = (root / destination_name).resolve()
        if not target.is_relative_to(root.resolve()):
            raise AssertionError(f"bundle target escapes staging root: {destination_name}")
        if source.is_dir():
            shutil.copytree(source, target, dirs_exist_ok=True)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)


def _run_staged_worker(root: Path) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(root)
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    child = subprocess.Popen(
        [sys.executable, "-B", "-m", "App.worker.main", "--real",
         "--settings", str(root / "settings.json")],
        cwd=root, env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True)
    first_frames = queue.Queue(maxsize=1)
    reader = threading.Thread(target=lambda: first_frames.put(child.stdout.readline()),
                              name='package-first-frame')
    reader.start()
    try:
        child.stdin.write(json.dumps(dict(v=2, id="package-ping", method="ping", params={}, context=None)) + "\n")
        child.stdin.flush()
        try:
            first = first_frames.get(timeout=HANDSHAKE_TIMEOUT)
        except queue.Empty as error:
            raise TimeoutError('isolated real worker handshake timed out') from error
        if first:
            identity = json.loads(first)
            for id, method in (("package-status", "status"), ("package-stop", "shutdown")):
                child.stdin.write(json.dumps(dict(v=2, id=id, method=method, params={},
                    context=identity["context"])) + "\n")
            child.stdin.flush()
        child.wait(timeout=15)
        return subprocess.CompletedProcess(child.args, child.returncode,
            first + child.stdout.read(), child.stderr.read())
    finally:
        try:
            child.stdin.close()
        except BrokenPipeError:
            pass
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            # This helper only owns a staged --real child, never a bench worker.
            child.terminate()
            child.wait(timeout=3)
        reader.join(timeout=3)
        child.stdout.close()
        child.stderr.close()


class PackageLayoutTests(unittest.TestCase):
    def test_vendor_serial_driver_files_are_available_in_an_isolated_package(self):
        with tempfile.TemporaryDirectory(prefix='yang-serial-drivers-') as temporary:
            bundle = Path(temporary)
            _stage_resources(bundle)
            for name in ('drivers/ch340/CH341SER.INF', 'drivers/ch340/CH341SER.CAT',
                         'drivers/ch340/CH341S64.sys', 'drivers/cp210x/silabser.inf',
                         'drivers/cp210x/silabser.cat', 'drivers/cp210x/x64/silabser.sys',
                         'drivers/cp210x/SLAB_License_Agreement_VCP_Windows.txt'):
                self.assertTrue((bundle / name).is_file(), name)
                self.assertEqual((bundle / name).read_bytes(),
                                 (TAURI_ROOT.parent / name).read_bytes())

    def test_release_resources_exclude_test_experiments_and_bytecode(self):
        with tempfile.TemporaryDirectory(prefix="yang-package-inventory-") as temporary:
            bundle = Path(temporary)
            _stage_resources(bundle)
            self.assertFalse((bundle / "Code" / "Debugs").exists())
            self.assertFalse((bundle / "Code" / "Experiments").exists())
            self.assertFalse(any(bundle.rglob("*.pyc")))
            self.assertTrue((bundle / "Code" / "Utils" / "osa.py").is_file())
            self.assertTrue((bundle / "Code" / "Setups" / "fiber_coupling.py").is_file())

    def test_silent_live_handshake_times_out_closes_stdin_and_reaps_disconnected_real_child(self):
        # A deadline must not depend on a first newline. This child imports no
        # project code, observes EOF, and has a last-resort test-only watchdog.
        children = []
        real_popen = subprocess.Popen
        with tempfile.TemporaryDirectory(prefix='sil-package-silent-') as temporary:
            root = Path(temporary)
            source = (
                'import os, sys, threading\n'
                'from pathlib import Path\n'
                'watchdog = threading.Timer(2, lambda: os._exit(7))\n'
                'watchdog.start()\n'
                'sys.stdin.read()\n'
                'Path("eof").touch()\n'
                'watchdog.cancel()\n'
            )
            def launch(_args, **kwargs):
                child = real_popen([sys.executable, '-B', '-u', '-c', source], **kwargs)
                children.append(child)
                return child
            with patch(__name__ + '.subprocess.Popen', side_effect=launch), \
                    patch(__name__ + '.HANDSHAKE_TIMEOUT', 0.2, create=True):
                with self.assertRaisesRegex(TimeoutError, 'handshake'):
                    _run_staged_worker(root)
            self.assertTrue((root / 'eof').exists(), 'failure cleanup must close stdin')
            self.assertEqual(children[0].poll(), 0, 'failure cleanup must reap the child')
            self.assertTrue(all(stream.closed for stream in (
                children[0].stdin, children[0].stdout, children[0].stderr)))

    def test_configured_resources_start_isolated_disconnected_real_worker(self) -> None:
        with tempfile.TemporaryDirectory(prefix="sil-package-smoke-") as temporary:
            bundle = Path(temporary) / "bundle"
            bundle.mkdir()
            _stage_resources(bundle)
            self.assertTrue((bundle / "Config" / "fiber_coupling.json").is_file())
            completed = _run_staged_worker(bundle)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            replies = [json.loads(line) for line in completed.stdout.splitlines()]
            by_id = {item["id"]: item for item in replies}
            self.assertEqual(set(by_id), {"package-ping", "package-status", "package-stop"})
            self.assertTrue(all(item["ok"] for item in replies), completed.stdout)
            identity = by_id["package-ping"]["result"]
            self.assertEqual(Path(identity["project_root"]).resolve(), bundle.resolve())
            self.assertEqual(Path(identity["python_executable"]).resolve(),
                             Path(sys.executable).resolve())
            self.assertEqual(identity["environment_name"].casefold(), "visa")
            self.assertEqual(identity["mode"], "real")
            self.assertEqual(by_id["package-status"]["result"]["devices"], {})
            self.assertEqual(by_id["package-stop"]["result"]["unreleased"], [])

    def test_missing_code_resource_cannot_import_from_source_checkout(self) -> None:
        with tempfile.TemporaryDirectory(prefix="sil-package-negative-") as temporary:
            bundle = Path(temporary) / "bundle"
            bundle.mkdir()
            _stage_resources(bundle, omit_code=True)
            completed = _run_staged_worker(bundle)
            self.assertNotEqual(completed.returncode, 0)
            self.assertEqual(completed.stdout, "")
            self.assertIn("No module named 'Code'", completed.stderr)


if __name__ == "__main__":
    unittest.main()
