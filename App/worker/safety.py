"""Role safety responsibility; WAITING_OLD never occupies an execution worker.

The scheduler owns synchronization and runs selected calls outside its lock.
Only the injected callback knows the public-driver action boundary.
"""

import copy
import uuid
from concurrent.futures import Future
from dataclasses import asdict

from .contracts import Outcome


INTENT_ORDER = {'gain': ('current_off', 'tec_off', 'disconnect'),
                'voltage': ('zero', 'disconnect')}


class SafetyCoordinator:
    def __init__(self, role, invoke, normal_active, wake, *, context, defer_initial):
        self.role = role
        self.invoke = invoke
        self.normal_active = normal_active
        self.wake = wake
        self.context = context
        self.defer_initial = defer_initial
        self.state = 'RELEASED'
        self.intent = None
        self.attempt_id = None
        self.running = False
        self.pending = False
        self.final_needed = False
        self.last = None
        self.waiters = []
        self.attempts = []
        self.outcomes = []

    @property
    def active(self):
        return bool(self.waiters)

    def stronger(self, intent):
        order = INTENT_ORDER.get(self.role, ('disconnect',))
        return self.intent is None or order.index(intent) > order.index(self.intent)

    def request(self, intent: str, request_id: str) -> Future:
        future = Future()
        future.set_running_or_notify_cancel()
        if self.active and not self.stronger(intent):
            future.set_result(Outcome('rejected_before_call', self.context(), error={
                'type': 'AlreadyRunning', 'message': 'existing safety responsibility covers this intent',
                'attempt_id': self.attempt_id}))
            return future
        if not self.active:
            self.attempt_id = uuid.uuid4().hex
            self.last = None
            self.final_needed = False
        self.intent = intent
        self.pending = True
        self.waiters.append((request_id, future))
        self.state = 'CLOSING' if intent == 'disconnect' else 'REQUESTED'
        self.wake()
        return future

    def normal_finished(self) -> None:
        self.wake()

    def take(self):
        if not self.active or self.running:
            return None
        if self.pending:
            if self.defer_initial(self.intent):
                self.state = 'WAITING_OLD'
                return None
            self.pending = False
            final = False
        elif self.final_needed:
            if self.normal_active():
                self.state = 'WAITING_OLD'
                return None
            self.final_needed = False
            final = True
        else:
            return None
        self.running = True
        self.state = 'FINAL_RUNNING' if final else 'INITIAL_RUNNING'
        return self.intent, self.normal_active(), final

    def accept(self, job, outcome):
        intent, overlapped, final = job
        self.running = False
        self.attempts.append({'attempt_id': self.attempt_id, 'intent': intent, 'final': final,
                              'context': asdict(outcome.context) if outcome.context else None,
                              'phase': outcome.phase, 'result': copy.deepcopy(outcome.result),
                              'error': copy.deepcopy(outcome.error)})
        self.last = outcome
        if intent != self.intent:
            return
        deferred_disconnect = (intent == 'disconnect' and isinstance(outcome.result, dict)
                               and outcome.result.get('deferred_release') is True)
        self.final_needed = overlapped and not final and (intent != 'disconnect' or deferred_disconnect)
        if self.normal_active():
            self.state = 'WAITING_OLD'

    def ready(self):
        return (self.active and not self.running and not self.pending and not self.final_needed
                and self.last is not None and not self.normal_active())

    def outcome(self):
        original = self.last
        result = dict(original.result or {}) if original.phase == 'completed' else None
        if result is not None:
            result.update(effective_intent=self.intent, attempt_id=self.attempt_id)
        return Outcome(original.phase, self.context(), result, original.error)

    def settle(self, outcome):
        if outcome.error is not None:
            # Annotate at the final terminal boundary, including registry-release
            # failures that can replace the initial coordinator outcome.
            outcome = Outcome(outcome.phase, outcome.context, error={
                **outcome.error, 'attempt_id': self.attempt_id,
                'message': f"{outcome.error['message']}; effective_intent={self.intent}"})
        self.outcomes.append({'attempt_id': self.attempt_id, 'effective_intent': self.intent,
                              **asdict(outcome)})
        released = self.intent == 'disconnect' and outcome.phase == 'completed' \
            and outcome.result.get('connected') is False
        self.state = ('RELEASED' if released else 'STOP_HELD' if
                      self.intent != 'disconnect' and outcome.phase == 'completed' else 'RETAINED')
        waiters, self.waiters = self.waiters, []
        return [(future, Outcome(outcome.phase, outcome.context, outcome.result, outcome.error))
                for _, future in waiters]

    def snapshot(self) -> dict:
        return copy.deepcopy({'state': self.state, 'effective_intent': self.intent,
                              'attempt_id': self.attempt_id,
                              'request_ids': [ident for ident, _ in self.waiters],
                              'attempts': self.attempts, 'outcomes': self.outcomes})
