"""Package contract; missing Host must never fall back to source or spawn Python."""
import json,unittest,tempfile
from pathlib import Path
from App.tests.test_package_layout import _stage_resources
ROOT=Path(__file__).resolve().parents[1]
class HostPackageTests(unittest.TestCase):
    def test_v3_catalog_is_a_bundled_worker_resource(self):
        with tempfile.TemporaryDirectory(prefix='yang-host-catalog-') as temporary:
            bundle=Path(temporary)
            _stage_resources(bundle)
            # Consume the packaged catalogue; a path spelling is not the contract.
            data=json.loads((bundle/'App/catalog/devices.json').read_text())
            self.assertIn('aq6370', {model['id'] for model in data['models']})
    def test_missing_sidecar_never_uses_source_fallback(self):
        config=json.loads((ROOT/'src-tauri/tauri.conf.json').read_text())
        self.assertEqual(config['bundle'].get('externalBin'),['binaries/yang-lab-host'])
        source=(ROOT/'src-tauri/src/gui.rs').read_text()
        self.assertIn('resolve_packaged_host',source)
        self.assertNotIn('with_file_name("yang-lab-host.exe")',source)
        self.assertTrue((ROOT/'scripts/build-host.ps1').is_file())
        self.assertIn('default-run = "sil-instrument-console"',(ROOT/'src-tauri/Cargo.toml').read_text())
