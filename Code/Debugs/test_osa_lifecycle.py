"""Offline OSA lifetime tests; only backend I/O is simulated."""
import unittest
import threading
import time
from unittest.mock import Mock, patch

import pyvisa
from pyvisa.constants import InterfaceType, StatusCode
from pyvisa.highlevel import ResourceInfo, VisaLibraryBase

from Code.Debugs.test_pm400 import FakeVisaResource
from Code.Utils.common import (DriverError, DriverState, InstrumentConnectionError,
                               InstrumentProtocolError, InstrumentTimeoutError)
from Code.Utils.osa import AQ6370
from Code.Utils.pm400 import PM400


class OsaLifecycleTests(unittest.TestCase):
    def test_failure_logging_never_replaces_primary_or_skips_cleanup(self):
        for operation in ("connect", "acquire", "context"):
            for failure in (KeyboardInterrupt("stop"), SystemExit("exit"),
                            InstrumentConnectionError("primary")):
                with self.subTest(operation=operation, failure=type(failure).__name__):
                    driver, resource, manager, instrument = self.make_driver()
                    if operation != "connect":
                        driver.connect()
                    cleanup = OSError("close failed")
                    resource.close.side_effect = cleanup
                    resource.query.side_effect = failure
                    try:
                        with patch.object(driver.log, "error", side_effect=RuntimeError("logger failed")):
                            with self.assertRaises(type(failure)) as caught:
                                if operation == "context":
                                    with driver:
                                        raise failure
                                else:
                                    getattr(driver, operation)()
                        self.assertIs(caught.exception, failure)
                        self.assertIs(driver.cleanup_error, cleanup)
                        self.assertIs(caught.exception.cleanup_error, cleanup)
                        self.assertEqual(driver.state, DriverState.FAULT)
                        resource.close.assert_called_once()
                    finally:
                        resource.close.side_effect = None
                        driver.close()

    def test_propagated_operation_error_retains_cleanup_evidence(self):
        for operation in ("connect", "acquire", "context"):
            with self.subTest(operation=operation):
                driver, resource, manager, instrument = self.make_driver(retries=0)
                if operation != "connect":
                    driver.connect()
                primary = OSError("transport failed")
                cleanup = OSError("close failed")
                resource.query.side_effect = primary
                resource.close.side_effect = cleanup
                expected = OSError if operation == "context" else InstrumentConnectionError
                try:
                    with self.assertRaises(expected) as caught:
                        if operation == "context":
                            with driver:
                                raise primary
                        else:
                            getattr(driver, operation)()
                    if operation == "context":
                        self.assertIs(caught.exception, primary)
                    else:
                        self.assertIs(caught.exception.__cause__, primary)
                    self.assertIs(caught.exception.cleanup_error, driver.cleanup_error)
                    self.assertIs(caught.exception.cleanup_error, cleanup)
                    resource.close.side_effect = None
                    driver.close()
                    self.assertIsNone(driver.cleanup_error)
                    self.assertIs(caught.exception.cleanup_error, cleanup)
                finally:
                    resource.close.side_effect = None
                    driver.close()

    def test_rejected_exception_annotation_preserves_primary_and_driver_evidence(self):
        class RejectAnnotations(InstrumentProtocolError):
            def __setattr__(self, name, value):
                if name in {"abort_error", "cleanup_error"}:
                    raise RuntimeError("annotation refused")
                super().__setattr__(name, value)
        driver, resource, manager, instrument = self.make_driver()
        driver.connect()
        primary = RejectAnnotations("sweep failed")
        abort_failure = OSError("abort failed")
        resource.query.side_effect = primary
        instrument.abort.side_effect = abort_failure
        try:
            with self.assertRaises(RejectAnnotations) as caught:
                driver.acquire()
            self.assertIs(caught.exception, primary)
            self.assertIs(driver.cleanup_error, abort_failure)
            instrument.initiate_sweep.assert_called_once()
        finally:
            instrument.abort.side_effect = None
            driver.close()

    def test_helper_start_failure_allows_explicit_cleanup_retry(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.04)
        driver.connect()
        try:
            with patch("Code.Utils.osa.Thread.start", side_effect=RuntimeError("cannot start thread")):
                with self.assertRaises(InstrumentConnectionError):
                    driver.close()
            self.assertIs(driver._resource, resource)
            driver.close()
            self.assertEqual(driver.state, DriverState.DISCONNECTED)
            self.assertIsNone(driver._lease)
            resource.close.assert_called_once()
        finally:
            # Settle even the original broken implementation's unstarted helpers.
            for helper in list(driver._cleanup_helpers.values()):
                if helper.thread.ident is None:
                    helper.thread.start()
                helper.thread.join(2)
            driver.close()

    def test_start_error_after_launch_retains_running_helper(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.04)
        entered, release = threading.Event(), threading.Event()
        actual_start = threading.Thread.start
        def abort():
            entered.set()
            if not release.wait(5):
                raise AssertionError("abort gate never released")
        def start_then_fail(thread):
            actual_start(thread)
            self.assertTrue(entered.wait(2))
            raise RuntimeError("start interrupted after launch")
        instrument.abort.side_effect = abort
        driver.connect()
        self.arm_owned_sweep(driver)
        try:
            with patch("Code.Utils.osa.Thread.start", start_then_fail):
                with self.assertRaises(DriverError):
                    driver.close()
            with self.assertRaises(DriverError):
                driver.close()
            self.assertIs(driver._resource, resource)
            self.assertIsNotNone(driver._lease)
            resource.close.assert_not_called()
            instrument.abort.assert_called_once()
        finally:
            self.release_and_clean(release, driver)
        instrument.abort.assert_called_once()

    def test_start_interruption_before_ident_retains_abort_ownership(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.04)
        driver.connect()
        self.arm_owned_sweep(driver)
        interrupt = KeyboardInterrupt("launch status unknown")
        try:
            with patch("Code.Utils.osa.Thread.start", side_effect=interrupt):
                with self.assertRaises(DriverError) as caught:
                    driver.close()
            self.assertIs(caught.exception.__cause__, interrupt)
            self.assertEqual(set(driver._cleanup_helpers), {"abort"})
            self.assertIs(driver._resource, resource)
            self.assertIsNotNone(driver._lease)
            with self.assertRaises(DriverError):
                driver.close()
            resource.close.assert_not_called()
            manager.close.assert_not_called()
        finally:
            # Simulate completion of the possibly delayed native launch.
            for helper in list(driver._cleanup_helpers.values()):
                if helper.thread.ident is None:
                    helper.thread.start()
                helper.thread.join(2)
            driver.close()

    def make_driver(self, **options):
        from Code.Debugs.test_osa_read import attach_trace_wire
        resource, manager, instrument = Mock(), Mock(), Mock()
        attach_trace_wire(resource)
        resource.query.side_effect = lambda command: "YOKOGAWA,AQ6370D,SN,FW" if command == "*IDN?" else "1"
        manager.open_resource.return_value = resource
        instrument.get_xdata.return_value = [1.549e-6, 1.550e-6]
        instrument.get_ydata.return_value = [-41.0, -40.0]
        driver = AQ6370(resource_manager_factory=lambda: manager,
                        instrument_factory=lambda resource: instrument, **options)
        return driver, resource, manager, instrument

    @staticmethod
    def arm_owned_sweep(driver):
        # Abort-cleanup cases model an unfinished sweep initiated by this session.
        # Idle connection must remain read-only, so do not arm every fixture.
        with driver._lifecycle_lock:
            driver._sweep_owned = True

    @staticmethod
    def launch(operation):
        results, errors = [], []
        def run():
            try:
                results.append(operation())
            except BaseException as error:
                errors.append(error)
        thread = threading.Thread(target=run)
        thread.start()
        return thread, results, errors

    def release_and_clean(self, release, driver, *threads):
        release.set()
        for thread in threads:
            thread.join(5)
            self.assertFalse(thread.is_alive(), "test operation did not finish")
        helpers = list(getattr(driver, "_cleanup_helpers", {}).values())
        driver.close()
        for helper in helpers:
            helper.thread.join(5)
            self.assertFalse(helper.thread.is_alive())

    def test_osa_close_preserves_live_pm400_on_real_shared_manager(self):
        lib = Mock(spec=VisaLibraryBase)
        lib.resource_manager = None
        lib.open_default_resource_manager.return_value = (940, StatusCode.success)
        lib.parse_resource_extended.side_effect = lambda session, name: (
            ResourceInfo(InterfaceType.gpib, 0, "INSTR", name, None), StatusCode.success
        )
        osa_resource = FakeVisaResource({"*IDN?": "YOKOGAWA,AQ6370D,SN,FW"})
        pm_resource = FakeVisaResource({
            "*IDN?": "Thorlabs,PM400,P001,1.0",
            "SYSTem:SENSor:IDN?": '"S130C","S001","cal",1,0,49',
            "SYSTem:VERSion?": "1999.0",
        })
        with patch("pyvisa.highlevel.open_visa_library", side_effect=AssertionError("hardware forbidden")):
            manager = pyvisa.ResourceManager(lib)
            def open_resource(name):
                resource = osa_resource if name == "GPIB0::4::INSTR" else pm_resource
                manager._created_resources.add(resource)
                return resource
            with patch.object(manager, "open_resource", side_effect=open_resource):
                pm = PM400("GPIB0::5::INSTR", resource_manager_factory=lambda: pyvisa.ResourceManager(lib))
                osa = AQ6370(resource_manager_factory=lambda: pyvisa.ResourceManager(lib),
                             instrument_factory=lambda resource: Mock())
                try:
                    pm.connect()
                    osa.connect()
                    osa.close()
                    self.assertFalse(pm_resource.closed)
                    self.assertEqual(manager.session, 940)
                    self.assertEqual(pm.system.get_scpi_version(), "1999.0")
                    self.assertEqual(osa_resource.mutating_commands, [])
                    self.assertEqual(pm_resource.mutating_commands, [])
                finally:
                    osa.close()
                    pm.close()

    def test_failed_resource_cleanup_retains_ownership_for_explicit_retry(self):
        resource, manager, instrument = Mock(), Mock(), Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,SN,FW"
        manager.open_resource.return_value = resource
        driver = AQ6370(resource_manager_factory=lambda: manager,
                        instrument_factory=lambda resource: instrument).connect()
        resource.close.side_effect = OSError("resource close failed")
        manager.close.side_effect = OSError("manager close failed")
        try:
            with self.assertRaises(DriverError):
                driver.close()
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsNotNone(driver.cleanup_error)
            self.assertIs(driver._resource, resource)
            resource.close.side_effect = None
            with self.assertRaises(DriverError):
                driver.close()
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIs(driver._manager, manager)
            self.assertIsNone(driver._resource)
        finally:
            resource.close.side_effect = None
            manager.close.side_effect = None
            driver.close()

    def test_close_cancels_blocked_acquire_with_default_total_budget(self):
        driver, resource, manager, instrument = self.make_driver()
        started, release = threading.Event(), threading.Event()
        def wait_for_opc(command):
            started.set()
            if not release.wait(8):
                raise AssertionError("OPC gate never released")
            return "1"
        driver.connect()
        resource.query.side_effect = wait_for_opc
        thread, results, errors = self.launch(driver.acquire)
        try:
            self.assertTrue(started.wait(2))
            before = time.monotonic()
            with self.assertRaises(DriverError):
                driver.close()
            elapsed = time.monotonic() - before
            self.assertGreaterEqual(elapsed, 1.8)
            self.assertLess(elapsed, 2.8)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIs(driver._resource, resource)
            resource.close.assert_not_called()
            manager.close.assert_not_called()
        finally:
            self.release_and_clean(release, driver, thread)
        self.assertEqual(results, [])
        self.assertIsInstance(errors[0], InstrumentConnectionError)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertFalse(any(call.args[0].startswith(":TRACe:X?") for call in resource.write.call_args_list))

    def test_close_blocks_late_spectrum_publication(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.08)
        started, release = threading.Event(), threading.Event()
        wire = resource.visalib
        original_read = wire.read
        def get_power(session, count):
            if wire.pending.startswith(":TRACe:Y?"):
                started.set()
                if not release.wait(5):
                    raise AssertionError("data gate never released")
            return original_read(session, count)
        wire.read = get_power
        driver.connect()
        thread, results, errors = self.launch(driver.acquire)
        try:
            self.assertTrue(started.wait(2))
            with self.assertRaises(DriverError):
                driver.close()
        finally:
            self.release_and_clean(release, driver, thread)
        self.assertEqual(results, [])
        self.assertIsInstance(errors[0], InstrumentConnectionError)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_blocked_resource_close_is_reaped_without_duplicate_helper(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.08)
        started, release = threading.Event(), threading.Event()
        def close_resource():
            started.set()
            if not release.wait(5):
                raise AssertionError("close gate never released")
        resource.close.side_effect = close_resource
        driver.connect()
        try:
            for _ in range(2):
                before = time.monotonic()
                with self.assertRaises(DriverError):
                    driver.close()
                self.assertLess(time.monotonic() - before, 0.5)
            self.assertTrue(started.is_set())
            resource.close.assert_called_once()
            manager.close.assert_not_called()
            self.assertIs(driver._resource, resource)
            self.assertEqual(driver.state, DriverState.FAULT)
        finally:
            self.release_and_clean(release, driver)
        resource.close.assert_called_once()
        self.assertIsNone(driver.cleanup_error)

    def test_blocked_abort_holds_resource_and_reuses_original_helper(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.08)
        release = threading.Event()
        instrument.abort.side_effect = lambda: release.wait(5) or (_ for _ in ()).throw(AssertionError("abort gate"))
        driver.connect()
        self.arm_owned_sweep(driver)
        try:
            for _ in range(2):
                with self.assertRaises(DriverError):
                    driver.close()
            instrument.abort.assert_called_once()
            resource.close.assert_not_called()
        finally:
            self.release_and_clean(release, driver)
        instrument.abort.assert_called_once()

    def test_blocked_manager_close_retains_lease_for_explicit_reap(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.08)
        release = threading.Event()
        manager.close.side_effect = lambda: release.wait(5) or (_ for _ in ()).throw(AssertionError("manager gate"))
        driver.connect()
        try:
            for _ in range(2):
                with self.assertRaises(DriverError):
                    driver.close()
            manager.close.assert_called_once()
            self.assertIs(driver._manager, manager)
            self.assertEqual(driver.state, DriverState.FAULT)
        finally:
            self.release_and_clean(release, driver)
        manager.close.assert_called_once()

    def test_acquisition_baseexception_survives_cleanup_failure(self):
        for failure in (KeyboardInterrupt("stop"), SystemExit("exit"), RuntimeError("bug")):
            with self.subTest(failure=type(failure).__name__):
                driver, resource, manager, instrument = self.make_driver()
                driver.connect()
                resource.query.side_effect = failure
                resource.close.side_effect = OSError("cleanup failed")
                try:
                    with self.assertRaises(type(failure)) as caught:
                        driver.acquire()
                    self.assertIs(caught.exception, failure)
                    self.assertEqual(driver.state, DriverState.FAULT)
                    self.assertIsNotNone(driver.cleanup_error)
                    self.assertIs(driver._resource, resource)
                finally:
                    resource.close.side_effect = None
                    driver.close()

    def test_close_cancels_connect_blocked_in_factory_without_late_ready(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.08)
        started, release = threading.Event(), threading.Event()
        def factory():
            started.set()
            if not release.wait(5):
                raise AssertionError("factory gate never released")
            return manager
        driver._resource_manager_factory = factory
        thread, results, errors = self.launch(driver.connect)
        try:
            self.assertTrue(started.wait(2))
            with self.assertRaises(DriverError):
                driver.close()
            with self.assertRaises(DriverError):
                driver.connect()
        finally:
            self.release_and_clean(release, driver, thread)
        self.assertEqual(results, [])
        self.assertIsInstance(errors[0], InstrumentConnectionError)
        manager.open_resource.assert_not_called()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_duplicate_alias_is_rejected_before_open_and_borrowed_manager_survives(self):
        driver, resource, manager, instrument = self.make_driver()
        # Use the exact shared borrowed object for both clients.
        manager.resource_info.return_value.resource_name = "GPIB0::4::INSTR"
        first = AQ6370(resource_manager=manager, instrument_factory=lambda resource: instrument)
        duplicate = AQ6370("GPIB::4", resource_manager=manager,
                           instrument_factory=lambda resource: instrument)
        try:
            first.connect()
            with self.assertRaises(DriverError):
                duplicate.connect()
            self.assertEqual(first.state, DriverState.READY)
            manager.open_resource.assert_called_once()
        finally:
            duplicate.close()
            first.close()
        manager.close.assert_not_called()

    def test_retry_abort_and_close_abort_never_overlap(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.08)
        started, release = threading.Event(), threading.Event()
        concurrent, peak = 0, 0
        counter_lock = threading.Lock()
        def abort():
            nonlocal concurrent, peak
            with counter_lock:
                concurrent += 1
                peak = max(peak, concurrent)
            started.set()
            try:
                if not release.wait(5):
                    raise AssertionError("retry abort gate never released")
            finally:
                with counter_lock:
                    concurrent -= 1
        driver.connect()
        resource.query.side_effect = TimeoutError("retry")
        instrument.abort.side_effect = abort
        thread, results, errors = self.launch(driver.acquire)
        try:
            self.assertTrue(started.wait(2))
            with self.assertRaises(DriverError):
                driver.close()
            self.assertEqual(peak, 1)
            instrument.initiate_sweep.assert_called_once()
        finally:
            self.release_and_clean(release, driver, thread)
        self.assertEqual(results, [])
        self.assertIsInstance(errors[0], InstrumentConnectionError)

    def test_pending_close_caller_keeps_reconnect_disarmed(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.5)
        first_started, first_release = threading.Event(), threading.Event()
        second_started, second_release = threading.Event(), threading.Event()
        actual_lock = threading.Lock()
        class GateSecondClose:
            calls = 0
            def acquire(self, **kwargs):
                self.calls += 1
                call = self.calls
                acquired = actual_lock.acquire(**kwargs)
                if acquired and call == 2:
                    second_started.set()
                    if not second_release.wait(5):
                        raise AssertionError("second close gate never released")
                return acquired
            def release(self):
                actual_lock.release()
        driver._close_lock = GateSecondClose()
        def close_manager():
            first_started.set()
            if not first_release.wait(5):
                raise AssertionError("first close gate never released")
        manager.close.side_effect = close_manager
        driver.connect()
        first, first_results, first_errors = self.launch(driver.close)
        second = None
        try:
            self.assertTrue(first_started.wait(2))
            second, second_results, second_errors = self.launch(driver.close)
            first_release.set()
            self.assertTrue(second_started.wait(2))
            factory = Mock(side_effect=AssertionError("reconnect must not reach backend"))
            driver._resource_manager_factory = factory
            with self.assertRaises(DriverError):
                driver.connect()
            factory.assert_not_called()
        finally:
            first_release.set()
            second_release.set()
            first.join(5)
            if second is not None:
                second.join(5)
            driver.close()
        self.assertFalse(first.is_alive())
        self.assertFalse(second.is_alive())
        self.assertEqual(first_errors + second_errors, [])

    def test_invalid_close_timeout_rejected_before_factory(self):
        for value in (0, -1, True, None, float("nan"), float("inf")):
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.make_driver(close_timeout=value)

    def test_retry_abort_failure_stops_sweeps_and_retains_operation_cause(self):
        for original in (TimeoutError("sweep timeout"), InstrumentProtocolError("bad sweep")):
            with self.subTest(original=type(original).__name__):
                driver, resource, manager, instrument = self.make_driver()
                driver.connect()
                resource.query.side_effect = original
                abort_failure = OSError("abort failed")
                instrument.abort.side_effect = abort_failure
                resource.close.side_effect = OSError("close failed")
                expected = InstrumentTimeoutError if isinstance(original, TimeoutError) else InstrumentProtocolError
                try:
                    with self.assertRaises(expected) as caught:
                        driver.acquire()
                    if isinstance(original, TimeoutError):
                        self.assertIs(caught.exception.__cause__, original)
                    else:
                        self.assertIs(caught.exception, original)
                    self.assertIs(caught.exception.abort_error, abort_failure)
                    self.assertEqual(driver.state, DriverState.FAULT)
                    self.assertIsNotNone(driver.cleanup_error)
                    instrument.initiate_sweep.assert_called_once()
                finally:
                    instrument.abort.side_effect = None
                    resource.close.side_effect = None
                    driver.close()

    def test_retry_abort_interrupt_preserves_its_identity(self):
        for interrupt in (KeyboardInterrupt("abort stop"), SystemExit("abort exit")):
            with self.subTest(interrupt=type(interrupt).__name__):
                driver, resource, manager, instrument = self.make_driver()
                driver.connect()
                resource.query.side_effect = TimeoutError("sweep")
                instrument.abort.side_effect = interrupt
                try:
                    with self.assertRaises(type(interrupt)) as caught:
                        driver.acquire()
                    self.assertIs(caught.exception, interrupt)
                    instrument.initiate_sweep.assert_called_once()
                finally:
                    instrument.abort.side_effect = None
                    driver.close()

    def test_late_fault_cleanup_cannot_close_a_new_connection(self):
        driver, resource, manager, instrument = self.make_driver()
        driver.connect()
        original_failure = RuntimeError("old transaction failed")
        resource.query.side_effect = original_failure
        reached, release = threading.Event(), threading.Event()
        actual_lock = driver._lifecycle_lock
        class GateFaultPublication:
            gated = False
            def __enter__(self):
                actual_lock.acquire()
            def __exit__(self, *args):
                should_gate = (not self.gated and threading.current_thread() is not threading.main_thread()
                               and driver.state == DriverState.FAULT)
                if should_gate:
                    self.gated = True
                actual_lock.release()
                if should_gate:
                    reached.set()
                    if not release.wait(5):
                        raise AssertionError("fault publication gate never released")
        driver._lifecycle_lock = GateFaultPublication()
        thread, results, errors = self.launch(driver.acquire)
        try:
            self.assertTrue(reached.wait(2))
            driver.close()
            new_resource, new_manager = Mock(), Mock()
            new_resource.query.return_value = "YOKOGAWA,AQ6370D,NEW,FW"
            new_manager.open_resource.return_value = new_resource
            driver._resource_manager_factory = lambda: new_manager
            driver.connect()
            release.set()
            thread.join(5)
            self.assertEqual(driver.state, DriverState.READY)
            new_resource.close.assert_not_called()
            self.assertIs(errors[0], original_failure)
        finally:
            self.release_and_clean(release, driver, thread)

    def test_close_shares_one_deadline_across_abort_and_resource_waits(self):
        driver, resource, manager, instrument = self.make_driver(close_timeout=0.4)
        abort_release, resource_release = threading.Event(), threading.Event()
        resource_started = threading.Event()
        timer = threading.Timer(0.25, abort_release.set)
        def abort():
            timer.start()
            if not abort_release.wait(5):
                raise AssertionError("abort deadline gate never released")
        def close_resource():
            resource_started.set()
            if not resource_release.wait(5):
                raise AssertionError("resource deadline gate never released")
        instrument.abort.side_effect = abort
        resource.close.side_effect = close_resource
        driver.connect()
        self.arm_owned_sweep(driver)
        try:
            before = time.monotonic()
            with self.assertRaises(DriverError):
                driver.close()
            elapsed = time.monotonic() - before
            self.assertGreaterEqual(elapsed, 0.35)
            self.assertLess(elapsed, 0.55)
            self.assertTrue(resource_started.is_set())
            manager.close.assert_not_called()
        finally:
            abort_release.set()
            timer.cancel()
            timer.join(5)
            self.release_and_clean(resource_release, driver)


if __name__ == "__main__":
    unittest.main()
