"""Offline contracts for shared VISA ownership; no real backend is opened."""
import gc
import unittest
import weakref
from concurrent.futures import ThreadPoolExecutor
from threading import Event
from unittest.mock import Mock, patch

import pyvisa
from pyvisa.highlevel import ResourceInfo, VisaLibraryBase
from pyvisa.constants import InterfaceType, StatusCode

from Code.Utils.common import InstrumentConnectionError
from Code.Utils.visa import acquire_visa_manager


class RecordingManager:
    def __init__(self):
        self.close_calls = 0

    def close(self):
        self.close_calls += 1


class LeaseTestCase(unittest.TestCase):
    def lease(self, manager, *, borrowed=False):
        lease = acquire_visa_manager(**(
            {"resource_manager": manager} if borrowed else {"factory": lambda: manager}
        ))
        self.addCleanup(lease.close)
        return lease


class VisaLeaseTests(LeaseTestCase):
    def test_last_lease_alone_closes_manager(self):
        manager = RecordingManager()
        first = acquire_visa_manager(factory=lambda: manager)
        second = acquire_visa_manager(factory=lambda: manager)
        try:
            first.close()
            self.assertTrue(first.released)
            self.assertEqual(manager.close_calls, 0)
            second.close()
            self.assertTrue(second.released)
            self.assertEqual(manager.close_calls, 1)
            second.close()
            self.assertEqual(manager.close_calls, 1)
        finally:
            first.close()
            second.close()

    def test_one_factory_returning_different_managers_keeps_ownership_separate(self):
        managers = [RecordingManager(), RecordingManager()]
        factory = iter(managers).__next__
        first = acquire_visa_manager(factory=factory)
        second = acquire_visa_manager(factory=factory)
        self.addCleanup(first.close)
        self.addCleanup(second.close)
        first.close()
        self.assertEqual([m.close_calls for m in managers], [1, 0])
        second.close()
        self.assertEqual([m.close_calls for m in managers], [1, 1])

    def test_borrowed_ownership_survives_either_acquisition_order(self):
        for borrow_first in (False, True):
            with self.subTest(borrow_first=borrow_first):
                manager = RecordingManager()
                first = self.lease(manager, borrowed=borrow_first)
                second = self.lease(manager, borrowed=not borrow_first)
                borrowed, owned = (first, second) if borrow_first else (second, first)
                borrowed.close()
                owned.close()
                later = self.lease(manager)
                later.close()
                self.assertEqual(manager.close_calls, 0)

    def test_finished_borrowed_metadata_does_not_retain_manager(self):
        manager = RecordingManager()
        ref = weakref.ref(manager)
        lease = acquire_visa_manager(resource_manager=manager)
        lease.close()
        del lease, manager
        gc.collect()
        self.assertIsNone(ref())

    def test_nonweak_borrowed_manager_is_rejected_without_close(self):
        class SlottedManager:
            __slots__ = ("close_calls",)
            def __init__(self):
                self.close_calls = 0
            def close(self):
                self.close_calls += 1
        manager = SlottedManager()
        with self.assertRaises(TypeError):
            acquire_visa_manager(resource_manager=manager)
        self.assertEqual(manager.close_calls, 0)

    def test_close_failure_retains_lease_until_explicit_retry(self):
        manager = RecordingManager()
        original_close = manager.close
        error = RuntimeError("backend close failed")
        def failing_close():
            original_close()
            raise error
        manager.close = failing_close
        lease = acquire_visa_manager(factory=lambda: manager)
        try:
            with self.assertRaises(RuntimeError) as caught:
                lease.close()
            self.assertIs(caught.exception, error)
            self.assertFalse(lease.released)
            with self.assertRaises(InstrumentConnectionError):
                acquire_visa_manager(factory=lambda: manager)
            with self.assertRaises(InstrumentConnectionError):
                lease.reserve_resource("GPIB::4")
        finally:
            manager.close = original_close
            lease.close()
        self.assertTrue(lease.released)
        self.assertEqual(manager.close_calls, 2)

    def test_closing_rejects_registration_without_blocking_other_managers(self):
        entered, finish = Event(), Event()
        manager = RecordingManager()
        def blocked_close():
            manager.close_calls += 1
            entered.set()
            if not finish.wait(5):
                raise TimeoutError("test did not release close")
        manager.close = blocked_close
        lease = acquire_visa_manager(factory=lambda: manager)
        pool = ThreadPoolExecutor(max_workers=2)
        future = pool.submit(lease.close)
        try:
            self.assertTrue(entered.wait(2))
            def check_during_close():
                with self.assertRaises(InstrumentConnectionError):
                    acquire_visa_manager(factory=lambda: manager)
                with self.assertRaises(InstrumentConnectionError):
                    lease.close()
                other = RecordingManager()
                other_lease = acquire_visa_manager(factory=lambda: other)
                try:
                    other_lease.close()
                    self.assertEqual(other.close_calls, 1)
                finally:
                    other_lease.close()
            pool.submit(check_during_close).result(timeout=2)
        finally:
            finish.set()
            try:
                future.result(timeout=3)
            finally:
                pool.shutdown(wait=True)
                lease.close()
        self.assertEqual(manager.close_calls, 1)

    def test_concurrent_lease_releases_close_once(self):
        manager = RecordingManager()
        leases = [self.lease(manager) for _ in range(12)]
        with ThreadPoolExecutor(max_workers=6) as pool:
            list(pool.map(lambda lease: lease.close(), leases))
        self.assertEqual(manager.close_calls, 1)

    def test_interrupted_close_can_be_retried_without_retaining_manager(self):
        class InterruptedManager(RecordingManager):
            def close(self):
                super().close()
                if self.close_calls == 1:
                    raise KeyboardInterrupt("cancelled close")
        manager = InterruptedManager()
        reference = weakref.ref(manager)
        lease = acquire_visa_manager(factory=lambda: manager)
        try:
            with self.assertRaises(KeyboardInterrupt):
                lease.close()
            self.assertFalse(lease.released)
            with self.assertRaises(InstrumentConnectionError):
                acquire_visa_manager(resource_manager=manager)
        finally:
            lease.close()
        self.assertEqual(manager.close_calls, 2)
        del lease, manager
        gc.collect()
        self.assertIsNone(reference())

    def test_nonweak_owned_manager_can_close_normally(self):
        class SlottedManager:
            __slots__ = ("closed",)
            def close(self):
                self.closed = True
        manager = SlottedManager()
        lease = self.lease(manager)
        lease.close()
        self.assertTrue(manager.closed)

    def test_factories_cannot_enroll_manager_closed_before_they_return(self):
        class SlottedManager:
            __slots__ = ("close_calls",)
            def __init__(self):
                self.close_calls = 0
            def close(self):
                self.close_calls += 1

        for manager_type in (RecordingManager, SlottedManager):
            with self.subTest(manager_type=manager_type.__name__):
                manager = manager_type()
                owner = acquire_visa_manager(factory=lambda: manager)
                entered = [Event(), Event()]
                finish = [Event(), Event()]
                accepted = []
                pool = ThreadPoolExecutor(max_workers=2)

                def acquire_delayed(index):
                    def factory():
                        captured = manager
                        entered[index].set()
                        if not finish[index].wait(5):
                            raise TimeoutError("test did not release factory")
                        return captured
                    return acquire_visa_manager(factory=factory)

                futures = [pool.submit(acquire_delayed, i) for i in range(2)]
                try:
                    self.assertTrue(all(event.wait(2) for event in entered))
                    owner.close()
                    self.assertEqual(manager.close_calls, 1)
                    for index, future in enumerate(futures):
                        finish[index].set()
                        with self.assertRaises(InstrumentConnectionError):
                            accepted.append(future.result(timeout=2))
                    self.assertEqual(manager.close_calls, 1)
                finally:
                    for event in finish:
                        event.set()
                    pool.shutdown(wait=True)
                    for future in futures:
                        if future.exception() is None:
                            future.result().close()
                    owner.close()

    def test_closed_weak_manager_cannot_be_reenrolled(self):
        manager = RecordingManager()
        lease = acquire_visa_manager(factory=lambda: manager)
        lease.close()
        for options in ({"factory": lambda: manager}, {"resource_manager": manager}):
            accepted = None
            try:
                with self.assertRaises(InstrumentConnectionError):
                    accepted = acquire_visa_manager(**options)
            finally:
                if accepted is not None:
                    accepted.close()
        self.assertEqual(manager.close_calls, 1)

    def test_cancelled_factory_releases_retired_nonweak_manager(self):
        class SlottedManager:
            __slots__ = ("payload",)
            def close(self):
                pass

        entered, finish = Event(), Event()
        def cancelled_factory():
            entered.set()
            if not finish.wait(5):
                raise TimeoutError("test did not release factory")
            raise KeyboardInterrupt("cancelled factory")

        manager = SlottedManager()
        manager.payload = RecordingManager()
        payload = weakref.ref(manager.payload)
        owner = acquire_visa_manager(factory=lambda: manager)
        pool = ThreadPoolExecutor(max_workers=1)
        future = pool.submit(acquire_visa_manager, factory=cancelled_factory)
        try:
            self.assertTrue(entered.wait(2))
            owner.close()
            del owner, manager
            finish.set()
            with self.assertRaises(KeyboardInterrupt):
                future.result(timeout=2)
            gc.collect()
            self.assertIsNone(payload())
        finally:
            finish.set()
            pool.shutdown(wait=True)
            if "owner" in locals():
                owner.close()

    def test_reentrant_retired_destructor_preserves_factory_result_or_error(self):
        for should_fail in (False, True):
            with self.subTest(should_fail=should_fail):
                completed, destructor_errors = [], []

                class ReentrantManager:
                    __slots__ = ()
                    def close(self):
                        pass
                    def __del__(self):
                        try:
                            nested = acquire_visa_manager(factory=RecordingManager)
                            try:
                                nested.close()
                                completed.append("nested acquisition closed")
                            finally:
                                nested.close()
                        except BaseException as error:
                            destructor_errors.append(error)

                owners = [acquire_visa_manager(factory=ReentrantManager)]
                factory_error = ValueError("original factory failure")
                manager = RecordingManager()
                def factory():
                    owners.pop().close()
                    if should_fail:
                        raise factory_error
                    return manager

                lease = None
                try:
                    if should_fail:
                        with self.assertRaises(ValueError) as caught:
                            acquire_visa_manager(factory=factory)
                        self.assertIs(caught.exception, factory_error)
                    else:
                        lease = acquire_visa_manager(factory=factory)
                        self.assertIs(lease.manager, manager)
                        self.assertFalse(lease.released)
                        lease.close()
                        self.assertEqual(manager.close_calls, 1)
                    self.assertEqual(completed, ["nested acquisition closed"])
                    self.assertEqual(destructor_errors, [])
                finally:
                    if lease is not None:
                        lease.close()
                    for owner in owners:
                        owner.close()

    def test_retired_destructor_does_not_hold_global_registry_lock(self):
        entered, finish = Event(), Event()
        destructor_errors = []

        class BlockingDestructor:
            __slots__ = ()
            def close(self):
                pass
            def __del__(self):
                entered.set()
                if not finish.wait(5):
                    destructor_errors.append("test did not release destructor")

        owners = [acquire_visa_manager(factory=BlockingDestructor)]
        def factory():
            owners.pop().close()
            return RecordingManager()

        pool = ThreadPoolExecutor(max_workers=2)
        future = pool.submit(acquire_visa_manager, factory=factory)
        other_future = None
        try:
            self.assertTrue(entered.wait(2))
            other_future = pool.submit(acquire_visa_manager, factory=RecordingManager)
            other_future.result(timeout=2).close()
        finally:
            finish.set()
            try:
                future.result(timeout=3).close()
                if other_future is not None:
                    other_future.result(timeout=3).close()
            finally:
                pool.shutdown(wait=True)
                for owner in owners:
                    owner.close()
        self.assertEqual(destructor_errors, [])


