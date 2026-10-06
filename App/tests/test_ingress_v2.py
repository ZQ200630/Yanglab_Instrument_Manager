"""Offline tests for bounded publication and asynchronous ingress."""
import io
import json
import os
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from App.worker.controller import ConsoleController
from App.worker.main import run, _Replies
from App.worker.contracts import Context, Outcome, Request
from concurrent.futures import Future



class IngressTests(unittest.TestCase):
    def test_query_handoff_waits_only_for_current_publication_without_extra_liability(self):
        flushing, release, waiting = (threading.Event() for _ in range(3))
        class Output:
            def write(self, line):
                pass
            def flush(self):
                flushing.set()
                if not release.wait(2):
                    raise TimeoutError('Offline flush gate')
        class Retirement:
            def __init__(self):
                self.event = threading.Event()
                self.timeouts = []
            def clear(self):
                self.event.clear()
            def set(self):
                self.event.set()
            def wait(self, timeout):
                self.timeouts.append(timeout)
                waiting.set()
                return self.event.wait(timeout)
        replies = _Replies(Output())
        retirement = Retirement()
        replies.query_retired = retirement
        context = Context('s', None, 0)
        first = Request('first', 'ping', {}, context)
        second = Request('second', 'status', {}, context)
        result = []
        caller = threading.Thread(target=lambda: result.append(replies.reserve(second)))
        try:
            self.assertIsNone(replies.reserve(first))
            done = Future()
            done.set_result(Outcome('completed', context, {}))
            replies.publish(first, done)
            self.assertTrue(flushing.wait(1))
            caller.start()
            self.assertTrue(waiting.wait(1), 'The publishing query must get a bounded retirement handoff')
            # Safety admission never acquires or waits on the stdout operation.
            safe = Request('stop', 'shutdown', {}, context)
            self.assertIsNone(replies.reserve(safe))
            release.set()
            caller.join(2)
            self.assertEqual(result, [None])
            self.assertLessEqual(max(retirement.timeouts), .005)
            self.assertNotIn('first', replies.active)
            self.assertEqual(sum(group == 'query' for group, _ in replies.active.values()), 1)
        finally:
            release.set()
            if caller.ident is not None:
                caller.join(2)
            replies.closed.set()
            replies.thread.join(2)

    def test_fault_preserves_diagnostic_and_shutdown_admission(self):
        replies = _Replies(io.StringIO())
        try:
            replies.fault.set()
            self.assertIsNotNone(replies.reserve(Request("work", "inventory", {}, None)))
            self.assertIsNone(replies.reserve(Request("status", "status", {}, None)))
            self.assertIsNone(replies.reserve(Request("retry", "shutdown", {}, Context("s", None, 0))))
            self.assertIsNotNone(replies.reserve(Request("status2", "status", {}, None)))
            self.assertIsNotNone(replies.reserve(Request("retry2", "shutdown", {}, Context("s", None, 0))))
        finally:
            replies.closed.set()
            replies.thread.join(2)

    def test_query_publication_grace_does_not_retire_a_blocked_flush(self):
        entered, release = threading.Event(), threading.Event()
        class Output:
            def write(self, line):
                pass
            def flush(self):
                entered.set()
                release.wait(2)
        replies = _Replies(Output())
        first = Request('first', 'status', {}, None)
        try:
            self.assertIsNone(replies.reserve(first))
            done = Future()
            done.set_result(Outcome('completed', Context('s', None, 0), {}))
            replies.publish(first, done)
            self.assertTrue(entered.wait(1))
            self.assertIsNotNone(replies.reserve(Request('next', 'ping', {}, None)))
            self.assertIn('first', replies.active)
            self.assertNotIn('next', replies.active)
            self.assertIsNone(replies.reserve(Request('shutdown', 'shutdown', {}, Context('s', None, 0))))
        finally:
            release.set()
            replies.closed.set()
            replies.thread.join(2)

    def test_rejected_id_is_reserved_during_write_and_remembered_after_flush(self):
        ordinary_entered, ordinary_release = threading.Event(), threading.Event()
        rejected_entered, rejected_release = threading.Event(), threading.Event()
        frames = []
        class Output:
            def write(self, line):
                frame = json.loads(line)
                entered, release = ((rejected_entered, rejected_release) if frame["id"] == "overflow"
                                    else (ordinary_entered, ordinary_release))
                entered.set()
                if not release.wait(3):
                    raise TimeoutError("test writer barrier")
                frames.append(frame)
            def flush(self):
                pass
        replies = _Replies(Output())
        context = Context("session", None, 0)
        request = Request("overflow", "inventory", {}, context)
        try:
            for index in range(31):
                accepted = Request(str(index), "inventory", {}, context)
                self.assertIsNone(replies.reserve(accepted))
                done = Future()
                done.set_result(Outcome("completed", context, {}))
                replies.publish(accepted, done)
            self.assertTrue(ordinary_entered.wait(1))
            self.assertIsNotNone(replies.reserve(request))
            replies.reject(request.id, context, "capacity full")
            ordinary_release.set()
            self.assertTrue(rejected_entered.wait(2))
            self.assertEqual(len(replies.active), 0, "ordinary capacity is now free")
            self.assertEqual(replies.reserve(request), "duplicate request ID")
            rejected_release.set()
            replies.closed.set()
            replies.thread.join(2)
            self.assertEqual(replies.reserve(request), "duplicate request ID")
            self.assertEqual(sum(frame["id"] == "overflow" for frame in frames), 1)
            self.assertEqual(frames[-1]["phase"], "rejected_before_call")
        finally:
            ordinary_release.set()
            rejected_release.set()
            replies.closed.set()
            replies.thread.join(3)

    def test_settings_io_cannot_block_status(self):
        entered, release = threading.Event(), threading.Event()
        def slow_settings(_path):
            entered.set()
            release.wait(3)
            return {"version": 1}
        with tempfile.TemporaryDirectory() as directory:
            controller = ConsoleController(port_enumerator=lambda: ())
            read_fd, write_fd = os.pipe()
            output_read, output_write = os.pipe()
            source = os.fdopen(read_fd, "r", encoding="utf-8")
            sink = os.fdopen(output_write, "w", encoding="utf-8", buffering=1)
            incoming = os.fdopen(output_read, "r", encoding="utf-8")
            received = {}
            condition = threading.Condition()
            def collect():
                for line in incoming:
                    frame = json.loads(line)
                    with condition:
                        received[frame["id"]] = frame
                        condition.notify_all()
            reader = threading.Thread(target=collect)
            reader.start()
            def send(id, method, params={}, context=None):
                os.write(write_fd, (json.dumps(dict(v=2, id=id, method=method,
                    params=params, context=context)) + "\n").encode())
            def wait(id):
                with condition:
                    self.assertTrue(condition.wait_for(lambda: id in received, 2), id)
                    return received[id]
            with patch("App.worker.main.load_settings", slow_settings):
                server = threading.Thread(target=run, kwargs=dict(controller=controller,
                    input_stream=source, output_stream=sink, settings_path=Path(directory)/"s.json"))
                server.start()
                try:
                    send("ping", "ping")
                    ping = wait("ping")
                    self.assertTrue(ping["ok"], ping)
                    self.assertEqual(ping["result"]["protocol_version"], 2)
                    context = ping["context"]
                    send("settings", "settings_get", context=context)
                    self.assertTrue(entered.wait(2))
                    send("status", "status")
                    status = wait("status")
                    self.assertEqual(status["result"]["mode"], "real")
                    self.assertIn("observed_at", status["result"])
                    self.assertIn("role_cleanup_attempts", status["result"])
                    self.assertNotIn("settings", received)
                    release.set()
                    self.assertEqual(wait("settings")["result"]["version"], 1)
                    send("shutdown", "shutdown", context=context)
                    self.assertEqual(wait("shutdown")["result"]["unreleased"], [])
                    server.join(2)
                    self.assertFalse(server.is_alive(), "stdin is still open")
                finally:
                    release.set()
                    os.close(write_fd)
                    server.join(3)
                    sink.close()
                    reader.join(3)
                    incoming.close()
                    source.close()
                    controller.close()

    def test_terminal_capacity_includes_current_writer_and_finite_safety_upgrades(self):
        entered, release = threading.Event(), threading.Event()
        lines = []
        class BlockedOutput:
            def write(self, line):
                entered.set()
                if not release.wait(3):
                    raise TimeoutError("test did not unblock writer")
                lines.append(json.loads(line))
            def flush(self):
                pass
        replies = _Replies(BlockedOutput())
        context = Context("session", "connection", 0)
        accepted = []
        def submit(id, method, params):
            request = Request(id, method, params, context)
            self.assertIsNone(replies.reserve(request))
            accepted.append(request)
            done = Future()
            done.set_result(Outcome("completed", context, {}))
            replies.publish(request, done)
        try:
            for index in range(31):
                submit(str(index), "action", {"role": "osa", "name": "acquire"})
            self.assertTrue(entered.wait(1))
            self.assertIsNotNone(replies.reserve(Request("overflow", "inventory", {}, context)))
            submit("status", "status", {})
            self.assertIsNotNone(replies.reserve(Request("second-query", "status", {}, context)))
            for role, names in (("gain", ["disable_current", "disable_tec"]),
                                ("voltage", ["zero"]), ("osa", []), ("pm400", []), ("fiber", [])):
                for name in names:
                    submit(role + name, "action", {"role": role, "name": name})
                submit(role + "disconnect", "disconnect", {"role": role})
            submit("shutdown", "shutdown", {})
            self.assertEqual(len(replies.active), 41)
            replies.reject("invalid", None, "bad frame")
            for _ in range(100):
                replies.reject("invalid", None, "bad frame")
            self.assertTrue(replies.fault.is_set())
            self.assertEqual(len(replies.active), 41)
            self.assertEqual(replies.queue.qsize(), 41)  # 40 terminals + one error; current write is held.
        finally:
            release.set()
            replies.closed.set()
            replies.thread.join(3)
        self.assertFalse(replies.thread.is_alive())
        self.assertEqual(len(lines), 42)
        self.assertEqual(len({line["id"] for line in lines}), 42)

    def test_handled_ingress_exception_still_uses_coordinated_cleanup(self):
        from App.tests.test_worker_controller import FakeVoltage
        controller = ConsoleController(factories={"voltage": FakeVoltage}, port_enumerator=lambda: ())
        try:
            controller.handle("connect", {"role": "voltage", "resource": "COM990",
                                         "acknowledge_lifecycle": True})
            incoming = io.StringIO(json.dumps(dict(v=2, id="status", method="status",
                                                   params={}, context=None)) + "\n")
            with tempfile.TemporaryDirectory() as directory, patch(
                    "App.worker.main._Replies.reserve", side_effect=RuntimeError("ingress fault")), patch(
                    "sys.stderr", io.StringIO()) as errors:
                code = run(controller, input_stream=incoming, output_stream=io.StringIO(),
                           settings_path=Path(directory) / "settings.json")
            self.assertEqual(code, 2)
            self.assertIn("ingress fault", errors.getvalue())
            self.assertEqual(controller.cached_status()["last_cleanup"]["unreleased"], [])
            self.assertTrue(controller._scheduler.join(1))
        finally:
            controller.close()

    def test_unexpected_queue_failure_keeps_reserved_terminal_evidence(self):
        replies = _Replies(io.StringIO())
        request = Request("queue-fault", "status", {}, None)
        self.assertIsNone(replies.reserve(request))
        outcome = Outcome("completed", Context("session", None, 0), {"devices": {}})
        done = Future()
        done.set_result(outcome)
        try:
            with patch.object(replies.queue, "put_nowait", side_effect=RuntimeError("publication failed")):
                replies.publish(request, done)
            self.assertTrue(replies.fault.is_set())
            self.assertEqual(replies.failed_publications["queue-fault"], outcome)
            self.assertIn("queue-fault", replies.active)
        finally:
            replies.closed.set()
            replies.thread.join(2)

    def test_failed_publication_is_written_without_stranding_query_reservation(self):
        written, release = threading.Event(), threading.Event()
        class Output(io.StringIO):
            def flush(self):
                written.set()
                if not release.wait(2):
                    raise TimeoutError("test did not release flush")
        replies = _Replies(Output())
        request = Request("query-publication", "status", {}, None)
        self.assertIsNone(replies.reserve(request))
        done = Future()
        done.set_result(Outcome("completed", Context("s", None, 0), {"devices": {}}))
        try:
            with patch.object(replies.queue, "put_nowait", side_effect=RuntimeError("publication failed")):
                replies.publish(request, done)
            self.assertTrue(written.wait(1), "writer must deliver retained terminal on healthy output")
            self.assertIn(request.id, replies.active, "reservation survives through blocked flush")
            self.assertIn(request.id, replies.failed_publications)
            release.set()
            replies.closed.set()
            replies.thread.join(2)
            self.assertEqual(json.loads(replies.output.getvalue())["id"], request.id)
            self.assertIsNone(replies.reserve(Request("diagnostic", "status", {}, None)))
            self.assertEqual(replies.reserve(request), "duplicate request ID")
            self.assertEqual(replies.failed_publications, {})
        finally:
            release.set()
            replies.closed.set()
            replies.thread.join(2)

    def test_anonymous_rejection_cannot_claim_an_active_or_recent_request_id(self):
        for stage in ("active", "recent"):
            with self.subTest(stage=stage):
                replies = _Replies(io.StringIO())
                request = Request("invalid", "status", {}, None)
                self.assertIsNone(replies.reserve(request))
                done = Future()
                done.set_result(Outcome("completed", Context("s", None, 0), {"devices": {}}))
                try:
                    if stage == "active":
                        replies.reject(None, None, "malformed frame")
                    replies.publish(request, done)
                    replies.closed.set()
                    replies.thread.join(2)
                    if stage == "recent":
                        replies.reject(None, None, "malformed frame")
                    frames = [json.loads(line) for line in replies.output.getvalue().splitlines()]
                    self.assertEqual([(frame["id"], frame["phase"]) for frame in frames],
                                     [("invalid", "completed")])
                    self.assertTrue(replies.fault.is_set(), "collision signals fault without a false terminal")
                    self.assertTrue(replies.queue.empty(), "no conflicting terminal remains queued")
                finally:
                    replies.closed.set()
                    replies.thread.join(2)

    def test_anonymous_wire_id_is_protected_pending_current_and_after_flush(self):
        for stage in ("pending", "current", "flushed"):
            with self.subTest(stage=stage):
                entered, release = threading.Event(), threading.Event()
                frames = []
                class Output:
                    def write(self, line):
                        frame = json.loads(line)
                        if stage != "flushed" and (stage == "current" or frame["id"] == "other"):
                            entered.set()
                            if not release.wait(2):
                                raise TimeoutError("test writer barrier")
                        frames.append(frame)
                    def flush(self):
                        pass
                replies = _Replies(Output())
                try:
                    if stage == "pending":
                        other = Request("other", "inventory", {}, Context("s", None, 0))
                        self.assertIsNone(replies.reserve(other))
                        done = Future()
                        done.set_result(Outcome("completed", other.context, {}))
                        replies.publish(other, done)
                        self.assertTrue(entered.wait(1))
                    replies.reject(None, None, "malformed frame")
                    if stage == "current":
                        self.assertTrue(entered.wait(1))
                    elif stage == "flushed":
                        replies.closed.set()
                        replies.thread.join(2)
                    request = Request("invalid", "status", {}, None)
                    self.assertEqual(replies.reserve(request), "duplicate request ID")
                    replies.reject(request.id, None, "duplicate request ID")
                    release.set()
                    replies.closed.set()
                    replies.thread.join(2)
                    terminals = [frame for frame in frames if frame["id"] == "invalid"]
                    self.assertEqual(len(terminals), 1)
                    self.assertEqual(terminals[0]["phase"], "rejected_before_call")
                    self.assertEqual(terminals[0]["error"]["message"], "malformed frame")
                    self.assertNotIn("invalid", replies.active)
                    self.assertIn("invalid", replies.recent)
                finally:
                    release.set()
                    replies.closed.set()
                    replies.thread.join(2)

    def test_callback_baseexception_retains_one_terminal_and_requests_cleanup(self):
        replies = _Replies(io.StringIO())
        request = Request("callback-fault", "status", {}, None)
        self.assertIsNone(replies.reserve(request))
        done = Future()
        done.set_exception(SystemExit("consumer stopped"))
        replies.publish(request, done)
        replies.closed.set()
        replies.thread.join(2)
        self.assertTrue(replies.fault.is_set())
        frame = json.loads(replies.output.getvalue())
        self.assertEqual(frame["id"], "callback-fault")
        self.assertEqual(frame["phase"], "completed_readback_failed")
        self.assertEqual(frame["error"]["type"], "SystemExit")


if __name__ == "__main__":
    unittest.main()
