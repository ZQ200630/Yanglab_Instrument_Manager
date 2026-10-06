from __future__ import annotations

import ast
import json
import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def _requirements_contract(text: str) -> dict[str, str]:
    """Validate the public direct-dependency input, not pip's general syntax."""
    result: dict[str, str] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        entry = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([0-9]+(?:\.[0-9]+)*)", line)
        if entry is None:
            raise AssertionError(f"requirement must be an exact package version: {line!r}")
        name = re.sub(r"[-_.]+", "-", entry[1]).lower()
        if name in result:
            raise AssertionError(f"duplicate requirement: {name}")
        result[name] = entry[2]
    return result


def _manifest_contract(path: Path) -> tuple[str, str, tuple[str, ...], tuple[str, ...]]:
    """Parse the deliberately small environment.yml contract without PyYAML."""
    lines = path.read_text(encoding="utf-8").splitlines()
    top_level = tuple(line for line in lines if line and not line.startswith(" "))
    if top_level != ("name: VISA", "dependencies:"):
        raise AssertionError(f"unexpected top-level environment schema: {top_level!r}")
    if any(line.strip().startswith("prefix:") for line in lines):
        raise AssertionError("environment manifest must remain machine-independent")

    conda_dependencies: list[str] = []
    pip_dependencies: list[str] = []
    in_pip = False
    for line in lines[2:]:
        if line == "  - pip:":
            in_pip = True
        elif in_pip and line.startswith("      - "):
            pip_dependencies.append(line.removeprefix("      - "))
        elif not in_pip and line.startswith("  - "):
            conda_dependencies.append(line.removeprefix("  - "))
        elif line.strip():
            raise AssertionError(f"unexpected environment entry: {line!r}")
    return "VISA", conda_dependencies[0], tuple(conda_dependencies[1:]), tuple(pip_dependencies)


def _reviewed_external_imports() -> set[str]:
    """Find imports from the six reviewed packages; new dependencies need source review."""
    imported: set[str] = set()
    for path in (ROOT / "Code").rglob("*.py"):
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                imported.update(alias.name.partition(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
                imported.add(node.module.partition(".")[0])
    return imported & {"numpy", "serial", "pyvisa", "pymeasure", "matplotlib", "PIL"}


class EnvironmentManifestTests(unittest.TestCase):
    def test_requirements_match_development_manifest(self):
        path = ROOT / "requirements.txt"
        self.assertTrue(path.is_file(), "Missing public development requirements.txt")
        requirements = _requirements_contract(path.read_text(encoding="utf-8"))
        self.assertEqual({"numpy": "2.2.4", "pyserial": "3.5", "pyvisa": "1.14.1",
                          "pymeasure": "0.15.0", "matplotlib": "3.10.1", "pillow": "11.1.0"},
                         requirements)
        _, _, _, entries = _manifest_contract(ROOT / "environment.yml")
        self.assertEqual(_requirements_contract("\n".join(entries)), requirements)

    def test_requirements_contract_rejects_duplicate_or_conflicting_normalized_names(self):
        for text in ("PyVISA==1.14.1\npyvisa==1.14.1", "numpy==2.2.4\nnumpy==2.1.0",
                     "some_pkg==1.0\nsome-pkg==1.0", "some.pkg==1.0\nsome_pkg==1.0"):
            with self.subTest(text=text), self.assertRaisesRegex(AssertionError, "duplicate"):
                _requirements_contract(text)

    def test_requirements_contract_rejects_unpinned_or_external_install_inputs(self):
        for text in ("numpy", "numpy>=2.2.4", "numpy~=2.2", "numpy==2.2.4; python_version>'3'",
                     "-r other.txt", "-e .", "../local.whl", "https://example.test/pkg.whl",
                     "pkg @ https://example.test/pkg.whl", "--extra-index-url https://example.test"):
            with self.subTest(text=text), self.assertRaisesRegex(AssertionError, "exact package"):
                _requirements_contract(text)

    def test_requirements_contract_allows_comments_and_whitespace_without_install_options(self):
        self.assertEqual({"pyvisa": "1.14.1", "numpy": "2.2.4"},
                         _requirements_contract("# Direct dependency pins\n\n PyVISA==1.14.1 \n\tnumpy==2.2.4\n"))

    def test_manifest_declares_only_reviewed_direct_and_test_dependencies(self):
        name, python, conda_dependencies, pip_dependencies = _manifest_contract(
            ROOT / "environment.yml"
        )
        self.assertEqual("VISA", name)
        self.assertEqual("python=3.10.16", python)
        self.assertEqual(("pip",), conda_dependencies)
        self.assertEqual(
            (
                "numpy==2.2.4",
                "pyserial==3.5",
                "PyVISA==1.14.1",
                "PyMeasure==0.15.0",
                "matplotlib==3.10.1",
                "Pillow==11.1.0",
            ),
            pip_dependencies,
        )

    def test_manifest_covers_reviewed_direct_third_party_imports(self):
        package_for_import = {
            "numpy": "numpy",
            "serial": "pyserial",
            "pyvisa": "PyVISA",
            "pymeasure": "PyMeasure",
            "matplotlib": "matplotlib",
            "PIL": "Pillow",
        }
        _, _, _, pip_dependencies = _manifest_contract(ROOT / "environment.yml")
        declared = {requirement.partition("==")[0] for requirement in pip_dependencies}
        required = {package_for_import[name] for name in _reviewed_external_imports()}
        self.assertEqual(required, declared)


class FiberConfigurationContractTests(unittest.TestCase):
    def test_fiber_configuration_keeps_physical_side_bindings_and_nominal_limits(self):
        payload = json.loads((ROOT / "Config" / "fiber_coupling.json").read_text(encoding="utf-8"))
        self.assertEqual("MAX312D", payload["model"])
        self.assertEqual({"toward_chip": 0.2, "other": 1.0}, payload["operator_limits_um"])
        self.assertEqual("2110148249-10", payload["stages"]["left"]["serial_number"])
        self.assertEqual("160721175410", payload["stages"]["right"]["serial_number"])
        self.assertEqual({"x": "Y", "y": "X", "z": "Z"}, payload["stages"]["left"]["axis_map"])
        self.assertEqual({"x": "X", "y": "Y", "z": "Z"}, payload["stages"]["right"]["axis_map"])
        self.assertEqual(1, payload["stages"]["left"]["toward_chip_sign"])
        self.assertEqual(-1, payload["stages"]["right"]["toward_chip_sign"])
        for stage in payload["stages"].values():
            for calibration in stage["calibration"].values():
                for direction in ("positive", "negative"):
                    coefficient = calibration[direction]
                    self.assertEqual(0.2666666666666667, coefficient["um_per_v"])
                    self.assertEqual("nominal_MAX312D", coefficient["source"])
                    self.assertIsNone(coefficient["date"])


if __name__ == "__main__":
    unittest.main()
