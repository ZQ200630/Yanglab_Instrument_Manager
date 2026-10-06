"""Local v2 ingress: bounded terminal evidence, one writer, asynchronous cleanup."""
from __future__ import annotations

import argparse
from collections import deque
from concurrent.futures import Future
import os
from pathlib import Path
import queue
import sys
import threading
import time
import uuid

from .controller import ConsoleController
from .contracts import Context, Outcome, Request, classify
from .protocol import ProtocolError, encode_v2, parse_v2
from .settings import load_settings, save_settings

PROJECT_ROOT = Path(__file__).resolve().parents[2]
OUTPUT_STALL_SECONDS = 5.0
QUERY_RETIRE_GRACE_SECONDS = 0.005


def _parser():
    parser = argparse.ArgumentParser(description="SIL Tauri instrument worker", allow_abbrev=False)
    parser.add_argument("--real", action="store_true", required=True)
    parser.add_argument("--protocol", type=int, choices=(2, 3), default=2)
    parser.add_argument("--ownership-nonce", default=None)
    parser.add_argument("--capture-spool", type=Path, default=None)
    parser.add_argument("--settings", type=Path, default=None)
    return parser


def _settings_path(explicit):
    return explicit if explicit is not None else (
        Path(os.environ.get("LOCALAPPDATA", str(Path.home()))) /
        "SILInstrumentConsole" / "settings.json")


