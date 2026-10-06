"""Protocol v3 is instance-addressed and never falls back to v2."""
from __future__ import annotations
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

from App.worker.contracts import ProtocolError, parse_v2
from App.worker import main
try:
    from App.worker import contracts_v3 as api
except ImportError:
    api = None

SESSION = "a" * 32
DEVICE_A = {"kind":"device", "id":"1"*32}
DEVICE_B = {"kind":"device", "id":"2"*32}
CONTEXT = {"session_id":SESSION,"domain":DEVICE_A,"connection_id":None,"epoch":0}

def wire(method="action", params=None, context=CONTEXT):
    if params is None: params = {"name":"zero","args":{}}
    return json.dumps({"v":3,"id":"request-1","method":method,"params":params,"context":context})

class ContractsV3Tests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(api, "The separate v3 contract is not implemented")

    def test_versions_never_fallback(self):
        v3 = wire()
        self.assertRaises(ProtocolError,parse_v2,v3)
        v2 = json.loads(v3)
        v2["v"] = 2
        self.assertRaises(ProtocolError,api.parse_v3,json.dumps(v2))

    def test_context_cannot_cross_same_kind_devices(self):
        first = api.parse_v3(wire()).context
        context_b = dict(CONTEXT,domain=DEVICE_B)
        second = api.parse_v3(wire(context=context_b)).context
        self.assertNotEqual(first.domain,second.domain)
        config = api.domain_config({"domain":DEVICE_A,"config_rev":1,"driver_kind":"voltage",
            "model_id":"voltage","profile_id":"ch340-serial","params":{"port":"COM3"},
            "expected_identity":{"model":"8-channel voltage source"},"members":[]})
        self.assertEqual(api.classify_v3(api.parse_v3(wire()),config),"zero")
        self.assertRaises(ProtocolError,api.classify_v3,api.parse_v3(wire(context=context_b)),config)

    def test_v3_rejects_duplicate_and_nonfinite_fields(self):
        duplicate = wire().replace('"epoch": 0','"epoch":0,"epoch":1')
        bad = [duplicate, wire(params={"name":"zero","args":{"value":float("nan")}}),
               wire(params={"name":"zero","args":{"value":1e309}})]
        for epoch in (True,-1,9007199254740992):
            bad.append(wire(context=dict(CONTEXT,epoch=epoch)))
        for line in bad:
            with self.subTest(line=line),self.assertRaises(ProtocolError):
                api.parse_v3(line)

    def test_strict_schema_rejects_role_priority_and_unreviewed_management(self):
        for params in ({"role":"voltage","name":"zero"},{"name":"zero","args":{},"priority":"safety"},
                       {"name":"zero","args":{"driver_kind":"gain"}}):
            self.assertRaises(ProtocolError,api.parse_v3,wire(params=params))
        self.assertRaises(ProtocolError,api.parse_v3,wire(method="urgent"))
        self.assertRaises(ProtocolError,api.parse_v3,wire(method="configure_domain",params={}))
        self.assertRaises(ProtocolError,api.parse_v3,wire(context=dict(CONTEXT,other=1)))
        self.assertRaises(ProtocolError,api.parse_v3,wire(context=dict(CONTEXT,domain=dict(DEVICE_A,kind="role"))))
        self.assertRaises(ProtocolError,api.parse_v3,wire(context=None))
        self.assertIsNone(api.parse_v3(wire(method="status",params={},context=None)).context)

    def test_size_limit_is_utf8_bytes_before_json_parse(self):
        self.assertRaises(ProtocolError,api.parse_v3,wire(params={"name":"zero","args":{"pad":"x"*65536}}))
        line = wire(params={"name":"zero","args":{"pad":"界"*22000}})
        line = json.dumps(json.loads(line),ensure_ascii=False)
        self.assertLess(len(line),65536)
        self.assertRaises(ProtocolError,api.parse_v3,line)

    def test_all_five_phases_preserve_context_and_falsey_result(self):
        phases = ("rejected_before_call","superseded_before_call","completed",
                  "completed_readback_failed","failed_after_call_started")
        context = api.parse_v3(wire()).context
        for phase in phases:
            outcome = api.OutcomeV3(phase,context,result=False) if phase == "completed" else api.OutcomeV3(
                phase,context,error={"type":"Example","message":"Example error"})
            result = json.loads(api.encode_v3("terminal",outcome))
            self.assertEqual(result["v"],3)
            self.assertEqual(result["context"],CONTEXT)
            self.assertEqual(result["phase"],phase)
            self.assertEqual(result["ok"],phase=="completed")
            if phase=="completed": self.assertIs(result["result"],False)
        self.assertEqual(set(api.FIVE_PHASES),set(phases))

    def test_shared_vectors_match_decoder(self):
        vectors = json.loads((Path(__file__).parent/"fixtures"/"v3-contracts.json").read_text(encoding="utf-8"))
        for case in vectors:
            with self.subTest(name=case["name"]):
                if case["valid"]:
                    self.assertEqual(api.parse_v3(case["line"]).method,case["method"])
                else:
                    self.assertRaises(ProtocolError,api.parse_v3,case["line"])

    def test_cli_selects_one_codec_and_requires_ownership_nonce_for_v3(self):
        parser = main._parser()
        self.assertEqual(parser.parse_args(["--real","--protocol","3","--ownership-nonce","b"*32]).protocol,3)
        for version,line in ((2,wire()),(3,json.dumps({"v":2,"id":"wrong","method":"ping","params":{},"context":None}))):
            with self.subTest(version=version),patch.object(main,"run",return_value=0) as v2run,patch.object(main,"run_v3",return_value=0) as v3run,patch.object(main,"ConsoleController") as legacy_factory,patch("sys.stdout",io.StringIO()):
                main.main(["--real","--protocol",str(version)] + (["--ownership-nonce","b"*32] if version==3 else []))
                self.assertEqual(v2run.call_count, int(version==2))
                self.assertEqual(v3run.call_count, int(version==3))
                self.assertEqual(legacy_factory.call_count,int(version==2))
        with self.assertRaises(SystemExit):
            main.main(["--real","--protocol","3"])

    def test_ownership_activation_is_exact_once_and_never_opens_a_driver(self):
        sink = io.StringIO()
        controller = main._bootstrap_v3(ownership_nonce="b"*32)
        def send(id,method,params,context):
            return json.dumps(dict(v=3,id=id,method=method,params=params,context=context))+"\n"
        global_context = {"session_id":controller.session_id,"domain":None,"connection_id":None,"epoch":0}
        lines = [
            send("ping","ping",{},None),
            send("before","inventory",{},global_context),
            send("wrong","activate",{"ownership_nonce":"c"*32},global_context),
            send("activate","activate",{"ownership_nonce":"b"*32},global_context),
            send("again","activate",{"ownership_nonce":"b"*32},global_context),
            send("stop","shutdown",{},global_context),
        ]
        self.assertEqual(main.run_v3(controller,input_stream=io.StringIO("".join(lines)),output_stream=sink),0)
        replies = {item["id"]:item for item in map(json.loads,sink.getvalue().splitlines())}
        self.assertEqual(replies["ping"]["result"]["protocol_version"],3)
        self.assertEqual(replies["ping"]["result"]["mode"],"real")
        self.assertEqual(replies["ping"]["result"]["domains"],{})
        self.assertFalse(replies["before"]["ok"])
        self.assertFalse(replies["wrong"]["ok"])
        self.assertTrue(replies["activate"]["ok"])
        self.assertFalse(replies["again"]["ok"])
        self.assertEqual(replies["stop"]["result"]["unreleased"],[])
        self.assertEqual(controller.driver_open_calls,0)

    def test_cross_version_ingress_rejects_before_any_driver_connection(self):
        from App.worker.controller import ConsoleController
        v3_controller = main._bootstrap_v3(ownership_nonce="b"*32)
        sink = io.StringIO()
        wrong_v2 = json.dumps({"v":2,"id":"wrong","method":"ping","params":{},"context":None})
        self.assertEqual(main.run_v3(v3_controller,input_stream=io.StringIO(wrong_v2+"\n"),output_stream=sink),0)
        self.assertFalse(json.loads(sink.getvalue())["ok"])
        self.assertEqual(v3_controller.driver_open_calls,0)
        v2_controller = ConsoleController()
        try:
            sink = io.StringIO()
            main.run(v2_controller,input_stream=io.StringIO(wire()+"\n"),output_stream=sink,settings_path=Path("unused.json"))
            self.assertFalse(json.loads(sink.getvalue())["ok"])
            self.assertFalse(v2_controller._scheduler.status()["devices"])
        finally:
            v2_controller.close()

    def test_deep_input_is_rejected_with_a_protocol_error(self):
        nested = "["*600 + "0" + "]"*600
        line = wire().replace('"args": {}','"args":{"nested":'+nested+'}')
        self.assertRaises(ProtocolError,api.parse_v3,line)

if __name__=="__main__": unittest.main()
