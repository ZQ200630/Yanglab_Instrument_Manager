"""Offline tests of ownership, safety ordering and cached session health."""

from __future__ import annotations

import itertools
import threading
import unittest
from dataclasses import FrozenInstanceError
from types import SimpleNamespace

from Code.Debugs.test_fiber_coupling import FakeSetupMDT, make_mdt_status
from Code.Setups.session import CleanupReport, CleanupStep, DeviceHealth, InstrumentSession, safe_shutdown
from Code.Setups.fiber_coupling import (
    FiberCouplingSetup, FiberDiscovery, StageSide, load_fiber_coupling_config,
)
from Code.Utils.common import DeviceFault, DriverState, InstrumentSafetyError
from Code.Utils.gain import GainStatus
from Code.Utils.mdt693b import AXIS_BASELINE_ATTESTATION_EVIDENCE
from Code.Utils.voltage import VoltageStatus, ZeroEvidence, ZeroState


class Device:
    """Only public driver calls; transport effects are recorded at this boundary."""

    def __init__(self, role, events, failures=None):
        self.role, self.events = role, events
        self.failures = failures or {}
        self.state = DriverState.READY

    def call(self, action):
        self.events.append(f"{self.role}.{action}")
        if action in self.failures:
            raise self.failures[action]

    def connect(self):
        self.call("connect")
        return self

    def close(self):
        self.call("close")
        self.state = DriverState.DISCONNECTED


class Voltage(Device):
    def __init__(self, events, failures=None):
        super().__init__("voltage", events, failures)
        self.resources_released = False
        self.zero_evidence = ZeroEvidence(ZeroState.MEASURED_ZERO, 1., 2., (0.,) * 8)

    def zero(self, *, emergency=False):
        if emergency is not True:
            raise AssertionError("Shutdown must use immediate emergency zero")
        self.call("zero")

    def close(self):
        super().close()
        self.resources_released = True

    def read_status(self):
        return VoltageStatus((0.,) * 8, (0.,) * 8, 12.)


class Gain(Device):
    def __init__(self, events, failures=None):
        super().__init__("gain", events, failures)
        self.status = GainStatus(25., 25., True, 0., False, 13.)

    def disable_current(self):
        self.call("current_off")
        return False

    def disable_tec(self):
        self.call("tec_off")
        return False


class Fiber(Device):
    def __init__(self, events, failures=None):
        super().__init__("fiber", events, failures)

    def connect(self):
        raise AssertionError("A delivered Fiber setup must never be reconnected")

    def ramp_to_zero(self):
        raise AssertionError("Piezo must hold")

    def emergency_zero(self, **kwargs):
        raise AssertionError("Piezo must hold")