class VisaReservationTests(LeaseTestCase):
    def reservation(self, lease, name):
        reservation = lease.reserve_resource(name)
        self.addCleanup(reservation.release)
        return reservation

    def test_same_canonical_target_blocked_across_distinct_managers(self):
        first = self.lease(RecordingManager())
        second = self.lease(RecordingManager())
        reserved = self.reservation(first, "  GPIB::4  ")
        self.assertEqual(reserved.canonical_name, "GPIB0::4::INSTR")
        with self.assertRaises(InstrumentConnectionError):
            second.reserve_resource("GPIB0::4::INSTR")
        reserved.release()
        reserved.release()
        self.assertTrue(reserved.released)
        replacement = self.reservation(second, "GPIB0::4::INSTR")
        self.assertFalse(replacement.released)

    def test_reservation_blocks_owner_close_but_other_targets_are_independent(self):
        manager = RecordingManager()
        lease = self.lease(manager)
        first = self.reservation(lease, "GPIB::4")
        second = self.reservation(lease, "GPIB::5")
        with self.assertRaises(InstrumentConnectionError):
            lease.close()
        self.assertFalse(lease.released)
        self.assertEqual(manager.close_calls, 0)
        first.release()
        second.release()
        lease.close()
        self.assertEqual(manager.close_calls, 1)
        with self.assertRaises(InstrumentConnectionError):
            lease.reserve_resource("GPIB::4")

    def test_backend_alias_expansion_and_usb_case_are_preserved(self):
        class AliasManager(RecordingManager):
            def resource_info(self, name):
                if name != "meter":
                    raise ValueError("unknown alias")
                return ResourceInfo(InterfaceType.usb, 0, "INSTR",
                                    "USB0::0x1313::0x8078::AbC123::0::INSTR", "meter")
        lease = self.lease(AliasManager())
        reserved = self.reservation(lease, " meter ")
        self.assertEqual(reserved.canonical_name, "USB0::0x1313::0x8078::AbC123::0::INSTR")
        other = self.lease(RecordingManager())
        with self.assertRaises(InstrumentConnectionError):
            other.reserve_resource("USB0::0x1313::0x8078::AbC123::0::INSTR")
        different = self.reservation(other, "USB0::0x1313::0x8078::abc123::0::INSTR")
        self.assertNotEqual(different.canonical_name, reserved.canonical_name)

    def test_invalid_names_and_unresolved_aliases_are_rejected(self):
        lease = self.lease(RecordingManager())
        for name in ("", " ", "unknown_alias", "USB::1", "GPIB::4::INSTR::junk"):
            with self.subTest(name=name), self.assertRaises(InstrumentConnectionError):
                lease.reserve_resource(name)

    def test_backend_parse_failure_falls_back_only_to_valid_resource_name(self):
        class FailedParser(RecordingManager):
            def resource_info(self, name):
                raise ValueError("parse failed")
        lease = self.lease(FailedParser())
        reserved = self.reservation(lease, "GPIB::4")
        self.assertEqual(reserved.canonical_name, "GPIB0::4::INSTR")
        with self.assertRaises(InstrumentConnectionError):
            lease.reserve_resource("meter")

    def test_inflight_name_resolution_prevents_lease_close(self):
        entered, finish = Event(), Event()
        class SlowParser(RecordingManager):
            def resource_info(self, name):
                entered.set()
                if not finish.wait(5):
                    raise TimeoutError("test did not release parser")
                return ResourceInfo(InterfaceType.gpib, 0, "INSTR", "GPIB0::4::INSTR", None)
        manager = SlowParser()
        lease = self.lease(manager)
        pool = ThreadPoolExecutor(max_workers=2)
        future = pool.submit(lease.reserve_resource, "meter")
        try:
            self.assertTrue(entered.wait(2))
            def close_during_parse():
                with self.assertRaises(InstrumentConnectionError):
                    lease.close()
            pool.submit(close_during_parse).result(timeout=2)
            self.assertEqual(manager.close_calls, 0)
        finally:
            finish.set()
            try:
                reservation = future.result(timeout=3)
                reservation.release()
            finally:
                pool.shutdown(wait=True)

    def test_cancelled_resolution_releases_inflight_ownership(self):
        class InterruptedParser(RecordingManager):
            def resource_info(self, name):
                raise KeyboardInterrupt("cancelled parse")
        manager = InterruptedParser()
        lease = self.lease(manager)
        with self.assertRaises(KeyboardInterrupt):
            lease.reserve_resource("meter")
        lease.close()
        self.assertEqual(manager.close_calls, 1)

    def test_concurrent_reservations_have_one_winner(self):
        leases = [self.lease(RecordingManager()) for _ in range(8)]
        def attempt(lease):
            try:
                return lease.reserve_resource("GPIB::4")
            except InstrumentConnectionError:
                return None
        reservations = []
        try:
            with ThreadPoolExecutor(max_workers=8) as pool:
                reservations = list(pool.map(attempt, leases))
            self.assertEqual(sum(r is not None for r in reservations), 1)
        finally:
            for reservation in reservations:
                if reservation is not None:
                    reservation.release()


