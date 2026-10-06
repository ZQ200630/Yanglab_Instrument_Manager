"""Pipe-only native window acceptance peer; never an instrument worker.

Stage this file as App/worker/main.py in an isolated copy beside the verified
native executable, with empty App package initializers and a harmless
Code/Utils/osa.py marker for resource-root resolution. No production modules
are imported. The deliberately failed report and exit 19 exercise the host's
failed-cleanup evidence and window stay-open behavior. They describe no device.
"""

import datetime
import json
from pathlib import Path
import sys


def reply(request_id, *, result=None, error=None):
    envelope = {"v": 2, "id": request_id, "ok": error is None,
                "phase": "completed" if error is None else "rejected_before_call",
                "context": {"session_id": "native-close-fixture", "connection_id": None, "epoch": 0}}
    envelope["result" if error is None else "error"] = result if error is None else error
    print(json.dumps(envelope), flush=True)


def main():
    if sys.argv[1:] != ["--real", "--protocol", "2"]:
        print("Native close fixture requires exactly --real --protocol 2; no hardware is supported.",
              file=sys.stderr)
        return 2

    python = str(Path(sys.executable).resolve())
    for line in sys.stdin:
        request_id = "invalid"
        try:
            request = json.loads(line)
            if (type(request) is not dict
                    or set(request) != {"v", "id", "method", "params", "context"}
                    or type(request["v"]) is not int or request["v"] != 2
                    or type(request["id"]) is not str or not 1 <= len(request["id"]) <= 64
                    or type(request["method"]) is not str
                    or type(request["params"]) is not dict):
                raise ValueError("invalid fixture request envelope")
            if request["context"] != {"session_id": "native-close-fixture", "connection_id": None, "epoch": 0} and not (
                    request["method"] in {"ping", "status"} and request["context"] is None):
                raise ValueError("stale fixture context")
            request_id = request["id"]
            method = request["method"]
            if method not in {"ping", "settings_get", "status", "shutdown"}:
                reply(request_id, error={
                    "type": "FixtureOnlyError",
                    "message": "Pipe fixture denies all instrument operations and settings writes.",
                })
                continue
            if request["params"]:
                raise ValueError("fixture read and shutdown requests require empty params")
            if method == "ping":
                result = {
                    "mode": "real", "connected": False, "protocol_version": 2, "session_id": "native-close-fixture",
                    "roles": {role: {"session_id": "native-close-fixture", "connection_id": None, "epoch": 0}
                              for role in ("osa", "voltage", "gain", "pm400", "fiber")},
                    "python_executable": python,
                    "project_root": str(Path(__file__).resolve().parents[2]),
                    "environment_name": Path(sys.prefix).name,
                }
            elif method == "settings_get":
                result = {"version": 1, "python_path": python,
                          "bindings": {"osa": None, "pm400": None, "voltage": None, "gain": None}}
            elif method == "status":
                result = {"mode": "real", "devices": {}, "last_cleanup": None, "session_id": "native-close-fixture",
                          "roles": {role: {"session_id": "native-close-fixture", "connection_id": None, "epoch": 0}
                                    for role in ("osa", "voltage", "gain", "pm400", "fiber")},
                          "observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}
            else:
                reply(request_id, result={
                    "steps": [{"role": "pipe_fixture", "action": "close", "ok": False,
                               "error": "Intentional native close fixture failure; no hardware owned."}],
                    "unreleased": ["pipe_fixture"], "voltage_zero": None,
                })
                return 19
            reply(request_id, result=result)
        except (ValueError, TypeError) as error:
            reply(request_id, error={"type": "ProtocolError", "message": str(error)})
    # EOF owns no resources and must not fabricate a cleanup response.
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
