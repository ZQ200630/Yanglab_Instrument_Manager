"""Actual AQ6370 lifetime and read transactions over finite byte fixtures."""

import re
import struct
import threading
import time
import unittest
from contextlib import contextmanager
from unittest.mock import Mock

import numpy as np
from pyvisa.constants import StatusCode

from Code.Utils.common import DriverError, DriverState, InstrumentConnectionError, InstrumentProtocolError
from Code.Utils.osa import AQ6370


class TraceWire:
    """A canned byte transport, not an instrument/output model."""

    def __init__(self, fmt="ASCII", *, points=2, fragment=4096):
        self.format = fmt
        self.x = (np.linspace(1.55e-6, 1.75e-6, points) if points != 2 else
                  np.array([0.00000095367431640625, 0.000001430511474609375]))
        self.y = np.full(points, -40.)
        if points == 2:
            self.y[1] = -30.
        self.meta = {
            ":FORMat:DATA?": fmt,
            ":DISPlay:TRACe:Y1:SCALe:SPACing?": "0",
            ":DISPlay:TRACe:Y1:SCALe:UNIT?": "0",
            ":UNIT:X?": "0", ":TRACe:DATA:SNUMber? TRA": str(points),
            ":TRACe:ATTRibute:TRA?": "0", ":TRACe:ACTive?": "TRA",
            ":SENSe:WAVelength:CENTer?": "1.55e-6",
            ":SENSe:WAVelength:SPAN?": "2e-9",
            ":SENSe:BWIDth:RESolution?": "2e-11", ":INITiate:SMODe?": "2",
        }
        for trace in "BCDEFG":
            self.meta[f":TRACe:DATA:SNUMber? TR{trace}"] = str(points)
            self.meta[f":TRACe:ATTRibute:TR{trace}?"] = "0"
        self.visalib = self
        self.session = 1
        self.timeout = 1000
        self.read_termination = "\n"
        self.fragment = fragment
        self.commands = []
        self.actions = []
        self.read_sizes = []
        self.counts = {}
        self.overrides = {}
        self.buffer = b""
        self.closed = False
        self.in_read = False
        self.close_overlap = False
        self.block_read = False
        self.read_entered = threading.Event()
        self.release_read = threading.Event()
        self.block_init = False
        self.init_entered = threading.Event()
        self.release_init = threading.Event()
        self.pending = ""
        self.read_delay = 0.

    def query(self, command):
        self.commands.append(command)
        if command == "*IDN?":
            return "YOKOGAWA,AQ6370E,SN,FW"
        if command == "*OPC?":
            return "1"
        raise AssertionError(f"Unexpected unbounded query: {command}")

    def write(self, command):
        self.commands.append(command)
        self.pending = command
        if command == ":INITiate:IMMediate":
            if self.block_init:
                self.init_entered.set()
                if not self.release_init.wait(4):
                    raise AssertionError("init fixture gate not released")
            self.actions.append("INIT")
            return
        if command == ":ABORt":
            self.actions.append("ABORT")
            return
        self.counts[command] = self.counts.get(command, 0) + 1
        if command in self.overrides:
            self.buffer = self.overrides[command]
        elif command == ":TRACe:ATTRibute?":
            active = self.meta[":TRACe:ACTive?"]
            self.buffer = str(self.meta[f":TRACe:ATTRibute:{active}?"]).encode("ascii") + b"\n"
        elif command in self.meta:
            value = self.meta[command]
            if callable(value):
                value = value(self.counts[command])
            self.buffer = str(value).encode("ascii") + b"\n"
        else:
            match = re.fullmatch(r":TRACe:([XY])\? TR[A-G],(\d+),(\d+)", command)
            if match is None:
                raise AssertionError(f"Unexpected command or setter: {command}")
            axis, first, last = match.group(1), int(match.group(2)), int(match.group(3))
            if not 1 <= first <= last <= len(self.x) or last - first + 1 > 1024:
                raise AssertionError("Trace range is not bounded")
            values = (self.x if axis == "X" else self.y)[first - 1:last]
            if self.format == "ASCII":
                self.buffer = ",".join(f"{value:.17g}" for value in values).encode("ascii") + b"\n"
            else:
                code = "f" if self.format == "REAL,32" else "d"
                payload = struct.pack("<" + code * len(values), *values)
                size = str(len(payload)).encode("ascii")
                self.buffer = b"#" + str(len(size)).encode("ascii") + size + payload + b"\n"

    def read(self, session, count):
        if session != 1 or self.closed or not 1 <= count <= 4096:
            raise AssertionError("Invalid low-level read")
        self.read_sizes.append(count)
        self.in_read = True
        try:
            if self.read_delay:
                time.sleep(self.read_delay)
            if self.block_read and self.pending.startswith(":TRACe:Y?"):
                self.read_entered.set()
                if not self.release_read.wait(4):
                    raise AssertionError("read fixture gate not released")
            size = min(count, self.fragment, len(self.buffer))
            status = StatusCode.success if size == len(self.buffer) else StatusCode.success_max_count_read
            if self.read_termination is not None:
                term = self.buffer[:size].find(self.read_termination.encode("ascii"))
                if term != -1:
                    size = term + 1
                    status = StatusCode.success_termination_character_read
            data, self.buffer = self.buffer[:size], self.buffer[size:]
            return data, status
        finally:
            self.in_read = False

    def close(self):
        self.close_overlap = self.in_read
        self.closed = True


