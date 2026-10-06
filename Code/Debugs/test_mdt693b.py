from __future__ import annotations

import math
import io
import sys
import threading
import time
import unittest
from collections.abc import Iterable
from contextlib import redirect_stderr, redirect_stdout
from dataclasses import replace
from types import SimpleNamespace
from unittest.mock import patch

import serial

from Code.Utils.common import (
    DeviceFault,
    DriverState,
    InstrumentCapabilityError,
    InstrumentConnectionError,
    InstrumentProtocolError,
    InstrumentSafetyError,
    InstrumentTimeoutError,
)

from Code.Utils.mdt693b import Axis, AxisState, MDTStatus, RotaryMode, VoltageLimit
from Code.Debugs import check_mdt


class FakeMDTSerial:
    """Byte-accurate serial transport used solely by MDT protocol tests."""

    def __init__(
        self,
        chunks: Iterable[bytes] = (),
        *,
        one_byte_reads: bool = True,
        gated_reads: int = 0,
        short_write: int | None = None,
        write_error: BaseException | None = None,
        read_error: BaseException | None = None,
        timeout: float | None = 1.0,
        in_waiting_override: int | None = None,
        timeout_setup_error: BaseException | None = None,
        timeout_restore_error: BaseException | None = None,
    ) -> None:
        self._chunks: list[bytearray] = []
        self._scheduled = [bytearray(chunk) for chunk in chunks]
        self._one_byte_reads = one_byte_reads
        self._gated_reads = gated_reads
        self._short_write = short_write
        self._write_error = write_error
        self._read_error = read_error
        self._timeout = timeout
        self._in_waiting_override = in_waiting_override
        self._timeout_setup_error = timeout_setup_error
        self._timeout_restore_error = timeout_restore_error
        self._after_next_write: list[bytearray] = []
        self.writes: list[bytes] = []
        self.read_calls = 0
        self.read_records: list[tuple[int, float | None]] = []
        self.timeout_assignments: list[float | None] = []
        self.reset_input_buffer_calls = 0
        self.closed = False
        self.is_open = True

    def write(self, data: bytes) -> int:
        self.writes.append(data)
        if self._write_error is not None:
            raise self._write_error
        if self._scheduled:
            self._chunks.extend(self._scheduled)
            self._scheduled = []
        if self._after_next_write:
            self._chunks.extend(self._after_next_write)
            self._after_next_write = []
        return len(data) if self._short_write is None else self._short_write

    def read(self, size: int = 1) -> bytes:
        self.read_calls += 1
        self.read_records.append((size, self.timeout))
        if self._read_error is not None:
            raise self._read_error
        if self._gated_reads:
            self._gated_reads -= 1
            return b""
        while self._chunks and not self._chunks[0]:
            self._chunks.pop(0)
        if not self._chunks:
            return b""
        count = 1 if self._one_byte_reads else max(1, size)
        chunk = self._chunks[0]
        result = bytes(chunk[:count])
        del chunk[:count]
        return result

    @property
    def in_waiting(self) -> int:
        if self._in_waiting_override is not None:
            return self._in_waiting_override
        return sum(len(chunk) for chunk in self._chunks)

    @property
    def timeout(self) -> float | None:
        return self._timeout

    @timeout.setter
    def timeout(self, value: float | None) -> None:
        self.timeout_assignments.append(value)
        if value == 0 and self._timeout_setup_error is not None:
            self._timeout = value
            raise self._timeout_setup_error
        if self._timeout == 0 and value != 0 and self._timeout_restore_error is not None:
            raise self._timeout_restore_error
        self._timeout = value

    def queue(self, *chunks: bytes) -> None:
        self._chunks.extend(bytearray(chunk) for chunk in chunks)

    def queue_after_next_write(self, *chunks: bytes) -> None:
        self._after_next_write.extend(bytearray(chunk) for chunk in chunks)

    def reset_input_buffer(self) -> None:
        self.reset_input_buffer_calls += 1

    def close(self) -> None:
        self.closed = True
        self.is_open = False


def make_protocol(
    fake: FakeMDTSerial, echo_enabled: bool | None, *, max_response_bytes: int = 4096
):
    from Code.Utils.mdt693b import _MDTProtocol

    return _MDTProtocol(fake, echo_enabled=echo_enabled, max_response_bytes=max_response_bytes)


class MDTProtocolTests(unittest.TestCase):
    """Each test guards one parser/transport boundary contract."""

    def test_echo_off_single_line_success(self):
        fake = FakeMDTSerial([b"12.3\r\n*\r\n"])
        protocol = make_protocol(fake, echo_enabled=False)
        self.assertEqual(protocol.transact("xvoltage?"), ("12.3",))
        self.assertEqual(fake.writes, [b"xvoltage?\r\n"])

    def test_echo_on_multiline_success(self):
        fake = FakeMDTSerial([b"?\n", b"id?\nxvoltage?\nyvoltage?\n*\n"])
        protocol = make_protocol(fake, echo_enabled=True)
        self.assertEqual(protocol.transact("?"), ("id?", "xvoltage?", "yvoltage?"))

    def test_initial_unknown_echo_is_learned_without_writing_a_configuration_command(self):
        fake = FakeMDTSerial([b"id?\rMODEL\r*\r"])
        protocol = make_protocol(fake, echo_enabled=None)
        self.assertEqual(protocol.transact("id?"), ("MODEL",))
        self.assertEqual(fake.writes, [b"id?\r\n"])
        self.assertTrue(protocol.echo_enabled)

    def test_response_terminators_and_split_prompt_are_accepted(self):
        for response in (b"answer\r*\r", b"answer\n*\n", b"answer\r\n*\r\n"):
            with self.subTest(response=response):
                fake = FakeMDTSerial([response])
                self.assertEqual(make_protocol(fake, False).transact("id?"), ("answer",))

    def test_successful_lf_prompt_does_not_issue_a_post_prompt_blocking_read(self):
        response = b"answer\n*\n"
        fake = FakeMDTSerial([response])
        self.assertEqual(make_protocol(fake, False).transact("id?"), ("answer",))
        self.assertEqual(fake.read_calls, len(response))

    def test_successful_cr_prompt_does_not_read_beyond_the_prompt(self):
        response = b"answer\r*\r"
        fake = FakeMDTSerial([response])
        self.assertEqual(make_protocol(fake, False).transact("id?"), ("answer",))
        self.assertEqual(fake.read_calls, len(response))

    def test_deferred_lf_after_a_cr_prompt_is_consumed_before_the_next_write(self):
        fake = FakeMDTSerial([b"first\r*\r"])
        protocol = make_protocol(fake, False)
        self.assertEqual(protocol.transact("first?"), ("first",))
        fake.queue(b"\n")
        fake.queue_after_next_write(b"second\n*\n")
        self.assertEqual(protocol.transact("second?"), ("second",))
        self.assertEqual(fake.writes, [b"first?\r\n", b"second?\r\n"])

    def test_late_lf_after_a_cr_prompt_is_consumed_at_the_next_frame_start(self):
        fake = FakeMDTSerial([b"first\r*\r"])
        protocol = make_protocol(fake, False)
        self.assertEqual(protocol.transact("first?"), ("first",))
        fake.queue_after_next_write(b"\nsecond\n*\n")
        self.assertEqual(protocol.transact("second?"), ("second",))

    def test_immediate_data_after_prompt_is_protocol_failure_without_blocking(self):
        fake = FakeMDTSerial([b"answer\n*\ntrailing"])
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(fake, False).transact("id?")
        self.assertEqual(fake.read_calls, len(b"answer\n*\n") + 1)

    def test_stale_data_is_rejected_before_a_later_command_is_written(self):
        fake = FakeMDTSerial([b"first\n*\n"])
        protocol = make_protocol(fake, False)
        self.assertEqual(protocol.transact("first?"), ("first",))
        fake.queue(b"stale")
        with self.assertRaises(InstrumentProtocolError):
            protocol.transact("second?")
        self.assertEqual(fake.writes, [b"first?\r\n"])

    def test_optional_crlf_byte_counts_toward_the_response_limit(self):
        accepted = FakeMDTSerial([b"*\r\n"])
        self.assertEqual(make_protocol(accepted, False, max_response_bytes=3).transact("id?"), ())
        rejected = FakeMDTSerial([b"*\r\n"])
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(rejected, False, max_response_bytes=2).transact("id?")

    def test_enormous_available_count_is_probed_with_one_nonblocking_byte(self):
        fake = FakeMDTSerial(in_waiting_override=10**9)
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(fake, False).transact("id?")
        self.assertEqual(fake.writes, [])
        self.assertEqual(fake.read_records, [(1, 0)])
        self.assertEqual(fake.timeout, 1.0)

    def test_positive_available_count_with_empty_nonblocking_read_is_protocol_failure(self):
        fake = FakeMDTSerial(in_waiting_override=1)
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(fake, False).transact("id?")
        self.assertEqual(fake.timeout_assignments, [0, 1.0])

    def test_surplus_probe_uses_zero_timeout_and_restores_it(self):
        fake = FakeMDTSerial(timeout=2.5)
        fake.queue(b"stale")
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(fake, False).transact("id?")
        self.assertEqual(fake.read_records, [(1, 0)])
        self.assertEqual(fake.timeout_assignments, [0, 2.5])
        self.assertEqual(fake.timeout, 2.5)

    def test_timeout_restore_failure_without_primary_error_is_typed_connection_failure(self):
        restore_error = OSError("restore failed")
        fake = FakeMDTSerial([b"answer\r*\r"])
        protocol = make_protocol(fake, False)
        self.assertEqual(protocol.transact("first?"), ("answer",))
        fake._timeout_restore_error = restore_error
        fake.queue(b"\n")
        with self.assertRaises(InstrumentConnectionError) as caught:
            protocol.transact("second?")
        self.assertIs(caught.exception.__cause__, restore_error)

    def test_timeout_restore_base_exception_preserves_primary_protocol_failure(self):
        fake = FakeMDTSerial(
            in_waiting_override=1,
            timeout_restore_error=KeyboardInterrupt("restore interrupted"),
        )
        with self.assertRaises(InstrumentProtocolError) as caught:
            make_protocol(fake, False).transact("id?")
        self.assertIsInstance(
            getattr(caught.exception, "mdt_timeout_restore_error", None), KeyboardInterrupt
        )

    def test_timeout_setup_mutation_failure_restores_prior_timeout_and_stays_primary(self):
        setup_error = KeyboardInterrupt("setup interrupted")
        fake = FakeMDTSerial(timeout=2.5, timeout_setup_error=setup_error)
        fake.queue(b"stale")
        with self.assertRaises(KeyboardInterrupt) as caught:
            make_protocol(fake, False).transact("id?")
        self.assertIs(caught.exception, setup_error)
        self.assertEqual(fake.timeout_assignments, [0, 2.5])
        self.assertEqual(fake.timeout, 2.5)

    def test_trailing_data_keeps_protocol_failure_primary_when_timeout_restore_fails(self):
        restoration_error = OSError("restore failed")
        fake = FakeMDTSerial(
            [b"answer\n*\ntrailing"], timeout_restore_error=restoration_error
        )
        with self.assertRaises(InstrumentProtocolError) as caught:
            make_protocol(fake, False).transact("id?")
        self.assertIs(getattr(caught.exception, "mdt_timeout_restore_error", None), restoration_error)

    def test_blank_result_lines_are_preserved(self):
        fake = FakeMDTSerial([b"one\n\ntwo\n*\n"])
        self.assertEqual(make_protocol(fake, False).transact("?"), ("one", "", "two"))

    def test_error_prompt_is_typed_protocol_failure(self):
        fake = FakeMDTSerial([b"xvoltage?\r\n!\r\n"])
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(fake, echo_enabled=True).transact("xvoltage?")

    def test_missing_prompt_is_timeout(self):
        with self.assertRaises(InstrumentTimeoutError):
            make_protocol(FakeMDTSerial([b"answer\n"]), False).transact("id?")

    def test_unexpected_echo_is_protocol_failure(self):
        fake = FakeMDTSerial([b"id?\nanswer\n*\n"])
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(fake, False).transact("id?")

    def test_extra_data_after_prompt_is_protocol_failure(self):
        fake = FakeMDTSerial([b"answer\n*\ntrailing"])
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(fake, False).transact("id?")

    def test_invalid_ascii_is_protocol_failure(self):
        fake = FakeMDTSerial([b"\xff\n*\n"])
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(fake, False).transact("id?")

    def test_short_write_is_connection_failure(self):
        fake = FakeMDTSerial([b"*\n"], short_write=1)
        with self.assertRaises(InstrumentConnectionError):
            make_protocol(fake, False).transact("id?")

    def test_write_timeout_is_a_typed_timeout_failure(self):
        fake = FakeMDTSerial([b"*\n"], write_error=serial.SerialTimeoutException("write"))
        with self.assertRaises(InstrumentTimeoutError):
            make_protocol(fake, False).transact("id?")

    def test_read_timeout_is_a_typed_timeout_failure(self):
        fake = FakeMDTSerial(read_error=TimeoutError("read"))
        with self.assertRaises(InstrumentTimeoutError):
            make_protocol(fake, False).transact("id?")

    def test_serial_io_errors_are_typed_connection_failures(self):
        fake = FakeMDTSerial([b"*\n"], read_error=OSError("cable"))
        with self.assertRaises(InstrumentConnectionError):
            make_protocol(fake, False).transact("id?")

    def test_response_size_is_bounded_before_unterminated_frame_grows(self):
        fake = FakeMDTSerial([b"123456789"])
        with self.assertRaises(InstrumentProtocolError):
            make_protocol(fake, False, max_response_bytes=8).transact("id?")

    def test_commands_must_be_ascii_text_without_line_breaks(self):
        for command in (b"id?", "id?\n", "id?\r", "voltage\N{SNOWMAN}"):
            with self.subTest(command=command):
                with self.assertRaises(InstrumentProtocolError):
                    make_protocol(FakeMDTSerial(), False).transact(command)  # type: ignore[arg-type]


class MDTTypeTests(unittest.TestCase):
    def test_enum_wire_values_and_voltage_limit_accessors(self):
        from Code.Utils.mdt693b import Axis, RotaryMode, VoltageLimit

        self.assertEqual(tuple(axis.value for axis in Axis), ("x", "y", "z"))
        self.assertEqual(tuple(limit.value for limit in VoltageLimit), ((0, 75.0), (1, 100.0), (2, 150.0)))
        self.assertEqual(tuple(limit.code for limit in VoltageLimit), (0, 1, 2))
        self.assertEqual(tuple(limit.volts for limit in VoltageLimit), (75.0, 100.0, 150.0))
        self.assertEqual(tuple(mode.value for mode in RotaryMode), (0, 1, 2))

    def test_axis_state_rejects_boolean_and_nonfinite_numbers(self):
        from Code.Utils.mdt693b import AxisState

        for value in (True, float("nan"), float("inf"), -float("inf")):
            with self.subTest(value=value):
                with self.assertRaises(InstrumentProtocolError):
                    AxisState(value, 0.0, 1.0)

    def test_axis_state_parses_finite_numeric_text(self):
        from Code.Utils.mdt693b import AxisState

        state = AxisState("1.25", "0", "75.0")
        self.assertEqual((state.actual_v, state.minimum_v, state.maximum_v), (1.25, 0.0, 75.0))

    def test_status_snapshot_is_frozen_and_axes_mapping_is_immutable(self):
        from Code.Utils.mdt693b import Axis, AxisState, MDTStatus, RotaryMode, VoltageLimit

        supplied_axes = {Axis.X: AxisState(1.0, 0.0, 75.0)}
        status = MDTStatus(
            product="MDT693B", firmware="1.0", serial_number="SN", friendly_name="lab",
            echo_enabled=True, hardware_limit=VoltageLimit.V75, display_intensity=1,
            master_scan_enabled=False, master_scan_voltage_v=0.0,
            axes=supplied_axes, dac_step=1,
            compatibility_enabled=False, rotary_mode=RotaryMode.DEFAULT,
            push_to_adjust_disabled=False, supported_commands=frozenset({"id?"}),
            restricted=False, fault_evidence=None, observed_at=1.0,
        )
        with self.assertRaises(TypeError):
            status.axes[Axis.Y] = AxisState(0.0, 0.0, 75.0)  # type: ignore[index]
        supplied_axes[Axis.Y] = AxisState(0.0, 0.0, 75.0)
        self.assertNotIn(Axis.Y, status.axes)
        with self.assertRaises(AttributeError):
            status.product = "other"  # type: ignore[misc]

    def test_status_rejects_boolean_and_nonfinite_numeric_fields(self):
        from Code.Utils.mdt693b import Axis, AxisState, MDTStatus, RotaryMode, VoltageLimit

        base = dict(
            product="MDT693B", firmware="1.0", serial_number="SN", friendly_name="lab",
            echo_enabled=True, hardware_limit=VoltageLimit.V75, display_intensity=1,
            master_scan_enabled=False, master_scan_voltage_v=0.0,
            axes={Axis.X: AxisState(1.0, 0.0, 75.0)}, dac_step=1,
            compatibility_enabled=False, rotary_mode=RotaryMode.DEFAULT,
            push_to_adjust_disabled=False, supported_commands=frozenset({"id?"}),
            restricted=False, fault_evidence=None, observed_at=1.0,
        )
        for field, value in (("display_intensity", True), ("master_scan_voltage_v", math.nan), ("observed_at", math.inf)):
            with self.subTest(field=field, value=value):
                values = dict(base)
                values[field] = value
                with self.assertRaises(InstrumentProtocolError):
                    MDTStatus(**values)


EXPECTED_CONNECT_QUERIES = [
    "?", "id?", "serial?", "friendly?", "echo?", "vlimit?", "intensity?",
    "msenable?", "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
    "xmin?", "ymin?", "zmin?", "xmax?", "ymax?", "zmax?", "dacstep?",
    "cm?", "rotarymode?", "pushdisable?",
]

MDT693B_REQUIRED_KEYWORDS = {
    "?", "id?", "restore", "echo?", "echo=", "vlimit?", "intensity?",
    "intensity=", "allvoltage=", "msenable?", "msenable=", "msvoltage?",
    "msvoltage=", "xvoltage?", "xvoltage=", "yvoltage?", "yvoltage=",
    "zvoltage?", "zvoltage=", "xmin?", "xmin=", "ymin?", "ymin=",
    "zmin?", "zmin=", "xmax?", "xmax=", "ymax?", "ymax=", "zmax?",
    "zmax=", "dacstep?", "dacstep=", "friendly?", "friendly=", "serial?",
    "cm?", "cm=", "rotarymode?", "rotarymode=", "pushdisable?", "pushdisable=",
    "UP", "DOWN", "LEFT", "RIGHT",
}


class ScriptedMDTDevice(FakeMDTSerial):
    """Command-aware transport model for read and confirmed-setter tests."""

    def __init__(
        self,
        *,
        x: float = 20.0,
        y: float = 30.0,
        z: float = 40.0,
        master_scan_v: float = 5.0,
        master_scan_enabled: bool = True,
        echo_enabled: bool = False,
        product: str = "MDT693B",
        firmware: str = "1.23",
        supported_commands: Iterable[str] = EXPECTED_CONNECT_QUERIES,
        echo_set_response_mode: str = "new",
        hardware_limit_code: int = 0,
        dac_step: int = 100,
        selected_axis: str = "x",
    ) -> None:
        super().__init__(timeout=0.5)
        self.values = {
            "xvoltage?": x, "yvoltage?": y, "zvoltage?": z,
            "friendly?": "SIL lab", "intensity?": 7,
            "xmin?": 0.0, "ymin?": 0.0, "zmin?": 0.0,
            "xmax?": 75.0, "ymax?": 75.0, "zmax?": 75.0,
            "dacstep?": dac_step, "cm?": False, "rotarymode?": 2,
            "pushdisable?": True,
        }
        self.master_scan_v = master_scan_v
        self.master_scan_enabled = master_scan_enabled
        contribution = master_scan_v if master_scan_enabled else 0.0
        self.axis_base_v = {
            "x": x - contribution,
            "y": y - contribution,
            "z": z - contribution,
        }
        self.axis_set_offsets_v = {"x": 0.0, "y": 0.0, "z": 0.0}
        self.echo_enabled = echo_enabled
        self.product = product
        self.firmware = firmware
        self.supported_commands = tuple(supported_commands)
        self.echo_set_response_mode = echo_set_response_mode
        self.hardware_limit_code = hardware_limit_code
        self.selected_axis = selected_axis
        self.arrow_prompts = {
            b"\x1b[A": "*", b"\x1b[B": "*",
            b"\x1b[C": "*", b"\x1b[D": "*",
        }
        self.arrow_response_overrides: dict[bytes, tuple[str, ...]] = {}
        self.arrow_raw_responses: dict[bytes, bytes] = {}
        self.arrow_read_errors: dict[bytes, BaseException] = {}
        self.arrow_motion_overrides: dict[bytes, dict[str, float]] = {}
        self.ignored_arrows: set[bytes] = set()
        self.restore_axis_base_v: dict[str, float] | None = None
        self.response_overrides: dict[str, tuple[str, ...]] = {}
        self.response_overrides_on_occurrences: dict[
            tuple[str, int], tuple[str, ...]
        ] = {}
        self.commands: list[str] = []
        self.command_occurrences: dict[str, int] = {}
        self.fail_on_occurrences: dict[str, set[int]] = {}
        self.effects_on_occurrences: dict[tuple[str, int], BaseException] = {}
        self.short_write_on_occurrences: dict[tuple[str, int], int] = {}
        self.ignored_setters: set[str] = set()
        self.block_on_occurrence: tuple[str, int, threading.Event, threading.Event] | None = None
        self.gate_read_on_occurrence: tuple[str, int, threading.Event, threading.Event] | None = None
        self._active_read_gate: tuple[threading.Event, threading.Event] | None = None
        self.cancel_read_calls = 0
        self.cancel_read_gate: tuple[threading.Event, threading.Event] | None = None
        self.cancel_read_effects: list[BaseException | None] = []
        self.close_effects: list[BaseException | None] = []

    def _response_lines(self, command: str) -> tuple[str, ...]:
        responses: dict[str, tuple[str, ...]] = {
            "?": tuple(self.supported_commands),
            "id?": (f"{self.product},{self.firmware}",),
            "serial?": ("SN123",),
            "echo?": ("1" if self.echo_enabled else "0",),
            "vlimit?": (str(self.hardware_limit_code),),
            "msenable?": ("1" if self.master_scan_enabled else "0",),
            "msvoltage?": (str(self.master_scan_v),),
        }
        if command in self.response_overrides:
            return self.response_overrides[command]
        if command in self.values:
            value = self.values[command]
            if isinstance(value, bool):
                return ("1" if value else "0",)
            return (str(value),)
        return responses[command]

    def _apply_setter(self, command: str) -> None:
        if "=" not in command:
            return
        keyword, raw_value = command.split("=", 1)
        if keyword in self.ignored_setters:
            return
        query = f"{keyword}?"
        if keyword == "friendly":
            self.values[query] = raw_value
        elif keyword in {"echo", "cm", "pushdisable"}:
            enabled = raw_value == "1"
            if keyword == "echo":
                self.echo_enabled = enabled
            else:
                self.values[query] = enabled
        elif keyword in {"intensity", "dacstep", "rotarymode"}:
            self.values[query] = int(raw_value)
        elif keyword in {"xmin", "ymin", "zmin", "xmax", "ymax", "zmax"}:
            self.values[query] = float(raw_value)
        elif keyword in {"xvoltage", "yvoltage", "zvoltage"}:
            axis = keyword[0]
            self.axis_base_v[axis] = float(raw_value)
            self._refresh_actual_voltages()
        elif keyword == "allvoltage":
            value = float(raw_value)
            self.axis_base_v = {axis: value for axis in ("x", "y", "z")}
            self._refresh_actual_voltages()
        elif keyword == "msvoltage":
            self.master_scan_v = float(raw_value)
            self._refresh_actual_voltages()
        elif keyword == "msenable":
            self.master_scan_enabled = raw_value == "1"
            self._refresh_actual_voltages()

    def _apply_restore(self) -> None:
        self.values.update({
            "friendly?": "MDT693B", "intensity?": 15,
            "xmin?": 0.0, "ymin?": 0.0, "zmin?": 0.0,
            "xmax?": 75.0, "ymax?": 75.0, "zmax?": 75.0,
            "dacstep?": 100, "cm?": False, "rotarymode?": 0,
            "pushdisable?": False,
        })
        self.master_scan_v = 0.0
        self.master_scan_enabled = False
        self.axis_base_v = (
            {"x": 0.0, "y": 0.0, "z": 0.0}
            if self.restore_axis_base_v is None
            else dict(self.restore_axis_base_v)
        )
        self.axis_set_offsets_v = {"x": 0.0, "y": 0.0, "z": 0.0}
        self.echo_enabled = False
        self._refresh_actual_voltages()

    def _write_arrow(self, data: bytes) -> int:
        names = {
            b"\x1b[A": "UP", b"\x1b[B": "DOWN",
            b"\x1b[C": "RIGHT", b"\x1b[D": "LEFT",
        }
        command = names[data]
        self.writes.append(data)
        self.commands.append(command)
        occurrence = self.command_occurrences.get(command, 0) + 1
        self.command_occurrences[command] = occurrence
        if (command, occurrence) in self.effects_on_occurrences:
            raise self.effects_on_occurrences[(command, occurrence)]
        if occurrence in self.fail_on_occurrences.get(command, set()):
            raise OSError(f"scripted {command} communication failure {occurrence}")
        block = self.block_on_occurrence
        if block is not None and (command, occurrence) == block[:2]:
            block[2].set()
            block[3].wait(2.0)

        old_echo = self.echo_enabled
        if data not in self.ignored_arrows:
            if data in (b"\x1b[C", b"\x1b[D"):
                order = ("x", "y", "z")
                index = order.index(self.selected_axis)
                offset = 1 if data == b"\x1b[C" else -1
                self.selected_axis = order[(index + offset) % len(order)]
            else:
                scale = {0: 75.0, 1: 100.0, 2: 150.0}[self.hardware_limit_code]
                default_delta = self.values["dacstep?"] * scale / 65535.0
                direction = 1.0 if data == b"\x1b[A" else -1.0
                deltas = self.arrow_motion_overrides.get(
                    data, {self.selected_axis: direction * default_delta}
                )
                for axis, delta in deltas.items():
                    self.axis_base_v[axis] += delta
            for axis, delta in self.arrow_motion_overrides.get(data, {}).items():
                if data in (b"\x1b[C", b"\x1b[D"):
                    self.axis_base_v[axis] += delta
            self._refresh_actual_voltages()

        if data in self.arrow_raw_responses:
            self.queue(self.arrow_raw_responses[data])
        else:
            lines = self.arrow_response_overrides.get(
                data, (self.selected_axis.upper(),)
            )
            echoed = ((data.decode("ascii"),) if old_echo else ())
            framed = echoed + lines + (self.arrow_prompts[data],)
            self.queue(("\r\n".join(framed) + "\r\n").encode("ascii"))
        gate = self.gate_read_on_occurrence
        if gate is not None and (command, occurrence) == gate[:2]:
            self._active_read_gate = (gate[2], gate[3])
        if data in self.arrow_read_errors:
            self._read_error = self.arrow_read_errors[data]
        return len(data) if self._short_write is None else self._short_write

    def _refresh_actual_voltages(self) -> None:
        contribution = self.master_scan_v if self.master_scan_enabled else 0.0
        for axis in ("x", "y", "z"):
            self.values[f"{axis}voltage?"] = (
                self.axis_base_v[axis]
                + contribution
                + self.axis_set_offsets_v[axis]
            )

    def write(self, data: bytes) -> int:
        if data in {b"\x1b[A", b"\x1b[B", b"\x1b[C", b"\x1b[D"}:
            return self._write_arrow(data)
        command = data.decode("ascii").removesuffix("\r\n")
        self.writes.append(data)
        self.commands.append(command)
        occurrence = self.command_occurrences.get(command, 0) + 1
        self.command_occurrences[command] = occurrence
        if (command, occurrence) in self.effects_on_occurrences:
            raise self.effects_on_occurrences[(command, occurrence)]
        if occurrence in self.fail_on_occurrences.get(command, set()):
            raise OSError(f"scripted {command} communication failure {occurrence}")
        block = self.block_on_occurrence
        if block is not None and (command, occurrence) == block[:2]:
            block[2].set()
            block[3].wait(2.0)
        old_echo = self.echo_enabled
        if command == "restore":
            self._apply_restore()
        else:
            self._apply_setter(command)
        lines = (
            ()
            if "=" in command or command == "restore"
            else self.response_overrides_on_occurrences.get(
                (command, occurrence), self._response_lines(command)
            )
        )
        response_echo = (
            old_echo
            if command.startswith("echo=") and self.echo_set_response_mode == "old"
            else self.echo_enabled
        )
        framed_lines = ((command,) if response_echo else ()) + lines + ("*",)
        self.queue(("\r\n".join(framed_lines) + "\r\n").encode("ascii"))
        gate = self.gate_read_on_occurrence
        if gate is not None and (command, occurrence) == gate[:2]:
            self._active_read_gate = (gate[2], gate[3])
        return self.short_write_on_occurrences.get((command, occurrence), len(data))

    def read(self, size: int = 1) -> bytes:
        gate = self._active_read_gate
        if gate is not None:
            gate[0].set()
            gate[1].wait()
            self._active_read_gate = None
        return super().read(size)

    def cancel_read(self) -> None:
        self.cancel_read_calls += 1
        if self.cancel_read_effects:
            effect = self.cancel_read_effects.pop(0)
            if effect is not None:
                raise effect
        if self.cancel_read_gate is not None:
            self.cancel_read_gate[0].set()
            self.cancel_read_gate[1].wait()
        gate = self._active_read_gate
        if gate is not None:
            gate[1].set()

    def close(self) -> None:
        if self.close_effects:
            effect = self.close_effects.pop(0)
            if effect is not None:
                raise effect
        super().close()


class RecordingSerialFactory:
    def __init__(self, device: ScriptedMDTDevice) -> None:
        self.device = device
        self.calls: list[dict[str, object]] = []

    def __call__(self, **kwargs: object) -> ScriptedMDTDevice:
        self.calls.append(dict(kwargs))
        return self.device


class GatedFakeTime:
    """Monotonic fake whose sleeps advance only when a test permits a cycle."""

    def __init__(self) -> None:
        self.now = 0.0
        self.sleep_requests: list[float] = []
        self._condition = threading.Condition()
        self._permits = 0

    def monotonic(self) -> float:
        with self._condition:
            return self.now

    def sleep(self, seconds: float) -> None:
        with self._condition:
            self.sleep_requests.append(seconds)
            self._condition.notify_all()
            while self._permits == 0:
                self._condition.wait()
            self._permits -= 1
            self.now += seconds

    def wait_for_sleeps(self, count: int, timeout: float = 2.0) -> bool:
        deadline = time.monotonic() + timeout
        with self._condition:
            while len(self.sleep_requests) < count:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return False
                self._condition.wait(remaining)
            return True

    def permit(self, count: int = 1) -> None:
        with self._condition:
            self._permits += count
            self._condition.notify_all()


