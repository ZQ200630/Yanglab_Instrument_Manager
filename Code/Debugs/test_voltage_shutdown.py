"""Finite serial-boundary regressions for Windows read/close ownership."""

import _thread
import threading
import time
import unittest
from unittest.mock import patch

from Code.Debugs.test_voltage_evidence import EvidenceSerial
from Code.Utils.common import DeviceFault, DriverState
from Code.Utils.voltage import VoltageSource


class PendingReadSerial(EvidenceSerial):
    def __init__(self, *, cancellable=True):
        super().__init__()
        self.in_read = False
        self.pending_read = threading.Event()
        self.finish_read = threading.Event()
        self.read_finished = threading.Event()
        self.close_overlap = False
        self.close_reads = 0
        self.cancellable = cancellable

    def read_until(self, expected):
        self.in_read = True
        try:
            frame = super().read_until(expected)
            if len(self.writes) >= 2:
                self.close_reads += 1
                if self.close_reads >= 3:
                    self.read_finished.clear()
                    self.pending_read.set()
                    self.finish_read.wait(4.0)
            return frame
        finally:
            self.in_read = False
            self.read_finished.set()

    def cancel_read(self):
        if self.cancellable:
            self.finish_read.set()

    def close(self):
        self.close_overlap = self.in_read
        # Characterize the destructive Windows close: it invalidates the read
        # handle while releasing its wait, before read_until has unwound.
        super().close()
        self.finish_read.set()


class VoltageShutdownTests(unittest.TestCase):
    def connected(self, *, cancellable=True):
        transport = PendingReadSerial(cancellable=cancellable)
        driver = VoltageSource(port="TEST", io_timeout=0.01, startup_timeout=0.5,
                               serial_factory=lambda **_: transport).connect()
        self.addCleanup(self.cleanup, driver, transport)
        return driver, transport

    @staticmethod
    def cleanup(driver, transport):
        transport.finish_read.set()
        try:
            driver.close()
        except DeviceFault:
            pass

    def close_with_pending_read(self, driver, transport):
        original_wait = driver._status_changed.wait

        def wait(timeout):
            result = original_wait(timeout)
            if driver.zero_evidence.state.value == "measured_zero":
                driver._status_changed.release()
                try:
                    self.assertTrue(transport.pending_read.wait(1.0))
                finally:
                    driver._status_changed.acquire()
            return result

        with patch.object(driver._status_changed, "wait", side_effect=wait):
            driver.close()

    def test_close_waits_for_cancelled_read_before_destroying_serial_handle(self):
        # Catches serial.close() preceding reader cancellation and join.
        driver, transport = self.connected()
        self.close_with_pending_read(driver, transport)
        self.assertFalse(transport.close_overlap, "serial handle destroyed during read")
        self.assertFalse(transport.in_read)
        self.assertTrue(driver.resources_released)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertEqual(driver.zero_evidence.state.value, "measured_zero")

    def test_unstoppable_reader_keeps_transport_owned_until_later_cleanup(self):
        # Catches closing a live read merely to claim the port was released.
        driver, transport = self.connected(cancellable=False)
        with self.assertRaises(DeviceFault):
            self.close_with_pending_read(driver, transport)
        self.assertTrue(transport.is_open)
        self.assertTrue(driver._reader_thread.is_alive())
        self.assertFalse(transport.close_overlap)
        self.assertFalse(driver.resources_released)
        self.assertEqual(driver.state, DriverState.FAULT)
        transport.finish_read.set()
        driver._reader_thread.join(1.0)
        with self.assertRaises(DeviceFault):
            driver.close()
        self.assertTrue(driver.resources_released)
        self.assertFalse(transport.close_overlap)

    def test_interrupted_join_keeps_pending_read_owned_on_cleanup_retry(self):
        # Catches trusting CPython 3.10's poisoned is_alive() after Ctrl+C,
        # including a later close attempt while the same read remains pending.
        driver, transport = self.connected(cancellable=False)
        reader = driver._reader_thread
        original_join = reader.join
        interrupt_sent = threading.Event()

        def interrupted_join(timeout):
            def interrupt():
                # read_until is held for four seconds; deliver Ctrl+C while
                # the real bounded join is blocked, never during startup.
                time.sleep(0.02)
                _thread.interrupt_main()
                interrupt_sent.set()

            interrupter = threading.Thread(target=interrupt)
            interrupter.start()
            try:
                original_join(timeout)
            finally:
                interrupter.join(1.0)

        try:
            with patch.object(reader, "join", side_effect=interrupted_join):
                with self.assertRaises(KeyboardInterrupt):
                    self.close_with_pending_read(driver, transport)
            self.assertTrue(interrupt_sent.is_set())
            self.assertFalse(transport.close_overlap, "interrupted join destroyed live read")
            self.assertTrue(transport.in_read)
            self.assertTrue(transport.is_open, "interrupted join destroyed live read")
            self.assertFalse(driver.resources_released)
            self.assertEqual(driver.state, DriverState.FAULT)
            with self.assertRaises(DeviceFault):
                driver.close()
            self.assertTrue(transport.is_open, "retry trusted poisoned thread metadata")
            self.assertFalse(transport.close_overlap)
            self.assertFalse(driver.resources_released)
        finally:
            transport.finish_read.set()
            self.assertTrue(transport.read_finished.wait(1.0))
        with self.assertRaises(DeviceFault):
            driver.close()
        self.assertTrue(driver.resources_released)
        self.assertFalse(transport.close_overlap)


if __name__ == "__main__":
    unittest.main()