class _Replies:
    """41 accepted IDs plus one bounded protocol/rejection frame.

    A reservation includes a pending call, an unsent reply, or the current write.
    Finite safety chains admit only strictly stronger intent per owned role.
    """
    def __init__(self, output, *,version=2,config_resolver=None):
        self.version=version
        self.config_resolver=config_resolver
        if version==3:
            from .contracts_v3 import OutcomeV3,encode_v3
            self.outcome_type=OutcomeV3
            self.encode=encode_v3
        else:
            self.outcome_type=Outcome
            self.encode=encode_v2
        self.output = output
        self.responsibility_limit=225 if version==3 else 41
        self.queue = queue.Queue(maxsize=self.responsibility_limit+1)
        self.lock = threading.Lock()
        self.active = {}
        self.query_retired = threading.Event()
        self.query_retired.set()
        self.recent = deque(maxlen=256)
        self.error_slot = False
        self.error_id = None
        self.fault = threading.Event()
        self.output_failed = threading.Event()
        self.shutdown_flushed = threading.Event()
        self.closed = threading.Event()
        self.writing_since = None
        self.unsent = None
        self.failed_publications = {}
        self.thread = threading.Thread(target=self._write, name="console-replies")
        self.thread.start()

    def reserve(self, request):
        if self.version==3:
            from .contracts_v3 import classify_v3
            ref=request.context.domain if request.context is not None else None
            config=self.config_resolver(ref) if ref is not None else None
            kind=classify_v3(request,config)
            role=None if ref is None else (ref.kind,ref.id)
            driver_kind=None if config is None else config.driver_kind
        else:
            kind = classify(request)
            role = request.params.get("role")
            driver_kind=role
        group = ("query" if request.method in {"ping", "status"} else
                 "shutdown" if kind == "shutdown" else
                 role if kind in {"zero", "current_off", "tec_off", "disconnect"} else "normal")
        rank = {"zero": 1, "current_off": 1, "tec_off": 2,
                "disconnect": 3 if driver_kind == "gain" else 2 if driver_kind == "voltage" else 1}.get(kind, 0)
        with self.lock:
            publishing_query = (group == 'query' and self.unsent is not None and self.unsent[2]
                                and self.active.get(self.unsent[0], (None, 0))[0] == 'query')
        if publishing_query:
            # A serial client can observe the flushed newline just before the
            # writer retires that query. Allow only a bounded handoff, with no
            # lock held and no extra reservation; blocked stdout still rejects.
            self.query_retired.wait(QUERY_RETIRE_GRACE_SECONDS)
        with self.lock:
            if request.id in self.active or request.id in self.recent or request.id == self.error_id:
                return "duplicate request ID"
            same = [rank for category, rank in self.active.values() if category == group]
            if self.output_failed.is_set() or (self.fault.is_set() and group not in {"query", "shutdown"}) or (
                group == "normal" and len(same) >= 31) or (
                group in {"query", "shutdown"} and same) or (
                group not in {"normal", "query", "shutdown"} and same and rank <= max(same)):
                return "terminal admission full or existing control attempt"
            if len(self.active) >= self.responsibility_limit:
                return "terminal admission full"
            self.active[request.id] = (group, rank)
            if group == 'query':
                self.query_retired.clear()
        return None

    def reject(self, request_id, context, message):
        # Protect the actual transport placeholder, not an inferred malformed-input ID.
        # It must never compete with an accepted/recent terminal on the wire.
        request_id = "invalid" if request_id is None else request_id
        with self.lock:
            if self.error_slot or request_id in self.active or request_id in self.recent:
                self.fault.set()
                return
            self.error_slot = True
            self.error_id = request_id
        self.queue.put_nowait((request_id, self.outcome_type("rejected_before_call", context,
            error={"type": "ProtocolError", "message": message}), False, False))

    def publish(self, request, completed):
        try:
            outcome = completed.result()
            if not isinstance(outcome, self.outcome_type):
                raise TypeError("terminal callback did not return an Outcome")
        except BaseException as error:
            self.fault.set()
            outcome = self.outcome_type("completed_readback_failed", request.context,
                error={"type": type(error).__name__, "message": "terminal callback failed"})
        shutdown = (request.method == "shutdown" and outcome.phase == "completed"
                    and isinstance(outcome.result, dict) and outcome.result.get("unreleased") == [])
        try:
            self.queue.put_nowait((request.id, outcome, True, shutdown))
        except BaseException:
            self.fault.set()
            with self.lock:
                if request.id in self.active:
                    self.failed_publications[request.id] = outcome

    def stalled(self):
        with self.lock:
            return (self.writing_since is not None and
                    time.monotonic() - self.writing_since >= OUTPUT_STALL_SECONDS)

    def _write(self):
        while not self.closed.is_set() or not self.queue.empty() or self.failed_publications:
            try:
                frame = self.queue.get(timeout=.05)
            except queue.Empty:
                # These are existing terminals, not new admissions or operation retries.
                # Keep evidence and its reservation until the actual write/flush succeeds.
                with self.lock:
                    if not self.failed_publications:
                        continue
                    request_id, outcome = next(iter(self.failed_publications.items()))
                    shutdown = (self.active[request_id][0] == "shutdown" and
                                outcome.phase == "completed" and isinstance(outcome.result, dict) and
                                outcome.result.get("unreleased") == [])
                    frame = (request_id, outcome, True, shutdown)
            request_id, outcome, accepted, shutdown = frame
            with self.lock:
                self.writing_since = time.monotonic()
                self.unsent = frame
            try:
                self.output.write(self.encode(request_id, outcome))
                self.output.flush()
            except BaseException as error:
                self.output_failed.set()
                self.fault.set()
                print("protocol output failed: " + type(error).__name__, file=sys.stderr)
                return
            with self.lock:
                self.writing_since = None
                self.unsent = None
                if accepted:
                    group = self.active.get(request_id, (None, 0))[0]
                    self.active.pop(request_id, None)
                    self.failed_publications.pop(request_id, None)
                    self.recent.append(request_id)
                    if group == 'query':
                        self.query_retired.set()
                else:
                    self.error_slot = False
                    self.error_id = None
                    self.recent.append(request_id)
            if shutdown:
                self.shutdown_flushed.set()


def _read_lines(source, incoming, stopped, maximum=1_000_000):
    """Raw-descriptor reader never holds a buffered stdio lock during teardown."""
    def put(item):
        while not stopped.is_set():
            try:
                incoming.put(item, timeout=.05)
                return
            except queue.Full:
                pass
    try:
        try:
            descriptor = source.fileno()
        except (AttributeError, OSError):
            descriptor = None
        if descriptor is None:
            lines=(iter(lambda:source.readline(maximum+1),'') if maximum==65536 and hasattr(source,'readline') else iter(source))
            for line in lines:
                if stopped.is_set():
                    return
                if maximum==65536 and len(line.encode('utf-8'))>maximum:
                    raise ProtocolError('input line exceeds protocol limit')
                put(line)
        else:
            pending = b""
            while not stopped.is_set():
                block = os.read(descriptor, 4096)
                if not block:
                    if pending:
                        put(pending.decode("utf-8"))
                    break
                pending += block
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    if maximum==65536 and len(line)+1>maximum:
                        raise ProtocolError('input line exceeds protocol limit')
                    put(line.decode("utf-8"))
                if len(pending) > maximum:
                    raise ProtocolError("input line exceeds protocol limit")
        put(None)
    except BaseException as error:
        put(error)


