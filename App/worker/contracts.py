"""Strict v2 console request, reply, and dispatch contracts.

This module is independent of the live v1 transport until the coordinated cutover.
"""

from __future__ import annotations

import copy
import json
import math
from dataclasses import dataclass
from typing import Any, Literal


Role = Literal["osa", "voltage", "gain", "pm400", "fiber"]
Phase = Literal[
    "rejected_before_call", "superseded_before_call", "completed",
    "completed_readback_failed", "failed_after_call_started",
]

VERSION = 2
MAX_EPOCH = 9007199254740991
ROLES = frozenset({"osa", "voltage", "gain", "pm400", "fiber"})
METHODS = frozenset({
    "ping", "inventory", "connect", "disconnect", "status", "action",
    "settings_get", "settings_save", "shutdown", "resume",
})
QUERY_METHODS = frozenset({"ping", "inventory", "status", "settings_get"})
ROLE_METHODS = frozenset({"connect", "disconnect", "action", "resume"})
PHASES = frozenset({
    "rejected_before_call", "superseded_before_call", "completed",
    "completed_readback_failed", "failed_after_call_started",
})
SAFE_ACTIONS = {
    ("voltage", "zero"): "zero",
    ("gain", "disable_current"): "current_off",
    ("gain", "disable_tec"): "tec_off",
}


class ProtocolError(ValueError):
    """A request or response violates the console's v2 wire contract."""


@dataclass(frozen=True)
class Context:
    session_id: str
    connection_id: str | None
    epoch: int


@dataclass(frozen=True)
class Request:
    id: str
    method: str
    params: dict[str, Any]
    context: Context | None

    def __post_init__(self) -> None:
        object.__setattr__(self, "params", copy.deepcopy(self.params))


@dataclass(frozen=True)
class Outcome:
    phase: Phase
    context: Context | None
    result: Any = None
    error: dict[str, str] | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "result", copy.deepcopy(self.result))
        object.__setattr__(self, "error", copy.deepcopy(self.error))


@dataclass(frozen=True)
class Observation:
    status: dict[str, Any]
    more: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "status", copy.deepcopy(self.status))


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ProtocolError(f"duplicate JSON field: {key}")
        value[key] = item
    return value


def _reject_constant(value: str) -> None:
    raise ProtocolError(f"non-finite JSON value: {value}")


def _finite(value: Any) -> bool:
    if isinstance(value, float):
        return math.isfinite(value)
    if isinstance(value, list):
        return all(_finite(item) for item in value)
    if isinstance(value, dict):
        return all(_finite(item) for item in value.values())
    return True


def _valid_id(value: Any) -> bool:
    return type(value) is str and 1 <= len(value) <= 64


def _context(value: Any) -> Context | None:
    if value is None:
        return None
    if type(value) is not dict or set(value) != {"session_id", "connection_id", "epoch"}:
        raise ProtocolError("context fields must be session_id, connection_id, epoch")
    session_id = value["session_id"]
    connection_id = value["connection_id"]
    epoch = value["epoch"]
    if type(session_id) is not str or not session_id.strip():
        raise ProtocolError("context session_id must be nonempty")
    if connection_id is not None and (
        type(connection_id) is not str or not connection_id.strip()
    ):
        raise ProtocolError("context connection_id must be null or nonempty")
    if type(epoch) is not int or not 0 <= epoch <= MAX_EPOCH:
        raise ProtocolError("context epoch must be a nonnegative JS-safe integer")
    return Context(session_id, connection_id, epoch)


def _validate_method_params(method: Any, params: Any) -> None:
    if type(method) is not str or method not in METHODS:
        raise ProtocolError("unknown method")
    if type(params) is not dict or not _finite(params):
        raise ProtocolError("params must be a finite JSON object")
    if method in ROLE_METHODS:
        if type(params.get("role")) is not str or params["role"] not in ROLES:
            raise ProtocolError("unknown instrument role")
    if method == "action" and (
        type(params.get("name")) is not str or not params["name"].strip()
    ):
        raise ProtocolError("action name must be nonempty")
    if method == "resume" and (
        set(params) != {"role", "confirm"} or params["confirm"] is not True
    ):
        raise ProtocolError("resume requires only role and confirm=true")


def parse_v2(line: str) -> Request:
    """Parse one v2 JSON request without touching a device or dispatch queue."""
    if not isinstance(line, str) or not line or len(line) > 1_000_000:
        raise ProtocolError("request must be a nonempty JSON line under 1 MB")
    try:
        value = json.loads(
            line, object_pairs_hook=_unique_object, parse_constant=_reject_constant,
        )
    except (ValueError, TypeError) as error:
        raise ProtocolError(f"invalid JSON request: {error}") from error
    if type(value) is not dict or set(value) != {"v", "id", "method", "params", "context"}:
        raise ProtocolError("request fields must be v, id, method, params, context")
    if type(value["v"]) is not int or value["v"] != VERSION:
        raise ProtocolError("unsupported protocol version")
    if not _valid_id(value["id"]):
        raise ProtocolError("request id must contain 1–64 characters")
    method = value["method"]
    params = value["params"]
    _validate_method_params(method, params)
    context = _context(value["context"])
    if context is None and method not in {"ping", "status"}:
        raise ProtocolError("context is required outside read-only bootstrap")
    if method in ROLE_METHODS and context is None:
        raise ProtocolError("role request requires context")
    if method not in ROLE_METHODS and context is not None and (
        context.connection_id is not None or context.epoch != 0
    ):
        raise ProtocolError("global request requires null connection and epoch zero")
    return Request(value["id"], method, params, context)


def classify(request: Request) -> str:
    """Classify from the backend allowlist, never a client priority field."""
    if not isinstance(request, Request):
        raise ProtocolError("request must be a Request")
    _validate_method_params(request.method, request.params)
    if request.method == "action":
        return SAFE_ACTIONS.get((request.params["role"], request.params["name"]), "normal")
    if request.method in QUERY_METHODS:
        return "query"
    if request.method in {"disconnect", "shutdown", "resume"}:
        return request.method
    return "normal"


def encode_v2(request_id: str, outcome: Outcome) -> str:
    """Encode one terminal reply, preserving falsey successful results."""
    if not _valid_id(request_id):
        raise ProtocolError("invalid response id")
    if not isinstance(outcome, Outcome) or outcome.phase not in PHASES:
        raise ProtocolError("invalid outcome phase")
    if outcome.context is None:
        context = None
    elif isinstance(outcome.context, Context):
        context = _context({
            "session_id": outcome.context.session_id,
            "connection_id": outcome.context.connection_id,
            "epoch": outcome.context.epoch,
        })
        context = {"session_id": context.session_id,
                   "connection_id": context.connection_id, "epoch": context.epoch}
    else:
        raise ProtocolError("invalid response context")
    success = outcome.phase == "completed"
    error = copy.deepcopy(outcome.error)
    if (success and error is not None) or (not success and error is None):
        raise ProtocolError("completed requires no error; other phases require an error")
    if error is not None:
        if outcome.result is not None:
            raise ProtocolError("response cannot have both result and error")
        if type(error) is not dict or not {"type", "message"} <= set(error) \
                or set(error) - {"type", "message", "attempt_id"} \
                or any(type(error[key]) is not str or not error[key].strip()
                       for key in ("type", "message")) \
                or ("attempt_id" in error and type(error["attempt_id"]) is not str):
            raise ProtocolError("error must contain nonempty type/message and optional string attempt_id")
    response = {"v": VERSION, "id": request_id, "ok": success,
                "phase": outcome.phase, "context": context}
    if success:
        response["result"] = copy.deepcopy(outcome.result)
    else:
        response["error"] = error
    try:
        return json.dumps(response, separators=(",", ":"),
                          ensure_ascii=False, allow_nan=False) + "\n"
    except (TypeError, ValueError) as cause:
        raise ProtocolError(f"response is not JSON serializable: {cause}") from cause
