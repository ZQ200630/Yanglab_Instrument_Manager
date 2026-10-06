import logging
import threading
import time
import unittest
from dataclasses import replace
from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import Mock

import numpy as np
from pyvisa.constants import StatusCode
from pyvisa.errors import InvalidSession, VisaIOError

from Code.Utils.common import (
    DeviceFault,
    DriverError,
    DriverState,
    InstrumentConnectionError,
    InstrumentProtocolError,
    InstrumentSafetyError,
    InstrumentTimeoutError,
    find_serial_port,
    get_logger,
    require_state,
)
from Code.Utils.voltage import VoltageStatus, decode_telemetry, encode_voltages
from Code.Utils.gain import GainDriver, GainStatus, build_command, format_fixed, parse_ack, parse_reply


class GainProtocolTests(unittest.TestCase):
    def test_formats_fixed_three_decimal_value(self):
        self.assertEqual(format_fixed(22.0, 15.0, 40.0, "temperature"), "022000")
        self.assertEqual(format_fixed(200.0, 0.0, 200.0, "current"), "200000")

    def test_rejects_out_of_range_and_nonfinite_value(self):
        for value in (14.999, 40.001, float("nan"), float("inf")):
            with self.subTest(value=value):
                with self.assertRaises(InstrumentSafetyError):
                    format_fixed(value, 15.0, 40.0, "temperature")

    def test_builds_read_write_and_boolean_commands(self):
        self.assertEqual(build_command("RDTA"), b"RDTA\r\n")
        self.assertEqual(build_command("STEA", 22.0, 15.0, 40.0, "temperature"), b"STEA022000\r\n")
        self.assertEqual(build_command("STQA", True), b"STQA000001\r\n")

    def test_parses_expected_ready_field(self):
        self.assertEqual(parse_reply(b"READY;T=21.456\r\n", "T"), "21.456")

    def test_rejects_wrong_field_bad_prefix_and_missing_terminator(self):
        for reply in (b"READY;E=22.000\r\n", b"ERROR;T=21.456\r\n", b"READY;T=21.456"):
            with self.subTest(reply=reply):
                with self.assertRaises(InstrumentProtocolError):
                    parse_reply(reply, "T")

    def test_parses_generic_ready_acknowledgement(self):
        self.assertEqual(parse_ack(b"READY\r\n"), "READY")
        self.assertEqual(parse_ack(b"READY;P=0.350\r\n"), "READY;P=0.350")
        with self.assertRaises(InstrumentProtocolError):
            parse_ack(b"ERROR\r\n")


class FakeGainSerial:
    """Deterministic CRLF request/reply transport for the Gain driver."""

    def __init__(self, replies=(), *, write_results=(), auto_shutdown_replies=True):
        self.replies = list(replies)
        self.write_results = list(write_results)
        self.auto_shutdown_replies = auto_shutdown_replies
        self.writes = []
        self.read_terminators = []
        self.is_open = True

    def write(self, data):
        frame = bytes(data)
        self.writes.append(frame)
        return self.write_results.pop(0) if self.write_results else len(frame)

    def read_until(self, expected=b"\n"):
        self.read_terminators.append(expected)
        if self.auto_shutdown_replies and self.writes:
            command = self.writes[-1]
            expected_reply = {
                b"STQA000000\r\n": b"READY;Q=0\r\n",
                b"STRA000000\r\n": b"READY;D=0\r\n",
            }.get(command)
            if expected_reply is not None:
                if not self.replies or self.replies[0] != expected_reply:
                    if not self.replies or self.replies[0] != b"ERROR\r\n":
                        return expected_reply
        return self.replies.pop(0) if self.replies else b""

    def close(self):
        self.is_open = False

    def queue(self, *replies):
        self.replies.extend(replies)

    def queue_temperature(self, *temperatures):
        self.queue(*(f"READY;T={value:.3f}\r\n".encode("ascii") for value in temperatures))


class FakeClock:
    """Injectable monotonic clock for deterministic Gain timeout tests."""

    def __init__(self, value=0.0):
        self.value = value

    def __call__(self):
        return self.value

    def advance(self, seconds):
        self.value += seconds


class GateStopEvent:
    """Event double that makes a watchdog join observable without sleeping."""

    def __init__(self):
        self.entered_wait = threading.Event()
        self.set_called = threading.Event()
        self.release = threading.Event()
        self._set = False
        self._lock = threading.Lock()

    def clear(self):
        with self._lock:
            self._set = False

    def is_set(self):
        with self._lock:
            return self._set

    def set(self):
        with self._lock:
            self._set = True
        self.set_called.set()

    def wait(self, timeout=None):
        self.entered_wait.set()
        self.release.wait()
        return self.is_set()


class GatedEnableSerial:
    """Command-aware fake that pauses the final enable temperature read."""

    def __init__(self):
        self.writes = []
        self.read_terminators = []
        self.is_open = True
        self.temperature_reads = 0
        self.gate_enable_temperature = False
        self.enable_temperature_entered = threading.Event()
        self.release_enable_temperature = threading.Event()

    def write(self, data):
        frame = bytes(data)
        self.writes.append(frame)
        return len(frame)

    def read_until(self, expected=b"\n"):
        self.read_terminators.append(expected)
        command = self.writes[-1]
        if command == b"RDTA\r\n":
            self.temperature_reads += 1
            if self.gate_enable_temperature and self.temperature_reads == 8:
                self.enable_temperature_entered.set()
                self.release_enable_temperature.wait(timeout=2.0)
            return b"READY;T=22.000\r\n"
        replies = {
            b"RDEA\r\n": b"READY;E=22.000\r\n",
            b"RDRA\r\n": b"READY;R=1\r\n",
            b"RDCA\r\n": b"READY;C=150.000\r\n",
            b"RDQA\r\n": b"READY;Q=0\r\n",
            b"STQA000001\r\n": b"READY;Q=1\r\n",
            b"STQA000000\r\n": b"READY;Q=0\r\n",
            b"STRA000000\r\n": b"READY;D=0\r\n",
        }
        return replies[command]

    def close(self):
        self.is_open = False


def gain_startup_replies():
    return [
        b"READY;T=21.456\r\n",
        b"READY;E=22.000\r\n",
        b"READY;R=1\r\n",
        b"READY;C=150.000\r\n",
        b"READY;Q=0\r\n",
    ]


class GainConnectionTests(unittest.TestCase):
    def make_driver(self, fake, **kwargs):
        return GainDriver(
            port="COM5",
            serial_factory=lambda **serial_kwargs: fake,
            start_watchdog=False,
            **kwargs,
        )

    def test_connect_reads_complete_immutable_status_with_exact_8n1_configuration(self):
        fake = FakeGainSerial(gain_startup_replies())
        seen = []
        driver = GainDriver(
            port="COM5",
            serial_factory=lambda **serial_kwargs: (seen.append(serial_kwargs), fake)[1],
            start_watchdog=False,
        ).connect()

        self.assertIsInstance(driver.status, GainStatus)
        self.assertAlmostEqual(driver.status.temperature_c, 21.456)
        self.assertAlmostEqual(driver.status.target_c, 22.0)
        self.assertTrue(driver.status.tec_enabled)
        self.assertAlmostEqual(driver.status.current_ma, 150.0)
        self.assertFalse(driver.status.current_enabled)
        self.assertEqual(
            fake.writes,
            [b"RDTA\r\n", b"RDEA\r\n", b"RDRA\r\n", b"RDCA\r\n", b"RDQA\r\n"],
        )
        self.assertEqual(fake.read_terminators, [b"\r\n"] * 5)
        self.assertEqual(
            seen,
            [{
                "port": "COM5", "baudrate": 115200, "bytesize": 8, "parity": "N",
                "stopbits": 1, "timeout": 1.0, "write_timeout": 1.0,
            }],
        )
        with self.assertRaises((AttributeError, TypeError)):
            driver.status.target_c = 25.0
        driver.close()
        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
        self.assertFalse(fake.is_open)

    def test_connect_uses_r_for_rdra_read_and_d_for_stra_write(self):
        fake = FakeGainSerial([
            b"READY;T=21.344\r\n",
            b"READY;E=22.000\r\n",
            b"READY;R=1\r\n",
            b"READY;C=0.000\r\n",
            b"READY;Q=0\r\n",
            b"READY;Q=0\r\n",
            b"READY;D=0\r\n",
        ])
        driver = self.make_driver(fake).connect()

        self.assertTrue(driver.status.tec_enabled)
        driver.close()
        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
        self.assertFalse(fake.is_open)

    def test_connect_wrong_reply_field_shuts_down_then_closes_partial_connection(self):
        fake = FakeGainSerial([b"READY;E=22.000\r\n"])
        driver = self.make_driver(fake)

        with self.assertRaises(InstrumentProtocolError):
            driver.connect()

        self.assertEqual(fake.writes, [b"RDTA\r\n", b"STQA000000\r\n", b"STRA000000\r\n"])
        self.assertFalse(fake.is_open)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_discovery_prefers_matching_cp210x_serial_then_falls_back_only_when_absent(self):
        preferred = "E42E432326A3ED118B3D99412981D5C7"
        ports = [
            SimpleNamespace(device="COM5", vid=0x10C4, pid=0xEA60, serial_number=preferred),
            SimpleNamespace(device="COM6", vid=0x10C4, pid=0xEA60, serial_number="other"),
        ]
        fake = FakeGainSerial(gain_startup_replies())
        driver = GainDriver(
            serial_factory=lambda **kwargs: fake,
            ports_provider=lambda: ports,
            start_watchdog=False,
        ).connect()
        self.assertEqual(driver.port, "COM5")
        driver.close()

        fallback = FakeGainSerial(gain_startup_replies())
        driver = GainDriver(
            serial_factory=lambda **kwargs: fallback,
            ports_provider=lambda: [ports[1]],
            start_watchdog=False,
        ).connect()
        self.assertEqual(driver.port, "COM6")
        driver.close()

    def test_rejects_short_writes_and_malformed_typed_values(self):
        short = FakeGainSerial(gain_startup_replies(), write_results=[3])
        with self.assertRaises(InstrumentConnectionError):
            self.make_driver(short).connect()
        self.assertFalse(short.is_open)

        for reply in (b"READY;R=true\r\n", b"READY;R=2\r\n"):
            with self.subTest(reply=reply):
                fake = FakeGainSerial(gain_startup_replies()[:2] + [reply])
                with self.assertRaises(InstrumentProtocolError):
                    self.make_driver(fake).connect()

    def test_connect_rejects_out_of_bound_or_nonfinite_target_and_current_then_safely_closes(self):
        cases = (
            ("target", [b"READY;T=21.000\r\n"], b"READY;E=14.999\r\n", gain_startup_replies()[2:], b"RDEA\r\n"),
            ("target", [b"READY;T=21.000\r\n"], b"READY;E=40.001\r\n", gain_startup_replies()[2:], b"RDEA\r\n"),
            ("target", [b"READY;T=21.000\r\n"], b"READY;E=nan\r\n", gain_startup_replies()[2:], b"RDEA\r\n"),
            ("current", gain_startup_replies()[:3], b"READY;C=-0.001\r\n", gain_startup_replies()[4:], b"RDCA\r\n"),
            ("current", gain_startup_replies()[:3], b"READY;C=200.001\r\n", gain_startup_replies()[4:], b"RDCA\r\n"),
            ("current", gain_startup_replies()[:3], b"READY;C=inf\r\n", gain_startup_replies()[4:], b"RDCA\r\n"),
        )
        for name, prefix, invalid, suffix, read_command in cases:
            with self.subTest(field=name, reply=invalid):
                fake = FakeGainSerial(prefix + [invalid] + suffix)
                driver = self.make_driver(fake)
                try:
                    with self.assertRaises(InstrumentProtocolError):
                        driver.connect()
                    self.assertIsNone(driver.status)
                    self.assertEqual(
                        fake.writes[-3:],
                        [read_command, b"STQA000000\r\n", b"STRA000000\r\n"],
                    )
                    self.assertFalse(fake.is_open)
                finally:
                    driver.close()

    def test_device_target_and_current_reply_bounds_preserve_confirmed_status(self):
        cases = (
            ("read target", "target", b"READY;E=14.999\r\n", lambda driver: driver.read_target()),
            ("read target", "target", b"READY;E=40.001\r\n", lambda driver: driver.read_target()),
            ("read target", "target", b"READY;E=nan\r\n", lambda driver: driver.read_target()),
            ("set target", "target", b"READY;E=14.999\r\n", lambda driver: driver.set_temperature(20.0)),
            ("set target", "target", b"READY;E=40.001\r\n", lambda driver: driver.set_temperature(20.0)),
            ("set target", "target", b"READY;E=-inf\r\n", lambda driver: driver.set_temperature(20.0)),
            ("read current", "current", b"READY;C=-0.001\r\n", lambda driver: driver.read_current()),
            ("read current", "current", b"READY;C=200.001\r\n", lambda driver: driver.read_current()),
            ("read current", "current", b"READY;C=inf\r\n", lambda driver: driver.read_current()),
            ("set current", "current", b"READY;C=-0.001\r\n", lambda driver: driver.set_current(100.0)),
            ("set current", "current", b"READY;C=200.001\r\n", lambda driver: driver.set_current(100.0)),
            ("set current", "current", b"READY;C=nan\r\n", lambda driver: driver.set_current(100.0)),
        )
        for operation, field, invalid, call in cases:
            with self.subTest(operation=operation, reply=invalid):
                fake = FakeGainSerial(gain_startup_replies() + [invalid])
                driver = self.make_driver(fake).connect()
                confirmed = driver.status

                with self.assertRaises(InstrumentProtocolError):
                    call(driver)

                self.assertIs(driver.status, confirmed)
                self.assertEqual(
                    driver.status.target_c if field == "target" else driver.status.current_ma,
                    22.0 if field == "target" else 150.0,
                )
                driver.close()

    def test_device_pid_reply_must_stay_inside_six_digit_protocol_range(self):
        cases = (
            (b"READY;P=-0.001\r\n", b"READY;I=1.000\r\n", b"READY;D=1.000\r\n"),
            (b"READY;P=1.000\r\n", b"READY;I=999.9999\r\n", b"READY;D=1.000\r\n"),
            (b"READY;P=1.000\r\n", b"READY;I=1.000\r\n", b"READY;D=nan\r\n"),
        )
        for replies in cases:
            with self.subTest(replies=replies):
                fake = FakeGainSerial(gain_startup_replies() + list(replies))
                driver = self.make_driver(fake).connect()
                with self.assertRaises(InstrumentProtocolError):
                    driver.read_pid()
                driver.close()
        for reply in (b"READY;T=nan\r\n", b"READY;T=inf\r\n"):
            with self.subTest(reply=reply):
                fake = FakeGainSerial([reply])
                with self.assertRaises(InstrumentProtocolError):
                    self.make_driver(fake).connect()

    def test_typed_setters_refresh_status_and_pid_update_is_atomic(self):
        fake = FakeGainSerial(gain_startup_replies() + [
            b"READY;E=23.500\r\n", b"READY;C=100.250\r\n", b"READY;D=0\r\n",
            b"READY;P=1.250\r\n", b"READY;I=2.500\r\n", b"READY;D=3.750\r\n",
        ])
        driver = self.make_driver(fake).connect()
        first_status = driver.status

        driver.set_temperature(23.5)
        self.assertIsNot(driver.status, first_status)
        self.assertEqual(driver.status.target_c, 23.5)
        driver.set_current(100.25)
        self.assertEqual(driver.status.current_ma, 100.25)
        driver.disable_tec()
        self.assertFalse(driver.status.tec_enabled)
        self.assertEqual(driver.set_pid(1.25, 2.5, 3.75), (1.25, 2.5, 3.75))
        self.assertEqual(
            fake.writes[5:],
            [
                b"STEA023500\r\n", b"STCA100250\r\n", b"STRA000000\r\n",
                b"STPA001250\r\n", b"STIA002500\r\n", b"STDA003750\r\n",
            ],
        )
        driver.close()

    def test_pid_reset_clear_and_public_calls_require_ready_state(self):
        fake = FakeGainSerial(gain_startup_replies() + [
            b"READY\r\n", b"READY;P=1.000\r\n", b"READY;I=2.000\r\n", b"READY;D=3.000\r\n", b"READY\r\n",
        ])
        driver = self.make_driver(fake).connect()
        self.assertEqual(driver.reset_pid(), (1.0, 2.0, 3.0))
        driver.clear_integral()
        self.assertEqual(fake.writes[5:], [b"RST\r\n", b"RDPA\r\n", b"RDIA\r\n", b"RDDA\r\n", b"CLR\r\n"])
        driver.close()

        disconnected = self.make_driver(FakeGainSerial())
        for call in (
            disconnected.read_status, disconnected.read_temperature, disconnected.read_target,
            disconnected.read_tec_enabled, disconnected.read_current, disconnected.read_current_enabled,
            lambda: disconnected.set_temperature(20.0), lambda: disconnected.set_current(1.0),
            disconnected.enable_tec, disconnected.disable_tec, disconnected.disable_current,
            disconnected.read_pid, lambda: disconnected.set_pid(0.0, 0.0, 0.0),
            disconnected.reset_pid, disconnected.clear_integral,
        ):
            with self.subTest(call=call):
                with self.assertRaises(InstrumentSafetyError):
                    call()


def ready_gain_driver(*, target=22.0, tec_enabled=True, current_enabled=False, **kwargs):
    fake = FakeGainSerial([
        f"READY;T={target:.3f}\r\n".encode("ascii"),
        f"READY;E={target:.3f}\r\n".encode("ascii"),
        f"READY;R={int(tec_enabled)}\r\n".encode("ascii"),
        b"READY;C=150.000\r\n",
        f"READY;Q={int(current_enabled)}\r\n".encode("ascii"),
    ])
    return GainDriver(
        port="COM5",
        serial_factory=lambda **serial_kwargs: fake,
        **kwargs,
    ).connect(), fake