class SessionTests(unittest.TestCase):
    def bundle(self, events, failure=None):
        devices = dict(osa=Device("osa", events), voltage=Voltage(events),
                       gain=Gain(events), pm400=Device("pm400", events), fiber=Fiber(events))
        if failure:
            role, action, error = failure
            devices[role].failures[action] = error
        return devices

    def test_construction_is_inert_connect_order_and_no_fiber_reconnect(self):
        events = []
        session = InstrumentSession(**self.bundle(events))
        self.assertEqual(events, [])
        self.assertIs(session.connect(), session)
        self.assertEqual(events, ["osa.connect", "voltage.connect", "gain.connect", "pm400.connect"])
        session.connect()
        self.assertEqual(len(events), 4)
        session.close()

    def test_all_optional_combinations_have_fixed_cleanup_order(self):
        for present in itertools.product((False, True), repeat=5):
            with self.subTest(present=present):
                events = []
                bundle = self.bundle(events)
                chosen = {role: device for (role, device), enabled in zip(bundle.items(), present) if enabled}
                report = InstrumentSession(**chosen).close()
                expected = [item for item in (
                    "voltage.zero", "gain.current_off", "gain.tec_off", "gain.close",
                    "voltage.close", "osa.close", "pm400.close", "fiber.close",
                ) if item.split(".")[0] in chosen]
                self.assertEqual(events, expected)
                self.assertTrue(report.ok)
                self.assertEqual(report.unreleased, ())
                self.assertEqual([(step.role, step.action) for step in report.steps],
                                 [tuple(item.split(".")) for item in expected])

    def test_each_cleanup_failure_preserves_first_error_and_runs_remaining_actions(self):
        expected = ["voltage.zero", "gain.current_off", "gain.tec_off", "gain.close",
                    "voltage.close", "osa.close", "pm400.close", "fiber.close"]
        for failure in expected:
            with self.subTest(failure=failure):
                events = []
                error = OSError(failure)
                role, action = failure.split(".")
                devices = self.bundle(events, (role, action, error))
                with self.assertRaises(OSError) as caught:
                    InstrumentSession(**devices).close()
                self.assertIs(caught.exception, error)
                self.assertEqual(events, expected)
                report = error.cleanup_report
                self.assertFalse(report.ok)
                self.assertEqual([s.error for s in report.steps if s.error], [error])
                self.assertEqual(report.unreleased, ((role, devices[role]),) if action == "close" else ())

    def test_multiple_base_exceptions_and_broken_logging_do_not_interrupt_cleanup(self):
        events = []
        devices = self.bundle(events)
        first, second = KeyboardInterrupt(), SystemExit(9)
        devices["gain"].failures.update(current_off=first, tec_off=second)

        class Log:
            def error(self, *args):
                events.append("log.error")
                raise RuntimeError("logger failed")

        with self.assertRaises(KeyboardInterrupt) as caught:
            InstrumentSession(**devices, log=Log()).close()
        self.assertIs(caught.exception, first)
        self.assertEqual([s.error for s in first.cleanup_report.steps if s.error], [first, second])
        self.assertLess(events.index("fiber.close"), events.index("log.error"))

    def test_close_retries_only_unreleased_roles_and_old_report_is_immutable(self):
        events = []
        error = OSError("held gain")
        gain = Gain(events, {"close": error})
        session = InstrumentSession(gain=gain, fiber=Fiber(events))
        with self.assertRaises(OSError):
            session.close()
        old = error.cleanup_report
        with self.assertRaises(FrozenInstanceError):
            old.steps = ()
        with self.assertRaises(FrozenInstanceError):
            old.steps[0].action = "changed"
        gain.failures.clear()
        events.clear()
        report = session.close()
        self.assertEqual(events, ["gain.current_off", "gain.tec_off", "gain.close"])
        self.assertTrue(report.ok)
        self.assertEqual(old.unreleased, (("gain", gain),))
        self.assertFalse(old.ok)
        self.assertIs(session.close(), report)
        self.assertEqual(len(events), 3)
        with self.assertRaises(InstrumentSafetyError):
            session.connect()

    def test_unknown_voltage_can_be_released_without_claiming_safe_output(self):
        events = []
        voltage = Voltage(events)
        error = DeviceFault("unconfirmed zero")
        voltage.zero_evidence = ZeroEvidence(ZeroState.UNKNOWN, None, None, None)

        def close():
            events.append("voltage.close")
            voltage.resources_released = True
            voltage.state = DriverState.FAULT
            raise error

        voltage.close = close
        session = InstrumentSession(voltage=voltage)
        with self.assertRaises(DeviceFault):
            session.close()
        report = error.cleanup_report
        self.assertEqual(report.unreleased, ())
        self.assertEqual(report.voltage_zero.state, ZeroState.UNKNOWN)
        self.assertFalse(report.ok)
        events.clear()
        self.assertIs(session.close(), report)
        self.assertEqual(events, [])

    def test_close_returning_with_live_resource_is_retained(self):
        events = []
        device = Device("osa", events)
        device.close = lambda: events.append("osa.close")
        with self.assertRaises(InstrumentSafetyError) as caught:
            InstrumentSession(osa=device).close()
        self.assertEqual(caught.exception.cleanup_report.unreleased, (("osa", device),))

    def test_unknown_or_command_only_voltage_evidence_is_unsuccessful_after_release(self):
        for state in (ZeroState.UNKNOWN, ZeroState.COMMAND_SENT):
            with self.subTest(state=state):
                voltage = Voltage([])
                voltage.zero_evidence = ZeroEvidence(state, 1., None, None)
                with self.assertRaises(InstrumentSafetyError) as caught:
                    InstrumentSession(voltage=voltage).close()
                report = caught.exception.cleanup_report
                self.assertEqual(report.unreleased, ())
                self.assertIs(report.voltage_zero, voltage.zero_evidence)
                self.assertFalse(report.ok)

    def test_legacy_missing_evidence_never_manufactures_measured_zero(self):
        voltage = Voltage([])
        del voltage.zero_evidence
        report = InstrumentSession(voltage=voltage).close()
        self.assertIsNone(report.voltage_zero)

    def test_public_only_piezo_and_gain_failure_use_no_extra_methods(self):
        events = []
        primary = OSError("confirmation failed")

        class Piezo:
            def close(self):
                events.append("fiber.close")

        class PublicGain:
            def disable_current(self):
                events.append("gain.current_off")
                raise primary

            def disable_tec(self):
                events.append("gain.tec_off")

            def close(self):
                events.append("gain.close")

        with self.assertRaises(OSError) as caught:
            InstrumentSession(gain=PublicGain(), fiber=Piezo()).close()
        self.assertIs(caught.exception, primary)
        self.assertEqual(events, ["gain.current_off", "gain.tec_off", "gain.close", "fiber.close"])

    def test_zero_evidence_descriptor_failure_does_not_abort_other_cleanup(self):
        events = []

        class BrokenEvidence(Voltage):
            @property
            def zero_evidence(self):
                raise OSError("evidence unavailable")

            @zero_evidence.setter
            def zero_evidence(self, value):
                pass

        with self.assertRaises(OSError) as caught:
            InstrumentSession(voltage=BrokenEvidence(events), fiber=Fiber(events)).close()
        self.assertEqual(events, ["voltage.zero", "voltage.close", "fiber.close"])
        self.assertEqual(caught.exception.cleanup_report.unreleased, ())

    def test_connection_failures_cleanup_unattempted_delivered_devices(self):
        for failed_role in ("osa", "voltage", "gain", "pm400"):
            with self.subTest(failed_role=failed_role):
                events = []
                primary = OSError("connect failed")
                devices = self.bundle(events, (failed_role, "connect", primary))
                devices["fiber"].failures["close"] = RuntimeError("retained")
                with self.assertRaises(OSError) as caught:
                    InstrumentSession(**devices).connect()
                self.assertIs(caught.exception, primary)
                self.assertEqual(events[-8:], ["voltage.zero", "gain.current_off", "gain.tec_off",
                    "gain.close", "voltage.close", "osa.close", "pm400.close", "fiber.close"])
                self.assertEqual(primary.cleanup_report.unreleased, (("fiber", devices["fiber"]),))
                self.assertEqual(events[:events.index("voltage.zero")],
                                 [f"{r}.connect" for r in ("osa", "voltage", "gain", "pm400")][:("osa", "voltage", "gain", "pm400").index(failed_role) + 1])

    def test_context_preserves_operation_exception_even_if_cleanup_succeeds_or_fails(self):
        for primary in (ValueError("operation"), KeyboardInterrupt(), SystemExit(7)):
            for cleanup_fails in (False, True):
                with self.subTest(primary=primary, cleanup_fails=cleanup_fails):
                    events = []
                    gain = Gain(events, {"close": OSError("cleanup")} if cleanup_fails else None)
                    with self.assertRaises(type(primary)) as caught:
                        with InstrumentSession(gain=gain):
                            raise primary
                    self.assertIs(caught.exception, primary)
                    self.assertEqual(primary.cleanup_report.ok, not cleanup_fails)

    def test_context_connection_keyboardinterrupt_preserves_primary_and_report(self):
        events = []
        primary = KeyboardInterrupt()
        devices = self.bundle(events, ("voltage", "connect", primary))
        devices["gain"].failures["close"] = OSError("retained")
        with self.assertRaises(KeyboardInterrupt) as caught:
            with InstrumentSession(**devices):
                self.fail("body cannot run after failed connection")
        self.assertIs(caught.exception, primary)
        self.assertEqual(primary.cleanup_report.unreleased, (("gain", devices["gain"]),))

    def test_normal_context_surfaces_cleanup_error(self):
        error = OSError("close failed")
        with self.assertRaises(OSError) as caught:
            with InstrumentSession(osa=Device("osa", [], {"close": error})):
                pass
        self.assertIs(caught.exception, error)
        self.assertFalse(error.cleanup_report.ok)

    def test_compatibility_shutdown_returns_none_and_uses_same_order(self):
        events = []
        self.assertIsNone(safe_shutdown(Voltage(events), Gain(events), Device("osa", events), None,
                                        fiber=Fiber(events)))
        self.assertEqual(events, ["voltage.zero", "gain.current_off", "gain.tec_off", "gain.close",
                                  "voltage.close", "osa.close", "fiber.close"])

    def test_health_only_reads_cached_metadata(self):
        events = []
        devices = self.bundle(events)
        devices["gain"].state = DriverState.ACTIVE

        class ForbiddenStatus:
            def __getattribute__(self, name):
                raise AssertionError("PM400 status command group must not be inspected")

        devices["pm400"].status = ForbiddenStatus()
        snapshots = InstrumentSession(**devices).check_health()
        self.assertEqual(events, [])
        self.assertEqual([s.role for s in snapshots], ["osa", "voltage", "gain", "pm400", "fiber"])
        self.assertEqual([s.observed_at for s in snapshots], [None, 12., 13., None, None])
        self.assertTrue(all(s.cached for s in snapshots))
        self.assertEqual(snapshots[2].state, DriverState.ACTIVE)

    def test_fault_disconnected_closing_and_unknown_states_are_not_healthy(self):
        for state in (DriverState.FAULT, DriverState.DISCONNECTED, DriverState.CLOSING,
                      DriverState.CONNECTING, None, "READY"):
            with self.subTest(state=state):
                device = Device("osa", [])
                device.state = state
                with self.assertRaises((DeviceFault, InstrumentSafetyError)) as caught:
                    InstrumentSession(osa=device).check_health()
                snapshot = caught.exception.device_health[0]
                self.assertEqual(snapshot.state, state if isinstance(state, DriverState) else None)
        with self.assertRaises(InstrumentSafetyError):
            InstrumentSession(fiber=SimpleNamespace(close=lambda: None)).check_health()

    def test_request_stop_is_inert_and_prevents_connect_and_health(self):
        events = []
        session = InstrumentSession(**self.bundle(events))
        session.request_stop()
        session.request_stop()
        self.assertEqual(events, [])
        with self.assertRaises(InstrumentSafetyError):
            session.check_health()
        with self.assertRaises(InstrumentSafetyError):
            session.connect()
        self.assertEqual(events, [])
        session.close()

    def test_stop_returns_while_close_is_blocked_and_concurrent_close_does_not_duplicate(self):
        entered, release, stopped = threading.Event(), threading.Event(), threading.Event()
        events, results, failures = [], [], []
        gain = Gain(events)
        original = gain.disable_current

        def blocked():
            entered.set()
            if not release.wait(3):
                raise AssertionError("test gate not released")
            original()

        gain.disable_current = blocked
        session = InstrumentSession(gain=gain)

        def closing():
            try:
                results.append(session.close())
            except BaseException as error:
                failures.append(error)

        def stopping():
            session.request_stop()
            stopped.set()

        threads = [threading.Thread(target=closing), threading.Thread(target=closing),
                   threading.Thread(target=stopping)]
        try:
            threads[0].start()
            self.assertTrue(entered.wait(1))
            threads[1].start()
            threads[2].start()
            self.assertTrue(stopped.wait(1))
        finally:
            release.set()
            for thread in threads:
                if thread.ident is not None:
                    thread.join(4)
        self.assertFalse(any(t.is_alive() for t in threads))
        self.assertEqual(failures, [])
        self.assertEqual(events, ["gain.current_off", "gain.tec_off", "gain.close"])
        self.assertEqual(len(results), 2)
        self.assertIs(results[0], results[1])

    def test_close_during_connect_stops_later_connections_and_releases_delivered_devices(self):
        entered, release, stopped = threading.Event(), threading.Event(), threading.Event()
        events, errors = [], []
        osa = Device("osa", events)

        def blocked_connect():
            events.append("osa.connect")
            entered.set()
            if not release.wait(3):
                raise AssertionError("connect gate not released")

        osa.connect = blocked_connect
        session = InstrumentSession(osa=osa, pm400=Device("pm400", events))

        def connecting():
            try:
                session.connect()
            except BaseException as error:
                errors.append(error)

        def closing():
            session.request_stop()
            stopped.set()
            session.close()

        threads = [threading.Thread(target=connecting), threading.Thread(target=closing)]
        try:
            threads[0].start()
            self.assertTrue(entered.wait(1))
            threads[1].start()
            self.assertTrue(stopped.wait(1))
        finally:
            release.set()
            for thread in threads:
                if thread.ident is not None:
                    thread.join(4)
        self.assertFalse(any(thread.is_alive() for thread in threads))
        self.assertEqual(events, ["osa.connect", "osa.close", "pm400.close"])
        self.assertEqual(len(errors), 1)
        self.assertIsInstance(errors[0], InstrumentSafetyError)
        self.assertTrue(errors[0].cleanup_report.ok)

    def test_connect_failure_joins_completed_failed_close_without_automatic_retry(self):
        cleanup_entered, continue_cleanup = threading.Event(), threading.Event()
        events, outcomes = [], {}
        primary = ValueError("connect failed")
        retained = OSError("gain still open")
        gain = Gain(events, {"close": retained})

        class PausedFailureSession(InstrumentSession):
            def _cleanup_preserving(self, error, **kwargs):
                cleanup_entered.set()
                if not continue_cleanup.wait(3):
                    raise AssertionError("cleanup gate not released")
                return super()._cleanup_preserving(error, **kwargs)

        session = PausedFailureSession(osa=Device("osa", events, {"connect": primary}), gain=gain)

        def connecting():
            try:
                session.connect()
            except BaseException as error:
                outcomes["connect"] = error

        thread = threading.Thread(target=connecting)
        try:
            thread.start()
            self.assertTrue(cleanup_entered.wait(1))
            with self.assertRaises(OSError) as caught:
                session.close()
            first_report = caught.exception.cleanup_report
        finally:
            continue_cleanup.set()
            thread.join(4)
        self.assertFalse(thread.is_alive())
        self.assertEqual(events, ["osa.connect", "gain.current_off", "gain.tec_off", "gain.close", "osa.close"])
        self.assertIs(outcomes["connect"], primary)
        self.assertIs(primary.cleanup_report, first_report)
        self.assertEqual(first_report.unreleased, (("gain", gain),))
        gain.failures.clear()
        events.clear()
        self.assertTrue(session.close().ok)
        self.assertEqual(events, ["gain.current_off", "gain.tec_off", "gain.close"])
        self.assertIs(primary.cleanup_report, first_report)
        self.assertFalse(first_report.ok)

    def test_close_waiters_keep_their_failed_attempt_when_one_resumes_late(self):
        connect_entered, continue_connect = threading.Event(), threading.Event()
        delayed_entered, continue_delayed = threading.Event(), threading.Event()
        waiting = {name: threading.Event() for name in ("fast-close", "delayed-close")}
        finished = {name: threading.Event() for name in ("connect", "fast-close", "delayed-close")}
        events, outcomes = [], {}
        primary = ValueError("connection failed")
        retained = OSError("gain retained")

        class DelayedWakeCondition(threading.Condition):
            """Hold one awakened waiter outside the lock to force overtaking."""
            def wait_for(self, predicate, timeout=None):
                name = threading.current_thread().name
                if name in waiting:
                    waiting[name].set()
                result = super().wait_for(predicate, timeout)
                if name == "delayed-close" and not delayed_entered.is_set():
                    self.release()
                    try:
                        delayed_entered.set()
                        if not continue_delayed.wait(3):
                            raise AssertionError("delayed waiter gate not released")
                    finally:
                        self.acquire()
                return result

        osa = Device("osa", events)

        def connect_osa():
            events.append("osa.connect")
            connect_entered.set()
            if not continue_connect.wait(3):
                raise AssertionError("connection gate not released")
            raise primary

        osa.connect = connect_osa
        gain = Gain(events, {"close": retained})
        session = InstrumentSession(osa=osa, gain=gain)
        session._condition = DelayedWakeCondition()

        def invoke(name, operation):
            try:
                operation()
            except BaseException as error:
                outcomes[name] = (error, getattr(error, "cleanup_report", None))
            finally:
                finished[name].set()

        threads = [threading.Thread(name="connect", target=invoke, args=("connect", session.connect)),
                   threading.Thread(name="fast-close", target=invoke, args=("fast-close", session.close)),
                   threading.Thread(name="delayed-close", target=invoke, args=("delayed-close", session.close))]
        try:
            threads[0].start()
            self.assertTrue(connect_entered.wait(1))
            threads[1].start()
            threads[2].start()
            self.assertTrue(waiting["fast-close"].wait(1))
            self.assertTrue(waiting["delayed-close"].wait(1))
            continue_connect.set()
            self.assertTrue(delayed_entered.wait(1))
            self.assertTrue(finished["fast-close"].wait(1))
            self.assertTrue(finished["connect"].wait(1))
            self.assertEqual(events, ["osa.connect", "gain.current_off", "gain.tec_off", "gain.close", "osa.close"])
            # A genuinely later explicit request may retry. The delayed caller
            # still belongs to the failed first attempt, not this new report.
            gain.failures.clear()
            retry_report = session.close()
            self.assertTrue(retry_report.ok)
        finally:
            continue_connect.set()
            continue_delayed.set()
            for thread in threads:
                if thread.ident is not None:
                    thread.join(4)
        self.assertFalse(any(thread.is_alive() for thread in threads))
        self.assertEqual(events, ["osa.connect", "gain.current_off", "gain.tec_off", "gain.close", "osa.close",
                                  "gain.current_off", "gain.tec_off", "gain.close"])
        self.assertIs(outcomes["connect"][0], primary)
        self.assertIs(outcomes["fast-close"][1], outcomes["connect"][1])
        self.assertIs(outcomes["delayed-close"][1], outcomes["connect"][1])
        self.assertIsNot(outcomes["delayed-close"][1], retry_report)
        self.assertFalse(outcomes["delayed-close"][1].ok)