class SweepBoundary:
    def __init__(self, wire):
        self.wire = wire

    def initiate_sweep(self):
        self.wire.write(":INITiate:IMMediate")

    def abort(self):
        self.wire.write(":ABORt")

    def get_xdata(self, trace):
        return self.wire.x

    def get_ydata(self, trace):
        return self.wire.y


def attach_trace_wire(resource):
    """Keep legacy actual-driver tests at the bounded VISA byte boundary."""
    wire = TraceWire()
    wire.meta[":INITiate:SMODe?"] = "1"
    wire.x[:] = [1.549e-6, 1.550e-6]
    wire.y[:] = [-41., -40.]
    resource.visalib, resource.session = wire, 1
    resource.read_termination = "\n"
    resource.write.side_effect = wire.write
    return wire


class OsaReadTests(unittest.TestCase):
    def connected(self, fmt="ASCII", *, points=2, wire_factory=TraceWire, **options):
        wire = wire_factory(fmt, points=points)
        manager = Mock()
        manager.open_resource.return_value = wire
        driver = AQ6370(resource_manager_factory=lambda: manager,
                        instrument_factory=SweepBoundary, **options).connect()
        self.addCleanup(self.cleanup, driver, wire)
        return driver, wire, manager

    @staticmethod
    def cleanup(driver, wire):
        wire.release_init.set()
        wire.release_read.set()
        try:
            driver.close()
        except DriverError:
            pass

    def read(self, driver, *args, **kwargs):
        self.assertTrue(callable(getattr(driver, "read_trace", None)), "Existing-trace read API is missing")
        return driver.read_trace(*args, **kwargs)

    @contextmanager
    def case(self, *args, **kwargs):
        driver, wire, manager = self.connected(*args, **kwargs)
        try:
            yield driver, wire, manager
        finally:
            self.cleanup(driver, wire)

    def test_idle_connect_close_preserves_panel_measurement(self):
        driver, wire, manager = self.connected()
        driver.close()
        self.assertEqual(wire.commands, ["*IDN?"])
        self.assertEqual(wire.actions, [])
        self.assertFalse(driver.has_resource_responsibility)

    def test_nonactive_trace_is_rejected_before_attribute_query_can_switch_panel(self):
        driver, wire, manager = self.connected()
        wire.meta[":TRACe:ACTive?"] = "TRB"
        with self.assertRaises(InstrumentProtocolError):
            self.read(driver, "A")
        self.assertFalse(any(c.startswith(":TRACe:ATTRibute") for c in wire.commands))
        self.assertFalse(any(c.startswith(":TRACe:X?") for c in wire.commands))
        self.assertEqual(wire.meta[":TRACe:ACTive?"], "TRB")
        self.assertEqual(wire.actions, [])

    def test_frequency_context_is_rejected_without_mislabeled_metadata_or_data(self):
        driver, wire, manager = self.connected()
        wire.meta[":UNIT:X?"] = "1"
        wire.meta[":SENSe:WAVelength:CENTer?"] = "193414489032258.06"
        wire.meta[":SENSe:WAVelength:SPAN?"] = "250000000000"
        wire.meta[":SENSe:BWIDth:RESolution?"] = "2500000000"
        with self.assertRaises(InstrumentProtocolError):
            self.read(driver)
        self.assertNotIn(":SENSe:WAVelength:CENTer?", wire.commands)
        self.assertFalse(any(c.startswith(":TRACe:X?") for c in wire.commands))
        self.assertEqual(wire.actions, [])
        self.assertFalse(driver.has_resource_responsibility)

    def test_existing_repeat_trace_formats_and_raw_units(self):
        for fmt in ("ASCII", "REAL,32", "REAL,64"):
            with self.subTest(fmt=fmt), self.case(fmt) as (driver, wire, manager):
                # This literal f32 contains LF inside its payload, not an EOI.
                wire.y[0] = -34.5
                capture = self.read(driver, "TRA")
                np.testing.assert_array_equal(capture.wavelength_nm, [953.67431640625, 1430.511474609375])
                np.testing.assert_array_equal(capture.native_values, [-34.5, -30.])
                self.assertEqual(capture.native_unit, "dBm")
                self.assertEqual(capture.context_before.sweep_mode, 2)
                self.assertEqual(capture.context_before.transfer_format, fmt)
                self.assertEqual(capture.consistency, "unproven")
                self.assertIn(":TRACe:ATTRibute?", wire.commands)
                self.assertNotIn(":TRACe:ATTRibute:TRA?", wire.commands)
                self.assertEqual(wire.read_termination, "\n")
                self.assertEqual(wire.timeout, 30000)
                driver.close()
                self.assertEqual(wire.actions, [])
                self.assertTrue(all(command == "*IDN?" or "?" in command for command in wire.commands))

    def test_absolute_watts_are_kept_separate_from_dbm_compatibility(self):
        driver, wire, manager = self.connected()
        wire.meta[":DISPlay:TRACe:Y1:SCALe:SPACing?"] = "1"
        wire.meta[":DISPlay:TRACe:Y1:SCALe:UNIT?"] = "1"
        wire.y[:] = [.001, .01]
        capture = self.read(driver)
        np.testing.assert_array_equal(capture.native_values, [.001, .01])
        np.testing.assert_allclose(capture.power_dbm, [0, 10], rtol=0, atol=1e-12)
        self.assertEqual(capture.native_unit, "W")

    def test_full_model_trace_uses_consecutive_bounded_ranges(self):
        driver, wire, manager = self.connected(points=200001)
        capture = self.read(driver)
        self.assertEqual(len(capture.wavelength_nm), 200001)
        self.assertAlmostEqual(capture.wavelength_nm[0], 1550.)
        self.assertAlmostEqual(capture.wavelength_nm[-1], 1750.)
        for axis in ("X", "Y"):
            queries = [command for command in wire.commands if command.startswith(f":TRACe:{axis}?")]
            self.assertEqual(len(queries), 196)
            self.assertEqual(queries[0], f":TRACe:{axis}? TRA,1,1024")
            self.assertEqual(queries[-1], f":TRACe:{axis}? TRA,199681,200001")
            previous = 0
            for query in queries:
                first, last = map(int, query.rsplit(" ", 1)[1].split(",")[1:])
                self.assertEqual(first, previous + 1)
                self.assertLessEqual(last - first + 1, 1024)
                previous = last
        self.assertLessEqual(max(wire.read_sizes), 4096)

    def test_changed_context_is_discarded_without_retry_or_abort(self):
        for command, changed in ((":FORMat:DATA?", "REAL,64"),
                                 (":TRACe:DATA:SNUMber? TRA", "3"),
                                 (":DISPlay:TRACe:Y1:SCALe:UNIT?", "1"),
                                 (":SENSe:WAVelength:CENTer?", "1.56e-6")):
            with self.subTest(command=command), self.case() as (driver, wire, manager):
                before = wire.meta[command]
                wire.meta[command] = lambda count: before if count == 1 else changed
                with self.assertRaises(InstrumentProtocolError):
                    self.read(driver)
                self.assertEqual(wire.actions, [])
                self.assertEqual(wire.commands.count(":TRACe:X? TRA,1,2"), 1)
                self.assertEqual(wire.read_termination, "\n")
                self.assertFalse(driver.has_resource_responsibility)

    def test_unsupported_interpretation_rejects_before_trace_queries(self):
        for command, value in ((":DISPlay:TRACe:Y1:SCALe:UNIT?", "2"),
                               (":TRACe:ATTRibute:TRA?", "5"),
                               (":TRACe:DATA:SNUMber? TRA", "0")):
            with self.subTest(command=command), self.case() as (driver, wire, manager):
                wire.meta[command] = value
                with self.assertRaises(InstrumentProtocolError):
                    self.read(driver)
                self.assertFalse(any(c.startswith(":TRACe:X?") for c in wire.commands))
                self.assertEqual(wire.actions, [])

    def test_bad_reply_has_no_complete_or_retried_capture(self):
        for reply in (b"#0payload", b"-40,", b"1" * 65537):
            with self.subTest(reply_length=len(reply)), self.case() as (driver, wire, manager):
                wire.overrides[":TRACe:Y? TRA,1,2"] = reply
                with self.assertRaises(InstrumentProtocolError):
                    self.read(driver)
                self.assertEqual(wire.commands.count(":TRACe:Y? TRA,1,2"), 1)
                self.assertEqual(wire.actions, [])
                self.assertEqual(wire.read_termination, "\n")
                self.assertTrue(wire.closed)

    def test_read_deadline_covers_metadata_and_restores_local_settings(self):
        driver, wire, manager = self.connected()
        wire.read_delay = .004
        before = time.monotonic()
        with self.assertRaises(DriverError):
            self.read(driver, timeout=.006)
        self.assertLess(time.monotonic() - before, .5)
        self.assertEqual(wire.read_termination, "\n")
        self.assertEqual(wire.timeout, 30000)
        self.assertEqual(wire.actions, [])
        self.assertFalse(driver.has_resource_responsibility)

    def test_bad_read_arguments_fail_before_any_query(self):
        driver, wire, manager = self.connected()
        for trace, timeout in (("A;:ABORt", 1), ("H", 1), ("A", 0),
                               ("A", True), ("A", float("nan"))):
            with self.subTest(trace=trace, timeout=timeout):
                with self.assertRaises(ValueError):
                    self.read(driver, trace, timeout)
        self.assertEqual(wire.commands, ["*IDN?"])
        self.assertEqual(driver.state, DriverState.READY)

    def test_local_restore_failure_preserves_primary_and_attempts_both_properties(self):
        # A disconnected VISA session may reject local-property restoration.
        # Dropping the primary read error or skipping timeout restoration is a bug.
        class RestoreWire(TraceWire):
            def __setattr__(self, name, value):
                failure = getattr(self, "restore_failure", None)
                if name == "read_termination" and value == "\n" and failure is not None:
                    raise failure
                super().__setattr__(name, value)

        for malformed in (True, False):
            with self.subTest(malformed=malformed), self.case(wire_factory=RestoreWire) as (driver, wire, manager):
                failure = OSError("VISA property restore failed")
                wire.restore_failure = failure
                if malformed:
                    wire.overrides[":TRACe:Y? TRA,1,2"] = b"-40,"
                    expected = InstrumentProtocolError
                else:
                    expected = InstrumentConnectionError
                with self.assertRaises(DriverError) as caught:
                    self.read(driver, timeout=.3)
                self.assertIsInstance(caught.exception, expected)
                if malformed:
                    self.assertEqual(caught.exception.local_restore_errors, (failure,))
                else:
                    self.assertIs(caught.exception.__cause__, failure)
                self.assertEqual(wire.timeout, 30000)
                self.assertEqual(wire.actions, [])
                self.assertFalse(driver.has_resource_responsibility)

    def test_close_retains_blocked_read_and_prevents_late_capture(self):
        driver, wire, manager = self.connected(close_timeout=.04)
        self.assertTrue(callable(getattr(driver, "read_trace", None)), "Existing-trace read API is missing")
        wire.block_read = True
        results, errors = [], []
        def read():
            try:
                results.append(driver.read_trace())
            except BaseException as error:
                errors.append(error)
        thread = threading.Thread(target=read)
        thread.start()
        try:
            self.assertTrue(wire.read_entered.wait(1))
            with self.assertRaises(DriverError):
                driver.close()
            self.assertFalse(wire.closed)
            self.assertTrue(driver.has_resource_responsibility)
            self.assertEqual(wire.actions, [])
        finally:
            wire.release_read.set()
            thread.join(2)
        self.assertFalse(thread.is_alive())
        self.assertEqual(results, [])
        self.assertIsInstance(errors[0], InstrumentConnectionError)
        driver.close()
        self.assertFalse(wire.close_overlap)
        self.assertFalse(driver.has_resource_responsibility)

    def test_close_cannot_abort_then_allow_a_late_sweep_start(self):
        driver, wire, manager = self.connected(close_timeout=.04)
        wire.meta[":INITiate:SMODe?"] = "1"
        wire.block_init = True
        errors = []
        def acquire():
            try:
                driver.acquire()
            except BaseException as error:
                errors.append(error)
        thread = threading.Thread(target=acquire)
        thread.start()
        try:
            self.assertTrue(wire.init_entered.wait(1))
            with self.assertRaises(DriverError):
                driver.close()
        finally:
            wire.release_init.set()
            thread.join(2)
            driver.close()
        self.assertFalse(thread.is_alive())
        self.assertEqual(wire.actions, ["INIT", "ABORT"])
        self.assertIsInstance(errors[0], InstrumentConnectionError)
        self.assertFalse(driver.has_resource_responsibility)

    def test_acquire_refuses_panel_repeat_without_start_or_abort(self):
        driver, wire, manager = self.connected()
        with self.assertRaises(InstrumentProtocolError):
            driver.acquire()
        self.assertEqual(wire.actions, [])
        self.assertTrue(all(c == "*IDN?" or "?" in c for c in wire.commands))
        self.assertFalse(driver.has_resource_responsibility)

    def test_acquire_nonactive_trace_never_replaces_active_trace_with_a_sweep(self):
        driver, wire, manager = self.connected()
        wire.meta[":INITiate:SMODe?"] = "1"
        with self.assertRaises(InstrumentProtocolError):
            driver.acquire("B")
        self.assertEqual(wire.actions, [])
        self.assertNotIn("*OPC?", wire.commands)
        self.assertEqual(wire.meta[":TRACe:ACTive?"], "TRA")
        self.assertFalse(driver.has_resource_responsibility)

    def test_mode_change_at_init_keeps_sweep_owned_until_abort_without_retry(self):
        class ChangingModeWire(TraceWire):
            def write(self, command):
                super().write(command)
                if command == ":INITiate:IMMediate":
                    self.meta[":INITiate:SMODe?"] = "2"

        for mode in ("1", "3"):
            with self.subTest(mode=mode), self.case(wire_factory=ChangingModeWire) as (driver, wire, manager):
                wire.meta[":INITiate:SMODe?"] = mode
                with self.assertRaises(InstrumentProtocolError):
                    driver.acquire()
                self.assertEqual(wire.actions, ["INIT", "ABORT"])
                self.assertFalse(any(c.startswith(":TRACe:X?") for c in wire.commands))
                self.assertFalse(driver.has_resource_responsibility)

    def test_invalid_post_opc_mode_aborts_owned_sweep_without_restarting(self):
        driver, wire, manager = self.connected()
        wire.meta[":INITiate:SMODe?"] = lambda count: "1" if count == 1 else "invalid"
        with self.assertRaises(InstrumentProtocolError):
            driver.acquire()
        self.assertEqual(wire.actions, ["INIT", "ABORT"])
        self.assertFalse(driver.has_resource_responsibility)

    def test_single_and_auto_acquire_use_validated_native_absolute_units(self):
        for mode in ("1", "3"):
            for watts in (False, True):
                with self.subTest(mode=mode, watts=watts), self.case() as (driver, wire, manager):
                    wire.meta[":INITiate:SMODe?"] = mode
                    if watts:
                        wire.meta[":DISPlay:TRACe:Y1:SCALe:SPACing?"] = "1"
                        wire.meta[":DISPlay:TRACe:Y1:SCALe:UNIT?"] = "1"
                        wire.y[:] = [.001, .01]
                    capture = driver.acquire()
                    np.testing.assert_allclose(capture.power_dbm, [0., 10.] if watts else [-40., -30.], atol=1e-12)
                    np.testing.assert_array_equal(capture.wavelength_nm, [953.67431640625, 1430.511474609375])
                    self.assertIn(":TRACe:Y? TRA,1,2", wire.commands)
                    driver.close()
                    self.assertEqual(wire.actions, ["INIT"])

    def test_completed_sweep_data_failure_never_starts_replacement(self):
        for failure in ("malformed", "density", "zero_w"):
            with self.subTest(failure=failure), self.case() as (driver, wire, manager):
                wire.meta[":INITiate:SMODe?"] = "1"
                if failure == "malformed":
                    wire.overrides[":TRACe:Y? TRA,1,2"] = b"-40,"
                elif failure == "density":
                    wire.meta[":DISPlay:TRACe:Y1:SCALe:UNIT?"] = "2"
                else:
                    wire.meta[":DISPlay:TRACe:Y1:SCALe:SPACing?"] = "1"
                    wire.meta[":DISPlay:TRACe:Y1:SCALe:UNIT?"] = "1"
                    wire.y[:] = [0, .001]
                with self.assertRaises(InstrumentProtocolError):
                    driver.acquire()
                self.assertEqual(wire.actions, ["INIT"])
                self.assertTrue(wire.closed)

    def test_native_acquire_preserves_zero_watts_and_read_provenance_without_second_transfer(self):
        driver, wire, manager = self.connected()
        wire.meta[":INITiate:SMODe?"] = "1"
        wire.meta[":DISPlay:TRACe:Y1:SCALe:SPACing?"] = "1"
        wire.meta[":DISPlay:TRACe:Y1:SCALe:UNIT?"] = "1"
        wire.y[:] = [0., .001]
        self.assertTrue(callable(getattr(driver, "acquire_trace", None)), "Native acquisition API is missing")
        capture = driver.acquire_trace()
        np.testing.assert_array_equal(capture.native_values, [0., .001])
        self.assertEqual(capture.native_unit, "W")
        self.assertIsNone(capture.power_dbm)
        self.assertEqual(capture.context_before, capture.context_after)
        self.assertIsNotNone(capture.read_started_at.tzinfo)
        self.assertEqual(capture.consistency, "unproven")
        self.assertEqual(wire.counts[":TRACe:X? TRA,1,2"], 1)
        self.assertEqual(wire.counts[":TRACe:Y? TRA,1,2"], 1)
        driver.close()
        self.assertEqual(wire.actions, ["INIT"])

    def test_native_acquire_keeps_repeat_and_nonactive_trace_start_gates(self):
        for mode, trace in (("2", "A"), ("1", "B")):
            with self.subTest(mode=mode, trace=trace), self.case() as (driver, wire, manager):
                wire.meta[":INITiate:SMODe?"] = mode
                self.assertTrue(callable(getattr(driver, "acquire_trace", None)), "Native acquisition API is missing")
                with self.assertRaises(InstrumentProtocolError):
                    driver.acquire_trace(trace)
                self.assertEqual(wire.actions, [])
                self.assertFalse(driver.has_resource_responsibility)


if __name__ == "__main__":
    unittest.main()
