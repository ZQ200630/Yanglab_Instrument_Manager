"""Bounded role dispatch with independent safety and observation workers."""

from __future__ import annotations

import copy
import json
import time
import uuid
from collections import deque
from concurrent.futures import Future
from dataclasses import asdict, dataclass, field
from threading import Condition, Thread, current_thread

from .contracts import Context, Observation, Outcome, Request, ROLES, MAX_EPOCH, classify, encode_v2, parse_v2
from .safety import SafetyCoordinator


_RETAINED_SAFETY = frozenset({"zero", "current_off", "tec_off"})
_SAFETY = _RETAINED_SAFETY | {"disconnect"}


class PostReadback(dict):
    """Successful public call whose reply needs the role observation slot."""


class CallbackFailure(Exception):
    """Explicit adapter evidence; never infer call entry from exception type."""

    def __init__(self, message, *, phase="rejected_before_call", status=None,
                 release_confirmed=False):
        if phase not in {"rejected_before_call", "failed_after_call_started", "completed_readback_failed"}:
            raise ValueError("invalid callback phase")
        if status is not None and type(status) is not dict:
            raise TypeError("callback status must be a dict")
        if type(message) is not str or not message.strip():
            raise ValueError("callback message must be nonempty")
        self.phase = phase
        self.status = copy.deepcopy(status or {})
        encode_v2("callback", Outcome("completed", None, self.status))
        self.release_confirmed = release_confirmed is True
        super().__init__(message)


@dataclass(frozen=True)
class _CallbackOutcome(Outcome):
    status_delta: dict = field(default_factory=dict)
    release_confirmed: bool = False


class _ReplyFuture(Future):
    """Isolate each consumer callback so later callbacks still receive evidence."""

    def __init__(self, record_callback_error):
        super().__init__()
        self._record_callback_error = record_callback_error

    def add_done_callback(self, fn):
        def guarded(future):
            try:
                fn(future)
            except BaseException as error:
                self._record_callback_error(error)
        # Future invokes callbacks after releasing its completion lock, including
        # callbacks registered after completion. Do not wrap its private locks.
        return super().add_done_callback(guarded)


@dataclass
class _Work:
    request: Request
    future: Future


@dataclass
class _Lane:
    context: Context
    state: str = "DISCONNECTED"
    revision: int = 0
    queue: deque = field(default_factory=deque)
    active: _Work | None = None
    safety: SafetyCoordinator | None = None
    observing: bool = False
    refresh: bool = False
    next_refresh: float = float("inf")
    status: dict | None = None
    readback: tuple[_Work, Outcome] | None = None
    safety_readback: bool = False
    epoch_exhausted: bool = False
    healthy_context: Context | None = None
    observation_revision: int | None = None


class Scheduler:
    """One ordinary and one observation worker per role; bounded global I/O.

    All callbacks run outside the Condition. Results keep their execution
    context even if it has since expired; only current results update caches.
    """

    def __init__(self, execute, observe, *, session_id: str, clock=time.monotonic,
                 role_kinds=None, fixed_pools=False):
        self._execute = execute
        self._observe = observe
        self._clock = clock
        self._global_context = Context(session_id, None, 0)
        self._condition = Condition()
        self._role_kinds = dict({role: role for role in ROLES} if role_kinds is None else role_kinds)
        self._fixed_pools = fixed_pools
        self._roles = {role: _Lane(Context(session_id, None, 0)) for role in sorted(self._role_kinds)}
        self._management = _Lane(self._global_context)
        self._revision = 0
        self._closing = False
        self._terminated = False
        # App-only hook: no I/O; verifies expected identity before forgetting ownership.
        self._release_role = lambda role, context: None
        self._invalidate_role = lambda role, context: None
        self._status_overlay = lambda role, context, status: status
        self._prepare_disconnect = None
        self._shutdown: _Work | None = None
        self._shutdown_outcome: Outcome | None = None
        self._active_ids = {}
        self._recent_ids = deque(maxlen=256)
        self._consumer_callback_errors = deque(maxlen=256)
        self._threads = []
        for role, lane in self._roles.items():
            self._init_safety(role, lane)
            if not fixed_pools:
                self._threads.append(Thread(target=self._ordinary_loop, args=(role,), name=f"console-{role}"))
                self._threads.append(Thread(target=self._observation_loop, args=(role,), name=f"observe-{role}"))
                self._threads.append(Thread(target=self._safety_loop, args=(role,), name=f"safety-{role}"))
        if fixed_pools:
            for count, target, name in ((4,self._ordinary_loop,'console'),(4,self._observation_loop,'observe'),(64,self._safety_loop,'safety')):
                self._threads.extend(Thread(target=target,args=(None,True),name=f'{name}-pool-{index}') for index in range(count))
        self._threads.append(Thread(target=self._ordinary_loop, args=(None,), name="console-management"))
        if not fixed_pools:
            self._threads.append(Thread(target=self._shutdown_loop, name="console-quiescence"))
        for thread in self._threads:
            thread.start()

    def _init_safety(self, role, lane):
        kind = self._role_kinds[role]
        lane.safety = SafetyCoordinator(kind,
            lambda intent, r=role: self._execute(self._safety_request(r,intent)),
            lambda l=lane: bool(l.active or l.observing or l.readback),
            self._condition.notify_all, context=lambda l=lane:l.context,
            defer_initial=lambda intent,l=lane,k=kind: bool(
                (l.active and l.active.request.method=='connect') or
                (intent=='disconnect' and (l.active or l.observing or l.readback) and
                 k in {'pm400','mdt','fiber','laser'})))

    def add_lane(self, key, kind):
        if not self._fixed_pools:
            raise ValueError('Dynamic lanes require the instance pool')
        with self._condition:
            if key in self._roles or len(self._roles)>=64 or self._closing:
                raise ValueError('Domain already exists, capacity exhausted, or closing')
            self._role_kinds[key]=kind
            lane=_Lane(Context(self._global_context.session_id,None,0))
            self._init_safety(key,lane)
            self._roles[key]=lane
            self._condition.notify_all()

    def _canonical_request(self, request):
        if not self._fixed_pools:
            return request
        params=copy.deepcopy(request.params)
        if request.method in {'connect','disconnect','action','resume'}:
            kind=self._role_kinds[params['role']]
            params['role']='pm400' if kind in {'mdt', 'laser'} else kind
        return Request(request.id,request.method,params,request.context)

    def _classify(self, request):
        return classify(self._canonical_request(request))

    def context(self, role: str) -> Context:
        with self._condition:
            return self._roles[role].context

    def status(self) -> dict:
        with self._condition:
            return copy.deepcopy({
                "session_id": self._global_context.session_id,
                "revision": self._revision, "closing": self._closing,
                "consumer_callback_errors": list(self._consumer_callback_errors),
                "devices": {role: self._overlay(role, lane) for role, lane in self._roles.items()
                            if lane.status is not None},
                "roles": {role: {**asdict(lane.context), "state": lane.state,
                                 "revision": lane.revision,
                                 "active_request_id": (lane.active.request.id if lane.active else
                                                       lane.readback[0].request.id if lane.readback else None),
                                 "readback_request_id": lane.readback[0].request.id if lane.readback else None,
                                 "pending_request_id": lane.queue[0].request.id if lane.queue else None,
                                 "safety_request_id": lane.safety.waiters[0][0] if lane.safety.active else None,
                                 "safety": lane.safety.snapshot(),
                                 "observing": lane.observing}
                          for role, lane in self._roles.items()},
            })

    def _overlay(self, role, lane):
        # Hooks are bounded metadata only, under Condition -> registry -> store.
        try:
            return copy.deepcopy(self._status_overlay(role, lane.context, copy.deepcopy(lane.status)))
        except BaseException as error:
            status = copy.deepcopy(lane.status)
            status['status_error'] = 'evidence overlay failed: ' + type(error).__name__
            for field in status.get('fields', {}).values():
                field['quality'] = 'unknown'
            lane.healthy_context = None
            return status

    def join(self, timeout: float) -> bool:
        # Real elapsed time bounds join even when a deterministic test clock is injected.
        deadline = time.monotonic() + max(0.0, timeout)
        for thread in self._threads:
            if thread is not current_thread():
                thread.join(max(0.0, deadline - time.monotonic()))
        return not any(thread.is_alive() for thread in self._threads)

    @staticmethod
    def _error(context, kind, message, phase="rejected_before_call"):
        return Outcome(phase, context, error={"type": kind, "message": message})

    def _touch(self, lane):
        lane.revision += 1
        self._revision += 1

    def _record_callback_error(self, request_id, error):
        # Exception formatting can itself invoke consumer code. Keep it outside
        # the scheduler lock and prevent a broken __str__ from escaping again.
        try:
            message = str(error)[:4096] or type(error).__name__
        except BaseException:
            message = "exception message unavailable"
        diagnostic = {"request_id": request_id,
                      "error": {"type": type(error).__name__, "message": message}}
        with self._condition:
            self._consumer_callback_errors.append(diagnostic)
            self._revision += 1
            self._condition.notify_all()

    def _terminal(self, work, outcome):
        with self._condition:
            if self._active_ids.get(work.request.id) is work:
                del self._active_ids[work.request.id]
                self._recent_ids.append(work.request.id)
        # Future callbacks are arbitrary consumer code; never run them under our lock.
        work.future.set_result(outcome)

    def _invalidate(self, lane):
        if lane is not self._management:
            lane.epoch_exhausted |= lane.context.epoch == MAX_EPOCH
            lane.context = Context(lane.context.session_id, lane.context.connection_id,
                                   min(MAX_EPOCH, lane.context.epoch + 1))
        lane.healthy_context = None
        if lane is not self._management:
            role = next(role for role, item in self._roles.items() if item is lane)
            try:
                self._invalidate_role(role, lane.context)
            except BaseException as error:
                lane.status = {**(lane.status or {}),
                    'status_error': 'evidence invalidation failed: ' + type(error).__name__}
        lane.refresh = False
        self._touch(lane)
        invalidated = []
        if lane.readback:
            work, _ = lane.readback
            lane.readback = None
            if lane.safety_readback:
                lane.safety_readback = False
                lane.safety.running = False
            invalidated.append((work, self._error(work.request.context, "StaleReadback",
                "command completed, but status readback superseded by stop/context change",
                "completed_readback_failed")))
        while lane.queue:
            work = lane.queue.popleft()
            invalidated.append((work, self._error(lane.context, "StaleContext", "context changed",
                                                "superseded_before_call")))
        return invalidated

    def submit(self, request: Request) -> Future:
        future = _ReplyFuture(lambda error, request_id=getattr(request, "id", "invalid"):
                              self._record_callback_error(request_id, error))
        # Timeout/cancel by a consumer cannot erase admitted work or its evidence.
        future.set_running_or_notify_cancel()
        invalidated = []
        immediate = None
        shared_shutdown = None
        safety_future = None
        try:
            if not isinstance(request, Request):
                raise TypeError("request must be a Request")
            canonical = self._canonical_request(request)
            validated = parse_v2(json.dumps({"v": 2, "id": request.id, "method": request.method,
                "params": canonical.params,
                "context": asdict(request.context) if request.context is not None else None}, allow_nan=False))
            request = Request(validated.id,validated.method,request.params,validated.context)
            kind = self._classify(request)
        except Exception as error:
            future.set_result(self._error(self._global_context, type(error).__name__, str(error)))
            return future
        work = _Work(request, future)
        with self._condition:
            role = request.params.get("role") if request.method in {"connect", "disconnect", "action", "resume"} else None
            lane = self._roles[role] if role else self._management
            context = lane.context
            duplicate = request.id in self._active_ids or request.id in self._recent_ids
            if not duplicate:
                # Even cache-only replies own their ID until terminalized. A
                # concurrent submit must never slip through the reply boundary.
                self._active_ids[request.id] = work
            if duplicate:
                immediate = self._error(context, "DuplicateRequest", "request id already used")
            elif request.context != context and not (
                request.method in {"ping", "status"} and request.context is None
            ) and not (
                kind in _SAFETY and request.context is not None
                and request.context.session_id == context.session_id
                and request.context.connection_id == context.connection_id
                and request.context.epoch <= context.epoch
            ):
                immediate = self._error(context, "StaleContext", "context changed")
            elif request.method in {"ping", "status"}:
                immediate = Outcome("completed", self._global_context, self.status())
            elif request.method == "shutdown":
                if self._shutdown is not None:
                    shared_shutdown = self._shutdown.future
                else:
                    self._closing = True
                    self._shutdown = work
                    for item in [*self._roles.values(), self._management]:
                        invalidated.extend(self._invalidate(item))
                        item.state = "CLOSING"
                    for role_name, item in self._roles.items():
                        if item.context.connection_id is not None:
                            if not item.safety.active or item.safety.stronger('disconnect'):
                                item.safety.request('disconnect', request.id + ':' + role_name)
            elif self._closing and kind not in _SAFETY:
                immediate = self._error(context, "Closing", "session is closing")
            elif kind == "resume":
                if (lane.state == 'STOP_HELD' and not lane.safety.active and not lane.active
                        and not lane.observing and not lane.readback and not lane.epoch_exhausted
                        and lane.healthy_context == lane.context):
                    lane.state = 'READY'
                    self._touch(lane)
                    immediate = Outcome('completed', lane.context, {'resumed': True})
                else:
                    immediate = self._error(context, 'ResumeRestricted',
                        'final stop, healthy current observation and quiescent role are required')
            elif kind in _SAFETY:
                if context.connection_id is None:
                    immediate = self._error(context, "Disconnected", "role has no owned connection")
                else:
                    if not lane.safety.active or lane.safety.stronger(kind):
                        invalidated.extend(self._invalidate(lane))
                        lane.state = 'CLOSING' if kind == 'disconnect' else 'STOPPING'
                    safety_future = lane.safety.request(kind, request.id)
            elif lane.epoch_exhausted:
                immediate = self._error(context, 'EpochExhausted', 'epoch limit reached; explicitly close this session')
            elif role and request.method == "connect" and context.connection_id is not None:
                immediate = self._error(context, "AlreadyOwned", "connection or failed attempt is retained")
            elif role and request.method != "connect" and lane.state != "READY":
                immediate = self._error(context, "NotReady", "role is not ready for ordinary calls")
            elif lane.queue or (self._fixed_pools and role and (lane.active or lane.readback)) or (
                    sum(self._classify(item.request) not in _SAFETY | {'query','shutdown'} for item in self._active_ids.values()) > 31
                    if self._fixed_pools else len(self._active_ids) > 32):
                immediate = self._error(context, "QueueFull", "ordinary request capacity exhausted")
            else:
                if request.method == "connect":
                    lane.context = Context(context.session_id, uuid.uuid4().hex, context.epoch)
                    lane.state = "CONNECTING"
                    work.request = Request(request.id, request.method, request.params, lane.context)
                    self._touch(lane)
                lane.queue.append(work)
                self._touch(lane)
            self._condition.notify_all()
        for old, outcome in invalidated:
            self._terminal(old, outcome)
        if shared_shutdown is not None:
            def complete_shared(completed):
                outcome = completed.result()
                self._terminal(work, Outcome(outcome.phase, outcome.context, outcome.result, outcome.error))
            shared_shutdown.add_done_callback(complete_shared)
        if safety_future is not None:
            safety_future.add_done_callback(lambda completed: self._terminal(work, completed.result()))
        if immediate is not None:
            # A duplicate rejection must not remove the original active ID.
            if immediate.error and immediate.error["type"] == "DuplicateRequest":
                future.set_result(immediate)
            else:
                self._terminal(work, immediate)
        return future

    def _allows_observation(self, work):
        return bool(work and work.request.method == "action"
                    and self._role_kinds.get(work.request.params.get("role")) == "gain"
                    and work.request.params.get("name") == "wait_stable")

    def _invoke(self, request, execute=None):
        try:
            result = (execute or self._execute)(request)
        except CallbackFailure as error:
            return _CallbackOutcome(error.phase, request.context,
                error={"type": type(error).__name__, "message": str(error)},
                status_delta=copy.deepcopy(error.status), release_confirmed=error.release_confirmed)
        except BaseException as error:
            return self._error(request.context, type(error).__name__, str(error) or type(error).__name__,
                               "failed_after_call_started")
        try:
            if isinstance(result, PostReadback) and type(result.get("status", {})) is not dict:
                raise TypeError("post-readback status must be a dict")
            outcome = Outcome("completed", request.context, result)
            encode_v2(request.id, outcome)
            return outcome
        except BaseException as error:
            return self._error(request.context, type(error).__name__, str(error) or type(error).__name__,
                               "completed_readback_failed")

    def _ordinary_loop(self, role, pooled=False):
        lane = None if pooled else self._roles[role] if role else self._management
        while True:
            shutdown = None
            with self._condition:
                while True:
                    if self._closing:
                        if not pooled and role is None and self._fixed_pools:
                            if self._terminated:
                                return
                            if self._shutdown is not None and not any(
                                    item.active or item.observing or item.readback or (item.safety and item.safety.active)
                                    for item in [*self._roles.values(),self._management]):
                                shutdown=self._shutdown
                                break
                            self._condition.wait()
                            continue
                        return
                    if pooled:
                        ready=next(((key,item) for key,item in self._roles.items() if item.queue
                            and not item.active and not item.readback
                            and (not item.observing or self._allows_observation(item.queue[0]))),None)
                        if ready is None:
                            self._condition.wait()
                            continue
                        role,lane=ready
                    candidate = lane.queue[0] if lane.queue else None
                    if candidate and not lane.readback and (not lane.observing or self._allows_observation(candidate)):
                        break
                    self._condition.wait()
                work = None if shutdown else lane.queue.popleft()
                stale = not shutdown and work.request.context != lane.context
                if not shutdown and not stale:
                    # Admission to active precedes callback entry, for later safety barriers.
                    lane.active = work
                    self._touch(lane)
                    self._condition.notify_all()
            if shutdown:
                self._complete_shutdown(shutdown)
                continue
            if stale:
                self._terminal(work, self._error(lane.context, "StaleContext", "context changed",
                                                 "superseded_before_call"))
                continue
            outcome = self._invoke(work.request)
            if (role and work.request.method == 'connect' and isinstance(outcome, _CallbackOutcome)
                    and outcome.release_confirmed):
                # Driver connect and its synchronous Session cleanup have both
                # returned. Keep the active slot reserved until registry-only
                # finalization returns, preventing a competing safety invocation.
                try:
                    self._release_role(role, work.request.context)
                except BaseException as error:
                    outcome = _CallbackOutcome('failed_after_call_started', work.request.context,
                        error={'type': type(error).__name__,
                               'message': outcome.error['message'] + '; release finalization failed: ' + str(error)},
                        status_delta=outcome.status_delta, release_confirmed=False)
            invalidated = []
            deferred = False
            with self._condition:
                lane.active = None
                if lane.safety:
                    lane.safety.normal_finished()
                self._touch(lane)
                if work.request.context == lane.context and not lane.epoch_exhausted and not self._closing and not (
                        lane.safety and lane.safety.active):
                    if role:
                        if outcome.phase == "completed" and isinstance(outcome.result, PostReadback):
                            lane.status = {**(lane.status or {}), **copy.deepcopy(outcome.result.get("status", {}))}
                            lane.readback = (work, outcome)
                            lane.refresh = True
                            deferred = True
                        else:
                            self._publish_execution(lane, work.request, outcome)
                        if lane.state == "FAULT":
                            while lane.queue:
                                pending = lane.queue.popleft()
                                invalidated.append((pending, self._error(
                                    lane.context, "RoleFault", "preceding call faulted",
                                    "superseded_before_call")))
                self._condition.notify_all()
            for pending, superseded in invalidated:
                self._terminal(pending, superseded)
            if not deferred:
                if isinstance(outcome.result, PostReadback):
                    outcome = self._error(work.request.context, "StaleReadback",
                        "command completed, but status readback superseded by stop/context change",
                        "completed_readback_failed")
                self._terminal(work, outcome)

    def _publish_execution(self, lane, request, outcome):
        if isinstance(outcome, _CallbackOutcome):
            if outcome.status_delta:
                lane.status = {**(lane.status or {}), **copy.deepcopy(outcome.status_delta)}
            if request.method == "connect" and outcome.release_confirmed:
                lane.context = Context(lane.context.session_id, None, lane.context.epoch)
                lane.state, lane.status = "DISCONNECTED", None
                return
            if outcome.phase == "rejected_before_call":
                return
        if outcome.phase != "completed":
            lane.state = "FAULT"
            lane.status = {**(lane.status or {}), "dispatch_error": copy.deepcopy(outcome.error)}
            return
        result = outcome.result
        if request.method == "disconnect":
            if isinstance(result, dict) and result.get("connected") is False:
                lane.context = Context(lane.context.session_id, None, lane.context.epoch)
                lane.state, lane.status = "DISCONNECTED", None
            else:
                lane.state = "FAULT"
            return
        if request.method == "connect":
            lane.state = "READY" if isinstance(result, dict) and result.get("connected") is True else "FAULT"
        elif self._classify(request) in _RETAINED_SAFETY:
            lane.state = "STOP_HELD"
        if isinstance(result, dict):
            snapshot = result.get("status")
            if isinstance(snapshot, dict):
                lane.status = {**(lane.status or {}), **copy.deepcopy(snapshot)}
            elif lane.status is None:
                lane.status = {"connected": lane.state == "READY"}
        if lane.state in {"READY", "STOP_HELD"}:
            lane.refresh = True
            lane.next_refresh = self._clock() + 2.5

    def _observation_loop(self, role, pooled=False):
        lane = None if pooled else self._roles[role]
        while True:
            with self._condition:
                while True:
                    if self._closing:
                        return
                    now = self._clock()
                    if pooled:
                        ready=None
                        delays=[]
                        for key,item in self._roles.items():
                            if item.state in {'READY','STOP_HELD'}:
                                if now>=item.next_refresh:
                                    item.refresh=True
                                    item.next_refresh=now+2.5
                                delays.append(max(.001,item.next_refresh-now))
                            if not item.observing and ((item.readback and (not item.safety.active or item.safety_readback)) or (
                                    item.state in {'READY','STOP_HELD'} and item.refresh and not item.queue
                                    and not item.safety.active and (not item.active or self._allows_observation(item.active)))):
                                ready=key,item
                                break
                        if ready is None:
                            self._condition.wait(min(delays) if delays else None)
                            continue
                        role,lane=ready
                    if lane.state in {"READY", "STOP_HELD"} and now >= lane.next_refresh:
                        lane.refresh = True
                        lane.next_refresh = now + 2.5
                    if (lane.readback and (not lane.safety.active or lane.safety_readback)) or (lane.state in {"READY", "STOP_HELD"} and lane.refresh and not lane.queue and not lane.safety.active
                            and (lane.active is None or self._allows_observation(lane.active))):
                        break
                    delay = max(0.001, lane.next_refresh - now) if lane.state in {"READY", "STOP_HELD"} else None
                    self._condition.wait(delay)
                required = lane.readback
                superseded_before_read = bool(required and lane.queue)
                lane.observing = not superseded_before_read
                lane.refresh = False
                context, revision = lane.context, lane.revision
                lane.observation_revision = revision
                if superseded_before_read:
                    lane.readback = None
                    self._touch(lane)
                    self._condition.notify_all()
            if superseded_before_read:
                work, _ = required
                self._terminal(work, self._error(work.request.context, "ReadbackSuperseded",
                    "command completed, but status readback superseded by a subsequent ordinary request",
                    "completed_readback_failed"))
                continue
            try:
                observation = self._observe(role, context)
                if not isinstance(observation, Observation):
                    raise TypeError("observe must return Observation")
                if not isinstance(observation.status, dict) or type(observation.more) is not bool:
                    raise TypeError("observation requires status dict and boolean more")
                reported_error = observation.status.get('observation_error')
                if reported_error is not None and (
                        type(reported_error) is not dict or set(reported_error) != {'type', 'message'}
                        or any(type(value) is not str or not value for value in reported_error.values())):
                    raise TypeError('observation error requires nonempty type and message')
                encode_v2("observation", Outcome("completed", context, observation.status))
                # Gain reports partial successes through the same public shape;
                # a failed round still faults a required command readback.
                error = reported_error if not observation.more else None
            except BaseException as cause:
                observation = Observation({})
                error = {"type": type(cause).__name__, "message": str(cause)}
            completion = None
            invalidated = []
            with self._condition:
                lane.observing = False
                supersede_readback = bool(required and lane.readback is required and lane.queue and not error)
                if context == lane.context and not self._closing and not lane.epoch_exhausted and (not lane.safety.active or lane.safety_readback) and (
                    revision == lane.revision or (required and lane.readback is required and error)
                ):
                    lane.status = {**(lane.status or {}), **copy.deepcopy(observation.status)}
                    if error:
                        lane.status["observation_error"] = error
                        lane.status["status_error"] = f"{error['type']}: {error['message']}"
                    elif not observation.status.get('observation_error'):
                        lane.status.pop("observation_error", None)
                        lane.status.pop("status_error", None)
                    lane.healthy_context = (context if not error and not observation.more
                        and not observation.status.get('observation_error')
                        and lane.status.get('state') in {'READY', 'ACTIVE'} else None)
                    self._touch(lane)
                    lane.refresh |= observation.more
                    if required and lane.readback is required and not supersede_readback:
                        work, original = required
                        if error:
                            prefix = "connection established" if work.request.method == "connect" else "command completed"
                            outcome = self._error(context, error["type"],
                                f"{prefix}, but status readback failed: {error['message']}; inspect before retrying",
                                "completed_readback_failed")
                        elif not observation.more:
                            outcome = Outcome("completed", context, {**original.result, "status": copy.deepcopy(lane.status)})
                        else:
                            outcome = None
                        if outcome is not None:
                            lane.readback = None
                            self._publish_execution(lane, work.request, outcome)
                            # Required refresh is already complete.
                            lane.refresh = False
                            completion = (work, outcome)
                            if lane.state == "FAULT":
                                while lane.queue:
                                    pending = lane.queue.popleft()
                                    invalidated.append((pending, self._error(lane.context, "RoleFault",
                                        "preceding readback faulted", "superseded_before_call")))
                elif required and lane.readback is required and not supersede_readback:
                    # A queued admission changed revision, so repeat the read
                    # before publishing; stop removes the readback responsibility.
                    lane.refresh = True
                if supersede_readback:
                    work, _ = lane.readback
                    lane.readback = None
                    lane.refresh = False
                    completion = (work, self._error(work.request.context, "ReadbackSuperseded",
                        "command completed, but status readback superseded by a subsequent ordinary request",
                        "completed_readback_failed"))
                    self._touch(lane)
                self._condition.notify_all()
            for work, outcome in invalidated:
                self._terminal(work, outcome)
            if completion:
                self._terminal(*completion)

    def _safety_request(self, role, intent):
        lane = self._roles[role]
        with self._condition:
            context = lane.context
            ident = lane.safety.attempt_id
        names = {'zero': 'zero', 'current_off': 'disable_current', 'tec_off': 'disable_tec'}
        params = {'role': role} if intent == 'disconnect' else {'role': role, 'name': names[intent]}
        return Request(ident, 'disconnect' if intent == 'disconnect' else 'action', params, context)

    def _finish_safety_readback(self, lane, token, future):
        with self._condition:
            coordinator = lane.safety
            if token != (coordinator.attempt_id, coordinator.intent, lane.context) or not lane.safety_readback:
                return
            reply = future.result()
            coordinator.last = Outcome(reply.phase, reply.context,
                dict(reply.result) if reply.result is not None else None, reply.error)
            coordinator.running = False
            lane.safety_readback = False
            self._condition.notify_all()

    def _safety_loop(self, role, pooled=False):
        lane = None if pooled else self._roles[role]
        coordinator = None if pooled else lane.safety
        while True:
            with self._condition:
                while True:
                    if self._terminated:
                        return
                    if pooled:
                        ready=None
                        for key,item in self._roles.items():
                            selected=item.safety.take()
                            if selected or item.safety.ready():
                                ready=key,item,selected
                                break
                        if ready is None:
                            self._condition.wait()
                            continue
                        role,lane,job=ready
                        coordinator=lane.safety
                        break
                    job = coordinator.take()
                    if job or coordinator.ready():
                        break
                    self._condition.wait()
                if not job:
                    if isinstance(coordinator.last.result, PostReadback) and lane.epoch_exhausted:
                        coordinator.last = self._error(lane.context, 'EpochExhausted',
                            'safe-off call completed; readback publication unavailable at epoch limit; explicitly close',
                            'completed_readback_failed')
                    if isinstance(coordinator.last.result, PostReadback) and not lane.epoch_exhausted:
                        token = (coordinator.attempt_id, coordinator.intent, lane.context)
                        future = _ReplyFuture(lambda error,ident=coordinator.attempt_id: self._record_callback_error(ident, error))
                        future.set_running_or_notify_cancel()
                        future.add_done_callback(lambda done,t=token,l=lane: self._finish_safety_readback(l,t,done))
                        names = {'zero': 'zero', 'current_off': 'disable_current', 'tec_off': 'disable_tec'}
                        request = Request(coordinator.attempt_id, 'action',
                            {'role': role, 'name': names[coordinator.intent]}, lane.context)
                        lane.readback = (_Work(request, future), coordinator.last)
                        lane.safety_readback = True
                        coordinator.running = True
                        lane.refresh = True
                        self._condition.notify_all()
                        continue
                    outcome = coordinator.outcome()
                    coordinator.running = True  # also serialize final registry release
                    release_context = lane.context
            if job:
                prepare = (job[0]=='disconnect' and job[1] and not job[2]
                           and self._prepare_disconnect is not None
                           and self._role_kinds[role] in {'gain','voltage'})
                outcome = self._invoke(self._safety_request(role, job[0]),
                    (lambda request: self._prepare_disconnect(role)) if prepare else
                    (lambda request: coordinator.invoke(job[0])))
                with self._condition:
                    coordinator.accept(job, outcome)
                    self._touch(lane)
                    self._condition.notify_all()
                continue
            released = (coordinator.intent == 'disconnect' and outcome.phase == 'completed'
                        and outcome.result.get('connected') is False)
            if released:
                try:
                    self._release_role(role, release_context)
                except BaseException as error:
                    outcome = self._error(release_context, type(error).__name__, str(error),
                                          'failed_after_call_started')
                    released = False
            with self._condition:
                coordinator.running = False
                if coordinator.pending:
                    self._condition.notify_all()
                    continue
                replies = coordinator.settle(outcome)
                if released:
                    lane.context = Context(lane.context.session_id, None, lane.context.epoch)
                    lane.state, lane.status = 'DISCONNECTED', None
                elif coordinator.intent == 'disconnect':
                    lane.state = 'RETAINED'
                else:
                    lane.state = 'STOP_HELD' if outcome.phase == 'completed' else 'RETAINED'
                    lane.refresh = outcome.phase == 'completed'
                    lane.next_refresh = self._clock() + 2.5
                if outcome.error:
                    lane.status = {**(lane.status or {}), 'dispatch_error': copy.deepcopy(outcome.error)}
                    if isinstance(coordinator.last, _CallbackOutcome):
                        lane.status.update(copy.deepcopy(coordinator.last.status_delta))
                self._touch(lane)
                self._condition.notify_all()
            for future, reply in replies:
                future.set_result(reply)

    def _shutdown_loop(self):
        while True:
            with self._condition:
                while self._shutdown is None:
                    self._condition.wait()
                work = self._shutdown
                while any(lane.active or lane.observing or lane.readback or (lane.safety and lane.safety.active)
                          for lane in [*self._roles.values(), self._management]):
                    self._condition.wait()
            self._complete_shutdown(work)
            if self._terminated:
                return

    def _complete_shutdown(self, work):
        outcome = self._invoke(work.request)
        with self._condition:
            self._shutdown_outcome = Outcome(outcome.phase,outcome.context,outcome.result,outcome.error)
            self._terminated = all(lane.context.connection_id is None for lane in self._roles.values())
            if not self._terminated:
                self._shutdown = None
            self._condition.notify_all()
        self._terminal(work,outcome)