class FiberHealthTests(unittest.TestCase):
    def setup_with(self, *states):
        config = load_fiber_coupling_config()
        drivers = {side: Device(side.value, []) for side, state in zip(StageSide, states)}
        for device, state in zip(drivers.values(), states):
            device.state = state
        setup = FiberCouplingSetup(config, FiberDiscovery({}, frozenset(), ()), drivers)
        return setup, drivers

    def test_unadopted_one_sided_setup_is_healthy_without_snapshot_or_motion_io(self):
        config = load_fiber_coupling_config()
        serial = config.stages[StageSide.LEFT].serial_number
        driver = FakeSetupMDT("FAKE", status=make_mdt_status(serial))
        driver.state = DriverState.READY
        setup = FiberCouplingSetup(config, FiberDiscovery({}, frozenset({StageSide.RIGHT}), ()),
                                   {StageSide.LEFT: driver})
        try:
            self.assertFalse(setup.left.status.baseline_known)
            reads = driver.status_reads
            self.assertEqual(setup.state, DriverState.READY)
            self.assertEqual(InstrumentSession(fiber=setup).check_health()[0].state, DriverState.READY)
            self.assertEqual(driver.status_reads, reads)
            self.assertEqual(driver.set_calls, [])
            self.assertEqual(driver.adopt_calls, [])
            self.assertEqual(driver.connect_calls, 0)
        finally:
            setup.close()
        self.assertEqual(setup.state, DriverState.DISCONNECTED)

    def test_benign_attestation_text_does_not_override_driver_lifecycle(self):
        setup, drivers = self.setup_with(DriverState.READY, DriverState.ACTIVE)
        for device in drivers.values():
            device.status = make_mdt_status("fake", axis_command_known=True,
                                            fault_evidence=AXIS_BASELINE_ATTESTATION_EVIDENCE)
        try:
            self.assertEqual(setup.state, DriverState.ACTIVE)
        finally:
            setup.close()

    def test_aggregate_rejects_fault_disconnect_and_unknown_children(self):
        for child, expected in ((DriverState.FAULT, DriverState.FAULT),
                                (DriverState.DISCONNECTED, DriverState.FAULT),
                                (DriverState.CLOSING, DriverState.CLOSING),
                                (None, None), ("READY", None)):
            with self.subTest(child=child):
                setup, _ = self.setup_with(DriverState.READY, child)
                self.assertEqual(setup.state, expected)
                with self.assertRaises((DeviceFault, InstrumentSafetyError)):
                    InstrumentSession(fiber=setup).check_health()
                setup.close()

    def test_retained_closing_setup_is_not_healthy(self):
        setup, drivers = self.setup_with(DriverState.READY)
        device = next(iter(drivers.values()))
        device.failures["close"] = OSError("retained")
        with self.assertRaises(Exception):
            setup.close()
        self.assertIn(setup.state, (DriverState.CLOSING, DriverState.FAULT))
        device.failures.clear()
        setup.close()
        self.assertEqual(setup.state, DriverState.DISCONNECTED)


if __name__ == "__main__":
    unittest.main()
