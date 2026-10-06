"""Offline contract tests for the desktop worker's wire and inventory boundary."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from App.worker.discovery import discover
from App.worker.protocol import ProtocolError, encode_v2, parse_v2
from App.worker.contracts import Outcome
from App.worker.settings import load_settings, save_settings


class ProtocolTests(unittest.TestCase):
    def test_request_version_id_and_params_are_validated(self) -> None:
        request = parse_v2('{"v":2,"id":"req-1","method":"ping","params":{},"context":null}')
        self.assertEqual((request.id, request.method, request.params), ("req-1", "ping", {}))
        for line in (
            '{"v":1,"id":"req-1","method":"ping","params":{},"context":null}',
            '{"v":2,"id":"req-1","id":"req-2","method":"ping","params":{},"context":null}',
            '{"v":2,"id":true,"method":"ping","params":{},"context":null}',
            '{"v":2,"id":"req-1","method":"ping","params":{"x":NaN},"context":null}',
            '{"v":2,"id":"req-1","method":"ping","params":{},"context":null,"code":"print(1)"}',
        ):
            with self.subTest(line=line), self.assertRaises(ProtocolError):
                parse_v2(line)

    def test_response_is_one_strict_json_line(self) -> None:
        encoded = encode_v2("req-1", Outcome("completed", None, {"connected": False}))
        self.assertTrue(encoded.endswith("\n"))
        self.assertEqual(
            json.loads(encoded),
            {"v": 2, "id": "req-1", "ok": True, "phase": "completed", "context": None, "result": {"connected": False}},
        )
        with self.assertRaises(ProtocolError):
            encode_v2("req-1", Outcome("completed", None, {}, {"type": "Failure"}))


class InventoryTests(unittest.TestCase):
    def test_discovery_uses_metadata_and_closes_visa_manager(self) -> None:
        ports = (
            SimpleNamespace(device="COM6", vid=0x1313, pid=0x1003,
                            serial_number="2110148249-10", description="MDT693B", manufacturer="Thorlabs"),
            SimpleNamespace(device="COM7", vid=0x1313, pid=0x1003,
                            serial_number="160721175410", description="MDT693B", manufacturer="Thorlabs"),
            SimpleNamespace(device="COM4", vid=0x1A86, pid=0x7523,
                            serial_number=None, description="USB Serial", manufacturer="wch.cn"),
            SimpleNamespace(device="COM5", vid=0x10C4, pid=0xEA60,
                            serial_number="GAIN-1", description="CP210x", manufacturer="Silicon Labs"),
        )

        class Manager:
            closed = False

            def list_resources(self):
                return ("GPIB0::4::INSTR", "USB0::0x1313::0x8078::P1::INSTR")

            def close(self):
                self.closed = True

        manager = Manager()
        inventory = discover(port_enumerator=lambda: ports, resource_manager_factory=lambda: manager)
        self.assertTrue(manager.closed)
        self.assertEqual(inventory["fiber"]["left"]["resource"], "COM6")
        self.assertEqual(inventory["fiber"]["right"]["resource"], "COM7")
        self.assertEqual(inventory["suggestions"]["voltage"], ["COM4"])
        self.assertEqual(inventory["suggestions"]["gain"], ["COM5"])
        self.assertIn("GPIB0::4::INSTR", inventory["visa"])

    def test_visa_enumeration_error_does_not_hide_serial_metadata(self) -> None:
        class Manager:
            closed = False

            def list_resources(self):
                raise OSError("backend unavailable")

            def close(self):
                self.closed = True

        manager = Manager()
        inventory = discover(port_enumerator=lambda: (), resource_manager_factory=lambda: manager)
        self.assertTrue(manager.closed)
        self.assertEqual(inventory["visa"], [])
        self.assertIn("backend unavailable", inventory["errors"]["visa"])

    def test_registered_mdt_is_not_suggested_as_gain_even_with_cp210x_id(self) -> None:
        ports = (
            SimpleNamespace(device="COM6", vid=0x10C4, pid=0xEA60,
                            serial_number="2110148249-10", description="MDT693B",
                            manufacturer="Thorlabs"),
            SimpleNamespace(device="COM5", vid=0x10C4, pid=0xEA60,
                            serial_number="GAIN-1", description="CP210x",
                            manufacturer="Silicon Labs"),
        )

        class Manager:
            def list_resources(self):
                return ()

            def close(self):
                pass

        inventory = discover(port_enumerator=lambda: ports,
                             resource_manager_factory=Manager)
        self.assertEqual(inventory["fiber"]["left"]["resource"], "COM6")
        self.assertEqual(inventory["suggestions"]["gain"], ["COM5"])

    def test_unregistered_mdt_candidate_is_not_suggested_as_gain(self) -> None:
        port = SimpleNamespace(device="COM8", vid=0x10C4, pid=0xEA60,
                               serial_number="OTHER-MDT", description="MDT693B",
                               manufacturer="Thorlabs", product="MDT693B")

        class Manager:
            def list_resources(self):
                return ()

            def close(self):
                pass

        inventory = discover(port_enumerator=lambda: (port,),
                             resource_manager_factory=Manager)
        self.assertEqual(inventory["fiber"]["unknown"][0]["resource"], "COM8")
        self.assertEqual(inventory["suggestions"]["gain"], [])


class SettingsTests(unittest.TestCase):
    def test_unversioned_bindings_migrate_but_future_schema_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "console.json"
            path.write_text(json.dumps({
                "bindings": {"osa": "GPIB0::4::INSTR", "gain": "COM5"},
            }), encoding="utf-8")
            migrated = load_settings(path)
            self.assertEqual(migrated["version"], 1)
            self.assertEqual(migrated["bindings"]["gain"], "COM5")

            path.write_text(json.dumps({"version": 2, "bindings": {"gain": "COM5"}}),
                            encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "version"):
                load_settings(path)
            with self.assertRaisesRegex(ValueError, "version"):
                save_settings(path, {"version": 2, "bindings": {"gain": "COM5"}})

    def test_settings_persist_only_role_bindings(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "console.json"
            save_settings(path, {
                "python_path": "D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe",
                "bindings": {"osa": "GPIB0::4::INSTR", "voltage": "COM4"},
                "armed": True,
                "estimated_position_um": {"left": [1, 2, 3]},
            })
            saved = load_settings(path)
            self.assertEqual(saved["bindings"]["osa"], "GPIB0::4::INSTR")
            self.assertNotIn("armed", saved)
            self.assertNotIn("estimated_position_um", saved)
            self.assertEqual(json.loads(path.read_text(encoding="utf-8")), saved)

    def test_missing_settings_are_disconnected_defaults(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            saved = load_settings(Path(directory) / "missing.json")
        self.assertEqual(saved["bindings"]["osa"], "GPIB0::4::INSTR")
        self.assertIsNone(saved["bindings"]["voltage"])
        self.assertNotIn("connected", saved)


if __name__ == "__main__":
    unittest.main()