class GainSafetyTests(unittest.TestCase):
    def test_five_samples_covering_four_seconds_cannot_enable(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(start_watchdog=False, clock=clock)
        try:
            fake.queue_temperature(*([22.0] * 5))
            for index in range(5):
                if index:
                    clock.advance(1.0)
                driver._watchdog_iteration()
            fake.queue(b'READY;R=1\r\n', b'READY;C=150.000\r\n',
                       b'READY;T=22.000\r\n', b'READY;Q=1\r\n',
                       b'READY;C=3.000\r\n')
            with self.assertRaises(InstrumentSafetyError):
                driver.enable_current()
            self.assertNotIn(b'STQA000001\r\n', fake.writes)
        finally:
            fake.replies.clear()
            driver.close()

    def test_six_samples_spanning_4_999_seconds_are_not_stable(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(start_watchdog=False, clock=clock)
        try:
            clock.advance(4.999)
            with driver._watchdog_condition:
                driver._stable_samples = 6
                driver._stable_sample_times = [0.0, 1.0, 2.0, 3.0, 4.0, 4.999]
                driver._latest_temperature_at = 4.999
                driver._last_stable_sample_at = 4.999
                self.assertFalse(driver._stability_ready_locked(clock()))
        finally:
            fake.replies.clear()
            driver.close()

    def test_five_samples_spanning_more_than_five_seconds_cannot_enable(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(start_watchdog=False, clock=clock)
        try:
            fake.queue_temperature(*([22.0] * 5))
            for index in range(5):
                if index:
                    clock.advance(1.5)
                driver._watchdog_iteration()
            fake.queue(
                b"READY;R=1\r\n", b"READY;C=150.000\r\n",
                b"READY;T=22.000\r\n", b"READY;Q=1\r\n",
                b"READY;C=3.000\r\n",
            )
            with self.assertRaises(InstrumentSafetyError):
                driver.enable_current()
            self.assertNotIn(b"STQA000001\r\n", fake.writes)
        finally:
            fake.replies.clear()
            driver.close()

    def test_enable_current_requires_ready_tec_latest_temperature_and_six_samples(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(start_watchdog=False, clock=clock)
        fake.queue_temperature(22.05, 22.05, 22.05, 22.05)
        for _ in range(4):
            driver._watchdog_iteration()
            clock.advance(1.0)
        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()

        fake.queue_temperature(22.05, 22.05)
        for index in range(2):
            if index:
                clock.advance(1.0)
            driver._watchdog_iteration()
        fake.queue(
            b"READY;R=1\r\n",
            b"READY;C=150.000\r\n",
            b"READY;T=22.050\r\n",
            b"READY;Q=1\r\n",
            b"READY;C=3.000\r\n",
        )
        self.assertTrue(driver.enable_current())
        self.assertEqual(fake.writes[-2:], [b"STQA000001\r\n", b"RDCA\r\n"])
        driver.close()

    def test_enable_current_rejects_active_state_and_temperature_outside_exact_boundary(self):
        driver, fake = ready_gain_driver(start_watchdog=False)
        driver._latest_temperature = 22.200001
        driver._stable_samples = 6
        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()
        driver._latest_temperature = 22.2
        driver.state = DriverState.ACTIVE
        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()
        self.assertNotIn(b"STQA000001\r\n", fake.writes)
        driver.state = DriverState.READY
        driver.close()

    def test_watchdog_counts_exact_stable_and_moderate_boundaries(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(
            start_watchdog=False,
            current_enabled=True,
            clock=clock,
        )
        fake.queue_temperature(22.2, 23.0, 23.001, 23.001, 23.001)
        fake.queue(b"READY;Q=0\r\n")

        driver._watchdog_iteration()
        self.assertEqual(driver._stable_samples, 1)
        self.assertEqual(driver._moderate_deviation_samples, 0)
        clock.advance(1.0)
        driver._watchdog_iteration()
        self.assertEqual(driver._stable_samples, 0)
        self.assertEqual(driver._moderate_deviation_samples, 0)
        clock.advance(1.0)
        driver._watchdog_iteration()
        clock.advance(1.0)
        driver._watchdog_iteration()
        self.assertEqual(driver._moderate_deviation_samples, 2)
        clock.advance(1.0)
        driver._watchdog_iteration()

        self.assertEqual(driver._moderate_deviation_samples, 3)
        self.assertEqual(driver.state, DriverState.READY)
        self.assertFalse(driver.status.current_enabled)
        self.assertTrue(driver.status.tec_enabled)
        self.assertEqual(fake.writes[-1], b"STQA000000\r\n")
        driver.close()

    def test_watchdog_trips_only_above_three_degrees_and_shuts_down_in_order(self):
        driver, fake = ready_gain_driver(start_watchdog=False, current_enabled=True)
        fake.queue_temperature(25.0, 25.001)
        fake.queue(b"READY;Q=0\r\n", b"READY;D=0\r\n")

        driver._watchdog_iteration()
        self.assertEqual(driver.state, DriverState.ACTIVE)
        driver._watchdog_iteration()

        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertTrue(driver._stop_watchdog.is_set())
        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
        self.assertIsInstance(driver.fault_error, DeviceFault)
        driver.close()

    def test_successful_temperature_read_resets_consecutive_failure_count(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(start_watchdog=False, clock=clock)
        fake.queue(b"", b"READY;T=22.000\r\n", b"", b"")

        driver._watchdog_iteration()
        self.assertEqual(driver._read_failures, 1)
        clock.advance(1.0)
        driver._watchdog_iteration()
        self.assertEqual(driver._read_failures, 0)
        clock.advance(1.0)
        driver._watchdog_iteration()
        clock.advance(1.0)
        driver._watchdog_iteration()

        self.assertEqual(driver._read_failures, 2)
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_three_read_failures_trip_and_preserve_original_fault_when_current_shutdown_reply_fails(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(
            start_watchdog=False,
            current_enabled=True,
            clock=clock,
        )
        original = InstrumentProtocolError("watchdog read failed")
        fake.queue(b"", b"", b"", b"ERROR\r\n", b"READY;D=0\r\n")

        driver._watchdog_iteration()
        clock.advance(1.0)
        driver._watchdog_iteration()
        clock.advance(1.0)
        driver._watchdog_iteration()

        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(driver._read_failures, 3)
        self.assertIsInstance(driver.fault_error, InstrumentProtocolError)
        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
        self.assertTrue(driver._stop_watchdog.is_set())
        driver.close()

    def test_wait_stable_returns_after_six_samples_and_times_out_with_typed_error(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(start_watchdog=False, clock=clock)
        self.assertEqual(driver.status.received_at, 0.0)
        with self.assertRaises(InstrumentTimeoutError):
            driver.wait_stable(timeout=0.0)
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        self.assertIsNone(driver.wait_stable(timeout=0.0))
        driver.close()

    def test_connect_starts_daemon_watchdog_only_when_requested_and_close_stops_it(self):
        driver, fake = ready_gain_driver(start_watchdog=True, poll_interval=1.0)
        thread = driver._watchdog_thread
        self.assertIsNotNone(thread)
        self.assertTrue(thread.daemon)
        self.assertTrue(thread.is_alive())
        driver.close()
        thread.join(timeout=1.0)
        self.assertFalse(thread.is_alive())

        no_thread, _ = ready_gain_driver(start_watchdog=False)
        self.assertIsNone(no_thread._watchdog_thread)
        no_thread.close()


class GainSafetyReviewTests(unittest.TestCase):
    def _stable_driver(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(
            start_watchdog=False,
            clock=clock,
            poll_interval=1.0,
        )
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        return driver, fake, clock

    def test_fast_polls_do_not_create_stable_samples(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(
            start_watchdog=False,
            clock=clock,
            poll_interval=1.0,
        )
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(5):
            driver._watchdog_iteration()
            clock.advance(0.1)

        self.assertEqual(driver._stable_samples, 1)
        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()
        driver.close()

    def test_set_temperature_success_resets_stability_epoch(self):
        driver, fake, clock = self._stable_driver()
        self.assertEqual(driver._stable_samples, 6)
        fake.queue(b"READY;E=23.000\r\n")

        driver.set_temperature(23.0)

        self.assertEqual(driver._stable_samples, 0)
        with self.assertRaises(InstrumentTimeoutError):
            driver.wait_stable(timeout=0.0)
        driver.close()

    def test_tec_transitions_reset_stability_and_off_samples_do_not_accumulate(self):
        driver, fake, clock = self._stable_driver()
        fake.queue(b"READY;D=0\r\n")

        driver.disable_tec()
        self.assertEqual(driver._stable_samples, 0)
        fake.queue_temperature(22.0)
        clock.advance(2.0)
        driver._watchdog_iteration()
        self.assertEqual(driver._stable_samples, 0)
        fake.queue(b"READY;D=1\r\n")
        driver.enable_tec()
        self.assertEqual(driver._stable_samples, 0)
        driver.close()

    def test_enable_current_rechecks_d_c_t_then_enters_active_without_duplicate_write(self):
        driver, fake, clock = self._stable_driver()
        fake.queue(
            b"READY;R=1\r\n",
            b"READY;C=150.000\r\n",
            b"READY;T=22.000\r\n",
            b"READY;Q=1\r\n",
            b"READY;C=3.000\r\n",
        )

        self.assertTrue(driver.enable_current())
        self.assertEqual(driver.state, DriverState.ACTIVE)
        self.assertEqual(
            fake.writes[-5:],
            [b"RDRA\r\n", b"RDCA\r\n", b"RDTA\r\n", b"STQA000001\r\n", b"RDCA\r\n"],
        )
        writes_after_enable = len(fake.writes)
        self.assertTrue(driver.enable_current())
        self.assertEqual(len(fake.writes), writes_after_enable)
        driver.close()

    def test_enable_current_reads_device_reset_current_after_output_turns_on(self):
        """Catches caching the pre-enable setpoint when Q=1 resets hardware to 3 mA."""
        driver, fake, clock = self._stable_driver()
        fake.queue(
            b"READY;R=1\r\n",
            b"READY;C=80.000\r\n",
            b"READY;T=22.000\r\n",
            b"READY;Q=1\r\n",
            b"READY;C=3.000\r\n",
        )

        self.assertTrue(driver.enable_current())

        self.assertEqual(driver.status.current_ma, 3.0)
        self.assertEqual(
            fake.writes[-5:],
            [b"RDRA\r\n", b"RDCA\r\n", b"RDTA\r\n", b"STQA000001\r\n", b"RDCA\r\n"],
        )
        driver.close()

    def test_ramp_current_waits_before_each_one_ma_step_and_rechecks_final_state(self):
        """Catches abrupt current jumps or accepting an unverified final setpoint."""
        clock = FakeClock()
        driver, fake = ready_gain_driver(
            start_watchdog=False,
            clock=clock,
            sleep=clock.advance,
        )
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        fake.queue(
            b"READY;R=1\r\n",
            b"READY;C=80.000\r\n",
            b"READY;T=22.000\r\n",
            b"READY;Q=1\r\n",
            b"READY;C=3.000\r\n",
        )
        driver.enable_current()
        ramp_started = clock()
        fake.queue(
            b"READY;C=4.000\r\n",
            b"READY;C=5.000\r\n",
            b"READY;C=5.000\r\n",
            b"READY;Q=1\r\n",
            b"READY;R=1\r\n",
            b"READY;T=22.000\r\n",
        )

        applied = driver.ramp_current(5.0, step_ma=1.0, interval_s=0.1)

        self.assertEqual(applied, 5.0)
        self.assertAlmostEqual(clock() - ramp_started, 0.2)
        self.assertEqual(
            [write for write in fake.writes if write.startswith(b"STCA")][-2:],
            [b"STCA004000\r\n", b"STCA005000\r\n"],
        )
        self.assertEqual(
            fake.writes[-4:],
            [b"RDCA\r\n", b"RDQA\r\n", b"RDRA\r\n", b"RDTA\r\n"],
        )
        self.assertTrue(driver.status.current_enabled)
        self.assertTrue(driver.status.tec_enabled)
        self.assertEqual(driver.status.temperature_c, 22.0)
        driver.close()

    def test_stale_stability_evidence_cannot_enable_current(self):
        driver, fake, clock = self._stable_driver()
        clock.advance(10.0)

        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()

        self.assertNotIn(b"STQA000001\r\n", fake.writes)
        driver.close()

    def test_failed_temperature_read_resets_the_entire_stability_sequence(self):
        driver, fake, clock = self._stable_driver()
        fake.queue(b"")

        driver._watchdog_iteration()

        self.assertEqual(driver._stable_samples, 0)
        self.assertEqual(driver._stable_sample_times, [])
        self.assertIsNone(driver._last_stable_sample_at)
        driver.close()

    def test_connect_with_confirmed_current_on_is_active(self):
        driver, fake = ready_gain_driver(start_watchdog=False, current_enabled=True)
        self.assertEqual(driver.state, DriverState.ACTIVE)
        driver.close()

    def test_close_notifies_wait_stable_while_publishing_closing(self):
        driver, fake = ready_gain_driver(start_watchdog=False)
        entered_wait = threading.Event()
        original_wait = driver._watchdog_condition.wait

        def observed_wait(timeout=None):
            entered_wait.set()
            return original_wait(timeout)

        driver._watchdog_condition.wait = observed_wait
        result = {}

        def wait_for_stability():
            try:
                driver.wait_stable(timeout=60.0)
            except BaseException as error:
                result["error"] = error

        waiter = threading.Thread(target=wait_for_stability)
        waiter.start()
        self.assertTrue(entered_wait.wait(timeout=1.0))
        try:
            driver.close()
            waiter.join(timeout=1.0)
            self.assertFalse(waiter.is_alive())
            self.assertIsInstance(result.get("error"), InstrumentSafetyError)
        finally:
            with driver._watchdog_condition:
                driver._watchdog_condition.notify_all()
            waiter.join(timeout=1.0)

    def test_close_waits_for_watchdog_or_faults_without_resurrecting_it(self):
        driver, fake = ready_gain_driver(start_watchdog=False, io_timeout=0.001, poll_interval=1.0)
        stop_event = GateStopEvent()
        driver._stop_watchdog = stop_event
        driver._start_watchdog()
        watchdog = driver._watchdog_thread
        self.assertTrue(stop_event.entered_wait.wait(timeout=1.0))
        result = {}

        def close_driver():
            try:
                driver.close()
            except BaseException as error:
                result["error"] = error

        closer = threading.Thread(target=close_driver)
        closer.start()
        self.assertTrue(stop_event.set_called.wait(timeout=1.0))
        try:
            closer.join(timeout=0.05)
            self.assertTrue(closer.is_alive())
            stop_event.release.set()
            closer.join(timeout=2.0)
            self.assertFalse(closer.is_alive())
            self.assertFalse(watchdog.is_alive())
            self.assertIsNone(driver._watchdog_thread)
        finally:
            stop_event.release.set()
            closer.join(timeout=2.0)

    def test_close_reports_typed_fault_and_blocks_reconnect_if_watchdog_misses_bound(self):
        driver, fake = ready_gain_driver(start_watchdog=False, io_timeout=0.001, poll_interval=1.0)
        stop_event = GateStopEvent()
        driver._stop_watchdog = stop_event
        driver._start_watchdog()
        self.assertTrue(stop_event.entered_wait.wait(timeout=1.0))
        try:
            with self.assertRaises(DeviceFault):
                driver.close()
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertTrue(driver._watchdog_thread.is_alive())
            with self.assertRaises(DeviceFault):
                driver.connect()
        finally:
            stop_event.release.set()
            driver._watchdog_thread.join(timeout=1.0)
        self.assertFalse(driver._watchdog_thread.is_alive())
        driver.close()


class GainSafetyReview2Tests(unittest.TestCase):
    def _driver_with_clock(self, *, current_enabled=False):
        clock = FakeClock()
        driver, fake = ready_gain_driver(
            start_watchdog=False,
            clock=clock,
            poll_interval=1.0,
            current_enabled=current_enabled,
        )
        return driver, fake, clock

    def _gated_stable_driver(self):
        clock = FakeClock()
        serial_port = GatedEnableSerial()
        driver = GainDriver(
            port="COM5",
            serial_factory=lambda **kwargs: serial_port,
            start_watchdog=False,
            clock=clock,
            poll_interval=1.0,
        ).connect()
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        return driver, serial_port, clock

    def test_sparse_ten_second_temperature_samples_restart_stability_sequence(self):
        driver, fake, clock = self._driver_with_clock()
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(5):
            driver._watchdog_iteration()
            clock.advance(10.0)

        self.assertEqual(driver._stable_samples, 1)
        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()
        driver.close()

    def test_rapid_and_sparse_moderate_samples_never_trip_current_off(self):
        driver, fake, clock = self._driver_with_clock(current_enabled=True)
        fake.queue_temperature(23.2, 23.2, 23.2, 23.2)
        driver._watchdog_iteration()
        driver._watchdog_iteration()
        driver._watchdog_iteration()
        self.assertEqual(driver._moderate_deviation_samples, 1)
        self.assertTrue(driver.status.current_enabled)
        clock.advance(10.0)
        driver._watchdog_iteration()
        self.assertEqual(driver._moderate_deviation_samples, 1)
        self.assertTrue(driver.status.current_enabled)
        self.assertNotIn(b"STQA000000\r\n", fake.writes)
        driver.close()

    def test_manual_rapid_read_failures_are_three_watchdog_attempts_and_trip_fault(self):
        driver, fake, clock = self._driver_with_clock()
        fake.queue(b"", b"", b"", b"READY;Q=0\r\n", b"READY;D=0\r\n")
        driver._watchdog_iteration()
        driver._watchdog_iteration()
        driver._watchdog_iteration()
        self.assertEqual(driver._read_failures, 3)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_close_published_during_enable_recheck_prevents_current_enable(self):
        driver, serial_port, clock = self._gated_stable_driver()
        serial_port.gate_enable_temperature = True
        result = {}

        def enable():
            try:
                driver.enable_current()
            except BaseException as error:
                result["error"] = error

        enabler = threading.Thread(target=enable)
        enabler.start()
        self.assertTrue(serial_port.enable_temperature_entered.wait(timeout=1.0))
        closer = threading.Thread(target=driver.close)
        closer.start()
        self.assertTrue(driver._stop_watchdog.wait(timeout=1.0))
        try:
            serial_port.release_enable_temperature.set()
            enabler.join(timeout=1.0)
            closer.join(timeout=1.0)
            self.assertFalse(enabler.is_alive())
            self.assertFalse(closer.is_alive())
            self.assertIsInstance(result.get("error"), InstrumentSafetyError)
            self.assertNotIn(b"STQA000001\r\n", serial_port.writes)
        finally:
            serial_port.release_enable_temperature.set()
            enabler.join(timeout=1.0)
            closer.join(timeout=1.0)

    def test_severe_fault_published_during_enable_recheck_prevents_current_enable(self):
        driver, serial_port, clock = self._gated_stable_driver()
        serial_port.gate_enable_temperature = True
        result = {}

        def enable():
            try:
                driver.enable_current()
            except BaseException as error:
                result["error"] = error

        enabler = threading.Thread(target=enable)
        enabler.start()
        self.assertTrue(serial_port.enable_temperature_entered.wait(timeout=1.0))
        tripper = threading.Thread(target=lambda: driver._severe_trip(DeviceFault("test trip")))
        tripper.start()
        self.assertTrue(driver._stop_watchdog.wait(timeout=1.0))
        try:
            serial_port.release_enable_temperature.set()
            enabler.join(timeout=1.0)
            tripper.join(timeout=1.0)
            self.assertFalse(enabler.is_alive())
            self.assertFalse(tripper.is_alive())
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsInstance(result.get("error"), InstrumentSafetyError)
            self.assertNotIn(b"STQA000001\r\n", serial_port.writes)
            self.assertEqual(serial_port.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
        finally:
            serial_port.release_enable_temperature.set()
            enabler.join(timeout=1.0)
            tripper.join(timeout=1.0)


class GainSafetyReview3Tests(unittest.TestCase):
    def _driver(self, *, io_timeout=1.0, current_enabled=False):
        clock = FakeClock()
        driver, fake = ready_gain_driver(
            start_watchdog=False,
            io_timeout=io_timeout,
            poll_interval=1.0,
            current_enabled=current_enabled,
            clock=clock,
        )
        return driver, fake, clock

    def test_poll_interval_rejects_fast_nonfinite_and_boolean_values(self):
        for value in (0.0, 0.999, float("nan"), float("inf"), True):
            with self.subTest(value=value):
                with self.assertRaises(ValueError):
                    GainDriver(port="COM5", poll_interval=value)

    def test_large_io_timeout_cannot_make_sparse_samples_stable(self):
        driver, fake, clock = self._driver(io_timeout=10.0)
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(5):
            driver._watchdog_iteration()
            clock.advance(10.0)

        self.assertEqual(driver._stable_samples, 1)
        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()
        driver.close()

    def test_large_io_timeout_cannot_make_sparse_moderate_samples_consecutive(self):
        driver, fake, clock = self._driver(io_timeout=10.0, current_enabled=True)
        fake.queue_temperature(23.2, 23.2, 23.2, 23.2)
        for _ in range(4):
            driver._watchdog_iteration()
            clock.advance(2.001)

        self.assertEqual(driver._moderate_deviation_samples, 1)
        self.assertTrue(driver.status.current_enabled)
        self.assertNotIn(b"STQA000000\r\n", fake.writes)
        driver.close()

    def test_failures_separated_by_more_than_two_seconds_still_trip_on_third_attempt(self):
        driver, fake, clock = self._driver()
        fake.queue(b"", b"", b"", b"READY;Q=0\r\n", b"READY;D=0\r\n")
        driver._watchdog_iteration()
        clock.advance(2.001)
        driver._watchdog_iteration()
        clock.advance(2.001)
        driver._watchdog_iteration()

        self.assertEqual(driver._read_failures, 3)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()


class GainSafetyReview4Tests(unittest.TestCase):
    def test_old_stability_epoch_cannot_be_revived_by_fresh_enable_reads_with_large_io_timeout(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(
            start_watchdog=False,
            io_timeout=10.0,
            poll_interval=1.0,
            clock=clock,
        )
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        clock.advance(9.0)
        fake.queue(
            b"READY;R=1\r\n",
            b"READY;C=150.000\r\n",
            b"READY;T=22.000\r\n",
            b"READY;Q=1\r\n",
        )

        with self.assertRaises(InstrumentSafetyError):
            driver.enable_current()

        self.assertNotIn(b"STQA000001\r\n", fake.writes)
        driver.close()

    def test_poll_interval_accepts_only_exactly_one_second(self):
        self.assertEqual(GainDriver(port="COM5", poll_interval=1).poll_interval, 1.0)
        self.assertEqual(GainDriver(port="COM5", poll_interval=1.0).poll_interval, 1.0)
        for value in (1.001, 60.0):
            with self.subTest(value=value):
                with self.assertRaises(ValueError):
                    GainDriver(port="COM5", poll_interval=value)


class GainCleanupTests(unittest.TestCase):
    def test_close_is_idempotent_and_clears_resources_only_after_confirmed_shutdown(self):
        driver, fake = ready_gain_driver(
            start_watchdog=False,
        )
        fake.queue(b"READY;Q=0\r\n", b"READY;D=0\r\n")

        driver.close()
        writes_after_first_close = list(fake.writes)
        driver.close()

        self.assertEqual(
            writes_after_first_close[-2:],
            [b"STQA000000\r\n", b"STRA000000\r\n"],
        )
        self.assertEqual(fake.writes, writes_after_first_close)
        self.assertFalse(fake.is_open)
        self.assertTrue(driver._stop_watchdog.is_set())
        self.assertIsNone(driver._serial)
        self.assertIsNone(driver.status)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_close_continues_after_base_exception_and_retains_typed_fault_evidence(self):
        class InterruptingSerial(FakeGainSerial):
            def write(self, data):
                if bytes(data) == b"STQA000000\r\n":
                    self.writes.append(bytes(data))
                    raise KeyboardInterrupt("current shutdown interrupted")
                return super().write(data)

        fake = InterruptingSerial(gain_startup_replies() + [b"READY;D=0\r\n"])
        driver = GainDriver(
            port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False
        ).connect()

        with self.assertRaisesRegex(KeyboardInterrupt, "current shutdown interrupted"):
            driver.close()

        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
        self.assertFalse(fake.is_open)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIsInstance(driver.cleanup_error, KeyboardInterrupt)
        self.assertIs(driver._serial, fake)
        self.assertIsNotNone(driver.status)

    def test_context_body_exception_remains_primary_when_cleanup_interrupts(self):
        class InterruptingSerial(FakeGainSerial):
            def write(self, data):
                if bytes(data) == b"STQA000000\r\n":
                    self.writes.append(bytes(data))
                    raise SystemExit("cleanup exit")
                return super().write(data)

        fake = InterruptingSerial(gain_startup_replies() + [b"READY;D=0\r\n"])
        driver = GainDriver(
            port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False
        )

        with self.assertRaisesRegex(RuntimeError, "body failure"):
            with driver:
                raise RuntimeError("body failure")

        self.assertFalse(fake.is_open)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIsInstance(driver.cleanup_error, SystemExit)

    def test_self_close_and_watchdog_timeout_raise_typed_fault_without_false_disconnect(self):
        driver, fake = ready_gain_driver(start_watchdog=False)
        fake.queue(b"READY;Q=0\r\n", b"READY;D=0\r\n")
        driver._watchdog_thread = threading.current_thread()

        with self.assertRaises(DeviceFault):
            driver.close()

        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, fake)
        self.assertIsInstance(driver.cleanup_error, DeviceFault)

        class StubbornWatchdog:
            def __init__(self):
                self.join_timeouts = []

            def is_alive(self):
                return True

            def join(self, timeout=None):
                self.join_timeouts.append(timeout)

        driver, fake = ready_gain_driver(start_watchdog=False, io_timeout=0.001)
        fake.queue(b"READY;Q=0\r\n", b"READY;D=0\r\n")
        watchdog = StubbornWatchdog()
        driver._watchdog_thread = watchdog

        with self.assertRaises(DeviceFault):
            driver.close()

        self.assertEqual(watchdog.join_timeouts, [2.001])
        self.assertFalse(fake.is_open)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, fake)
        self.assertIsInstance(driver.cleanup_error, DeviceFault)

    def test_utils_exports_gain_driver_and_status(self):
        from Code.Utils import GainDriver as ExportedGainDriver
        from Code.Utils import GainStatus as ExportedGainStatus

        self.assertIs(ExportedGainDriver, GainDriver)
        self.assertIs(ExportedGainStatus, GainStatus)


class GainDiagnosticTests(unittest.TestCase):
    def test_default_diagnostic_replays_confirmed_values_without_reset_or_enable(self):
        from Code.Debugs import check_gain

        events = []

        class DiagnosticDriver:
            status = GainStatus(22.0, 22.0, True, 150.0, False, 1.0)

            def __enter__(self):
                events.append("enter")
                return self

            def __exit__(self, *args):
                events.append("exit")
                return False

            def read_status(self):
                events.append("read_status")
                return self.status

            def read_pid(self):
                events.append("read_pid")
                return (1.0, 2.0, 3.0)

            def set_temperature(self, value):
                events.append(("set_temperature", value))
                return value

            def set_current(self, value):
                events.append(("set_current", value))
                return value

            def set_pid(self, *values):
                events.append(("set_pid", values))
                return values

            def read_target(self):
                events.append("fresh_target")
                return self.status.target_c

            def read_current(self):
                events.append("fresh_current")
                return self.status.current_ma

            def read_tec_enabled(self):
                events.append("fresh_tec")
                return self.status.tec_enabled

            def read_current_enabled(self):
                events.append("fresh_current_enabled")
                return self.status.current_enabled

            def read_temperature(self):
                events.append("fresh_temperature")
                return self.status.temperature_c

        check_gain._run_check(
            driver_factory=lambda **kwargs: DiagnosticDriver(),
            input_fn=lambda _: "n",
            print_fn=lambda _: None,
        )

        self.assertEqual(
            events,
            [
                "enter", "read_status", "read_pid",
                ("set_temperature", 22.0), ("set_current", 150.0),
                ("set_pid", (1.0, 2.0, 3.0)), "fresh_target", "fresh_current",
                "fresh_tec", "fresh_current_enabled", "fresh_temperature", "read_pid", "exit",
            ],
        )

    def test_optional_output_cleanup_preserves_primary_error_when_current_disable_fails(self):
        from Code.Debugs import check_gain

        events = []

        class DiagnosticDriver:
            status = GainStatus(22.0, 22.0, False, 0.0, False, 1.0)

            def __enter__(self):
                return self

            def __exit__(self, *args):
                events.append("context_close")
                return False

            def read_status(self):
                return self.status

            def read_pid(self):
                return (1.0, 2.0, 3.0)

            def set_temperature(self, value):
                events.append(("set_temperature", value))
                return value

            def set_current(self, value):
                events.append(("set_current", value))
                return value

            def set_pid(self, *values):
                events.append(("set_pid", values))
                return values

            def read_target(self): return self.status.target_c
            def read_current(self): return self.status.current_ma
            def read_tec_enabled(self): return self.status.tec_enabled
            def read_current_enabled(self): return self.status.current_enabled
            def read_temperature(self): return self.status.temperature_c

            def enable_tec(self):
                events.append("enable_tec")

            def wait_stable(self):
                events.append("wait_stable")

            def enable_current(self):
                events.append("enable_current")
                raise RuntimeError("primary enable failure")

            def disable_current(self):
                events.append("disable_current")
                raise DeviceFault("disable failure")

        answers = iter(("y", "10", "0"))
        with self.assertRaisesRegex(RuntimeError, "primary enable failure"):
            check_gain._run_check(
                driver_factory=lambda **kwargs: DiagnosticDriver(),
                input_fn=lambda _: next(answers),
                print_fn=lambda _: None,
                sleep_fn=lambda _: events.append("sleep"),
            )

        self.assertEqual(
            events[-6:],
            ["enable_tec", "wait_stable", ("set_current", 10.0),
             "enable_current", "disable_current", "context_close"],
        )


class GainFinalReviewTests(unittest.TestCase):
    def test_rejects_whitespace_numeric_reply(self):
        for reply in (b"READY;T= 22.000\r\n", b"READY;T=22.000 \r\n"):
            with self.subTest(reply=reply):
                with self.assertRaises(InstrumentProtocolError):
                    GainDriver._float_value(parse_reply(reply, "T"), "T")

    def test_connect_rejects_current_on_with_tec_off_after_safe_shutdown(self):
        fake = FakeGainSerial([
            b"READY;T=22.000\r\n", b"READY;E=22.000\r\n",
            b"READY;R=0\r\n", b"READY;C=150.000\r\n", b"READY;Q=1\r\n",
            b"READY;Q=0\r\n", b"READY;D=0\r\n",
        ], auto_shutdown_replies=False)
        driver = GainDriver(
            port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False
        )

        with self.assertRaises(DeviceFault):
            driver.connect()

        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
        self.assertFalse(fake.is_open)

    def test_disable_tec_disables_current_first_and_faults_if_current_is_uncertain(self):
        driver, fake = ready_gain_driver(start_watchdog=False, current_enabled=True)
        fake.queue(b"READY;Q=0\r\n", b"READY;D=0\r\n")
        driver.disable_tec()
        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

        fake = FakeGainSerial(gain_startup_replies() + [b"ERROR\r\n", b"READY;D=0\r\n"], auto_shutdown_replies=False)
        driver = GainDriver(port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False).connect()
        driver.status = replace(driver.status, current_enabled=True)
        driver.state = DriverState.ACTIVE
        with self.assertRaises(InstrumentProtocolError):
            driver.disable_tec()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])

    def test_enable_current_uncertainty_faults_and_attempts_both_shutdowns(self):
        for name, write_results, reply in (
            ("bad field", None, b"READY;D=1\r\n"),
            ("not enabled", None, b"READY;Q=0\r\n"),
        ):
            with self.subTest(name=name):
                clock = FakeClock()
                driver, fake = ready_gain_driver(start_watchdog=False, clock=clock)
                fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
                for _ in range(6):
                    driver._watchdog_iteration()
                    clock.advance(1.0)
                if write_results is not None:
                    fake.write_results.extend(write_results)
                fake.queue(
            b"READY;R=1\r\n", b"READY;C=150.000\r\n", b"READY;T=22.000\r\n",
                    *(item for item in (reply, b"READY;Q=0\r\n", b"READY;D=0\r\n") if item is not None),
                )
                with self.assertRaises(DriverError):
                    driver.enable_current()
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])

        class ShortEnableSerial(FakeGainSerial):
            def write(self, data):
                if bytes(data) == b"STQA000001\r\n":
                    self.writes.append(bytes(data))
                    return 0
                return super().write(data)

        clock = FakeClock()
        fake = ShortEnableSerial(gain_startup_replies())
        driver = GainDriver(
            port="COM5", serial_factory=lambda **kwargs: fake,
            start_watchdog=False, clock=clock,
        ).connect()
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        fake.queue(
            b"READY;R=1\r\n", b"READY;C=150.000\r\n", b"READY;T=22.000\r\n",
            b"READY;Q=0\r\n", b"READY;D=0\r\n",
        )
        with self.assertRaises(InstrumentConnectionError):
            driver.enable_current()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(
            fake.writes[-3:],
            [b"STQA000001\r\n", b"STQA000000\r\n", b"STRA000000\r\n"],
        )

        class TimeoutEnableSerial(FakeGainSerial):
            def read_until(self, expected=b"\n"):
                if self.writes[-1] == b"STQA000001\r\n":
                    raise TimeoutError("enable reply timeout")
                return super().read_until(expected)

        clock = FakeClock()
        fake = TimeoutEnableSerial(gain_startup_replies())
        driver = GainDriver(
            port="COM5", serial_factory=lambda **kwargs: fake,
            start_watchdog=False, clock=clock,
        ).connect()
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        fake.queue(
            b"READY;R=1\r\n", b"READY;C=150.000\r\n", b"READY;T=22.000\r\n",
            b"READY;Q=0\r\n", b"READY;D=0\r\n",
        )
        with self.assertRaises(InstrumentConnectionError):
            driver.enable_current()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(fake.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])

    def test_read_observations_reset_epoch_sync_state_and_trip_invalid_pair(self):
        driver, fake = ready_gain_driver(start_watchdog=False)
        driver._stable_samples = 6
        driver._stable_sample_times = [0, 1, 2, 3, 4, 5]
        fake.queue(b"READY;E=23.000\r\n")
        driver.read_target()
        self.assertEqual(driver._stable_samples, 0)

        driver._stable_samples = 6
        driver._stable_sample_times = [0, 1, 2, 3, 4, 5]
        fake.queue(b"READY;R=0\r\n")
        driver.read_tec_enabled()
        self.assertEqual(driver._stable_samples, 0)

        fake.queue(b"READY;Q=1\r\n", b"READY;Q=0\r\n", b"READY;D=0\r\n")
        with self.assertRaises(DeviceFault):
            driver.read_current_enabled()
        self.assertEqual(driver.state, DriverState.FAULT)

        driver, fake = ready_gain_driver(start_watchdog=False)
        fake.queue(b"READY;Q=1\r\n", b"READY;Q=0\r\n")
        self.assertTrue(driver.read_current_enabled())
        self.assertEqual(driver.state, DriverState.ACTIVE)
        self.assertFalse(driver.read_current_enabled())
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_watchdog_uses_absolute_one_second_deadlines_and_keeps_six_samples(self):
        clock = FakeClock()
        driver, _ = ready_gain_driver(start_watchdog=False, clock=clock)

        class DeadlineStop:
            def __init__(self):
                self.waits = []
                self.calls = 0

            def wait(self, timeout):
                self.waits.append(timeout)
                clock.advance(timeout)
                return self.calls >= 3

            def is_set(self):
                return self.calls >= 3

            def set(self):
                self.calls = 3

            def clear(self):
                self.calls = 0

        stop = DeadlineStop()
        driver._stop_watchdog = stop
        starts = []

        def iteration():
            starts.append(clock())
            clock.advance(0.6)
            stop.calls += 1

        driver._watchdog_iteration = iteration
        driver._watchdog_loop()
        self.assertEqual(starts, [1.0, 2.0, 3.0])
        self.assertEqual(len(stop.waits), 3)
        for actual, expected in zip(stop.waits, (1.0, 0.4, 0.4), strict=True):
            self.assertAlmostEqual(actual, expected)

        clock = FakeClock()
        driver, fake = ready_gain_driver(start_watchdog=False, clock=clock)
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        self.assertEqual(driver._stable_samples, 6)
        self.assertEqual(driver._stable_sample_times, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0])
        driver.close()

    def test_watchdog_processes_a_gated_temperature_while_holding_the_request_lock(self):
        driver, fake = ready_gain_driver(start_watchdog=False)
        observed_request_ownership = []
        original_notify = driver._watchdog_condition.notify_all

        def observed_notify():
            observed_request_ownership.append(driver._request_lock._is_owned())
            original_notify()

        driver._watchdog_condition.notify_all = observed_notify
        fake.queue_temperature(22.0)
        driver._watchdog_iteration()

        self.assertEqual(observed_request_ownership, [True])
        driver.close()

    def test_six_tenths_second_io_still_trips_moderate_shutdown_on_third_deadline(self):
        clock = FakeClock()

        class DeadlineStop:
            def __init__(self):
                self.set_value = False
                self.waits = []

            def wait(self, timeout):
                self.waits.append(timeout)
                clock.advance(timeout)
                return self.set_value

            def is_set(self): return self.set_value
            def set(self): self.set_value = True
            def clear(self): self.set_value = False

        stop = DeadlineStop()

        class SlowSerial(FakeGainSerial):
            slow_reads = False

            def read_until(self, expected=b"\n"):
                if self.slow_reads and self.writes[-1] == b"RDTA\r\n":
                    clock.advance(0.6)
                return super().read_until(expected)

            def write(self, data):
                result = super().write(data)
                if bytes(data) == b"STQA000000\r\n":
                    stop.set()
                return result

        fake = SlowSerial(gain_startup_replies() + [
            b"READY;T=23.200\r\n", b"READY;T=23.200\r\n", b"READY;T=23.200\r\n",
            b"READY;Q=0\r\n",
        ])
        driver = GainDriver(
            port="COM5", serial_factory=lambda **kwargs: fake,
            start_watchdog=False, clock=clock,
        ).connect()
        driver._stop_watchdog = stop
        fake.slow_reads = True

        driver._watchdog_loop()

        self.assertIn(b"STQA000000\r\n", fake.writes)
        self.assertEqual(driver._moderate_deviation_samples, 3)
        self.assertEqual(len(stop.waits), 3)
        for actual, expected in zip(stop.waits, (1.0, 0.4, 0.4), strict=True):
            self.assertAlmostEqual(actual, expected)
        driver.close()

    def test_partial_connect_cleanup_failure_retains_transport_and_preserves_connect_error(self):
        class CloseFailSerial(FakeGainSerial):
            fail_close = True

            def close(self):
                self.is_open = False
                if self.fail_close:
                    raise OSError("close failure")

        fake = CloseFailSerial([b"READY;E=22.000\r\n", b"READY;Q=0\r\n", b"READY;D=0\r\n"])
        driver = GainDriver(port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False)
        with self.assertRaises(InstrumentProtocolError):
            driver.connect()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, fake)
        self.assertIsInstance(driver.cleanup_error, InstrumentConnectionError)
        fake.fail_close = False
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_discovery_is_repeated_only_for_an_unspecified_port(self):
        ports = [SimpleNamespace(device="COM5", vid=0x10C4, pid=0xEA60, serial_number="x")]
        fakes = [FakeGainSerial(gain_startup_replies()), FakeGainSerial(gain_startup_replies())]
        driver = GainDriver(
            serial_factory=lambda **kwargs: fakes.pop(0), ports_provider=lambda: ports,
            usb_serial=None, start_watchdog=False,
        )
        driver.connect(); driver.close()
        ports[0] = SimpleNamespace(device="COM6", vid=0x10C4, pid=0xEA60, serial_number="x")
        driver.connect()
        self.assertEqual(driver.port, "COM6")
        driver.close()

        explicit = FakeGainSerial(gain_startup_replies())
        seen = []
        explicit_driver = GainDriver(
            port="COM9", usb_serial=None, start_watchdog=False,
            serial_factory=lambda **kwargs: seen.append(kwargs["port"]) or explicit,
            ports_provider=lambda: [SimpleNamespace(device="COM10", vid=0x10C4, pid=0xEA60, serial_number="x")],
        ).connect()
        self.assertEqual(seen, ["COM9"])
        explicit_driver.close()

    def test_diagnostic_requires_applied_and_fresh_readback_matches_before_confirmation(self):
        from Code.Debugs import check_gain

        messages = []

        class MismatchDriver:
            status = GainStatus(22.0, 22.0, True, 150.0, False, 0.0)
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def read_status(self): return self.status
            def read_pid(self): return (1.0, 2.0, 3.0)
            def read_temperature(self): return 22.0
            def read_target(self): return 22.0
            def read_tec_enabled(self): return True
            def read_current(self): return 150.0
            def read_current_enabled(self): return False
            def set_temperature(self, value): return value
            def set_current(self, value): return value
            def set_pid(self, *values): return (9.0, 2.0, 3.0)

        with self.assertRaises(InstrumentProtocolError):
            check_gain._run_check(
                driver_factory=lambda **kwargs: MismatchDriver(),
                input_fn=lambda _: "n", print_fn=messages.append,
            )
        self.assertNotIn("No-op target/current/PID writes confirmed.", messages)

    def test_diagnostic_reports_fresh_temperature_without_requiring_it_to_equal_initial(self):
        from Code.Debugs import check_gain

        messages = []

        class WarmingDriver:
            status = GainStatus(22.0, 22.0, True, 150.0, False, 0.0)
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def read_status(self): return self.status
            def read_pid(self): return (1.0, 2.0, 3.0)
            def read_target(self): return 22.0
            def read_current(self): return 150.0
            def read_tec_enabled(self): return True
            def read_current_enabled(self): return False
            def read_temperature(self): return 22.100
            def set_temperature(self, value): return value
            def set_current(self, value): return value
            def set_pid(self, *values): return values

        check_gain._run_check(
            driver_factory=lambda **kwargs: WarmingDriver(),
            input_fn=lambda _: "n", print_fn=messages.append,
        )
        self.assertIn("No-op target/current/PID writes confirmed.", messages)

    def test_disable_tec_keeps_q0_to_d0_under_one_request_lock_against_enable_race(self):
        clock = FakeClock()
        driver, fake = ready_gain_driver(
            start_watchdog=False, current_enabled=True, clock=clock,
        )
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        fake.queue(b"READY;Q=0\r\n", b"READY;D=0\r\n")
        d_phase_entered = threading.Event()
        release_d_phase = threading.Event()
        enable_started = threading.Event()
        enable_errors = []
        original_set_bool = driver._set_bool

        def gated_set_bool(command, field, enabled, action):
            if command == "STRA" and not enabled:
                d_phase_entered.set()
                release_d_phase.wait(timeout=0.2)
            return original_set_bool(command, field, enabled, action)

        driver._set_bool = gated_set_bool

        def enable():
            enable_started.set()
            try:
                driver.enable_current()
            except BaseException as error:
                enable_errors.append(error)

        disabling = threading.Thread(target=driver.disable_tec)
        enabler = threading.Thread(target=enable)
        disabling.start()
        self.assertTrue(d_phase_entered.wait(timeout=1.0))
        enabler.start()
        self.assertTrue(enable_started.wait(timeout=1.0))
        self.assertNotIn(b"STQA000001\r\n", fake.writes)
        release_d_phase.set()
        disabling.join(timeout=1.0)
        enabler.join(timeout=1.0)
        self.assertFalse(disabling.is_alive())
        self.assertFalse(enabler.is_alive())
        d_index = fake.writes.index(b"STRA000000\r\n")
        self.assertNotIn(b"STQA000001\r\n", fake.writes[:d_index])
        self.assertEqual(fake.writes[d_index - 1], b"STQA000000\r\n")
        self.assertTrue(enable_errors)
        driver.close()

    def test_invariant_failure_in_context_closes_serial_before_propagating_device_fault(self):
        fake = FakeGainSerial([
            b"READY;T=22.000\r\n", b"READY;E=22.000\r\n",
            b"READY;R=0\r\n", b"READY;C=150.000\r\n", b"READY;Q=1\r\n",
            b"READY;Q=0\r\n", b"READY;D=0\r\n",
        ], auto_shutdown_replies=False)
        driver = GainDriver(port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False)
        with self.assertRaises(DeviceFault):
            with driver:
                pass
        self.assertFalse(fake.is_open)
        self.assertIsNone(driver._serial)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_connect_invariant_shutdown_must_confirm_both_outputs_before_transport_close(self):
        cases = (
            ("current error", b"ERROR\r\n", b"READY;D=0\r\n"),
            ("current timeout", b"", b"READY;D=0\r\n"),
            ("current wrong field", b"READY;D=0\r\n", b"READY;D=0\r\n"),
            ("tec error", b"READY;Q=0\r\n", b"ERROR\r\n"),
            ("both errors", b"ERROR\r\n", b"ERROR\r\n"),
        )
        for name, current_reply, tec_reply in cases:
            with self.subTest(name=name):
                fake = FakeGainSerial([
                    b"READY;T=22.000\r\n", b"READY;E=22.000\r\n",
            b"READY;R=0\r\n", b"READY;C=150.000\r\n", b"READY;Q=1\r\n",
                    current_reply, tec_reply,
                ], auto_shutdown_replies=False)
                driver = GainDriver(
                    port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False
                )
                with self.assertRaises(DeviceFault):
                    driver.connect()
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertTrue(fake.is_open)
                self.assertIs(driver._serial, fake)
                self.assertIsNotNone(driver.cleanup_error)

                fake.queue(b"READY;Q=0\r\n", b"READY;D=0\r\n")
                driver.close()
                self.assertEqual(driver.state, DriverState.DISCONNECTED)
                self.assertFalse(fake.is_open)

    def test_connect_invariant_short_current_shutdown_write_retains_transport_until_retry(self):
        class ShortCurrentShutdownSerial(FakeGainSerial):
            short_once = True

            def write(self, data):
                if self.short_once and bytes(data) == b"STQA000000\r\n":
                    self.writes.append(bytes(data))
                    self.short_once = False
                    return 0
                return super().write(data)

        fake = ShortCurrentShutdownSerial([
            b"READY;T=22.000\r\n", b"READY;E=22.000\r\n",
            b"READY;R=0\r\n", b"READY;C=150.000\r\n", b"READY;Q=1\r\n",
            b"READY;D=0\r\n",
        ], auto_shutdown_replies=False)
        driver = GainDriver(port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False)
        with self.assertRaises(DeviceFault):
            driver.connect()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertTrue(fake.is_open)
        self.assertIs(driver._serial, fake)
        self.assertIsNotNone(driver.cleanup_error)
        fake.queue(b"READY;Q=0\r\n", b"READY;D=0\r\n")
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_connect_invariant_failed_explicit_retry_keeps_fault_and_transport(self):
        fake = FakeGainSerial([
            b"READY;T=22.000\r\n", b"READY;E=22.000\r\n",
            b"READY;R=0\r\n", b"READY;C=150.000\r\n", b"READY;Q=1\r\n",
            b"ERROR\r\n", b"READY;D=0\r\n",
        ], auto_shutdown_replies=False)
        driver = GainDriver(port="COM5", serial_factory=lambda **kwargs: fake, start_watchdog=False)
        with self.assertRaises(DeviceFault):
            driver.connect()
        fake.queue(b"ERROR\r\n", b"ERROR\r\n")
        with self.assertRaises(InstrumentProtocolError):
            driver.close()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, fake)

    def test_enable_current_write_exception_faults_and_attempts_q0_then_d0(self):
        class ExplodingEnableSerial(FakeGainSerial):
            def write(self, data):
                if bytes(data) == b"STQA000001\r\n":
                    self.writes.append(bytes(data))
                    raise OSError("enable write failure")
                return super().write(data)

        clock = FakeClock()
        fake = ExplodingEnableSerial(gain_startup_replies())
        driver = GainDriver(
            port="COM5", serial_factory=lambda **kwargs: fake,
            start_watchdog=False, clock=clock,
        ).connect()
        fake.queue_temperature(22.0, 22.0, 22.0, 22.0, 22.0, 22.0)
        for _ in range(6):
            driver._watchdog_iteration()
            clock.advance(1.0)
        fake.queue(
            b"READY;R=1\r\n", b"READY;C=150.000\r\n", b"READY;T=22.000\r\n",
            b"READY;Q=0\r\n", b"READY;D=0\r\n",
        )
        with self.assertRaises(InstrumentConnectionError):
            driver.enable_current()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(
            fake.writes[-3:],
            [b"STQA000001\r\n", b"STQA000000\r\n", b"STRA000000\r\n"],
        )


class CommonTests(unittest.TestCase):
    def test_require_state_accepts_allowed_state(self):
        require_state(DriverState.READY, {DriverState.READY}, "read")

    def test_require_state_rejects_wrong_state(self):
        with self.assertRaises(InstrumentSafetyError) as caught:
            require_state(DriverState.FAULT, {DriverState.READY}, "read")
        self.assertIn("read", str(caught.exception))
        self.assertIn("FAULT", str(caught.exception))

    def test_logger_is_namespaced_and_has_null_handler(self):
        logger = get_logger("voltage")
        self.assertEqual(logger.name, "sil.voltage")
        self.assertTrue(any(isinstance(item, logging.NullHandler) for item in logger.handlers))

    def test_package_exports_find_serial_port(self):
        from Code.Utils import find_serial_port as exported

        self.assertIs(exported, find_serial_port)

    def test_package_exports_osa_driver_and_spectrum(self):
        from Code.Utils import AQ6370, Spectrum
        from Code.Utils.osa import AQ6370 as DirectAQ6370
        from Code.Utils.osa import Spectrum as DirectSpectrum

        self.assertIs(AQ6370, DirectAQ6370)
        self.assertIs(Spectrum, DirectSpectrum)


class VoltageProtocolTests(unittest.TestCase):
    def test_encode_zero_and_full_safe_range(self):
        frame = encode_voltages([0.0, 14.0, 0, 0, 0, 0, 0, 0])
        self.assertEqual(len(frame), 18)
        self.assertEqual(frame[:2], b"\x00\x00")
        expected = int(14.0 * 65535 / 28)
        self.assertEqual(frame[2:4], expected.to_bytes(2, "big"))
        self.assertTrue(frame.endswith(b"\r\n"))

    def test_encode_rejects_count_and_unsafe_values(self):
        import math

        for values in (
            [0.0] * 7,
            [0.0] * 7 + [14.01],
            [0.0] * 7 + [math.nan],
            [-0.01] + [0.0] * 7,
            [True] + [0.0] * 7,
        ):
            with self.subTest(values=values):
                with self.assertRaises(InstrumentSafetyError):
                    encode_voltages(values)

    def test_decode_eight_voltage_current_pairs(self):
        payload = bytearray()
        for channel in range(8):
            voltage_raw = 1000 + channel
            current_raw = -20 + channel
            payload.extend(voltage_raw.to_bytes(2, "big"))
            payload.extend(current_raw.to_bytes(2, "big", signed=True))
        status = decode_telemetry(bytes(payload) + b"\r\n", received_at=123.0)
        self.assertAlmostEqual(status.voltage_v[0], 1.6)
        self.assertAlmostEqual(status.current_ma[0], -0.05)
        self.assertEqual(status.received_at, 123.0)

    def test_decode_rejects_short_or_unterminated_frame(self):
        for frame in (b"\x00" * 32, b"\x00" * 31 + b"\r\n"):
            with self.assertRaises(InstrumentProtocolError):
                decode_telemetry(frame)


def make_voltage_frame(voltages, currents):
    payload = bytearray()
    for voltage, current in zip(voltages, currents, strict=True):
        payload.extend(round(voltage / 0.0016).to_bytes(2, "big", signed=False))
        payload.extend(round(current / 0.0025).to_bytes(2, "big", signed=True))
    return bytes(payload) + b"\r\n"


class FakeVoltageSerial:
    def __init__(self, frames, *, unblocks_on_close=True):
        self._frames = list(frames)
        self._startup_observation = self._frames[-1] if self._frames else None
        self._zero_writes = 0
        self.read_waiting = threading.Event()
        self._condition = threading.Condition()
        self._closed = threading.Event()
        self._zero_command_written = threading.Event()
        self._reads = 0
        self._released = False
        self._unblocks_on_close = unblocks_on_close
        self.writes = []
        self.is_open = True

    def read_until(self, expected=b"\n"):
        with self._condition:
            while not self._frames:
                self.read_waiting.set()
                if self._released or (not self.is_open and self._unblocks_on_close):
                    return b""
                self._condition.wait()
            frame = self._frames.pop(0)
            self._reads += 1
            requires_zero_command = self._reads > 1
        if requires_zero_command and not self._zero_command_written.wait(timeout=1.0):
            return b""
        time.sleep(0.001)
        return frame

    def write(self, data):
        self.writes.append(bytes(data))
        if data == encode_voltages([0.0] * 8):
            self._zero_command_written.set()
            self._queue_zero_observations()
        return len(data)

    def _queue_zero_observations(self):
        # Streaming firmware can supply more than one observation per write.
        # The first may complete a pre-write read; the next starts after it.
        self._zero_writes += 1
        if self._zero_writes > 1 and not self._unblocks_on_close:
            return
        frame = (self._startup_observation if self._zero_writes == 1 and self._startup_observation
                 else make_voltage_frame([0.0] * 8, [0.0] * 8))
        self.queue_frames([frame, frame])

    def close(self):
        self.is_open = False
        self._closed.set()
        with self._condition:
            self._condition.notify_all()

    def cancel_read(self):
        # Mirror pySerial's non-destructive cancellation, except in the
        # deliberately unstoppable-reader fixture.
        if self._unblocks_on_close:
            self.release()

    def queue_frames(self, frames):
        with self._condition:
            self.read_waiting.clear()
            self._frames.extend(frames)
            self._condition.notify_all()

    def release(self):
        with self._condition:
            self._released = True
            self._condition.notify_all()


class FinalFixVoltageSerial(FakeVoltageSerial):
    """Deterministic serial fake for shutdown and exact-write regressions."""

    def __init__(self, frames, *, write_results=None, write_effects=None, close_error=None, read_error=None):
        super().__init__(frames)
        self._write_results = list(write_results or [])
        self._write_effects = list(write_effects or [])
        self._close_error = close_error
        self._read_error = read_error
        self.nonzero_written = threading.Event()

    def read_until(self, expected=b"\n"):
        if self._read_error is not None and not self._frames:
            error = self._read_error
            self._read_error = None
            raise error
        return super().read_until(expected)

    def write(self, data):
        frame = bytes(data)
        self.writes.append(frame)
        if frame != encode_voltages([0.0] * 8):
            self.nonzero_written.set()
        if frame == encode_voltages([0.0] * 8):
            self._zero_command_written.set()
        if self._write_effects:
            effect = self._write_effects.pop(0)
            if isinstance(effect, BaseException):
                raise effect
            if effect == len(frame) and frame == encode_voltages([0.0] * 8):
                self._queue_zero_observations()
            return effect
        if self._write_results:
            result = self._write_results.pop(0)
            if result == len(frame) and frame == encode_voltages([0.0] * 8):
                self._queue_zero_observations()
            return result
        if frame == encode_voltages([0.0] * 8):
            self._queue_zero_observations()
        return len(frame)

    def close(self):
        super().close()
        if self._close_error is not None:
            raise self._close_error


class Round2VoltageSerial(FinalFixVoltageSerial):
    def __init__(self, frames, **kwargs):
        super().__init__(frames, **kwargs)
        self.read_started = threading.Event()

    def read_until(self, expected=b"\n"):
        self.read_started.set()
        return super().read_until(expected)


class RecoveringVoltageSerial:
    """Test transport whose read/write failures can recover on the same port."""

    def __init__(self, initial_voltage=1.0):
        self._condition = threading.Condition()
        self._effects = [make_voltage_frame([initial_voltage] * 8, [0.0] * 8)]
        self._write_effects = []
        self.read_error_seen = threading.Event()
        self.writes = []
        self.is_open = True
        self.auto_telemetry = True
        self.read_waiting = threading.Event()

    def read_until(self, expected=b"\n"):
        with self._condition:
            while not self._effects and self.is_open:
                self.read_waiting.set()
                self._condition.wait()
            if not self.is_open:
                return b""
            effect = self._effects.pop(0)
        if isinstance(effect, BaseException):
            self.read_error_seen.set()
            raise effect
        time.sleep(0.001)
        return effect

    def write(self, data):
        frame = bytes(data)
        self.writes.append(frame)
        if self._write_effects:
            effect = self._write_effects.pop(0)
            if isinstance(effect, BaseException):
                raise effect
            if effect != len(frame):
                return effect
        if self.auto_telemetry:
            voltages = decode_command(frame)
            self.queue_read_effects(
                [make_voltage_frame(voltages, [0.0] * 8)]
                * (2 if frame == encode_voltages([0.0] * 8) else 1)
            )
        return len(frame)

    def queue_read_effects(self, effects):
        with self._condition:
            self.read_waiting.clear()
            self._effects.extend(effects)
            self._condition.notify_all()

    def queue_write_effects(self, effects):
        self._write_effects.extend(effects)

    def cancel_read(self):
        with self._condition:
            self._effects.append(b"")
            self._condition.notify_all()

    def close(self):
        self.is_open = False
        with self._condition:
            self._condition.notify_all()


def connected_voltage_driver(**kwargs):
    from Code.Utils.voltage import VoltageSource

    initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
    zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
    fake = kwargs.pop("fake", FakeVoltageSerial([initial, zeroed]))
    driver = VoltageSource(port="COM4", serial_factory=lambda **factory_kwargs: fake, **kwargs)
    return driver.connect(), fake


def decode_command(frame):
    """Return voltage values from a voltage-source command frame."""
    if len(frame) != 18 or not frame.endswith(b"\r\n"):
        raise AssertionError(f"Unexpected voltage command frame: {frame!r}")
    return tuple(
        int.from_bytes(frame[offset : offset + 2], "big") * 28.0 / 65535
        for offset in range(0, 16, 2)
    )


def assert_voltage_unknown_released(testcase, driver):
    """Fault-stop tests must distinguish output uncertainty from resource leaks."""
    with testcase.assertRaises(DeviceFault):
        driver.close()
    testcase.assertEqual(driver.state, DriverState.FAULT)
    testcase.assertEqual(driver.zero_evidence.state.value, "unknown")
    testcase.assertTrue(driver.resources_released)


class VoltageControlTests(unittest.TestCase):
    def test_ramp_never_exceeds_step_and_reaches_target(self):
        driver, fake = connected_voltage_driver(
            ramp_step=0.1,
            ramp_interval=0.05,
            sleep=lambda _duration: None,
        )
        try:
            fake.writes.clear()
            driver.set_channel(1, 0.25)
            decoded = [decode_command(frame)[0] for frame in fake.writes]
            self.assertGreaterEqual(len(decoded), 3)
            self.assertTrue(
                all(0 < b - a <= 0.1005 for a, b in zip([0.0] + decoded[:-1], decoded))
            )
            self.assertAlmostEqual(decoded[-1], 0.25, places=3)
        finally:
            driver.close()


    def test_invalid_target_writes_nothing(self):
        driver, fake = connected_voltage_driver()
        try:
            fake.writes.clear()
            with self.assertRaises(InstrumentSafetyError):
                driver.set_channel(1, 14.1)
            self.assertEqual(fake.writes, [])
        finally:
            driver.close()

    def test_close_sends_immediate_zero_twice_safely(self):
        driver, fake = connected_voltage_driver()
        driver.close()
        writes_after_first = list(fake.writes)
        driver.close()
        self.assertEqual(fake.writes, writes_after_first)
        self.assertEqual(writes_after_first[-1], encode_voltages([0.0] * 8))

    def test_context_cleanup_failure_does_not_mask_body_error(self):
        from Code.Utils.voltage import VoltageSource

        fake = FakeVoltageSerial(
            [
                make_voltage_frame([1.0] * 8, [0.0] * 8),
                make_voltage_frame([0.0] * 8, [0.0] * 8),
            ],
            unblocks_on_close=False,
        )
        driver = VoltageSource(
            port="COM4",
            io_timeout=0.001,
            serial_factory=lambda **kwargs: fake,
        )
        try:
            with self.assertRaisesRegex(RuntimeError, "body failure"):
                with driver:
                    raise RuntimeError("body failure")
            self.assertIsInstance(driver.cleanup_error, DeviceFault)
            self.assertEqual(driver.state, DriverState.FAULT)
        finally:
            fake.release()
            assert_voltage_unknown_released(self, driver)

    def test_concurrent_channel_updates_preserve_both_targets(self):
        class SnapshotBarrierCommanded(list):
            def __init__(self, values, barrier, lock_is_owned):
                super().__init__(values)
                self._barrier = barrier
                self._lock_is_owned = lock_is_owned

            def __iter__(self):
                if not self._lock_is_owned():
                    self._barrier.wait(timeout=1.0)
                return super().__iter__()

        driver, fake = connected_voltage_driver(sleep=lambda _duration: None)
        failures = []
        try:
            fake.writes.clear()
            driver._commanded = SnapshotBarrierCommanded(
                [0.0] * 8,
                threading.Barrier(2),
                driver._lock._is_owned,
            )

            def update(channel):
                try:
                    driver.set_channel(channel, 0.2)
                except BaseException as error:
                    failures.append(error)

            first = threading.Thread(target=update, args=(1,))
            second = threading.Thread(target=update, args=(2,))
            first.start()
            second.start()
            first.join(timeout=2.0)
            second.join(timeout=2.0)

            self.assertFalse(first.is_alive())
            self.assertFalse(second.is_alive())
            self.assertEqual(failures, [])
            final_command = decode_command(fake.writes[-1])
            self.assertAlmostEqual(final_command[0], 0.2, places=3)
            self.assertAlmostEqual(final_command[1], 0.2, places=3)
        finally:
            driver.close()


class VoltageTransientRecoveryTests(unittest.TestCase):
    def make_connected(self, **kwargs):
        from Code.Utils.voltage import VoltageSource

        fake = RecoveringVoltageSerial()
        driver = VoltageSource(
            port="COM4",
            serial_factory=lambda **factory_kwargs: fake,
            **kwargs,
        ).connect()
        return driver, fake

    def test_single_read_exception_holds_last_voltage_and_recovers_without_zero(self):
        """Catches treating the first recoverable read error as a terminal fault."""

        driver, fake = self.make_connected()
        try:
            driver.set_channel(1, 0.1)
            fake.writes.clear()
            fake.queue_read_effects([OSError("transient read")])
            self.assertTrue(fake.read_error_seen.wait(timeout=1.0))

            self.assertEqual(driver.state, DriverState.ACTIVE)
            self.assertEqual(fake.writes, [])

            result = []
            waiter = threading.Thread(target=lambda: result.append(driver.wait_for_status()))
            waiter.start()
            fake.queue_read_effects(
                [make_voltage_frame([0.1] + [0.0] * 7, [0.0] * 8)]
            )
            waiter.join(timeout=1.0)
            self.assertFalse(waiter.is_alive())
            self.assertAlmostEqual(result[0].voltage_v[0], 0.1, places=2)
            self.assertEqual(fake.writes, [])
        finally:
            driver.close()

    def test_read_recovery_log_preserves_underlying_serial_error_detail(self):
        """Catches reducing actionable communication evidence to an exception class."""

        driver, fake = self.make_connected()
        audit = Mock()
        driver.log = audit
        try:
            driver.set_channel(1, 0.1)
            fake.queue_read_effects([OSError("USB bridge glitch 17")])
            self.assertTrue(fake.read_error_seen.wait(timeout=1.0))
            fake.queue_read_effects(
                [make_voltage_frame([0.1] + [0.0] * 7, [0.0] * 8)]
            )
            driver.wait_for_status()

            warning_args = [
                argument
                for call in audit.warning.call_args_list
                for argument in call.args
            ]
            self.assertTrue(
                any("USB bridge glitch 17" in str(argument) for argument in warning_args),
                warning_args,
            )
        finally:
            driver.close()

    def test_malformed_frame_burst_recovers_without_zero_when_valid_frame_returns(self):
        """Catches using a small malformed-frame count as an immediate zero trigger."""

        driver, fake = self.make_connected()
        try:
            driver.set_channel(1, 0.1)
            self.assertTrue(fake.read_waiting.wait(1.0))
            fake.writes.clear()
            result = []
            errors = []

            def wait_for_recovery():
                try:
                    result.append(driver.wait_for_status())
                except BaseException as error:
                    errors.append(error)

            waiter = threading.Thread(target=wait_for_recovery)
            waiter.start()
            fake.queue_read_effects(
                [b"bad\r\n"] * 4
                + [make_voltage_frame([0.1] + [0.0] * 7, [0.0] * 8)]
            )
            waiter.join(timeout=1.0)

            self.assertFalse(waiter.is_alive())
            self.assertEqual(errors, [])
            self.assertAlmostEqual(result[0].voltage_v[0], 0.1, places=2)
            self.assertEqual(driver.state, DriverState.ACTIVE)
            self.assertEqual(fake.writes, [])
        finally:
            driver.close()

    def test_read_failures_past_recovery_deadline_fault_and_zero(self):
        """Catches unbounded read retries that can never reach safe shutdown."""

        driver, fake = self.make_connected()
        driver.communication_recovery_timeout = 0.05
        try:
            driver.set_channel(1, 0.1)
            fake.writes.clear()
            fake.queue_read_effects([OSError("persistent read")] * 10)
            deadline = time.monotonic() + 1.0
            while time.monotonic() < deadline and driver.state != DriverState.FAULT:
                time.sleep(0.01)

            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertEqual(fake.writes[-1], encode_voltages([0.0] * 8))
        finally:
            assert_voltage_unknown_released(self, driver)

    def test_transient_write_exception_retries_same_frame_without_zero(self):
        """Catches faulting before the approved identical-frame write retry."""

        driver, fake = self.make_connected()
        try:
            fake.writes.clear()
            fake.queue_write_effects([OSError("transient write")])
            errors = []
            try:
                driver.set_channel(1, 0.1)
            except BaseException as error:
                errors.append(error)

            target = encode_voltages([0.1] + [0.0] * 7)
            self.assertEqual(errors, [])
            self.assertEqual(fake.writes, [target, target])
            self.assertEqual(driver.state, DriverState.ACTIVE)
        finally:
            driver.close()

    def test_partial_write_retries_complete_frame_without_zero(self):
        """Catches accepting or faulting on a recoverable short write."""

        driver, fake = self.make_connected()
        try:
            fake.writes.clear()
            fake.queue_write_effects([5])
            errors = []
            try:
                driver.set_channel(1, 0.1)
            except BaseException as error:
                errors.append(error)

            target = encode_voltages([0.1] + [0.0] * 7)
            self.assertEqual(errors, [])
            self.assertEqual(fake.writes, [target, target])
            self.assertEqual(driver.state, DriverState.ACTIVE)
        finally:
            driver.close()

    def test_three_failed_writes_exhaust_retries_then_fault_and_zero(self):
        """Catches a fourth normal write or failure to zero after retry exhaustion."""

        driver, fake = self.make_connected()
        try:
            fake.writes.clear()
            fake.queue_write_effects([OSError("persistent write")] * 3)
            with self.assertRaises(InstrumentConnectionError):
                driver.set_channel(1, 0.1)

            target = encode_voltages([0.1] + [0.0] * 7)
            self.assertEqual(fake.writes[:3], [target, target, target])
            self.assertEqual(fake.writes[3:], [encode_voltages([0.0] * 8)])
            self.assertEqual(driver.state, DriverState.FAULT)
        finally:
            assert_voltage_unknown_released(self, driver)

    def test_status_wait_uses_active_recovery_deadline_instead_of_short_timeout(self):
        """Catches aborting the experiment before the recovery window expires."""

        driver, fake = self.make_connected(
            telemetry_timeout=0.05,
            communication_recovery_timeout=0.3,
        )
        try:
            driver.set_channel(1, 0.1)
            fake.queue_read_effects([OSError("transient read")])
            self.assertTrue(fake.read_error_seen.wait(timeout=1.0))
            result = []
            errors = []

            def wait_for_recovery():
                try:
                    result.append(driver.wait_for_status())
                except BaseException as error:
                    errors.append(error)

            waiter = threading.Thread(target=wait_for_recovery)
            waiter.start()
            time.sleep(0.08)
            fake.queue_read_effects(
                [make_voltage_frame([0.1] + [0.0] * 7, [0.0] * 8)]
            )
            waiter.join(timeout=1.0)

            self.assertFalse(waiter.is_alive())
            self.assertEqual(errors, [])
            self.assertAlmostEqual(result[0].voltage_v[0], 0.1, places=2)
            self.assertEqual(driver.state, DriverState.ACTIVE)
        finally:
            driver.close()

    def test_new_voltage_command_waits_for_telemetry_recovery_before_writing(self):
        """Catches advancing the ramp while communication health is unknown."""

        driver, fake = self.make_connected()
        errors = []
        command = None
        try:
            driver.set_channel(1, 0.1)
            fake.writes.clear()
            fake.queue_read_effects([OSError("transient read")])
            self.assertTrue(fake.read_error_seen.wait(timeout=1.0))

            def set_next_voltage():
                try:
                    driver.set_channel(1, 0.2)
                except BaseException as error:
                    errors.append(error)

            command = threading.Thread(target=set_next_voltage)
            command.start()
            time.sleep(0.05)
            self.assertTrue(command.is_alive())
            self.assertEqual(fake.writes, [])

            fake.queue_read_effects(
                [make_voltage_frame([0.1] + [0.0] * 7, [0.0] * 8)]
            )
            command.join(timeout=1.0)
            self.assertFalse(command.is_alive())
            self.assertEqual(errors, [])
            self.assertEqual(fake.writes[-1], encode_voltages([0.2] + [0.0] * 7))
        finally:
            if command is not None:
                command.join(timeout=1.0)
            driver.close()

    def test_target_wait_ignores_stale_voltage_until_commanded_channel_arrives(self):
        """Catches acquiring on the first frame even when it still reports the old voltage."""

        driver, fake = self.make_connected()
        result = []
        errors = []
        waiter = None
        try:
            fake.auto_telemetry = False
            driver.set_channel(1, 0.1)

            def wait_for_target():
                try:
                    result.append(driver.wait_for_channel(1, 0.1))
                except BaseException as error:
                    errors.append(error)

            waiter = threading.Thread(target=wait_for_target)
            waiter.start()
            time.sleep(0.02)
            self.assertTrue(waiter.is_alive())

            fake.queue_read_effects(
                [make_voltage_frame([0.0] * 8, [0.0] * 8)]
            )
            time.sleep(0.02)
            self.assertTrue(waiter.is_alive())

            fake.queue_read_effects(
                [make_voltage_frame([0.1] + [0.0] * 7, [0.0] * 8)]
            )
            waiter.join(timeout=1.0)
            self.assertFalse(waiter.is_alive())
            self.assertEqual(errors, [])
            self.assertAlmostEqual(result[0].voltage_v[0], 0.1, places=2)
        finally:
            if waiter is not None:
                waiter.join(timeout=1.0)
            fake.auto_telemetry = True
            driver.close()

    def test_target_wait_default_accepts_configured_ninety_five_mv_offset(self):
        """Catches narrowing the operator-selected 100 mV confirmation tolerance."""

        driver, fake = self.make_connected(telemetry_timeout=0.05)
        try:
            fake.auto_telemetry = False
            driver.set_channel(1, 0.2)
            fake.queue_read_effects(
                [make_voltage_frame([0.104] + [0.0] * 7, [0.0] * 8)]
            )

            status = driver.wait_for_channel(1, 0.2)

            self.assertAlmostEqual(status.voltage_v[0], 0.104, places=4)
        finally:
            fake.auto_telemetry = True
            driver.close()


class VoltageConnectionTests(unittest.TestCase):
    def tearDown(self):
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline and any(
            thread.name == "VoltageSourceReader" for thread in threading.enumerate()
        ):
            time.sleep(0.01)
        self.assertFalse(
            any(thread.name == "VoltageSourceReader" for thread in threading.enumerate()),
            "VoltageSource reader thread leaked from test",
        )

    def test_connect_reads_frame_zeros_and_confirms_new_zero_status(self):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = FakeVoltageSerial([initial, zeroed])
        driver = VoltageSource(
            port="COM4", serial_factory=lambda **kwargs: fake, startup_timeout=1.0
        )
        driver.connect()
        self.assertEqual(fake.writes[-1], encode_voltages([0.0] * 8))
        self.assertTrue(all(abs(value) < 0.05 for value in driver.read_status().voltage_v))
        driver.close()
        self.assertFalse(fake.is_open)

    def test_connect_fails_if_zero_is_not_confirmed(self):
        from Code.Utils.voltage import VoltageSource

        nonzero = make_voltage_frame([1.0] * 8, [0.0] * 8)
        fake = FakeVoltageSerial([nonzero, nonzero])
        driver = VoltageSource(
            port="COM4", serial_factory=lambda **kwargs: fake, startup_timeout=0.05
        )
        with self.assertRaises(InstrumentTimeoutError):
            driver.connect()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertFalse(fake.is_open)

    def test_constructor_enforces_safe_ramp_boundaries(self):
        from Code.Utils.voltage import VoltageSource

        VoltageSource(ramp_step=0.1, ramp_interval=0.05)
        for kwargs in (
            {"ramp_step": 0.1001},
            {"ramp_step": True},
            {"ramp_step": float("nan")},
            {"ramp_interval": 0.0499},
            {"ramp_interval": True},
            {"ramp_interval": float("inf")},
        ):
            with self.subTest(kwargs=kwargs):
                with self.assertRaises(ValueError):
                    VoltageSource(**kwargs)

    def test_faulted_driver_requires_cleanup_before_reconnect(self):
        from Code.Utils.voltage import VoltageSource

        first = FakeVoltageSerial(
            [
                make_voltage_frame([1.0] * 8, [0.0] * 8),
                make_voltage_frame([0.0] * 8, [0.0] * 8),
            ]
        )
        second = FakeVoltageSerial(
            [
                make_voltage_frame([1.0] * 8, [0.0] * 8),
                make_voltage_frame([0.0] * 8, [0.0] * 8),
            ]
        )
        factory_calls = []

        def factory(**kwargs):
            factory_calls.append(kwargs)
            return (first, second)[len(factory_calls) - 1]

        driver = VoltageSource(port="COM4", serial_factory=factory).connect()
        try:
            driver._enter_fault(InstrumentProtocolError("simulated fault"))
            with self.assertRaises(DeviceFault):
                driver.connect()
            self.assertEqual(len(factory_calls), 1)
            self.assertTrue(first.is_open)
        finally:
            first.close()
            second.close()
            assert_voltage_unknown_released(self, driver)

    def test_close_faults_and_retains_reader_when_transport_cannot_unblock(self):
        from Code.Utils.voltage import VoltageSource

        fake = FakeVoltageSerial(
            [
                make_voltage_frame([1.0] * 8, [0.0] * 8),
                make_voltage_frame([0.0] * 8, [0.0] * 8),
            ],
            unblocks_on_close=False,
        )
        driver = VoltageSource(
            port="COM4",
            io_timeout=0.001,
            serial_factory=lambda **kwargs: fake,
        ).connect()
        try:
            with self.assertRaises(DeviceFault):
                driver.close()
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsNotNone(driver._reader_thread)
            self.assertTrue(driver._reader_thread.is_alive())
            with self.assertRaises(DeviceFault):
                driver.connect()
        finally:
            fake.release()
            assert_voltage_unknown_released(self, driver)

    def test_connection_failure_preserves_original_error_when_cleanup_times_out(self):
        from Code.Utils.voltage import VoltageSource

        fake = FakeVoltageSerial(
            [
                make_voltage_frame([1.0] * 8, [0.0] * 8),
                make_voltage_frame([1.0] * 8, [0.0] * 8),
            ],
            unblocks_on_close=False,
        )
        driver = VoltageSource(
            port="COM4",
            io_timeout=0.001,
            startup_timeout=0.01,
            serial_factory=lambda **kwargs: fake,
        )
        try:
            with self.assertRaises(InstrumentTimeoutError):
                driver.connect()
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsInstance(driver.cleanup_error, DeviceFault)
        finally:
            fake.release()
            assert_voltage_unknown_released(self, driver)

    def test_connect_discovers_ch340_port_through_injected_provider(self):
        from Code.Utils.voltage import VoltageSource

        fake = FakeVoltageSerial(
            [
                make_voltage_frame([1.0] * 8, [0.0] * 8),
                make_voltage_frame([0.0] * 8, [0.0] * 8),
            ]
        )
        calls = []
        driver = VoltageSource(
            serial_factory=lambda **kwargs: calls.append(kwargs) or fake,
            ports_provider=lambda: [
                SimpleNamespace(device="COM13", vid=0x1A86, pid=0x7523, serial_number=None)
            ],
        ).connect()
        self.assertEqual(calls[0]["port"], "COM13")
        driver.close()

    def test_read_status_rejects_stale_telemetry(self):
        now = [10.0]
        driver, _ = connected_voltage_driver(clock=lambda: now[0])
        now[0] = 11.1
        with self.assertRaises(InstrumentTimeoutError):
            driver.read_status(max_age=1.0)
        driver.close()


class VoltageFinalFixTests(unittest.TestCase):
    def make_connected(self, *, serial_type=FinalFixVoltageSerial, **kwargs):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = serial_type([initial, zeroed], **kwargs.pop("serial_kwargs", {}))
        driver = VoltageSource(
            port="COM4", serial_factory=lambda **factory_kwargs: fake, **kwargs
        ).connect()
        return driver, fake

    def assert_no_voltage_reader(self):
        self.assertFalse(
            any(thread.name == "VoltageSourceReader" for thread in threading.enumerate()),
            "VoltageSource reader thread leaked",
        )

    def test_fault_interrupts_long_ramp_and_zero_is_final_frame(self):
        driver, fake = self.make_connected(ramp_interval=0.05)
        failure = []
        ramp = threading.Thread(
            target=lambda: self._capture(failure, lambda: driver.set_channel(1, 3.0))
        )
        try:
            ramp.start()
            self.assertTrue(fake.nonzero_written.wait(timeout=1.0))
            started = time.monotonic()
            driver._enter_fault(InstrumentProtocolError("injected fault"))
            ramp.join(timeout=0.5)
            self.assertFalse(ramp.is_alive())
            self.assertLess(time.monotonic() - started, 0.5)
            self.assertIsInstance(failure[0], DriverError)
            self.assertEqual(fake.writes[-1], encode_voltages([0.0] * 8))
            self.assertEqual(driver._commanded, [0.0] * 8)
        finally:
            assert_voltage_unknown_released(self, driver)
        self.assert_no_voltage_reader()

    def test_close_interrupts_long_ramp_without_post_zero_frame(self):
        driver, fake = self.make_connected(ramp_interval=0.05)
        failure = []
        ramp = threading.Thread(
            target=lambda: self._capture(failure, lambda: driver.set_channel(1, 3.0))
        )
        try:
            ramp.start()
            self.assertTrue(fake.nonzero_written.wait(timeout=1.0))
            closer = threading.Thread(target=lambda: self._capture(failure, driver.close))
            closer.start()
            closer.join(timeout=0.5)
            self.assertFalse(closer.is_alive())
            ramp.join(timeout=0.5)
            self.assertFalse(ramp.is_alive())
            self.assertEqual(failure, [failure[0]])
            self.assertIsInstance(failure[0], DriverError)
            self.assertEqual(fake.writes[-1], encode_voltages([0.0] * 8))
            self.assertEqual(driver.state, DriverState.DISCONNECTED)
        finally:
            if ramp.is_alive():
                ramp.join(timeout=3.0)
            if driver.state != DriverState.DISCONNECTED:
                driver.close()
        self.assert_no_voltage_reader()

    def test_connect_and_close_race_leaves_no_open_unowned_serial(self):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = FinalFixVoltageSerial([initial, zeroed])
        factory_entered = threading.Event()
        factory_continue = threading.Event()
        failures = []

        def factory(**kwargs):
            factory_entered.set()
            factory_continue.wait(timeout=1.0)
            return fake

        driver = VoltageSource(port="COM4", serial_factory=factory)
        connecting = threading.Thread(target=lambda: self._capture(failures, driver.connect))
        closing = threading.Thread(target=lambda: self._capture(failures, driver.close))
        try:
            connecting.start()
            self.assertTrue(factory_entered.wait(timeout=1.0))
            closing.start()
            factory_continue.set()
            connecting.join(timeout=1.0)
            closing.join(timeout=1.0)
            self.assertFalse(connecting.is_alive())
            self.assertFalse(closing.is_alive())
            self.assertFalse(fake.is_open)
            self.assertIn(driver.state, {DriverState.DISCONNECTED, DriverState.FAULT})
            self.assertTrue(driver.resources_released)
            if driver.state == DriverState.DISCONNECTED:
                self.assertEqual(driver.zero_evidence.state.value, "measured_zero")
            else:
                self.assertEqual(driver.zero_evidence.state.value, "unknown")
                self.assertIsInstance(driver.cleanup_error, DriverError)
        finally:
            factory_continue.set()
            connecting.join(timeout=2.0)
            closing.join(timeout=2.0)
            if fake.is_open:
                driver.close()
        self.assert_no_voltage_reader()

    def test_close_surfaces_zero_and_serial_close_failures_without_losing_evidence(self):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = FinalFixVoltageSerial(
            [initial, zeroed], write_results=[18, 0, 0, 0], close_error=OSError("close failed")
        )
        driver = VoltageSource(port="COM4", serial_factory=lambda **kwargs: fake).connect()
        with self.assertRaises(DriverError):
            driver.close()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIsNotNone(driver.cleanup_error)
        self.assertIsNone(driver._serial)
        self.assertIsNone(driver._reader_thread)
        self.assertTrue(driver.resources_released)
        fake._close_error = None
        with self.assertRaises(DriverError):
            driver.close()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assert_no_voltage_reader()

    def test_exact_write_counts_are_required_for_startup_normal_and_emergency_zero(self):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        startup = FinalFixVoltageSerial(
            [initial, zeroed], write_results=[17, 17, 17, 18]
        )
        startup_driver = VoltageSource(port="COM4", serial_factory=lambda **kwargs: startup)
        try:
            with self.assertRaises(InstrumentConnectionError):
                startup_driver.connect()
        finally:
            if startup_driver.state != DriverState.DISCONNECTED:
                startup_driver.close()
        self.assertFalse(startup.is_open)

        driver, normal = self.make_connected(
            serial_kwargs={"write_results": [18, 17, 17, 17, 18]}
        )
        try:
            with self.assertRaises(InstrumentConnectionError):
                driver.set_channel(1, 0.1)
            self.assertEqual(driver._commanded, [0.0] * 8)
        finally:
            assert_voltage_unknown_released(self, driver)

        driver, emergency = self.make_connected(
            serial_kwargs={"write_results": [18, 17, 17, 17]}
        )
        try:
            with self.assertRaises(InstrumentConnectionError):
                driver.zero(emergency=True)
            self.assertEqual(driver.state, DriverState.FAULT)
        finally:
            assert_voltage_unknown_released(self, driver)
        self.assert_no_voltage_reader()

    def test_configured_telemetry_timeout_is_read_status_default(self):
        now = [10.0]
        driver, _ = self.make_connected(clock=lambda: now[0], telemetry_timeout=0.1)
        try:
            now[0] = 10.11
            with self.assertRaises(InstrumentTimeoutError):
                driver.read_status()
            self.assertIsInstance(driver.read_status(max_age=1.0), VoltageStatus)
        finally:
            driver.close()

    def test_factory_uses_exact_115200_8n1_timeouts(self):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = FinalFixVoltageSerial([initial, zeroed])
        captured = []
        driver = VoltageSource(
            port="COM4",
            io_timeout=0.123,
            serial_factory=lambda **kwargs: captured.append(kwargs) or fake,
        ).connect()
        try:
            self.assertEqual(
                captured,
                [{
                    "port": "COM4", "baudrate": 115200, "bytesize": 8,
                    "parity": "N", "stopbits": 1, "timeout": 0.123,
                    "write_timeout": 0.123,
                }],
            )
        finally:
            driver.close()

    def test_wait_for_status_requires_a_new_sequence_after_command(self):
        driver, fake = self.make_connected()
        try:
            self.assertTrue(fake.read_waiting.wait(1.0))
            driver.set_channel(1, 0.1)
            fake.queue_frames([make_voltage_frame([0.1] + [0.0] * 7, [0.0] * 8)])
            status = driver.wait_for_status(timeout=0.5)
            self.assertAlmostEqual(status.voltage_v[0], 0.1, places=2)
        finally:
            driver.close()

    def test_diagnostic_preserves_primary_error_when_inner_zero_fails(self):
        from Code.Debugs import check_voltage

        class Source:
            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

            def read_status(self):
                return VoltageStatus((0.0,) * 8, (0.0,) * 8, 0.0)

            def set_channel(self, *_):
                raise RuntimeError("primary")

            def wait_for_status(self):
                raise AssertionError("not reached")

            def zero(self, **kwargs):
                raise DeviceFault("zero failed")

        with self.assertRaisesRegex(RuntimeError, "primary"):
            check_voltage._run_check(
                source_factory=lambda **kwargs: Source(), input_fn=lambda _: "y", print_fn=lambda _: None
            )

    def test_diagnostic_success_waits_for_new_status_then_inner_zeros_before_context_exit(self):
        from Code.Debugs import check_voltage

        events = []

        class Source:
            def __enter__(self):
                events.append("enter")
                return self

            def __exit__(self, *args):
                events.append("exit")
                return False

            def read_status(self):
                return VoltageStatus((0.0,) * 8, (0.0,) * 8, 0.0)

            def set_channel(self, channel, voltage):
                events.append(("set", channel, voltage))

            def wait_for_status(self):
                events.append("wait")
                return VoltageStatus((0.1,) + (0.0,) * 7, (0.0,) * 8, 1.0)

            def zero(self, **kwargs):
                events.append(("zero", kwargs))

        check_voltage._run_check(
            source_factory=lambda **kwargs: Source(), input_fn=lambda _: "y", print_fn=lambda _: None
        )
        self.assertEqual(
            events,
            ["enter", ("set", 1, 0.1), "wait", ("zero", {"emergency": True}), "exit"],
        )

    def test_zero_monitor_never_commands_nonzero_and_zeros_before_context_exit(self):
        from Code.Debugs import check_voltage

        events = []
        clock_values = iter((0.0, 0.0, 1.0, 2.0))

        class Source:
            def __enter__(self):
                events.append("enter")
                return self

            def __exit__(self, *args):
                events.append("exit")
                return False

            def read_status(self):
                events.append("read")
                return VoltageStatus((0.0,) * 8, (0.0,) * 8, 0.0)

            def wait_for_status(self, timeout=None):
                events.append(("wait", timeout))
                return VoltageStatus((0.0,) * 8, (0.0,) * 8, 1.0)

            def zero(self, **kwargs):
                events.append(("zero", kwargs))

            def set_channel(self, *_):
                raise AssertionError("zero monitor must never command a nonzero channel")

        samples = check_voltage._run_zero_monitor(
            duration_s=2.0,
            source_factory=lambda **kwargs: Source(),
            print_fn=lambda _: None,
            clock=lambda: next(clock_values),
        )

        self.assertEqual(samples, 2)
        self.assertEqual(
            events,
            [
                "enter",
                "read",
                ("wait", 1.0),
                ("wait", 1.0),
                ("zero", {"emergency": True}),
                "exit",
            ],
        )

    def test_zero_monitor_does_not_shrink_final_telemetry_timeout_to_a_time_sliver(self):
        """Catches a false timeout when the soak deadline is nearly reached."""

        from Code.Debugs import check_voltage

        observed_timeouts = []
        clock_values = iter((0.0, 1.9999, 2.0))

        class Source:
            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

            def read_status(self):
                return VoltageStatus((0.0,) * 8, (0.0,) * 8, 0.0)

            def wait_for_status(self, timeout=None):
                observed_timeouts.append(timeout)
                return VoltageStatus((0.0,) * 8, (0.0,) * 8, 1.0)

            def zero(self, **kwargs):
                pass

        check_voltage._run_zero_monitor(
            duration_s=2.0,
            source_factory=lambda **kwargs: Source(),
            print_fn=lambda _: None,
            clock=lambda: next(clock_values),
        )

        self.assertEqual(observed_timeouts, [1.0])

    @staticmethod
    def _capture(errors, operation):
        try:
            operation()
        except BaseException as error:
            errors.append(error)


class VoltageFinalFixRound2Tests(unittest.TestCase):
    def make_connected(self, *, serial_type=Round2VoltageSerial, **kwargs):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = serial_type([initial, zeroed], **kwargs.pop("serial_kwargs", {}))
        driver = VoltageSource(
            port="COM4", serial_factory=lambda **factory_kwargs: fake, **kwargs
        ).connect()
        return driver, fake

    @staticmethod
    def capture(errors, operation):
        try:
            operation()
        except BaseException as error:
            errors.append(error)

    def test_public_emergency_zero_cancels_ramp_then_allows_later_operation(self):
        driver, fake = self.make_connected(ramp_interval=0.05)
        errors = []
        ramp = threading.Thread(
            target=lambda: self.capture(errors, lambda: driver.set_channel(1, 0.5))
        )
        try:
            ramp.start()
            self.assertTrue(fake.nonzero_written.wait(timeout=1.0))
            driver.zero(emergency=True)
            zero_index = len(fake.writes) - 1
            self.assertEqual(fake.writes[zero_index], encode_voltages([0.0] * 8))
            ramp.join(timeout=0.5)
            self.assertFalse(ramp.is_alive())
            self.assertIsInstance(errors[0], DriverError)
            self.assertEqual(fake.writes[zero_index + 1 :], [])
            driver.set_channel(1, 0.1)
            self.assertAlmostEqual(decode_command(fake.writes[-1])[0], 0.1, places=3)
        finally:
            driver.close()

    def test_keyboard_interrupt_during_shutdown_zero_still_closes_and_joins(self):
        driver, fake = self.make_connected(serial_kwargs={"write_effects": [18, KeyboardInterrupt("zero interrupt")]})
        try:
            with self.assertRaisesRegex(KeyboardInterrupt, "zero interrupt"):
                driver.close()
            self.assertFalse(fake.is_open)
            self.assertTrue(driver.resources_released)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsInstance(driver.cleanup_error, KeyboardInterrupt)
        finally:
            fake._close_error = None
            fake.close()
            driver._stop_event.set()
            if driver._reader_thread is not None:
                driver._reader_thread.join(timeout=1.0)

    def test_base_exception_during_serial_close_still_joins_and_retains_evidence(self):
        driver, fake = self.make_connected(serial_kwargs={"close_error": SystemExit("close exit")})
        try:
            with self.assertRaisesRegex(SystemExit, "close exit"):
                driver.close()
            self.assertFalse(fake.is_open)
            self.assertTrue(driver.resources_released)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsInstance(driver.cleanup_error, SystemExit)
        finally:
            fake._close_error = None
            fake.close()
            driver._stop_event.set()
            if driver._reader_thread is not None:
                driver._reader_thread.join(timeout=1.0)

    def test_failed_zero_followed_by_closed_transport_cannot_claim_disconnected(self):
        driver, fake = self.make_connected(
            serial_kwargs={"write_results": [18, 0, 0, 0]}
        )
        with self.assertRaises(DriverError) as first:
            driver.close()
        self.assertFalse(fake.is_open)
        self.assertEqual(driver.state, DriverState.FAULT)
        with self.assertRaises(DeviceFault):
            driver.close()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertTrue(driver.resources_released)
        self.assertIsNotNone(driver.cleanup_error)

    def test_confirmed_zero_allows_retry_after_close_failure(self):
        driver, fake = self.make_connected(serial_kwargs={"close_error": OSError("close once")})
        with self.assertRaises(OSError):
            driver.close()
        self.assertEqual(driver.state, DriverState.FAULT)
        fake._close_error = None
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertIsNone(driver._serial)


class VoltageFinalFixRound3Tests(unittest.TestCase):
    def make_connected(self, **kwargs):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = Round2VoltageSerial([initial, zeroed])
        driver = VoltageSource(
            port="COM4", ramp_interval=0.05,
            serial_factory=lambda **factory_kwargs: fake, **kwargs,
        ).connect()
        return driver, fake

    @staticmethod
    def capture(errors, operation):
        try:
            operation()
        except BaseException as error:
            errors.append(error)

    def test_queued_pre_emergency_operation_cannot_clear_cancellation_or_write(self):
        driver, fake = self.make_connected()
        validation_entered = threading.Event()
        release_validation = threading.Event()
        a_errors = []
        b_errors = []

        class BlockingTarget:
            def __iter__(self):
                validation_entered.set()
                release_validation.wait(timeout=1.0)
                return iter([0.2] + [0.0] * 7)

        a = threading.Thread(
            target=lambda: self.capture(a_errors, lambda: driver.set_channel(1, 0.5))
        )
        b = threading.Thread(
            target=lambda: self.capture(b_errors, lambda: driver.ramp_to(BlockingTarget()))
        )
        try:
            a.start()
            self.assertTrue(fake.nonzero_written.wait(timeout=1.0))
            b.start()
            self.assertTrue(validation_entered.wait(timeout=1.0))
            driver.zero(emergency=True)
            zero_index = len(fake.writes) - 1
            self.assertEqual(fake.writes[zero_index], encode_voltages([0.0] * 8))
            release_validation.set()
            a.join(timeout=0.5)
            b.join(timeout=0.5)
            self.assertFalse(a.is_alive())
            self.assertFalse(b.is_alive())
            self.assertIsInstance(a_errors[0], DriverError)
            self.assertIsInstance(b_errors[0], DriverError)
            self.assertEqual(fake.writes[zero_index + 1 :], [])
            driver.set_channel(1, 0.1)
            self.assertAlmostEqual(decode_command(fake.writes[-1])[0], 0.1, places=3)
        finally:
            release_validation.set()
            a.join(timeout=1.0)
            b.join(timeout=1.0)
            driver.close()

    def test_emergency_zero_with_missing_or_closed_serial_faults_without_false_bookkeeping(self):
        from Code.Utils.voltage import VoltageSource

        for serial_port in (None, FinalFixVoltageSerial([])):
            with self.subTest(serial_port=serial_port):
                if serial_port is not None:
                    serial_port.close()
                driver = VoltageSource(port="COM4")
                driver.state = DriverState.READY
                driver._serial = serial_port
                driver._commanded = [0.3] * 8
                with self.assertRaises(InstrumentConnectionError):
                    driver.zero(emergency=True)
                self.assertEqual(getattr(serial_port, "writes", []), [])
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertEqual(driver._commanded, [0.3] * 8)
                self.assertEqual(driver.zero_evidence.state.value, "unknown")
                self.assertIsNotNone(driver.cleanup_error)

    def test_wait_for_status_exits_on_close_and_fault(self):
        driver, _ = self.make_connected(telemetry_timeout=5.0)
        close_errors = []
        waiter = threading.Thread(target=lambda: self.capture(close_errors, lambda: driver.wait_for_status()))
        try:
            waiter.start()
            driver.close()
            waiter.join(timeout=0.5)
            self.assertFalse(waiter.is_alive())
            self.assertIsInstance(close_errors[0], DriverError)
        finally:
            if driver.state != DriverState.DISCONNECTED:
                driver.close()

        driver, _ = self.make_connected(telemetry_timeout=5.0)
        fault_errors = []
        waiter = threading.Thread(target=lambda: self.capture(fault_errors, lambda: driver.wait_for_status()))
        try:
            waiter.start()
            driver._enter_fault(InstrumentProtocolError("fault while waiting"))
            waiter.join(timeout=0.5)
            self.assertFalse(waiter.is_alive())
            self.assertIsInstance(fault_errors[0], DeviceFault)
        finally:
            assert_voltage_unknown_released(self, driver)

    def test_close_cancels_connect_startup_wait_without_timeout_or_leak(self):
        from Code.Utils.voltage import VoltageSource

        fake = Round2VoltageSerial([])
        driver = VoltageSource(
            port="COM4", startup_timeout=5.0, io_timeout=0.01,
            serial_factory=lambda **kwargs: fake,
        )
        errors = []
        connecting = threading.Thread(target=lambda: self.capture(errors, driver.connect))
        connecting.start()
        self.assertTrue(fake.read_started.wait(timeout=1.0))
        closer = threading.Thread(target=lambda: self.capture(errors, driver.close))
        closer.start()
        connecting.join(timeout=0.5)
        closer.join(timeout=0.5)
        self.assertFalse(connecting.is_alive())
        self.assertFalse(closer.is_alive())
        self.assertFalse(fake.is_open)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertTrue(any(isinstance(error, InstrumentConnectionError) for error in errors))
        self.assertFalse(any(thread.name == "VoltageSourceReader" for thread in threading.enumerate()))

    def test_reader_thread_close_keeps_truthful_fault_until_another_thread_finishes_cleanup(self):
        from Code.Utils.voltage import VoltageSource

        fake = Round2VoltageSerial([])
        driver = VoltageSource(port="COM4", serial_factory=lambda **kwargs: fake)
        driver._serial = fake
        driver.state = DriverState.READY
        errors = []

        def reader_owned_close():
            driver._reader_thread = threading.current_thread()
            self.capture(errors, driver.close)

        reader = threading.Thread(target=reader_owned_close, name="VoltageSourceReader")
        reader.start()
        reader.join(timeout=1.0)
        self.assertFalse(reader.is_alive())
        self.assertIsInstance(errors[0], DriverError)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._reader_thread, reader)
        self.assertFalse(driver.resources_released)
        assert_voltage_unknown_released(self, driver)


class Round4VoltageSerial(Round2VoltageSerial):
    def __init__(self, frames):
        super().__init__(frames)
        self.gate_zero = False
        self.zero_write_entered = threading.Event()
        self.release_zero_write = threading.Event()

    def write(self, data):
        if self.gate_zero and bytes(data) == encode_voltages([0.0] * 8):
            self.zero_write_entered.set()
            self.release_zero_write.wait(timeout=1.0)
        return super().write(data)


class VoltageFinalFixRound4Tests(unittest.TestCase):
    @staticmethod
    def capture(errors, operation):
        try:
            operation()
        except BaseException as error:
            errors.append(error)

    def test_operation_started_during_gated_emergency_zero_is_expired_at_completion(self):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = Round4VoltageSerial([initial, zeroed])
        driver = VoltageSource(port="COM4", serial_factory=lambda **kwargs: fake).connect()
        fake.gate_zero = True
        emergency_errors = []
        c_errors = []
        emergency = threading.Thread(
            target=lambda: self.capture(emergency_errors, lambda: driver.zero(emergency=True))
        )
        c = threading.Thread(
            target=lambda: self.capture(c_errors, lambda: driver.set_channel(1, 0.2))
        )
        try:
            emergency.start()
            self.assertTrue(fake.zero_write_entered.wait(timeout=1.0))
            c.start()
            fake.release_zero_write.set()
            emergency.join(timeout=0.5)
            c.join(timeout=0.5)
            self.assertFalse(emergency.is_alive())
            self.assertFalse(c.is_alive())
            self.assertEqual(emergency_errors, [])
            self.assertIsInstance(c_errors[0], DriverError)
            self.assertEqual(fake.writes[-1], encode_voltages([0.0] * 8))
            self.assertEqual(driver._commanded, [0.0] * 8)
            self.assertEqual(driver.state, DriverState.READY)
            self.assertEqual(driver._cancellation_generation, 2)
            driver.set_channel(1, 0.1)
            self.assertAlmostEqual(decode_command(fake.writes[-1])[0], 0.1, places=3)
        finally:
            fake.release_zero_write.set()
            emergency.join(timeout=1.0)
            c.join(timeout=1.0)
            driver.close()


class VoltageFinalFixRound5Tests(unittest.TestCase):
    @staticmethod
    def capture(errors, operation):
        try:
            operation()
        except BaseException as error:
            errors.append(error)

    def test_reader_fault_during_gated_emergency_zero_wins_over_ready_completion(self):
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = Round4VoltageSerial([initial, zeroed])
        driver = VoltageSource(port="COM4", serial_factory=lambda **kwargs: fake).connect()
        fake.gate_zero = True
        emergency_errors = []
        emergency = threading.Thread(
            target=lambda: self.capture(emergency_errors, lambda: driver.zero(emergency=True))
        )
        fault = threading.Thread(
            target=lambda: driver._enter_fault(InstrumentProtocolError("reader fault"))
        )
        try:
            emergency.start()
            self.assertTrue(fake.zero_write_entered.wait(timeout=1.0))
            fault.start()
            with driver._status_changed:
                while driver.state != DriverState.FAULT:
                    self.assertTrue(driver._status_changed.wait(timeout=1.0))
            self.assertTrue(driver._stop_event.is_set())
            fake.release_zero_write.set()
            emergency.join(timeout=0.5)
            fault.join(timeout=0.5)
            self.assertFalse(emergency.is_alive())
            self.assertFalse(fault.is_alive())
            self.assertIsInstance(emergency_errors[0], DeviceFault)
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertTrue(driver._stop_event.is_set())
            self.assertTrue(driver._cancel_event.is_set())
            last_zero = max(index for index, frame in enumerate(fake.writes) if frame == encode_voltages([0.0] * 8))
            self.assertEqual(fake.writes[last_zero + 1 :], [])
            with self.assertRaises(DriverError):
                driver.set_channel(1, 0.1)
        finally:
            fake.release_zero_write.set()
            emergency.join(timeout=1.0)
            fault.join(timeout=1.0)
            assert_voltage_unknown_released(self, driver)


class StatusTransactionObserver:
    """Test-side Condition wrapper that samples only after a transaction exits."""

    def __init__(self, condition, driver, *, generation):
        self._condition = condition
        self._driver = driver
        self._generation = generation
        self.entered = threading.Event()
        self.release = threading.Event()
        self.first_tuple = None
        self.lock_owned_at_snapshot = None

    def __enter__(self):
        self._condition.__enter__()
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        result = self._condition.__exit__(exc_type, exc_value, traceback)
        self.lock_owned_at_snapshot = self._condition._is_owned()
        snapshot = (
            self._driver._cancellation_generation,
            self._driver._cancel_event.is_set(),
            self._driver.state,
            self._driver._stop_event.is_set(),
            self._driver._shutdown_requested,
        )
        if self.first_tuple is None and snapshot[0] >= self._generation:
            self.first_tuple = snapshot
            self.entered.set()
            self.release.wait(timeout=1.0)
        return result

    def __getattr__(self, name):
        return getattr(self._condition, name)


class VoltageAtomicTransitionTests(unittest.TestCase):
    @staticmethod
    def capture(errors, operation):
        try:
            operation()
        except BaseException as error:
            errors.append(error)

    def test_fault_publication_is_one_observable_terminal_transaction(self):
        """A gated emergency must see FAULT, never a live post-generation gap."""
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = Round4VoltageSerial([initial, zeroed])
        driver = VoltageSource(port="COM4", serial_factory=lambda **kwargs: fake).connect()
        observer = StatusTransactionObserver(driver._status_changed, driver, generation=2)
        driver._status_changed = observer
        fake.gate_zero = True
        emergency_errors = []
        emergency = threading.Thread(
            target=lambda: self.capture(emergency_errors, lambda: driver.zero(emergency=True))
        )
        fault = threading.Thread(
            target=lambda: driver._enter_fault(InstrumentProtocolError("reader fault"))
        )
        try:
            emergency.start()
            self.assertTrue(fake.zero_write_entered.wait(timeout=1.0))
            fault.start()
            self.assertTrue(observer.entered.wait(timeout=1.0))
            generation, cancelled, state, stopped, shutdown_requested = observer.first_tuple
            self.assertFalse(observer.lock_owned_at_snapshot)
            self.assertGreaterEqual(generation, 2)
            self.assertTrue(cancelled)
            self.assertEqual(state, DriverState.FAULT)
            self.assertTrue(stopped)
            self.assertFalse(shutdown_requested)
            observer.release.set()
            fake.release_zero_write.set()
            emergency.join(timeout=1.0)
            fault.join(timeout=1.0)
            self.assertFalse(emergency.is_alive())
            self.assertFalse(fault.is_alive())
            self.assertIsInstance(emergency_errors[0], DeviceFault)
            self.assertEqual(driver.state, DriverState.FAULT)
            last_zero = max(index for index, frame in enumerate(fake.writes) if frame == encode_voltages([0.0] * 8))
            self.assertEqual(fake.writes[last_zero + 1 :], [])
            with self.assertRaises(DriverError):
                driver.set_channel(1, 0.1)
            self.assertFalse(hasattr(driver, "_terminal_transition_hook"))
        finally:
            observer.release.set()
            fake.release_zero_write.set()
            emergency.join(timeout=1.0)
            fault.join(timeout=1.0)
            assert_voltage_unknown_released(self, driver)

    def test_close_publication_precedes_lifecycle_wait_and_cannot_leave_ready(self):
        """Closing while lifecycle is occupied must publish CLOSING before waiting."""
        from Code.Utils.voltage import VoltageSource

        initial = make_voltage_frame([1.0] * 8, [0.0] * 8)
        zeroed = make_voltage_frame([0.0] * 8, [0.0] * 8)
        fake = Round4VoltageSerial([initial, zeroed])
        driver = VoltageSource(port="COM4", serial_factory=lambda **kwargs: fake).connect()
        observer = StatusTransactionObserver(driver._status_changed, driver, generation=2)
        driver._status_changed = observer
        fake.gate_zero = True
        emergency_errors = []
        close_errors = []
        emergency = threading.Thread(
            target=lambda: self.capture(emergency_errors, lambda: driver.zero(emergency=True))
        )
        close_thread = threading.Thread(target=lambda: self.capture(close_errors, driver.close))
        lifecycle_held = False
        try:
            driver._lifecycle_lock.acquire()
            lifecycle_held = True
            emergency.start()
            self.assertTrue(fake.zero_write_entered.wait(timeout=1.0))
            close_thread.start()
            self.assertTrue(observer.entered.wait(timeout=1.0))
            snapshot = observer.first_tuple
            self.assertFalse(observer.lock_owned_at_snapshot)
            self.assertGreaterEqual(snapshot[0], 2)
            self.assertTrue(snapshot[1])
            self.assertEqual(snapshot[2], DriverState.CLOSING)
            self.assertFalse(snapshot[3])
            self.assertTrue(snapshot[4])
            observer.release.set()
            fake.release_zero_write.set()
            emergency.join(timeout=1.0)
            self.assertFalse(emergency.is_alive())
            self.assertIsInstance(emergency_errors[0], InstrumentConnectionError)
            with self.assertRaises(DriverError):
                driver.set_channel(1, 0.1)
            driver._lifecycle_lock.release()
            lifecycle_held = False
            close_thread.join(timeout=1.0)
            self.assertFalse(close_thread.is_alive())
            self.assertEqual(close_errors, [])
            self.assertEqual(driver.state, DriverState.DISCONNECTED)
            self.assertFalse(hasattr(driver, "_terminal_transition_hook"))
        finally:
            observer.release.set()
            fake.release_zero_write.set()
            if lifecycle_held:
                driver._lifecycle_lock.release()
            emergency.join(timeout=1.0)
            close_thread.join(timeout=1.0)
            driver.close()


class PortDiscoveryTests(unittest.TestCase):
    def setUp(self):
        self.ports = [
            SimpleNamespace(device="COM4", vid=0x1A86, pid=0x7523, serial_number=None),
            SimpleNamespace(device="COM5", vid=0x10C4, pid=0xEA60, serial_number="GAIN-1"),
            SimpleNamespace(device="COM7", vid=0x1A86, pid=0xEA60, serial_number="DECOY-VID"),
            SimpleNamespace(device="COM8", vid=0x1234, pid=0x7523, serial_number="DECOY-PID"),
            SimpleNamespace(device="COM9", vid=0x10C4, pid=0xEA60, serial_number="OTHER"),
        ]

    def test_finds_unique_vid_pid(self):
        found = find_serial_port(0x1A86, 0x7523, ports_provider=lambda: self.ports)
        self.assertEqual(found, "COM4")

    def test_filters_by_serial_number(self):
        found = find_serial_port(
            0x10C4, 0xEA60, "GAIN-1", ports_provider=lambda: self.ports
        )
        self.assertEqual(found, "COM5")

    def test_rejects_no_match_and_reports_serial(self):
        with self.assertRaises(InstrumentConnectionError) as caught:
            find_serial_port(
                0xFFFF, 0xFFFF, "MISSING", ports_provider=lambda: self.ports
            )
        self.assertIn("MISSING", str(caught.exception))

    def test_rejects_ambiguous_match(self):
        duplicate = self.ports + [
            SimpleNamespace(device="COM6", vid=0x1A86, pid=0x7523, serial_number="DUPLICATE")
        ]
        duplicate[0] = SimpleNamespace(
            device="COM4", vid=0x1A86, pid=0x7523, serial_number="DUPLICATE"
        )
        with self.assertRaises(InstrumentConnectionError) as caught:
            find_serial_port(0x1A86, 0x7523, "DUPLICATE", ports_provider=lambda: duplicate)
        self.assertIn("DUPLICATE", str(caught.exception))


class OsaConnectionTests(unittest.TestCase):
    def make_spectrum(self, wavelengths, powers):
        from Code.Utils.osa import Spectrum

        return Spectrum(
            wavelengths,
            powers,
            "A",
            datetime(2026, 8, 19, tzinfo=timezone.utc),
            "AQ6370",
        )

    def test_direct_spectrum_construction_rejects_invalid_arrays(self):
        invalid_pairs = [
            ([], []),
            ([[1550.0]], [[-30.0]]),
            ([1550.0], [-30.0, -31.0]),
            ([1550.0, float("nan")], [-30.0, -31.0]),
            ([1550.0, 1551.0], [-30.0, float("inf")]),
            ([1550.0, 1550.0], [-30.0, -31.0]),
            ([1551.0, 1550.0], [-30.0, -31.0]),
        ]
        for wavelengths, powers in invalid_pairs:
            with self.subTest(wavelengths=wavelengths, powers=powers):
                with self.assertRaises(InstrumentProtocolError):
                    self.make_spectrum(wavelengths, powers)

    def test_direct_spectrum_owns_immutable_source_storage(self):
        wavelengths = np.array([1550.0, 1551.0])
        powers = np.array([-30.0, -31.0])

        spectrum = self.make_spectrum(wavelengths, powers)
        wavelengths[0] = 1552.0
        powers[0] = -32.0

        np.testing.assert_array_equal(spectrum.wavelength_nm, [1550.0, 1551.0])
        np.testing.assert_array_equal(spectrum.power_dbm, [-30.0, -31.0])
        with self.assertRaises(ValueError):
            spectrum.wavelength_nm[0] = 1600.0
        with self.assertRaises(ValueError):
            spectrum.power_dbm[0] = 0.0
        with self.assertRaises(ValueError):
            spectrum.wavelength_nm.setflags(write=True)
        with self.assertRaises(ValueError):
            spectrum.power_dbm.setflags(write=True)

    def test_spectrum_create_uses_the_same_immutable_construction_path(self):
        from Code.Utils.osa import Spectrum

        wavelengths = np.array([1550.0, 1551.0])
        powers = np.array([-30.0, -31.0])
        spectrum = Spectrum.create(wavelengths, powers, "A", "AQ6370")
        wavelengths[:] = 0.0
        powers[:] = 0.0

        np.testing.assert_array_equal(spectrum.wavelength_nm, [1550.0, 1551.0])
        np.testing.assert_array_equal(spectrum.power_dbm, [-30.0, -31.0])
        with self.assertRaises(ValueError):
            spectrum.wavelength_nm.setflags(write=True)

    def test_constructor_rejects_invalid_timeout_and_retries(self):
        from Code.Utils.osa import AQ6370

        for timeout in (0.0, -1.0, float("nan"), float("inf"), -float("inf")):
            with self.subTest(timeout=timeout):
                with self.assertRaises(ValueError):
                    AQ6370(timeout=timeout)
        for retries in (-1, True, False, 1.5, "2"):
            with self.subTest(retries=retries):
                with self.assertRaises(ValueError):
                    AQ6370(retries=retries)

    def test_spectrum_rejects_mismatched_arrays(self):
        from Code.Utils.osa import Spectrum

        with self.assertRaises(InstrumentProtocolError):
            Spectrum.create([1550.0], [-30.0, -31.0], "A", "AQ6370")

    def test_spectrum_rejects_nonfinite_values(self):
        from Code.Utils.osa import Spectrum

        with self.assertRaises(InstrumentProtocolError):
            Spectrum.create([1550.0, float("nan")], [-30.0, -31.0], "A", "AQ6370")

    def test_spectrum_rejects_empty_arrays(self):
        from Code.Utils.osa import Spectrum

        with self.assertRaises(InstrumentProtocolError):
            Spectrum.create([], [], "A", "AQ6370")

    def test_spectrum_rejects_non_one_dimensional_arrays(self):
        from Code.Utils.osa import Spectrum

        with self.assertRaises(InstrumentProtocolError):
            Spectrum.create([[1550.0]], [[-30.0]], "A", "AQ6370")

    def test_spectrum_rejects_non_increasing_wavelengths(self):
        from Code.Utils.osa import Spectrum

        with self.assertRaises(InstrumentProtocolError):
            Spectrum.create([1550.0, 1550.0], [-30.0, -31.0], "A", "AQ6370")

    def test_spectrum_arrays_are_read_only(self):
        from Code.Utils.osa import Spectrum

        spectrum = Spectrum.create([1550.0, 1551.0], [-30.0, -31.0], "A", "AQ6370")
        with self.assertRaises(ValueError):
            spectrum.wavelength_nm[0] = 1552.0
        with self.assertRaises(ValueError):
            spectrum.power_dbm[0] = -32.0

    def test_spectrum_owns_source_arrays(self):
        from Code.Utils.osa import Spectrum

        wavelengths = np.array([1550.0, 1551.0])
        powers = np.array([-30.0, -31.0])
        spectrum = Spectrum.create(wavelengths, powers, "A", "AQ6370")
        wavelengths[0] = 1552.0
        powers[0] = -32.0
        self.assertEqual(spectrum.wavelength_nm[0], 1550.0)
        self.assertEqual(spectrum.power_dbm[0], -30.0)

    def test_connect_identifies_instrument_without_writing_settings(self):
        from Code.Utils.osa import AQ6370

        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,91V000000,01.00\n"
        manager = Mock()
        manager.open_resource.return_value = resource
        instrument = Mock()
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        )
        driver.connect()
        self.assertEqual(driver.identity, "YOKOGAWA,AQ6370D,91V000000,01.00")
        manager.open_resource.assert_called_once_with("GPIB0::4::INSTR")
        resource.query.assert_called_once_with("*IDN?")
        instrument.initiate_sweep.assert_not_called()
        driver.close()
        resource.close.assert_called_once()
        manager.close.assert_called_once()

    def test_check_osa_uses_verified_lab_address_by_default(self):
        import sys

        from Code.Debugs import check_osa

        kwargs_seen = {}

        class FakeOSA:
            identity = "YOKOGAWA,AQ6370E,901C12907,01.04"

            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, traceback):
                return False

            def acquire(self):
                return SimpleNamespace(
                    wavelength_nm=np.array([1050.0, 1070.0]),
                    power_dbm=np.array([-80.0, -70.0]),
                )

        def factory(**kwargs):
            kwargs_seen.update(kwargs)
            return FakeOSA()

        with (
            unittest.mock.patch.object(check_osa, "AQ6370", factory),
            unittest.mock.patch.object(sys, "argv", ["check_osa", "--sweep"]),
        ):
            self.assertEqual(check_osa.main(), 0)

        self.assertEqual(kwargs_seen["resource_name"], "GPIB0::4::INSTR")
        self.assertEqual(kwargs_seen["timeout"], 30.0)

    def test_connect_logs_success_without_trace_data(self):
        from Code.Utils.osa import AQ6370

        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,SN,FW"
        manager = Mock()
        manager.open_resource.return_value = resource
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: Mock(),
        )
        with self.assertLogs("sil.osa", level="INFO") as captured:
            driver.connect()
        self.assertIn("connected", " ".join(captured.output).lower())
        driver.close()

    def test_connect_failure_logs_and_cleans_up(self):
        from Code.Utils.osa import AQ6370

        resource = Mock()
        resource.query.return_value = "SOME,OTHER,INSTRUMENT"
        manager = Mock()
        manager.open_resource.return_value = resource
        driver = AQ6370(resource_manager_factory=lambda: manager)
        with self.assertLogs("sil.osa", level="ERROR") as captured:
            with self.assertRaises(InstrumentConnectionError):
                driver.connect()
        self.assertIn("connect", " ".join(captured.output).lower())
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        resource.close.assert_called_once_with()
        manager.close.assert_called_once_with()

    def test_keyboard_interrupt_during_identity_query_closes_partial_connection_in_order(self):
        from Code.Utils.osa import AQ6370

        events = []
        resource = Mock()
        resource.query.side_effect = lambda command: (
            events.append(f"query:{command}"),
            (_ for _ in ()).throw(KeyboardInterrupt("identity interrupted")),
        )[1]
        resource.close.side_effect = lambda: events.append("resource.close")
        manager = Mock()
        manager.open_resource.side_effect = lambda name: (
            events.append(f"open:{name}"), resource
        )[1]
        manager.close.side_effect = lambda: events.append("manager.close")
        driver = AQ6370(resource_manager_factory=lambda: manager)

        with self.assertRaisesRegex(KeyboardInterrupt, "identity interrupted"):
            driver.connect()

        self.assertEqual(
            events,
            ["open:GPIB0::4::INSTR", "query:*IDN?", "resource.close", "manager.close"],
        )
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_close_is_idempotent_and_aborts_owned_sweep_before_closing(self):
        from Code.Utils.osa import AQ6370

        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,91V000000,01.00"
        manager = Mock()
        manager.open_resource.return_value = resource
        instrument = Mock()
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        )
        driver.connect()
        with driver._lifecycle_lock:
            driver._sweep_owned = True
        driver.close()
        driver.close()
        instrument.abort.assert_called_once_with()
        resource.close.assert_called_once_with()
        manager.close.assert_called_once_with()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_close_reports_abort_interrupt_after_resource_cleanup(self):
        from Code.Utils.osa import AQ6370

        events = []
        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,SN,FW"
        resource.close.side_effect = lambda: events.append("resource.close")
        manager = Mock()
        manager.open_resource.return_value = resource
        manager.close.side_effect = lambda: events.append("manager.close")
        instrument = Mock()

        def interrupt_abort():
            events.append("instrument.abort")
            raise KeyboardInterrupt("abort interrupted")

        instrument.abort.side_effect = interrupt_abort
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        ).connect()

        with driver._lifecycle_lock:
            driver._sweep_owned = True
        with self.assertRaises(InstrumentConnectionError) as caught:
            driver.close()

        self.assertEqual(events, ["instrument.abort", "resource.close", "manager.close"])
        self.assertIsInstance(caught.exception.__cause__, KeyboardInterrupt)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_resource_interrupt_keeps_manager_owned_until_explicit_retry(self):
        from Code.Utils.osa import AQ6370

        events = []
        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,SN,FW"

        def interrupt_resource_close():
            events.append("resource.close")
            raise KeyboardInterrupt("resource interrupted")

        resource.close.side_effect = interrupt_resource_close
        manager = Mock()
        manager.open_resource.return_value = resource
        manager.close.side_effect = lambda: events.append("manager.close")
        instrument = Mock()
        instrument.abort.side_effect = lambda: events.append("instrument.abort")
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        ).connect()

        with driver._lifecycle_lock:
            driver._sweep_owned = True
        try:
            with self.assertRaises(InstrumentConnectionError) as caught:
                driver.close()
            self.assertIsInstance(caught.exception.__cause__, KeyboardInterrupt)
            self.assertEqual(events, ["instrument.abort", "resource.close"])
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIs(driver._resource, resource)
        finally:
            resource.close.side_effect = None
            driver.close()

    def test_context_manager_closes_when_body_raises(self):
        from Code.Utils.osa import AQ6370

        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,91V000000,01.00"
        manager = Mock()
        manager.open_resource.return_value = resource
        with self.assertRaisesRegex(RuntimeError, "body failure"):
            with AQ6370(
                resource_manager_factory=lambda: manager,
                instrument_factory=lambda opened: Mock(),
            ):
                raise RuntimeError("body failure")
        resource.close.assert_called_once_with()
        manager.close.assert_called_once_with()

    def test_cleanup_errors_do_not_mask_body_error(self):
        from Code.Utils.osa import AQ6370

        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,91V000000,01.00"
        resource.close.side_effect = OSError("resource cleanup failure")
        manager = Mock()
        manager.open_resource.return_value = resource
        instrument = Mock()
        instrument.abort.side_effect = OSError("abort failure")
        manager.close.side_effect = OSError("manager cleanup failure")
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        )
        try:
            with self.assertRaisesRegex(RuntimeError, "body failure"):
                with driver:
                    with driver._lifecycle_lock:
                        driver._sweep_owned = True
                    raise RuntimeError("body failure")
            self.assertEqual(driver.state, DriverState.FAULT)
            self.assertIsNotNone(driver.cleanup_error)
        finally:
            instrument.abort.side_effect = None
            resource.close.side_effect = None
            manager.close.side_effect = None
            driver.close()

    def test_cleanup_interrupt_does_not_mask_context_body_error(self):
        from Code.Utils.osa import AQ6370

        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,SN,FW"
        manager = Mock()
        manager.open_resource.return_value = resource
        instrument = Mock()
        instrument.abort.side_effect = KeyboardInterrupt("cleanup interrupted")
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        )
        with self.assertRaisesRegex(RuntimeError, "body failure"):
            with driver:
                with driver._lifecycle_lock:
                    driver._sweep_owned = True
                raise RuntimeError("body failure")
        resource.close.assert_called_once_with()
        manager.close.assert_called_once_with()

    def test_close_logs_shutdown(self):
        from Code.Utils.osa import AQ6370

        resource = Mock()
        resource.query.return_value = "YOKOGAWA,AQ6370D,SN,FW"
        manager = Mock()
        manager.open_resource.return_value = resource
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: Mock(),
        ).connect()
        with self.assertLogs("sil.osa", level="INFO") as captured:
            driver.close()
        self.assertIn("shutdown", " ".join(captured.output).lower())