class ScriptedEffectClock:
    def __init__(self, effects: Iterable[float | BaseException]) -> None:
        self.effects = list(effects)

    def __call__(self) -> float:
        effect = self.effects.pop(0)
        if isinstance(effect, BaseException):
            raise effect
        return effect


class ContentionObservedRLock:
    """Real re-entrant lock with a deterministic contention gate for tests."""

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._condition = threading.Condition()
        self._contentions = 0
        self.contention_event = threading.Event()

    def acquire(self, *args: object, **kwargs: object) -> bool:
        if self._lock.acquire(False):
            return True
        with self._condition:
            self._contentions += 1
            self.contention_event.set()
            self._condition.notify_all()
        return self._lock.acquire(*args, **kwargs)

    def release(self) -> None:
        self._lock.release()

    def __enter__(self):
        self.acquire()
        return self

    def __exit__(self, exc_type, exc_value, traceback) -> bool:
        self.release()
        return False

    def wait_for_contentions(self, count: int, timeout: float = 1.0) -> bool:
        deadline = time.monotonic() + timeout
        with self._condition:
            while self._contentions < count:
                remaining = deadline - time.monotonic()
                if remaining <= 0.0:
                    return False
                self._condition.wait(remaining)
            return True


class DeterministicMonitorHandle:
    """Inert live-monitor handle for request-path tests without a worker thread."""

    def __init__(self) -> None:
        self.alive = True

    def is_alive(self) -> bool:
        return self.alive

    def join(self, timeout: float | None = None) -> None:
        self.alive = False


class ExitEffectRLock:
    """Re-entrant lock whose outermost context exit fails deterministically."""

    def __init__(self, phase: str, effect: BaseException) -> None:
        self.phase = phase
        self.effect: BaseException | None = effect
        self.release_effect: BaseException | None = None
        self._lock = threading.RLock()
        self.depth = 0
        self.fallback_releases = 0

    def acquire(self, *args: object, **kwargs: object) -> bool:
        acquired = self._lock.acquire(*args, **kwargs)
        if acquired:
            self.depth += 1
        return acquired

    def release(self) -> None:
        self.fallback_releases += 1
        if self.release_effect is not None:
            raise self.release_effect
        self._lock.release()
        self.depth -= 1

    def _is_owned(self) -> bool:
        return self.depth > 0

    def __enter__(self):
        self.acquire()
        return self

    def __exit__(self, exc_type, exc_value, traceback) -> bool:
        if self.depth > 1 or self.effect is None:
            self._lock.release()
            self.depth -= 1
            return False
        effect = self.effect
        if self.phase == "after_release":
            self._lock.release()
            self.depth -= 1
        raise effect


def make_mdt_driver(
    device: ScriptedMDTDevice | None = None,
    fake_time: GatedFakeTime | None = None,
    **kwargs: object,
):
    from Code.Utils.mdt693b import MDT693B

    device = device or ScriptedMDTDevice()
    fake_time = fake_time or GatedFakeTime()
    factory = RecordingSerialFactory(device)
    driver = MDT693B(
        "COM_TEST",
        serial_factory=factory,
        clock=fake_time.monotonic,
        sleep=fake_time.sleep,
        **kwargs,
    )
    return driver, device, fake_time, factory


def establish_test_axis_authority(driver: object, device: ScriptedMDTDevice) -> None:
    """Arrange a previously confirmed baseline for pre-Task-14 motion tests."""
    actual = {
        axis: float(device.values[f"{axis.value}voltage?"])
        for axis in Axis
    }
    commands = {
        axis: float(device.axis_base_v[axis.value])
        for axis in Axis
    }
    with driver._lifecycle_condition:  # type: ignore[attr-defined]
        driver._transition_axis_authority_unlocked(  # type: ignore[attr-defined]
            True, commands=commands, actual=actual
        )


def close_gated_driver(driver: object, fake_time: GatedFakeTime) -> None:
    errors: list[BaseException] = []

    def close_target() -> None:
        try:
            driver.close()  # type: ignore[attr-defined]
        except BaseException as error:
            errors.append(error)

    closer = threading.Thread(target=close_target, name="MDTTestClose")
    closer.start()
    if not driver._monitor_stop.wait(2.0):  # type: ignore[attr-defined]
        fake_time.permit()
        closer.join(2.0)
        if errors:
            raise errors[0]
        if closer.is_alive():
            raise AssertionError("driver close did not publish monitor stop intent")
        raise AssertionError("driver close returned without monitor stop intent")
    fake_time.permit()
    closer.join(2.0)
    if closer.is_alive():
        raise AssertionError("driver close did not terminate")
    if errors:
        raise errors[0]


class MDTConfigurationTests(unittest.TestCase):
    """Task 9 typed configuration, confirmation, and capability boundaries."""

    @staticmethod
    def _configured_driver(*, before_connect=None, **device_kwargs: object):
        device = ScriptedMDTDevice(
            supported_commands=MDT693B_REQUIRED_KEYWORDS,
            **device_kwargs,
        )
        driver, device, fake_time, _ = make_mdt_driver(device)
        if before_connect is not None:
            before_connect(device)
        driver.connect()
        try:
            if not fake_time.wait_for_sleeps(1):
                raise AssertionError("monitor did not park after its initial triplet")
        except BaseException:
            close_gated_driver(driver, fake_time)
            raise
        return driver, device, fake_time

    def test_configured_driver_returns_only_after_initial_monitor_triplet_parks(self):
        entered = threading.Event()
        release = threading.Event()
        returned = threading.Event()
        built: list[tuple[object, ScriptedMDTDevice, GatedFakeTime]] = []
        errors: list[BaseException] = []

        def before_connect(device: ScriptedMDTDevice) -> None:
            device.block_on_occurrence = ("xvoltage?", 2, entered, release)

        def build() -> None:
            try:
                built.append(self._configured_driver(before_connect=before_connect))
            except BaseException as error:
                errors.append(error)
            finally:
                returned.set()

        worker = threading.Thread(target=build)
        worker.start()
        try:
            self.assertTrue(entered.wait(1.0))
            self.assertFalse(
                returned.wait(0.1),
                "fixture returned while the initial monitor triplet was still in progress",
            )
        finally:
            release.set()
            worker.join(2.0)
            if built:
                driver, _, fake_time = built[0]
                close_gated_driver(driver, fake_time)
        self.assertFalse(worker.is_alive())
        self.assertEqual(errors, [])
        self.assertEqual(len(built), 1)

    def test_full_firmware_keyword_contract_is_documented_without_future_public_apis(self):
        from Code.Utils.mdt693b import MDT693B

        self.assertEqual(len(MDT693B_REQUIRED_KEYWORDS), 46)
        self.assertTrue(set(EXPECTED_CONNECT_QUERIES).issubset(MDT693B_REQUIRED_KEYWORDS))
        for task10_name in (
            "get_axis_voltage", "set_axis_voltage", "get_all_voltages",
            "set_all_voltages", "get_master_scan_enabled",
            "set_master_scan_enabled", "get_master_scan_voltage",
            "set_master_scan_voltage",
        ):
            self.assertTrue(hasattr(MDT693B, task10_name), task10_name)
        for future_name in (
            "restore", "arrow_up", "arrow_down", "arrow_left", "arrow_right",
        ):
            self.assertFalse(hasattr(MDT693B, future_name), future_name)

    def test_all_21_task9_methods_gate_lifecycle_and_capability_without_fault_or_io(self):
        from Code.Utils.mdt693b import Axis, MDT693B, RotaryMode

        unconnected_device = ScriptedMDTDevice(supported_commands=MDT693B_REQUIRED_KEYWORDS)
        unconnected = MDT693B(
            "COM_TEST", serial_factory=RecordingSerialFactory(unconnected_device)
        )
        lifecycle_calls = (
            lambda: unconnected.get_supported_commands(),
            lambda: unconnected.get_product_information(),
            lambda: unconnected.get_serial_number(),
            lambda: unconnected.get_friendly_name(),
            lambda: unconnected.set_friendly_name("lab"),
            lambda: unconnected.get_echo_enabled(),
            lambda: unconnected.set_echo_enabled(True),
            lambda: unconnected.get_hardware_voltage_limit(),
            lambda: unconnected.get_display_intensity(),
            lambda: unconnected.set_display_intensity(9),
            lambda: unconnected.get_axis_state(Axis.X),
            lambda: unconnected.set_axis_minimum(Axis.X, 0.0),
            lambda: unconnected.set_axis_maximum(Axis.X, 50.0),
            lambda: unconnected.get_dac_step(),
            lambda: unconnected.set_dac_step(10),
            lambda: unconnected.get_compatibility_enabled(),
            lambda: unconnected.set_compatibility_enabled(True),
            lambda: unconnected.get_rotary_mode(),
            lambda: unconnected.set_rotary_mode(RotaryMode.DEFAULT),
            lambda: unconnected.get_push_to_adjust_disabled(),
            lambda: unconnected.set_push_to_adjust_disabled(False),
        )
        self.assertEqual(len(lifecycle_calls), 21)
        for call in lifecycle_calls:
            with self.subTest(gate="lifecycle", call=call):
                with self.assertRaises(InstrumentConnectionError):
                    call()
                self.assertEqual(unconnected_device.commands, [])
                self.assertEqual(unconnected.state, DriverState.DISCONNECTED)

        driver, device, fake_time = self._configured_driver()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        capability_calls = (
            ("?", lambda: driver.get_supported_commands()),
            ("id?", lambda: driver.get_product_information()),
            ("serial?", lambda: driver.get_serial_number()),
            ("friendly?", lambda: driver.get_friendly_name()),
            ("friendly=", lambda: driver.set_friendly_name("lab")),
            ("echo?", lambda: driver.get_echo_enabled()),
            ("echo=", lambda: driver.set_echo_enabled(True)),
            ("vlimit?", lambda: driver.get_hardware_voltage_limit()),
            ("intensity?", lambda: driver.get_display_intensity()),
            ("intensity=", lambda: driver.set_display_intensity(9)),
            ("xvoltage?", lambda: driver.get_axis_state(Axis.X)),
            ("xmin=", lambda: driver.set_axis_minimum(Axis.X, 0.0)),
            ("xmax=", lambda: driver.set_axis_maximum(Axis.X, 50.0)),
            ("dacstep?", lambda: driver.get_dac_step()),
            ("dacstep=", lambda: driver.set_dac_step(10)),
            ("cm?", lambda: driver.get_compatibility_enabled()),
            ("cm=", lambda: driver.set_compatibility_enabled(True)),
            ("rotarymode?", lambda: driver.get_rotary_mode()),
            ("rotarymode=", lambda: driver.set_rotary_mode(RotaryMode.DEFAULT)),
            ("pushdisable?", lambda: driver.get_push_to_adjust_disabled()),
            ("pushdisable=", lambda: driver.set_push_to_adjust_disabled(False)),
        )
        self.assertEqual(len(capability_calls), 21)
        complete = driver.status.supported_commands
        for missing, call in capability_calls:
            with self.subTest(gate="capability", missing=missing):
                driver.status = replace(
                    driver.status,
                    supported_commands=complete.difference({missing}),
                )
                before = len(device.commands)
                with self.assertRaises(InstrumentCapabilityError):
                    call()
                self.assertEqual(len(device.commands), before)
                self.assertEqual(driver.state, DriverState.READY)
                self.assertIsNone(driver.status.fault_evidence)
                driver.status = replace(driver.status, supported_commands=complete)
        close_gated_driver(driver, fake_time)

    def test_getters_issue_exact_queries_and_return_typed_values(self):
        from Code.Utils.mdt693b import Axis, AxisState, RotaryMode, VoltageLimit

        driver, device, fake_time = self._configured_driver()
        baseline = len(device.commands)
        cases = (
            (driver.get_supported_commands, "?", frozenset(
                command.lower() for command in MDT693B_REQUIRED_KEYWORDS
            )),
            (driver.get_product_information, "id?", ("MDT693B", "1.23")),
            (driver.get_serial_number, "serial?", "SN123"),
            (driver.get_friendly_name, "friendly?", "SIL lab"),
            (driver.get_echo_enabled, "echo?", False),
            (driver.get_hardware_voltage_limit, "vlimit?", VoltageLimit.V75),
            (driver.get_display_intensity, "intensity?", 7),
            (lambda: driver.get_axis_state(Axis.X), "xvoltage?", AxisState(20, 0, 75)),
            (driver.get_dac_step, "dacstep?", 100),
            (driver.get_compatibility_enabled, "cm?", False),
            (driver.get_rotary_mode, "rotarymode?", RotaryMode.FINE),
            (driver.get_push_to_adjust_disabled, "pushdisable?", True),
        )
        for call, command, expected in cases:
            with self.subTest(command=command):
                before = len(device.commands)
                self.assertEqual(call(), expected)
                issued = device.commands[before:]
                if command.endswith("voltage?"):
                    self.assertEqual(issued, [command, "xmin?", "xmax?"])
                else:
                    self.assertEqual(issued, [command])
        self.assertGreater(len(device.commands), baseline)
        close_gated_driver(driver, fake_time)

    def test_help_getter_keeps_question_token_as_content_when_echo_is_off(self):
        # '?' is legitimate help content, not evidence that echo became enabled.
        # Force this order instead of relying on the random order of a set.
        commands = ("?", *sorted(MDT693B_REQUIRED_KEYWORDS - {"?"}))
        for echo in (False, True):
            with self.subTest(echo=echo):
                driver, device, fake_time = self._configured_driver(echo_enabled=echo)
                try:
                    device.response_overrides["?"] = commands
                    before = len(device.commands)
                    try:
                        supported = driver.get_supported_commands()
                    except InstrumentProtocolError as error:
                        self.fail(f"Valid help content was rejected as an echo: {error}")
                    self.assertEqual(supported, frozenset(c.lower() for c in commands))
                    self.assertEqual(device.commands[before:], ["?"])
                    self.assertEqual(driver.status.echo_enabled, echo)
                    self.assertEqual(driver.state, DriverState.READY)
                finally:
                    close_gated_driver(driver, fake_time)

    def test_non_motion_setters_use_one_exact_command_same_property_readback_and_publish(self):
        from Code.Utils.mdt693b import RotaryMode

        cases = (
            ("set_friendly_name", "optics bench", "friendly=optics bench", "friendly?", "optics bench", "friendly_name"),
            ("set_display_intensity", 0, "intensity=0", "intensity?", 0, "display_intensity"),
            ("set_display_intensity", 15, "intensity=15", "intensity?", 15, "display_intensity"),
            ("set_dac_step", 1, "dacstep=1", "dacstep?", 1, "dac_step"),
            ("set_dac_step", 1000, "dacstep=1000", "dacstep?", 1000, "dac_step"),
            ("set_compatibility_enabled", True, "cm=1", "cm?", True, "compatibility_enabled"),
            ("set_rotary_mode", RotaryMode.TEN_TURN, "rotarymode=1", "rotarymode?", RotaryMode.TEN_TURN, "rotary_mode"),
            ("set_push_to_adjust_disabled", False, "pushdisable=0", "pushdisable?", False, "push_to_adjust_disabled"),
        )
        for method, argument, setter, query, expected, field in cases:
            with self.subTest(method=method, argument=argument):
                driver, device, fake_time = self._configured_driver()
                try:
                    previous = driver.status
                    before = len(device.commands)
                    self.assertEqual(getattr(driver, method)(argument), expected)
                    self.assertEqual(device.commands[before:], [setter, query])
                    self.assertIsNot(driver.status, previous)
                    self.assertEqual(getattr(driver.status, field), expected)
                finally:
                    close_gated_driver(driver, fake_time)

    def test_echo_transition_accepts_old_or_new_set_response_then_confirms_strict_mode(self):
        for initial, target in ((False, True), (True, False)):
            for response_mode in ("old", "new"):
                with self.subTest(initial=initial, target=target, response_mode=response_mode):
                    driver, device, fake_time = self._configured_driver(
                        echo_enabled=initial,
                        echo_set_response_mode=response_mode,
                    )
                    before = len(device.commands)
                    self.assertIs(driver.set_echo_enabled(target), target)
                    self.assertEqual(device.commands[before:], [f"echo={int(target)}", "echo?"])
                    self.assertIs(driver.status.echo_enabled, target)
                    self.assertIs(driver._protocol.echo_enabled, target)
                    self.assertEqual(driver.get_serial_number(), "SN123")
                    close_gated_driver(driver, fake_time)

    def test_invalid_configuration_inputs_are_rejected_before_any_write(self):
        from Code.Utils.mdt693b import Axis, RotaryMode

        driver, device, fake_time = self._configured_driver()
        invalid_calls = (
            lambda: driver.set_friendly_name(""),
            lambda: driver.set_friendly_name("line\nfeed"),
            lambda: driver.set_friendly_name("line\rfeed"),
            lambda: driver.set_friendly_name("nul\0byte"),
            lambda: driver.set_friendly_name("snow\N{SNOWMAN}"),
            lambda: driver.set_friendly_name(1),
            lambda: driver.set_echo_enabled(1),
            lambda: driver.set_display_intensity(True),
            lambda: driver.set_display_intensity(-1),
            lambda: driver.set_display_intensity(16),
            lambda: driver.set_display_intensity(1.0),
            lambda: driver.set_dac_step(True),
            lambda: driver.set_dac_step(0),
            lambda: driver.set_dac_step(1001),
            lambda: driver.set_compatibility_enabled(1),
            lambda: driver.set_rotary_mode(1),
            lambda: driver.set_push_to_adjust_disabled(0),
            lambda: driver.set_axis_minimum("x", 1.0),
            lambda: driver.set_axis_maximum(Axis.X, True),
            lambda: driver.set_axis_maximum(Axis.X, math.nan),
        )
        before = len(device.commands)
        for call in invalid_calls:
            with self.subTest(call=call):
                with self.assertRaises(InstrumentProtocolError):
                    call()
                self.assertEqual(len(device.commands), before)
        self.assertIsInstance(RotaryMode.DEFAULT, RotaryMode)
        close_gated_driver(driver, fake_time)

    def test_friendly_name_has_no_invented_length_limit(self):
        driver, device, fake_time = self._configured_driver()
        long_name = "a" * 500
        before = len(device.commands)
        self.assertEqual(driver.set_friendly_name(long_name), long_name)
        self.assertEqual(device.commands[before:], [f"friendly={long_name}", "friendly?"])
        close_gated_driver(driver, fake_time)

    def test_friendly_name_preserves_leading_trailing_and_whitespace_only_values(self):
        for value in (" lab ", "   "):
            with self.subTest(value=value):
                driver, device, fake_time = self._configured_driver()
                before = len(device.commands)
                self.assertEqual(driver.set_friendly_name(value), value)
                self.assertEqual(
                    device.commands[before:],
                    [f"friendly={value}", "friendly?"],
                )
                self.assertEqual(driver.status.friendly_name, value)
                before = len(device.commands)
                self.assertEqual(driver.get_friendly_name(), value)
                self.assertEqual(device.commands[before:], ["friendly?"])
                close_gated_driver(driver, fake_time)

    def test_axis_limit_setters_validate_snapshot_and_confirm_same_property(self):
        from Code.Utils.mdt693b import Axis, AxisState

        driver, device, fake_time = self._configured_driver()
        previous = driver.status
        before = len(device.commands)
        self.assertEqual(driver.set_axis_minimum(Axis.X, 10), AxisState(20, 10, 75))
        self.assertEqual(device.commands[before:], ["xmin=10", "xmin?"])
        self.assertIsNot(driver.status, previous)
        before = len(device.commands)
        self.assertEqual(driver.set_axis_maximum(Axis.X, 50.0), AxisState(20, 10, 50))
        self.assertEqual(device.commands[before:], ["xmax=50", "xmax?"])
        close_gated_driver(driver, fake_time)

    def test_axis_limit_wire_value_round_trips_high_precision_without_false_fault(self):
        from Code.Utils.mdt693b import Axis, AxisState

        driver, device, fake_time = self._configured_driver()
        before = len(device.commands)
        self.assertEqual(
            driver.set_axis_minimum(Axis.X, 0.123456789),
            AxisState(20.0, 0.123456789, 75.0),
        )
        self.assertEqual(
            device.commands[before:],
            ["xmin=0.123456789", "xmin?"],
        )
        self.assertEqual(driver.state, DriverState.READY)
        close_gated_driver(driver, fake_time)

    def test_axis_limits_cannot_invert_exceed_any_ceiling_or_exclude_actual(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._configured_driver()
        before = len(device.commands)
        invalid_calls = (
            lambda: driver.set_axis_minimum(Axis.X, -0.1),
            lambda: driver.set_axis_minimum(Axis.X, 21.0),
            lambda: driver.set_axis_minimum(Axis.X, 76.0),
            lambda: driver.set_axis_maximum(Axis.X, -1.0),
            lambda: driver.set_axis_maximum(Axis.X, 19.0),
            lambda: driver.set_axis_maximum(Axis.X, 76.0),
        )
        for call in invalid_calls:
            with self.subTest(call=call):
                with self.assertRaises(InstrumentProtocolError):
                    call()
                self.assertEqual(len(device.commands), before)
        close_gated_driver(driver, fake_time)

        driver, device, fake_time, _ = make_mdt_driver(
            ScriptedMDTDevice(supported_commands=MDT693B_REQUIRED_KEYWORDS),
            application_limits_v={Axis.X: 25.0},
        )
        driver.connect()
        before = len(device.commands)
        with self.assertRaises(InstrumentProtocolError):
            driver.set_axis_maximum(Axis.X, 25.1)
        self.assertEqual(len(device.commands), before)
        close_gated_driver(driver, fake_time)

    def test_missing_setter_or_readback_capability_fails_before_wire_io(self):
        driver, device, fake_time = self._configured_driver()
        for missing in ("intensity=", "intensity?"):
            with self.subTest(missing=missing):
                driver.status = replace(
                    driver.status,
                    supported_commands=driver.status.supported_commands.difference({missing}),
                )
                before = len(device.commands)
                with self.assertRaises(InstrumentCapabilityError):
                    driver.set_display_intensity(9)
                self.assertEqual(len(device.commands), before)
                driver.status = replace(
                    driver.status,
                    supported_commands=frozenset(MDT693B_REQUIRED_KEYWORDS),
                )
        close_gated_driver(driver, fake_time)

    def test_setter_readback_mismatch_faults_with_evidence_and_no_compensation(self):
        driver, device, fake_time = self._configured_driver()
        device.response_overrides["intensity?"] = ("8",)
        before = len(device.commands)
        with self.assertRaises(DeviceFault):
            driver.set_display_intensity(9)
        tail = device.commands[before:]
        self.assertEqual(tail, ["intensity=9", "intensity?"])
        self.assertEqual(sum("=" in command for command in tail), 1)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIn("intensity", driver.status.fault_evidence.lower())
        fake_time.permit()
        driver.close()

    def test_success_publication_baseexception_faults_with_stale_confirmed_status(self):
        for primary in (
            KeyboardInterrupt("success publication interrupted"),
            SystemExit("success publication exited"),
            RuntimeError("success publication failed"),
        ):
            with self.subTest(primary=type(primary).__name__):
                driver, device, fake_time = self._configured_driver()
                self.assertTrue(fake_time.wait_for_sleeps(1))
                old_status = driver.status
                secondary = RuntimeError("fault timestamp failed")
                driver._clock = ScriptedEffectClock((primary, secondary))
                before = len(device.commands)
                with self.assertRaises(type(primary)) as caught:
                    driver.set_display_intensity(9)
                self.assertIs(caught.exception, primary)
                self.assertIs(getattr(primary, "mdt_cleanup_error", None), secondary)
                self.assertEqual(device.commands[before:], ["intensity=9", "intensity?"])
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertEqual(driver.status.display_intensity, old_status.display_intensity)
                self.assertIn("intensity=9", driver.status.fault_evidence)
                self.assertEqual(driver.status.observed_at, old_status.observed_at)
                driver._clock = fake_time.monotonic
                fake_time.permit()
                driver.close()

    def test_fault_publication_baseexception_keeps_mismatch_primary_and_fault_state(self):
        for secondary in (
            KeyboardInterrupt("fault publication interrupted"),
            SystemExit("fault publication exited"),
            RuntimeError("fault publication failed"),
        ):
            with self.subTest(secondary=type(secondary).__name__):
                driver, device, fake_time = self._configured_driver()
                self.assertTrue(fake_time.wait_for_sleeps(1))
                old_status = driver.status
                device.response_overrides["intensity?"] = ("8",)
                driver._clock = ScriptedEffectClock((secondary,))
                before = len(device.commands)
                with self.assertRaises(DeviceFault) as caught:
                    driver.set_display_intensity(9)
                self.assertIs(
                    getattr(caught.exception, "mdt_cleanup_error", None), secondary
                )
                self.assertEqual(device.commands[before:], ["intensity=9", "intensity?"])
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertEqual(driver.status.display_intensity, old_status.display_intensity)
                self.assertIn("intensity=9", driver.status.fault_evidence)
                self.assertEqual(driver.status.observed_at, old_status.observed_at)
                driver._clock = fake_time.monotonic
                fake_time.permit()
                driver.close()

    def test_replace_failure_after_confirmed_readback_uses_immutable_fault_fallback(self):
        driver, device, fake_time = self._configured_driver()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        old_status = driver.status
        primary = RuntimeError("success replace failed")
        secondary = SystemExit("fault replace failed")
        effects = iter((primary, secondary))

        def fail_replace(*args: object, **kwargs: object):
            raise next(effects)

        before = len(device.commands)
        with patch("Code.Utils.mdt693b.replace", side_effect=fail_replace):
            with self.assertRaises(RuntimeError) as caught:
                driver.set_display_intensity(9)
        self.assertIs(caught.exception, primary)
        self.assertIs(getattr(primary, "mdt_cleanup_error", None), secondary)
        self.assertEqual(device.commands[before:], ["intensity=9", "intensity?"])
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(driver.status.display_intensity, old_status.display_intensity)
        self.assertIn("intensity=9", driver.status.fault_evidence)
        self.assertIsNot(driver.status, old_status)
        fake_time.permit()
        driver.close()

    def test_every_direct_getter_transport_failure_faults_and_blocks_later_setter(self):
        from Code.Utils.mdt693b import Axis

        getter_cases = (
            ("?", lambda driver: driver.get_supported_commands()),
            ("id?", lambda driver: driver.get_product_information()),
            ("serial?", lambda driver: driver.get_serial_number()),
            ("friendly?", lambda driver: driver.get_friendly_name()),
            ("echo?", lambda driver: driver.get_echo_enabled()),
            ("vlimit?", lambda driver: driver.get_hardware_voltage_limit()),
            ("intensity?", lambda driver: driver.get_display_intensity()),
            ("xvoltage?", lambda driver: driver.get_axis_state(Axis.X)),
            ("dacstep?", lambda driver: driver.get_dac_step()),
            ("cm?", lambda driver: driver.get_compatibility_enabled()),
            ("rotarymode?", lambda driver: driver.get_rotary_mode()),
            ("pushdisable?", lambda driver: driver.get_push_to_adjust_disabled()),
        )
        self.assertEqual(len(getter_cases), 12)
        for command, getter in getter_cases:
            with self.subTest(command=command):
                driver, device, fake_time = self._configured_driver()
                self.assertTrue(fake_time.wait_for_sleeps(1))
                occurrence = device.command_occurrences.get(command, 0) + 1
                device.fail_on_occurrences[command] = {occurrence}
                before = len(device.commands)
                with self.assertRaises(InstrumentConnectionError):
                    getter(driver)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIn(command, driver.status.fault_evidence)
                after_failure = len(device.commands)
                self.assertGreater(after_failure, before)
                with self.assertRaises(InstrumentConnectionError):
                    driver.set_display_intensity(9)
                self.assertEqual(len(device.commands), after_failure)
                fake_time.permit()
                driver.close()

    def test_getter_protocol_and_baseexception_failures_atomically_fault(self):
        driver, device, fake_time = self._configured_driver()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        device.response_overrides["intensity?"] = ("not-an-integer",)
        with self.assertRaises(InstrumentProtocolError):
            driver.get_display_intensity()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIn("intensity?", driver.status.fault_evidence)
        fake_time.permit()
        driver.close()

        for primary in (
            KeyboardInterrupt("getter interrupted"),
            SystemExit("getter exited"),
        ):
            with self.subTest(primary=type(primary).__name__):
                driver, device, fake_time = self._configured_driver()
                self.assertTrue(fake_time.wait_for_sleeps(1))
                occurrence = device.command_occurrences.get("serial?", 0) + 1
                device.effects_on_occurrences[("serial?", occurrence)] = primary
                with self.assertRaises(type(primary)) as caught:
                    driver.get_serial_number()
                self.assertIs(caught.exception, primary)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIn("serial?", driver.status.fault_evidence)
                fake_time.permit()
                driver.close()

    def test_setter_and_readback_hold_request_lock_against_monitor(self):
        entered = threading.Event()
        release = threading.Event()
        driver, device, fake_time = self._configured_driver()
        device.block_on_occurrence = ("intensity=9", 1, entered, release)
        errors: list[BaseException] = []
        setter = threading.Thread(
            target=lambda: self._capture_error(lambda: driver.set_display_intensity(9), errors)
        )
        baseline = len(device.commands)
        setter.start()
        self.assertTrue(entered.wait(1.0))
        fake_time.permit()
        release.set()
        setter.join(1.0)
        self.assertFalse(setter.is_alive())
        self.assertEqual(errors, [])
        deadline = time.monotonic() + 1.0
        while len(device.commands) < baseline + 5 and time.monotonic() < deadline:
            time.sleep(0.001)
        self.assertEqual(device.commands[baseline:baseline + 5], [
            "intensity=9", "intensity?", "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        close_gated_driver(driver, fake_time)

    def test_setter_preserves_transient_monitor_evidence_until_successful_triplet(self):
        device = ScriptedMDTDevice(supported_commands=MDT693B_REQUIRED_KEYWORDS)
        device.fail_on_occurrences["xvoltage?"] = {2}
        driver, device, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        evidence = driver.status.fault_evidence
        self.assertIn("communication failure 1/3", evidence)

        self.assertEqual(driver.set_display_intensity(9), 9)
        self.assertEqual(driver.status.fault_evidence, evidence)

        fake_time.permit()
        self.assertTrue(fake_time.wait_for_sleeps(2))
        self.assertIsNone(driver.status.fault_evidence)
        close_gated_driver(driver, fake_time)

    def test_close_publishes_and_cancels_gated_task9_getter_without_late_publication(self):
        self._assert_close_cancels_task9_operation(
            gate_command="serial?",
            gate_occurrence=2,
            operation=lambda driver: driver.get_serial_number(),
        )

    def test_close_publishes_and_cancels_gated_setter_readback_without_late_publication(self):
        self._assert_close_cancels_task9_operation(
            gate_command="intensity?",
            gate_occurrence=2,
            operation=lambda driver: driver.set_display_intensity(9),
        )

    def _assert_close_cancels_task9_operation(
        self, *, gate_command: str, gate_occurrence: int, operation: object
    ) -> None:
        entered = threading.Event()
        release = threading.Event()
        cancel_entered = threading.Event()
        cancel_release = threading.Event()
        device = ScriptedMDTDevice(supported_commands=MDT693B_REQUIRED_KEYWORDS)
        device.gate_read_on_occurrence = (
            gate_command, gate_occurrence, entered, release,
        )
        device.cancel_read_gate = (cancel_entered, cancel_release)
        driver, device, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        original_status = driver.status
        operation_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        requester = threading.Thread(
            target=lambda: self._capture_error(lambda: operation(driver), operation_errors)  # type: ignore[operator]
        )
        requester.start()
        self.assertTrue(entered.wait(1.0))
        closer = threading.Thread(target=lambda: self._capture_error(driver.close, close_errors))
        closer.start()
        cancellation_started = cancel_entered.wait(0.2)
        closing_visible = cancellation_started and driver.state is DriverState.CLOSING
        if cancellation_started:
            cancel_release.set()
        else:
            release.set()
            cancel_release.set()
        fake_time.permit()
        requester.join(2.0)
        closer.join(2.0)
        release.set()
        self.assertTrue(closing_visible)
        self.assertFalse(requester.is_alive())
        self.assertFalse(closer.is_alive())
        self.assertEqual(close_errors, [])
        self.assertEqual(len(operation_errors), 1)
        self.assertIsInstance(operation_errors[0], InstrumentConnectionError)
        self.assertIs(driver.status, original_status)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertEqual(device.cancel_read_calls, 1)
        self.assertTrue(device.closed)

    @staticmethod
    def _capture_error(callable_: object, errors: list[BaseException]) -> None:
        try:
            callable_()  # type: ignore[operator]
        except BaseException as error:
            errors.append(error)


class MDTMotionTests(unittest.TestCase):
    """Task 10 deterministic ordinary-motion and cancellation boundaries."""

    @staticmethod
    def _motion_driver(
        *,
        driver_kwargs: dict[str, object] | None = None,
        **device_kwargs: object,
    ):
        settings = {
            "supported_commands": MDT693B_REQUIRED_KEYWORDS,
            "x": 1.0,
            "y": 1.0,
            "z": 1.0,
            "master_scan_v": 0.0,
            "master_scan_enabled": False,
        }
        settings.update(device_kwargs)
        device = ScriptedMDTDevice(**settings)
        driver, device, fake_time, _ = make_mdt_driver(
            device, **({} if driver_kwargs is None else driver_kwargs)
        )
        driver.connect()
        if not fake_time.wait_for_sleeps(1):
            raise AssertionError("monitor did not complete its initial triplet")
        driver._monitor_stop.set()
        fake_time.permit()
        driver._monitor_thread.join(1.0)
        if driver._monitor_thread.is_alive():
            raise AssertionError("monitor did not stop for deterministic motion test")
        driver._monitor_stop.clear()
        driver._monitor_started.set()
        driver._monitor_exited.clear()
        driver._monitor_thread = DeterministicMonitorHandle()
        establish_test_axis_authority(driver, device)
        fake_time.sleep_requests.clear()
        device.commands.clear()
        return driver, device, fake_time

    @staticmethod
    def _values(device: ScriptedMDTDevice, keyword: str) -> list[float]:
        prefix = f"{keyword}="
        return [
            float(command.removeprefix(prefix))
            for command in device.commands
            if command.startswith(prefix)
        ]

    @staticmethod
    def _capture(callable_: object, errors: list[BaseException]) -> None:
        try:
            callable_()  # type: ignore[operator]
        except BaseException as error:
            errors.append(error)

    def test_axis_ramps_use_linear_point_one_bounded_steps_triplet_confirmation_and_spacing(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver()
        fake_time.permit(10)
        status = driver.set_axis_voltage(Axis.X, 1.25)
        expected = [1.0 + 0.25 / 3.0, 1.0 + 0.5 / 3.0, 1.25]
        self.assertEqual(self._values(device, "xvoltage"), expected)
        self.assertEqual(device.commands[:5], [
            "msenable?", "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        query_indexes = [
            index for index, command in enumerate(device.commands)
            if command == "xvoltage?"
        ]
        self.assertEqual(len(query_indexes), 6)
        for index in query_indexes:
            self.assertEqual(device.commands[index:index + 3], [
                "xvoltage?", "yvoltage?", "zvoltage?",
            ])
        self.assertEqual(fake_time.sleep_requests, [0.050, 0.050])
        self.assertEqual(status.axes[Axis.X].actual_v, 1.25)
        driver.close()

    def test_axis_decreasing_and_exact_boundary_ramps_have_exact_trajectories(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver(x=1.25)
        fake_time.permit(10)
        driver.set_axis_voltage(Axis.X, 1.0)
        expected = [1.25 - 0.25 / 3.0, 1.25 - 0.5 / 3.0, 1.0]
        self.assertEqual(self._values(device, "xvoltage"), expected)
        device.commands.clear()
        fake_time.sleep_requests.clear()
        fake_time.permit(10)
        driver.set_axis_voltage(Axis.X, 1.2)
        self.assertEqual(self._values(device, "xvoltage"), [1.1, 1.2])
        self.assertEqual(fake_time.sleep_requests, [0.050])
        driver.close()

    def test_all_voltage_ramp_is_coordinated_and_publishes_actual_mapping(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver()
        fake_time.permit(10)
        status = driver.set_all_voltages(1.25, confirm=True)
        expected = [1.0 + 0.25 / 3.0, 1.0 + 0.5 / 3.0, 1.25]
        self.assertEqual(self._values(device, "allvoltage"), expected)
        self.assertEqual(
            {axis: status.axes[axis].actual_v for axis in Axis},
            {axis: 1.25 for axis in Axis},
        )
        self.assertEqual(fake_time.sleep_requests, [0.050, 0.050])
        driver.close()

    def test_unequal_all_voltage_ramp_vectorizes_safely_before_final_all_command(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver(x=1.0, y=1.2, z=1.4)
        fake_time.permit(20)
        status = driver.set_all_voltages(1.5, confirm=True)
        self.assertEqual(self._values(device, "xvoltage"), [1.1, 1.2, 1.3, 1.4])
        self.assertEqual(self._values(device, "yvoltage"), [1.26, 1.32, 1.38, 1.44])
        self.assertEqual(self._values(device, "zvoltage"), [1.42, 1.44, 1.46, 1.48])
        self.assertEqual(self._values(device, "allvoltage"), [1.5])
        query_indexes = [
            index for index, command in enumerate(device.commands)
            if command == "xvoltage?"
        ]
        self.assertEqual(len(query_indexes), 10)
        for index in query_indexes:
            self.assertEqual(device.commands[index:index + 3], [
                "xvoltage?", "yvoltage?", "zvoltage?",
            ])
        self.assertEqual(fake_time.sleep_requests, [0.050] * 4)
        self.assertEqual(
            {axis: status.axes[axis].actual_v for axis in Axis},
            {axis: 1.5 for axis in Axis},
        )
        driver.close()

    def test_master_scan_ramp_validates_and_confirms_its_contribution_on_every_axis(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        fake_time.permit(10)
        status = driver.set_master_scan_voltage(5.2, confirm=True)
        values = self._values(device, "msvoltage")
        self.assertEqual(values[-1], 5.2)
        self.assertTrue(all(
            abs(later - earlier) <= 0.1 + 1e-9
            for earlier, later in zip([5.0, *values], values)
        ))
        self.assertEqual(
            {axis: status.axes[axis].actual_v for axis in Axis},
            {Axis.X: 10.2, Axis.Y: 20.2, Axis.Z: 30.2},
        )
        self.assertEqual(status.master_scan_voltage_v, 5.2)
        driver.close()

    def test_master_scan_voltage_property_readback_gates_cache_advance(self):
        from Code.Utils.mdt693b import Axis

        for observed, controls_confirmed in ((5.0, True), (5.05, False)):
            with self.subTest(observed=observed):
                driver, device, fake_time = self._motion_driver(
                    x=10.0, y=20.0, z=30.0,
                    master_scan_v=5.0, master_scan_enabled=True,
                )
                device.response_overrides_on_occurrences[(
                    "msvoltage?", 3
                )] = (str(observed),)
                fake_time.permit(5)
                with self.assertRaises(DeviceFault):
                    driver.set_master_scan_voltage(5.1, confirm=True)
                self.assertEqual(device.commands, [
                    "msenable?", "msvoltage?",
                    "xvoltage?", "yvoltage?", "zvoltage?",
                    "msvoltage=5.1", "msvoltage?",
                    "xvoltage?", "yvoltage?", "zvoltage?",
                ])
                self.assertEqual(driver._master_scan_command_v, 5.0)
                self.assertEqual(driver.status.master_scan_voltage_v, 5.0)
                self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 10.1)
                self.assertEqual(
                    driver._motion_controls_confirmed, controls_confirmed
                )
                self.assertIn("msvoltage?", driver.status.fault_evidence)
                driver.close()

    def test_confirmation_guards_reject_before_any_wire_io(self):
        driver, device, _ = self._motion_driver()
        calls = (
            lambda: driver.set_all_voltages(1.0),
            lambda: driver.set_all_voltages(1.0, confirm=1),
            lambda: driver.set_master_scan_voltage(0.0),
            lambda: driver.set_master_scan_voltage(0.0, confirm=1),
            lambda: driver.set_master_scan_enabled(True),
            lambda: driver.set_master_scan_enabled(True, confirm=1),
        )
        for call in calls:
            with self.subTest(call=call):
                before = list(device.commands)
                with self.assertRaises(InstrumentSafetyError):
                    call()
                self.assertEqual(device.commands, before)
        driver.close()

    def test_master_scan_enable_toggle_requires_confirmed_zero_and_confirms_actuals(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._motion_driver()
        enabled = driver.set_master_scan_enabled(True, confirm=True)
        disabled = driver.set_master_scan_enabled(False, confirm=True)
        self.assertEqual(device.commands, [
            "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
            "msenable=1", "msenable?",
            "xvoltage?", "yvoltage?", "zvoltage?",
            "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
            "msenable=0", "msenable?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        self.assertTrue(enabled.master_scan_enabled)
        self.assertFalse(disabled.master_scan_enabled)
        self.assertEqual(disabled.axes[Axis.X].actual_v, 1.0)
        driver.close()

        nonzero, device, _ = self._motion_driver(
            master_scan_v=0.1, master_scan_enabled=True,
        )
        with self.assertRaises(InstrumentSafetyError):
            nonzero.set_master_scan_enabled(False, confirm=True)
        self.assertEqual(device.commands, ["msvoltage?"])
        nonzero.close()

    def test_master_scan_enable_uses_a_fresh_zero_gate_and_exact_enable_readback(self):
        driver, device, _ = self._motion_driver()
        device.master_scan_v = 5.0
        device._refresh_actual_voltages()
        with self.assertRaises(InstrumentSafetyError):
            driver.set_master_scan_enabled(True, confirm=True)
        self.assertEqual(device.commands, ["msvoltage?"])
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

        driver, device, _ = self._motion_driver()
        device.ignored_setters.add("msenable")
        with self.assertRaises(DeviceFault):
            driver.set_master_scan_enabled(True, confirm=True)
        self.assertEqual(device.commands, [
            "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
            "msenable=1", "msenable?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        self.assertFalse(driver.status.master_scan_enabled)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_master_scan_voltage_uses_fresh_enable_and_voltage_state(self):
        driver, device, _ = self._motion_driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.master_scan_enabled = False
        device._refresh_actual_voltages()
        with self.assertRaises(InstrumentSafetyError):
            driver.set_master_scan_voltage(5.2, confirm=True)
        self.assertEqual(device.commands, ["msenable?", "msvoltage?"])
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

        driver, device, _ = self._motion_driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.master_scan_v = 5.05
        device._refresh_actual_voltages()
        with self.assertRaises(DeviceFault):
            driver.set_master_scan_voltage(5.2, confirm=True)
        self.assertEqual(device.commands, [
            "msenable?", "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertFalse(driver.status.axis_command_known)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_master_scan_dynamic_gates_use_fresh_not_cached_state(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._motion_driver(
            x=1.0, y=1.0, z=1.0,
            master_scan_v=0.1, master_scan_enabled=True,
        )
        device.master_scan_v = 0.0
        device._refresh_actual_voltages()
        with self.assertRaises(DeviceFault):
            driver.set_master_scan_enabled(False, confirm=True)
        self.assertEqual(device.commands, [
            "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertFalse(driver.status.axis_command_known)
        self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 0.9)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

        driver, device, _ = self._motion_driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=False,
        )
        device.master_scan_enabled = True
        device._refresh_actual_voltages()
        with self.assertRaises(DeviceFault):
            driver.set_master_scan_voltage(5.1, confirm=True)
        self.assertEqual(device.commands, [
            "msenable?", "msvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertFalse(driver.status.axis_command_known)
        self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 15.0)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_master_scan_fresh_xyz_change_blocks_increase_and_reduction(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._motion_driver(
            x=74.9, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.axis_set_offsets_v["x"] = 1.1
        device._refresh_actual_voltages()
        with self.assertRaises(DeviceFault):
            driver.set_master_scan_voltage(5.1, confirm=True)
        self.assertEqual(device.commands, [
            "msenable?", "msvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertFalse(driver.status.axis_command_known)
        self.assertTrue(driver.status.restricted)
        self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 76.0)
        self.assertIn("X expected", driver.status.fault_evidence)
        driver.close()

        driver, device, _ = self._motion_driver(
            x=74.9, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.axis_set_offsets_v["x"] = 1.1
        device._refresh_actual_voltages()
        with self.assertRaises(DeviceFault):
            driver.set_master_scan_voltage(4.5, confirm=True)
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertFalse(driver.status.axis_command_known)
        self.assertTrue(driver.status.restricted)
        self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 76.0)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_master_scan_partial_reduction_rejects_crossing_to_other_bound_side(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._motion_driver(
            x=49.9, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        axes = dict(driver.status.axes)
        axes[Axis.X] = replace(axes[Axis.X], minimum_v=49.0, maximum_v=50.0)
        driver.status = replace(driver.status, axes=axes)
        device.axis_set_offsets_v["x"] = 1.1
        device._refresh_actual_voltages()

        with self.assertRaises(DeviceFault):
            driver.set_master_scan_voltage(2.9, confirm=True)
        self.assertEqual(device.commands, [
            "msenable?", "msvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertFalse(driver.status.axis_command_known)
        self.assertTrue(driver.status.restricted)
        self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 51.0)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_master_scan_fresh_queries_are_preflighted_before_any_io(self):
        cases = (
            ("msvoltage?", lambda driver: driver.set_master_scan_enabled(
                True, confirm=True
            )),
            ("msenable?", lambda driver: driver.set_master_scan_voltage(
                5.1, confirm=True
            )),
        )
        for missing, call in cases:
            with self.subTest(missing=missing):
                driver, device, _ = self._motion_driver(
                    x=10.0, y=20.0, z=30.0,
                    master_scan_v=5.0, master_scan_enabled=True,
                )
                driver.status = replace(
                    driver.status,
                    supported_commands=frozenset(
                        command for command in driver.status.supported_commands
                        if command != missing
                    ),
                )
                with self.assertRaises(InstrumentCapabilityError):
                    call(driver)
                self.assertEqual(device.commands, [])
                driver.close()

    def test_close_during_master_scan_fresh_query_prevents_the_setter(self):
        cases = (
            (
                "enable",
                {},
                ("msvoltage?", 2),
                lambda driver: driver.set_master_scan_enabled(True, confirm=True),
                "msenable=1",
            ),
            (
                "voltage",
                {
                    "x": 10.0, "y": 20.0, "z": 30.0,
                    "master_scan_v": 5.0, "master_scan_enabled": True,
                },
                ("msenable?", 2),
                lambda driver: driver.set_master_scan_voltage(5.1, confirm=True),
                "msvoltage=5.1",
            ),
        )
        for name, settings, gate_command, call, forbidden in cases:
            with self.subTest(name=name):
                entered = threading.Event()
                release = threading.Event()
                driver, device, fake_time = self._motion_driver(**settings)
                device.gate_read_on_occurrence = (*gate_command, entered, release)
                motion_errors: list[BaseException] = []
                close_errors: list[BaseException] = []
                motion = threading.Thread(target=lambda: self._capture(
                    lambda: call(driver), motion_errors,
                ))
                motion.start()
                self.assertTrue(entered.wait(1.0))
                closer = threading.Thread(target=lambda: self._capture(
                    driver.close, close_errors,
                ))
                closer.start()
                fake_time.permit(10)
                motion.join(2.0)
                closer.join(2.0)
                self.assertFalse(motion.is_alive() or closer.is_alive())
                self.assertEqual(close_errors, [])
                self.assertEqual(len(motion_errors), 1)
                self.assertIsInstance(motion_errors[0], InstrumentConnectionError)
                self.assertNotIn(forbidden, device.commands)
                self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_master_scan_transaction_excludes_monitor_interleaving(self):
        entered = threading.Event()
        release = threading.Event()
        device = ScriptedMDTDevice(
            supported_commands=MDT693B_REQUIRED_KEYWORDS,
            x=1.0, y=1.0, z=1.0,
            master_scan_v=0.0, master_scan_enabled=False,
        )
        driver, device, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        establish_test_axis_authority(driver, device)
        device.commands.clear()
        device.block_on_occurrence = ("msenable?", 2, entered, release)
        errors: list[BaseException] = []
        motion = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_master_scan_enabled(True, confirm=True), errors,
        ))
        motion.start()
        self.assertTrue(entered.wait(1.0))
        fake_time.permit()
        time.sleep(0.01)
        self.assertEqual(device.commands, [
            "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
            "msenable=1", "msenable?",
        ])
        release.set()
        motion.join(2.0)
        self.assertFalse(motion.is_alive())
        self.assertEqual(errors, [])
        self.assertTrue(fake_time.wait_for_sleeps(2))
        self.assertEqual(device.commands[:12], [
            "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
            "msenable=1", "msenable?",
            "xvoltage?", "yvoltage?", "zvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        close_gated_driver(driver, fake_time)

    def test_master_scan_voltage_requires_enabled_state_and_complete_axis_limits(self):
        from Code.Utils.mdt693b import Axis, AxisState

        driver, device, _ = self._motion_driver()
        with self.assertRaises(InstrumentSafetyError):
            driver.set_master_scan_voltage(0.0, confirm=True)
        self.assertEqual(device.commands, ["msenable?", "msvoltage?"])
        driver.close()

        driver, device, _ = self._motion_driver(
            x=75.0, y=75.0, z=75.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        driver.status = replace(
            driver.status,
            axes={
                axis: AxisState(state.actual_v, state.minimum_v, 75.0)
                for axis, state in driver.status.axes.items()
            },
        )
        with self.assertRaises(InstrumentSafetyError):
            driver.set_master_scan_voltage(5.01, confirm=True)
        self.assertEqual(device.commands, [
            "msenable?", "msvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        fake_status = driver.set_axis_voltage(Axis.X, 70.0)
        self.assertEqual(fake_status.axes[Axis.X].actual_v, 75.0)
        driver.close()

    def test_invalid_axis_targets_and_summed_boundaries_write_nothing(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._motion_driver(
            x=75.0, y=10.0, z=10.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        calls = (
            (lambda: driver.set_axis_voltage(Axis.X, True), []),
            (lambda: driver.set_axis_voltage(Axis.X, "1"), []),
            (lambda: driver.set_axis_voltage(Axis.X, math.nan), []),
            (lambda: driver.set_axis_voltage(Axis.X, math.inf), []),
            (lambda: driver.set_axis_voltage(Axis.X, -0.001), []),
            (lambda: driver.set_axis_voltage(Axis.X, 75.0), []),
            (lambda: driver.set_axis_voltage("x", 1.0), []),
            (lambda: driver.set_all_voltages(75.0, confirm=True), []),
            (
                lambda: driver.set_master_scan_voltage(75.0, confirm=True),
                [
                    "msenable?", "msvoltage?",
                    "xvoltage?", "yvoltage?", "zvoltage?",
                ],
            ),
        )
        for call, expected_queries in calls:
            with self.subTest(call=call):
                before = list(device.commands)
                with self.assertRaises((InstrumentProtocolError, InstrumentSafetyError)):
                    call()
                self.assertEqual(device.commands, before + expected_queries)
        driver.close()

    def test_motion_getters_query_exact_properties_and_publish_actual_values(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._motion_driver(
            x=2.0, y=3.0, z=4.0,
            master_scan_v=0.0, master_scan_enabled=True,
        )
        self.assertEqual(driver.get_axis_voltage(Axis.Y), 3.0)
        self.assertEqual(dict(driver.get_all_voltages()), {
            Axis.X: 2.0, Axis.Y: 3.0, Axis.Z: 4.0,
        })
        self.assertTrue(driver.get_master_scan_enabled())
        self.assertEqual(driver.get_master_scan_voltage(), 0.0)
        self.assertEqual(device.commands, [
            "yvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
            "msenable?", "msvoltage?",
        ])
        self.assertEqual(driver.status.axes[Axis.Z].actual_v, 4.0)
        driver.close()

    def test_getter_observed_external_deviation_disarms_next_ramp(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver()
        device.axis_set_offsets_v["x"] = -0.02
        device._refresh_actual_voltages()
        self.assertEqual(driver.get_all_voltages()[Axis.X], 0.98)
        device.commands.clear()
        with self.assertRaises(InstrumentSafetyError):
            driver.set_axis_voltage(Axis.X, 1.25)
        self.assertEqual(self._values(device, "xvoltage"), [])
        self.assertFalse(driver.status.axis_command_known)
        driver.close()

    def test_confirmed_residual_is_recomputed_after_each_bounded_step_deviation(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver()
        device.response_overrides_on_occurrences[(
            "xvoltage?", 4
        )] = ("1.07500075",)
        device.response_overrides_on_occurrences[(
            "xvoltage?", 5
        )] = ("1.07500075",)
        device.response_overrides_on_occurrences[(
            "xvoltage?", 6
        )] = ("1.1500015",)
        fake_time.permit(5)
        status = driver.set_axis_voltage(Axis.X, 1.15)
        self.assertAlmostEqual(status.axes[Axis.X].actual_v, 1.1500015)
        self.assertEqual(driver._axis_commands_v[Axis.X], 1.15)
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_actual_transition_uses_the_documented_motion_tolerance(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver(x=1.2)
        device.response_overrides_on_occurrences[(
            "xvoltage?", 4
        )] = ("1.09999925",)
        fake_time.permit(5)
        status = driver.set_axis_voltage(Axis.X, 1.1)
        self.assertAlmostEqual(status.axes[Axis.X].actual_v, 1.09999925)
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_axis_equal_all_and_master_scan_require_the_commanded_effect(self):
        from Code.Utils.mdt693b import Axis

        cases = (
            ("xvoltage", lambda driver: driver.set_axis_voltage(Axis.X, 1.1)),
            ("allvoltage", lambda driver: driver.set_all_voltages(
                1.1, confirm=True
            )),
            ("msvoltage", lambda driver: driver.set_master_scan_voltage(
                5.1, confirm=True
            )),
        )
        for keyword, call in cases:
            with self.subTest(keyword=keyword):
                settings = {}
                if keyword == "msvoltage":
                    settings = {
                        "x": 10.0, "y": 20.0, "z": 30.0,
                        "master_scan_v": 5.0, "master_scan_enabled": True,
                    }
                driver, device, fake_time = self._motion_driver(**settings)
                original_axis_commands = dict(driver._axis_commands_v)
                original_master = driver._master_scan_command_v
                device.ignored_setters.add(keyword)
                fake_time.permit(10)
                with self.assertRaises(DeviceFault):
                    call(driver)
                self.assertEqual(driver._axis_commands_v, original_axis_commands)
                self.assertEqual(driver._master_scan_command_v, original_master)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIn("commanded effect", driver.status.fault_evidence)
                driver.close()

    def test_unequal_all_requires_each_vector_and_final_commanded_effect(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver(x=1.0, y=1.2, z=1.4)
        original = dict(driver._axis_commands_v)
        device.ignored_setters.add("yvoltage")
        fake_time.permit(10)
        with self.assertRaises(DeviceFault):
            driver.set_all_voltages(1.5, confirm=True)
        self.assertIsNone(driver._axis_commands_v)
        self.assertFalse(driver.status.axis_command_known)
        self.assertEqual(self._values(device, "xvoltage"), [1.1])
        self.assertEqual(self._values(device, "yvoltage"), [1.26])
        self.assertEqual(self._values(device, "zvoltage"), [1.42])
        self.assertNotIn("allvoltage=1.5", device.commands)
        driver.close()

        driver, device, fake_time = self._motion_driver(x=1.0, y=1.2, z=1.4)
        device.ignored_setters.add("allvoltage")
        fake_time.permit(20)
        with self.assertRaises(DeviceFault):
            driver.set_all_voltages(1.5, confirm=True)
        self.assertEqual(driver._axis_commands_v, {
            Axis.X: 1.4, Axis.Y: 1.44, Axis.Z: 1.48,
        })
        self.assertEqual(self._values(device, "allvoltage"), [1.5])
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_restricted_and_fault_steps_reject_increases_on_every_axis(self):
        from Code.Utils.mdt693b import Axis

        restricted, device, fake_time = self._motion_driver(x=76.0)
        device.axis_set_offsets_v["x"] = 0.15
        fake_time.permit(15)
        with self.assertRaises(DeviceFault):
            restricted.set_axis_voltage(Axis.X, 75.0)
        self.assertAlmostEqual(restricted.status.axes[Axis.X].actual_v, 76.05)
        self.assertIn("restricted/FAULT actual increase", restricted.status.fault_evidence)
        self.assertIsNone(restricted._axis_commands_v)
        self.assertFalse(restricted.status.axis_command_known)
        restricted.close()

        faulted, device, fake_time = self._motion_driver()
        faulted.state = DriverState.FAULT
        faulted.status = replace(faulted.status, fault_evidence="latched test fault")
        device.axis_set_offsets_v["y"] = 0.05
        fake_time.permit(5)
        with self.assertRaises(DeviceFault):
            faulted.set_axis_voltage(Axis.X, 0.9)
        self.assertAlmostEqual(faulted.status.axes[Axis.Y].actual_v, 1.05)
        self.assertIn("restricted/FAULT actual increase", faulted.status.fault_evidence)
        self.assertIsNone(faulted._axis_commands_v)
        self.assertFalse(faulted.status.axis_command_known)
        faulted.close()

    def test_motion_step_count_uses_exact_ceiling_without_boundary_subtraction(self):
        from Code.Utils.mdt693b import MDT693B

        self.assertEqual(MDT693B._motion_steps(0.20000000000000018, 0.1), 3)

    def test_excess_actual_transition_faults_and_stops_without_compensation(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver()
        device.axis_set_offsets_v["x"] = 0.2
        fake_time.permit(10)
        with self.assertRaises(DeviceFault):
            driver.set_axis_voltage(Axis.X, 1.25)
        self.assertEqual(len(self._values(device, "xvoltage")), 1)
        self.assertNotIn("xvoltage=1", device.commands)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIn("actual transition", driver.status.fault_evidence)
        driver.close()

    def test_small_external_offset_that_crosses_absolute_ceiling_faults_with_actual_evidence(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver(x=74.96)
        device.axis_set_offsets_v["x"] = 0.06
        fake_time.permit(5)
        with self.assertRaises(DeviceFault):
            driver.set_axis_voltage(Axis.X, 74.95)
        self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 75.01)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(self._values(device, "xvoltage"), [74.95])
        driver.close()

    def test_mid_ramp_baseexception_retains_last_confirmed_actual_and_holds(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver()
        primary = KeyboardInterrupt("scripted interruption")
        device.effects_on_occurrences[("xvoltage=1.2", 1)] = primary
        fake_time.permit(10)
        with self.assertRaises(KeyboardInterrupt) as caught:
            driver.set_axis_voltage(Axis.X, 1.2)
        self.assertIs(caught.exception, primary)
        self.assertEqual(self._values(device, "xvoltage"), [1.1, 1.2])
        self.assertEqual(driver.status.axes[Axis.X].actual_v, 1.1)
        self.assertEqual(driver.state, DriverState.FAULT)
        before_retry = list(device.commands)
        with self.assertRaises(InstrumentSafetyError):
            driver.set_axis_voltage(Axis.X, 0.5)
        self.assertEqual(device.commands, before_retry)
        driver.close()

    def test_close_cancels_active_and_queued_motion_before_queued_write(self):
        from Code.Utils.mdt693b import Axis

        entered = threading.Event()
        release = threading.Event()
        driver, device, fake_time = self._motion_driver()
        device.block_on_occurrence = ("xvoltage=1.1", 1, entered, release)
        active_errors: list[BaseException] = []
        queued_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        active = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.X, 1.2), active_errors,
        ))
        queued = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.Y, 0.8), queued_errors,
        ))
        active.start()
        self.assertTrue(entered.wait(1.0))
        queued.start()
        closer = threading.Thread(target=lambda: self._capture(driver.close, close_errors))
        closer.start()
        deadline = time.monotonic() + 1.0
        while driver.state is not DriverState.CLOSING and time.monotonic() < deadline:
            time.sleep(0.001)
        self.assertEqual(driver.state, DriverState.CLOSING)
        release.set()
        fake_time.permit(10)
        active.join(2.0)
        queued.join(2.0)
        closer.join(2.0)
        self.assertFalse(active.is_alive() or queued.is_alive() or closer.is_alive())
        self.assertEqual(close_errors, [])
        self.assertEqual(len(active_errors), 1)
        self.assertEqual(len(queued_errors), 1)
        self.assertTrue(all(isinstance(error, InstrumentConnectionError)
                            for error in active_errors + queued_errors))
        self.assertEqual(self._values(device, "yvoltage"), [])
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_close_cancels_a_motion_read_even_when_monitor_has_already_stopped(self):
        from Code.Utils.mdt693b import Axis

        entered = threading.Event()
        release = threading.Event()
        driver, device, fake_time = self._motion_driver()
        device.gate_read_on_occurrence = ("xvoltage?", 3, entered, release)
        motion_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        motion = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.X, 1.1), motion_errors,
        ))
        motion.start()
        self.assertTrue(entered.wait(1.0))
        closer = threading.Thread(target=lambda: self._capture(driver.close, close_errors))
        closer.start()
        deadline = time.monotonic() + 1.0
        while device.cancel_read_calls == 0 and time.monotonic() < deadline:
            time.sleep(0.001)
        release.set()
        fake_time.permit(5)
        motion.join(2.0)
        closer.join(2.0)
        self.assertEqual(device.cancel_read_calls, 1)
        self.assertFalse(motion.is_alive() or closer.is_alive())
        self.assertEqual(close_errors, [])
        self.assertEqual(len(motion_errors), 1)
        self.assertIsInstance(motion_errors[0], InstrumentConnectionError)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_cancel_after_applied_write_marks_axis_unequal_and_enable_uncertain(self):
        from Code.Utils.mdt693b import Axis

        cases = (
            (
                "axis",
                {},
                ("xvoltage?", 4),
                lambda driver: driver.set_axis_voltage(Axis.X, 1.1),
            ),
            (
                "unequal-all",
                {"x": 1.0, "y": 1.2, "z": 1.4},
                ("xvoltage?", 4),
                lambda driver: driver.set_all_voltages(1.5, confirm=True),
            ),
            (
                "master-enable",
                {},
                ("msenable?", 2),
                lambda driver: driver.set_master_scan_enabled(True, confirm=True),
            ),
        )
        for name, settings, gate_command, motion_call in cases:
            with self.subTest(name=name):
                entered = threading.Event()
                release = threading.Event()
                driver, device, fake_time = self._motion_driver(**settings)
                device.gate_read_on_occurrence = (*gate_command, entered, release)
                device.close_effects.append(OSError("scripted close failure"))
                motion_errors: list[BaseException] = []
                close_errors: list[BaseException] = []
                motion = threading.Thread(target=lambda: self._capture(
                    lambda: motion_call(driver), motion_errors,
                ))
                motion.start()
                self.assertTrue(entered.wait(1.0))
                closer = threading.Thread(target=lambda: self._capture(
                    driver.close, close_errors,
                ))
                closer.start()
                fake_time.permit(20)
                motion.join(2.0)
                closer.join(2.0)
                self.assertFalse(motion.is_alive() or closer.is_alive())
                self.assertEqual(len(motion_errors), 1)
                self.assertIsInstance(motion_errors[0], InstrumentConnectionError)
                self.assertEqual(len(close_errors), 1)
                self.assertIsInstance(close_errors[0], InstrumentConnectionError)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIs(driver._serial, device)
                self.assertIsNotNone(driver.status.fault_evidence)
                self.assertFalse(driver._motion_controls_confirmed)
                before = list(device.commands)
                with self.assertRaises(InstrumentConnectionError):
                    driver.set_axis_voltage(Axis.X, 0.9)
                self.assertEqual(device.commands, before)
                driver.close()

    def test_cancel_after_master_property_readback_before_xyz_keeps_cache_uncertain(self):
        entered = threading.Event()
        release = threading.Event()
        driver, device, fake_time = self._motion_driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.gate_read_on_occurrence = ("xvoltage?", 4, entered, release)
        device.close_effects.append(OSError("scripted close failure"))
        motion_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        motion = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_master_scan_voltage(5.1, confirm=True),
            motion_errors,
        ))
        motion.start()
        self.assertTrue(entered.wait(1.0))
        self.assertEqual(device.commands[-3:], [
            "msvoltage=5.1", "msvoltage?", "xvoltage?",
        ])
        closer = threading.Thread(target=lambda: self._capture(
            driver.close, close_errors,
        ))
        closer.start()
        fake_time.permit(5)
        motion.join(2.0)
        closer.join(2.0)
        self.assertFalse(motion.is_alive() or closer.is_alive())
        self.assertEqual(len(motion_errors), 1)
        self.assertIsInstance(motion_errors[0], InstrumentConnectionError)
        self.assertEqual(len(close_errors), 1)
        self.assertIsInstance(close_errors[0], InstrumentConnectionError)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertEqual(driver._master_scan_command_v, 5.0)
        self.assertEqual(driver.status.master_scan_voltage_v, 5.0)
        self.assertFalse(driver._motion_controls_confirmed)
        self.assertIsNotNone(driver.status.fault_evidence)
        driver.close()

    def test_active_fault_cancels_queued_motion_and_preserves_primary_baseexception(self):
        from Code.Utils.mdt693b import Axis

        entered = threading.Event()
        release = threading.Event()
        driver, device, fake_time = self._motion_driver()
        device.block_on_occurrence = ("xvoltage=1.1", 1, entered, release)
        primary = SystemExit("scripted stop")
        device.effects_on_occurrences[("xvoltage=1.2", 1)] = primary
        active_errors: list[BaseException] = []
        queued_errors: list[BaseException] = []
        fake_time.permit(10)
        active = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.X, 1.2), active_errors,
        ))
        queued = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.Y, 0.8), queued_errors,
        ))
        active.start()
        self.assertTrue(entered.wait(1.0))
        queued.start()
        release.set()
        active.join(2.0)
        queued.join(2.0)
        self.assertEqual(active_errors, [primary])
        self.assertEqual(len(queued_errors), 1)
        self.assertIsInstance(queued_errors[0], InstrumentConnectionError)
        self.assertEqual(self._values(device, "yvoltage"), [])
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_restricted_and_fault_states_allow_only_provable_reductions(self):
        from Code.Utils.mdt693b import Axis

        other_axis, _, fake_time = self._motion_driver(x=76.0)
        fake_time.permit(10)
        other_reduced = other_axis.set_axis_voltage(Axis.Y, 0.8)
        self.assertEqual(other_reduced.axes[Axis.X].actual_v, 76.0)
        self.assertEqual(other_reduced.axes[Axis.Y].actual_v, 0.8)
        other_axis.close()

        restricted, device, fake_time = self._motion_driver(x=76.0)
        self.assertTrue(restricted.status.restricted)
        fake_time.permit(20)
        reduced = restricted.set_axis_voltage(Axis.X, 75.0)
        self.assertEqual(reduced.axes[Axis.X].actual_v, 75.0)
        self.assertTrue(reduced.restricted)
        restricted.close()

        faulted, device, fake_time = self._motion_driver()
        faulted.state = DriverState.FAULT
        faulted.status = replace(faulted.status, fault_evidence="latched test fault")
        fake_time.permit(10)
        reduced = faulted.set_axis_voltage(Axis.X, 0.8)
        self.assertEqual(reduced.axes[Axis.X].actual_v, 0.8)
        self.assertEqual(faulted.state, DriverState.FAULT)
        before = list(device.commands)
        with self.assertRaises(InstrumentSafetyError):
            faulted.set_axis_voltage(Axis.X, 0.9)
        self.assertEqual(device.commands, before)
        faulted.close()

    def test_restricted_axis_external_residual_disarms_base_reduction(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver(x=74.9)
        device.axis_set_offsets_v["x"] = 1.1
        device._refresh_actual_voltages()
        driver._publish_monitor_success({
            Axis.X: 76.0,
            Axis.Y: 1.0,
            Axis.Z: 1.0,
        })
        self.assertTrue(driver.status.restricted)

        with self.assertRaises(InstrumentSafetyError):
            driver.set_axis_voltage(Axis.X, 74.4)
        self.assertEqual(self._values(device, "xvoltage"), [])
        self.assertFalse(driver.status.axis_command_known)
        driver.close()

        faulted, device, fake_time = self._motion_driver(x=74.9)
        device.axis_set_offsets_v["x"] = 1.1
        device._refresh_actual_voltages()
        faulted._publish_monitor_success({
            Axis.X: 76.0,
            Axis.Y: 1.0,
            Axis.Z: 1.0,
        })
        faulted.state = DriverState.FAULT
        faulted.status = replace(
            faulted.status, fault_evidence="latched partial-reduction fault"
        )
        with self.assertRaises(InstrumentSafetyError):
            faulted.set_axis_voltage(Axis.X, 74.4)
        self.assertEqual(self._values(device, "xvoltage"), [])
        self.assertEqual(faulted.state, DriverState.FAULT)
        faulted.close()

    def test_normal_targets_use_exact_minimum_maximum_and_effective_ceilings(self):
        from Code.Utils.mdt693b import Axis, VoltageLimit

        cases = (
            (
                "axis maximum", 49.9, 0.1, 49.9000005,
                None, 0.0, 50.0, VoltageLimit.V150,
            ),
            (
                "axis minimum", 1.1, -0.1, 1.0999995,
                None, 1.0, 75.0, VoltageLimit.V150,
            ),
            (
                "application ceiling", 59.9, 0.1, 59.9000005,
                {"application_limits_v": {Axis.X: 60.0}},
                0.0, 100.0, VoltageLimit.V150,
            ),
            (
                "project 75 V ceiling", 74.9, 0.1, 74.9000005,
                None, 0.0, 100.0, VoltageLimit.V150,
            ),
            (
                "hardware 75 V ceiling", 74.9, 0.1, 74.9000005,
                None, 0.0, 100.0, VoltageLimit.V75,
            ),
        )
        for (
            name, start, residual, target, kwargs, minimum, maximum, hardware
        ) in cases:
            with self.subTest(name=name):
                driver, device, _ = self._motion_driver(
                    x=start, driver_kwargs=kwargs
                )
                axes = dict(driver.status.axes)
                axes[Axis.X] = replace(
                    axes[Axis.X], minimum_v=minimum, maximum_v=maximum
                )
                driver.status = replace(
                    driver.status, axes=axes, hardware_limit=hardware
                )
                device.axis_set_offsets_v["x"] = residual
                device._refresh_actual_voltages()
                driver._publish_monitor_success({
                    Axis.X: start + residual,
                    Axis.Y: 1.0,
                    Axis.Z: 1.0,
                })

                with self.assertRaises(InstrumentSafetyError):
                    driver.set_axis_voltage(Axis.X, target)
                self.assertEqual(device.commands, [])
                driver.close()

        driver, device, _ = self._motion_driver()
        with self.assertRaises(InstrumentSafetyError):
            driver.set_axis_voltage(Axis.X, 75.0000005)
        self.assertEqual(device.commands, [])
        driver.close()

    def test_affected_upper_violation_requires_strict_exact_reduction(self):
        from Code.Utils.mdt693b import Axis

        for state_name in ("restricted", "fault"):
            for name, target in (
                ("equal hold", 74.9),
                ("microincrease", 74.9000005),
            ):
                with self.subTest(state=state_name, name=name):
                    driver, device, _ = self._motion_driver(x=74.9)
                    device.axis_set_offsets_v["x"] = 1.1
                    device._refresh_actual_voltages()
                    driver._publish_monitor_success({
                        Axis.X: 76.0,
                        Axis.Y: 1.0,
                        Axis.Z: 1.0,
                    })
                    if state_name == "fault":
                        driver.state = DriverState.FAULT
                        driver.status = replace(
                            driver.status,
                            fault_evidence="scripted strict-reduction fault",
                        )

                    with self.assertRaises(InstrumentSafetyError):
                        driver.set_axis_voltage(Axis.X, target)
                    self.assertEqual(device.commands, [])
                    driver.close()

    def test_affected_lower_violation_has_no_task10_reduction_or_side_crossing(self):
        from Code.Utils.mdt693b import Axis

        cases = (
            ("equal hold", 1.0),
            ("microincrease repair", 1.0000005),
            ("microdecrease worsening", 0.9999995),
            ("opposite-side crossing", 3.1),
        )
        for name, target in cases:
            with self.subTest(name=name):
                driver, device, _ = self._motion_driver(x=1.0)
                axes = dict(driver.status.axes)
                axes[Axis.X] = replace(
                    axes[Axis.X], minimum_v=2.0, maximum_v=3.0
                )
                driver.status = replace(
                    driver.status,
                    axes=axes,
                    restricted=True,
                    fault_evidence="scripted lower-bound violation",
                )

                with self.assertRaises(InstrumentSafetyError):
                    driver.set_axis_voltage(Axis.X, target)
                self.assertEqual(device.commands, [])
                driver.close()

    def test_unaffected_unsafe_axes_may_hold_with_observation_tolerance(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._motion_driver(x=76.0)
        device.response_overrides_on_occurrences[
            ("xvoltage?", 4)
        ] = ("76.0000005",)
        fake_time.permit(5)
        status = driver.set_axis_voltage(Axis.Y, 0.9)
        self.assertAlmostEqual(status.axes[Axis.X].actual_v, 76.0000005)
        self.assertAlmostEqual(status.axes[Axis.Y].actual_v, 0.9)
        self.assertTrue(status.restricted)
        driver.close()

        driver, device, fake_time = self._motion_driver(x=1.0)
        axes = dict(driver.status.axes)
        axes[Axis.X] = replace(axes[Axis.X], minimum_v=2.0)
        driver.status = replace(
            driver.status,
            axes=axes,
            restricted=True,
            fault_evidence="scripted unaffected lower-bound violation",
        )
        fake_time.permit(5)
        status = driver.set_axis_voltage(Axis.Y, 0.9)
        self.assertEqual(status.axes[Axis.X].actual_v, 1.0)
        self.assertEqual(status.axes[Axis.Y].actual_v, 0.9)
        driver.close()

    def test_master_scan_treats_every_axis_as_affected_by_strict_reduction(self):
        from Code.Utils.mdt693b import Axis

        for name, target in (("equal hold", 5.0), ("microincrease", 5.0000005)):
            with self.subTest(name=name):
                driver, device, _ = self._motion_driver(
                    x=74.9,
                    y=74.9,
                    z=30.0,
                    master_scan_v=5.0,
                    master_scan_enabled=True,
                )
                device.axis_set_offsets_v["x"] = 1.1
                device.axis_set_offsets_v["y"] = 1.1
                device._refresh_actual_voltages()

                with self.assertRaises(DeviceFault):
                    driver.set_master_scan_voltage(target, confirm=True)
                self.assertEqual(device.commands, [
                    "msenable?", "msvoltage?",
                    "xvoltage?", "yvoltage?", "zvoltage?",
                ])
                self.assertFalse(any("=" in command for command in device.commands))
                self.assertFalse(driver.status.axis_command_known)
                self.assertTrue(driver.status.restricted)
                self.assertEqual(driver.state, DriverState.FAULT)
                driver.close()


class MDTAxisCommandAuthorityTests(unittest.TestCase):
    """Task 14 base-command authority and external-change regressions."""

    @staticmethod
    def _driver(*, live_monitor: bool = False, **device_kwargs: object):
        axis_offsets_v = device_kwargs.pop("axis_offsets_v", {})
        driver_kwargs = dict(device_kwargs.pop("driver_kwargs", {}))
        settings = {
            "supported_commands": MDT693B_REQUIRED_KEYWORDS,
            "x": 1.0, "y": 1.0, "z": 1.0,
            "master_scan_v": 0.0, "master_scan_enabled": False,
        }
        settings.update(device_kwargs)
        device = ScriptedMDTDevice(**settings)
        for axis_name, offset in axis_offsets_v.items():
            axis = Axis(axis_name)
            device.axis_set_offsets_v[axis_name] = float(offset)
            device.axis_base_v[axis_name] = (
                float(device.values[f"{axis.value}voltage?"]) - float(offset)
            )
        device._refresh_actual_voltages()
        driver, device, fake_time, _ = make_mdt_driver(
            device, **driver_kwargs
        )
        driver.connect()
        if not fake_time.wait_for_sleeps(1):
            raise AssertionError("monitor did not complete its initial triplet")
        if not live_monitor:
            driver._monitor_stop.set()
            fake_time.permit()
            driver._monitor_thread.join(1.0)
            if driver._monitor_thread.is_alive():
                raise AssertionError("monitor did not stop for authority test")
            driver._monitor_stop.clear()
            driver._monitor_started.set()
            driver._monitor_exited.clear()
            driver._monitor_thread = DeterministicMonitorHandle()
            fake_time.sleep_requests.clear()
        device.commands.clear()
        device.writes.clear()
        return driver, device, fake_time

    @staticmethod
    def _setters(device: ScriptedMDTDevice) -> list[str]:
        return [command for command in device.commands if "=" in command]

    @staticmethod
    def _capture(callable_: object, errors: list[BaseException]) -> None:
        try:
            callable_()  # type: ignore[operator]
        except BaseException as error:
            errors.append(error)

    def test_external_offset_at_connect_blocks_absolute_motion_without_setters(self):
        driver, device, _ = self._driver(x=2.0, axis_offsets_v={"x": 1.0})
        setters_before = self._setters(device)

        with self.assertRaises(InstrumentSafetyError):
            driver.set_axis_voltage(Axis.X, 2.1)
        with self.assertRaises(InstrumentSafetyError):
            driver.set_all_voltages(2.1, confirm=True)
        with self.assertRaises(InstrumentSafetyError):
            driver.ramp_to_zero()

        self.assertFalse(driver.status.axis_command_known)
        self.assertIsNone(driver._axis_commands_v)
        self.assertEqual(self._setters(device), setters_before)
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_channel_selection_remains_available_while_axis_authority_is_unknown(self):
        driver, device, _ = self._driver()

        self.assertEqual(driver.select_next_channel(), "Y")

        self.assertEqual(
            device.commands,
            [
                "xvoltage?", "yvoltage?", "zvoltage?", "RIGHT",
                "xvoltage?", "yvoltage?", "zvoltage?",
            ],
        )
        self.assertFalse(driver.status.axis_command_known)
        self.assertIsNone(driver._axis_commands_v)
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_arrow_increment_observation_invalidates_authority_before_arrow_write(self):
        driver, device, _ = self._driver(dac_step=50)
        driver.adopt_current_axis_baseline(confirm=True)
        device.commands.clear()
        device.writes.clear()
        device.axis_set_offsets_v["x"] = 0.05
        device._refresh_actual_voltages()

        with self.assertRaises(DeviceFault):
            driver.increment_selected()

        self.assertFalse(driver.status.axis_command_known)
        self.assertIn("authority", driver.status.fault_evidence.lower())
        self.assertFalse(any(frame.startswith(b"\x1b") for frame in device.writes))
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_rejected_arrow_observations_do_not_ratchet_authority_tolerance(self):
        driver, device, _ = self._driver(dac_step=100)
        driver.adopt_current_axis_baseline(confirm=True)
        device.commands.clear()
        device.writes.clear()

        device.axis_set_offsets_v["x"] = 0.0000009
        device._refresh_actual_voltages()
        with self.assertRaises(InstrumentSafetyError):
            driver.increment_selected()
        self.assertTrue(driver.status.axis_command_known)
        self.assertEqual(driver.state, DriverState.READY)

        device.axis_set_offsets_v["x"] = 0.0000018
        device._refresh_actual_voltages()
        with self.assertRaises(DeviceFault):
            driver.increment_selected()

        self.assertFalse(driver.status.axis_command_known)
        self.assertIn("authority", driver.status.fault_evidence.lower())
        self.assertFalse(any(frame.startswith(b"\x1b") for frame in device.writes))
        self.assertEqual(self._setters(device), [])
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_master_scan_prewrite_observation_invalidates_authority_before_setter(self):
        driver, device, _ = self._driver()
        driver.adopt_current_axis_baseline(confirm=True)
        device.commands.clear()
        device.axis_set_offsets_v["x"] = 0.05
        device._refresh_actual_voltages()

        with self.assertRaises(DeviceFault):
            driver.set_master_scan_enabled(True, confirm=True)

        self.assertFalse(driver.status.axis_command_known)
        self.assertIn("authority", driver.status.fault_evidence.lower())
        self.assertFalse(any("=" in command for command in device.commands))
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_getter_observed_manual_change_invalidates_authority_and_blocks_motion(self):
        driver, device, _ = self._driver()
        driver.adopt_current_axis_baseline(confirm=True)
        device.commands.clear()
        device.axis_set_offsets_v["x"] = 0.05
        device._refresh_actual_voltages()

        self.assertEqual(driver.get_axis_voltage(Axis.X), 1.05)
        self.assertFalse(driver.status.axis_command_known)
        self.assertIn("authority", driver.status.fault_evidence.lower())
        with self.assertRaises(InstrumentSafetyError):
            driver.set_axis_voltage(Axis.X, 1.1)
        self.assertEqual(self._setters(device), [])
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_getter_observations_do_not_ratchet_tolerance_for_slow_manual_drift(self):
        driver, device, _ = self._driver()
        driver.adopt_current_axis_baseline(confirm=True)

        device.axis_set_offsets_v["x"] = 0.0000009
        device._refresh_actual_voltages()
        driver.get_axis_voltage(Axis.X)
        self.assertTrue(driver.status.axis_command_known)

        device.axis_set_offsets_v["x"] = 0.0000018
        device._refresh_actual_voltages()
        driver.get_axis_voltage(Axis.X)

        self.assertFalse(driver.status.axis_command_known)
        self.assertIn("authority", driver.status.fault_evidence.lower())
        self.assertEqual(self._setters(device), [])
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_monitor_observed_manual_change_invalidates_authority(self):
        driver, device, fake_time = self._driver(live_monitor=True)
        driver.adopt_current_axis_baseline(confirm=True)
        device.axis_set_offsets_v["y"] = 0.05
        device._refresh_actual_voltages()

        fake_time.permit()
        self.assertTrue(fake_time.wait_for_sleeps(2))

        self.assertFalse(driver.status.axis_command_known)
        self.assertIn("authority", driver.status.fault_evidence.lower())
        self.assertEqual(self._setters(device), [])
        self.assertEqual(driver.state, DriverState.READY)
        close_gated_driver(driver, fake_time)

    def test_monitor_observations_do_not_ratchet_tolerance_for_slow_manual_drift(self):
        driver, device, fake_time = self._driver(live_monitor=True)
        driver.adopt_current_axis_baseline(confirm=True)

        device.axis_set_offsets_v["x"] = 0.0000009
        device._refresh_actual_voltages()
        fake_time.permit()
        self.assertTrue(fake_time.wait_for_sleeps(2))
        self.assertTrue(driver.status.axis_command_known)

        device.axis_set_offsets_v["x"] = 0.0000018
        device._refresh_actual_voltages()
        fake_time.permit()
        self.assertTrue(fake_time.wait_for_sleeps(3))

        self.assertFalse(driver.status.axis_command_known)
        self.assertIn("authority", driver.status.fault_evidence.lower())
        self.assertEqual(self._setters(device), [])
        self.assertEqual(driver.state, DriverState.READY)
        close_gated_driver(driver, fake_time)

    def test_manual_change_while_waiting_for_request_lock_prevents_first_setter(self):
        driver, device, _ = self._driver()
        driver.adopt_current_axis_baseline(confirm=True)
        device.commands.clear()
        request_lock = ContentionObservedRLock()
        driver._request_lock = request_lock
        errors: list[BaseException] = []

        request_lock.acquire()
        motion = threading.Thread(
            target=lambda: self._capture(
                lambda: driver.set_axis_voltage(Axis.X, 1.05), errors
            )
        )
        motion.start()
        self.assertTrue(request_lock.contention_event.wait(1.0))
        device.axis_set_offsets_v["x"] = 0.05
        device._refresh_actual_voltages()
        request_lock.release()
        motion.join(2.0)

        self.assertFalse(motion.is_alive())
        self.assertEqual(len(errors), 1)
        self.assertIsInstance(errors[0], DeviceFault)
        self.assertNotIn("xvoltage=1.05", device.commands)
        self.assertEqual(self._setters(device), [])
        self.assertFalse(driver.status.axis_command_known)
        self.assertIn("authority", driver.status.fault_evidence.lower())
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_change_during_ramp_interval_prevents_next_setter(self):
        driver, device, fake_time = self._driver()
        driver.adopt_current_axis_baseline(confirm=True)
        device.commands.clear()
        errors: list[BaseException] = []
        motion = threading.Thread(
            target=lambda: self._capture(
                lambda: driver.set_axis_voltage(Axis.X, 1.2), errors
            )
        )
        motion.start()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        device.axis_set_offsets_v["x"] = 0.05
        device._refresh_actual_voltages()
        fake_time.permit()
        motion.join(2.0)

        self.assertFalse(motion.is_alive())
        self.assertEqual(len(errors), 1)
        self.assertIsInstance(errors[0], DeviceFault)
        self.assertEqual(self._setters(device), ["xvoltage=1.1"])
        self.assertNotIn("xvoltage=1.2", device.commands)
        self.assertFalse(driver.status.axis_command_known)
        self.assertIn("authority", driver.status.fault_evidence.lower())
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_read_only_recover_sends_no_setters_and_does_not_restore_authority(self):
        driver, device, _ = self._driver()
        driver.adopt_current_axis_baseline(confirm=True)
        driver.state = DriverState.FAULT
        device.commands.clear()

        status = driver.recover()

        self.assertEqual(self._setters(device), [])
        self.assertFalse(status.axis_command_known)
        self.assertIsNone(driver._axis_commands_v)
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_emergency_zero_success_establishes_known_zero_commands(self):
        driver, device, _ = self._driver(x=2.0, y=3.0, z=4.0)

        status = driver.emergency_zero(confirm=True)

        self.assertEqual(
            self._setters(device),
            ["msvoltage=0", "allvoltage=0", "msenable=0"],
        )
        self.assertTrue(status.axis_command_known)
        self.assertEqual(driver._axis_commands_v, {axis: 0.0 for axis in Axis})
        self.assertEqual(tuple(status.axes[axis].actual_v for axis in Axis), (0.0, 0.0, 0.0))
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_emergency_zero_residual_keeps_authority_unknown_and_faults(self):
        driver, device, _ = self._driver()
        device.axis_set_offsets_v["x"] = 0.2
        device._refresh_actual_voltages()

        with self.assertRaises(DeviceFault):
            driver.emergency_zero(confirm=True)

        self.assertEqual(
            self._setters(device),
            ["msvoltage=0", "allvoltage=0", "msenable=0"],
        )
        self.assertFalse(driver.status.axis_command_known)
        self.assertIsNone(driver._axis_commands_v)
        self.assertIn("0.2", driver.status.fault_evidence)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_adoption_requires_exact_true_without_query_or_write(self):
        driver, device, _ = self._driver()
        for confirm in (False, 1, "yes", None):
            with self.subTest(confirm=confirm):
                with self.assertRaises(InstrumentSafetyError):
                    driver.adopt_current_axis_baseline(confirm=confirm)
        self.assertEqual(device.commands, [])
        self.assertFalse(driver.status.axis_command_known)
        driver.close()

    def test_adoption_rejects_enabled_or_nonzero_master_scan_without_setter(self):
        cases = (
            {"master_scan_enabled": True, "master_scan_v": 0.0},
            {"master_scan_enabled": False, "master_scan_v": 0.01},
        )
        for settings in cases:
            with self.subTest(settings=settings):
                driver, device, _ = self._driver(**settings)
                with self.assertRaises(InstrumentSafetyError):
                    driver.adopt_current_axis_baseline(confirm=True)
                self.assertEqual(self._setters(device), [])
                self.assertFalse(driver.status.axis_command_known)
                self.assertEqual(driver.state, DriverState.READY)
                driver.close()

    def test_adoption_rejects_live_limit_violations_with_truthful_restricted_status(self):
        cases = (
            ("project and hardware", {"x": 76.0}, {}, None),
            (
                "application",
                {"x": 3.0},
                {"application_limits_v": {Axis.X: 2.0}},
                None,
            ),
            ("axis minimum", {}, {}, (2.0, 75.0)),
            ("axis maximum", {}, {}, (0.0, 0.5)),
        )
        for name, actuals, driver_kwargs, axis_limits in cases:
            with self.subTest(name=name):
                driver, device, _ = self._driver(driver_kwargs=driver_kwargs)
                try:
                    for axis_name, value in actuals.items():
                        device.axis_base_v[axis_name] = value
                    device._refresh_actual_voltages()
                    if axis_limits is not None:
                        axes = dict(driver.status.axes)
                        axes[Axis.X] = AxisState(
                            axes[Axis.X].actual_v,
                            axis_limits[0],
                            axis_limits[1],
                        )
                        driver.status = replace(driver.status, axes=axes)
                    device.commands.clear()

                    with self.assertRaises(InstrumentSafetyError):
                        driver.adopt_current_axis_baseline(confirm=True)

                    self.assertEqual(
                        device.commands,
                        [
                            "msenable?", "msvoltage?",
                            "xvoltage?", "yvoltage?", "zvoltage?",
                        ],
                    )
                    self.assertEqual(self._setters(device), [])
                    self.assertTrue(driver.status.restricted)
                    self.assertIn("X=", driver.status.fault_evidence)
                    self.assertFalse(driver.status.axis_command_known)
                    self.assertIsNone(driver._axis_commands_v)

                    with self.assertRaises(InstrumentSafetyError):
                        driver.set_axis_voltage(Axis.Y, 1.05)
                    self.assertEqual(self._setters(device), [])
                    self.assertEqual(driver.state, DriverState.READY)
                finally:
                    driver.close()

    def test_query_only_adoption_records_attestation_and_enables_bounded_motion(self):
        from Code.Utils import mdt693b as mdt693b_module

        driver, device, fake_time = self._driver()

        status = driver.adopt_current_axis_baseline(confirm=True)

        self.assertEqual(device.commands, [
            "msenable?", "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        self.assertEqual(self._setters(device), [])
        self.assertTrue(status.axis_command_known)
        self.assertIn("operator", status.fault_evidence.lower())
        self.assertIn("cannot verify", status.fault_evidence.lower())
        self.assertEqual(
            status.fault_evidence,
            mdt693b_module.AXIS_BASELINE_ATTESTATION_EVIDENCE,
        )
        with self.assertRaises(AttributeError):
            status.axis_command_known = False  # type: ignore[misc]

        device.commands.clear()
        fake_time.permit(5)
        moved = driver.set_axis_voltage(Axis.X, 1.15)
        setter_values = [
            float(command.split("=", 1)[1])
            for command in self._setters(device)
        ]
        self.assertEqual(setter_values, [1.075, 1.15])
        self.assertTrue(moved.axis_command_known)
        driver.close()

    def test_factory_restore_leaves_axis_authority_unknown(self):
        driver, device, _ = self._driver()
        driver.adopt_current_axis_baseline(confirm=True)
        device.commands.clear()

        status = driver.restore_factory_defaults(confirm=True)

        self.assertEqual(device.commands[0], "restore")
        self.assertFalse(status.axis_command_known)
        self.assertIsNone(driver._axis_commands_v)
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()


class MDTArrowRestoreTests(unittest.TestCase):
    """Task 11 fixed-arrow verification and coherent factory restore."""

    @staticmethod
    def _driver(**device_kwargs: object):
        settings = {
            "supported_commands": MDT693B_REQUIRED_KEYWORDS,
            "x": 10.0, "y": 20.0, "z": 30.0,
            "master_scan_v": 0.0, "master_scan_enabled": False,
            "dac_step": 50, "selected_axis": "x",
        }
        settings.update(device_kwargs)
        device = ScriptedMDTDevice(**settings)
        driver, device, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        if not fake_time.wait_for_sleeps(1):
            raise AssertionError("monitor did not complete its initial triplet")
        driver._monitor_stop.set()
        fake_time.permit()
        driver._monitor_thread.join(1.0)
        if driver._monitor_thread.is_alive():
            raise AssertionError("monitor did not stop for deterministic Task 11 test")
        driver._monitor_stop.clear()
        driver._monitor_started.set()
        driver._monitor_exited.clear()
        driver._monitor_thread = DeterministicMonitorHandle()
        establish_test_axis_authority(driver, device)
        fake_time.sleep_requests.clear()
        device.commands.clear()
        device.writes.clear()
        return driver, device, fake_time

    @staticmethod
    def _capture(callable_: object, errors: list[BaseException]) -> None:
        try:
            callable_()  # type: ignore[operator]
        except BaseException as error:
            errors.append(error)

    def _assert_all_arrows_blocked_without_writes(
        self, driver: object, device: ScriptedMDTDevice
    ) -> None:
        writes = list(device.writes)
        for action_name in (
            "select_previous_channel", "select_next_channel",
            "increment_selected", "decrement_selected",
        ):
            with self.subTest(action_name=action_name):
                with self.assertRaises(InstrumentSafetyError):
                    getattr(driver, action_name)()
                self.assertEqual(device.writes, writes)

    def test_only_five_task11_public_apis_are_added_without_raw_escape_api(self):
        from Code.Utils.mdt693b import MDT693B

        for name in (
            "select_previous_channel", "select_next_channel",
            "increment_selected", "decrement_selected",
            "restore_factory_defaults",
        ):
            self.assertTrue(callable(getattr(MDT693B, name, None)), name)
        for forbidden in (
            "write_raw", "send_raw", "transact_raw", "arrow",
        ):
            self.assertFalse(hasattr(MDT693B, forbidden), forbidden)

    def test_selection_uses_exact_fixed_frames_returns_axis_and_proves_xyz_unchanged(self):
        driver, device, _ = self._driver()

        self.assertEqual(driver.select_previous_channel(), "Z")
        self.assertEqual(driver.select_next_channel(), "X")
        self.assertEqual(
            device.commands,
            [
                "xvoltage?", "yvoltage?", "zvoltage?", "LEFT",
                "xvoltage?", "yvoltage?", "zvoltage?",
                "xvoltage?", "yvoltage?", "zvoltage?", "RIGHT",
                "xvoltage?", "yvoltage?", "zvoltage?",
            ],
        )
        self.assertEqual(
            [frame for frame in device.writes if frame.startswith(b"\x1b")],
            [b"\x1b[D", b"\x1b[C"],
        )
        driver.close()

    def test_arrow_capability_preflight_and_rejection_latch_prevent_risky_retry(self):
        driver, device, _ = self._driver()
        driver.status = replace(
            driver.status,
            supported_commands=driver.status.supported_commands.difference({"up"}),
        )
        with self.assertRaises(InstrumentCapabilityError):
            driver.increment_selected()
        self.assertEqual(device.commands, [])
        driver.status = replace(
            driver.status, supported_commands=frozenset(
                command.lower() for command in MDT693B_REQUIRED_KEYWORDS
            )
        )
        device.arrow_prompts[b"\x1b[A"] = "!"
        with self.assertRaises(InstrumentCapabilityError):
            driver.increment_selected()
        first = list(device.commands)
        with self.assertRaises(InstrumentCapabilityError):
            driver.increment_selected()
        self.assertEqual(device.commands, first)
        self.assertEqual(device.commands.count("UP"), 1)
        driver.close()

    def test_unrecognized_arrow_framing_latches_uncertainty_and_faults_without_retry(self):
        driver, device, _ = self._driver()
        device.arrow_response_overrides[b"\x1b[A"] = ("not a selected channel",)

        with self.assertRaises(InstrumentCapabilityError):
            driver.increment_selected()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertFalse(driver._motion_controls_confirmed)
        self._assert_all_arrows_blocked_without_writes(driver, device)
        driver.close()

    def test_guessed_selection_wrapper_is_not_accepted_without_firmware_evidence(self):
        driver, device, _ = self._driver()
        device.arrow_response_overrides[b"\x1b[D"] = ("Selected: Z",)
        with self.assertRaises(InstrumentCapabilityError):
            driver.select_previous_channel()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertFalse(driver._motion_controls_confirmed)
        self.assertEqual(device.commands.count("LEFT"), 1)
        self.assertEqual(device.commands[-3:], ["xvoltage?", "yvoltage?", "zvoltage?"])
        driver.close()

    def test_selection_motion_is_a_fault_with_actual_evidence_and_no_compensation(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver()
        device.arrow_motion_overrides[b"\x1b[D"] = {"x": 0.01}
        with self.assertRaises(DeviceFault):
            driver.select_previous_channel()
        self.assertEqual(device.commands.count("LEFT"), 1)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 10.01)
        self.assertFalse(any(command in {"UP", "DOWN", "RIGHT"} for command in device.commands))
        self.assertFalse(driver._motion_controls_confirmed)
        self._assert_all_arrows_blocked_without_writes(driver, device)
        driver.close()

    def test_short_arrow_write_latches_uncertainty_and_blocks_every_arrow_before_bytes(self):
        driver, device, _ = self._driver()
        device._short_write = 1

        with self.assertRaises(InstrumentConnectionError):
            driver.increment_selected()
        self.assertFalse(driver._motion_controls_confirmed)
        device._short_write = None
        self._assert_all_arrows_blocked_without_writes(driver, device)
        driver.close()

    def test_arrow_transport_exception_latches_uncertainty_and_blocks_every_arrow_before_bytes(self):
        driver, device, _ = self._driver()
        device.effects_on_occurrences[("RIGHT", 1)] = OSError("scripted arrow write failure")

        with self.assertRaises(InstrumentConnectionError):
            driver.select_next_channel()
        self.assertFalse(driver._motion_controls_confirmed)
        self._assert_all_arrows_blocked_without_writes(driver, device)
        driver.close()

    def test_arrow_write_and_read_timeouts_keep_timeout_type_without_unsupported_latch(self):
        cases = (
            (
                "write timeout", b"\x1b[C", "RIGHT", "select_next_channel",
                serial.SerialTimeoutException("scripted arrow write timeout"),
            ),
            (
                "read timeout", b"\x1b[B", "DOWN", "decrement_selected",
                TimeoutError("scripted arrow read timeout"),
            ),
        )
        for name, candidate, command, action_name, error in cases:
            with self.subTest(name=name):
                driver, device, _ = self._driver()
                if name == "write timeout":
                    device.effects_on_occurrences[(command, 1)] = error
                else:
                    device.arrow_read_errors[candidate] = error

                with self.assertRaises(InstrumentTimeoutError):
                    getattr(driver, action_name)()
                self.assertIsNone(driver._arrow_capabilities[candidate])
                self.assertFalse(driver._motion_controls_confirmed)
                self._assert_all_arrows_blocked_without_writes(driver, device)
                driver.close()

    def test_increment_uses_fresh_dac_hardware_and_xyz_before_rejecting_unsafe_step(self):
        driver, device, _ = self._driver(hardware_limit_code=0, dac_step=10)
        device.hardware_limit_code = 2
        device.values["dacstep?"] = 44

        with self.assertRaises(InstrumentSafetyError):
            driver.increment_selected()
        self.assertEqual(
            device.commands,
            ["dacstep?", "vlimit?", "xvoltage?", "yvoltage?", "zvoltage?"],
        )
        self.assertNotIn("UP", device.commands)
        driver.close()

    def test_increment_and_decrement_confirm_selected_direction_magnitude_and_other_axes(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._driver(dac_step=50)
        up = driver.increment_selected()
        fake_time.permit()
        down = driver.decrement_selected()
        expected_step = 50 * 75.0 / 65535.0
        self.assertAlmostEqual(up.axes[Axis.X].actual_v, 10.0 + expected_step)
        self.assertAlmostEqual(down.axes[Axis.X].actual_v, 10.0)
        self.assertEqual(down.axes[Axis.Y].actual_v, 20.0)
        self.assertEqual(down.axes[Axis.Z].actual_v, 30.0)
        self.assertEqual(
            [frame for frame in device.writes if frame.startswith(b"\x1b")],
            [b"\x1b[A", b"\x1b[B"],
        )
        driver.close()

    def test_prompt_only_increment_acknowledgement_infers_the_single_changed_axis(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver(dac_step=50)
        device.arrow_response_overrides[b"\x1b[A"] = ()
        status = driver.increment_selected()
        expected_step = 50 * 75.0 / 65535.0
        self.assertAlmostEqual(status.axes[Axis.X].actual_v, 10.0 + expected_step)
        self.assertEqual(status.axes[Axis.Y].actual_v, 20.0)
        self.assertEqual(status.axes[Axis.Z].actual_v, 30.0)
        self.assertEqual(device.commands.count("UP"), 1)
        driver.close()

    def test_up_down_delta_must_match_fresh_dac_step_for_axis_and_prompt_acknowledgements(self):
        from Code.Utils.mdt693b import Axis

        cases = (
            ("axis up under-step", b"\x1b[A", "increment_selected", 0.5, False),
            ("axis down over-step", b"\x1b[B", "decrement_selected", 1.5, False),
            ("prompt up over-step", b"\x1b[A", "increment_selected", 1.5, True),
            ("prompt down under-step", b"\x1b[B", "decrement_selected", 0.5, True),
        )
        for name, candidate, action_name, scale, prompt_only in cases:
            with self.subTest(name=name):
                driver, device, _ = self._driver(dac_step=50)
                step_v = 50 * 75.0 / 65535.0
                direction = 1.0 if candidate == b"\x1b[A" else -1.0
                observed_delta = direction * step_v * scale
                device.arrow_motion_overrides[candidate] = {"x": observed_delta}
                if prompt_only:
                    device.arrow_response_overrides[candidate] = ()
                try:
                    with self.assertRaises(DeviceFault):
                        getattr(driver, action_name)()
                    self.assertAlmostEqual(
                        driver.status.axes[Axis.X].actual_v,
                        10.0 + observed_delta,
                    )
                    self.assertEqual(driver.state, DriverState.FAULT)
                    self.assertFalse(driver._motion_controls_confirmed)
                finally:
                    driver.close()

    def test_prompt_only_multi_axis_motion_faults_with_complete_actual_evidence(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver()
        device.arrow_response_overrides[b"\x1b[A"] = ()
        device.arrow_motion_overrides[b"\x1b[A"] = {"x": 0.01, "y": 0.02}
        with self.assertRaises(DeviceFault):
            driver.increment_selected()
        self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 10.01)
        self.assertAlmostEqual(driver.status.axes[Axis.Y].actual_v, 20.02)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_wrong_direction_ignored_and_overlarge_increment_each_fault_and_hold(self):
        cases = (
            ("wrong direction", {"x": -0.01}, False),
            ("ignored", {}, True),
            ("overlarge", {"x": 0.100002}, False),
        )
        for name, override, ignored in cases:
            with self.subTest(name=name):
                driver, device, _ = self._driver()
                device.arrow_motion_overrides[b"\x1b[A"] = override
                if ignored:
                    device.ignored_arrows.add(b"\x1b[A")
                with self.assertRaises(DeviceFault):
                    driver.increment_selected()
                self.assertEqual(device.commands.count("UP"), 1)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertFalse(any(
                    command in {"DOWN", "LEFT", "RIGHT"}
                    for command in device.commands
                ))
                driver.close()

    def test_consecutive_up_down_actions_are_spaced_by_at_least_fifty_ms(self):
        driver, device, fake_time = self._driver()
        driver.increment_selected()
        fake_time.permit()
        driver.decrement_selected()
        self.assertEqual(fake_time.sleep_requests, [0.050])
        self.assertEqual(device.commands.count("UP"), 1)
        self.assertEqual(device.commands.count("DOWN"), 1)
        driver.close()

    def test_close_after_arrow_bytes_latches_uncertainty_when_resource_is_retained(self):
        entered = threading.Event()
        release = threading.Event()
        driver, device, _ = self._driver()
        device.gate_read_on_occurrence = ("UP", 1, entered, release)
        device.close_effects.append(OSError("scripted close failure"))
        arrow_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        arrow = threading.Thread(target=lambda: self._capture(
            driver.increment_selected, arrow_errors,
        ))
        arrow.start()
        self.assertTrue(entered.wait(1.0))
        closer = threading.Thread(target=lambda: self._capture(driver.close, close_errors))
        closer.start()
        arrow.join(2.0)
        closer.join(2.0)
        release.set()
        self.assertFalse(arrow.is_alive() or closer.is_alive())
        self.assertEqual(len(arrow_errors), 1)
        self.assertEqual(len(close_errors), 1)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertFalse(driver._motion_controls_confirmed)
        generation = driver._operation_generation
        commands = list(device.commands)
        writes = list(device.writes)
        for action_name in (
            "select_previous_channel", "select_next_channel",
            "increment_selected", "decrement_selected",
        ):
            with self.subTest(action_name=action_name):
                with self.assertRaises(InstrumentConnectionError):
                    getattr(driver, action_name)()
                self.assertEqual(driver._operation_generation, generation)
                self.assertEqual(device.commands, commands)
                self.assertEqual(device.writes, writes)
        driver.close()

    def test_arrow_uncertainty_without_close_intent_remains_a_safety_error(self):
        driver, device, _ = self._driver()
        driver._motion_controls_confirmed = False
        generation = driver._operation_generation
        commands = list(device.commands)
        writes = list(device.writes)

        for action_name in (
            "select_previous_channel", "select_next_channel",
            "increment_selected", "decrement_selected",
        ):
            with self.subTest(action_name=action_name):
                with self.assertRaises(InstrumentSafetyError):
                    getattr(driver, action_name)()
                self.assertEqual(driver._operation_generation, generation)
                self.assertEqual(device.commands, commands)
                self.assertEqual(device.writes, writes)
        driver.close()

    def test_restore_confirmation_must_be_exact_true_and_decline_writes_nothing(self):
        driver, device, _ = self._driver()
        for confirm in (False, 1, None):
            with self.subTest(confirm=confirm):
                with self.assertRaises(InstrumentSafetyError):
                    driver.restore_factory_defaults(confirm=confirm)
                self.assertEqual(device.commands, [])
        driver.close()

    def test_confirmed_restore_uses_one_command_then_full_coherent_snapshot_and_resets_caches(self):
        from Code.Utils.mdt693b import Axis, RotaryMode

        driver, device, _ = self._driver()
        status = driver.restore_factory_defaults(confirm=True)
        self.assertEqual(device.commands, ["restore", *EXPECTED_CONNECT_QUERIES])
        self.assertEqual(device.writes[0], b"restore\r\n")
        self.assertEqual(status.friendly_name, "MDT693B")
        self.assertEqual(status.rotary_mode, RotaryMode.DEFAULT)
        self.assertEqual(
            {axis: status.axes[axis].actual_v for axis in Axis},
            {axis: 0.0 for axis in Axis},
        )
        self.assertIsNone(driver._axis_commands_v)
        self.assertFalse(status.axis_command_known)
        self.assertEqual(driver._master_scan_command_v, 0.0)
        self.assertTrue(driver._motion_controls_confirmed)
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_restore_accepts_echo_mode_transition_then_relearns_snapshot_framing(self):
        driver, device, _ = self._driver(echo_enabled=True)
        status = driver.restore_factory_defaults(confirm=True)
        self.assertFalse(status.echo_enabled)
        self.assertFalse(driver._protocol.echo_enabled)
        self.assertEqual(device.commands, ["restore", *EXPECTED_CONNECT_QUERIES])
        driver.close()

    def test_restore_overlimit_snapshot_publishes_restricted_fault_without_motion(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver()
        device.restore_axis_base_v = {"x": 80.0, "y": 0.0, "z": 0.0}
        status = driver.restore_factory_defaults(confirm=True)
        self.assertTrue(status.restricted)
        self.assertIn("X=80", status.fault_evidence)
        self.assertEqual(status.axes[Axis.X].actual_v, 80.0)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(device.commands.count("restore"), 1)
        self.assertFalse(any(command in {"UP", "DOWN", "LEFT", "RIGHT"}
                             for command in device.commands))
        driver.close()

    def test_restore_partial_snapshot_faults_without_publishing_partial_caches(self):
        driver, device, _ = self._driver()
        old_status = driver.status
        device.fail_on_occurrences["zmax?"] = {2}
        with self.assertRaises(InstrumentConnectionError):
            driver.restore_factory_defaults(confirm=True)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(driver.status.product, old_status.product)
        self.assertIn("restore", driver.status.fault_evidence.lower())
        self.assertIsNone(driver._axis_commands_v)
        self.assertIsNone(driver._master_scan_command_v)
        self.assertFalse(driver._motion_controls_confirmed)
        self.assertEqual(device.commands.count("restore"), 1)
        driver.close()

    def test_restore_cancels_active_and_queued_motion_before_either_can_continue(self):
        from Code.Utils.mdt693b import Axis

        entered = threading.Event()
        release = threading.Event()
        driver, device, fake_time = self._driver()
        device.block_on_occurrence = ("xvoltage=10.1", 1, entered, release)
        active_errors: list[BaseException] = []
        queued_errors: list[BaseException] = []
        restore_errors: list[BaseException] = []
        active = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.X, 10.2), active_errors,
        ))
        queued = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.Y, 19.8), queued_errors,
        ))
        restorer = threading.Thread(target=lambda: self._capture(
            lambda: driver.restore_factory_defaults(confirm=True), restore_errors,
        ))
        active.start()
        self.assertTrue(entered.wait(1.0))
        queued.start()
        restorer.start()
        deadline = time.monotonic() + 1.0
        while driver._operation_generation == 0 and time.monotonic() < deadline:
            time.sleep(0.001)
        self.assertGreater(driver._operation_generation, 0)
        release.set()
        fake_time.permit(10)
        active.join(2.0)
        queued.join(2.0)
        restorer.join(2.0)
        self.assertFalse(active.is_alive() or queued.is_alive() or restorer.is_alive())
        self.assertEqual(len(active_errors), 1)
        self.assertEqual(len(queued_errors), 1)
        self.assertTrue(all(isinstance(error, InstrumentConnectionError)
                            for error in active_errors + queued_errors))
        self.assertEqual(restore_errors, [])
        self.assertEqual(device.commands.count("restore"), 1)
        self.assertFalse(any(command.startswith("yvoltage=") for command in device.commands))
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_close_during_restore_resnapshot_retains_uncertain_caches_if_close_fails(self):
        entered = threading.Event()
        release = threading.Event()
        driver, device, _ = self._driver()
        device.gate_read_on_occurrence = ("xvoltage?", 3, entered, release)
        device.close_effects.append(OSError("scripted restore close failure"))
        restore_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        restorer = threading.Thread(target=lambda: self._capture(
            lambda: driver.restore_factory_defaults(confirm=True), restore_errors,
        ))
        restorer.start()
        self.assertTrue(entered.wait(1.0))
        closer = threading.Thread(target=lambda: self._capture(driver.close, close_errors))
        closer.start()
        restorer.join(2.0)
        closer.join(2.0)
        release.set()
        self.assertFalse(restorer.is_alive() or closer.is_alive())
        self.assertEqual(len(restore_errors), 1)
        self.assertIsInstance(restore_errors[0], InstrumentConnectionError)
        self.assertEqual(len(close_errors), 1)
        self.assertIsInstance(close_errors[0], InstrumentConnectionError)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertIsNone(driver._axis_commands_v)
        self.assertIsNone(driver._master_scan_command_v)
        self.assertFalse(driver._motion_controls_confirmed)
        driver.close()


class MDTFaultZeroCleanupTests(unittest.TestCase):
    """Task 12 fault recovery, explicit zero, and cleanup truth contracts."""

    @staticmethod
    def _driver(**device_kwargs: object):
        settings = {
            "supported_commands": MDT693B_REQUIRED_KEYWORDS,
            "x": 1.0, "y": 1.0, "z": 1.0,
            "master_scan_v": 0.0, "master_scan_enabled": False,
        }
        settings.update(device_kwargs)
        device = ScriptedMDTDevice(**settings)
        driver, device, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        if not fake_time.wait_for_sleeps(1):
            raise AssertionError("monitor did not complete its initial triplet")
        driver._monitor_stop.set()
        fake_time.permit()
        driver._monitor_thread.join(1.0)
        if driver._monitor_thread.is_alive():
            raise AssertionError("monitor did not stop for deterministic Task 12 test")
        driver._monitor_stop.clear()
        driver._monitor_started.set()
        driver._monitor_exited.clear()
        driver._monitor_thread = DeterministicMonitorHandle()
        establish_test_axis_authority(driver, device)
        fake_time.sleep_requests.clear()
        device.commands.clear()
        device.writes.clear()
        return driver, device, fake_time

    @staticmethod
    def _capture(callable_: object, errors: list[BaseException]) -> None:
        try:
            callable_()  # type: ignore[operator]
        except BaseException as error:
            errors.append(error)

    @staticmethod
    def _setters(device: ScriptedMDTDevice) -> list[str]:
        return [
            command for command in device.commands
            if "=" in command or command == "restore"
        ]

    @staticmethod
    def _terminal_monitor_driver():
        device = ScriptedMDTDevice(
            supported_commands=MDT693B_REQUIRED_KEYWORDS,
            x=0.0, y=0.0, z=0.0,
            master_scan_v=0.0, master_scan_enabled=False,
        )
        device.fail_on_occurrences["xvoltage?"] = {2, 3, 4}
        driver, device, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        for sleep_count in (1, 2):
            if not fake_time.wait_for_sleeps(sleep_count):
                raise AssertionError("monitor failure cycle did not reach its gate")
            fake_time.permit()
        with driver._lifecycle_condition:
            if not driver._lifecycle_condition.wait_for(
                lambda: driver.state is DriverState.FAULT, timeout=1.0
            ):
                raise AssertionError("monitor did not publish terminal FAULT")
        terminal_monitor = driver._monitor_thread
        terminal_monitor.join(1.0)
        if terminal_monitor.is_alive():
            raise AssertionError("terminal monitor did not exit")
        device.commands.clear()
        device.writes.clear()
        return driver, device, fake_time, terminal_monitor

    def test_task12_public_surface_and_emergency_exact_true_guard_are_zero_io(self):
        from Code.Utils.mdt693b import MDT693B

        for name in ("recover", "ramp_to_zero", "emergency_zero"):
            self.assertTrue(callable(getattr(MDT693B, name, None)), name)
        driver, device, _ = self._driver()
        for confirm in (False, 1, None):
            with self.subTest(confirm=confirm):
                before = list(device.commands)
                with self.assertRaises(InstrumentSafetyError):
                    driver.emergency_zero(confirm=confirm)
                self.assertEqual(device.commands, before)
        driver.close()

    def test_recover_reads_one_complete_snapshot_without_writes_and_preserves_arrow_rejection(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver()
        driver.state = DriverState.FAULT
        driver._motion_controls_confirmed = False
        driver._arrow_capabilities[b"\x1b[A"] = False
        device.axis_base_v = {"x": 2.0, "y": 3.0, "z": 4.0}
        device.master_scan_enabled = False
        device.master_scan_v = 0.0
        device._refresh_actual_voltages()

        status = driver.recover()

        self.assertEqual(device.commands, EXPECTED_CONNECT_QUERIES)
        self.assertEqual(self._setters(device), [])
        self.assertEqual(
            {axis: status.axes[axis].actual_v for axis in Axis},
            {Axis.X: 2.0, Axis.Y: 3.0, Axis.Z: 4.0},
        )
        self.assertEqual(driver.state, DriverState.READY)
        self.assertTrue(driver._motion_controls_confirmed)
        self.assertIs(driver._arrow_capabilities[b"\x1b[A"], False)
        driver.close()

    def test_recover_accepts_command_list_starting_with_question_token_in_both_echo_modes(self):
        # Real help content may start with "?", exactly like an echoed query.
        # Recovery must re-learn echo from id?/echo?, not reject the help list.
        commands = ('?', *sorted(MDT693B_REQUIRED_KEYWORDS - {'?'}))
        for echo in (False, True):
            with self.subTest(echo=echo):
                driver, device, _ = self._driver(
                    supported_commands=commands, echo_enabled=echo)
                try:
                    driver.state = DriverState.FAULT
                    try:
                        status = driver.recover()
                    except InstrumentProtocolError as error:
                        self.fail(f'A valid command list was mistaken for an echo: {error}')
                    self.assertEqual(status.echo_enabled, echo)
                    self.assertEqual(status.axes[Axis.X].actual_v, 1.0)
                    self.assertFalse(status.axis_command_known)
                    self.assertEqual(self._setters(device), [])
                    self.assertEqual(device.commands, EXPECTED_CONNECT_QUERIES)
                finally:
                    driver.close()

    def test_recover_above_ceiling_is_ready_restricted_and_never_writes(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver()
        driver.state = DriverState.FAULT
        device.axis_base_v["x"] = 80.0
        device._refresh_actual_voltages()

        status = driver.recover()

        self.assertEqual(self._setters(device), [])
        self.assertEqual(status.axes[Axis.X].actual_v, 80.0)
        self.assertTrue(status.restricted)
        self.assertIn("80", status.fault_evidence)
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_recover_partial_failure_retains_old_caches_faults_and_never_writes(self):
        driver, device, _ = self._driver()
        old_status = driver.status
        old_master = driver._master_scan_command_v
        driver.state = DriverState.FAULT
        driver._motion_controls_confirmed = False
        device.fail_on_occurrences["zmax?"] = {2}

        with self.assertRaises(InstrumentConnectionError):
            driver.recover()

        self.assertEqual(self._setters(device), [])
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(driver.status.axes, old_status.axes)
        self.assertIsNone(driver._axis_commands_v)
        self.assertFalse(driver.status.axis_command_known)
        self.assertEqual(driver._master_scan_command_v, old_master)
        self.assertFalse(driver._motion_controls_confirmed)
        driver.close()

    def test_fault_queries_remain_available_and_successful_recover_reenables_safe_motion(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._driver(x=1.0)
        driver.state = DriverState.FAULT
        driver.status = replace(driver.status, fault_evidence="scripted fault")
        self.assertEqual(driver.get_axis_voltage(Axis.X), 1.0)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver._motion_controls_confirmed = False
        before_recover = len(device.commands)
        driver.recover()
        self.assertEqual(
            device.commands[before_recover:], EXPECTED_CONNECT_QUERIES
        )
        with self.assertRaises(InstrumentSafetyError):
            driver.set_axis_voltage(Axis.X, 0.9)
        driver.adopt_current_axis_baseline(confirm=True)
        fake_time.permit(10)
        status = driver.set_axis_voltage(Axis.X, 0.9)
        self.assertAlmostEqual(status.axes[Axis.X].actual_v, 0.9)
        driver.close()

    def test_close_cancels_gated_recover_without_late_ready_publication(self):
        entered = threading.Event()
        release = threading.Event()
        driver, device, _ = self._driver()
        driver.state = DriverState.FAULT
        device.gate_read_on_occurrence = ("?", 2, entered, release)
        recover_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        worker = threading.Thread(target=lambda: self._capture(
            driver.recover, recover_errors,
        ))
        worker.start()
        self.assertTrue(entered.wait(1.0))
        closer = threading.Thread(target=lambda: self._capture(driver.close, close_errors))
        closer.start()
        worker.join(2.0)
        closer.join(2.0)
        release.set()
        self.assertFalse(worker.is_alive() or closer.is_alive())
        self.assertEqual(close_errors, [])
        self.assertEqual(len(recover_errors), 1)
        self.assertIsInstance(recover_errors[0], InstrumentConnectionError)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_recover_publication_failure_keeps_fault_and_old_command_caches(self):
        driver, device, fake_time = self._driver()
        driver.state = DriverState.FAULT
        old_master = driver._master_scan_command_v
        primary = RuntimeError("snapshot timestamp failed")
        driver._clock = ScriptedEffectClock((primary, fake_time.monotonic()))

        with self.assertRaises(RuntimeError) as caught:
            driver.recover()

        self.assertIs(caught.exception, primary)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIsNone(driver._axis_commands_v)
        self.assertFalse(driver.status.axis_command_known)
        self.assertEqual(driver._master_scan_command_v, old_master)
        self.assertEqual(self._setters(device), [])
        driver._clock = fake_time.monotonic
        driver.close()

    def test_ramp_to_zero_rejects_nonzero_minimum_before_any_setter(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver()
        axes = dict(driver.status.axes)
        axes[Axis.Y] = replace(axes[Axis.Y], minimum_v=0.01)
        driver.status = replace(driver.status, axes=axes)

        with self.assertRaises(InstrumentSafetyError):
            driver.ramp_to_zero()

        self.assertEqual(device.commands, [])
        self.assertEqual(driver.state, DriverState.READY)
        self.assertTrue(driver._motion_controls_confirmed)
        driver.close()

    def test_ramp_to_zero_late_safety_error_after_writes_faults_without_compensation(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._driver(
            x=0.2, y=0.2, z=0.2,
            master_scan_v=0.1, master_scan_enabled=True,
        )
        # Connect is occurrence 1, the fresh zero snapshot is 2, Master Scan
        # refresh/confirmation are 3/4, and disable refresh is occurrence 5.
        device.response_overrides_on_occurrences[("msvoltage?", 5)] = ("0.2",)
        fake_time.permit(20)

        with self.assertRaises(DeviceFault):
            driver.ramp_to_zero()

        self.assertEqual(
            self._setters(device), ["msvoltage=0"]
        )
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertFalse(driver._motion_controls_confirmed)
        self.assertEqual(
            tuple(driver.status.axes[axis].actual_v for axis in Axis),
            (0.1, 0.1, 0.1),
        )
        self.assertEqual(driver.status.master_scan_voltage_v, 0.2)
        self.assertEqual(driver._master_scan_command_v, 0.0)
        self.assertIsNone(driver._axis_commands_v)
        driver.close()

    def test_recover_after_terminal_monitor_fault_restarts_live_two_hz_monitor(self):
        driver, device, fake_time, terminal_monitor = self._terminal_monitor_driver()

        status = driver.recover()

        replacement = driver._monitor_thread
        self.assertEqual(driver.state, DriverState.READY)
        self.assertIs(driver.status, status)
        self.assertIsNot(replacement, terminal_monitor)
        self.assertTrue(replacement.is_alive())
        self.assertTrue(fake_time.wait_for_sleeps(3))
        self.assertEqual(self._setters(device), [])
        close_gated_driver(driver, fake_time)

    def test_other_ready_publishers_restart_terminal_monitor_without_duplicates(self):
        operations = (
            lambda driver: driver.restore_factory_defaults(confirm=True),
            lambda driver: driver.emergency_zero(confirm=True),
        )
        for operation in operations:
            with self.subTest(operation=operation.__code__.co_firstlineno):
                driver, _, fake_time, terminal_monitor = (
                    self._terminal_monitor_driver()
                )
                try:
                    operation(driver)

                    replacement = driver._monitor_thread
                    self.assertEqual(driver.state, DriverState.READY)
                    self.assertIsNot(replacement, terminal_monitor)
                    self.assertTrue(replacement.is_alive())
                    self.assertTrue(fake_time.wait_for_sleeps(3))
                    self.assertEqual(
                        sum(
                            thread.name == "MDT693BMonitor" and thread.is_alive()
                            for thread in threading.enumerate()
                        ),
                        1,
                    )
                finally:
                    close_gated_driver(driver, fake_time)

    def test_emergency_generation_cancels_getter_and_config_admitted_during_gate(self):
        entered = threading.Event()
        release = threading.Event()
        driver, device, _ = self._driver()
        observed_lock = ContentionObservedRLock()
        driver._request_lock = observed_lock
        device.gate_read_on_occurrence = ("msvoltage=0", 1, entered, release)
        emergency_errors: list[BaseException] = []
        getter_errors: list[BaseException] = []
        config_errors: list[BaseException] = []
        emergency = threading.Thread(target=lambda: self._capture(
            lambda: driver.emergency_zero(confirm=True), emergency_errors,
        ))
        getter = threading.Thread(target=lambda: self._capture(
            driver.get_serial_number, getter_errors,
        ))
        config = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_friendly_name("queued"), config_errors,
        ))

        emergency.start()
        self.assertTrue(entered.wait(1.0))
        getter.start()
        config.start()
        self.assertTrue(observed_lock.wait_for_contentions(2))
        release.set()
        emergency.join(2.0)
        getter.join(2.0)
        config.join(2.0)

        self.assertFalse(emergency.is_alive() or getter.is_alive() or config.is_alive())
        self.assertEqual(emergency_errors, [])
        self.assertEqual(len(getter_errors), 1)
        self.assertEqual(len(config_errors), 1)
        self.assertIsInstance(getter_errors[0], InstrumentConnectionError)
        self.assertIsInstance(config_errors[0], InstrumentConnectionError)
        self.assertNotIn("serial?", device.commands)
        self.assertNotIn("friendly=queued", device.commands)

        self.assertEqual(driver.get_serial_number(), "SN123")
        self.assertEqual(driver.set_friendly_name("after"), "after")
        driver.close()

    def test_close_boundedly_cancels_blocked_getter_after_monitor_has_exited(self):
        entered = threading.Event()
        release = threading.Event()
        cancel_entered = threading.Event()
        cancel_release = threading.Event()
        driver, device, _ = self._driver()
        device.gate_read_on_occurrence = ("serial?", 2, entered, release)
        device.cancel_read_gate = (cancel_entered, cancel_release)
        getter_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        getter = threading.Thread(target=lambda: self._capture(
            driver.get_serial_number, getter_errors,
        ))
        closer = threading.Thread(target=lambda: self._capture(
            driver.close, close_errors,
        ))

        getter.start()
        self.assertTrue(entered.wait(1.0))
        closer.start()
        cancellation_started = cancel_entered.wait(0.2)
        if cancellation_started:
            cancel_release.set()
        else:
            release.set()
        getter.join(2.0)
        closer.join(2.0)
        release.set()

        self.assertTrue(cancellation_started)
        self.assertFalse(getter.is_alive() or closer.is_alive())
        self.assertEqual(len(getter_errors), 1)
        self.assertIsInstance(getter_errors[0], InstrumentConnectionError)
        self.assertEqual(close_errors, [])
        self.assertEqual(device.cancel_read_calls, 1)
        self.assertTrue(device.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_ramp_to_zero_uses_bounded_master_then_axes_then_disable_and_confirms_zero(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._driver(
            x=1.2, y=1.2, z=1.2,
            master_scan_v=0.2, master_scan_enabled=True,
        )
        fake_time.permit(100)

        status = driver.ramp_to_zero()

        setters = self._setters(device)
        self.assertEqual(setters[-1], "msenable=0")
        master_values = [
            float(command.split("=", 1)[1])
            for command in setters if command.startswith("msvoltage=")
        ]
        all_values = [
            float(command.split("=", 1)[1])
            for command in setters if command.startswith("allvoltage=")
        ]
        self.assertEqual(master_values[-1], 0.0)
        self.assertEqual(all_values[-1], 0.0)
        self.assertLess(setters.index("msvoltage=0"), setters.index("allvoltage=0"))
        self.assertLess(setters.index("allvoltage=0"), setters.index("msenable=0"))
        self.assertTrue(all(
            abs(after - before) <= 0.1 + 1e-9
            for before, after in zip([0.2, *master_values], master_values)
        ))
        self.assertTrue(all(
            abs(after - before) <= 0.1 + 1e-9
            for before, after in zip([1.0, *all_values], all_values)
        ))
        self.assertEqual(
            tuple(status.axes[axis].actual_v for axis in Axis),
            (0.0, 0.0, 0.0),
        )
        self.assertFalse(status.master_scan_enabled)
        driver.close()

    def test_ramp_to_zero_external_change_faults_before_any_setter(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._driver()
        device.axis_set_offsets_v["x"] = 0.01
        device._refresh_actual_voltages()
        fake_time.permit(100)

        with self.assertRaises(DeviceFault):
            driver.ramp_to_zero()

        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertAlmostEqual(driver.status.axes[Axis.X].actual_v, 1.01)
        setters = self._setters(device)
        self.assertEqual(setters, [])
        self.assertFalse(driver.status.axis_command_known)
        driver.close()

    def test_ramp_to_zero_disabled_nonzero_master_is_already_zero_contribution(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time = self._driver(
            x=1.0, y=1.0, z=1.0,
            master_scan_v=5.0, master_scan_enabled=False,
        )
        fake_time.permit(100)

        status = driver.ramp_to_zero()

        self.assertFalse(status.master_scan_enabled)
        self.assertEqual(status.master_scan_voltage_v, 5.0)
        self.assertEqual(
            tuple(status.axes[axis].actual_v for axis in Axis),
            (0.0, 0.0, 0.0),
        )
        self.assertFalse(any(
            command.startswith("msvoltage=") or command.startswith("msenable=")
            for command in self._setters(device)
        ))
        driver.close()

    def test_ramp_to_zero_spaces_master_and_axis_voltage_phases(self):
        driver, _, fake_time = self._driver(
            x=1.2, y=1.2, z=1.2,
            master_scan_v=0.2, master_scan_enabled=True,
        )
        fake_time.permit(100)

        driver.ramp_to_zero()

        # 2 Master Scan steps => 1 internal interval; one phase boundary;
        # 10 all-voltage steps => 9 internal intervals.
        self.assertEqual(fake_time.sleep_requests, [0.050] * 11)
        driver.close()

    def test_close_cancels_gated_zero_ramp_and_sends_no_compensation(self):
        entered = threading.Event()
        release = threading.Event()
        driver, device, fake_time = self._driver(x=0.1, y=0.1, z=0.1)
        device.gate_read_on_occurrence = ("allvoltage=0", 1, entered, release)
        ramp_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        worker = threading.Thread(target=lambda: self._capture(
            driver.ramp_to_zero, ramp_errors,
        ))
        worker.start()
        self.assertTrue(entered.wait(1.0))
        closer = threading.Thread(target=lambda: self._capture(driver.close, close_errors))
        closer.start()
        fake_time.permit(20)
        worker.join(2.0)
        closer.join(2.0)
        release.set()
        self.assertFalse(worker.is_alive() or closer.is_alive())
        self.assertEqual(close_errors, [])
        self.assertEqual(len(ramp_errors), 1)
        self.assertIsInstance(ramp_errors[0], InstrumentConnectionError)
        self.assertEqual(self._setters(device), ["allvoltage=0"])
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_close_cancels_zero_ramp_during_read_only_preflight_snapshot(self):
        entered = threading.Event()
        release = threading.Event()
        cancel_entered = threading.Event()
        cancel_release = threading.Event()
        driver, device, _ = self._driver()
        device.gate_read_on_occurrence = ("?", 2, entered, release)
        device.cancel_read_gate = (cancel_entered, cancel_release)
        ramp_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        worker = threading.Thread(target=lambda: self._capture(
            driver.ramp_to_zero, ramp_errors,
        ))
        worker.start()
        self.assertTrue(entered.wait(1.0))
        closer = threading.Thread(target=lambda: self._capture(driver.close, close_errors))
        closer.start()
        cancellation_started = cancel_entered.wait(0.2)
        if cancellation_started:
            cancel_release.set()
        else:
            release.set()
            cancel_release.set()
        worker.join(2.0)
        closer.join(2.0)
        release.set()
        self.assertTrue(cancellation_started)
        self.assertFalse(worker.is_alive() or closer.is_alive())
        self.assertEqual(close_errors, [])
        self.assertEqual(len(ramp_errors), 1)
        self.assertIsInstance(ramp_errors[0], InstrumentConnectionError)
        self.assertEqual(self._setters(device), [])
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_emergency_zero_sends_exact_three_setters_in_order_and_confirms_actuals(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )

        status = driver.emergency_zero(confirm=True)

        self.assertEqual(self._setters(device), [
            "msvoltage=0", "allvoltage=0", "msenable=0",
        ])
        self.assertEqual(device.commands[-3:], [
            "xvoltage?", "yvoltage?", "zvoltage?",
        ])
        self.assertEqual(
            tuple(status.axes[axis].actual_v for axis in Axis),
            (0.0, 0.0, 0.0),
        )
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_emergency_partial_write_faults_latches_uncertainty_and_sends_no_compensation(self):
        driver, device, _ = self._driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.short_write_on_occurrences[("msvoltage=0", 1)] = 1

        with self.assertRaises(InstrumentConnectionError):
            driver.emergency_zero(confirm=True)

        self.assertEqual(device.commands, ["msvoltage=0"])
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertFalse(driver._motion_controls_confirmed)
        driver.close()

    def test_emergency_second_setter_timeout_holds_and_never_sends_third(self):
        driver, device, _ = self._driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.effects_on_occurrences[("allvoltage=0", 1)] = TimeoutError(
            "scripted emergency timeout"
        )

        with self.assertRaises(InstrumentTimeoutError):
            driver.emergency_zero(confirm=True)

        self.assertEqual(self._setters(device), ["msvoltage=0", "allvoltage=0"])
        self.assertNotIn("msenable=0", device.commands)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertFalse(driver._motion_controls_confirmed)
        driver.close()

    def test_emergency_external_residual_records_actuals_and_never_claims_ready(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.axis_set_offsets_v["z"] = 0.02
        device._refresh_actual_voltages()

        with self.assertRaises(DeviceFault):
            driver.emergency_zero(confirm=True)

        self.assertEqual(self._setters(device), [
            "msvoltage=0", "allvoltage=0", "msenable=0",
        ])
        self.assertAlmostEqual(driver.status.axes[Axis.Z].actual_v, 0.02)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertFalse(driver._motion_controls_confirmed)
        driver.close()

    def test_emergency_capabilities_are_preflighted_before_any_io(self):
        driver, device, _ = self._driver()
        driver.status = replace(
            driver.status,
            supported_commands=driver.status.supported_commands.difference(
                {"allvoltage="}
            ),
        )

        with self.assertRaises(InstrumentCapabilityError):
            driver.emergency_zero(confirm=True)

        self.assertEqual(device.commands, [])
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_emergency_cancels_active_queued_and_during_work_but_post_completion_work_runs(self):
        from Code.Utils.mdt693b import Axis
        from Code.Utils.mdt693b import MDT693B

        self.assertTrue(callable(getattr(MDT693B, "emergency_zero", None)))

        entered = threading.Event()
        release = threading.Event()
        emergency_entered = threading.Event()
        emergency_release = threading.Event()
        driver, device, fake_time = self._driver()
        device.block_on_occurrence = ("xvoltage=1.1", 1, entered, release)
        active_errors: list[BaseException] = []
        queued_errors: list[BaseException] = []
        during_errors: list[BaseException] = []
        emergency_errors: list[BaseException] = []
        active = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.X, 1.2), active_errors,
        ))
        queued = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.Y, 0.8), queued_errors,
        ))
        active.start()
        self.assertTrue(entered.wait(1.0))
        queued.start()
        emergency = threading.Thread(target=lambda: self._capture(
            lambda: driver.emergency_zero(confirm=True), emergency_errors,
        ))
        emergency.start()
        deadline = time.monotonic() + 1.0
        while driver._operation_generation == 0 and time.monotonic() < deadline:
            time.sleep(0.001)
        self.assertGreater(driver._operation_generation, 0)
        device.block_on_occurrence = (
            "allvoltage=0", 1, emergency_entered, emergency_release,
        )
        release.set()
        fake_time.permit(100)
        self.assertTrue(emergency_entered.wait(1.0))
        during = threading.Thread(target=lambda: self._capture(
            lambda: driver.set_axis_voltage(Axis.Z, 0.1), during_errors,
        ))
        during.start()
        emergency_release.set()
        for thread in (active, queued, during, emergency):
            thread.join(2.0)
            self.assertFalse(thread.is_alive())
        self.assertEqual(emergency_errors, [])
        self.assertEqual(len(active_errors), 1)
        self.assertEqual(len(queued_errors), 1)
        self.assertEqual(len(during_errors), 1)
        self.assertTrue(all(
            isinstance(error, InstrumentConnectionError)
            for error in active_errors + queued_errors + during_errors
        ))
        fake_time.permit(10)
        status = driver.set_axis_voltage(Axis.Z, 0.1)
        self.assertAlmostEqual(status.axes[Axis.Z].actual_v, 0.1)
        driver.close()

    def test_monitor_fault_wins_gated_emergency_and_never_resurrects_ready(self):
        from Code.Utils.mdt693b import MDT693B

        self.assertTrue(callable(getattr(MDT693B, "emergency_zero", None)))
        entered = threading.Event()
        release = threading.Event()
        driver, device, _ = self._driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.gate_read_on_occurrence = ("msvoltage=0", 1, entered, release)
        errors: list[BaseException] = []
        worker = threading.Thread(target=lambda: self._capture(
            lambda: driver.emergency_zero(confirm=True), errors,
        ))
        worker.start()
        self.assertTrue(entered.wait(1.0))
        driver._publish_monitor_failure(OSError("one"), 1)
        driver._publish_monitor_failure(OSError("two"), 2)
        driver._publish_monitor_failure(OSError("three"), 3)
        release.set()
        worker.join(2.0)
        self.assertFalse(worker.is_alive())
        self.assertEqual(len(errors), 1)
        self.assertIsInstance(errors[0], InstrumentConnectionError)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertFalse(driver._motion_controls_confirmed)
        driver.close()

    def test_close_wins_gated_emergency_and_normal_close_never_adds_a_setter(self):
        from Code.Utils.mdt693b import MDT693B

        self.assertTrue(callable(getattr(MDT693B, "emergency_zero", None)))
        entered = threading.Event()
        release = threading.Event()
        driver, device, _ = self._driver(
            x=10.0, y=20.0, z=30.0,
            master_scan_v=5.0, master_scan_enabled=True,
        )
        device.gate_read_on_occurrence = ("msvoltage=0", 1, entered, release)
        emergency_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        emergency = threading.Thread(target=lambda: self._capture(
            lambda: driver.emergency_zero(confirm=True), emergency_errors,
        ))
        emergency.start()
        self.assertTrue(entered.wait(1.0))
        closer = threading.Thread(target=lambda: self._capture(driver.close, close_errors))
        closer.start()
        emergency.join(2.0)
        closer.join(2.0)
        release.set()
        self.assertFalse(emergency.is_alive() or closer.is_alive())
        self.assertEqual(close_errors, [])
        self.assertEqual(len(emergency_errors), 1)
        self.assertIsInstance(emergency_errors[0], InstrumentConnectionError)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        setters_after_emergency = list(self._setters(device))
        driver.close()
        self.assertEqual(self._setters(device), setters_after_emergency)

    def test_serial_close_failure_retains_resource_then_retry_disconnects_without_writes(self):
        driver, device, _ = self._driver()
        device.close_effects = [OSError("scripted close failure"), None]
        before = list(self._setters(device))

        with self.assertRaises(InstrumentConnectionError):
            driver.close()

        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertFalse(device.closed)
        self.assertEqual(self._setters(device), before)
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertTrue(device.closed)
        self.assertEqual(self._setters(device), before)

    def test_serial_close_returning_while_still_open_is_a_truthful_retryable_fault(self):
        driver, device, _ = self._driver()
        real_close = device.close
        device.close = lambda: None  # type: ignore[method-assign]

        with self.assertRaises(DeviceFault):
            driver.close()

        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertTrue(device.is_open)
        device.close = real_close  # type: ignore[method-assign]
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_context_body_exception_stays_primary_and_retains_cleanup_error(self):
        body_error = RuntimeError("body is primary")
        close_error = KeyboardInterrupt("close cleanup interrupted")
        driver, device, _ = self._driver()
        device.close_effects = [close_error, None]

        self.assertFalse(driver.__exit__(RuntimeError, body_error, None))

        self.assertIs(getattr(body_error, "mdt_cleanup_error", None), close_error)
        self.assertIs(driver.cleanup_error, close_error)
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_connect_wait_cleanup_baseexceptions_retain_resource_and_fault_truth(self):
        effects = (
            RuntimeError("connect wait failed"),
            KeyboardInterrupt("connect wait interrupted"),
            SystemExit("connect wait exited"),
        )
        for effect in effects:
            with self.subTest(effect=type(effect).__name__):
                driver, device, _ = self._driver()
                original_wait = driver._connect_done.wait
                driver._connect_done.clear()

                def fail_wait(timeout: float, *, _effect: BaseException = effect) -> bool:
                    raise _effect

                driver._connect_done.wait = fail_wait  # type: ignore[method-assign]
                with self.assertRaises(type(effect)) as caught:
                    driver.close()
                self.assertIs(caught.exception, effect)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIs(driver.cleanup_error, effect)
                self.assertIs(driver._serial, device)
                self.assertFalse(device.closed)
                driver._connect_done.wait = original_wait  # type: ignore[method-assign]
                driver._connect_done.set()
                driver.close()

    def test_monitor_join_cleanup_baseexceptions_retain_resource_and_fault_truth(self):
        class JoinEffect:
            def __init__(self, effect: BaseException) -> None:
                self.effect = effect

            def is_alive(self) -> bool:
                return True

            def join(self, timeout: float) -> None:
                raise self.effect

        effects = (
            RuntimeError("join failed"),
            KeyboardInterrupt("join interrupted"),
            SystemExit("join exited"),
        )
        for effect in effects:
            with self.subTest(effect=type(effect).__name__):
                driver, device, _ = self._driver()
                driver._monitor_thread = JoinEffect(effect)  # type: ignore[assignment]
                with self.assertRaises(type(effect)) as caught:
                    driver.close()
                self.assertIs(caught.exception, effect)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIs(driver.cleanup_error, effect)
                self.assertIs(driver._serial, device)
                self.assertFalse(device.closed)
                driver._monitor_thread = None
                driver.close()

    def test_monitor_liveness_baseexceptions_retain_primary_and_retryable_close_truth(self):
        class LivenessEffects:
            def __init__(self, effects: list[bool | BaseException]) -> None:
                self.effects = effects

            def is_alive(self) -> bool:
                effect = self.effects.pop(0)
                if isinstance(effect, BaseException):
                    raise effect
                return effect

            def join(self, timeout: float) -> None:
                return None

        for phase in ("before_join", "after_join"):
            for effect in (
                RuntimeError(f"{phase} liveness failed"),
                KeyboardInterrupt(f"{phase} liveness interrupted"),
                SystemExit(f"{phase} liveness exited"),
            ):
                with self.subTest(phase=phase, effect=type(effect).__name__):
                    driver, device, _ = self._driver()
                    sequence: list[bool | BaseException] = (
                        [effect] if phase == "before_join" else [True, effect]
                    )
                    driver._monitor_thread = LivenessEffects(sequence)  # type: ignore[assignment]

                    with self.assertRaises(type(effect)) as caught:
                        driver.close()

                    self.assertIs(caught.exception, effect)
                    self.assertEqual(driver.state, DriverState.FAULT)
                    self.assertIs(driver.cleanup_error, effect)
                    self.assertIs(driver._serial, device)
                    self.assertFalse(device.closed)
                    driver._monitor_thread = None
                    driver.close()

    def test_failed_connect_liveness_cleanup_is_secondary_and_retains_open_resource(self):
        from Code.Utils.common import InstrumentProtocolError
        from Code.Utils.mdt693b import MDT693B

        class LivenessEffects:
            def __init__(self, effects: list[bool | BaseException]) -> None:
                self.effects = effects

            def is_alive(self) -> bool:
                effect = self.effects.pop(0)
                if isinstance(effect, BaseException):
                    raise effect
                return effect

            def join(self, timeout: float) -> None:
                return None

        for phase in ("before_join", "after_join"):
            for cleanup_effect in (
                RuntimeError(f"failed-connect {phase} liveness failed"),
                KeyboardInterrupt(f"failed-connect {phase} interrupted"),
                SystemExit(f"failed-connect {phase} exited"),
            ):
                with self.subTest(phase=phase, effect=type(cleanup_effect).__name__):
                    device = ScriptedMDTDevice(
                        supported_commands=MDT693B_REQUIRED_KEYWORDS
                    )
                    driver = MDT693B(
                        "COM_TEST", serial_factory=RecordingSerialFactory(device)
                    )
                    primary = InstrumentProtocolError("snapshot failed first")
                    sequence: list[bool | BaseException] = (
                        [cleanup_effect]
                        if phase == "before_join"
                        else [True, cleanup_effect]
                    )

                    def fail_snapshot(*args: object, **kwargs: object):
                        driver._monitor_thread = LivenessEffects(sequence)  # type: ignore[assignment]
                        raise primary

                    driver._snapshot_from_queries = fail_snapshot  # type: ignore[method-assign]
                    with self.assertRaises(InstrumentProtocolError) as caught:
                        driver.connect()

                    self.assertIs(caught.exception, primary)
                    self.assertIs(
                        getattr(primary, "mdt_cleanup_error", None), cleanup_effect
                    )
                    self.assertIs(driver.cleanup_error, cleanup_effect)
                    self.assertEqual(driver.state, DriverState.FAULT)
                    self.assertIs(driver._serial, device)
                    self.assertFalse(device.closed)
                    driver._monitor_thread = None
                    driver.close()

    def test_recover_partial_snapshot_retains_fresh_xyz_and_overlimit_evidence_only(self):
        from Code.Utils.mdt693b import Axis

        driver, device, _ = self._driver()
        driver.state = DriverState.FAULT
        driver._motion_controls_confirmed = False
        confirmed_master_command = driver._master_scan_command_v
        device.axis_base_v = {"x": 80.0, "y": 2.0, "z": 3.0}
        device.master_scan_enabled = False
        device.master_scan_v = 0.0
        device._refresh_actual_voltages()
        device.fail_on_occurrences["zmax?"] = {2}

        with self.assertRaises(InstrumentConnectionError):
            driver.recover()

        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertFalse(driver._motion_controls_confirmed)
        self.assertEqual(
            tuple(driver.status.axes[axis].actual_v for axis in Axis),
            (80.0, 2.0, 3.0),
        )
        self.assertTrue(driver.status.restricted)
        self.assertIn("80", driver.status.fault_evidence)
        self.assertIsNone(driver._axis_commands_v)
        self.assertFalse(driver.status.axis_command_known)
        self.assertEqual(driver._master_scan_command_v, confirmed_master_command)
        self.assertEqual(self._setters(device), [])
        driver.close()

    def test_emergency_owner_rejects_restore_and_second_emergency_at_every_phase(self):
        phases = (
            ("msvoltage=0", 1),
            ("allvoltage=0", 1),
            ("msenable=0", 1),
            ("xvoltage?", 3),
        )
        contenders = (
            lambda driver: driver.restore_factory_defaults(confirm=True),
            lambda driver: driver.emergency_zero(confirm=True),
            lambda driver: driver.recover(),
            lambda driver: driver.ramp_to_zero(),
        )
        for phase in phases:
            for contender_call in contenders:
                with self.subTest(
                    phase=phase,
                    contender=contender_call.__code__.co_firstlineno,
                ):
                    entered = threading.Event()
                    release = threading.Event()
                    driver, device, _ = self._driver(
                        x=0.0, y=0.0, z=0.0,
                        master_scan_v=0.0, master_scan_enabled=False,
                    )
                    observed_lock = ContentionObservedRLock()
                    driver._request_lock = observed_lock
                    device.gate_read_on_occurrence = (*phase, entered, release)
                    owner_errors: list[BaseException] = []
                    contender_errors: list[BaseException] = []
                    owner = threading.Thread(target=lambda: self._capture(
                        lambda: driver.emergency_zero(confirm=True), owner_errors,
                    ))

                    def contend() -> None:
                        try:
                            self._capture(
                                lambda: contender_call(driver), contender_errors
                            )
                        finally:
                            observed_lock.contention_event.set()

                    contender = threading.Thread(target=contend)
                    owner.start()
                    self.assertTrue(entered.wait(1.0))
                    owner_generation = driver._operation_generation
                    contender.start()
                    self.assertTrue(observed_lock.contention_event.wait(1.0))
                    generation_before_release = driver._operation_generation
                    release.set()
                    owner.join(2.0)
                    contender.join(2.0)

                    self.assertFalse(owner.is_alive() or contender.is_alive())
                    self.assertEqual(generation_before_release, owner_generation)
                    self.assertEqual(owner_errors, [])
                    self.assertEqual(len(contender_errors), 1)
                    self.assertIsInstance(
                        contender_errors[0], InstrumentConnectionError
                    )
                    self.assertEqual(self._setters(device), [
                        "msvoltage=0", "allvoltage=0", "msenable=0",
                    ])
                    driver.close()

    def test_high_impact_operations_are_allowed_after_emergency_completion(self):
        driver, device, _ = self._driver(
            x=0.0, y=0.0, z=0.0,
            master_scan_v=0.0, master_scan_enabled=False,
        )

        driver.emergency_zero(confirm=True)
        driver.restore_factory_defaults(confirm=True)
        driver.emergency_zero(confirm=True)

        self.assertEqual(self._setters(device), [
            "msvoltage=0", "allvoltage=0", "msenable=0",
            "restore",
            "msvoltage=0", "allvoltage=0", "msenable=0",
        ])
        self.assertEqual(driver.state, DriverState.READY)
        driver.close()

    def test_emergency_failure_keeps_owner_admission_until_fault_is_published(self):
        fault_entered = threading.Event()
        fault_release = threading.Event()
        driver, device, _ = self._driver()
        observed_lock = ContentionObservedRLock()
        driver._request_lock = observed_lock
        device.effects_on_occurrences[("msvoltage=0", 1)] = OSError(
            "emergency write failed"
        )
        original_publish_fault = driver._publish_operation_fault

        def gated_publish_fault(*args: object, **kwargs: object) -> None:
            fault_entered.set()
            fault_release.wait()
            original_publish_fault(*args, **kwargs)

        driver._publish_operation_fault = gated_publish_fault  # type: ignore[method-assign]
        owner_errors: list[BaseException] = []
        contender_errors: list[BaseException] = []
        owner = threading.Thread(target=lambda: self._capture(
            lambda: driver.emergency_zero(confirm=True), owner_errors,
        ))

        def contend() -> None:
            try:
                self._capture(
                    lambda: driver.restore_factory_defaults(confirm=True),
                    contender_errors,
                )
            finally:
                observed_lock.contention_event.set()

        contender = threading.Thread(target=contend)
        owner.start()
        self.assertTrue(fault_entered.wait(1.0))
        owner_generation = driver._operation_generation
        contender.start()
        self.assertTrue(observed_lock.contention_event.wait(1.0))
        generation_before_fault_publication = driver._operation_generation
        fault_release.set()
        owner.join(2.0)
        contender.join(2.0)

        self.assertFalse(owner.is_alive() or contender.is_alive())
        self.assertEqual(generation_before_fault_publication, owner_generation)
        self.assertEqual(len(owner_errors), 1)
        self.assertEqual(len(contender_errors), 1)
        self.assertIsInstance(contender_errors[0], InstrumentConnectionError)
        self.assertEqual(self._setters(device), ["msvoltage=0"])
        self.assertEqual(driver.state, DriverState.FAULT)
        driver.close()

    def test_failed_connect_close_returning_open_retains_typed_retryable_truth(self):
        from Code.Utils.common import InstrumentProtocolError
        from Code.Utils.mdt693b import MDT693B

        device = ScriptedMDTDevice(
            supported_commands=MDT693B_REQUIRED_KEYWORDS
        )
        real_close = device.close
        device.close = lambda: None  # type: ignore[method-assign]
        driver = MDT693B(
            "COM_TEST", serial_factory=RecordingSerialFactory(device)
        )
        primary = InstrumentProtocolError("snapshot failed first")

        def fail_snapshot(*args: object, **kwargs: object):
            raise primary

        driver._snapshot_from_queries = fail_snapshot  # type: ignore[method-assign]
        with self.assertRaises(InstrumentProtocolError) as caught:
            driver.connect()

        cleanup = getattr(primary, "mdt_cleanup_error", None)
        self.assertIs(caught.exception, primary)
        self.assertIsInstance(cleanup, DeviceFault)
        self.assertIs(driver.cleanup_error, cleanup)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertIsNotNone(driver._protocol)
        self.assertTrue(device.is_open)
        device.close = real_close  # type: ignore[method-assign]
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_failed_connect_close_baseexceptions_preserve_primary_and_typed_secondary(self):
        from Code.Utils.common import InstrumentProtocolError
        from Code.Utils.mdt693b import MDT693B

        for close_effect in (
            OSError("failed-connect close failed"),
            KeyboardInterrupt("failed-connect close interrupted"),
            SystemExit("failed-connect close exited"),
        ):
            with self.subTest(effect=type(close_effect).__name__):
                device = ScriptedMDTDevice(
                    supported_commands=MDT693B_REQUIRED_KEYWORDS
                )
                device.close_effects = [close_effect, None]
                driver = MDT693B(
                    "COM_TEST", serial_factory=RecordingSerialFactory(device)
                )
                primary = InstrumentProtocolError("snapshot failed first")

                def fail_snapshot(*args: object, **kwargs: object):
                    raise primary

                driver._snapshot_from_queries = fail_snapshot  # type: ignore[method-assign]
                with self.assertRaises(InstrumentProtocolError) as caught:
                    driver.connect()

                cleanup = getattr(primary, "mdt_cleanup_error", None)
                self.assertIs(caught.exception, primary)
                if isinstance(close_effect, OSError):
                    self.assertIsInstance(cleanup, InstrumentConnectionError)
                    self.assertIs(cleanup.__cause__, close_effect)
                else:
                    self.assertIs(cleanup, close_effect)
                self.assertIs(driver.cleanup_error, cleanup)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIs(driver._serial, device)
                self.assertIsNotNone(driver._protocol)
                self.assertFalse(device.closed)
                driver.close()
                self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_close_cancel_read_lookup_baseexceptions_publish_retryable_fault_truth(self):
        class CancelLookupResource:
            def __init__(
                self, resource: ScriptedMDTDevice, effect: BaseException | None
            ) -> None:
                self.resource = resource
                self.effect = effect

            def __getattr__(self, name: str):
                if name == "cancel_read" and self.effect is not None:
                    raise self.effect
                return getattr(self.resource, name)

        for effect in (
            RuntimeError("cancel_read lookup failed"),
            KeyboardInterrupt("cancel_read lookup interrupted"),
            SystemExit("cancel_read lookup exited"),
        ):
            with self.subTest(effect=type(effect).__name__):
                driver, device, _ = self._driver()
                proxy = CancelLookupResource(device, effect)
                driver._serial = proxy

                with self.assertRaises(type(effect)) as caught:
                    driver.close()

                self.assertIs(caught.exception, effect)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIs(driver.cleanup_error, effect)
                self.assertIs(driver._serial, proxy)
                self.assertFalse(device.closed)
                proxy.effect = None
                driver.close()
                self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_close_request_lock_acquire_baseexceptions_preserve_earliest_primary(self):
        class EffectLock:
            def __init__(self, effect: BaseException | None) -> None:
                self.effect = effect
                self.lock = threading.RLock()

            def acquire(self, *args: object, **kwargs: object) -> bool:
                if self.effect is not None:
                    raise self.effect
                return self.lock.acquire(*args, **kwargs)

            def release(self) -> None:
                self.lock.release()

            def __enter__(self):
                self.acquire()
                return self

            def __exit__(self, exc_type, exc_value, traceback) -> bool:
                self.release()
                return False

        for acquire_effect in (
            RuntimeError("request acquire failed"),
            KeyboardInterrupt("request acquire interrupted"),
            SystemExit("request acquire exited"),
        ):
            with self.subTest(effect=type(acquire_effect).__name__):
                driver, device, _ = self._driver()
                effect_lock = EffectLock(acquire_effect)
                driver._request_lock = effect_lock

                with self.assertRaises(type(acquire_effect)) as caught:
                    driver.close()

                self.assertIs(caught.exception, acquire_effect)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIs(driver.cleanup_error, acquire_effect)
                self.assertIs(driver._serial, device)
                self.assertFalse(device.closed)
                effect_lock.effect = None
                driver.close()
                self.assertEqual(driver.state, DriverState.DISCONNECTED)

        primary = KeyboardInterrupt("cancel_read failed first")
        secondary = RuntimeError("request acquire failed second")
        driver, device, _ = self._driver()
        device.cancel_read_effects = [primary]
        effect_lock = EffectLock(secondary)
        driver._request_lock = effect_lock

        with self.assertRaises(KeyboardInterrupt) as caught:
            driver.close()

        self.assertIs(caught.exception, primary)
        self.assertIs(getattr(primary, "mdt_cleanup_error", None), secondary)
        self.assertIs(driver.cleanup_error, primary)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        effect_lock.effect = None
        driver.close()

    def test_close_request_lock_acquisition_is_bounded_and_retryable(self):
        class RefusingLock:
            def __init__(self) -> None:
                self.timeouts: list[float] = []

            def acquire(self, *args: object, **kwargs: object) -> bool:
                timeout = kwargs.get("timeout")
                if not isinstance(timeout, (int, float)):
                    raise AssertionError("request lock acquisition was unbounded")
                self.timeouts.append(float(timeout))
                return False

            def release(self) -> None:
                raise AssertionError("unacquired request lock was released")

            def __enter__(self):
                raise AssertionError("unbounded request-lock context entered")

            def __exit__(self, exc_type, exc_value, traceback) -> bool:
                return False

        driver, device, _ = self._driver()
        original_lock = driver._request_lock
        refusing = RefusingLock()
        driver._request_lock = refusing

        with self.assertRaises(DeviceFault):
            driver.close()

        self.assertEqual(
            refusing.timeouts, [driver.io_timeout + driver.monitor_interval_s + 1.0]
        )
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertFalse(device.closed)
        driver._request_lock = original_lock
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def _assert_close_intent_blocks_all_public_work(
        self, driver: object, device: ScriptedMDTDevice
    ) -> None:
        from Code.Utils.mdt693b import Axis, RotaryMode

        calls = (
            driver.get_supported_commands,
            driver.get_product_information,
            driver.get_serial_number,
            driver.get_friendly_name,
            lambda: driver.set_friendly_name("must-not-run"),
            driver.get_echo_enabled,
            lambda: driver.set_echo_enabled(True),
            driver.get_hardware_voltage_limit,
            driver.get_display_intensity,
            lambda: driver.set_display_intensity(8),
            lambda: driver.get_axis_voltage(Axis.X),
            lambda: driver.set_axis_voltage(Axis.X, 0.9),
            driver.get_all_voltages,
            lambda: driver.set_all_voltages(0.9, confirm=True),
            driver.get_master_scan_enabled,
            lambda: driver.set_master_scan_enabled(False, confirm=True),
            driver.get_master_scan_voltage,
            lambda: driver.set_master_scan_voltage(0.0, confirm=True),
            driver.select_previous_channel,
            driver.select_next_channel,
            driver.increment_selected,
            driver.decrement_selected,
            lambda: driver.get_axis_state(Axis.X),
            lambda: driver.set_axis_minimum(Axis.X, 0.0),
            lambda: driver.set_axis_maximum(Axis.X, 75.0),
            driver.get_dac_step,
            lambda: driver.set_dac_step(50),
            driver.get_compatibility_enabled,
            lambda: driver.set_compatibility_enabled(True),
            driver.get_rotary_mode,
            lambda: driver.set_rotary_mode(RotaryMode.DEFAULT),
            driver.get_push_to_adjust_disabled,
            lambda: driver.set_push_to_adjust_disabled(False),
            lambda: driver.restore_factory_defaults(confirm=True),
            driver.recover,
            driver.ramp_to_zero,
            lambda: driver.emergency_zero(confirm=True),
        )
        generation = driver._operation_generation
        commands = list(device.commands)
        writes = list(device.writes)
        for call in calls:
            with self.subTest(public_call=call):
                with self.assertRaises(InstrumentConnectionError):
                    call()
                self.assertEqual(driver._operation_generation, generation)
                self.assertEqual(device.commands, commands)
                self.assertEqual(device.writes, writes)
                self.assertNotEqual(driver.state, DriverState.READY)

    def test_close_intent_is_revalidated_after_task10_and_arrow_lock_wait(self):
        from Code.Utils.mdt693b import Axis

        calls = (
            ("axis", lambda d: d.set_axis_voltage(Axis.X, 0.9)),
            ("all", lambda d: d.set_all_voltages(0.9, confirm=True)),
            ("master-enable", lambda d: d.set_master_scan_enabled(False, confirm=True)),
            ("master-voltage", lambda d: d.set_master_scan_voltage(0.0, confirm=True)),
            ("left", lambda d: d.select_previous_channel()),
            ("right", lambda d: d.select_next_channel()),
            ("up", lambda d: d.increment_selected()),
            ("down", lambda d: d.decrement_selected()),
        )
        for name, call in calls:
            with self.subTest(operation=name):
                driver, device, fake_time = self._driver(
                    dac_step=50,
                    master_scan_v=0.0,
                    master_scan_enabled=True,
                )
                observed_lock = ContentionObservedRLock()
                driver._request_lock = observed_lock
                self.assertTrue(observed_lock.acquire())
                errors: list[BaseException] = []
                worker = threading.Thread(
                    target=lambda: self._capture(lambda: call(driver), errors)
                )
                generation = driver._operation_generation
                commands = list(device.commands)
                writes = list(device.writes)
                worker.start()
                self.assertTrue(observed_lock.wait_for_contentions(1))
                with driver._lifecycle_condition:
                    driver._close_requested.set()
                    driver._monitor_stop.set()
                    driver._lifecycle_condition.notify_all()
                fake_time.permit(100)
                observed_lock.release()
                worker.join(2.0)
                self.assertFalse(worker.is_alive())
                self.assertEqual(len(errors), 1)
                self.assertIsInstance(errors[0], InstrumentConnectionError)
                self.assertEqual(driver._operation_generation, generation)
                self.assertEqual(device.commands, commands)
                self.assertEqual(device.writes, writes)
                driver.close()

    def test_close_intent_is_revalidated_after_connected_and_high_impact_lock_wait(self):
        calls = (
            ("getter", lambda d: d.get_supported_commands()),
            ("configuration", lambda d: d.set_friendly_name("must-not-run")),
            ("restore", lambda d: d.restore_factory_defaults(confirm=True)),
            ("recover", lambda d: d.recover()),
            ("ramp", lambda d: d.ramp_to_zero()),
            ("emergency", lambda d: d.emergency_zero(confirm=True)),
        )
        for name, call in calls:
            with self.subTest(operation=name):
                driver, device, fake_time = self._driver(
                    x=0.0, y=0.0, z=0.0,
                    master_scan_v=0.0, master_scan_enabled=False,
                )
                observed_lock = ContentionObservedRLock()
                driver._request_lock = observed_lock
                self.assertTrue(observed_lock.acquire())
                errors: list[BaseException] = []
                worker = threading.Thread(
                    target=lambda: self._capture(lambda: call(driver), errors)
                )
                commands = list(device.commands)
                writes = list(device.writes)
                worker.start()
                self.assertTrue(observed_lock.wait_for_contentions(1))
                generation = driver._operation_generation
                with driver._lifecycle_condition:
                    driver._close_requested.set()
                    driver._monitor_stop.set()
                    driver._lifecycle_condition.notify_all()
                fake_time.permit(100)
                observed_lock.release()
                worker.join(2.0)
                self.assertFalse(worker.is_alive())
                self.assertEqual(len(errors), 1)
                self.assertIsInstance(errors[0], InstrumentConnectionError)
                self.assertEqual(driver._operation_generation, generation)
                self.assertEqual(device.commands, commands)
                self.assertEqual(device.writes, writes)
                self.assertFalse(driver._restore_in_progress)
                self.assertFalse(driver._zero_in_progress)
                self.assertFalse(driver._emergency_in_progress)
                driver.close()

    def test_high_impact_outer_exit_baseexceptions_fault_and_release_for_recovery(self):
        operations = (
            ("restore", lambda d: d.restore_factory_defaults(confirm=True), ["restore"]),
            ("recover", lambda d: d.recover(), []),
            ("ramp", lambda d: d.ramp_to_zero(), ["allvoltage=0"]),
            (
                "emergency",
                lambda d: d.emergency_zero(confirm=True),
                ["msvoltage=0", "allvoltage=0", "msenable=0"],
            ),
        )
        effects = (
            RuntimeError("request exit failed"),
            KeyboardInterrupt("request exit interrupted"),
            SystemExit("request exit exited"),
        )
        for operation_name, operation, expected_setters in operations:
            for phase in ("before_release", "after_release"):
                for effect_template in effects:
                    with self.subTest(
                        operation=operation_name,
                        phase=phase,
                        effect=type(effect_template).__name__,
                    ):
                        effect = type(effect_template)(str(effect_template))
                        driver, device, _ = self._driver(
                            x=0.0, y=0.0, z=0.0,
                            master_scan_v=0.0, master_scan_enabled=False,
                        )
                        lock = ExitEffectRLock(phase, effect)
                        driver._request_lock = lock

                        with self.assertRaises(type(effect)) as caught:
                            operation(driver)

                        self.assertIs(caught.exception, effect)
                        self.assertEqual(self._setters(device), expected_setters)
                        self.assertEqual(driver.state, DriverState.FAULT)
                        self.assertFalse(driver._restore_in_progress)
                        self.assertFalse(driver._zero_in_progress)
                        self.assertFalse(driver._emergency_in_progress)
                        self.assertFalse(driver._motion_controls_confirmed)
                        self.assertIn("request lock", driver.status.fault_evidence)
                        self.assertIs(driver.cleanup_error, effect)
                        self.assertEqual(lock.depth, 0)

                        lock.effect = None
                        recovered = driver.recover()
                        self.assertIs(driver.status, recovered)
                        self.assertEqual(driver.state, DriverState.READY)
                        driver.close()

    def test_unproven_high_impact_release_is_terminal_for_this_driver_instance(self):
        effect = RuntimeError("request exit failed before release")
        release_effect = SystemExit("fallback release truth unavailable")
        driver, device, _ = self._driver(
            x=0.0, y=0.0, z=0.0,
            master_scan_v=0.0, master_scan_enabled=False,
        )
        lock = ExitEffectRLock("before_release", effect)
        lock.release_effect = release_effect
        driver._request_lock = lock

        with self.assertRaises(RuntimeError) as caught:
            driver.recover()

        self.assertIs(caught.exception, effect)
        self.assertIs(getattr(effect, "mdt_cleanup_error", None), release_effect)
        self.assertTrue(driver._request_lock_uncertain)
        self.assertEqual(driver.state, DriverState.FAULT)
        before = list(device.commands)
        generation = driver._operation_generation
        with self.assertRaises(InstrumentConnectionError):
            driver.get_supported_commands()
        self.assertEqual(device.commands, before)
        self.assertEqual(driver._operation_generation, generation)

        lock.effect = None
        lock.release_effect = None
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        replacement_factory = RecordingSerialFactory(ScriptedMDTDevice())
        driver._serial_factory = replacement_factory
        with self.assertRaises(InstrumentConnectionError):
            driver.connect()
        self.assertEqual(replacement_factory.calls, [])

    def test_failed_close_intent_is_retry_only_before_any_public_io_or_generation(self):
        class UnexpectedAdmissionLock:
            def acquire(self, *args: object, **kwargs: object) -> bool:
                raise AssertionError("terminal public work reached the request lock")

            def release(self) -> None:
                raise AssertionError("unacquired terminal request lock was released")

            def __enter__(self):
                raise AssertionError("terminal public work entered the request lock")

            def __exit__(self, exc_type, exc_value, traceback) -> bool:
                return False

        class JoinFailure:
            def is_alive(self) -> bool:
                return True

            def join(self, timeout: float) -> None:
                raise RuntimeError("join failed")

        class CancelLookupFailure:
            def __init__(self, resource: ScriptedMDTDevice) -> None:
                self.resource = resource

            def __getattr__(self, name: str):
                if name == "cancel_read":
                    raise RuntimeError("cancel lookup failed")
                return getattr(self.resource, name)

        class AcquireFailure:
            def acquire(self, *args: object, **kwargs: object) -> bool:
                raise RuntimeError("request acquire failed")

            def release(self) -> None:
                raise AssertionError("unacquired lock released")

            def __enter__(self):
                raise RuntimeError("request enter failed")

            def __exit__(self, exc_type, exc_value, traceback) -> bool:
                return False

        scenarios = ("return_open", "close_exception", "close_ki", "close_se",
                     "join", "cancel_lookup", "request_acquire")
        for scenario in scenarios:
            with self.subTest(close_failure=scenario):
                driver, device, _ = self._driver()
                real_close = device.close
                original_lock = driver._request_lock
                if scenario == "return_open":
                    device.close = lambda: None  # type: ignore[method-assign]
                elif scenario == "close_exception":
                    device.close_effects = [OSError("close failed"), None]
                elif scenario == "close_ki":
                    device.close_effects = [KeyboardInterrupt("close interrupted"), None]
                elif scenario == "close_se":
                    device.close_effects = [SystemExit("close exited"), None]
                elif scenario == "join":
                    driver._monitor_thread = JoinFailure()
                elif scenario == "cancel_lookup":
                    driver._serial = CancelLookupFailure(device)
                else:
                    driver._request_lock = AcquireFailure()

                with self.assertRaises(BaseException):
                    driver.close()
                self.assertTrue(driver._close_requested.is_set())
                self.assertTrue(driver._monitor_stop.is_set())
                self.assertEqual(driver.state, DriverState.FAULT)
                driver._request_lock = UnexpectedAdmissionLock()
                self._assert_close_intent_blocks_all_public_work(driver, device)
                driver._request_lock = original_lock

                if scenario == "return_open":
                    device.close = real_close  # type: ignore[method-assign]
                elif scenario == "join":
                    driver._monitor_thread = None
                elif scenario == "cancel_lookup":
                    driver._serial = device
                driver.close()
                self.assertEqual(driver.state, DriverState.DISCONNECTED)
                self.assertTrue(device.closed)

    def test_close_retry_then_explicit_reconnect_is_the_only_way_to_clear_intent(self):
        driver, device, fake_time = self._driver()
        real_close = device.close
        device.close = lambda: None  # type: ignore[method-assign]
        with self.assertRaises(DeviceFault):
            driver.close()
        device.close = real_close  # type: ignore[method-assign]
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertTrue(driver._close_requested.is_set())
        self.assertTrue(driver._monitor_stop.is_set())

        replacement = ScriptedMDTDevice(
            supported_commands=MDT693B_REQUIRED_KEYWORDS,
            x=0.0, y=0.0, z=0.0,
            master_scan_v=0.0, master_scan_enabled=False,
        )
        driver._serial_factory = RecordingSerialFactory(replacement)
        driver.connect()
        self.assertEqual(driver.state, DriverState.READY)
        self.assertFalse(driver._close_requested.is_set())
        self.assertFalse(driver._monitor_stop.is_set())
        self.assertTrue(fake_time.wait_for_sleeps(1))
        fake_time.permit()
        driver.close()

    def test_ready_publication_requires_live_monitor_even_without_restart_flag(self):
        driver, device, _ = self._driver(x=0.0, y=0.0, z=0.0)
        monitor = driver._monitor_thread
        monitor.alive = False
        driver._monitor_exited.set()
        driver._monitor_restart_required = False
        before = list(self._setters(device))

        with self.assertRaises(DeviceFault):
            driver.recover()

        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertEqual(self._setters(device), before)
        driver._monitor_thread = None
        driver.close()

    def test_zero_ramp_cannot_return_ready_without_a_live_monitor(self):
        driver, device, _ = self._driver(x=0.0, y=0.0, z=0.0)
        monitor = driver._monitor_thread
        monitor.alive = False
        driver._monitor_exited.set()
        driver._monitor_restart_required = False

        with self.assertRaises(DeviceFault):
            driver.ramp_to_zero()

        self.assertEqual(driver.state, DriverState.FAULT)
        driver._monitor_thread = None
        driver.close()

    def test_high_impact_request_lock_acquire_and_enter_baseexceptions_are_bounded(self):
        class OperationLock:
            def __init__(self, phase: str, effect: BaseException) -> None:
                self.phase = phase
                self.effect: BaseException | None = effect
                self.lock = threading.RLock()

            def acquire(self, *args: object, **kwargs: object) -> bool:
                if self.phase == "acquire" and self.effect is not None:
                    raise self.effect
                return self.lock.acquire(*args, **kwargs)

            def release(self) -> None:
                self.lock.release()

            def __enter__(self):
                if self.phase == "enter" and self.effect is not None:
                    raise self.effect
                self.acquire()
                return self

            def __exit__(self, exc_type, exc_value, traceback) -> bool:
                self.release()
                return False

        operations = (
            ("restore", lambda d: d.restore_factory_defaults(confirm=True)),
            ("recover", lambda d: d.recover()),
            ("ramp", lambda d: d.ramp_to_zero()),
            ("emergency", lambda d: d.emergency_zero(confirm=True)),
        )
        effects = (
            RuntimeError("request lock failed"),
            KeyboardInterrupt("request lock interrupted"),
            SystemExit("request lock exited"),
        )
        for name, operation in operations:
            for phase in ("acquire", "enter"):
                for effect in effects:
                    with self.subTest(operation=name, phase=phase,
                                      effect=type(effect).__name__):
                        driver, device, _ = self._driver(
                            x=0.0, y=0.0, z=0.0,
                            master_scan_v=0.0, master_scan_enabled=False,
                        )
                        lock = OperationLock(phase, effect)
                        driver._request_lock = lock
                        commands = list(device.commands)
                        writes = list(device.writes)

                        with self.assertRaises(type(effect)) as caught:
                            operation(driver)

                        self.assertIs(caught.exception, effect)
                        self.assertEqual(device.commands, commands)
                        self.assertEqual(device.writes, writes)
                        self.assertNotEqual(driver.state, DriverState.ACTIVE)
                        self.assertFalse(driver._restore_in_progress)
                        self.assertFalse(driver._zero_in_progress)
                        self.assertFalse(driver._emergency_in_progress)

                        lock.effect = None
                        operation(driver)
                        self.assertNotEqual(driver.state, DriverState.ACTIVE)
                        driver.close()


class MDTConnectionMonitorTests(unittest.TestCase):
    def test_constructor_rejects_unsafe_or_unbounded_configuration_before_open(self):
        from Code.Utils.mdt693b import Axis, MDT693B

        invalid = [
            (("",), {}),
            (("COM_TEST",), {"application_limits_v": {Axis.X: 75.1}}),
            (("COM_TEST",), {"application_limits_v": {Axis.X: 0.0}}),
            (("COM_TEST",), {"ramp_step_v": 0.10001}),
            (("COM_TEST",), {"ramp_interval_s": 0.049}),
            (("COM_TEST",), {"io_timeout": math.inf}),
            (("COM_TEST",), {"monitor_interval_s": 0.0}),
            (("COM_TEST",), {"monitor_failure_limit": True}),
        ]
        for args, kwargs in invalid:
            with self.subTest(args=args, kwargs=kwargs):
                with self.assertRaises((TypeError, ValueError)):
                    MDT693B(*args, **kwargs)

    def test_validated_safety_configuration_is_read_only_after_construction(self):
        from Code.Utils.mdt693b import Axis

        driver, _, _, _ = make_mdt_driver(
            application_limits_v={Axis.X: 12.0},
            ramp_step_v=0.05,
            ramp_interval_s=0.075,
        )
        self.assertEqual(
            dict(driver.application_limits_v),
            {Axis.X: 12.0, Axis.Y: 75.0, Axis.Z: 75.0},
        )
        with self.assertRaises(TypeError):
            driver.application_limits_v[Axis.X] = 20.0
        for name, value in (
            ("application_limits_v", {Axis.X: 75.0}),
            ("ramp_step_v", 0.1),
            ("ramp_interval_s", 0.050),
            ("io_timeout", 1.0),
            ("monitor_interval_s", 1.0),
            ("monitor_failure_limit", 5),
        ):
            with self.subTest(name=name):
                with self.assertRaises(AttributeError):
                    setattr(driver, name, value)

    def test_connect_uses_exact_serial_settings_and_is_query_only_through_close(self):
        driver, device, fake_time, factory = make_mdt_driver()
        driver.connect()
        self.assertEqual(device.commands[:len(EXPECTED_CONNECT_QUERIES)], EXPECTED_CONNECT_QUERIES)
        self.assertEqual(factory.calls, [{
            "port": "COM_TEST", "baudrate": 115200, "bytesize": 8,
            "parity": "N", "stopbits": 1, "xonxoff": False,
            "rtscts": False, "dsrdtr": False, "timeout": 0.5,
            "write_timeout": 0.5,
        }])
        close_gated_driver(driver, fake_time)
        self.assertFalse(any("=" in command or command == "restore" for command in device.commands))
        self.assertTrue(device.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_connect_discovers_both_echo_modes_without_changing_echo(self):
        for enabled in (False, True):
            with self.subTest(enabled=enabled):
                driver, device, fake_time, _ = make_mdt_driver(
                    ScriptedMDTDevice(echo_enabled=enabled)
                )
                driver.connect()
                self.assertEqual(driver.status.echo_enabled, enabled)
                self.assertNotIn("echo=", device.commands)
                close_gated_driver(driver, fake_time)

    def test_snapshot_parses_identity_normalizes_commands_and_is_immutable(self):
        from Code.Utils.mdt693b import Axis, RotaryMode, VoltageLimit

        advertised = [f"  {command.upper()}  " for command in EXPECTED_CONNECT_QUERIES]
        driver, _, fake_time, _ = make_mdt_driver(
            ScriptedMDTDevice(supported_commands=advertised)
        )
        driver.connect()
        status = driver.status
        self.assertEqual((status.product, status.firmware), ("MDT693B", "1.23"))
        self.assertEqual(status.supported_commands, frozenset(EXPECTED_CONNECT_QUERIES))
        self.assertEqual(status.hardware_limit, VoltageLimit.V75)
        self.assertEqual(status.rotary_mode, RotaryMode.FINE)
        self.assertEqual(status.axes[Axis.Y].actual_v, 30.0)
        with self.assertRaises(AttributeError):
            status.firmware = "changed"  # type: ignore[misc]
        close_gated_driver(driver, fake_time)

    def test_missing_xmin_is_capability_failure_without_guessed_fallback(self):
        supported = [command for command in EXPECTED_CONNECT_QUERIES if command != "xmin?"]
        driver, device, _, _ = make_mdt_driver(
            ScriptedMDTDevice(supported_commands=supported)
        )
        with self.assertRaises(InstrumentCapabilityError):
            driver.connect()
        self.assertEqual(device.commands, ["?"])
        self.assertNotIn("ymin?", device.commands)
        self.assertTrue(device.closed)

    def test_wrong_product_is_rejected_and_resource_is_closed(self):
        driver, device, _, _ = make_mdt_driver(ScriptedMDTDevice(product="MDT694B"))
        with self.assertRaises(InstrumentConnectionError):
            driver.connect()
        self.assertTrue(device.closed)
        self.assertFalse(any("=" in command for command in device.commands))

    def test_over_limit_startup_is_ready_restricted_and_never_moves(self):
        from Code.Utils.mdt693b import Axis

        driver, device, fake_time, _ = make_mdt_driver(ScriptedMDTDevice(x=80.0))
        driver.connect()
        self.assertEqual(driver.state, DriverState.READY)
        self.assertTrue(driver.status.restricted)
        self.assertIn("80", driver.status.fault_evidence)
        self.assertEqual(driver.status.axes[Axis.X].actual_v, 80.0)
        self.assertFalse(any("=" in command for command in device.commands))
        close_gated_driver(driver, fake_time)

    def test_default_monitor_is_daemon_two_hz_and_queries_only_one_atomic_triplet(self):
        driver, device, fake_time, _ = make_mdt_driver()
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        self.assertEqual(fake_time.sleep_requests[0], 0.5)
        monitor = driver._monitor_thread
        self.assertEqual(monitor.name, "MDT693BMonitor")
        self.assertTrue(monitor.daemon)
        self.assertEqual(device.commands[-3:], ["xvoltage?", "yvoltage?", "zvoltage?"])
        close_gated_driver(driver, fake_time)

    def test_monitor_triplet_holds_request_lock_against_another_request(self):
        entered = threading.Event()
        release = threading.Event()
        device = ScriptedMDTDevice()
        device.block_on_occurrence = ("xvoltage?", 2, entered, release)
        driver, device, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        self.assertTrue(entered.wait(1.0))
        requester = threading.Thread(target=lambda: driver._query("serial?"))
        requester.start()
        release.set()
        requester.join(1.0)
        self.assertFalse(requester.is_alive())
        tail = device.commands[len(EXPECTED_CONNECT_QUERIES):]
        self.assertEqual(tail[:4], ["xvoltage?", "yvoltage?", "zvoltage?", "serial?"])
        self.assertTrue(fake_time.wait_for_sleeps(1))
        close_gated_driver(driver, fake_time)

    def test_monitor_over_limit_latches_restriction_and_evidence_without_writes(self):
        driver, device, fake_time, _ = make_mdt_driver()
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        device.values["zvoltage?"] = 76.0
        fake_time.permit()
        self.assertTrue(fake_time.wait_for_sleeps(2))
        self.assertTrue(driver.status.restricted)
        self.assertIn("76", driver.status.fault_evidence)
        self.assertFalse(any("=" in command or command == "restore" for command in device.commands))
        close_gated_driver(driver, fake_time)

    def test_isolated_monitor_failure_keeps_evidence_then_success_resets_it(self):
        device = ScriptedMDTDevice()
        device.fail_on_occurrences["xvoltage?"] = {2}
        driver, _, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        self.assertEqual(driver.state, DriverState.READY)
        self.assertIn("communication failure 1/3", driver.status.fault_evidence)
        fake_time.permit()
        self.assertTrue(fake_time.wait_for_sleeps(2))
        self.assertEqual(driver.state, DriverState.READY)
        self.assertIsNone(driver.status.fault_evidence)
        close_gated_driver(driver, fake_time)

    def test_restricted_failure_then_in_range_success_restores_restriction_evidence(self):
        device = ScriptedMDTDevice(x=80.0)
        device.fail_on_occurrences["xvoltage?"] = {3}
        driver, _, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        original_evidence = driver.status.fault_evidence
        self.assertIn("80", original_evidence)
        device.values["xvoltage?"] = 70.0
        fake_time.permit()
        self.assertTrue(fake_time.wait_for_sleeps(2))
        self.assertIn("communication failure 1/3", driver.status.fault_evidence)
        fake_time.permit()
        self.assertTrue(fake_time.wait_for_sleeps(3))
        self.assertTrue(driver.status.restricted)
        self.assertEqual(driver.status.fault_evidence, original_evidence)
        self.assertNotIn("communication failure", driver.status.fault_evidence)
        close_gated_driver(driver, fake_time)

    def test_three_consecutive_monitor_failures_atomically_publish_fault(self):
        device = ScriptedMDTDevice()
        device.fail_on_occurrences["xvoltage?"] = {2, 3, 4}
        driver, _, fake_time, _ = make_mdt_driver(device)
        driver.connect()
        for sleep_count in (1, 2):
            self.assertTrue(fake_time.wait_for_sleeps(sleep_count))
            self.assertEqual(driver.state, DriverState.READY)
            fake_time.permit()
        deadline = time.monotonic() + 2.0
        while driver.state is not DriverState.FAULT and time.monotonic() < deadline:
            time.sleep(0.001)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIn("communication failure 3/3", driver.status.fault_evidence)
        fake_time.permit()
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_close_stops_and_joins_monitor_before_closing_serial(self):
        driver, device, fake_time, _ = make_mdt_driver()
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        close_gated_driver(driver, fake_time)
        self.assertFalse(driver._monitor_thread and driver._monitor_thread.is_alive())
        self.assertFalse(any(
            thread.name == "MDT693BMonitor" and thread.is_alive()
            for thread in threading.enumerate()
        ))

    def test_close_fixture_waits_for_stop_intent_before_permitting_monitor(self):
        driver, device, fake_time, _ = make_mdt_driver(
            io_timeout=0.05,
            monitor_interval_s=0.05,
        )
        original_close = driver.close
        close_entered = threading.Event()
        allow_close = threading.Event()
        errors: list[BaseException] = []

        def delayed_close() -> None:
            close_entered.set()
            if not allow_close.wait(2.0):
                raise AssertionError("fixture close gate was not released")
            original_close()

        worker = threading.Thread(
            target=lambda: self._capture_error(
                lambda: close_gated_driver(driver, fake_time), errors
            ),
            name="MDTCloseFixtureRegression",
        )
        worker_started = False
        try:
            driver.connect()
            self.assertTrue(fake_time.wait_for_sleeps(1))
            driver.close = delayed_close  # type: ignore[method-assign]
            worker.start()
            worker_started = True
            self.assertTrue(close_entered.wait(1.0))
            self.assertFalse(
                fake_time.wait_for_sleeps(2, timeout=1.0),
                "fixture permit was consumed before close published stop intent",
            )
            allow_close.set()
            worker.join(2.0)
            self.assertFalse(worker.is_alive())
            self.assertEqual(errors, [])
            self.assertEqual(driver.state, DriverState.DISCONNECTED)
            self.assertTrue(device.closed)
            self.assertFalse(device.is_open)
            self.assertFalse(any(
                thread.name in {
                    "MDT693BMonitor",
                    "MDTTestClose",
                    "MDTCloseFixtureRegression",
                } and thread.is_alive()
                for thread in threading.enumerate()
            ))
        finally:
            allow_close.set()
            if worker_started:
                worker.join(2.0)
            driver.close = original_close  # type: ignore[method-assign]
            if driver._serial is None:
                original_close()
            else:
                close_gated_driver(driver, fake_time)

    def test_close_fixture_preserves_close_error_identity(self):
        close_error = KeyboardInterrupt("fixture-visible close failure")
        device = ScriptedMDTDevice()
        device.close_effects = [close_error, None]
        driver, device, fake_time, _ = make_mdt_driver(device)
        try:
            driver.connect()
            self.assertTrue(fake_time.wait_for_sleeps(1))
            with self.assertRaises(KeyboardInterrupt) as caught:
                close_gated_driver(driver, fake_time)
            self.assertIs(caught.exception, close_error)
        finally:
            device.close_effects.clear()
            if driver._serial is None:
                driver.close()
            else:
                close_gated_driver(driver, fake_time)

        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertTrue(device.closed)
        self.assertFalse(any(
            thread.name in {"MDT693BMonitor", "MDTTestClose"} and thread.is_alive()
            for thread in threading.enumerate()
        ))

    def test_close_publishes_intent_and_cancels_a_blocked_connect_read(self):
        from Code.Utils.mdt693b import MDT693B

        entered = threading.Event()
        release = threading.Event()
        device = ScriptedMDTDevice()
        device.gate_read_on_occurrence = ("?", 1, entered, release)
        cancel_entered = threading.Event()
        cancel_release = threading.Event()
        device.cancel_read_gate = (cancel_entered, cancel_release)
        factory = RecordingSerialFactory(device)
        driver = MDT693B(
            "COM_TEST",
            io_timeout=0.05,
            monitor_interval_s=0.05,
            serial_factory=factory,
        )
        connect_errors: list[BaseException] = []
        close_errors: list[BaseException] = []
        connector = threading.Thread(
            target=lambda: self._capture_error(driver.connect, connect_errors)
        )
        connector.start()
        self.assertTrue(entered.wait(1.0))
        closer = threading.Thread(target=lambda: self._capture_error(driver.close, close_errors))
        closer.start()
        cancellation_started = cancel_entered.wait(0.2)
        closing_was_published = cancellation_started and driver.state is DriverState.CLOSING
        if cancellation_started:
            cancel_release.set()
        else:
            release.set()
        connector.join(2.0)
        closer.join(2.0)
        release.set()
        self.assertTrue(closing_was_published)
        self.assertFalse(connector.is_alive())
        self.assertFalse(closer.is_alive())
        self.assertEqual(close_errors, [])
        self.assertEqual(len(connect_errors), 1)
        self.assertEqual(device.cancel_read_calls, 1)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertTrue(device.closed)
        self.assertIsNone(driver._monitor_thread)
        self.assertFalse(any("=" in command or command == "restore" for command in device.commands))

    def test_blocked_monitor_join_faults_and_retains_live_resource_until_retry(self):
        from Code.Utils.common import DeviceFault

        driver, device, fake_time, _ = make_mdt_driver(
            io_timeout=0.01,
            monitor_interval_s=0.01,
        )
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        with self.assertRaises(DeviceFault) as caught:
            driver.close()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertFalse(device.closed)
        self.assertIs(driver.cleanup_error, caught.exception)
        self.assertIn("monitor did not terminate", driver.status.fault_evidence)
        self.assertTrue(driver._monitor_thread.is_alive())
        fake_time.permit()
        driver._monitor_thread.join(1.0)
        driver.close()
        self.assertTrue(device.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_uncancellable_blocked_connect_faults_without_false_disconnect_or_close(self):
        from Code.Utils.common import DeviceFault
        from Code.Utils.mdt693b import MDT693B

        entered = threading.Event()
        release = threading.Event()
        device = ScriptedMDTDevice()
        device.gate_read_on_occurrence = ("?", 1, entered, release)
        device.cancel_read = None  # type: ignore[method-assign]
        driver = MDT693B(
            "COM_TEST",
            io_timeout=0.01,
            monitor_interval_s=0.01,
            serial_factory=RecordingSerialFactory(device),
        )
        connect_errors: list[BaseException] = []
        connector = threading.Thread(
            target=lambda: self._capture_error(driver.connect, connect_errors)
        )
        connector.start()
        self.assertTrue(entered.wait(1.0))
        with self.assertRaises(DeviceFault) as caught:
            driver.close()
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertFalse(device.closed)
        self.assertIs(driver.cleanup_error, caught.exception)
        self.assertTrue(connector.is_alive())
        release.set()
        connector.join(2.0)
        self.assertFalse(connector.is_alive())
        self.assertEqual(len(connect_errors), 1)
        self.assertTrue(device.closed)
        self.assertEqual(driver.state, DriverState.DISCONNECTED)
        self.assertIsNone(driver._monitor_thread)

    def test_cancel_base_exception_stays_primary_when_serial_close_also_fails(self):
        from Code.Utils.common import InstrumentConnectionError

        cancel_error = KeyboardInterrupt("cancel interrupted")
        driver, device, fake_time, _ = make_mdt_driver()
        device.cancel_read_effects = [cancel_error]
        device.close_effects = [OSError("close failed"), None]
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        errors: list[BaseException] = []
        closer = threading.Thread(target=lambda: self._capture_error(driver.close, errors))
        closer.start()
        fake_time.permit()
        closer.join(2.0)
        self.assertFalse(closer.is_alive())
        self.assertEqual(errors, [cancel_error])
        secondary = getattr(cancel_error, "mdt_cleanup_error", None)
        self.assertIsInstance(secondary, InstrumentConnectionError)
        self.assertIs(driver.cleanup_error, cancel_error)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        self.assertFalse(device.closed)
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_cancel_read_ordinary_exceptions_are_typed_with_original_cause(self):
        from Code.Utils.common import InstrumentConnectionError

        for cancel_error in (
            OSError("cancel failed"),
            serial.SerialException("serial cancel failed"),
        ):
            with self.subTest(cancel_error=type(cancel_error).__name__):
                driver, device, fake_time, _ = make_mdt_driver()
                device.cancel_read_effects = [cancel_error]
                driver.connect()
                self.assertTrue(fake_time.wait_for_sleeps(1))
                errors: list[BaseException] = []
                closer = threading.Thread(
                    target=lambda: self._capture_error(driver.close, errors)
                )
                closer.start()
                fake_time.permit()
                closer.join(2.0)
                self.assertFalse(closer.is_alive())
                self.assertEqual(len(errors), 1)
                self.assertIsInstance(errors[0], InstrumentConnectionError)
                self.assertIs(errors[0].__cause__, cancel_error)
                self.assertIs(driver.cleanup_error, errors[0])
                self.assertIn("InstrumentConnectionError", driver.status.fault_evidence)
                self.assertEqual(driver.state, DriverState.DISCONNECTED)
                self.assertIsNone(driver._serial)
                self.assertTrue(device.closed)

    def test_typed_cancel_error_keeps_serial_close_failure_as_secondary_evidence(self):
        from Code.Utils.common import InstrumentConnectionError

        cancel_error = OSError("cancel failed first")
        close_error = OSError("close failed second")
        driver, device, fake_time, _ = make_mdt_driver()
        device.cancel_read_effects = [cancel_error]
        device.close_effects = [close_error, None]
        driver.connect()
        self.assertTrue(fake_time.wait_for_sleeps(1))
        errors: list[BaseException] = []
        closer = threading.Thread(target=lambda: self._capture_error(driver.close, errors))
        closer.start()
        fake_time.permit()
        closer.join(2.0)
        self.assertEqual(len(errors), 1)
        primary = errors[0]
        self.assertIsInstance(primary, InstrumentConnectionError)
        self.assertIs(primary.__cause__, cancel_error)
        secondary = getattr(primary, "mdt_cleanup_error", None)
        self.assertIsInstance(secondary, InstrumentConnectionError)
        self.assertIs(secondary.__cause__, close_error)
        self.assertIs(driver.cleanup_error, primary)
        self.assertEqual(driver.state, DriverState.FAULT)
        self.assertIs(driver._serial, device)
        driver.close()
        self.assertEqual(driver.state, DriverState.DISCONNECTED)

    def test_serial_close_exception_and_base_exception_retain_truthful_fault_state(self):
        from Code.Utils.common import InstrumentConnectionError

        for close_error, expected_type in (
            (OSError("close failed"), InstrumentConnectionError),
            (KeyboardInterrupt("close interrupted"), KeyboardInterrupt),
        ):
            with self.subTest(close_error=type(close_error).__name__):
                driver, device, fake_time, _ = make_mdt_driver()
                device.close_effects = [close_error, None]
                driver.connect()
                self.assertTrue(fake_time.wait_for_sleeps(1))
                errors: list[BaseException] = []
                closer = threading.Thread(
                    target=lambda: self._capture_error(driver.close, errors)
                )
                closer.start()
                fake_time.permit()
                closer.join(2.0)
                self.assertFalse(closer.is_alive())
                self.assertEqual(len(errors), 1)
                self.assertIsInstance(errors[0], expected_type)
                if isinstance(close_error, KeyboardInterrupt):
                    self.assertIs(errors[0], close_error)
                self.assertEqual(driver.state, DriverState.FAULT)
                self.assertIs(driver._serial, device)
                self.assertFalse(device.closed)
                self.assertIs(driver.cleanup_error, errors[0])
                self.assertIn("cleanup failure", driver.status.fault_evidence)
                driver.close()
                self.assertTrue(device.closed)
                self.assertEqual(driver.state, DriverState.DISCONNECTED)

    @staticmethod
    def _capture_error(callable_: object, errors: list[BaseException]) -> None:
        try:
            callable_()  # type: ignore[operator]
        except BaseException as error:
            errors.append(error)


def _diagnostic_status(
    *, x=10.0, xmin=0.0, xmax=75.0,
    master_scan_enabled=False, master_scan_voltage_v=0.0,
    axis_command_known=True,
):
    return MDTStatus(
        product="MDT693B",
        firmware="1.09",
        serial_number="MDT1",
        friendly_name="piezo",
        echo_enabled=False,
        hardware_limit=VoltageLimit.V75,
        display_intensity=8,
        master_scan_enabled=master_scan_enabled,
        master_scan_voltage_v=master_scan_voltage_v,
        axes={
            Axis.X: AxisState(x, xmin, xmax),
            Axis.Y: AxisState(20.0, 0.0, 75.0),
            Axis.Z: AxisState(30.0, 0.0, 75.0),
        },
        dac_step=10,
        compatibility_enabled=False,
        rotary_mode=RotaryMode.DEFAULT,
        push_to_adjust_disabled=True,
        supported_commands=frozenset({"?", "xmin?", "xvoltage?"}),
        restricted=False,
        fault_evidence=None,
        observed_at=123.0,
        axis_command_known=axis_command_known,
    )


class _DiagnosticMDT:
    def __init__(self, status=None):
        self.status = status or _diagnostic_status()
        self.application_limits_v = {axis: 75.0 for axis in Axis}
        self.connect_calls = 0
        self.close_calls = 0
        self.set_calls = []
        self.all_voltage_queries = 0
        self.all_voltage_outcomes = []
        self.axis_state_error = None
        self.axis_state_values = []
        self.axis_state_queries = 0
        self.all_voltage_error = None
        self.master_enabled_values = []
        self.master_voltage_values = []
        self.master_enabled_queries = 0
        self.master_voltage_queries = 0
        self.set_outcomes = []
        self.close_error = None

    def connect(self):
        self.connect_calls += 1
        return self

    def close(self):
        self.close_calls += 1
        if self.close_error is not None:
            raise self.close_error

    def get_axis_state(self, axis):
        self.axis_state_queries += 1
        if self.axis_state_error is not None:
            raise self.axis_state_error
        if self.axis_state_values:
            value = self.axis_state_values.pop(0)
            if isinstance(value, BaseException):
                raise value
            return value
        return self.status.axes[axis]

    def get_master_scan_enabled(self):
        self.master_enabled_queries += 1
        if self.master_enabled_values:
            value = self.master_enabled_values.pop(0)
            if isinstance(value, BaseException):
                raise value
            return value
        return self.status.master_scan_enabled

    def get_master_scan_voltage(self):
        self.master_voltage_queries += 1
        if self.master_voltage_values:
            value = self.master_voltage_values.pop(0)
            if isinstance(value, BaseException):
                raise value
            return value
        return self.status.master_scan_voltage_v

    def set_axis_voltage(self, axis, target):
        self.set_calls.append((axis, target))
        if self.set_outcomes:
            outcome = self.set_outcomes.pop(0)
            if isinstance(outcome, BaseException):
                raise outcome
        return self.status

    def get_all_voltages(self):
        self.all_voltage_queries += 1
        if self.all_voltage_outcomes:
            outcome = self.all_voltage_outcomes.pop(0)
            if isinstance(outcome, BaseException):
                raise outcome
            return outcome
        if self.all_voltage_error is not None:
            raise self.all_voltage_error
        return {axis: state.actual_v for axis, state in self.status.axes.items()}


class MDTDiagnosticTests(unittest.TestCase):
    def run_main(self, argv, *, fake=None, prompt=None):
        fake = fake or _DiagnosticMDT()
        output = io.StringIO()
        patches = [
            patch.object(sys, "argv", argv),
            patch.object(check_mdt, "MDT693B", return_value=fake),
        ]
        if prompt is not None:
            patches.append(patch("builtins.input", side_effect=prompt))
        with patches[0], patches[1], redirect_stdout(output):
            if len(patches) == 3:
                with patches[2]:
                    result = check_mdt.main()
            else:
                result = check_mdt.main()
        return result, fake, output.getvalue()

    def test_no_port_lists_metadata_only_without_driver_construction(self):
        ports = [
            SimpleNamespace(
                device="COM9", vid=0x10C4, pid=0xEA60,
                serial_number="ABC", description="CP210x",
            )
        ]
        output = io.StringIO()
        with patch.object(sys, "argv", ["check_mdt"]), \
             patch.object(check_mdt.list_ports, "comports", return_value=ports), \
             patch.object(check_mdt, "MDT693B") as constructor, \
             redirect_stdout(output):
            self.assertEqual(check_mdt.main(), 0)
        constructor.assert_not_called()
        for text in ("COM9", "10C4", "EA60", "ABC", "CP210x"):
            self.assertIn(text, output.getvalue())

    def test_read_only_port_path_prints_complete_status_and_closes_without_motion(self):
        result, fake, output = self.run_main(["check_mdt", "--port", "COM9"])
        self.assertEqual(result, 0)
        self.assertEqual(fake.connect_calls, 1)
        self.assertEqual(fake.set_calls, [])
        self.assertEqual(fake.close_calls, 1)
        for text in (
            "MDT693B", "1.09", "MDT1", "piezo", "75", "xmin?",
            "Master Scan", "X", "Y", "Z", "DAC", "compatibility",
            "rotary", "push", "restricted", "fault", "123.0",
        ):
            self.assertIn(text.lower(), output.lower())

    def test_unknown_axis_baseline_prints_authority_without_prompt_target_or_write(self):
        status = _diagnostic_status(axis_command_known=False)
        fake = _DiagnosticMDT(status)
        output = io.StringIO()
        with patch.object(
            sys, "argv", ["check_mdt", "--port", "COM9", "--step-axis", "X"]
        ), patch.object(check_mdt, "MDT693B", return_value=fake), patch(
            "builtins.input"
        ) as prompt, redirect_stdout(output):
            self.assertEqual(check_mdt.main(), 0)

        prompt.assert_not_called()
        self.assertEqual(fake.set_calls, [])
        self.assertEqual(fake.master_enabled_queries, 0)
        self.assertEqual(fake.master_voltage_queries, 0)
        self.assertNotIn("Proposed reversible move", output.getvalue())
        self.assertNotIn("target=", output.getvalue())
        self.assertIn("axis_command_known=False", output.getvalue())
        self.assertEqual(fake.close_calls, 1)

    def test_selected_axis_change_during_confirmation_aborts_before_setter(self):
        status = _diagnostic_status(x=10.0, axis_command_known=True)
        fake = _DiagnosticMDT(status)
        fake.axis_state_values = [
            AxisState(10.0, 0.0, 75.0),
            AxisState(10.05, 0.0, 75.0),
        ]

        with self.assertRaises(InstrumentSafetyError):
            self.run_main(
                ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                fake=fake,
                prompt=["MOVE"],
            )

        self.assertEqual(fake.axis_state_queries, 2)
        self.assertEqual(fake.set_calls, [])
        self.assertEqual(fake.close_calls, 1)

    def test_selected_port_is_routed_exactly_to_constructor(self):
        fake = _DiagnosticMDT()
        with patch.object(sys, "argv", ["check_mdt", "--port", "COM19"]), \
             patch.object(check_mdt, "MDT693B", return_value=fake) as constructor, \
             redirect_stdout(io.StringIO()):
            self.assertEqual(check_mdt.main(), 0)
        constructor.assert_called_once_with("COM19")

    def test_step_requires_port_and_valid_magnitude_before_driver_construction(self):
        cases = (
            ["check_mdt", "--step-axis", "X"],
            ["check_mdt", "--port", "COM9", "--step-axis", "X", "--step-volts", "0"],
            ["check_mdt", "--port", "COM9", "--step-axis", "X", "--step-volts", "-1"],
            ["check_mdt", "--port", "COM9", "--step-axis", "X", "--step-volts", "0.1001"],
            ["check_mdt", "--port", "COM9", "--step-axis", "X", "--step-volts", "nan"],
            ["check_mdt", "--port", "COM9", "--step-axis", "X", "--step-volts", "inf"],
            ["check_mdt", "--port", "COM9", "--step-axis", "X", "--step-volts", "True"],
        )
        for argv in cases:
            with self.subTest(argv=argv), patch.object(sys, "argv", argv), \
                 patch.object(check_mdt, "MDT693B") as constructor, \
                 redirect_stderr(io.StringIO()), \
                 self.assertRaises(SystemExit):
                check_mdt.main()
            constructor.assert_not_called()

    def test_decline_blank_eof_interrupt_and_system_exit_never_move(self):
        for prompt_result in ("n", "", EOFError(), KeyboardInterrupt(), SystemExit(4)):
            with self.subTest(prompt=repr(prompt_result)):
                fake = _DiagnosticMDT()
                result, _, _ = self.run_main(
                    ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                    fake=fake,
                    prompt=[prompt_result],
                )
                self.assertEqual(result, 0)
                self.assertEqual(fake.set_calls, [])
                self.assertEqual(fake.master_enabled_queries, 1)
                self.assertEqual(fake.master_voltage_queries, 1)
                self.assertEqual(fake.close_calls, 1)

    def test_master_scan_contribution_blocks_disclosure_prompt_and_every_setter(self):
        for enabled, voltage in (
            (True, 0.0), (False, 1.000001e-6), (False, 5.0), (True, 5.0)
        ):
            with self.subTest(enabled=enabled, voltage=voltage):
                fake = _DiagnosticMDT(_diagnostic_status(x=20.0))
                fake.master_enabled_values = [enabled]
                fake.master_voltage_values = [voltage]
                with self.assertRaises(InstrumentSafetyError):
                    self.run_main(
                        ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                        fake=fake,
                        prompt=[AssertionError("unsafe state reached MOVE prompt")],
                    )
                self.assertEqual(fake.master_enabled_queries, 1)
                self.assertEqual(fake.master_voltage_queries, 1)
                self.assertEqual(fake.set_calls, [])
                self.assertEqual(fake.close_calls, 1)

    def test_master_scan_is_rechecked_after_move_confirmation_before_first_setter(self):
        fake = _DiagnosticMDT(_diagnostic_status(x=20.0))
        fake.master_enabled_values = [False, True]
        fake.master_voltage_values = [0.0, 0.0]
        with self.assertRaises(InstrumentSafetyError):
            self.run_main(
                ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                fake=fake,
                prompt=["MOVE"],
            )
        self.assertEqual(fake.master_enabled_queries, 2)
        self.assertEqual(fake.master_voltage_queries, 2)
        self.assertEqual(fake.set_calls, [])
        self.assertEqual(fake.close_calls, 1)

    def test_master_scan_zero_tolerance_boundary_remains_reversible(self):
        fake = _DiagnosticMDT(_diagnostic_status(x=20.0))
        fake.master_enabled_values = [False, False]
        fake.master_voltage_values = [1e-6, -1e-6]
        result, _, _ = self.run_main(
            ["check_mdt", "--port", "COM9", "--step-axis", "X"],
            fake=fake,
            prompt=["MOVE"],
        )
        self.assertEqual(result, 0)
        self.assertEqual(fake.set_calls, [(Axis.X, 20.1), (Axis.X, 20.0)])
        self.assertEqual(fake.master_enabled_queries, 2)
        self.assertEqual(fake.master_voltage_queries, 2)

    def test_approved_step_discloses_target_queries_all_axes_and_restores_exact_old(self):
        fake = _DiagnosticMDT(_diagnostic_status(x=10.0))
        result, _, output = self.run_main(
            ["check_mdt", "--port", "COM9", "--step-axis", "X", "--step-volts", "0.1"],
            fake=fake,
            prompt=["MOVE"],
        )
        self.assertEqual(result, 0)
        self.assertEqual(fake.set_calls, [(Axis.X, 10.1), (Axis.X, 10.0)])
        self.assertEqual(fake.master_enabled_queries, 2)
        self.assertEqual(fake.master_voltage_queries, 2)
        self.assertEqual(fake.all_voltage_queries, 2)
        self.assertIn("old=10 V", output)
        self.assertIn("target=10.1 V", output)
        self.assertIn("restoration=10 V", output)
        self.assertIn("Observed after restoration", output)
        self.assertEqual(fake.close_calls, 1)

    def test_step_uses_safe_decreasing_direction_at_upper_boundary(self):
        fake = _DiagnosticMDT(_diagnostic_status(x=75.0))
        result, _, _ = self.run_main(
            ["check_mdt", "--port", "COM9", "--step-axis", "X", "--step-volts", "0.1"],
            fake=fake,
            prompt=["MOVE"],
        )
        self.assertEqual(result, 0)
        self.assertEqual(fake.set_calls, [(Axis.X, 74.9), (Axis.X, 75.0)])
        self.assertEqual(fake.all_voltage_queries, 2)

    def test_preflight_failure_performs_zero_motion_and_closes(self):
        fake = _DiagnosticMDT(_diagnostic_status(x=10.0, xmin=10.0, xmax=10.0))
        with self.assertRaises(ValueError):
            self.run_main(
                ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                fake=fake,
                prompt=["MOVE"],
            )
        self.assertEqual(fake.set_calls, [])
        self.assertEqual(fake.close_calls, 1)

    def test_first_motion_primary_survives_restore_and_close_failures(self):
        fake = _DiagnosticMDT()
        primary = RuntimeError("move")
        restore = RuntimeError("restore")
        close = RuntimeError("close")
        fake.set_outcomes = [primary, restore]
        fake.close_error = close
        with self.assertRaises(RuntimeError) as caught:
            self.run_main(
                ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                fake=fake,
                prompt=["MOVE"],
            )
        self.assertIs(caught.exception, primary)
        self.assertIs(caught.exception.mdt_diagnostic_restore_error, restore)
        self.assertIs(caught.exception.mdt_diagnostic_close_error, close)
        self.assertEqual(fake.set_calls, [(Axis.X, 10.1), (Axis.X, 10.0)])
        self.assertEqual(fake.all_voltage_queries, 0)

    def test_failure_after_move_still_restores_and_preserves_primary(self):
        fake = _DiagnosticMDT()
        primary = RuntimeError("all axes")
        fake.all_voltage_error = primary
        with self.assertRaises(RuntimeError) as caught:
            self.run_main(
                ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                fake=fake,
                prompt=["MOVE"],
            )
        self.assertIs(caught.exception, primary)
        self.assertEqual(fake.set_calls, [(Axis.X, 10.1), (Axis.X, 10.0)])
        self.assertEqual(fake.all_voltage_queries, 2)
        self.assertEqual(fake.close_calls, 1)

    def test_post_restoration_query_failure_is_primary_and_prints_no_false_success(self):
        fake = _DiagnosticMDT()
        post_restore = RuntimeError("post-restoration query")
        fake.all_voltage_outcomes = [
            {axis: state.actual_v for axis, state in fake.status.axes.items()},
            post_restore,
        ]
        output = io.StringIO()
        with patch.object(sys, "argv", [
                 "check_mdt", "--port", "COM9", "--step-axis", "X"
             ]), patch.object(check_mdt, "MDT693B", return_value=fake), \
             patch("builtins.input", return_value="MOVE"), redirect_stdout(output), \
             self.assertRaises(RuntimeError) as caught:
            check_mdt.main()
        self.assertIs(caught.exception, post_restore)
        self.assertEqual(fake.set_calls, [(Axis.X, 10.1), (Axis.X, 10.0)])
        self.assertEqual(fake.all_voltage_queries, 2)
        self.assertNotIn("Restored X", output.getvalue())
        self.assertEqual(fake.close_calls, 1)

    def test_restoration_readback_under_and_over_mismatch_fault_without_false_success(self):
        for observed, observed_evidence in (
            (9.9999989, "observed=9.9999989 V"),
            (10.0000011, "observed=10.0000011 V"),
        ):
            with self.subTest(observed=observed):
                fake = _DiagnosticMDT()
                after_move = {
                    axis: state.actual_v for axis, state in fake.status.axes.items()
                }
                after_restore = dict(after_move)
                after_restore[Axis.X] = observed
                fake.all_voltage_outcomes = [after_move, after_restore]
                output = io.StringIO()
                with patch.object(sys, "argv", [
                         "check_mdt", "--port", "COM9", "--step-axis", "X"
                     ]), patch.object(check_mdt, "MDT693B", return_value=fake), \
                     patch("builtins.input", return_value="MOVE"), \
                     redirect_stdout(output), self.assertRaises(DeviceFault) as caught:
                    check_mdt.main()
                evidence = str(caught.exception)
                self.assertIn("X", evidence)
                self.assertIn("old=10.0 V", evidence)
                self.assertIn(observed_evidence, evidence)
                self.assertIn("abs_tolerance=1e-06 V", evidence)
                self.assertNotIn("Restored X", output.getvalue())
                self.assertEqual(fake.close_calls, 1)

    def test_restoration_readback_exact_tolerance_boundary_is_confirmed(self):
        for observed in (9.999999, 10.000001):
            with self.subTest(observed=observed):
                fake = _DiagnosticMDT()
                after_move = {
                    axis: state.actual_v for axis, state in fake.status.axes.items()
                }
                after_restore = dict(after_move)
                after_restore[Axis.X] = observed
                fake.all_voltage_outcomes = [after_move, after_restore]
                result, _, output = self.run_main(
                    ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                    fake=fake,
                    prompt=["MOVE"],
                )
                self.assertEqual(result, 0)
                self.assertIn("Observed after restoration", output)
                self.assertIn("Restored X", output)

    def test_original_primary_survives_restoration_mismatch_as_secondary(self):
        fake = _DiagnosticMDT()
        primary = RuntimeError("move observation")
        after_restore = {
            axis: state.actual_v for axis, state in fake.status.axes.items()
        }
        after_restore[Axis.X] = 10.01
        fake.all_voltage_outcomes = [primary, after_restore]
        with self.assertRaises(RuntimeError) as caught:
            self.run_main(
                ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                fake=fake,
                prompt=["MOVE"],
            )
        self.assertIs(caught.exception, primary)
        secondary = caught.exception.mdt_diagnostic_restore_error
        self.assertIsInstance(secondary, DeviceFault)
        self.assertIn("old=10.0 V", str(secondary))
        self.assertIn("observed=10.01 V", str(secondary))
        self.assertIn("abs_tolerance=1e-06 V", str(secondary))

    def test_restoration_mismatch_stays_primary_when_close_also_fails(self):
        fake = _DiagnosticMDT()
        close = RuntimeError("close")
        fake.close_error = close
        after_move = {
            axis: state.actual_v for axis, state in fake.status.axes.items()
        }
        after_restore = dict(after_move)
        after_restore[Axis.X] = 10.01
        fake.all_voltage_outcomes = [after_move, after_restore]
        with self.assertRaises(DeviceFault) as caught:
            self.run_main(
                ["check_mdt", "--port", "COM9", "--step-axis", "X"],
                fake=fake,
                prompt=["MOVE"],
            )
        self.assertIn("old=10.0 V", str(caught.exception))
        self.assertIn("observed=10.01 V", str(caught.exception))
        self.assertIn("abs_tolerance=1e-06 V", str(caught.exception))
        self.assertIs(caught.exception.mdt_diagnostic_close_error, close)

    def test_help_performs_no_port_discovery_or_driver_construction(self):
        with patch.object(sys, "argv", ["check_mdt", "--help"]), \
             patch.object(check_mdt.list_ports, "comports") as comports, \
             patch.object(check_mdt, "MDT693B") as constructor, \
             redirect_stdout(io.StringIO()), \
             self.assertRaises(SystemExit) as caught:
            check_mdt.main()
        self.assertEqual(caught.exception.code, 0)
        comports.assert_not_called()
        constructor.assert_not_called()


if __name__ == "__main__":
    unittest.main()
