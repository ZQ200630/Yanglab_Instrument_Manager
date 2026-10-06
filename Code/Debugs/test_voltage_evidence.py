"""Offline regressions for host-observed zero evidence, using a serial boundary fake."""
import threading
import time
import unittest
from dataclasses import replace
from unittest.mock import patch

from Code.Utils.common import DriverError, DriverState, InstrumentProtocolError
from Code.Utils.voltage import VoltageSource, decode_telemetry


ZERO_FRAME = b"\x00" * 32 + b"\r\n"


class EvidenceSerial:
    """Continuous telemetry with an explicit gate on one already-started read."""

    def __init__(self):
        self.is_open = True
        self.telemetry = ZERO_FRAME
        self.enabled = True
        self.writes = []
        self.read_entered = threading.Event()
        self.release_read = threading.Event()
        self.gate = False
        self.write_effects = []
        self.close_error = None
        self.unblocks_on_close = True

    def read_until(self, expected):
        value = self.telemetry
        if self.gate:
            self.gate = False
            self.read_entered.set()
            self.release_read.wait(timeout=3.0)
            return value
        time.sleep(0.005)
        return value if self.enabled and self.is_open else b""

    def write(self, frame):
        self.writes.append(bytes(frame))
        if self.write_effects:
            effect = self.write_effects.pop(0)
            if isinstance(effect, BaseException):
                raise effect
            if callable(effect):
                return effect()
            return effect
        return len(frame)

    def close(self):
        if self.close_error is not None:
            raise self.close_error
        self.is_open = False
        if self.unblocks_on_close:
            self.release_read.set()

    def cancel_read(self):
        if self.unblocks_on_close:
            self.release_read.set()