def run(controller, *, input_stream, output_stream, settings_path, protocol=2):
    def management(request):
        if request.method == "settings_save":
            if set(request.params) != {"settings"}:
                raise ValueError("settings_save requires only settings")
            save_settings(settings_path, request.params["settings"])
        return load_settings(settings_path)
    if protocol==2:
        controller._management = management
        global_context=lambda:Context(controller.context('osa').session_id,None,0)
        join_scheduler=controller._scheduler.join
        parse=parse_v2
        request_type=Request
        replies=_Replies(output_stream)
    else:
        from .contracts_v3 import RequestV3,parse_v3
        global_context=controller.global_context
        join_scheduler=controller.join
        parse=parse_v3
        request_type=RequestV3
        replies=_Replies(output_stream,version=3,config_resolver=controller.config)
    incoming = queue.Queue(maxsize=1)
    stopped = threading.Event()
    reader = threading.Thread(target=_read_lines, args=(input_stream,incoming,stopped,65536 if protocol==3 else 1_000_000),
                              name="console-ingress-reader", daemon=True)
    reader.start()
    cleanup = None
    last_shutdown = None
    eof = False
    code = 0
    try:
        while True:
            try:
                if replies.shutdown_flushed.is_set() and join_scheduler(.05):
                    break
                if replies.stalled():
                    replies.fault.set()
                if replies.fault.is_set() and cleanup is None:
                    code = 2
                    context = global_context()
                    cleanup = last_shutdown or controller.submit(request_type(uuid.uuid4().hex,"shutdown",{},context))
                if cleanup is not None and cleanup.done():
                    outcome = cleanup.result()
                    if outcome.phase == "completed" and outcome.result.get("unreleased") == []:
                        if join_scheduler(.05):
                            break
                    elif eof or replies.output_failed.is_set() or replies.stalled():
                        channel = "input unavailable" if eof else "output unavailable"
                        print("worker cleanup unresolved; " + channel + "; retained wait requires operator intervention",
                              file=sys.stderr)
                        # No automatic retry, spin, restart, or invented release evidence.
                        stopped.wait()
                try:
                    line = incoming.get(timeout=.05)
                except queue.Empty:
                    continue
                if line is None or isinstance(line, BaseException):
                    eof = True
                    if isinstance(line, BaseException):
                        replies.fault.set()
                        code = 2
                    if cleanup is None and not replies.shutdown_flushed.is_set():
                        context = global_context()
                        cleanup = last_shutdown or controller.submit(request_type(uuid.uuid4().hex,"shutdown",{},context))
                    continue
                try:
                    request = parse(line)
                except ProtocolError as error:
                    replies.reject(None, None, str(error))
                    continue
                try:
                    reason = replies.reserve(request)
                except ProtocolError as error:
                    replies.reject(request.id,request.context,str(error))
                    continue
                if reason:
                    replies.reject(request.id, request.context, reason)
                    continue
                try:
                    future = controller.submit(request)
                    if (request.method == "shutdown" and request.context ==
                            global_context()):
                        last_shutdown = future
                        cleanup = None  # An explicit retry replaces the supervised prior attempt.
                    future.add_done_callback(lambda done, request=request: replies.publish(request, done))
                except BaseException as error:
                    failed = Future()
                    failed.set_exception(error)
                    replies.publish(request, failed)
            except BaseException as error:
                code = 2
                print("worker ingress fault: " + str(error), file=sys.stderr)
                if cleanup is not None:
                    print("cleanup supervision failed; retained wait requires operator intervention", file=sys.stderr)
                    stopped.wait()
                context = global_context()
                cleanup = last_shutdown or controller.submit(request_type(uuid.uuid4().hex,"shutdown",{},context))
    finally:
        stopped.set()
        replies.closed.set()
        replies.thread.join(1)
    return code



