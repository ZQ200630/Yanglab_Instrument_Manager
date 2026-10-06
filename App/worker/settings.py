"""Legacy v2/version-1 role settings only; never use as a v3 device registry.

The independent Host owns version-2 registration and preserves raw v1 bytes
before migration. Live authority and status are never serialized here.
"""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any


DEFAULT_PYTHON = "D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe"
ROLES = ("osa", "pm400", "voltage", "gain")


def _clean(settings: dict[str, Any]) -> dict[str, Any]:
    if type(settings) is not dict:
        raise ValueError("settings must be an object")
    version = settings.get("version", 1)
    if type(version) is not int or version != 1:
        raise ValueError("unsupported settings version")
    python_path = settings.get("python_path", DEFAULT_PYTHON)
    if type(python_path) is not str or not python_path.strip():
        raise ValueError("python_path must be a nonempty path")
    bindings = settings.get("bindings", {})
    if type(bindings) is not dict or any(role not in ROLES for role in bindings):
        raise ValueError("bindings must contain only supported roles")
    cleaned = {role: bindings.get(role, "GPIB0::4::INSTR" if role == "osa" else None)
               for role in ROLES}
    if any(value is not None and (type(value) is not str or not value.strip())
           for value in cleaned.values()):
        raise ValueError("bindings must be nonempty strings or null")
    return {"version": 1, "python_path": python_path, "bindings": cleaned}


def load_settings(path: Path) -> dict[str, Any]:
    if not path.exists():
        return _clean({})
    value = json.loads(path.read_text(encoding="utf-8"))
    return _clean(value)


def save_settings(path: Path, settings: dict[str, Any]) -> None:
    cleaned = _clean(settings)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            "w", encoding="utf-8", dir=path.parent, prefix=f".{path.name}.",
            suffix=".tmp", delete=False,
        ) as output:
            temporary = output.name
            json.dump(cleaned, output, ensure_ascii=False, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        temporary = None
    finally:
        if temporary is not None:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass
