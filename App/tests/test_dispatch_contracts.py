"""Offline contract tests for the independent console v2 wire boundary."""

from __future__ import annotations

import json
import unittest

from App.worker.contracts import (
    Context, Observation, Outcome, ProtocolError, Request, classify, encode_v2,
    parse_v2,
)


_DEFAULT_CONTEXT = object()


def _wire(method="action", params=None, context=_DEFAULT_CONTEXT, **extra):
    if params is None:
        params = {"role": "voltage", "name": "zero"}
    if context is _DEFAULT_CONTEXT:
        context = {"session_id": "s", "connection_id": "c", "epoch": 0}
    value = {"v": 2, "id": "z", "method": method, "params": params,
             "context": context}
    value.update(extra)
    return json.dumps(value)


class ContractTests(unittest.TestCase):
    def test_zero_is_classified_by_allowlist(self):
        req = parse_v2('{"v":2,"id":"z","method":"action",'
                       '"params":{"role":"voltage","name":"zero"},'
                       '"context":{"session_id":"s","connection_id":"c","epoch":0}}')
        self.assertEqual(classify(req), "zero")
        self.assertEqual(req.context.epoch, 0)

    def test_invalid_wire_shapes_are_rejected(self):
        cases = {
            "v1": _wire(v=1),
            "duplicate top-level key": _wire()[:-1] + ',"id":"again"}',
            "duplicate nested key": ('{"v":2,"id":"z","method":"action",'
                                     '"params":{"role":"voltage","role":"gain","name":"zero"},'
                                     '"context":{"session_id":"s","connection_id":"c","epoch":0}}'),
            "bool epoch": _wire(context={"session_id": "s", "connection_id": "c", "epoch": True}),
            "negative epoch": _wire(context={"session_id": "s", "connection_id": "c", "epoch": -1}),
            "overflow epoch": _wire(context={"session_id": "s", "connection_id": "c", "epoch": 9007199254740992}),
            "empty session": _wire(context={"session_id": "", "connection_id": "c", "epoch": 0}),
            "blank connection": _wire(context={"session_id": "s", "connection_id": " ", "epoch": 0}),
            "extra priority": _wire(priority="safety"),
            "NaN": _wire(params={"role": "voltage", "name": "zero", "value": float("nan")}),
            "overflow float": _wire(params={"role": "voltage", "name": "zero", "value": float("inf")}),
            "wrong role": _wire(params={"role": "laser", "name": "zero"}),
            "wrong method": _wire(method="urgent"),
            "empty id": _wire(id=""),
            "long id": _wire(id="x" * 65),
            "non-object params": _wire(params=[]),
            "extra context key": _wire(context={"session_id": "s", "connection_id": "c", "epoch": 0, "priority": 1}),
        }
        for name, line in cases.items():
            with self.subTest(name=name), self.assertRaises(ProtocolError):
                parse_v2(line)

    def test_request_size_limit_and_id_boundary(self):
        with self.assertRaises(ProtocolError):
            parse_v2(_wire(params={"role": "voltage", "name": "zero", "pad": "x" * 1_000_000}))
        self.assertEqual(len(parse_v2(_wire(id="x" * 64)).id), 64)

    def test_bootstrap_context_is_null_only_for_read_only_methods(self):
        for method in ("ping", "status"):
            with self.subTest(method=method):
                request = parse_v2(_wire(method=method, params={}, context=None))
                self.assertIsNone(request.context)
        with self.assertRaises(ProtocolError):
            parse_v2(_wire(method="shutdown", params={}, context=None))

    def test_role_and_global_context_shapes(self):
        role_context = {"session_id": "s", "connection_id": None, "epoch": 7}
        self.assertEqual(parse_v2(_wire(method="connect", params={"role": "osa"},
                                        context=role_context)).context.epoch, 7)
        global_context = {"session_id": "s", "connection_id": None, "epoch": 0}
        self.assertEqual(classify(parse_v2(_wire(method="inventory", params={},
                                                  context=global_context))), "query")
        with self.assertRaises(ProtocolError):
            parse_v2(_wire(method="inventory", params={}, context={
                "session_id": "s", "connection_id": "c", "epoch": 0}))

    def test_method_and_action_classification(self):
        cases = (
            ("action", {"role": "voltage", "name": "zero"}, "zero"),
            ("action", {"role": "gain", "name": "disable_current"}, "current_off"),
            ("action", {"role": "gain", "name": "disable_tec"}, "tec_off"),
            ("action", {"role": "pm400", "name": "command", "priority": "shutdown"}, "normal"),
            ("action", {"role": "voltage", "name": "set_all"}, "normal"),
            ("connect", {"role": "osa"}, "normal"),
            ("disconnect", {"role": "osa"}, "disconnect"),
            ("shutdown", {}, "shutdown"),
            ("resume", {"role": "gain", "confirm": True}, "resume"),
            ("settings_save", {"value": {}}, "normal"),
        )
        for method, params, expected in cases:
            with self.subTest(method=method, params=params):
                context = ({"session_id": "s", "connection_id": None, "epoch": 0}
                           if method in {"shutdown", "settings_save"} else
                           {"session_id": "s", "connection_id": "c", "epoch": 0})
                self.assertEqual(classify(parse_v2(_wire(method=method, params=params,
                                                        context=context))), expected)

    def test_resume_requires_exact_confirmed_params(self):
        for params in ({"role": "gain"}, {"role": "gain", "confirm": 1},
                       {"role": "gain", "confirm": True, "priority": "safety"}):
            with self.subTest(params=params), self.assertRaises(ProtocolError):
                parse_v2(_wire(method="resume", params=params))

    def test_encode_success_false_result_and_error_are_distinct(self):
        context = Context("s", "c", 1)
        success = json.loads(encode_v2("z", Outcome("completed", context, result=False)))
        self.assertEqual(success, {"v": 2, "id": "z", "ok": True,
                                   "phase": "completed", "context": {
                                       "session_id": "s", "connection_id": "c", "epoch": 1},
                                   "result": False})
        failure = json.loads(encode_v2("z", Outcome("failed_after_call_started", context,
                                                   error={"type": "DeviceError", "message": "failed",
                                                          "attempt_id": "a1"})))
        self.assertEqual(failure, {"v": 2, "id": "z", "ok": False,
                                   "phase": "failed_after_call_started", "context": {
                                       "session_id": "s", "connection_id": "c", "epoch": 1},
                                   "error": {"type": "DeviceError", "message": "failed",
                                             "attempt_id": "a1"}})

    def test_phase_and_error_must_agree(self):
        context = Context("s", "c", 1)
        for phase in ("rejected_before_call", "superseded_before_call",
                      "completed_readback_failed", "failed_after_call_started"):
            with self.subTest(phase=phase, error="missing"), self.assertRaises(ProtocolError):
                encode_v2("z", Outcome(phase, context))
        with self.assertRaises(ProtocolError):
            encode_v2("z", Outcome("completed", context,
                                    error={"type": "DeviceError", "message": "failed"}))
        for error in ({"type": "", "message": "readback failed"},
                      {"type": "ReadbackError", "message": ""}):
            with self.subTest(error=error), self.assertRaises(ProtocolError):
                encode_v2("z", Outcome("completed_readback_failed", context, error=error))
        reply = json.loads(encode_v2(
            "z", Outcome("completed_readback_failed", context,
                         error={"type": "ReadbackError", "message": "status readback failed"})))
        self.assertFalse(reply["ok"])
        self.assertNotIn("result", reply)
        self.assertEqual(reply["error"]["message"], "status readback failed")

    def test_publish_copies_mutable_result_and_rejects_invalid_outcome(self):
        context = Context("s", "c", 1)
        result = {"items": [1]}
        encoded = encode_v2("z", Outcome("completed", context, result=result))
        result["items"].append(2)
        self.assertEqual(json.loads(encoded)["result"], {"items": [1]})
        for outcome in (
            Outcome("unknown", context),
            Outcome("completed", context, result=float("nan")),
            Outcome("completed", context, error={"type": "Bad"}),
            Outcome("completed", context, error={"type": "Bad", "message": "x", "extra": "x"}),
            Outcome("completed", context, error={"type": "Bad", "message": "x", "attempt_id": 3}),
        ):
            with self.subTest(outcome=outcome), self.assertRaises(ProtocolError):
                encode_v2("z", outcome)

    def test_direct_request_copies_params_and_contract_types_are_frozen(self):
        params = {"nested": [1]}
        request = Request("x", "action", params, Context("s", "c", 0))
        params["nested"].append(2)
        self.assertEqual(request.params, {"nested": [1]})
        with self.assertRaises(AttributeError):
            request.id = "changed"
        self.assertFalse(Observation({"ready": False}).more)


if __name__ == "__main__":
    unittest.main()
