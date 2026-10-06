"""Resolve this computer's explicitly built native Host for process tests."""
import os
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def native_host_binary(environ=None, *, root=ROOT):
    values = os.environ if environ is None else environ
    root = Path(root).resolve()
    explicit = values.get('YANG_LAB_TEST_HOST')
    if explicit:
        artifact = Path(explicit)
        if not artifact.is_absolute():
            raise ValueError('YANG_LAB_TEST_HOST must be an absolute verified artifact path')
        return artifact.resolve()
    target = Path(values.get('CARGO_TARGET_DIR') or root / 'App/src-tauri/target')
    if not target.is_absolute():
        target = root / target
    return target.resolve() / 'debug/yang-lab-host.exe'