class _BootstrapV3:
    """Startup gate only. Domain execution is installed by the v3 runtime."""
    def __init__(self, *, ownership_nonce, capture_spool=None):
        from .contracts_v3 import valid_id
        import secrets
        if not valid_id(ownership_nonce):
            raise ProtocolError("A 128-bit ownership nonce is required")
        self.ownership_nonce = ownership_nonce
        self.capture_spool = capture_spool
        self.session_id = secrets.token_hex(16)
        self.mode = "real"
        self.activated = False
        self.closed = False
        self.driver_open_calls = 0
        self.runtime = None

    def global_context(self):
        from .contracts_v3 import ContextV3
        return ContextV3(self.session_id,None,None,0)

    def handle(self, request):
        from .contracts_v3 import OutcomeV3
        context = self.global_context()
        if self.closed:
            return OutcomeV3("rejected_before_call",context,error={
                "type":"WorkerClosed","message":"The worker is closed"})
        if request.context is not None and request.context != context:
            return OutcomeV3("rejected_before_call",context,error={
                "type":"ContextMismatch","message":"Startup context does not match this worker"})
        if request.method in {"ping","status"}:
            if self.runtime is not None:
                return OutcomeV3('completed',context,result={**self.runtime.cached_status(),'activated':True})
            return OutcomeV3("completed",context,result={
                "protocol_version":3,"mode":self.mode,"session_id":self.session_id,
                "activated":self.activated,"connected":False,"domains":{},
                "python_executable":sys.executable,"project_root":str(PROJECT_ROOT),
                "environment_name":Path(sys.executable).parent.name,
            })
        if request.method == "shutdown":
            if self.runtime is not None:
                return self.runtime.submit(request).result()
            self.closed = True
            return OutcomeV3("completed",context,result={"steps":[],"unreleased":[],"voltage_zero":None,"domains":{}})
        if request.method == "activate":
            if self.activated or request.params["ownership_nonce"] != self.ownership_nonce:
                return OutcomeV3("rejected_before_call",context,error={
                    "type":"OwnershipRejected","message":"Ownership activation is invalid or already consumed"})
            from .controller import DomainController
            from .captures import CaptureSpool
            try:
                spool = (None if self.capture_spool is None else
                         CaptureSpool(self.capture_spool, self.ownership_nonce))
                self.runtime=DomainController(session_id=self.session_id,host_verification=True,
                    capture_spool=spool,ownership_nonce=self.ownership_nonce)
            except Exception as error:
                return OutcomeV3('rejected_before_call',context,error={
                    'type':type(error).__name__,'message':'Host capture staging could not be validated'})
            self.activated = True
            return OutcomeV3("completed",context,result={"activated":True})
        return OutcomeV3("rejected_before_call",context,error={
            "type":"RuntimeUnavailable" if self.activated else "OwnershipRequired",
            "message":"Domain runtime is not configured" if self.activated else "Host ownership activation is required"})

    def config(self, ref):
        if self.runtime is None:
            raise ProtocolError('Host ownership activation is required')
        return self.runtime.registry.get(ref).config

    def join(self, timeout):
        return self.runtime._scheduler.join(timeout) if self.runtime is not None else True

    def submit(self, request):
        if self.runtime is not None and request.method not in {'ping','status','activate'}:
            future=self.runtime.submit(request)
        else:
            future=Future()
            future.set_running_or_notify_cancel()
            future.set_result(self.handle(request))
        if request.method=='shutdown':
            def closed(done):
                outcome=done.result()
                if outcome.phase=='completed' and outcome.result.get('unreleased')==[]:
                    self.closed=True
            future.add_done_callback(closed)
        return future

def _bootstrap_v3(*, ownership_nonce, capture_spool=None):
    return _BootstrapV3(ownership_nonce=ownership_nonce,capture_spool=capture_spool)

def run_v3(controller, *, input_stream, output_stream):
    return run(controller,input_stream=input_stream,output_stream=output_stream,settings_path=None,protocol=3)


def main(argv=None):
    parser = _parser()
    args = parser.parse_args(argv)
    from .contracts_v3 import valid_id
    if args.protocol == 3 and not valid_id(args.ownership_nonce):
        parser.error("protocol 3 requires --ownership-nonce with 32 lowercase hexadecimal characters")
    if args.protocol == 2 and (args.ownership_nonce is not None or args.capture_spool is not None):
        parser.error("Host-owned nonce and capture spool are only supported by protocol 3")
    protocol_stdout = sys.stdout
    sys.stdout = sys.stderr
    if args.protocol == 3:
        controller = _bootstrap_v3(ownership_nonce=args.ownership_nonce,capture_spool=args.capture_spool)
        return run_v3(controller,input_stream=sys.stdin,output_stream=protocol_stdout)
    controller = ConsoleController()
    return run(controller, input_stream=sys.stdin, output_stream=protocol_stdout,
               settings_path=_settings_path(args.settings))


if __name__ == "__main__":
    raise SystemExit(main())
