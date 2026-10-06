"""Offline dispatch tests: callbacks are the only fake hardware boundary."""

import threading
import unittest
from contextlib import contextmanager
from concurrent.futures import TimeoutError as FutureTimeoutError
from unittest.mock import patch

from App.worker.contracts import Context, Observation, Request, encode_v2
from App.worker.scheduler import Scheduler
from App.worker import scheduler as dispatch


class DispatchTests(unittest.TestCase):
    def setUp(self):
        self.serial = 0

    def send(self, scheduler, method, role=None, context=None, **params):
        self.serial += 1
        if role:
            params["role"] = role
        if context is None:
            context = scheduler.context(role) if role else Context("s1", None, 0)
        return scheduler.submit(Request(str(self.serial), method, params, context))

    @contextmanager
    def running(self, execute=None, observe=None, releases=()):
        def fake_lifecycle(req):
            result = execute(req) if execute else {'connected': req.method != 'disconnect'}
            # These callback fakes own no resources. Their close completion is
            # explicit now that global shutdown dispatches each role's close.
            return {'connected': False} if req.method == 'disconnect' else result
        scheduler = Scheduler(fake_lifecycle,
                              observe or (lambda role, ctx: Observation({})), session_id="s1")
        try:
            yield scheduler
        finally:
            for event in releases:
                event.set()
            self.send(scheduler, "shutdown").result(4)
            self.assertTrue(scheduler.join(4))

    def connect(self, scheduler, role):
        self.assertEqual(self.send(scheduler, "connect", role, resource="COM1").result(2).phase,
                         "completed")

    def test_direct_malformed_requests_are_rejected_without_raising(self):
        with self.running() as scheduler:
            for request in (None, Request([], "status", {}, None),
                            Request("", "status", {}, None),
                            Request("bad-ctx", "action", {"role": "gain", "name": "x"}, {}),
                            Request("bad-epoch", "status", {}, Context("s1", None, True))):
                with self.subTest(request=request):
                    self.assertEqual(scheduler.submit(request).result(1).phase, "rejected_before_call")

    def test_required_readback_uses_observation_slot_and_delays_completion(self):
        self.assertTrue(hasattr(dispatch, "PostReadback"), "callback readback handoff is missing")
        entered, release = threading.Event(), threading.Event()
        def execute(req):
            return dispatch.PostReadback({"connected": True})
        def observe(role, context):
            self.assertTrue(threading.current_thread().name.startswith("observe-"))
            entered.set()
            self.assertTrue(release.wait(3))
            return Observation({"value": 7})
        with self.running(execute, observe, [release]) as scheduler:
            future = self.send(scheduler, "connect", "voltage", resource="COM1")
            self.assertTrue(entered.wait(2))
            self.assertFalse(future.done())
            self.assertEqual(scheduler.status()["roles"]["voltage"]["state"], "CONNECTING")
            release.set()
            outcome = future.result(2)
            self.assertEqual(outcome.phase, "completed")
            self.assertEqual(outcome.result["status"]["value"], 7)

    def test_stop_settles_pending_readback_without_publishing_old_ready(self):
        self.assertTrue(hasattr(dispatch, "PostReadback"), "callback readback handoff is missing")
        entered, release = threading.Event(), threading.Event()
        def execute(req):
            return dispatch.PostReadback({"connected": True}) if req.method == "connect" else {"connected": False}
        def observe(role, context):
            entered.set()
            self.assertTrue(release.wait(3))
            return Observation({"value": "old"})
        with self.running(execute, observe, [release]) as scheduler:
            connecting = self.send(scheduler, "connect", "osa", resource="GPIB0::1::INSTR")
            self.assertTrue(entered.wait(2))
            stop = self.send(scheduler, "disconnect", "osa")
            self.assertEqual(connecting.result(1).phase, "completed_readback_failed")
            self.assertEqual(scheduler.status()["roles"]["osa"]["state"], "CLOSING")
            release.set()
            self.assertEqual(stop.result(2).phase, "completed")
            self.assertNotIn("osa", scheduler.status()["devices"])

    def test_required_readback_failure_keeps_call_phase_and_faults_role(self):
        self.assertTrue(hasattr(dispatch, "PostReadback"), "callback readback handoff is missing")
        def observe(role, context):
            raise OSError("readback unavailable")
        with self.running(lambda req: dispatch.PostReadback({"connected": True}), observe) as scheduler:
            outcome = self.send(scheduler, "connect", "gain", resource="COM2").result(2)
            self.assertEqual(outcome.phase, "completed_readback_failed")
            self.assertIn("readback unavailable", outcome.error["message"])
            self.assertEqual(scheduler.status()["roles"]["gain"]["state"], "FAULT")

    def test_queued_ordinary_wins_at_required_readback_field_boundary(self):
        entered, release = threading.Event(), threading.Event()
        calls, reads = [], []
        def execute(req):
            if req.method == "action":
                calls.append(req.params["name"])
                return dispatch.PostReadback({"result": req.params["name"]})
            return {"connected": True}
        def observe(role, context):
            if calls == ["first"]:
                reads.append("first-field")
                entered.set()
                self.assertTrue(release.wait(3))
                return Observation({"partial": 1}, more=True)
            return Observation({"complete": True})
        with self.running(execute, observe, [release]) as scheduler:
            self.connect(scheduler, "gain")
            first = self.send(scheduler, "action", "gain", name="first")
            self.assertTrue(entered.wait(2))
            second = self.send(scheduler, "action", "gain", name="second")
            self.assertFalse(second.done())
            self.assertEqual(calls, ["first"])
            release.set()
            result = first.result(2)
            self.assertEqual(result.phase, "completed_readback_failed")
            self.assertEqual(result.error["type"], "ReadbackSuperseded")
            self.assertEqual(second.result(2).phase, "completed")
            self.assertEqual(calls, ["first", "second"])
            self.assertEqual(reads, ["first-field"])
            self.assertEqual(scheduler.status()["roles"]["gain"]["state"], "READY")
            self.assertIsNone(scheduler.status()["roles"]["gain"]["active_request_id"])

    def test_callback_failure_rejects_non_dict_cache_evidence(self):
        with self.assertRaises(TypeError):
            dispatch.CallbackFailure("rejected", status=["not", "a", "delta"])

    def test_required_read_error_faults_even_when_ordinary_admission_changed_revision(self):
        entered, release = threading.Event(), threading.Event()
        calls, reads = [], []
        def execute(req):
            if req.method == "action":
                calls.append(req.params["name"])
                return dispatch.PostReadback({"result": None})
            return {"connected": True}
        def observe(role, context):
            if calls:
                reads.append("field")
                entered.set()
                self.assertTrue(release.wait(3))
                if len(reads) == 1:
                    raise OSError("required field failed")
            return Observation({})
        with self.running(execute, observe, [release]) as scheduler:
            self.connect(scheduler, "gain")
            first = self.send(scheduler, "action", "gain", name="first")
            self.assertTrue(entered.wait(2))
            second = self.send(scheduler, "action", "gain", name="second")
            release.set()
            self.assertEqual(first.result(2).phase, "completed_readback_failed")
            self.assertEqual(second.result(2).phase, "superseded_before_call")
            self.assertEqual(calls, ["first"])
            self.assertEqual(reads, ["field"])
            self.assertEqual(scheduler.status()["roles"]["gain"]["state"], "FAULT")

    def test_queued_ordinary_does_not_start_an_unentered_old_readback(self):
        entered, release = threading.Event(), threading.Event()
        calls, reads = [], []
        def execute(req):
            if req.method == "action":
                calls.append(req.params["name"])
                if req.params["name"] == "first":
                    entered.set()
                    self.assertTrue(release.wait(3))
                return dispatch.PostReadback({"result": None})
            return {"connected": True}
        def observe(role, context):
            reads.append(tuple(calls))
            return Observation({})
        with self.running(execute, observe, [release]) as scheduler:
            self.connect(scheduler, "gain")
            first = self.send(scheduler, "action", "gain", name="first")
            self.assertTrue(entered.wait(2))
            second = self.send(scheduler, "action", "gain", name="second")
            release.set()
            self.assertEqual(first.result(2).error["type"], "ReadbackSuperseded")
            self.assertEqual(second.result(2).phase, "completed")
            self.assertNotIn(("first",), reads)

    def test_malformed_post_readback_delta_cannot_strand_worker(self):
        with self.running(lambda req: dispatch.PostReadback({"status": [1]})) as scheduler:
            self.assertEqual(self.send(scheduler, "connect", "osa").result(2).phase,
                             "completed_readback_failed")

    def test_rejected_callback_preserves_ready_and_snapshots_cache_delta(self):
        status = {"requested": [None]}
        def execute(req):
            if req.method == "action":
                raise dispatch.CallbackFailure("invalid parameter", status=status)
            return {"connected": True}
        with self.running(execute) as scheduler:
            self.connect(scheduler, "voltage")
            result = self.send(scheduler, "action", "voltage", name="bad").result(2)
            self.assertEqual(result.phase, "rejected_before_call")
            status["requested"].append(99)
            self.assertEqual(scheduler.status()["devices"]["voltage"]["requested"], [None])
            self.assertEqual(scheduler.status()["roles"]["voltage"]["state"], "READY")

    def test_other_role_runs_before_osa_finishes(self):
        entered, release = threading.Event(), threading.Event()
        def execute(req):
            if req.method == "action" and req.params["role"] == "osa":
                entered.set()
                self.assertTrue(release.wait(3))
            return {"connected": True}
        with self.running(execute, releases=[release]) as scheduler:
            self.connect(scheduler, "osa")
            self.connect(scheduler, "voltage")
            active = self.send(scheduler, "action", "osa", name="acquire")
            self.assertTrue(entered.wait(2))
            self.assertEqual(self.send(scheduler, "action", "voltage", name="set_channel").result(2).phase,
                             "completed")
            self.assertFalse(active.done())
            self.assertEqual(scheduler.status()["session_id"], "s1")

    def test_only_one_pending_and_submit_snapshots_params(self):
        entered, release = threading.Event(), threading.Event()
        seen = []
        def execute(req):
            if req.method == "action":
                seen.append(req.params["value"])
                if req.params["value"] == [1]:
                    entered.set()
                    self.assertTrue(release.wait(3))
            return {"connected": True}
        with self.running(execute, releases=[release]) as scheduler:
            self.connect(scheduler, "voltage")
            first = self.send(scheduler, "action", "voltage", name="set_channel", value=[1])
            self.assertTrue(entered.wait(2))
            request = Request("queued", "action", {"role": "voltage", "name": "set_channel", "value": [2]},
                              scheduler.context("voltage"))
            queued = scheduler.submit(request)
            request.params["value"].append(99)
            rejected = self.send(scheduler, "action", "voltage", name="set_channel", value=[3]).result(1)
            self.assertEqual(rejected.phase, "rejected_before_call")
            self.assertEqual(rejected.error["type"], "QueueFull")
            release.set()
            self.assertEqual(first.result(2).phase, "completed")
            self.assertEqual(queued.result(2).phase, "completed")
            self.assertEqual(seen, [[1], [2]])

    def test_reconnect_same_resource_never_accepts_old_identity(self):
        with self.running() as scheduler:
            self.connect(scheduler, "voltage")
            old = scheduler.context("voltage")
            self.assertEqual(self.send(scheduler, "disconnect", "voltage").result(2).phase, "completed")
            self.connect(scheduler, "voltage")
            self.assertNotEqual(old.connection_id, scheduler.context("voltage").connection_id)
            rejected = self.send(scheduler, "action", "voltage", context=old, name="set_channel").result(1)
            self.assertEqual(rejected.phase, "rejected_before_call")
            self.assertEqual(rejected.error["type"], "StaleContext")

    def test_failed_connect_keeps_attempt_identity_until_disconnect(self):
        def execute(req):
            if req.method == "connect":
                raise OSError("retained resource")
            return {"connected": False}
        with self.running(execute) as scheduler:
            initial = scheduler.context("gain")
            outcome = self.send(scheduler, "connect", "gain").result(2)
            self.assertEqual(outcome.phase, "failed_after_call_started")
            self.assertIsNotNone(outcome.context.connection_id)
            self.assertNotEqual(initial, outcome.context)
            self.assertEqual(outcome.context, scheduler.context("gain"))
            self.assertEqual(self.send(scheduler, "connect", "gain").result(1).phase, "rejected_before_call")
            self.assertEqual(self.send(scheduler, "disconnect", "gain").result(2).phase, "completed")
            self.assertIsNone(scheduler.context("gain").connection_id)

    def test_bootstrap_is_cached_and_disconnected_roles_are_not_observed(self):
        calls = []
        with self.running(lambda req: calls.append(req.method) or {},
                          lambda role, ctx: calls.append(role) or Observation({})) as scheduler:
            for method in ("ping", "status"):
                outcome = scheduler.submit(Request(method, method, {}, None)).result(1)
                self.assertEqual(outcome.context, Context("s1", None, 0))
                self.assertEqual(outcome.result["session_id"], "s1")
            self.assertEqual(scheduler.status()["devices"], {})
            self.assertEqual(calls, [])
            self.assertFalse(scheduler.join(0))

    def test_inventory_is_bounded_and_does_not_block_shutdown_admission(self):
        entered, release = threading.Event(), threading.Event()
        def execute(req):
            if req.method == "inventory":
                entered.set()
                self.assertTrue(release.wait(3))
            return {}
        with self.running(execute, releases=[release]) as scheduler:
            active = self.send(scheduler, "inventory")
            self.assertTrue(entered.wait(2))
            pending = self.send(scheduler, "settings_get")
            self.assertEqual(self.send(scheduler, "settings_get").result(1).error["type"], "QueueFull")
            closing = self.send(scheduler, "shutdown")
            self.assertEqual(pending.result(1).phase, "superseded_before_call")
            self.assertFalse(closing.done())
            self.assertEqual(self.send(scheduler, "status").result(1).phase, "completed")
            release.set()
            active.result(2)
            self.assertEqual(closing.result(2).phase, "completed")

    def test_shutdown_invalidates_queued_calls_and_old_result_publication(self):
        entered, release = threading.Event(), threading.Event()
        calls = []
        def execute(req):
            calls.append(req.method)
            if req.method == "action":
                entered.set()
                self.assertTrue(release.wait(3))
                return {"status": {"old": True}}
            return {"connected": True}
        with self.running(execute, releases=[release]) as scheduler:
            self.connect(scheduler, "osa")
            active = self.send(scheduler, "action", "osa", name="acquire")
            self.assertTrue(entered.wait(2))
            pending = self.send(scheduler, "action", "osa", name="acquire")
            closing = self.send(scheduler, "shutdown")
            self.assertEqual(pending.result(1).phase, "superseded_before_call")
            self.assertFalse(scheduler.join(0))
            self.assertEqual(self.send(scheduler, "action", "osa", name="acquire").result(1).phase,
                             "rejected_before_call")
            release.set()
            self.assertEqual(active.result(2).phase, "completed")
            closing.result(2)
            self.assertNotIn("old", scheduler.status()["devices"].get("osa", {}))
            self.assertEqual(calls.count("action"), 1)

    def test_cache_and_outcomes_do_not_alias_callback_results(self):
        result = {"connected": True, "status": {"values": [1]}}
        with self.running(lambda req: result) as scheduler:
            outcome = self.send(scheduler, "connect", "pm400").result(2)
            result["status"]["values"].append(2)
            outcome.result["status"]["values"].append(3)
            snapshot = scheduler.status()
            self.assertEqual(snapshot["devices"]["pm400"]["values"], [1])
            snapshot["devices"]["pm400"]["values"].append(4)
            self.assertEqual(scheduler.status()["devices"]["pm400"]["values"], [1])

    def test_observation_yields_to_normal_between_fields(self):
        entered, release, second = threading.Event(), threading.Event(), threading.Event()
        order = []
        def observe(role, context):
            order.append("read")
            if len(order) == 1:
                entered.set()
                self.assertTrue(release.wait(3))
                return Observation({"field": 1}, more=True)
            second.set()
            return Observation({"field": 2})
        def execute(req):
            if req.method == "action":
                order.append("write")
            return {"connected": True}
        with self.running(execute, observe, [release]) as scheduler:
            self.connect(scheduler, "gain")
            self.assertTrue(entered.wait(2))
            pending = self.send(scheduler, "action", "gain", name="set_current")
            self.assertFalse(pending.done())
            release.set()
            pending.result(2)
            self.assertTrue(second.wait(2))
            self.assertEqual(order[:3], ["read", "write", "read"])

    def test_gain_wait_stable_allows_observation_and_discards_old_revision(self):
        read_entered, read_release = threading.Event(), threading.Event()
        wait_entered, wait_release = threading.Event(), threading.Event()
        read_again, read_again_release = threading.Event(), threading.Event()
        reads = []
        def observe(role, context):
            reads.append(role)
            if len(reads) == 1:
                read_entered.set()
                self.assertTrue(read_release.wait(3))
                return Observation({"temperature": -99})
            read_again.set()
            self.assertTrue(read_again_release.wait(3))
            return Observation({})
        def execute(req):
            if req.method == "action":
                wait_entered.set()
                self.assertTrue(wait_release.wait(3))
                return {"status": {"temperature": 25}}
            return {"connected": True}
        with self.running(execute, observe, [read_release, wait_release, read_again_release]) as scheduler:
            self.connect(scheduler, "gain")
            self.assertTrue(read_entered.wait(2))
            waiting = self.send(scheduler, "action", "gain", name="wait_stable")
            self.assertTrue(wait_entered.wait(2))
            wait_release.set()
            waiting.result(2)
            read_release.set()
            self.assertTrue(read_again.wait(2))
            self.assertEqual(scheduler.status()["devices"]["gain"]["temperature"], 25)

    def test_bad_result_is_readback_failure_and_does_not_kill_worker(self):
        def execute(req):
            if req.method == "inventory":
                return {"bad": object()}
            return {}
        with self.running(execute) as scheduler:
            outcome = self.send(scheduler, "inventory").result(2)
            self.assertEqual(outcome.phase, "completed_readback_failed")
            self.assertIsNotNone(outcome.error)
            encode_v2("bad", outcome)
            self.assertEqual(self.send(scheduler, "settings_get").result(2).phase, "completed")

    def test_duplicate_active_and_recent_ids_do_not_repeat_callback(self):
        entered, release = threading.Event(), threading.Event()
        calls = []
        def execute(req):
            calls.append(req.id)
            if req.method == "inventory":
                entered.set()
                self.assertTrue(release.wait(3))
            return {}
        with self.running(execute, releases=[release]) as scheduler:
            request = Request("same", "inventory", {}, Context("s1", None, 0))
            first = scheduler.submit(request)
            self.assertTrue(entered.wait(2))
            self.assertFalse(first.cancel())
            for _ in range(2):
                self.assertEqual(scheduler.submit(request).result(1).error["type"], "DuplicateRequest")
            release.set()
            first.result(2)
            self.assertEqual(scheduler.submit(request).result(1).error["type"], "DuplicateRequest")
            self.assertEqual(calls, ["same"])

    def test_disconnect_slot_invalidates_pending_and_waits_for_active(self):
        entered, release, callback_read = threading.Event(), threading.Event(), threading.Event()
        calls = []
        def execute(req):
            calls.append(req.method)
            if req.method == "action":
                entered.set()
                self.assertTrue(release.wait(3))
            return {"connected": req.method != "disconnect"}
        with self.running(execute, releases=[release]) as scheduler:
            self.connect(scheduler, "osa")
            old = scheduler.context("osa")
            active = self.send(scheduler, "action", "osa", name="acquire")
            self.assertTrue(entered.wait(2))
            queued = self.send(scheduler, "action", "osa", name="acquire")
            # A terminal callback can read cached state on another thread. This
            # deadlocks if invalidated futures are completed under the Condition.
            readers = []
            def read_after_terminal(_):
                reader = threading.Thread(target=lambda: (scheduler.status(), callback_read.set()))
                readers.append(reader)
                reader.start()
                reader.join(1)
            queued.add_done_callback(read_after_terminal)
            closing = self.send(scheduler, "disconnect", "osa")
            self.assertTrue(callback_read.is_set())
            for reader in readers:
                reader.join(2)
            self.assertEqual(queued.result(1).phase, "superseded_before_call")
            self.assertGreater(scheduler.context("osa").epoch, old.epoch)
            self.assertFalse(closing.done())
            self.assertEqual(self.send(scheduler, "action", "osa", name="acquire").result(1).phase,
                             "rejected_before_call")
            release.set()
            active.result(2)
            self.assertEqual(closing.result(2).phase, "completed")
            self.assertEqual(calls, ["connect", "action", "disconnect"])

    def test_periodic_observation_coalesces_missed_intervals(self):
        now = [0.0]
        entered, release = threading.Event(), threading.Event()
        refreshed, release_refresh = threading.Event(), threading.Event()
        third = threading.Event()
        reads = []
        def observe(role, context):
            reads.append(now[0])
            if len(reads) == 1:
                entered.set()
                self.assertTrue(release.wait(3))
            elif len(reads) == 2:
                refreshed.set()
                self.assertTrue(release_refresh.wait(3))
            else:
                third.set()
            return Observation({})
        scheduler = Scheduler(lambda req: {"connected": req.method != 'disconnect'}, observe,
                              session_id="s1", clock=lambda: now[0])
        try:
            self.connect(scheduler, "gain")
            self.assertTrue(entered.wait(2))
            now[0] = 100.0
            release.set()
            self.assertTrue(refreshed.wait(2))
            release_refresh.set()
            # A normal action is a barrier behind the observation; its refresh
            # is the only new read, with no replay of forty missed timer ticks.
            self.send(scheduler, "action", "gain", name="set_current").result(2)
            self.assertTrue(third.wait(2))
            self.assertEqual(reads, [0.0, 100.0, 100.0])
        finally:
            release.set()
            release_refresh.set()
            self.send(scheduler, "shutdown").result(4)
            self.assertTrue(scheduler.join(4))

    def test_retained_safety_uses_reserved_slot_and_holds_ordinary_control(self):
        for role, name in [("voltage", "zero"), ("gain", "disable_current"), ("gain", "disable_tec")]:
            with self.subTest(role=role, name=name):
                entered, release, early = threading.Event(), threading.Event(), threading.Event()
                calls = []
                def execute(req):
                    if req.method == "action":
                        calls.append(req.params["name"])
                        if req.params["name"] == "ordinary":
                            entered.set()
                            self.assertTrue(release.wait(3))
                        else:
                            early.set()
                    return {"connected": req.method != "disconnect"}
                with self.running(execute, releases=[release]) as scheduler:
                    self.connect(scheduler, role)
                    before = scheduler.context(role)
                    active = self.send(scheduler, "action", role, name="ordinary")
                    self.assertTrue(entered.wait(2))
                    queued = self.send(scheduler, "action", role, name="ordinary")
                    safe = self.send(scheduler, "action", role, name=name)
                    self.assertGreater(scheduler.context(role).epoch, before.epoch)
                    self.assertEqual(queued.result(1).phase, "superseded_before_call")
                    self.assertTrue(early.wait(2))
                    self.assertFalse(safe.done())
                    release.set()
                    active.result(2)
                    self.assertEqual(safe.result(2).phase, "completed")
                    self.assertEqual(calls, ["ordinary", name, name])
                    self.assertEqual(scheduler.status()["roles"][role]["state"], "STOP_HELD")
                    self.assertEqual(self.send(scheduler, "action", role, name="ordinary").result(1).phase,
                                     "rejected_before_call")
                    # Safety from an older epoch is still admitted for this exact connection.
                    self.assertEqual(self.send(scheduler, "action", role, context=before, name=name).result(2).phase,
                                     "completed")

    def test_pending_admission_advances_published_revision(self):
        entered, release = threading.Event(), threading.Event()
        def execute(req):
            if req.method == "action":
                entered.set()
                self.assertTrue(release.wait(3))
            return {"connected": True}
        with self.running(execute, releases=[release]) as scheduler:
            self.connect(scheduler, "osa")
            active = self.send(scheduler, "action", "osa", name="acquire")
            self.assertTrue(entered.wait(2))
            before = scheduler.status()["revision"]
            queued = self.send(scheduler, "action", "osa", name="acquire")
            self.assertGreater(scheduler.status()["revision"], before)
            release.set()
            active.result(2)
            queued.result(2)

    def test_failed_outcome_error_does_not_alias_cached_evidence(self):
        def execute(req):
            if req.method == "connect":
                raise OSError("original evidence")
            return {}
        with self.running(execute) as scheduler:
            outcome = self.send(scheduler, "connect", "osa").result(2)
            outcome.error["message"] = "consumer changed it"
            self.assertEqual(scheduler.status()["devices"]["osa"]["dispatch_error"]["message"],
                             "original evidence")

    def test_invalid_observation_cannot_poison_cached_status(self):
        attempted = threading.Event()
        def observe(role, context):
            attempted.set()
            return Observation({"bad": object()})
        with self.running(observe=observe) as scheduler:
            self.connect(scheduler, "osa")
            self.assertTrue(attempted.wait(2))
            # Ordinary callback executes only after the invalid read has returned.
            self.send(scheduler, "action", "osa", name="acquire").result(2)
            result = self.send(scheduler, "status").result(1)
            self.assertNotIn("bad", result.result["devices"]["osa"])
            encode_v2("cached", result)

    def test_observation_boundary_has_only_one_pending_ordinary_slot(self):
        entered, release = threading.Event(), threading.Event()
        def observe(role, context):
            entered.set()
            self.assertTrue(release.wait(3))
            return Observation({})
        with self.running(observe=observe, releases=[release]) as scheduler:
            self.connect(scheduler, "osa")
            self.assertTrue(entered.wait(2))
            pending = self.send(scheduler, "action", "osa", name="acquire")
            overflow = self.send(scheduler, "action", "osa", name="acquire")
            self.assertTrue(overflow.done())
            self.assertEqual(overflow.result(1).error["type"], "QueueFull")
            release.set()
            self.assertEqual(pending.result(2).phase, "completed")

    def test_shutdown_quiesces_with_pending_retained_safety(self):
        entered, release = threading.Event(), threading.Event()
        def execute(req):
            if req.params.get("name") == "ordinary":
                entered.set()
                self.assertTrue(release.wait(3))
            return {"connected": True}
        with self.running(execute, releases=[release]) as scheduler:
            self.connect(scheduler, "voltage")
            active = self.send(scheduler, "action", "voltage", name="ordinary")
            self.assertTrue(entered.wait(2))
            safe = self.send(scheduler, "action", "voltage", name="zero")
            closing = self.send(scheduler, "shutdown")
            self.assertFalse(safe.done())
            release.set()
            active.result(2)
            self.assertEqual(closing.result(2).phase, "completed")
            self.assertEqual(safe.result(2).result['effective_intent'], 'disconnect')

    def test_pending_write_is_superseded_when_preceding_callback_faults(self):
        entered, release = threading.Event(), threading.Event()
        effects = []
        def execute(req):
            if req.method == "action":
                effects.append(req.params["name"])
                if req.params["name"] == "fault":
                    entered.set()
                    self.assertTrue(release.wait(3))
                    raise OSError("output state unknown")
            return {"connected": req.method != "disconnect"}
        with self.running(execute, releases=[release]) as scheduler:
            self.connect(scheduler, "voltage")
            before = scheduler.context("voltage")
            active = self.send(scheduler, "action", "voltage", name="fault")
            self.assertTrue(entered.wait(2))
            pending = self.send(scheduler, "action", "voltage", name="write")
            release.set()
            self.assertEqual(active.result(2).phase, "failed_after_call_started")
            self.assertEqual(pending.result(2).phase, "superseded_before_call")
            self.assertEqual(effects, ["fault"])
            self.assertEqual(scheduler.context("voltage").connection_id, before.connection_id)
            self.assertEqual(scheduler.status()["roles"]["voltage"]["state"], "FAULT")
            self.assertEqual(scheduler.status()["devices"]["voltage"]["dispatch_error"]["message"],
                             "output state unknown")

    def test_immediate_reply_reserves_id_before_leaving_admission_lock(self):
        entered, release = threading.Event(), threading.Event()
        effects, replies = [], []
        class PausedReplyScheduler(Scheduler):
            def _terminal(inner, work, outcome):
                if work.request.method == "status":
                    entered.set()
                    if not release.wait(3):
                        raise TimeoutError("reply barrier not released")
                super()._terminal(work, outcome)
        scheduler = PausedReplyScheduler(lambda req: effects.append(req.method) or {},
                                         lambda role, ctx: Observation({}), session_id="s1")
        sender = threading.Thread(target=lambda: replies.append(scheduler.submit(
            Request("race", "status", {}, None))))
        try:
            sender.start()
            self.assertTrue(entered.wait(2))
            contender = scheduler.submit(Request("race", "inventory", {}, Context("s1", None, 0)))
            self.assertEqual(contender.result(2).phase, "rejected_before_call")
            self.assertEqual(contender.result().error["type"], "DuplicateRequest")
            self.assertEqual(effects, [])
            release.set()
            sender.join(2)
            self.assertFalse(sender.is_alive())
            self.assertEqual(replies[0].result(2).phase, "completed")
        finally:
            release.set()
            sender.join(3)
            self.send(scheduler, "shutdown").result(4)
            self.assertTrue(scheduler.join(4))

    @staticmethod
    def daemon_test_thread(*args, **kwargs):
        # Only these deliberate fatal-callback regressions use daemon test
        # threads, so an unfixed RED run cannot strand the unittest process.
        return threading.Thread(*args, daemon=True, **kwargs)

    def test_ordinary_baseexception_returns_evidence_and_preserves_worker(self):
        entered, release = threading.Event(), threading.Event()
        def execute(req):
            if req.method == "action":
                entered.set()
                self.assertTrue(release.wait(3))
                raise SystemExit("callback exited")
            return {"connected": req.method != "disconnect"}
        with patch("App.worker.scheduler.Thread", self.daemon_test_thread), \
                self.running(execute, releases=[release]) as scheduler:
            self.connect(scheduler, "voltage")
            active = self.send(scheduler, "action", "voltage", name="write")
            self.assertTrue(entered.wait(2))
            release.set()
            outcome = active.result(2)
            self.assertEqual(outcome.phase, "failed_after_call_started")
            self.assertEqual(outcome.error["type"], "SystemExit")
            self.assertIsNone(scheduler.status()["roles"]["voltage"]["active_request_id"])
            self.assertEqual(self.send(scheduler, "disconnect", "voltage").result(2).phase, "completed")

    def test_observation_baseexception_clears_activity_and_preserves_worker(self):
        entered, release, second = threading.Event(), threading.Event(), threading.Event()
        reads = []
        def observe(role, context):
            reads.append(role)
            if len(reads) == 1:
                entered.set()
                self.assertTrue(release.wait(3))
                raise KeyboardInterrupt("read interrupted")
            second.set()
            return Observation({"recovered_read": True})
        with patch("App.worker.scheduler.Thread", self.daemon_test_thread), \
                self.running(observe=observe, releases=[release]) as scheduler:
            self.connect(scheduler, "gain")
            self.assertTrue(entered.wait(2))
            release.set()
            with scheduler._condition:
                self.assertTrue(scheduler._condition.wait_for(
                    lambda: not scheduler.status()["roles"]["gain"]["observing"], timeout=2))
            error = scheduler.status()["devices"]["gain"]["observation_error"]
            self.assertEqual(error, {"type": "KeyboardInterrupt", "message": "read interrupted"})
            # The normal call cannot enter until the observation clears its slot.
            self.assertEqual(self.send(scheduler, "action", "gain", name="read_cached").result(2).phase,
                             "completed")
            self.assertTrue(second.wait(2))

    def test_shutdown_baseexception_produces_terminal_failure_and_exits(self):
        entered, release = threading.Event(), threading.Event()
        def execute(req):
            if req.method == "shutdown":
                entered.set()
                self.assertTrue(release.wait(3))
                raise SystemExit("cleanup interrupted")
            return {}
        with patch("App.worker.scheduler.Thread", self.daemon_test_thread), \
                self.running(execute, releases=[release]) as scheduler:
            closing = self.send(scheduler, "shutdown")
            self.assertTrue(entered.wait(2))
            release.set()
            outcome = closing.result(2)
            self.assertEqual(outcome.phase, "failed_after_call_started")
            self.assertEqual(outcome.error["type"], "SystemExit")
            self.assertTrue(scheduler.join(2))

    def test_pending_consumer_baseexception_cannot_strand_active_fault(self):
        entered, release, later_callback = threading.Event(), threading.Event(), threading.Event()
        effects = []
        def execute(req):
            effects.append(req.method)
            if req.method == "action":
                entered.set()
                self.assertTrue(release.wait(3))
                raise OSError("original driver failure")
            return {"connected": req.method != "disconnect"}
        def failing_consumer(future):
            raise SystemExit("pending consumer exited")
        with patch("App.worker.scheduler.Thread", self.daemon_test_thread), \
                self.running(execute, releases=[release]) as scheduler:
            self.connect(scheduler, "voltage")
            context = scheduler.context("voltage")
            active = scheduler.submit(Request("active-fault", "action",
                                               {"role": "voltage", "name": "write"}, context))
            self.assertTrue(entered.wait(2))
            pending = scheduler.submit(Request("pending-fault", "action",
                                                {"role": "voltage", "name": "write"}, context))
            pending.add_done_callback(failing_consumer)
            pending.add_done_callback(lambda future: later_callback.set())
            release.set()
            try:
                outcome = active.result(2)
            except FutureTimeoutError:
                self.fail("consumer callback stranded the active driver's terminal evidence")
            self.assertEqual(outcome.phase, "failed_after_call_started")
            self.assertEqual(outcome.error, {"type": "OSError", "message": "original driver failure"})
            self.assertEqual(pending.result(2).phase, "superseded_before_call")
            self.assertTrue(later_callback.wait(2))
            with scheduler._condition:
                self.assertNotIn("active-fault", scheduler._active_ids)
                self.assertNotIn("pending-fault", scheduler._active_ids)
            self.assertEqual(scheduler.status()["consumer_callback_errors"], [
                {"request_id": "pending-fault",
                 "error": {"type": "SystemExit", "message": "pending consumer exited"}}])
            # A new permitted cleanup must still execute on the same fixed role worker.
            self.assertEqual(self.send(scheduler, "disconnect", "voltage").result(2).phase, "completed")
            self.assertEqual(effects, ["connect", "action", "disconnect"])

    def test_completed_reply_callback_diagnostics_are_bounded_and_copied(self):
        with self.running() as scheduler:
            reply = self.send(scheduler, "ping")
            outcome = reply.result(2)
            for index in range(257):
                def failing_consumer(future, label=f"failure-{index}"):
                    raise SystemExit(label)
                try:
                    reply.add_done_callback(failing_consumer)
                except BaseException as error:
                    self.fail(f"completed reply leaked consumer {type(error).__name__}")
            diagnostics = scheduler.status()["consumer_callback_errors"]
            self.assertEqual(len(diagnostics), 256)
            self.assertEqual(diagnostics[0]["error"]["message"], "failure-1")
            self.assertEqual(diagnostics[-1]["error"]["message"], "failure-256")
            diagnostics[-1]["error"]["message"] = "changed outside scheduler"
            self.assertEqual(scheduler.status()["consumer_callback_errors"][-1]["error"]["message"],
                             "failure-256")
            self.assertIs(reply.result(), outcome)
            self.assertEqual(outcome.phase, "completed")
            self.assertEqual(outcome.result["consumer_callback_errors"], [])

    def test_callback_exception_with_broken_message_keeps_later_callbacks(self):
        class BrokenMessage(SystemExit):
            def __str__(self):
                raise KeyboardInterrupt("error formatting failed")
        with self.running() as scheduler:
            reply = self.send(scheduler, "ping")
            called = threading.Event()
            def failing_consumer(future):
                raise BrokenMessage()
            try:
                reply.add_done_callback(failing_consumer)
                reply.add_done_callback(lambda future: called.set())
            except BaseException:
                self.fail("callback diagnostic formatting escaped its boundary")
            self.assertTrue(called.wait(2))
            self.assertEqual(scheduler.status()["consumer_callback_errors"][-1]["error"],
                             {"type": "BrokenMessage", "message": "exception message unavailable"})


if __name__ == "__main__":
    unittest.main()