class VoltageEvidenceTests(unittest.TestCase):
    def test_resources_are_not_released_while_serial_open_is_pending(self):
        fake = EvidenceSerial()
        entered = threading.Event()
        release = threading.Event()
        errors = []
        def factory(**kwargs):
            entered.set()
            release.wait(1.0)
            return fake
        driver = VoltageSource(port="FAKE", serial_factory=factory)
        self.assertTrue(driver.resources_released)
        def connect():
            try:
                driver.connect()
            except BaseException as error:
                errors.append(error)
        worker = threading.Thread(target=connect)
        try:
            worker.start()
            self.assertTrue(entered.wait(0.5))
            self.assertFalse(driver.resources_released)
            with self.assertRaises(AttributeError):
                driver.resources_released = True
        finally:
            release.set()
            worker.join(2.0)
            self.cleanup(driver, fake)
        self.assertFalse(worker.is_alive())
        self.assertEqual(errors, [])
        self.assertTrue(driver.resources_released)

    def connected(self, **kwargs):
        fake = EvidenceSerial()
        driver = VoltageSource(port="FAKE", serial_factory=lambda **_: fake,
                               io_timeout=0.01, startup_timeout=0.5, **kwargs).connect()
        self.addCleanup(self.cleanup, driver, fake)
        return driver, fake

    @staticmethod
    def cleanup(driver, fake):
        fake.release_read.set()
        fake.close_error = None
        try:
            driver.close()
        except DriverError:
            pass

    def test_full_zero_write_without_new_telemetry_is_not_confirmation(self):
        # Catches treating the serial byte count as proof of output zero.
        driver, fake = self.connected()
        fake.enabled = False
        self.assertTrue(hasattr(driver, "zero_evidence"), "zero evidence interface missing")
        driver.zero(emergency=True)
        self.assertEqual(driver.zero_evidence.state.value, "command_sent")
        self.assertIsNone(driver.zero_evidence.observed_at)
        started = time.monotonic()
        with self.assertRaises(DriverError):
            driver.close()
        self.assertLess(time.monotonic() - started, 1.3)
        self.assertFalse(fake.is_open)
        self.assertEqual(driver.zero_evidence.state.value, "unknown")
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertTrue(driver.resources_released)

    def wait_state(self, driver, state):
        deadline = time.monotonic() + 0.6
        with driver._status_changed:
            while driver.zero_evidence.state.value != state:
                remaining = deadline - time.monotonic()
                self.assertGreater(remaining, 0, driver.zero_evidence)
                driver._status_changed.wait(remaining)

    def test_pre_write_inflight_read_cannot_confirm_latest_zero(self):
        driver, fake = self.connected()
        fake.gate = True
        self.assertTrue(fake.read_entered.wait(0.5))
        fake.enabled = False
        driver.zero(emergency=True)
        self.assertEqual(driver.zero_evidence.state.value, "command_sent")
        with driver._status_changed:
            before = driver._status_sequence
            fake.release_read.set()
            deadline = time.monotonic() + 0.5
            while driver._status_sequence == before:
                driver._status_changed.wait(max(0, deadline - time.monotonic()))
                self.assertLess(time.monotonic(), deadline)
            self.assertNotEqual(driver.zero_evidence.state.value, "measured_zero")

    def test_delayed_all_channel_zero_confirms_and_evidence_is_immutable(self):
        driver, fake = self.connected()
        # One nonzero channel must revoke the startup measurement.
        fake.telemetry = b"\x00" * 28 + b"\x00\x20\x00\x00\r\n"
        driver.zero(emergency=True)
        self.wait_state(driver, "unknown")
        fake.telemetry = ZERO_FRAME
        self.wait_state(driver, "measured_zero")
        evidence = driver.zero_evidence
        self.assertEqual(evidence.voltage_v, (0.0,) * 8)
        self.assertGreaterEqual(evidence.observed_at, evidence.sent_at)
        with self.assertRaises(AttributeError):
            evidence.sent_at = 0
        with self.assertRaises(AttributeError):
            driver.zero_evidence = evidence
        self.assertFalse(driver.resources_released)
        driver.close()
        self.assertTrue(driver.resources_released)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_nonzero_command_invalidates_observed_zero(self):
        driver, fake = self.connected()
        driver.set_channel(1, 0.1)
        self.assertEqual(driver.zero_evidence.state.value, "unknown")
        time.sleep(0.02)
        self.assertEqual(driver.zero_evidence.state.value, "unknown")

    def test_communication_failure_invalidates_observed_zero(self):
        driver, fake = self.connected()
        fake.enabled = False
        self.wait_state(driver, "unknown")

    def test_stale_evidence_expires_without_io(self):
        now = [10.0]
        driver, fake = self.connected(clock=lambda: now[0])
        fake.gate = True
        self.assertTrue(fake.read_entered.wait(0.5))
        now[0] = 12.0
        self.assertEqual(driver.zero_evidence.state.value, "unknown")

    def test_write_retry_does_not_reuse_previous_measurement(self):
        driver, fake = self.connected(write_retry_interval=0.001)
        fake.gate = True
        self.assertTrue(fake.read_entered.wait(0.5))
        fake.write_effects = [0, 18]
        driver.zero(emergency=True)
        self.assertEqual(len(fake.writes), 3)
        self.assertEqual(driver.zero_evidence.state.value, "command_sent")

    def test_interrupt_during_zero_still_releases_transport_and_reader(self):
        driver, fake = self.connected()
        fake.write_effects = [KeyboardInterrupt("stop")]
        with self.assertRaisesRegex(KeyboardInterrupt, "stop"):
            driver.close()
        self.assertFalse(fake.is_open)
        self.assertTrue(driver.resources_released)
        self.assertEqual(driver.zero_evidence.state.value, "unknown")

    def test_fault_stopped_reader_cannot_confirm_close(self):
        driver, fake = self.connected()
        driver._enter_fault(DriverError("injected failure"))
        with self.assertRaises(DriverError):
            driver.close()
        self.assertTrue(driver.resources_released)
        self.assertEqual(driver.zero_evidence.state.value, "unknown")

    def test_emergency_zero_does_not_wait_for_telemetry(self):
        driver, fake = self.connected()
        fake.gate = True
        self.assertTrue(fake.read_entered.wait(0.5))
        started = time.monotonic()
        driver.zero(emergency=True)
        self.assertLess(time.monotonic() - started, 0.1)
        self.assertEqual(driver.zero_evidence.state.value, "command_sent")

    def test_close_wait_budget_does_not_reuse_cached_zero(self):
        driver, fake = self.connected()
        fake.gate = True
        self.assertTrue(fake.read_entered.wait(0.5))
        started = time.monotonic()
        with self.assertRaises(DriverError):
            driver.close()
        elapsed = time.monotonic() - started
        self.assertGreaterEqual(elapsed, 0.95)
        self.assertLess(elapsed, 1.3)
        self.assertTrue(driver.resources_released)

    def test_interrupted_confirmation_wait_still_releases_every_resource(self):
        driver, fake = self.connected()
        fake.gate = True
        self.assertTrue(fake.read_entered.wait(0.5))
        interruption = KeyboardInterrupt("wait interrupted")
        with patch.object(driver._status_changed, "wait", side_effect=interruption):
            with self.assertRaises(KeyboardInterrupt) as raised:
                driver.close()
        self.assertIs(raised.exception, interruption)
        self.assertTrue(driver.resources_released)
        self.assertEqual(driver.zero_evidence.state.value, "unknown")

    def test_port_close_failure_retains_only_unreleased_transport(self):
        driver, fake = self.connected()
        fake.close_error = OSError("port still open")
        with self.assertRaises(OSError):
            driver.close()
        self.assertFalse(driver.resources_released)
        self.assertIs(driver._serial, fake)
        self.assertIsNone(driver._reader_thread)
        fake.close_error = None
        with self.assertRaises(DriverError):
            driver.close()
        self.assertTrue(driver.resources_released)

    def test_blocked_reader_retains_port_until_read_stops(self):
        driver, fake = self.connected()
        fake.unblocks_on_close = False
        fake.gate = True
        self.assertTrue(fake.read_entered.wait(0.5))
        with self.assertRaises(DriverError):
            driver.close()
        self.assertFalse(driver.resources_released)
        self.assertIs(driver._serial, fake)
        self.assertTrue(fake.is_open)
        self.assertTrue(driver._reader_thread.is_alive())
        fake.release_read.set()
        with self.assertRaises(DriverError):
            driver.close()
        self.assertTrue(driver.resources_released)

    def test_exact_tolerance_boundary_does_not_confirm_zero(self):
        driver, fake = self.connected()
        value = [0.05]
        # 50 mV is not representable on the 1.6 mV wire grid; exercise the
        # reader's exact boundary with otherwise genuinely decoded telemetry.
        def decoded(frame, received_at):
            return replace(decode_telemetry(frame, received_at),
                           voltage_v=(0.0,) * 7 + (value[0],))
        with patch("Code.Utils.voltage.decode_telemetry", side_effect=decoded):
            driver.zero(emergency=True)
            self.wait_state(driver, "unknown")
            value[0] = 0.049999
            self.wait_state(driver, "measured_zero")

    def test_read_started_during_write_cannot_confirm_at_same_clock_tick(self):
        driver, fake = self.connected(clock=lambda: 10.0)
        write_entered = threading.Event()
        release_write = threading.Event()
        errors = []
        def gated_write():
            write_entered.set()
            release_write.wait(1.0)
            return 18
        def zero():
            try:
                driver.zero(emergency=True)
            except BaseException as error:
                errors.append(error)
        fake.write_effects = [gated_write]
        worker = threading.Thread(target=zero)
        try:
            worker.start()
            self.assertTrue(write_entered.wait(0.5))
            fake.gate = True
            self.assertTrue(fake.read_entered.wait(0.5))
            fake.enabled = False
            release_write.set()
            worker.join(0.5)
            self.assertFalse(worker.is_alive())
            self.assertEqual(errors, [])
            with driver._status_changed:
                sequence = driver._status_sequence
                fake.release_read.set()
                deadline = time.monotonic() + 0.5
                while driver._status_sequence == sequence:
                    driver._status_changed.wait(max(0, deadline - time.monotonic()))
                    self.assertLess(time.monotonic(), deadline)
                self.assertNotEqual(driver.zero_evidence.state.value, "measured_zero")
        finally:
            release_write.set()
            fake.release_read.set()
            worker.join(1.0)

    def test_late_decoder_cannot_revoke_confirmed_shutdown_after_stop(self):
        driver, fake = self.connected()
        decoder_entered = threading.Event()
        release_decoder = threading.Event()
        original_wait = driver._status_changed.wait
        original_cancel = fake.cancel_read
        reads = [0]

        def decoded(frame, received_at):
            status = decode_telemetry(frame, received_at)
            reads[0] += 1
            if reads[0] >= 3:
                decoder_entered.set()
                release_decoder.wait(2.0)
                return replace(status, voltage_v=(1.0,) * 8)
            return status

        def wait_until_decoder_is_inflight(timeout):
            result = original_wait(timeout)
            # Let the reader deliver the qualifying zero, then begin decoding
            # its next frame before the closing caller decides to stop it.
            driver._status_changed.release()
            try:
                self.assertTrue(decoder_entered.wait(1.0))
            finally:
                driver._status_changed.acquire()
            return result

        def cancel_and_release_decoder():
            original_cancel()
            release_decoder.set()

        try:
            with patch("Code.Utils.voltage.decode_telemetry", side_effect=decoded), \
                    patch.object(driver._status_changed, "wait", side_effect=wait_until_decoder_is_inflight), \
                    patch.object(fake, "cancel_read", side_effect=cancel_and_release_decoder):
                driver.close()
            self.assertTrue(driver.resources_released)
            self.assertEqual(driver.zero_evidence.state.value, "measured_zero")
        finally:
            release_decoder.set()

    def test_late_reader_error_cannot_revoke_finalized_shutdown_evidence(self):
        # Both exception branches must recheck stop after acquiring the condition.
        for error_stage in ("read", "decode"):
            with self.subTest(error_stage=error_stage):
                driver, fake = self.connected()
                error_entered = threading.Event()
                release_error = threading.Event()
                original_wait = driver._status_changed.wait
                original_cancel = fake.cancel_read
                original_read = fake.read_until
                original_error = driver._record_closing_reader_error
                calls = [0]

                def read(expected):
                    frame = original_read(expected)
                    if error_stage == "read":
                        calls[0] += 1
                        if calls[0] >= 3:
                            raise OSError("late serial read failure")
                    return frame

                def decode(frame, received_at):
                    status = decode_telemetry(frame, received_at)
                    if error_stage == "decode":
                        calls[0] += 1
                        if calls[0] >= 3:
                            raise InstrumentProtocolError("late decode failure")
                    return status

                def gated_error(error):
                    # The reader has already passed its outer stop check.
                    error_entered.set()
                    release_error.wait(2.0)
                    return original_error(error)

                def wait_until_error_is_inflight(timeout):
                    result = original_wait(timeout)
                    driver._status_changed.release()
                    try:
                        self.assertTrue(error_entered.wait(1.0))
                    finally:
                        driver._status_changed.acquire()
                    return result

                def cancel_and_release_error():
                    original_cancel()
                    release_error.set()

                try:
                    with patch.object(fake, "read_until", side_effect=read), \
                            patch("Code.Utils.voltage.decode_telemetry", side_effect=decode), \
                            patch.object(driver, "_record_closing_reader_error", side_effect=gated_error), \
                            patch.object(driver._status_changed, "wait", side_effect=wait_until_error_is_inflight), \
                            patch.object(fake, "cancel_read", side_effect=cancel_and_release_error):
                        driver.close()
                    self.assertEqual(driver.state, DriverState.DISCONNECTED)
                    self.assertTrue(driver.resources_released)
                    self.assertEqual(driver.zero_evidence.state.value, "measured_zero")
                    self.assertIsNone(driver.cleanup_error)
                finally:
                    release_error.set()
                    self.cleanup(driver, fake)


if __name__ == "__main__":
    unittest.main()
