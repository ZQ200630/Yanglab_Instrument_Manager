"""Pipe-only Rust test peer. This file imports no instrument code."""

import json
import sys
import argparse
import threading
import time
from pathlib import Path


def run_v2():
    """Deliberately controllable standard-library peer, never production dispatch."""
    options = {}
    held = []
    held_results = {}
    attempts = 0
    output_lock = threading.Lock()
    session = "pipe-session"

    def send(request, *, result=None, error=None, context=None):
        frame = {"v": 2, "id": request["id"], "ok": error is None,
                 "phase": "completed" if error is None else "failed_after_call_started",
                 "context": context if context is not None else request.get("context") or
                    {"session_id": session, "connection_id": None, "epoch": 0}}
        frame["result" if error is None else "error"] = result if error is None else error
        with output_lock:
            print(json.dumps(frame), flush=True)

    for line in sys.stdin:
        request = json.loads(line)
        method = request["method"]
        params = request["params"]
        if method == "configure":
            options.update(params)
            if params.get("release"):
                for old in reversed(held):
                    send(old, result=held_results.pop(old["id"], {"resumed": True, "released": True}))
                held.clear()
            send(request, result={"held": len(held), "attempts": attempts})
            if options.get("behavior") == "exit_cleanly":
                sys.exit(0)
        elif method == "ping":
            folder = Path(__file__).parent
            if (folder / "startup_hold").exists():
                (folder / "startup_entered").write_text("entered")
                while not (folder / "startup_release").exists():
                    time.sleep(.01)
            if (folder / "startup_v1").exists():
                print(json.dumps({"v": 1, "id": request["id"], "ok": True, "result": {}}), flush=True)
                continue
            send(request, result={"session_id": session, "mode": "real",
                                  "protocol_version": 2, "connected": False,
                "python_executable": str(Path(sys.executable).resolve()),
                "project_root": str(Path(__file__).resolve().parents[2]),
                "environment_name": Path(sys.prefix).name,
                "roles": {role: {"session_id": session, "connection_id": None, "epoch": 0}
                          for role in ("osa", "voltage", "gain", "pm400", "fiber")}})
        elif method == "status" and options.get("behavior") == "status_error":
            send(request, error={"type": "ReadError", "message": "status unavailable"})
        elif method == "status" and options.get("behavior") == "status_nonobject":
            send(request, result="invalid status")
        elif method == "status":
            send(request, result={"session_id": session, "devices": {}, "held": len(held)})
        elif method == "shutdown":
            attempts += 1
            behavior = options.get("behavior", "normal")
            if behavior == "shutdown_error_live":
                send(request, error={"type": "CloseError", "message": "still owned"})
                continue
            report = {"attempt_id": request["id"], "steps": ([{"role": "pipe_fixture", "action": "close", "ok": True}]
                                if behavior == "shutdown_reply_live_marked" else []),
                      "unreleased": ["pipe_fixture"] if behavior == "unreleased_live" else [],
                      "voltage_zero": None}
            if options.get("hold_shutdown"):
                held.append(request)
                held_results[request["id"]] = report
                continue
            send(request, result=report)
            if behavior in ("unreleased_live", "shutdown_reply_live", "shutdown_reply_live_marked"):
                continue
            if behavior == "delayed_exit":
                time.sleep(options.get("exit_delay", 5.2))
            sys.exit(19 if behavior == "exit_nonzero" else 0)
        elif method == "action" and params.get("name") == "no_reply":
            continue
        elif method == "action" and params.get("name") == "exit_cleanly":
            sys.exit(0)
        elif method == "action" and params.get("name") == "stale":
            send({"id": "prior-id"}, result="stale")
            send(request, result="matched")
        elif method == "action" and params.get("name") == "wrong_version":
            print(json.dumps({"v": 1, "id": request["id"], "ok": True, "result": {}}), flush=True)
        elif method == "action" and params.get("name") == "missing_ok":
            print(json.dumps({"v": 2, "id": request["id"], "result": {}}), flush=True)
        elif method == "action" and params.get("name") == "malformed":
            print("not JSON", flush=True)
        elif method == "action" and params.get("name") == "exit":
            sys.exit(23)
        elif method == "resume" and options.get("hold_resume"):
            held.append(request)
        elif method == "action" and params.get("name") in options.get("hold", []):
            held.append(request)
        elif method == "disconnect" or (method == "action" and params.get("name") in ("zero", "disable_current", "disable_tec")):
            context = dict(request["context"])
            context["epoch"] += 1
            send(request, result={"effective_intent": params.get("name", "disconnect")}, context=context)
        elif method == "resume":
            if options.get("reject_resume"):
                send(request, error={"type": "UnsafeResume", "message": "not authorized"})
            else:
                send(request, result={"resumed": True})
        else:
            send(request, result={})

    folder = Path(__file__).parent
    if (folder / "retain_eof").exists():
        (folder / "eof_entered").write_text("entered")
        while not (folder / "release_eof").exists():
            time.sleep(.01)



def run_v3(ownership_nonce):
    """Instance-aware native transport double; standard library, no instruments."""
    session="a"*32
    activated=False
    domains={}
    opens=0
    def send(request,result=None,error=None,context=None):
        context=context or request.get("context") or {
            "session_id":session,"domain":None,"connection_id":None,"epoch":0}
        frame={"v":3,"id":request["id"],"ok":error is None,"phase":
            "completed" if error is None else "rejected_before_call","context":context}
        frame["result" if error is None else "error"]=result if error is None else error
        print(json.dumps(frame),flush=True)
    for line in sys.stdin:
        request=json.loads(line)
        method=request["method"]
        if request.get("v")!=3:
            send(request,error={"type":"Protocol","message":"Version mismatch"});continue
        if method in ("ping","status"):
            send(request,{"protocol_version":3,"mode":"real","session_id":session,"activated":activated,
                "connected":bool(opens),"domains":domains,"driver_open_calls":opens,
                "python_executable":str(Path(sys.executable).resolve()),
                "project_root":str(Path(__file__).resolve().parents[2]),"environment_name":Path(sys.prefix).name})
        elif method=="activate":
            if activated or request["params"]["ownership_nonce"]!=ownership_nonce:
                send(request,error={"type":"Ownership","message":"Activation rejected"})
            else:activated=True;send(request,{"activated":True})
        elif method=="shutdown":
            send(request,{"steps":[],"unreleased":[],"voltage_zero":None});return
        elif not activated:
            send(request,error={"type":"Ownership","message":"No activation"})
        elif method=="configure_domain":
            configuration=request["params"]["config"]
            ref=configuration["domain"]
            key=ref["kind"]+":"+ref["id"]
            domains[key]={"context":{"session_id":session,"domain":ref,"connection_id":None,"epoch":0}}
            send(request,{"context":domains[key]["context"],"config_rev":configuration["config_rev"]})
        else:
            send(request,{})

parser = argparse.ArgumentParser(allow_abbrev=False)
parser.add_argument("--real", action="store_true")
parser.add_argument("--protocol", type=int, choices=(2,3), default=2)
parser.add_argument("--ownership-nonce")
parser.add_argument("--capture-spool")
args = parser.parse_args()
if not args.real:
    parser.error("pipe fixture requires exact --real")
if args.protocol==3:
    if (not args.ownership_nonce or len(args.ownership_nonce)!=32
            or any(char not in "0123456789abcdef" for char in args.ownership_nonce)):
        parser.error("V3 pipe fixture requires an ownership nonce")
    run_v3(args.ownership_nonce)
else:
    if args.ownership_nonce is not None:parser.error("Nonce is only valid with protocol 3")
    run_v2()
