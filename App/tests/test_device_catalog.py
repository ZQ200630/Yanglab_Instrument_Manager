"""Trusted model/profile validation, without importing hardware factories."""
from __future__ import annotations

import copy
import json
import tempfile
import unittest
from pathlib import Path

try:
    from App.worker import catalog as api
except ImportError:
    api = None


CATALOG_PATH = Path(__file__).resolve().parents[1] / "catalog" / "devices.json"


class DeviceCatalogTests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(api, "The trusted device catalog API is not implemented")
        self.catalog = api.load_catalog(CATALOG_PATH)

    def test_profile_does_not_invent_transports(self):
        profile = self.catalog.profile("aq6370", "gpib-visa")
        self.assertEqual(profile.access, "visa")
        self.assertEqual(profile.validate({"resource": "GPIB0::4::INSTR"})["resource"],
                         "GPIB0::4::INSTR")
        for model, profile_id in (("aq6370", "tcpip-visa"), ("gain", "usbtmc")):
            with self.assertRaises(api.CatalogError):
                self.catalog.profile(model, profile_id)

    def test_catalog_never_exposes_raw_transport_commands(self):
        for model in self.catalog.models.values():
            self.assertFalse(set(model.operations) & {'read','write','command','measure'})

    def test_unsupported_category_has_no_registration_driver(self):
        self.assertEqual(tuple(category.name for category in self.catalog.categories),
                         ("OSA", "ESA", "Oscilloscope", "Function Generator",
                          "Power Meter", "Piezo Controller", "Laser", "Custom"))
        self.assertFalse(self.catalog.category("ESA").can_register_without_driver)
        self.assertEqual(self.catalog.models_for("ESA"), ())
        self.assertEqual(self.catalog.model("voltage").category, "Custom")
        self.assertEqual(self.catalog.model("gain").category, "Custom")

    def test_serial_profile_normalizes_port_but_cannot_raise_fixed_settings(self):
        profile = self.catalog.profile("gain", "cp210x-serial")
        self.assertEqual(profile.baudrate, 115200)
        normalized = profile.validate({"port": "com12"})
        self.assertEqual(normalized, {"port": "COM12", "baudrate": 115200,
                                     "io_timeout_s": 1.0})
        for params in ({"port": "COM3", "baudrate": 9600}, {"port": "COM0"},
                       {"port": "COM3", "raw_command": "STCA999999"},
                       {"port": "COM3", "baudrate": True},
                       {"port": "COM3", "io_timeout_s": float("nan")}):
            with self.subTest(params=params), self.assertRaises(api.CatalogError):
                profile.validate(params)

    def test_visa_profile_rejects_unreviewed_interface_and_backend(self):
        profile = self.catalog.profile("aq6370", "gpib-visa")
        for params in ({"resource": "TCPIP0::10.0.0.1::INSTR"},
                       {"resource": "GPIB0::4::INSTR", "backend": "@py"},
                       {"resource": "GPIB0::4::INSTR\n*RST"},
                       {"resource": "GPIB0::4::INSTR", "timeout_s": True}):
            with self.subTest(params=params), self.assertRaises(api.CatalogError):
                profile.validate(params)

    def test_lifecycle_effects_never_claim_unsafe_connect_is_readonly(self):
        voltage = self.catalog.model("voltage")
        gain = self.catalog.model("gain")
        self.assertIn("zero_all_channels", voltage.connect_effects)
        self.assertIn("interlock_shutdown_possible", gain.connect_effects)
        for model, profile in (("voltage", "ch340-serial"), ("gain", "cp210x-serial"),
                               ("mdt693b", "serial")):
            self.assertFalse(self.catalog.profile(model, profile).automatic_probe)
        self.assertEqual(self.catalog.profile("voltage", "ch340-serial").probe_mode,
                         "supervised")
        self.assertNotIn("move", self.catalog.model("mdt693b").operations)

    def test_catalog_and_validation_outputs_cannot_change_trusted_definitions(self):
        profile = self.catalog.profile("gain", "cp210x-serial")
        result = profile.validate({"port": "COM3"})
        result["baudrate"] = 1
        self.assertEqual(profile.validate({"port": "COM3"})["baudrate"], 115200)
        with self.assertRaises((AttributeError, TypeError)):
            profile.fields["baudrate"] = {}

    def test_unknown_driver_and_duplicate_profile_are_rejected_on_load(self):
        source = json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
        mutations = []
        bad_driver = copy.deepcopy(source)
        bad_driver["models"][0]["driver_kind"] = "user.module"
        mutations.append(bad_driver)
        duplicate = copy.deepcopy(source)
        duplicate["models"][0]["profiles"].append(duplicate["models"][0]["profiles"][0])
        mutations.append(duplicate)
        for value in mutations:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "catalog.json"
                path.write_text(json.dumps(value), encoding="utf-8")
                with self.assertRaises(api.CatalogError):
                    api.load_catalog(path)

    def test_catalog_cannot_introduce_unreviewed_profiles_or_invalid_defaults(self):
        source = json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
        edits = (
            ("profile_id", lambda value: value["models"][0]["profiles"][0].update(id="tcpip-visa")),
            ("interface", lambda value: value["models"][0]["profiles"][0].update(interfaces=["TCPIP"])),
            ("default", lambda value: value["models"][2]["profiles"][0]["fields"]["baudrate"].update(default=9600)),
            ("metadata", lambda value: value["models"][0].update(name=[])),
        )
        for name, edit in edits:
            value = copy.deepcopy(source)
            edit(value)
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "catalog.json"
                path.write_text(json.dumps(value), encoding="utf-8")
                with self.assertRaises(api.CatalogError):
                    api.load_catalog(path)


if __name__ == "__main__":
    unittest.main()