class OsaAcquisitionTests(unittest.TestCase):
    def make_driver(self):
        from Code.Utils.osa import AQ6370
        from Code.Debugs.test_osa_read import attach_trace_wire

        resource = Mock()
        attach_trace_wire(resource)
        resource.query.side_effect = ["YOKOGAWA,AQ6370D,SN,FW", "1"]
        manager = Mock()
        manager.open_resource.return_value = resource
        instrument = Mock()
        instrument.get_xdata.return_value = [1.549e-6, 1.550e-6]
        instrument.get_ydata.return_value = [-41.0, -40.0]
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        ).connect()
        self.addCleanup(driver.close)
        return driver, resource, instrument

    def test_acquire_waits_and_converts_metres_to_nanometres(self):
        driver, resource, instrument = self.make_driver()
        spectrum = driver.acquire()
        instrument.initiate_sweep.assert_called_once()
        resource.query.assert_called_with("*OPC?")
        np.testing.assert_allclose(spectrum.wavelength_nm, [1549.0, 1550.0])
        np.testing.assert_allclose(spectrum.power_dbm, [-41.0, -40.0])
        driver.close()

    def test_acquire_aborts_and_retries(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = [TimeoutError("first"), "1"]
        spectrum = driver.acquire()
        self.assertEqual(instrument.initiate_sweep.call_count, 2)
        instrument.abort.assert_called()
        self.assertEqual(len(spectrum.wavelength_nm), 2)
        driver.close()

    def test_acquire_retries_python_timeout_only_until_success(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = [TimeoutError("first"), "1"]

        driver.acquire()

        self.assertEqual(instrument.initiate_sweep.call_count, 2)
        self.assertEqual(instrument.abort.call_count, 1)

    def test_acquire_exhausts_python_timeouts_as_instrument_timeout(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = TimeoutError("always")

        with self.assertRaises(InstrumentTimeoutError) as caught:
            driver.acquire()

        self.assertIsInstance(caught.exception.__cause__, TimeoutError)
        self.assertEqual(instrument.initiate_sweep.call_count, driver.retries + 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_acquire_retries_pyvisa_timeout_status_until_success(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = [VisaIOError(StatusCode.error_timeout), "1"]

        driver.acquire()

        self.assertEqual(instrument.initiate_sweep.call_count, 2)
        self.assertEqual(instrument.abort.call_count, 1)

    def test_acquire_exhausts_pyvisa_timeout_status_as_instrument_timeout(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = VisaIOError(StatusCode.error_timeout)

        with self.assertRaises(InstrumentTimeoutError) as caught:
            driver.acquire()

        self.assertIsInstance(caught.exception.__cause__, VisaIOError)
        self.assertEqual(instrument.initiate_sweep.call_count, driver.retries + 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_acquire_retries_protocol_failure_until_success_without_stale_read(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = ["0", "1"]

        spectrum = driver.acquire()

        self.assertEqual(instrument.initiate_sweep.call_count, 2)
        self.assertEqual(instrument.abort.call_count, 1)
        self.assertEqual(resource.write.call_args_list.count(unittest.mock.call(":TRACe:X? TRA,1,2")), 1)
        self.assertEqual(resource.write.call_args_list.count(unittest.mock.call(":TRACe:Y? TRA,1,2")), 1)
        instrument.get_xdata.assert_not_called()
        instrument.get_ydata.assert_not_called()
        np.testing.assert_array_equal(spectrum.power_dbm, [-41.0, -40.0])

    def test_acquire_preserves_protocol_error_after_retry_exhaustion(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = ["0"] * (driver.retries + 1)

        with self.assertRaises(InstrumentProtocolError):
            driver.acquire()

        self.assertEqual(instrument.initiate_sweep.call_count, driver.retries + 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_acquire_non_timeout_visa_failure_fails_fast_as_connection_error(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = VisaIOError(StatusCode.error_connection_lost)

        with self.assertRaises(InstrumentConnectionError) as caught:
            driver.acquire()

        self.assertIsInstance(caught.exception.__cause__, VisaIOError)
        self.assertEqual(instrument.initiate_sweep.call_count, 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        resource.close.assert_called_once_with()

    def test_acquire_invalid_visa_session_fails_fast_as_connection_error(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = InvalidSession()

        with self.assertRaises(InstrumentConnectionError) as caught:
            driver.acquire()

        self.assertIsInstance(caught.exception.__cause__, InvalidSession)
        self.assertEqual(instrument.initiate_sweep.call_count, 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        resource.close.assert_called_once_with()

    def test_acquire_os_transport_failure_fails_fast_as_connection_error(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = OSError("link down")

        with self.assertRaises(InstrumentConnectionError) as caught:
            driver.acquire()

        self.assertIsInstance(caught.exception.__cause__, OSError)
        self.assertEqual(instrument.initiate_sweep.call_count, 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_acquire_unexpected_internal_error_fails_fast_and_is_unchanged(self):
        driver, resource, instrument = self.make_driver()
        original_write = resource.write.side_effect
        def fail_trace(command):
            if command.startswith(":TRACe:X?"):
                raise AttributeError("driver bug")
            return original_write(command)
        resource.write.side_effect = fail_trace

        with self.assertRaisesRegex(AttributeError, "driver bug"):
            driver.acquire()

        self.assertEqual(instrument.initiate_sweep.call_count, 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        resource.close.assert_called_once_with()

    def test_final_failure_closes_and_raises(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = TimeoutError("always")
        with self.assertRaises(InstrumentTimeoutError):
            driver.acquire()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        resource.close.assert_called_once()

    def test_acquire_non_numeric_wavelength_never_restarts_completed_sweep(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = ["1"] * (driver.retries + 1)
        resource.visalib.overrides[":TRACe:X? TRA,1,2"] = b"1.549e-6,not-a-number\n"
        with self.assertRaises(InstrumentProtocolError):
            driver.acquire()
        self.assertEqual(instrument.initiate_sweep.call_count, 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        resource.close.assert_called_once()

    def test_acquire_non_numeric_power_never_restarts_completed_sweep(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = ["1"] * (driver.retries + 1)
        resource.visalib.overrides[":TRACe:Y? TRA,1,2"] = b"-41,not-a-number\n"
        with self.assertRaises(InstrumentProtocolError):
            driver.acquire()
        self.assertEqual(instrument.initiate_sweep.call_count, 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        resource.close.assert_called_once()

    def test_acquire_rounds_positive_submillisecond_timeout_up_to_one_millisecond(self):
        driver, resource, instrument = self.make_driver()
        # Freeze elapsed time to test VISA rounding, not claim a 100-us real sweep.
        with unittest.mock.patch("Code.Utils.osa.monotonic", return_value=100.):
            driver.acquire(timeout=0.0001)
        self.assertEqual(resource.timeout, 1)
        driver.close()

    def test_acquire_rejects_nonfinite_or_nonpositive_timeout_before_sweep(self):
        for timeout in (0.0, -1.0, float("nan"), float("inf"), -float("inf")):
            with self.subTest(timeout=timeout):
                driver, resource, instrument = self.make_driver()
                with self.assertRaises(ValueError):
                    driver.acquire(timeout=timeout)
                instrument.initiate_sweep.assert_not_called()
                self.assertEqual(driver.state, DriverState.READY)
                driver.close()

    def test_acquire_normalizes_supported_trace_forms(self):
        for supplied, expected in (("A", "A"), ("a", "A"), ("TRA", "A"), ("tra", "A"), ("TRG", "G")):
            with self.subTest(trace=supplied):
                driver, resource, instrument = self.make_driver()
                resource.visalib.meta[":TRACe:ACTive?"] = f"TR{expected}"
                spectrum = driver.acquire(trace=supplied)
                resource.write.assert_any_call(f":TRACe:X? TR{expected},1,2")
                resource.write.assert_any_call(f":TRACe:Y? TR{expected},1,2")
                self.assertEqual(spectrum.trace, expected)
                driver.close()

    def test_acquire_rejects_invalid_trace_before_sweep(self):
        for trace in ("", "TR", "H", "TRH", "AA", "TRACEA"):
            with self.subTest(trace=trace):
                driver, resource, instrument = self.make_driver()
                with self.assertRaises(ValueError):
                    driver.acquire(trace=trace)
                instrument.initiate_sweep.assert_not_called()
                self.assertEqual(driver.state, DriverState.READY)
                driver.close()

    def test_keyboard_interrupt_during_opc_closes_everything_in_order(self):
        from Code.Utils.osa import AQ6370
        from Code.Debugs.test_osa_read import attach_trace_wire

        events = []
        resource = Mock()
        attach_trace_wire(resource)

        def query(command):
            events.append(f"query:{command}")
            if command == "*IDN?":
                return "YOKOGAWA,AQ6370D,SN,FW"
            raise KeyboardInterrupt("opc interrupted")

        resource.query.side_effect = query
        resource.close.side_effect = lambda: events.append("resource.close")
        manager = Mock()
        manager.open_resource.return_value = resource
        manager.close.side_effect = lambda: events.append("manager.close")
        instrument = Mock()
        instrument.initiate_sweep.side_effect = lambda: events.append("sweep")
        instrument.abort.side_effect = lambda: events.append("abort")
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        ).connect()
        events.clear()

        with self.assertRaisesRegex(KeyboardInterrupt, "opc interrupted"):
            driver.acquire()

        self.assertEqual(
            events,
            ["sweep", "query:*OPC?", "abort", "resource.close", "manager.close"],
        )
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_keyboard_interrupt_setting_acquisition_timeout_closes_everything(self):
        from Code.Utils.osa import AQ6370

        events = []

        class Resource:
            def __init__(self):
                self.timeout_assignments = 0

            @property
            def timeout(self):
                return None

            @timeout.setter
            def timeout(self, value):
                self.timeout_assignments += 1
                events.append(f"timeout:{value}")
                if self.timeout_assignments == 2:
                    raise KeyboardInterrupt("timeout setting interrupted")

            def query(self, command):
                return "YOKOGAWA,AQ6370D,SN,FW"

            def close(self):
                events.append("resource.close")

        resource = Resource()
        manager = Mock()
        manager.open_resource.return_value = resource
        manager.close.side_effect = lambda: events.append("manager.close")
        instrument = Mock()
        instrument.abort.side_effect = lambda: events.append("abort")
        driver = AQ6370(
            resource_manager_factory=lambda: manager,
            instrument_factory=lambda opened: instrument,
        ).connect()
        events.clear()

        with self.assertRaisesRegex(KeyboardInterrupt, "timeout setting interrupted"):
            driver.acquire(timeout=1.0)

        # No sweep was attempted: aborting would affect a panel-owned measurement.
        self.assertEqual(events, ["timeout:1000", "resource.close", "manager.close"])
        instrument.initiate_sweep.assert_not_called()
        instrument.abort.assert_not_called()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_acquisition_logs_attempt_retry_and_final_fault_without_trace_arrays(self):
        driver, resource, instrument = self.make_driver()
        resource.query.side_effect = TimeoutError("always")

        with self.assertLogs("sil.osa", level="INFO") as captured:
            with self.assertRaises(InstrumentTimeoutError):
                driver.acquire()

        log_text = " ".join(captured.output).lower()
        self.assertIn("attempt", log_text)
        self.assertIn("retry", log_text)
        self.assertIn("fault", log_text)
        self.assertNotIn("1549", log_text)
        self.assertNotIn("-41", log_text)


class _FakeIntegrationOSA:
    def __init__(self, events):
        self.events = events
        self.identity = "FAKE-OSA"

    def __enter__(self):
        self.events.append("osa.enter")
        return self

    def __exit__(self, exc_type, exc, tb):
        self.events.append("osa.exit")

    def acquire(self):
        self.events.append("osa.acquire")
        return SimpleNamespace(wavelength_nm=[1550.0, 1551.0])


class _FakeIntegrationVoltage:
    def __init__(self, events, fail=False):
        self.events = events
        self.fail = fail

    def __enter__(self):
        self.events.append("voltage.enter")
        return self

    def __exit__(self, exc_type, exc, tb):
        self.events.append("voltage.exit")

    def read_status(self):
        self.events.append("voltage.status")
        if self.fail:
            raise RuntimeError("voltage failed")
        return SimpleNamespace(voltage_v=(0.0,), current_ma=(0.0,))


class _FakeIntegrationGain:
    def __init__(self, events):
        self.events = events
        self._status = SimpleNamespace(
            temperature_c=25.0,
            target_c=25.0,
            tec_enabled=False,
            current_ma=0.0,
            current_enabled=False,
        )

    @property
    def status(self):
        self.events.append("gain.status")
        return self._status

    def __enter__(self):
        self.events.append("gain.enter")
        return self

    def __exit__(self, exc_type, exc, tb):
        self.events.append("gain.exit")


class IntegrationTests(unittest.TestCase):
    def test_confirmation_decline_does_not_construct_any_factory(self):
        import builtins
        import sys

        from Code.Debugs import check_all

        constructors = []

        def forbidden_factory(*args, **kwargs):
            constructors.append((args, kwargs))
            raise AssertionError("factory must not be constructed after cancellation")

        with (
            unittest.mock.patch.object(check_all, "AQ6370", forbidden_factory),
            unittest.mock.patch.object(check_all, "VoltageSource", forbidden_factory),
            unittest.mock.patch.object(check_all, "GainDriver", forbidden_factory),
            unittest.mock.patch.object(sys, "argv", ["check_all"]),
            unittest.mock.patch.object(builtins, "input", return_value="N"),
        ):
            self.assertEqual(check_all.main(), 0)
        self.assertEqual(constructors, [])

    def test_confirmation_eof_and_keyboard_interrupt_do_not_construct_any_factory(self):
        import builtins
        import sys

        from Code.Debugs import check_all

        for interruption in (EOFError(), KeyboardInterrupt()):
            constructors = []

            def forbidden_factory(*args, **kwargs):
                constructors.append((args, kwargs))
                raise AssertionError("factory must not be constructed after cancellation")

            with (
                unittest.mock.patch.object(check_all, "AQ6370", forbidden_factory),
                unittest.mock.patch.object(check_all, "VoltageSource", forbidden_factory),
                unittest.mock.patch.object(check_all, "GainDriver", forbidden_factory),
                unittest.mock.patch.object(sys, "argv", ["check_all"]),
                unittest.mock.patch.object(builtins, "input", side_effect=interruption),
            ):
                self.assertEqual(check_all.main(), 0)
            self.assertEqual(constructors, [])

    def test_confirmation_y_runs_in_order_and_preserves_default_gain_serial(self):
        import builtins
        import sys

        from Code.Debugs import check_all

        events = []
        kwargs_seen = {}

        def osa_factory(**kwargs):
            events.append("osa.construct")
            kwargs_seen["osa"] = kwargs
            return _FakeIntegrationOSA(events)

        def voltage_factory(**kwargs):
            events.append("voltage.construct")
            kwargs_seen["voltage"] = kwargs
            return _FakeIntegrationVoltage(events)

        def gain_factory(**kwargs):
            events.append("gain.construct")
            kwargs_seen["gain"] = kwargs
            return _FakeIntegrationGain(events)

        with (
            unittest.mock.patch.object(check_all, "AQ6370", osa_factory),
            unittest.mock.patch.object(check_all, "VoltageSource", voltage_factory),
            unittest.mock.patch.object(check_all, "GainDriver", gain_factory),
            unittest.mock.patch.object(sys, "argv", ["check_all"]),
            unittest.mock.patch.object(builtins, "input", return_value=" y "),
        ):
            self.assertEqual(check_all.main(), 0)

        self.assertEqual(
            events,
            [
                "osa.construct", "osa.enter", "osa.acquire", "osa.exit",
                "voltage.construct", "voltage.enter", "voltage.status", "voltage.exit",
                "gain.construct", "gain.enter", "gain.status", "gain.exit",
            ],
        )
        self.assertEqual(kwargs_seen["gain"], {"port": None})
        self.assertEqual(
            kwargs_seen["osa"],
            {"resource_name": "GPIB0::4::INSTR", "timeout": 30.0},
        )

    def test_explicit_gain_serial_number_is_forwarded(self):
        import builtins
        import sys

        from Code.Debugs import check_all

        kwargs_seen = {}

        class EmptyCheck:
            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, tb):
                return False

        def capture_osa(**kwargs):
            kwargs_seen["osa"] = kwargs
            return _FakeIntegrationOSA([])

        def capture_voltage(**kwargs):
            kwargs_seen["voltage"] = kwargs
            return _FakeIntegrationVoltage([])

        def capture_gain(**kwargs):
            kwargs_seen["gain"] = kwargs
            return _FakeIntegrationGain([])

        with (
            unittest.mock.patch.object(check_all, "AQ6370", capture_osa),
            unittest.mock.patch.object(check_all, "VoltageSource", capture_voltage),
            unittest.mock.patch.object(check_all, "GainDriver", capture_gain),
            unittest.mock.patch.object(
                sys,
                "argv",
                ["check_all", "--gain-serial-number", "SERIAL-1"],
            ),
            unittest.mock.patch.object(builtins, "input", return_value="y"),
        ):
            self.assertEqual(check_all.main(), 0)

        self.assertEqual(kwargs_seen["gain"], {"port": None, "usb_serial": "SERIAL-1"})

    def test_safe_check_order_and_cleanup(self):
        from Code.Debugs.check_all import run_checks

        events = []
        results = run_checks(
            osa_factory=lambda: _FakeIntegrationOSA(events),
            voltage_factory=lambda: _FakeIntegrationVoltage(events),
            gain_factory=lambda: _FakeIntegrationGain(events),
        )
        self.assertEqual(events[:6], [
            "osa.enter", "osa.acquire", "osa.exit",
            "voltage.enter", "voltage.status", "voltage.exit",
        ])
        self.assertEqual(events[6:], ["gain.enter", "gain.status", "gain.exit"])
        self.assertEqual(set(results), {"osa", "voltage", "gain"})

    def test_later_failure_does_not_skip_cleanup(self):
        from Code.Debugs.check_all import run_checks

        events = []
        with self.assertRaises(RuntimeError):
            run_checks(
                osa_factory=lambda: _FakeIntegrationOSA(events),
                voltage_factory=lambda: _FakeIntegrationVoltage(events, fail=True),
                gain_factory=lambda: _FakeIntegrationGain(events),
            )
        self.assertIn("osa.exit", events)
        self.assertIn("voltage.exit", events)
        self.assertNotIn("gain.enter", events)


if __name__ == "__main__":
    unittest.main()