class RealPyVisaTests(unittest.TestCase):
    def test_shared_real_manager_stays_open_until_last_lease(self):
        lib = Mock(spec=VisaLibraryBase)
        lib.resource_manager = None
        lib.open_default_resource_manager.return_value = (901, StatusCode.success)
        lib.parse_resource_extended.return_value = (
            ResourceInfo(InterfaceType.gpib, 0, "INSTR", "GPIB0::4::INSTR", "osa"),
            StatusCode.success,
        )
        with patch("pyvisa.highlevel.open_visa_library", side_effect=AssertionError("real backend forbidden")):
            first = acquire_visa_manager(factory=lambda: pyvisa.ResourceManager(lib))
            second = acquire_visa_manager(factory=lambda: pyvisa.ResourceManager(lib))
            reservation = None
            try:
                self.assertIs(first.manager, second.manager)
                reservation = second.reserve_resource("osa")
                first.close()
                self.assertEqual(second.manager.session, 901)
                lib.close.assert_not_called()
                self.assertEqual(reservation.canonical_name, "GPIB0::4::INSTR")
                reservation.release()
                second.close()
                lib.close.assert_called_once_with(901)
                with self.assertRaises(pyvisa.errors.InvalidSession):
                    _ = second.manager.session
            finally:
                if reservation is not None:
                    reservation.release()
                first.close()
                second.close()

    def test_real_manager_close_failure_keeps_session_owned_for_retry(self):
        lib = Mock(spec=VisaLibraryBase)
        lib.resource_manager = None
        lib.open_default_resource_manager.return_value = (902, StatusCode.success)
        lib.close.side_effect = [RuntimeError("mock backend failed"), None]
        with patch("pyvisa.highlevel.open_visa_library", side_effect=AssertionError("real backend forbidden")):
            lease = acquire_visa_manager(factory=lambda: pyvisa.ResourceManager(lib))
            try:
                with self.assertRaises(RuntimeError):
                    lease.close()
                self.assertFalse(lease.released)
                self.assertEqual(lease.manager.session, 902)
                with self.assertRaises(InstrumentConnectionError):
                    acquire_visa_manager(factory=lambda: pyvisa.ResourceManager(lib))
            finally:
                lease.close()
            self.assertTrue(lease.released)
            self.assertEqual(lib.close.call_count, 2)
            with self.assertRaises(pyvisa.errors.InvalidSession):
                _ = lease.manager.session

    def test_real_borrowed_manager_survives_factory_lease_release(self):
        lib = Mock(spec=VisaLibraryBase)
        lib.resource_manager = None
        lib.open_default_resource_manager.return_value = (903, StatusCode.success)
        with patch("pyvisa.highlevel.open_visa_library", side_effect=AssertionError("real backend forbidden")):
            manager = pyvisa.ResourceManager(lib)
            borrowed = acquire_visa_manager(resource_manager=manager)
            owned = acquire_visa_manager(factory=lambda: pyvisa.ResourceManager(lib))
            try:
                borrowed.close()
                owned.close()
                self.assertEqual(manager.session, 903)
                lib.close.assert_not_called()
            finally:
                borrowed.close()
                owned.close()
                manager.close()  # Explicit external-owner cleanup, not a lease close.

if __name__ == "__main__":
    unittest.main()
