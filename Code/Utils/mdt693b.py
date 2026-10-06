"""Pure MDT693B data types and its prompt-terminated serial framing.

This module deliberately contains no port discovery or public raw-command API.
Connection and typed control surfaces are layered on top of ``_MDTProtocol``.
"""

from __future__ import annotations

import math
import re
import threading
import time
from contextlib import contextmanager
from collections.abc import Callable
from dataclasses import dataclass, replace
from enum import Enum
from numbers import Integral, Real
from types import MappingProxyType
from typing import Iterator, Mapping

import serial

from Code.Utils.common import (
    DeviceFault,
    DriverError,
    DriverState,
    InstrumentCapabilityError,
    InstrumentConnectionError,
    InstrumentProtocolError,
    InstrumentSafetyError,
    InstrumentTimeoutError,
    get_logger,
    ProbeReport, _identity_probe, _probe_connect_guard,
)


class Axis(Enum):
    X = "x"
    Y = "y"
    Z = "z"


class VoltageLimit(Enum):
    V75 = (0, 75.0)
    V100 = (1, 100.0)
    V150 = (2, 150.0)

    @property
    def code(self) -> int:
        return self.value[0]

    @property
    def volts(self) -> float:
        return self.value[1]


class RotaryMode(Enum):
    DEFAULT = 0
    TEN_TURN = 1
    FINE = 2


class _MDTOperationCancelled(InstrumentConnectionError):
    """A lifecycle transition won after a Task 9 request entered."""


class _MDTArrowRejected(InstrumentCapabilityError):
    """The firmware explicitly rejected one fixed arrow candidate."""


_ARROW_UP = b"\x1b[A"
_ARROW_DOWN = b"\x1b[B"
_ARROW_RIGHT = b"\x1b[C"
_ARROW_LEFT = b"\x1b[D"
_ARROW_CANDIDATES = frozenset({
    _ARROW_UP, _ARROW_DOWN, _ARROW_RIGHT, _ARROW_LEFT,
})

AXIS_BASELINE_ATTESTATION_EVIDENCE = (
    "MDT axis-command baseline adopted by explicit operator attestation; "
    "software cannot verify absence of external/manual contributions"
)


def _finite_float(value: object, context: str) -> float:
    if isinstance(value, bool):
        raise InstrumentProtocolError(f"{context} must be a finite number")
    try:
        result = float(value)
    except (TypeError, ValueError) as exc:
        raise InstrumentProtocolError(f"{context} must be a finite number") from exc
    if not math.isfinite(result):
        raise InstrumentProtocolError(f"{context} must be a finite number")
    return result


def _integer(value: object, context: str) -> int:
    if isinstance(value, bool):
        raise InstrumentProtocolError(f"{context} must be an integer")
    if isinstance(value, str):
        if re.fullmatch(r"[+-]?\d+", value) is None:
            raise InstrumentProtocolError(f"{context} must be an integer")
        return int(value)
    if not isinstance(value, Integral):
        raise InstrumentProtocolError(f"{context} must be an integer")
    return int(value)


def _boolean(value: object, context: str) -> bool:
    if not isinstance(value, bool):
        raise InstrumentProtocolError(f"{context} must be a boolean")
    return value


@dataclass(frozen=True)
class AxisState:
    actual_v: float
    minimum_v: float
    maximum_v: float

    def __post_init__(self) -> None:
        object.__setattr__(self, "actual_v", _finite_float(self.actual_v, "axis actual voltage"))
        object.__setattr__(self, "minimum_v", _finite_float(self.minimum_v, "axis minimum voltage"))
        object.__setattr__(self, "maximum_v", _finite_float(self.maximum_v, "axis maximum voltage"))


@dataclass(frozen=True)
class MDTStatus:
    product: str
    firmware: str
    serial_number: str
    friendly_name: str
    echo_enabled: bool
    hardware_limit: VoltageLimit
    display_intensity: int
    master_scan_enabled: bool
    master_scan_voltage_v: float
    axes: Mapping[Axis, AxisState]
    dac_step: int
    compatibility_enabled: bool
    rotary_mode: RotaryMode
    push_to_adjust_disabled: bool
    supported_commands: frozenset[str]
    restricted: bool
    fault_evidence: str | None
    observed_at: float
    axis_command_known: bool = False

    def __post_init__(self) -> None:
        for field in ("product", "firmware", "serial_number", "friendly_name"):
            if not isinstance(getattr(self, field), str):
                raise InstrumentProtocolError(f"{field} must be text")
        object.__setattr__(self, "echo_enabled", _boolean(self.echo_enabled, "echo enabled"))
        if not isinstance(self.hardware_limit, VoltageLimit):
            raise InstrumentProtocolError("hardware limit is invalid")
        object.__setattr__(
            self, "display_intensity", _integer(self.display_intensity, "display intensity")
        )
        object.__setattr__(
            self, "master_scan_enabled", _boolean(self.master_scan_enabled, "master scan enabled")
        )
        object.__setattr__(
            self,
            "master_scan_voltage_v",
            _finite_float(self.master_scan_voltage_v, "master scan voltage"),
        )
        if not isinstance(self.axes, Mapping):
            raise InstrumentProtocolError("axes must be a mapping")
        copied_axes = dict(self.axes)
        if any(not isinstance(axis, Axis) or not isinstance(state, AxisState)
               for axis, state in copied_axes.items()):
            raise InstrumentProtocolError("axes must map Axis values to AxisState values")
        object.__setattr__(self, "axes", MappingProxyType(copied_axes))
        object.__setattr__(self, "dac_step", _integer(self.dac_step, "DAC step"))
        object.__setattr__(
            self,
            "compatibility_enabled",
            _boolean(self.compatibility_enabled, "compatibility enabled"),
        )
        if not isinstance(self.rotary_mode, RotaryMode):
            raise InstrumentProtocolError("rotary mode is invalid")
        object.__setattr__(
            self,
            "push_to_adjust_disabled",
            _boolean(self.push_to_adjust_disabled, "push-to-adjust disabled"),
        )
        if not isinstance(self.supported_commands, frozenset) or any(
            not isinstance(command, str) for command in self.supported_commands
        ):
            raise InstrumentProtocolError("supported commands must be a frozenset of text")
        object.__setattr__(self, "restricted", _boolean(self.restricted, "restricted"))
        if self.fault_evidence is not None and not isinstance(self.fault_evidence, str):
            raise InstrumentProtocolError("fault evidence must be text or None")
        object.__setattr__(self, "observed_at", _finite_float(self.observed_at, "observed time"))
        object.__setattr__(
            self,
            "axis_command_known",
            _boolean(self.axis_command_known, "axis command known"),
        )


class _MDTProtocol:
    """Serialize and validate one MDT prompt-terminated command exchange."""

    _DEFAULT_MAX_RESPONSE_BYTES = 4096

    def __init__(
        self,
        serial_port: object,
        *,
        echo_enabled: bool | None,
        max_response_bytes: int = _DEFAULT_MAX_RESPONSE_BYTES,
    ) -> None:
        if echo_enabled is not None and not isinstance(echo_enabled, bool):
            raise InstrumentProtocolError("echo state must be a boolean or unknown")
        if isinstance(max_response_bytes, bool) or not isinstance(max_response_bytes, Integral) or max_response_bytes < 1:
            raise InstrumentProtocolError("maximum response size must be a positive integer")
        self._serial = serial_port
        self._echo_enabled = echo_enabled
        self._max_response_bytes = int(max_response_bytes)
        self._receive_buffer = bytearray()
        self._pending_cr_lf = False
        self._pending_response_size = 0

    @property
    def echo_enabled(self) -> bool | None:
        """The observed echo state, or ``None`` until a first response is framed."""
        return self._echo_enabled

    def transact(self, command: str) -> tuple[str, ...]:
        encoded_command = self._command_bytes(command)
        self._require_open()
        self._prepare_for_write()
        self._write_exact(encoded_command + b"\r\n")
        lines = self._read_response()
        if not lines:
            raise InstrumentProtocolError("MDT response did not contain a prompt")
        prompt = lines.pop()
        if prompt == "!":
            raise InstrumentProtocolError("MDT command was rejected by the device")
        if prompt != "*":
            raise InstrumentProtocolError("MDT response did not terminate with a prompt")
        return self._remove_echo(lines, command)

    def transact_echo_transition(self, command: str) -> tuple[str, ...]:
        """Exchange ``echo=`` while accepting a response in either echo mode."""
        encoded_command = self._command_bytes(command)
        self._require_open()
        self._prepare_for_write()
        self._write_exact(encoded_command + b"\r\n")
        lines = self._read_response()
        if not lines:
            raise InstrumentProtocolError("MDT response did not contain a prompt")
        prompt = lines.pop()
        if prompt == "!":
            raise InstrumentProtocolError("MDT command was rejected by the device")
        if prompt != "*":
            raise InstrumentProtocolError("MDT response did not terminate with a prompt")
        is_echo = bool(lines) and lines[0] == command
        self._echo_enabled = None
        return tuple(lines[1:] if is_echo else lines)

    def transact_arrow(self, candidate: bytes) -> tuple[str, ...]:
        """Exchange one of the four fixed, private ANSI arrow candidates."""
        if candidate not in _ARROW_CANDIDATES:
            raise InstrumentProtocolError("MDT arrow candidate is not approved")
        self._require_open()
        self._prepare_for_write()
        self._write_exact(candidate)
        lines = self._read_response()
        if not lines:
            raise InstrumentProtocolError("MDT arrow response did not contain a prompt")
        prompt = lines.pop()
        if prompt == "!":
            raise _MDTArrowRejected("MDT firmware rejected the arrow candidate")
        if prompt != "*":
            raise InstrumentProtocolError("MDT arrow response did not terminate with a prompt")
        return self._remove_echo(lines, candidate.decode("ascii"))

    @staticmethod
    def _command_bytes(command: object) -> bytes:
        if not isinstance(command, str) or not command or "\r" in command or "\n" in command:
            raise InstrumentProtocolError("MDT command must be one non-empty text line")
        try:
            return command.encode("ascii")
        except UnicodeEncodeError as exc:
            raise InstrumentProtocolError("MDT command must be ASCII") from exc

    def _require_open(self) -> None:
        if not getattr(self._serial, "is_open", True):
            raise InstrumentConnectionError("MDT serial port is not open")

    def _write_exact(self, frame: bytes) -> None:
        try:
            count = self._serial.write(frame)
        except (serial.SerialTimeoutException, TimeoutError) as exc:
            raise InstrumentTimeoutError("MDT serial write timed out") from exc
        except (OSError, serial.SerialException) as exc:
            raise InstrumentConnectionError("MDT serial write failed") from exc
        if count != len(frame):
            raise InstrumentConnectionError("MDT serial write was incomplete")

    def _read_byte(self) -> bytes:
        if self._receive_buffer:
            return bytes((self._receive_buffer.pop(0),))
        try:
            byte = self._serial.read(1)
        except (serial.SerialTimeoutException, TimeoutError) as exc:
            raise InstrumentTimeoutError("MDT serial read timed out") from exc
        except (OSError, serial.SerialException) as exc:
            raise InstrumentConnectionError("MDT serial read failed") from exc
        if not isinstance(byte, bytes):
            raise InstrumentProtocolError("MDT serial read did not return bytes")
        if len(byte) > 1:
            raise InstrumentProtocolError("MDT serial read exceeded requested byte count")
        return byte

    def _available_count(self) -> int:
        try:
            waiting = self._serial.in_waiting
        except (serial.SerialTimeoutException, TimeoutError) as exc:
            raise InstrumentTimeoutError("MDT serial receive-buffer check timed out") from exc
        except (AttributeError, OSError, serial.SerialException) as exc:
            raise InstrumentConnectionError("MDT serial receive-buffer check failed") from exc
        if isinstance(waiting, bool) or not isinstance(waiting, Integral) or waiting < 0:
            raise InstrumentProtocolError("MDT serial receive-buffer count is invalid")
        return int(waiting)

    def _timeout_configuration_error(self, error: BaseException, action: str) -> Exception:
        if isinstance(error, (serial.SerialTimeoutException, TimeoutError)):
            return InstrumentTimeoutError(f"MDT serial timeout {action} timed out")
        return InstrumentConnectionError(f"MDT serial timeout {action} failed")

    @contextmanager
    def _nonblocking_probe(self) -> Iterator[None]:
        """Temporarily make receive-buffer probing non-blocking and restore safely."""
        try:
            previous_timeout = self._serial.timeout
        except BaseException as error:
            raise self._timeout_configuration_error(error, "inspection") from error
        primary_error: BaseException | None = None
        try:
            # A setter can mutate before it raises, so restoration belongs in
            # this finally block even when enabling non-blocking mode fails.
            self._serial.timeout = 0
            yield
        except BaseException as error:
            primary_error = error
            raise
        finally:
            try:
                self._serial.timeout = previous_timeout
            except BaseException as error:
                restoration_error = self._timeout_configuration_error(error, "restoration")
                if primary_error is None:
                    raise restoration_error from error
                # Never replace a parser/transport failure with cleanup failure.
                setattr(primary_error, "mdt_timeout_restore_error", error)

    def _resolve_prompt_boundary(self) -> None:
        if self._pending_cr_lf and self._receive_buffer:
            if self._receive_buffer[0] != 0x0A:
                raise InstrumentProtocolError("MDT response contains data after its prompt")
            self._receive_buffer.pop(0)
            self._pending_response_size += 1
            self._check_response_size(self._pending_response_size)
            self._pending_cr_lf = False
            self._pending_response_size = 0
        if self._receive_buffer:
            raise InstrumentProtocolError("MDT response contains data after its prompt")
        if not self._pending_cr_lf:
            self._pending_response_size = 0

    def _drain_available(self, *, validate_boundary: bool = False) -> None:
        """Probe only the bytes needed to decide prompt-boundary validity."""
        waiting = self._available_count()
        if waiting == 0:
            if validate_boundary:
                self._resolve_prompt_boundary()
            return
        probe_limit = min(self._max_response_bytes, 2 if self._pending_cr_lf else 1)
        probe_count = min(waiting, probe_limit)
        with self._nonblocking_probe():
            for _ in range(probe_count):
                try:
                    received = self._serial.read(1)
                except (serial.SerialTimeoutException, TimeoutError) as error:
                    raise InstrumentTimeoutError("MDT serial non-blocking read timed out") from error
                except (OSError, serial.SerialException) as error:
                    raise InstrumentConnectionError("MDT serial non-blocking read failed") from error
                if not isinstance(received, bytes) or not received:
                    raise InstrumentProtocolError(
                        "MDT serial receive-buffer count disagrees with read"
                    )
                if len(received) != 1:
                    raise InstrumentProtocolError("MDT serial non-blocking read exceeded one byte")
                self._receive_buffer.extend(received)
            if validate_boundary:
                # Validate while restoration is still protected, so known
                # trailing data remains the primary protocol failure.
                self._resolve_prompt_boundary()

    def _prepare_for_write(self) -> None:
        self._drain_available(validate_boundary=True)

    def _finish_prompt(self, *, ended_with_cr: bool, byte_count: int) -> None:
        """Validate already-arrived post-prompt bytes without waiting for more."""
        self._pending_cr_lf = ended_with_cr
        self._pending_response_size = byte_count
        self._drain_available(validate_boundary=True)

    def _check_response_size(self, byte_count: int) -> None:
        if byte_count > self._max_response_bytes:
            raise InstrumentProtocolError("MDT response exceeded its maximum size")

    def _read_response(self) -> list[str]:
        lines: list[str] = []
        line = bytearray()
        byte_count = 0
        previous_was_cr = False
        while True:
            byte = self._read_byte()
            if not byte:
                raise InstrumentTimeoutError("MDT response timed out before its prompt")
            if self._pending_cr_lf:
                if byte == b"\n":
                    self._pending_response_size += 1
                    self._check_response_size(self._pending_response_size)
                    self._pending_cr_lf = False
                    self._pending_response_size = 0
                    continue
                self._pending_cr_lf = False
                self._pending_response_size = 0
            byte_count += 1
            self._check_response_size(byte_count)
            value = byte[0]
            if value > 0x7F:
                raise InstrumentProtocolError("MDT response is not ASCII")
            if value == 0x0A and previous_was_cr:
                previous_was_cr = False
                continue
            if value in (0x0A, 0x0D):
                lines.append(line.decode("ascii"))
                if lines[-1] in {"*", "!"}:
                    self._finish_prompt(ended_with_cr=value == 0x0D, byte_count=byte_count)
                    return lines
                line.clear()
                previous_was_cr = value == 0x0D
                continue
            previous_was_cr = False
            line.append(value)

    def _remove_echo(self, lines: list[str], command: str) -> tuple[str, ...]:
        is_echo = bool(lines) and lines[0] == command
        if self._echo_enabled is True:
            if not is_echo:
                raise InstrumentProtocolError("MDT response is missing its expected echo")
            return tuple(lines[1:])
        if self._echo_enabled is False:
            # A help response may legitimately begin with its own '?' keyword.
            # With echo confirmed off, preserve that content and the echo state.
            if is_echo and command != "?":
                raise InstrumentProtocolError("MDT response contains an unexpected echo")
            return tuple(lines)
        self._echo_enabled = is_echo
        return tuple(lines[1:] if is_echo else lines)


class MDT693B:
    """Typed MDT693B session owner with an immutable monitored snapshot."""

    # MDT readbacks are decimal text.  This tolerance admits only parsing and
    # device reporting noise; it is two orders below the 0.1 V motion bound.
    _MOTION_CONFIRMATION_TOLERANCE_V = 1e-6

    _CONNECT_QUERIES = (
        "?", "id?", "serial?", "friendly?", "echo?", "vlimit?", "intensity?",
        "msenable?", "msvoltage?", "xvoltage?", "yvoltage?", "zvoltage?",
        "xmin?", "ymin?", "zmin?", "xmax?", "ymax?", "zmax?", "dacstep?",
        "cm?", "rotarymode?", "pushdisable?",
    )

    def __init__(
        self,
        port: str,
        *,
        application_limits_v: Mapping[Axis, float] | None = None,
        ramp_step_v: float = 0.1,
        ramp_interval_s: float = 0.050,
        io_timeout: float = 0.5,
        monitor_interval_s: float = 0.5,
        monitor_failure_limit: int = 3,
        serial_factory: Callable[..., object] = serial.Serial,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        if not isinstance(port, str) or not port.strip():
            raise ValueError("port must be a non-empty string")
        if application_limits_v is not None and not isinstance(application_limits_v, Mapping):
            raise TypeError("application_limits_v must be a mapping or None")
        supplied_limits = {} if application_limits_v is None else dict(application_limits_v)
        if any(not isinstance(axis, Axis) for axis in supplied_limits):
            raise ValueError("application_limits_v keys must be Axis values")
        limits: dict[Axis, float] = {}
        for axis in Axis:
            value = supplied_limits.get(axis, 75.0)
            limits[axis] = self._constructor_float(
                value, f"{axis.name} application limit", minimum=0.0,
                maximum=75.0, minimum_inclusive=False,
            )
        ramp_step = self._constructor_float(
            ramp_step_v, "ramp_step_v", minimum=0.0, maximum=0.1,
            minimum_inclusive=False,
        )
        ramp_interval = self._constructor_float(
            ramp_interval_s, "ramp_interval_s", minimum=0.050,
        )
        timeout = self._constructor_float(
            io_timeout, "io_timeout", minimum=0.0, minimum_inclusive=False,
        )
        monitor_interval = self._constructor_float(
            monitor_interval_s, "monitor_interval_s", minimum=0.0,
            minimum_inclusive=False,
        )
        if (
            isinstance(monitor_failure_limit, bool)
            or not isinstance(monitor_failure_limit, Integral)
            or monitor_failure_limit < 1
        ):
            raise ValueError("monitor_failure_limit must be a positive integer")
        if not callable(serial_factory):
            raise TypeError("serial_factory must be callable")
        if not callable(clock) or not callable(sleep):
            raise TypeError("clock and sleep must be callable")

        self.port = port
        self._application_limits_v = MappingProxyType(limits)
        self._ramp_step_v = ramp_step
        self._ramp_interval_s = ramp_interval
        self._io_timeout = timeout
        self._monitor_interval_s = monitor_interval
        self._monitor_failure_limit = int(monitor_failure_limit)
        self._serial_factory = serial_factory
        self._clock = clock
        self._sleep = sleep
        self._request_lock = threading.RLock()
        self._lifecycle_lock = threading.RLock()
        self._lifecycle_condition = threading.Condition(self._lifecycle_lock)
        self._cancel_event = threading.Event()
        self._monitor_stop = threading.Event()
        self._monitor_started = threading.Event()
        self._monitor_exited = threading.Event()
        self._monitor_exited.set()
        self._close_requested = threading.Event()
        self._connect_done = threading.Event()
        self._connect_done.set()
        self._serial: object | None = None
        self._protocol: _MDTProtocol | None = None
        self._monitor_thread: threading.Thread | None = None
        self.state = DriverState.DISCONNECTED
        self.status: MDTStatus | None = None
        self.cleanup_error: BaseException | None = None
        self._restriction_evidence: str | None = None
        self._operation_generation = 0
        self._axis_commands_v: dict[Axis, float] | None = None
        self._last_confirmed_actual_v: dict[Axis, float] | None = None
        self._master_scan_command_v: float | None = None
        self._motion_controls_confirmed = True
        self._arrow_capabilities: dict[bytes, bool | None] = {
            candidate: None for candidate in _ARROW_CANDIDATES
        }
        self._last_arrow_motion_at: float | None = None
        self._restore_in_progress = False
        self._zero_in_progress = False
        self._zero_write_attempted = False
        self._emergency_in_progress = False
        self._monitor_failure_count = 0
        self._monitor_restart_required = False
        self._request_lock_uncertain = False
        self.log = get_logger("mdt693b")

    @property
    def application_limits_v(self) -> Mapping[Axis, float]:
        return self._application_limits_v

    @property
    def ramp_step_v(self) -> float:
        return self._ramp_step_v

    @property
    def ramp_interval_s(self) -> float:
        return self._ramp_interval_s

    @property
    def io_timeout(self) -> float:
        return self._io_timeout

    @property
    def monitor_interval_s(self) -> float:
        return self._monitor_interval_s

    @property
    def monitor_failure_limit(self) -> int:
        return self._monitor_failure_limit

    @staticmethod
    def _constructor_float(
        value: object,
        name: str,
        *,
        minimum: float,
        maximum: float | None = None,
        minimum_inclusive: bool = True,
    ) -> float:
        if isinstance(value, bool) or not isinstance(value, Real):
            raise ValueError(f"{name} must be a finite number")
        converted = float(value)
        if not math.isfinite(converted):
            raise ValueError(f"{name} must be a finite number")
        below_minimum = converted < minimum if minimum_inclusive else converted <= minimum
        if below_minimum or (maximum is not None and converted > maximum):
            upper = "" if maximum is None else f" and no greater than {maximum}"
            relation = "at least" if minimum_inclusive else "greater than"
            raise ValueError(f"{name} must be {relation} {minimum}{upper}")
        return converted

    @staticmethod
    def _normalize_commands(lines: tuple[str, ...]) -> frozenset[str]:
        commands: set[str] = set()
        for line in lines:
            for token in re.split(r"[\s,]+", line.strip()):
                normalized = token.strip().lower()
                if normalized:
                    commands.add(normalized)
        return frozenset(commands)

    @staticmethod
    def _single_response(lines: tuple[str, ...], command: str) -> str:
        if len(lines) != 1:
            raise InstrumentProtocolError(
                f"MDT {command} returned {len(lines)} result lines; expected one"
            )
        return lines[0].strip()

    @staticmethod
    def _raw_single_response(lines: tuple[str, ...], command: str) -> str:
        if len(lines) != 1:
            raise InstrumentProtocolError(
                f"MDT {command} returned {len(lines)} result lines; expected one"
            )
        return lines[0]

    @classmethod
    def _parse_number(cls, lines: tuple[str, ...], command: str) -> float:
        text = cls._single_response(lines, command)
        match = re.search(
            r"[-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][-+]?\d+)?\s*$", text
        )
        if match is None:
            raise InstrumentProtocolError(f"MDT {command} did not return a number")
        return _finite_float(match.group(0).strip(), f"MDT {command} response")

    @classmethod
    def _parse_int(cls, lines: tuple[str, ...], command: str) -> int:
        value = cls._parse_number(lines, command)
        if not value.is_integer():
            raise InstrumentProtocolError(f"MDT {command} did not return an integer")
        return int(value)

    @classmethod
    def _parse_bool(cls, lines: tuple[str, ...], command: str) -> bool:
        text = cls._single_response(lines, command)
        value = text.rsplit(":", 1)[-1].rsplit("=", 1)[-1].strip().lower()
        if value in {"1", "on", "true", "enabled"}:
            return True
        if value in {"0", "off", "false", "disabled"}:
            return False
        raise InstrumentProtocolError(f"MDT {command} did not return a boolean")

    @classmethod
    def _parse_identity(cls, lines: tuple[str, ...]) -> tuple[str, str]:
        cleaned = tuple(line.strip() for line in lines if line.strip())
        if len(cleaned) == 2:
            product, firmware = cleaned
        elif len(cleaned) == 1 and "," in cleaned[0]:
            product, firmware = (part.strip() for part in cleaned[0].split(",", 1))
        elif len(cleaned) == 1:
            match = re.fullmatch(r"(MDT69[34]B)\s+(?:firmware\s*)?(\S+)", cleaned[0], re.I)
            if match is None:
                raise InstrumentProtocolError("MDT id? response did not identify product and firmware")
            product, firmware = match.groups()
        else:
            raise InstrumentProtocolError("MDT id? response did not identify product and firmware")
        product = product.rsplit(":", 1)[-1].strip()
        firmware = firmware.rsplit(":", 1)[-1].strip()
        if not product or not firmware:
            raise InstrumentProtocolError("MDT id? response contained an empty identity field")
        return product, firmware

    @classmethod
    def _parse_hardware_limit(cls, lines: tuple[str, ...]) -> VoltageLimit:
        value = cls._parse_int(lines, "vlimit?")
        by_code = {limit.code: limit for limit in VoltageLimit}
        by_voltage = {int(limit.volts): limit for limit in VoltageLimit}
        try:
            return by_code[value] if value in by_code else by_voltage[value]
        except KeyError as error:
            raise InstrumentProtocolError("MDT vlimit? returned an unknown setting") from error

    def _query_unlocked(self, command: str) -> tuple[str, ...]:
        protocol = self._protocol
        if protocol is None:
            raise InstrumentConnectionError("MDT serial protocol is not connected")
        if "=" in command:
            with self._lifecycle_lock:
                if self._zero_in_progress:
                    # Once a zero-ramp setter transaction starts, later
                    # validation errors are no longer preflight-only: bytes
                    # may have reached the controller.
                    self._zero_write_attempted = True
        return protocol.transact(command)

    def _query(self, command: str) -> tuple[str, ...]:
        with self._request_lock:
            return self._query_unlocked(command)

    def _require_no_close_intent_unlocked(self) -> None:
        if self._request_lock_uncertain:
            raise _MDTOperationCancelled(
                "MDT693B request-lock cleanup is uncertain; close and replace "
                "this driver instance"
            )
        if self._close_requested.is_set() or self._monitor_stop.is_set():
            raise _MDTOperationCancelled(
                "MDT693B close is terminal and must be retried before reconnecting"
            )

    def _finish_high_impact_lock_failure(
        self,
        operation: str,
        primary: BaseException,
        cleanup_error: BaseException,
    ) -> None:
        """Publish one truthful terminal result for request-lock cleanup failure."""
        with self._lifecycle_condition:
            self._restore_in_progress = False
            self._zero_in_progress = False
            self._emergency_in_progress = False
            self._motion_controls_confirmed = False
            self.cleanup_error = cleanup_error
            self._lifecycle_condition.notify_all()
        if (
            self.state not in {DriverState.CLOSING, DriverState.DISCONNECTED}
            and not getattr(primary, "mdt_fault_published", False)
        ):
            self._publish_operation_fault(
                f"{operation} request lock cleanup", primary
            )

    def _clear_high_impact_owner_after_admission_failure(
        self,
        operation: str,
        motion_confirmed_before: bool | None,
    ) -> None:
        with self._lifecycle_condition:
            if operation == "restore":
                self._restore_in_progress = False
            elif operation == "ramp_to_zero":
                self._zero_in_progress = False
            elif operation == "emergency_zero":
                self._emergency_in_progress = False
            if motion_confirmed_before is not None:
                self._motion_controls_confirmed = motion_confirmed_before
            self._lifecycle_condition.notify_all()

    @contextmanager
    def _high_impact_request_lock(
        self,
        operation: str,
        *,
        motion_confirmed_before: bool | None = None,
    ) -> Iterator[object]:
        """Put request-lock entry inside the operation's BaseException truth boundary."""
        request_context = self._request_lock
        try:
            entered = request_context.__enter__()
        except BaseException as error:
            self._clear_high_impact_owner_after_admission_failure(
                operation, motion_confirmed_before
            )
            # This exception occurred before the operation body was entered, so
            # any marker carried by a reused exception cannot describe this
            # admission attempt.
            self._publish_operation_fault(operation, error)
            raise
        body_error: BaseException | None = None
        admitted = False
        try:
            with self._lifecycle_lock:
                self._require_no_close_intent_unlocked()
            admitted = True
        except BaseException as error:
            body_error = error
            self._clear_high_impact_owner_after_admission_failure(
                operation, motion_confirmed_before
            )
        if admitted:
            try:
                yield entered
            except BaseException as error:
                body_error = error

        exit_error: BaseException | None = None
        suppressed = False
        try:
            suppressed = bool(
                request_context.__exit__(
                    None if body_error is None else type(body_error),
                    body_error,
                    None if body_error is None else body_error.__traceback__,
                )
            )
        except BaseException as error:
            exit_error = error

        if exit_error is not None:
            release_error: BaseException | None = None
            release_proven = False
            try:
                request_context.release()
                release_proven = True
            except RuntimeError as error:
                release_error = error
                owned_probe = getattr(request_context, "_is_owned", None)
                if callable(owned_probe):
                    try:
                        release_proven = not bool(owned_probe())
                    except BaseException as probe_error:
                        self._attach_cleanup_error(exit_error, probe_error)
            except BaseException as error:
                release_error = error

            primary = body_error or exit_error
            if primary is not exit_error:
                self._attach_cleanup_error(primary, exit_error)
            if release_error is not None and not release_proven:
                self._attach_cleanup_error(primary, release_error)
                with self._lifecycle_lock:
                    self._request_lock_uncertain = True
            self._finish_high_impact_lock_failure(
                operation, primary, exit_error
            )
            raise primary

        if body_error is not None and not suppressed:
            raise body_error

    @contextmanager
    def _connected_request(
        self, *commands: str, allow_fault: bool = False
    ) -> Iterator[MDTStatus]:
        """Hold lifecycle and request state coherent for one public operation."""
        # Capture admission before waiting on the request lock. Generation is
        # monotonic; the lifecycle-locked check below accepts only the same
        # generation and a fully published connected state.
        with self._lifecycle_lock:
            self._require_no_close_intent_unlocked()
            ticket = self._operation_generation
        with self._request_lock:
            with self._lifecycle_lock:
                self._require_no_close_intent_unlocked()
                if ticket != self._operation_generation:
                    raise _MDTOperationCancelled(
                        "MDT693B queued operation was cancelled before it started"
                    )
                status = self.status
                if (
                    self.state not in (
                        {DriverState.READY, DriverState.FAULT}
                        if allow_fault else {DriverState.READY}
                    )
                    or status is None
                    or self._serial is None
                    or self._protocol is None
                ):
                    raise InstrumentConnectionError(
                        f"MDT693B operation requires READY state, not {self.state.name}"
                    )
                required = frozenset(command.lower() for command in commands)
                missing = required.difference(status.supported_commands)
                if missing:
                    names = ", ".join(sorted(missing))
                    raise InstrumentCapabilityError(
                        f"MDT firmware does not advertise required command(s): {names}"
                    )
            yield status

    def _publish_request_success(
        self,
        snapshot: MDTStatus,
        *,
        fault_context: str | None = None,
        actual_observation: Mapping[Axis, float] | None = None,
        **changes: object,
    ) -> MDTStatus:
        with self._lifecycle_lock:
            if (
                self.state not in {DriverState.READY, DriverState.FAULT}
                or self.status is not snapshot
            ):
                raise _MDTOperationCancelled(
                    f"MDT693B operation was cancelled while state is {self.state.name}"
                )
            try:
                published = replace(snapshot, **changes, observed_at=self._clock())
            except BaseException as error:
                if fault_context is not None:
                    self._publish_operation_fault(fault_context, error)
                raise
            self.status = published
            if actual_observation is not None and published.axis_command_known:
                if self._invalidate_axis_authority_for_observation_unlocked(
                    actual_observation,
                    context=fault_context or "getter observation",
                ):
                    assert self.status is not None
                    published = self.status
            return published

    @contextmanager
    def _getter_request(self, *commands: str) -> Iterator[MDTStatus]:
        context = "/".join(commands)
        with self._connected_request(*commands, allow_fault=True) as snapshot:
            try:
                yield snapshot
            except _MDTOperationCancelled:
                raise
            except BaseException as error:
                if not getattr(error, "mdt_fault_published", False):
                    self._publish_operation_fault(context, error)
                raise

    @staticmethod
    def _empty_response(lines: tuple[str, ...], command: str) -> None:
        if lines:
            raise InstrumentProtocolError(
                f"MDT {command} returned {len(lines)} unexpected result lines"
            )

    @staticmethod
    def _friendly_value(value: object) -> str:
        if not isinstance(value, str) or not value:
            raise InstrumentProtocolError("friendly name must be non-empty text")
        if any(character in value for character in ("\r", "\n", "\0")):
            raise InstrumentProtocolError("friendly name must be one line without NUL")
        try:
            value.encode("ascii")
        except UnicodeEncodeError as error:
            raise InstrumentProtocolError("friendly name must be ASCII") from error
        return value

    @staticmethod
    def _setter_integer(value: object, context: str, minimum: int, maximum: int) -> int:
        if isinstance(value, bool) or not isinstance(value, Integral):
            raise InstrumentProtocolError(f"{context} must be an integer")
        converted = int(value)
        if not minimum <= converted <= maximum:
            raise InstrumentProtocolError(
                f"{context} must be within {minimum}..{maximum}"
            )
        return converted

    @staticmethod
    def _setter_float(value: object, context: str) -> float:
        if isinstance(value, bool) or not isinstance(value, Real):
            raise InstrumentProtocolError(f"{context} must be a finite real number")
        converted = float(value)
        if not math.isfinite(converted):
            raise InstrumentProtocolError(f"{context} must be a finite real number")
        return converted

    @staticmethod
    def _wire_number(value: float) -> str:
        return str(int(value)) if value.is_integer() else repr(value)

    def _publish_operation_fault(
        self,
        context: str,
        error: BaseException,
        *,
        evidence_prefix: str | None = None,
    ) -> None:
        with self._lifecycle_condition:
            if self.state in {DriverState.CLOSING, DriverState.DISCONNECTED}:
                return
            self.state = DriverState.FAULT
            self._operation_generation += 1
            self._cancel_event.set()
            try:
                setattr(error, "mdt_fault_published", True)
            except BaseException:
                pass
            previous = self.status
            if previous is None:
                self._lifecycle_condition.notify_all()
                return
            evidence = (
                f"MDT {context} failed confirmation: "
                f"{type(error).__name__}: {error}"
            )
            if evidence_prefix is not None:
                evidence = f"{evidence_prefix}; {evidence}"
            observed_at = previous.observed_at
            try:
                observed_at = self._clock()
            except BaseException as publication_error:
                self._attach_cleanup_error(error, publication_error)
            try:
                fault_status = replace(
                    previous, fault_evidence=evidence, observed_at=observed_at
                )
            except BaseException as publication_error:
                self._attach_cleanup_error(error, publication_error)
                try:
                    fault_status = MDTStatus(
                        product=previous.product,
                        firmware=previous.firmware,
                        serial_number=previous.serial_number,
                        friendly_name=previous.friendly_name,
                        echo_enabled=previous.echo_enabled,
                        hardware_limit=previous.hardware_limit,
                        display_intensity=previous.display_intensity,
                        master_scan_enabled=previous.master_scan_enabled,
                        master_scan_voltage_v=previous.master_scan_voltage_v,
                        axes=previous.axes,
                        dac_step=previous.dac_step,
                        compatibility_enabled=previous.compatibility_enabled,
                        rotary_mode=previous.rotary_mode,
                        push_to_adjust_disabled=previous.push_to_adjust_disabled,
                        supported_commands=previous.supported_commands,
                        restricted=previous.restricted,
                        fault_evidence=evidence,
                        observed_at=previous.observed_at,
                        axis_command_known=previous.axis_command_known,
                    )
                except BaseException as fallback_error:
                    self._attach_cleanup_error(error, fallback_error)
                    self._lifecycle_condition.notify_all()
                    return
            self.status = fault_status
            self._lifecycle_condition.notify_all()

    def _confirmed_scalar_setter(
        self,
        *,
        setter_capability: str,
        query: str,
        command: str,
        expected: object,
        parser: Callable[[tuple[str, ...], str], object],
        field: str,
    ) -> object:
        with self._connected_request(setter_capability, query) as snapshot:
            try:
                self._empty_response(self._query_unlocked(command), command)
                observed = parser(self._query_unlocked(query), query)
                if observed != expected:
                    raise DeviceFault(
                        f"MDT {query} readback {observed!r} did not confirm {expected!r}"
                    )
                self._publish_request_success(
                    snapshot,
                    fault_context=command,
                    **{field: observed, "fault_evidence": snapshot.fault_evidence},
                )
            except BaseException as error:
                if (
                    not isinstance(error, _MDTOperationCancelled)
                    and not getattr(error, "mdt_fault_published", False)
                ):
                    self._publish_operation_fault(command, error)
                raise
            return observed

    def get_supported_commands(self) -> frozenset[str]:
        with self._getter_request("?") as snapshot:
            supported = self._normalize_commands(self._query_unlocked("?")).union({"?"})
            self._publish_request_success(
                snapshot, fault_context="?", supported_commands=supported
            )
            return supported

    def get_product_information(self) -> tuple[str, str]:
        with self._getter_request("id?") as snapshot:
            product, firmware = self._parse_identity(self._query_unlocked("id?"))
            if product.strip().upper() != "MDT693B":
                raise InstrumentConnectionError(f"Unexpected MDT product: {product!r}")
            self._publish_request_success(
                snapshot, fault_context="id?", product=product, firmware=firmware
            )
            return product, firmware

    def get_serial_number(self) -> str:
        with self._getter_request("serial?") as snapshot:
            value = self._single_response(self._query_unlocked("serial?"), "serial?")
            self._publish_request_success(
                snapshot, fault_context="serial?", serial_number=value
            )
            return value

    def get_friendly_name(self) -> str:
        with self._getter_request("friendly?") as snapshot:
            value = self._raw_single_response(self._query_unlocked("friendly?"), "friendly?")
            self._publish_request_success(
                snapshot, fault_context="friendly?", friendly_name=value
            )
            return value

    def set_friendly_name(self, value: str) -> str:
        expected = self._friendly_value(value)
        result = self._confirmed_scalar_setter(
            setter_capability="friendly=", query="friendly?",
            command=f"friendly={expected}", expected=expected,
            parser=self._raw_single_response, field="friendly_name",
        )
        return str(result)

    def get_echo_enabled(self) -> bool:
        with self._getter_request("echo?") as snapshot:
            value = self._parse_bool(self._query_unlocked("echo?"), "echo?")
            if self._protocol is None or self._protocol.echo_enabled is not value:
                raise InstrumentProtocolError("MDT observed echo does not match echo? response")
            self._publish_request_success(
                snapshot, fault_context="echo?", echo_enabled=value
            )
            return value

    def set_echo_enabled(self, enabled: bool) -> bool:
        expected = _boolean(enabled, "echo enabled")
        command = f"echo={int(expected)}"
        with self._connected_request("echo=", "echo?") as snapshot:
            protocol = self._protocol
            assert protocol is not None
            try:
                self._empty_response(protocol.transact_echo_transition(command), command)
                observed = self._parse_bool(self._query_unlocked("echo?"), "echo?")
                if protocol.echo_enabled is not observed:
                    raise DeviceFault("MDT echo parser mode did not match echo? readback")
                if observed is not expected:
                    raise DeviceFault(
                        f"MDT echo? readback {observed!r} did not confirm {expected!r}"
                    )
                self._publish_request_success(
                    snapshot,
                    fault_context=command,
                    echo_enabled=observed,
                    fault_evidence=snapshot.fault_evidence,
                )
            except BaseException as error:
                if (
                    not isinstance(error, _MDTOperationCancelled)
                    and not getattr(error, "mdt_fault_published", False)
                ):
                    self._publish_operation_fault(command, error)
                raise
            return observed

    def get_hardware_voltage_limit(self) -> VoltageLimit:
        with self._getter_request("vlimit?") as snapshot:
            value = self._parse_hardware_limit(self._query_unlocked("vlimit?"))
            self._publish_request_success(
                snapshot, fault_context="vlimit?", hardware_limit=value
            )
            return value

    def get_display_intensity(self) -> int:
        with self._getter_request("intensity?") as snapshot:
            value = self._parse_int(self._query_unlocked("intensity?"), "intensity?")
            if not 0 <= value <= 15:
                raise InstrumentProtocolError("MDT intensity? response is outside 0..15")
            self._publish_request_success(
                snapshot, fault_context="intensity?", display_intensity=value
            )
            return value

    def set_display_intensity(self, value: int) -> int:
        expected = self._setter_integer(value, "display intensity", 0, 15)
        result = self._confirmed_scalar_setter(
            setter_capability="intensity=", query="intensity?",
            command=f"intensity={expected}", expected=expected,
            parser=self._parse_int, field="display_intensity",
        )
        return int(result)

    @staticmethod
    def _axis_value(axis: object) -> Axis:
        if not isinstance(axis, Axis):
            raise InstrumentProtocolError("axis must be an Axis value")
        return axis

    @classmethod
    def _motion_value(cls, value: object, context: str) -> float:
        converted = cls._setter_float(value, context)
        if converted < 0.0:
            raise InstrumentSafetyError(f"{context} must be at least 0 V")
        if converted > 75.0:
            raise InstrumentSafetyError(f"{context} exceeds the 75 V project ceiling")
        return converted

    @staticmethod
    def _require_confirmation(confirm: object, context: str) -> None:
        if confirm is not True:
            raise InstrumentSafetyError(f"{context} requires confirm=True")

    def _transition_axis_authority_unlocked(
        self,
        known: bool,
        *,
        commands: Mapping[Axis, float] | None = None,
        actual: Mapping[Axis, float] | None = None,
        evidence: str | None = None,
        publish_evidence: bool = False,
    ) -> None:
        """Atomically publish command authority while holding lifecycle state."""
        if known:
            if commands is None or actual is None:
                raise InstrumentProtocolError(
                    "known axis-command authority requires commands and actuals"
                )
            copied_commands = dict(commands)
            copied_actual = dict(actual)
            if set(copied_commands) != set(Axis) or set(copied_actual) != set(Axis):
                raise InstrumentProtocolError(
                    "axis-command authority requires complete XYZ evidence"
                )
            self._axis_commands_v = copied_commands
            self._last_confirmed_actual_v = copied_actual
        else:
            self._axis_commands_v = None
            self._last_confirmed_actual_v = None
        if self.status is not None:
            changes: dict[str, object] = {"axis_command_known": known}
            if publish_evidence:
                changes["fault_evidence"] = evidence
            self.status = replace(self.status, **changes)
        self._lifecycle_condition.notify_all()

    def _axis_authority_deviations_unlocked(
        self, actual: Mapping[Axis, float]
    ) -> tuple[tuple[Axis, float, float], ...]:
        status = self.status
        expected = self._last_confirmed_actual_v
        if status is None or not status.axis_command_known or expected is None:
            return ()
        tolerance = self._MOTION_CONFIRMATION_TOLERANCE_V
        return tuple(
            (axis, expected[axis], observed)
            for axis, observed in actual.items()
            if axis in expected
            and not math.isclose(
                observed, expected[axis], rel_tol=0.0, abs_tol=tolerance
            )
        )

    def _invalidate_axis_authority_for_observation_unlocked(
        self, actual: Mapping[Axis, float], *, context: str
    ) -> bool:
        deviations = self._axis_authority_deviations_unlocked(actual)
        if not deviations:
            return False
        details = ", ".join(
            f"{axis.name} expected {expected:g} V but observed {observed:g} V"
            for axis, expected, observed in deviations
        )
        evidence = (
            f"MDT axis-command authority invalidated by {context}: {details}"
        )
        self._transition_axis_authority_unlocked(
            False, evidence=evidence, publish_evidence=True
        )
        return True

    def _command_state_unlocked(
        self, status: MDTStatus
    ) -> tuple[dict[Axis, float], float]:
        master_voltage = (
            status.master_scan_voltage_v
            if self._master_scan_command_v is None
            else self._master_scan_command_v
        )
        if not status.axis_command_known or self._axis_commands_v is None:
            raise InstrumentSafetyError(
                "MDT absolute axis motion is disarmed because the base-axis "
                "command is unknown; use confirmed emergency zero or explicit "
                "operator-attested baseline adoption"
            )
        commands = dict(self._axis_commands_v)
        return commands, master_voltage

    def _motion_preflight_snapshot(
        self, *commands: str, require_axis_authority: bool = True
    ) -> tuple[MDTStatus, DriverState, int, dict[Axis, float], float]:
        """Validate lifecycle/capability before waiting for the operation lock."""
        with self._lifecycle_lock:
            self._require_no_close_intent_unlocked()
            status = self.status
            state = self.state
            if (
                state not in {DriverState.READY, DriverState.ACTIVE, DriverState.FAULT}
                or status is None
                or self._serial is None
                or self._protocol is None
            ):
                raise InstrumentConnectionError(
                    f"MDT693B motion requires connected state, not {state.name}"
                )
            if self._emergency_in_progress:
                raise _MDTOperationCancelled(
                    "MDT693B motion was cancelled by the active emergency owner"
                )
            missing = frozenset(command.lower() for command in commands).difference(
                status.supported_commands
            )
            if missing:
                names = ", ".join(sorted(missing))
                raise InstrumentCapabilityError(
                    f"MDT firmware does not advertise required command(s): {names}"
                )
            if require_axis_authority:
                axis_commands, master_voltage = self._command_state_unlocked(status)
            else:
                axis_commands = {}
                master_voltage = (
                    status.master_scan_voltage_v
                    if self._master_scan_command_v is None
                    else self._master_scan_command_v
                )
            return (
                status,
                state,
                self._operation_generation,
                axis_commands,
                master_voltage,
            )

    def _validate_motion_outputs(
        self,
        status: MDTStatus,
        state: DriverState,
        current_axis_commands: Mapping[Axis, float],
        current_master_voltage: float,
        target_axis_commands: Mapping[Axis, float],
        target_master_voltage: float,
        target_master_enabled: bool,
        affected_axes: frozenset[Axis],
        *,
        changes_master_voltage: bool = False,
        residuals: Mapping[Axis, float] | None = None,
    ) -> None:
        hardware_ceiling = min(75.0, status.hardware_limit.volts)
        contribution = target_master_voltage if target_master_enabled else 0.0
        current_contribution = (
            current_master_voltage if status.master_scan_enabled else 0.0
        )
        reduction_only = status.restricted or state is DriverState.FAULT
        actual_residuals = (
            self._motion_residuals(
                {axis: status.axes[axis].actual_v for axis in Axis},
                current_axis_commands,
                current_master_voltage,
                status.master_scan_enabled,
            )
            if residuals is None
            else residuals
        )
        if reduction_only:
            if state is DriverState.FAULT and not self._motion_controls_confirmed:
                raise InstrumentSafetyError(
                    "FAULT-state motion is blocked because the last control command "
                    "was not confirmed"
                )
            if (
                changes_master_voltage
                and target_master_voltage > current_master_voltage
            ):
                raise InstrumentSafetyError(
                    "Master Scan motion is not a provable reduction from restricted/FAULT state"
                )

        for axis in affected_axes:
            actual_target = (
                target_axis_commands[axis]
                + contribution
                + actual_residuals[axis]
            )
            axis_state = status.axes[axis]
            ceiling = min(
                75.0,
                hardware_ceiling,
                self.application_limits_v[axis],
                axis_state.maximum_v,
            )
            current_actual = axis_state.actual_v
            current_lower_violation = max(
                axis_state.minimum_v - current_actual, 0.0
            )
            current_upper_violation = max(current_actual - ceiling, 0.0)
            target_lower_violation = max(
                axis_state.minimum_v - actual_target, 0.0
            )
            target_upper_violation = max(actual_target - ceiling, 0.0)
            current_violation = max(
                current_lower_violation, current_upper_violation
            )
            target_violation = max(
                target_lower_violation, target_upper_violation
            )

            if not reduction_only:
                if target_lower_violation > 0.0:
                    raise InstrumentSafetyError(
                        f"{axis.name} target {actual_target:g} V is below its "
                        f"{axis_state.minimum_v:g} V minimum"
                    )
                if target_upper_violation > 0.0:
                    raise InstrumentSafetyError(
                        f"{axis.name} target {actual_target:g} V exceeds its "
                        f"{ceiling:g} V effective ceiling"
                    )
                continue

            crossed_bound_sides = (
                current_lower_violation > 0.0
                and target_upper_violation > 0.0
            ) or (
                current_upper_violation > 0.0
                and target_lower_violation > 0.0
            )
            if crossed_bound_sides:
                raise InstrumentSafetyError(
                    f"{axis.name} target {actual_target:g} V crosses to the "
                    "opposite bound side from restricted/FAULT state"
                )

            current_expected = (
                current_axis_commands[axis]
                + current_contribution
                + actual_residuals[axis]
            )
            if (
                target_axis_commands[axis] > current_axis_commands[axis]
                or actual_target > current_actual
                or actual_target > current_expected
            ):
                raise InstrumentSafetyError(
                    f"{axis.name} motion is not a provable reduction from "
                    f"restricted/{state.name} state"
                )

            if current_violation > 0.0:
                if target_violation >= current_violation:
                    raise InstrumentSafetyError(
                        f"{axis.name} target violation {target_violation:g} V "
                        f"is not a strict reduction from the observed "
                        f"{current_violation:g} V restricted/FAULT violation"
                    )
                continue

            if target_lower_violation > 0.0:
                raise InstrumentSafetyError(
                    f"{axis.name} target {actual_target:g} V is below its "
                    f"{axis_state.minimum_v:g} V minimum"
                )
            if target_upper_violation > 0.0:
                raise InstrumentSafetyError(
                    f"{axis.name} target {actual_target:g} V exceeds its "
                    f"{ceiling:g} V effective ceiling"
                )

    def _revalidate_motion_start(
        self,
        generation: int,
        commands: tuple[str, ...],
        validator: Callable[
            [MDTStatus, DriverState, dict[Axis, float], float], None
        ],
        *,
        require_axis_authority: bool = True,
    ) -> tuple[MDTStatus, DriverState, dict[Axis, float], float]:
        with self._lifecycle_lock:
            self._require_no_close_intent_unlocked()
            if generation != self._operation_generation:
                raise _MDTOperationCancelled(
                    "MDT693B queued motion was cancelled before it started"
                )
            status = self.status
            state = self.state
            if (
                state not in {DriverState.READY, DriverState.FAULT}
                or status is None
                or self._serial is None
                or self._protocol is None
            ):
                raise _MDTOperationCancelled(
                    f"MDT693B motion was cancelled while state is {state.name}"
                )
            missing = frozenset(commands).difference(status.supported_commands)
            if missing:
                names = ", ".join(sorted(missing))
                raise InstrumentCapabilityError(
                    f"MDT firmware does not advertise required command(s): {names}"
                )
            if require_axis_authority:
                axis_commands, master_voltage = self._command_state_unlocked(status)
            else:
                axis_commands = {}
                master_voltage = (
                    status.master_scan_voltage_v
                    if self._master_scan_command_v is None
                    else self._master_scan_command_v
                )
            validator(status, state, axis_commands, master_voltage)
            origin_state = state
            self.state = DriverState.ACTIVE
            return status, origin_state, axis_commands, master_voltage

    def _require_motion_generation(self, generation: int) -> None:
        with self._lifecycle_lock:
            if (
                generation != self._operation_generation
                or self.state is not DriverState.ACTIVE
            ):
                raise _MDTOperationCancelled(
                    f"MDT693B motion was cancelled while state is {self.state.name}"
                )

    def _read_all_actual_unlocked(self) -> dict[Axis, float]:
        return {
            axis: self._parse_number(
                self._query_unlocked(f"{axis.value}voltage?"),
                f"{axis.value}voltage?",
            )
            for axis in Axis
        }

    def _fresh_absolute_motion_start(
        self,
        generation: int,
        axis_commands: Mapping[Axis, float],
        master_voltage: float,
    ) -> tuple[MDTStatus, dict[Axis, float], float]:
        """Revalidate authority after request-lock admission, before a setter."""
        enabled = self._parse_bool(
            self._query_unlocked("msenable?"), "msenable?"
        )
        self._require_motion_generation(generation)
        fresh_master = self._parse_number(
            self._query_unlocked("msvoltage?"), "msvoltage?"
        )
        self._require_motion_generation(generation)
        actual = self._read_all_actual_unlocked()
        self._require_motion_generation(generation)
        with self._lifecycle_condition:
            previous = self.status
            if previous is None or not previous.axis_command_known:
                raise InstrumentSafetyError(
                    "MDT absolute axis motion lost base-command authority before write"
                )
            axes = {
                axis: AxisState(
                    actual[axis],
                    previous.axes[axis].minimum_v,
                    previous.axes[axis].maximum_v,
                )
                for axis in Axis
            }
            self.status = replace(
                previous,
                axes=axes,
                master_scan_enabled=enabled,
                master_scan_voltage_v=fresh_master,
                observed_at=self._clock(),
            )
            deviations = self._axis_authority_deviations_unlocked(actual)
            master_changed = (
                enabled is not previous.master_scan_enabled
                or not math.isclose(
                    fresh_master,
                    master_voltage,
                    rel_tol=0.0,
                    abs_tol=self._MOTION_CONFIRMATION_TOLERANCE_V,
                )
            )
            if deviations or master_changed:
                details = [
                    f"{axis.name} expected {expected:g} V but observed {observed:g} V"
                    for axis, expected, observed in deviations
                ]
                if enabled is not previous.master_scan_enabled:
                    details.append(
                        "Master Scan enable changed from "
                        f"{previous.master_scan_enabled} to {enabled}"
                    )
                if not math.isclose(
                    fresh_master,
                    master_voltage,
                    rel_tol=0.0,
                    abs_tol=self._MOTION_CONFIRMATION_TOLERANCE_V,
                ):
                    details.append(
                        f"Master Scan voltage expected {master_voltage:g} V "
                        f"but observed {fresh_master:g} V"
                    )
                evidence = (
                    "MDT axis-command authority invalidated before first setter: "
                    + ", ".join(details)
                )
                self._transition_axis_authority_unlocked(
                    False, evidence=evidence, publish_evidence=True
                )
                raise DeviceFault(evidence)
            self._master_scan_command_v = fresh_master
            self._last_confirmed_actual_v = dict(actual)
            assert self.status is not None
            return self.status, dict(axis_commands), fresh_master

    def _revalidate_ramp_interval_actual(
        self,
        generation: int,
        expected: Mapping[Axis, float],
    ) -> dict[Axis, float]:
        """Observe XYZ after an enforced interval and before the next setter."""
        actual = self._read_all_actual_unlocked()
        self._require_motion_generation(generation)
        tolerance = self._MOTION_CONFIRMATION_TOLERANCE_V
        deviations = tuple(
            (axis, expected[axis], actual[axis])
            for axis in Axis
            if not math.isclose(
                actual[axis],
                expected[axis],
                rel_tol=0.0,
                abs_tol=tolerance,
            )
        )
        with self._lifecycle_condition:
            previous = self.status
            if previous is None or not previous.axis_command_known:
                raise InstrumentSafetyError(
                    "MDT ramp lost base-command authority before its next setter"
                )
            axes = {
                axis: AxisState(
                    actual[axis],
                    previous.axes[axis].minimum_v,
                    previous.axes[axis].maximum_v,
                )
                for axis in Axis
            }
            self.status = replace(
                previous, axes=axes, observed_at=self._clock()
            )
            if deviations:
                details = ", ".join(
                    f"{axis.name} expected {wanted:g} V but observed {seen:g} V"
                    for axis, wanted, seen in deviations
                )
                evidence = (
                    "MDT axis-command authority invalidated during ramp interval: "
                    + details
                )
                self._transition_axis_authority_unlocked(
                    False, evidence=evidence, publish_evidence=True
                )
                raise DeviceFault(evidence)
            self._last_confirmed_actual_v = dict(actual)
        return actual

    def _require_prewrite_actual_authority(
        self,
        generation: int,
        actual: Mapping[Axis, float],
        *,
        context: str,
    ) -> MDTStatus:
        """Publish a locked observation and reject if it revokes authority."""
        with self._lifecycle_lock:
            current = self.status
            if current is None:
                raise _MDTOperationCancelled(
                    f"MDT {context} lost its status snapshot"
                )
            restricted, evidence = self._classify_motion_actuals(current, actual)
        observed = self._publish_motion_step(
            generation,
            actual,
            restricted=restricted,
            fault_evidence=evidence,
        )
        if not observed.axis_command_known:
            raise DeviceFault(
                observed.fault_evidence
                or f"MDT axis-command authority was invalidated before {context}"
            )
        return observed

    @staticmethod
    def _motion_steps(maximum_delta: float, step_v: float) -> int:
        return max(1, math.ceil(maximum_delta / step_v))

    @staticmethod
    def _interpolated(start: float, target: float, index: int, steps: int) -> float:
        if index == steps:
            return target
        return start + (target - start) * (index / steps)

    def _verify_actual_transition(
        self, previous: Mapping[Axis, float], actual: Mapping[Axis, float]
    ) -> None:
        tolerance = self._MOTION_CONFIRMATION_TOLERANCE_V
        for axis in Axis:
            transition = abs(actual[axis] - previous[axis])
            if transition > 0.1 + tolerance:
                raise DeviceFault(
                    f"MDT {axis.name} actual transition {transition:g} V exceeds 0.1 V"
                )

    def _verify_actual_safety(
        self,
        status: MDTStatus,
        previous: Mapping[Axis, float],
        actual: Mapping[Axis, float],
    ) -> None:
        tolerance = self._MOTION_CONFIRMATION_TOLERANCE_V
        for axis in Axis:
            axis_state = status.axes[axis]
            ceiling = min(
                75.0,
                status.hardware_limit.volts,
                self.application_limits_v[axis],
                axis_state.maximum_v,
            )
            previous_distance = max(
                axis_state.minimum_v - previous[axis],
                previous[axis] - ceiling,
                0.0,
            )
            actual_distance = max(
                axis_state.minimum_v - actual[axis],
                actual[axis] - ceiling,
                0.0,
            )
            newly_unsafe = previous_distance <= tolerance and actual_distance > tolerance
            worsening_unsafe = actual_distance > previous_distance + tolerance
            if newly_unsafe or worsening_unsafe:
                raise DeviceFault(
                    f"MDT {axis.name} actual output {actual[axis]:g} V remains "
                    f"outside {axis_state.minimum_v:g}..{ceiling:g} V without "
                    "a provable reduction"
                )

    def _classify_motion_actuals(
        self,
        status: MDTStatus,
        actual: Mapping[Axis, float],
    ) -> tuple[bool, str | None]:
        violations: list[str] = []
        for axis in Axis:
            axis_state = status.axes[axis]
            ceiling = min(
                75.0,
                status.hardware_limit.volts,
                self.application_limits_v[axis],
                axis_state.maximum_v,
            )
            if actual[axis] < axis_state.minimum_v:
                violations.append(
                    f"{axis.name}={actual[axis]:g} V below "
                    f"{axis_state.minimum_v:g} V"
                )
            elif actual[axis] > ceiling:
                violations.append(
                    f"{axis.name}={actual[axis]:g} V exceeds {ceiling:g} V"
                )
        if violations:
            evidence = "MDT motion limit observation: " + ", ".join(violations)
            return True, evidence
        if status.restricted:
            return True, self._restriction_evidence or status.fault_evidence
        return False, status.fault_evidence

    @classmethod
    def _motion_residuals(
        cls,
        actual: Mapping[Axis, float],
        axis_commands: Mapping[Axis, float],
        master_voltage: float,
        master_enabled: bool,
    ) -> dict[Axis, float]:
        contribution = master_voltage if master_enabled else 0.0
        return {
            axis: actual[axis] - axis_commands[axis] - contribution
            for axis in Axis
        }

    @classmethod
    def _expected_motion_actuals(
        cls,
        residuals: Mapping[Axis, float],
        axis_commands: Mapping[Axis, float],
        master_voltage: float,
        master_enabled: bool,
    ) -> dict[Axis, float]:
        contribution = master_voltage if master_enabled else 0.0
        return {
            axis: axis_commands[axis] + contribution + residuals[axis]
            for axis in Axis
        }

    @classmethod
    def _verify_commanded_effect(
        cls,
        expected: Mapping[Axis, float],
        actual: Mapping[Axis, float],
    ) -> None:
        tolerance = cls._MOTION_CONFIRMATION_TOLERANCE_V
        for axis in Axis:
            if not math.isclose(
                actual[axis], expected[axis], rel_tol=0.0, abs_tol=tolerance
            ):
                raise DeviceFault(
                    f"MDT {axis.name} commanded effect expected "
                    f"{expected[axis]:g} V but observed {actual[axis]:g} V"
                )

    @classmethod
    def _verify_reduction_transition(
        cls,
        status: MDTStatus,
        origin_state: DriverState,
        previous: Mapping[Axis, float],
        actual: Mapping[Axis, float],
    ) -> None:
        if not status.restricted and origin_state is not DriverState.FAULT:
            return
        tolerance = cls._MOTION_CONFIRMATION_TOLERANCE_V
        for axis in Axis:
            if actual[axis] > previous[axis] + tolerance:
                raise DeviceFault(
                    f"MDT {axis.name} restricted/FAULT actual increase from "
                    f"{previous[axis]:g} V to {actual[axis]:g} V"
                )

    def _publish_motion_step(
        self,
        generation: int,
        actual: Mapping[Axis, float],
        *,
        axis_commands: Mapping[Axis, float] | None = None,
        master_voltage: float | None = None,
        master_enabled: bool | None = None,
        hardware_limit: VoltageLimit | None = None,
        dac_step: int | None = None,
        restricted: bool | None = None,
        fault_evidence: str | None = None,
        confirmed_output_change: bool = False,
    ) -> MDTStatus:
        with self._lifecycle_lock:
            if (
                generation != self._operation_generation
                or self.state is not DriverState.ACTIVE
            ):
                raise _MDTOperationCancelled(
                    f"MDT693B motion was cancelled while state is {self.state.name}"
                )
            previous = self.status
            if previous is None:
                raise _MDTOperationCancelled("MDT693B motion lost its status snapshot")
            axes = {
                axis: AxisState(
                    actual[axis],
                    previous.axes[axis].minimum_v,
                    previous.axes[axis].maximum_v,
                )
                for axis in Axis
            }
            changes: dict[str, object] = {"axes": axes, "observed_at": self._clock()}
            if master_voltage is not None:
                changes["master_scan_voltage_v"] = master_voltage
            if master_enabled is not None:
                changes["master_scan_enabled"] = master_enabled
            if hardware_limit is not None:
                changes["hardware_limit"] = hardware_limit
            if dac_step is not None:
                changes["dac_step"] = dac_step
            if restricted is not None:
                changes["restricted"] = restricted
                changes["fault_evidence"] = fault_evidence
            published = replace(previous, **changes)
            self.status = published
            if axis_commands is not None:
                self._axis_commands_v = dict(axis_commands)
            if master_voltage is not None:
                self._master_scan_command_v = master_voltage
            if restricted and fault_evidence is not None:
                self._restriction_evidence = fault_evidence
            if published.axis_command_known:
                if confirmed_output_change:
                    self._last_confirmed_actual_v = dict(actual)
                elif self._invalidate_axis_authority_for_observation_unlocked(
                    actual, context="unconfirmed motion observation"
                ):
                    assert self.status is not None
                    published = self.status
            return published

    def _complete_motion(self, generation: int, origin_state: DriverState) -> MDTStatus:
        with self._lifecycle_lock:
            if (
                generation != self._operation_generation
                or self.state is not DriverState.ACTIVE
                or self.status is None
            ):
                raise _MDTOperationCancelled(
                    f"MDT693B motion was cancelled while state is {self.state.name}"
                )
            self.state = DriverState.FAULT if origin_state is DriverState.FAULT else DriverState.READY
            return self.status

    def _run_ramp(
        self,
        *,
        generation: int,
        capabilities: tuple[str, ...],
        context: str,
        validator: Callable[
            [MDTStatus, DriverState, dict[Axis, float], float], None
        ],
        command_for_step: Callable[[float], str],
        start_for: Callable[[dict[Axis, float], float], float],
        target_v: float,
        axis_commands_for_step: Callable[
            [dict[Axis, float], float], Mapping[Axis, float] | None
        ],
        master_for_step: Callable[[float], float | None],
        refresh_start: Callable[
            [MDTStatus, DriverState, dict[Axis, float], float, int],
            tuple[MDTStatus, dict[Axis, float], float],
        ] | None = None,
        confirm_master_property: bool = False,
        absolute_axis_operation: bool = False,
    ) -> MDTStatus:
        with self._request_lock:
            command_unconfirmed = False
            try:
                status, origin_state, axis_commands, master_voltage = (
                    self._revalidate_motion_start(
                        generation, capabilities, validator
                    )
                )
                if absolute_axis_operation:
                    status, axis_commands, master_voltage = (
                        self._fresh_absolute_motion_start(
                            generation, axis_commands, master_voltage
                        )
                    )
                if refresh_start is not None:
                    status, axis_commands, master_voltage = refresh_start(
                        status,
                        origin_state,
                        axis_commands,
                        master_voltage,
                        generation,
                    )
                previous_actual = {
                    axis: status.axes[axis].actual_v for axis in Axis
                }
                residuals = self._motion_residuals(
                    previous_actual,
                    axis_commands,
                    master_voltage,
                    status.master_scan_enabled,
                )
                start = start_for(axis_commands, master_voltage)
                steps = self._motion_steps(abs(target_v - start), self.ramp_step_v)
                for index in range(1, steps + 1):
                    self._require_motion_generation(generation)
                    step_value = self._interpolated(start, target_v, index, steps)
                    command = command_for_step(step_value)
                    next_axis_commands = axis_commands_for_step(
                        axis_commands, step_value
                    )
                    expected_axis_commands = (
                        axis_commands
                        if next_axis_commands is None
                        else next_axis_commands
                    )
                    next_master = master_for_step(step_value)
                    expected_master = (
                        master_voltage if next_master is None else next_master
                    )
                    expected_actual = self._expected_motion_actuals(
                        residuals,
                        expected_axis_commands,
                        expected_master,
                        status.master_scan_enabled,
                    )
                    command_unconfirmed = True
                    self._empty_response(self._query_unlocked(command), command)
                    self._require_motion_generation(generation)
                    observed_master: float | None = None
                    master_property_matches = True
                    if confirm_master_property:
                        observed_master = self._parse_number(
                            self._query_unlocked("msvoltage?"), "msvoltage?"
                        )
                        self._require_motion_generation(generation)
                        master_property_matches = math.isclose(
                            observed_master,
                            step_value,
                            rel_tol=0.0,
                            abs_tol=self._MOTION_CONFIRMATION_TOLERANCE_V,
                        )
                    actual = self._read_all_actual_unlocked()
                    try:
                        self._verify_actual_transition(previous_actual, actual)
                        self._verify_reduction_transition(
                            status, origin_state, previous_actual, actual
                        )
                        self._verify_actual_safety(status, previous_actual, actual)
                        self._verify_commanded_effect(expected_actual, actual)
                        if not master_property_matches:
                            assert observed_master is not None
                            raise DeviceFault(
                                "MDT msvoltage? readback "
                                f"{observed_master:g} V did not confirm "
                                f"{step_value:g} V"
                            )
                    except BaseException:
                        # The unsafe observation is evidence even though the
                        # associated command target is not confirmed.
                        self._publish_motion_step(generation, actual)
                        if (
                            confirm_master_property
                            and observed_master is not None
                            and math.isclose(
                                observed_master,
                                master_voltage,
                                rel_tol=0.0,
                                abs_tol=self._MOTION_CONFIRMATION_TOLERANCE_V,
                            )
                        ):
                            # The requested step failed, but the property
                            # readback proves the prior cached control remains
                            # authoritative.
                            command_unconfirmed = False
                        raise
                    self._publish_motion_step(
                        generation,
                        actual,
                        axis_commands=next_axis_commands,
                        master_voltage=next_master,
                        confirmed_output_change=True,
                    )
                    command_unconfirmed = False
                    if next_axis_commands is not None:
                        axis_commands = dict(next_axis_commands)
                    if next_master is not None:
                        master_voltage = next_master
                    previous_actual = actual
                    residuals = self._motion_residuals(
                        actual,
                        axis_commands,
                        master_voltage,
                        status.master_scan_enabled,
                    )
                    if index != steps:
                        self._sleep(self.ramp_interval_s)
                        self._require_motion_generation(generation)
                        previous_actual = self._revalidate_ramp_interval_actual(
                            generation, previous_actual
                        )
                        residuals = self._motion_residuals(
                            previous_actual,
                            axis_commands,
                            master_voltage,
                            status.master_scan_enabled,
                        )
                return self._complete_motion(generation, origin_state)
            except _MDTOperationCancelled:
                if command_unconfirmed:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                raise
            except BaseException as error:
                with self._lifecycle_lock:
                    active = self.state is DriverState.ACTIVE
                if (
                    not active
                    and not command_unconfirmed
                    and isinstance(error, InstrumentSafetyError)
                ):
                    raise
                if command_unconfirmed:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                if not getattr(error, "mdt_fault_published", False):
                    self._publish_operation_fault(context, error)
                raise

    def _run_unequal_all_voltage_ramp(
        self,
        *,
        generation: int,
        capabilities: tuple[str, ...],
        target_v: float,
        validator: Callable[
            [MDTStatus, DriverState, dict[Axis, float], float], None
        ],
    ) -> MDTStatus:
        """Safely converge unequal bases before the final simultaneous command."""
        with self._request_lock:
            status, origin_state, axis_commands, master_voltage = self._revalidate_motion_start(
                generation, capabilities, validator
            )
            status, axis_commands, master_voltage = self._fresh_absolute_motion_start(
                generation, axis_commands, master_voltage
            )
            starts = dict(axis_commands)
            maximum_delta = max(abs(target_v - starts[axis]) for axis in Axis)
            steps = self._motion_steps(maximum_delta, self.ramp_step_v)
            previous_actual = {
                axis: status.axes[axis].actual_v for axis in Axis
            }
            residuals = self._motion_residuals(
                previous_actual,
                axis_commands,
                master_voltage,
                status.master_scan_enabled,
            )
            command_unconfirmed = False
            try:
                for index in range(1, steps + 1):
                    self._require_motion_generation(generation)
                    next_commands = {
                        axis: self._interpolated(
                            starts[axis], target_v, index, steps
                        )
                        for axis in Axis
                    }
                    expected_actual = self._expected_motion_actuals(
                        residuals,
                        next_commands,
                        master_voltage,
                        status.master_scan_enabled,
                    )
                    command_unconfirmed = True
                    if index == steps:
                        command = f"allvoltage={self._wire_number(target_v)}"
                        self._empty_response(self._query_unlocked(command), command)
                    else:
                        for axis in Axis:
                            value = next_commands[axis]
                            command = (
                                f"{axis.value}voltage={self._wire_number(value)}"
                            )
                            self._empty_response(
                                self._query_unlocked(command), command
                            )
                            self._require_motion_generation(generation)
                    self._require_motion_generation(generation)
                    actual = self._read_all_actual_unlocked()
                    try:
                        self._verify_actual_transition(previous_actual, actual)
                        self._verify_reduction_transition(
                            status, origin_state, previous_actual, actual
                        )
                        self._verify_actual_safety(status, previous_actual, actual)
                        self._verify_commanded_effect(expected_actual, actual)
                    except BaseException:
                        self._publish_motion_step(generation, actual)
                        raise
                    self._publish_motion_step(
                        generation,
                        actual,
                        axis_commands=next_commands,
                        confirmed_output_change=True,
                    )
                    command_unconfirmed = False
                    axis_commands = next_commands
                    previous_actual = actual
                    residuals = self._motion_residuals(
                        actual,
                        axis_commands,
                        master_voltage,
                        status.master_scan_enabled,
                    )
                    if index != steps:
                        self._sleep(self.ramp_interval_s)
                        self._require_motion_generation(generation)
                        previous_actual = self._revalidate_ramp_interval_actual(
                            generation, previous_actual
                        )
                        residuals = self._motion_residuals(
                            previous_actual,
                            axis_commands,
                            master_voltage,
                            status.master_scan_enabled,
                        )
                return self._complete_motion(generation, origin_state)
            except _MDTOperationCancelled:
                if command_unconfirmed:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                raise
            except BaseException as error:
                if command_unconfirmed:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                if not getattr(error, "mdt_fault_published", False):
                    self._publish_operation_fault("allvoltage=", error)
                raise

    def get_axis_voltage(self, axis: Axis) -> float:
        selected = self._axis_value(axis)
        query = f"{selected.value}voltage?"
        with self._getter_request(query) as snapshot:
            value = self._parse_number(self._query_unlocked(query), query)
            axes = dict(snapshot.axes)
            axes[selected] = AxisState(
                value,
                snapshot.axes[selected].minimum_v,
                snapshot.axes[selected].maximum_v,
            )
            self._publish_request_success(
                snapshot,
                fault_context=query,
                actual_observation={selected: value},
                axes=axes,
            )
            return value

    def set_axis_voltage(self, axis: Axis, target_v: float) -> MDTStatus:
        selected = self._axis_value(axis)
        target = self._motion_value(target_v, f"{selected.name} voltage target")
        capabilities = (
            f"{selected.value}voltage=", "msenable?", "msvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        )

        def validate(
            status: MDTStatus,
            state: DriverState,
            commands: dict[Axis, float],
            master: float,
        ) -> None:
            targets = dict(commands)
            targets[selected] = target
            self._validate_motion_outputs(
                status, state, commands, master, targets, master,
                status.master_scan_enabled, frozenset({selected}),
            )

        status, state, generation, commands, master = self._motion_preflight_snapshot(
            *capabilities
        )
        validate(status, state, commands, master)
        return self._run_ramp(
            generation=generation,
            capabilities=capabilities,
            context=f"{selected.value}voltage=",
            validator=validate,
            command_for_step=lambda value: (
                f"{selected.value}voltage={self._wire_number(value)}"
            ),
            start_for=lambda axis_commands, _: axis_commands[selected],
            target_v=target,
            axis_commands_for_step=lambda axis_commands, value: {
                **axis_commands, selected: value,
            },
            master_for_step=lambda _: None,
            absolute_axis_operation=True,
        )

    def get_all_voltages(self) -> Mapping[Axis, float]:
        queries = tuple(f"{axis.value}voltage?" for axis in Axis)
        with self._getter_request(*queries) as snapshot:
            actual = self._read_all_actual_unlocked()
            axes = {
                axis: AxisState(
                    actual[axis],
                    snapshot.axes[axis].minimum_v,
                    snapshot.axes[axis].maximum_v,
                )
                for axis in Axis
            }
            self._publish_request_success(
                snapshot,
                fault_context="all voltages",
                actual_observation=actual,
                axes=axes,
            )
            return MappingProxyType(dict(actual))

    def adopt_current_axis_baseline(
        self, *, confirm: bool = False
    ) -> MDTStatus:
        """Record an explicitly operator-attested query-only base baseline."""
        self._require_confirmation(confirm, "axis baseline adoption")
        capabilities = (
            "msenable?", "msvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        )
        with self._connected_request(*capabilities) as snapshot:
            enabled = self._parse_bool(
                self._query_unlocked("msenable?"), "msenable?"
            )
            master_voltage = self._parse_number(
                self._query_unlocked("msvoltage?"), "msvoltage?"
            )
            if enabled or not math.isclose(
                master_voltage,
                0.0,
                rel_tol=0.0,
                abs_tol=self._MOTION_CONFIRMATION_TOLERANCE_V,
            ):
                raise InstrumentSafetyError(
                    "Axis baseline adoption requires freshly confirmed disabled "
                    "Master Scan with programmed voltage zero"
                )
            actual = self._read_all_actual_unlocked()
            axes = {
                axis: AxisState(
                    actual[axis],
                    snapshot.axes[axis].minimum_v,
                    snapshot.axes[axis].maximum_v,
                )
                for axis in Axis
            }
            refreshed = replace(
                snapshot,
                axes=axes,
                master_scan_enabled=False,
                master_scan_voltage_v=master_voltage,
                observed_at=self._clock(),
            )
            restricted, restriction_evidence = self._classify_motion_actuals(
                refreshed, actual
            )
            evidence = AXIS_BASELINE_ATTESTATION_EVIDENCE
            with self._lifecycle_condition:
                if (
                    self.state is not DriverState.READY
                    or self.status is not snapshot
                ):
                    raise _MDTOperationCancelled(
                        "MDT baseline adoption was cancelled during publication"
                    )
                self.status = replace(
                    refreshed,
                    restricted=restricted,
                    fault_evidence=(
                        restriction_evidence
                        if restricted
                        else refreshed.fault_evidence
                    ),
                )
                self._master_scan_command_v = master_voltage
                self._motion_controls_confirmed = True
                if restricted:
                    self._restriction_evidence = restriction_evidence
                    self._transition_axis_authority_unlocked(False)
                    raise InstrumentSafetyError(
                        "Axis baseline adoption is blocked while a live output "
                        "violates its effective safety interval"
                    )
                self._transition_axis_authority_unlocked(
                    True,
                    commands=actual,
                    actual=actual,
                    evidence=evidence,
                    publish_evidence=True,
                )
                assert self.status is not None
                return self.status

    def set_all_voltages(
        self, target_v: float, *, confirm: bool = False
    ) -> MDTStatus:
        self._require_confirmation(confirm, "simultaneous all-voltage motion")
        target = self._motion_value(target_v, "all-voltage target")
        direct_capabilities = (
            "allvoltage=", "msenable?", "msvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        )

        def validate(
            status: MDTStatus,
            state: DriverState,
            commands: dict[Axis, float],
            master: float,
        ) -> None:
            targets = {axis: target for axis in Axis}
            self._validate_motion_outputs(
                status, state, commands, master, targets, master,
                status.master_scan_enabled, frozenset(Axis),
            )

        status, state, generation, commands, master = self._motion_preflight_snapshot(
            *direct_capabilities
        )
        validate(status, state, commands, master)
        unequal = max(commands.values()) - min(commands.values()) > 1e-9
        if unequal:
            capabilities = direct_capabilities + tuple(
                f"{axis.value}voltage=" for axis in Axis
            )
            status, state, generation, commands, master = self._motion_preflight_snapshot(
                *capabilities
            )
            validate(status, state, commands, master)
            return self._run_unequal_all_voltage_ramp(
                generation=generation,
                capabilities=capabilities,
                target_v=target,
                validator=validate,
            )
        return self._run_ramp(
            generation=generation,
            capabilities=direct_capabilities,
            context="allvoltage=",
            validator=validate,
            command_for_step=lambda value: f"allvoltage={self._wire_number(value)}",
            start_for=lambda axis_commands, _: next(iter(axis_commands.values())),
            target_v=target,
            axis_commands_for_step=lambda _, value: {axis: value for axis in Axis},
            master_for_step=lambda _: None,
            absolute_axis_operation=True,
        )

    def get_master_scan_enabled(self) -> bool:
        with self._getter_request("msenable?") as snapshot:
            enabled = self._parse_bool(self._query_unlocked("msenable?"), "msenable?")
            self._publish_request_success(
                snapshot,
                fault_context="msenable?",
                master_scan_enabled=enabled,
            )
            return enabled

    def set_master_scan_enabled(
        self, enabled: bool, *, confirm: bool = False
    ) -> MDTStatus:
        self._require_confirmation(confirm, "Master Scan enable change")
        expected = _boolean(enabled, "Master Scan enabled")
        capabilities = (
            "msenable=", "msenable?", "msvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        )

        def validate_static(
            status: MDTStatus,
            state: DriverState,
            commands: dict[Axis, float],
            master: float,
        ) -> None:
            # The zero-contribution gate is intentionally deferred until the
            # property has been freshly queried under the request lock.
            return None

        status, state, generation, commands, master = self._motion_preflight_snapshot(
            *capabilities
        )
        validate_static(status, state, commands, master)
        with self._request_lock:
            status, origin_state, commands, master = self._revalidate_motion_start(
                generation, capabilities, validate_static
            )
            previous_actual = {axis: status.axes[axis].actual_v for axis in Axis}
            command = f"msenable={int(expected)}"
            command_unconfirmed = False
            try:
                fresh_master = self._parse_number(
                    self._query_unlocked("msvoltage?"), "msvoltage?"
                )
                self._require_motion_generation(generation)
                self._publish_motion_step(
                    generation, previous_actual, master_voltage=fresh_master
                )
                master = fresh_master
                if abs(fresh_master) > 1e-9:
                    self._complete_motion(generation, origin_state)
                    raise InstrumentSafetyError(
                        "Master Scan enable may change only at zero freshly "
                        "confirmed contribution"
                    )
                fresh_actual = self._read_all_actual_unlocked()
                self._require_motion_generation(generation)
                status = self._require_prewrite_actual_authority(
                    generation, fresh_actual, context="Master Scan enable change"
                )
                restricted, evidence = self._classify_motion_actuals(
                    status, fresh_actual
                )
                status = self._publish_motion_step(
                    generation,
                    fresh_actual,
                    master_voltage=fresh_master,
                    restricted=restricted,
                    fault_evidence=evidence,
                )
                previous_actual = fresh_actual
                residuals = self._motion_residuals(
                    previous_actual,
                    commands,
                    fresh_master,
                    status.master_scan_enabled,
                )
                try:
                    self._validate_motion_outputs(
                        status,
                        origin_state,
                        commands,
                        fresh_master,
                        commands,
                        fresh_master,
                        expected,
                        frozenset(Axis),
                        residuals=residuals,
                    )
                except InstrumentSafetyError:
                    self._complete_motion(generation, origin_state)
                    raise
                expected_actual = self._expected_motion_actuals(
                    residuals, commands, fresh_master, expected
                )
                command_unconfirmed = True
                self._empty_response(self._query_unlocked(command), command)
                self._require_motion_generation(generation)
                observed_enabled = self._parse_bool(
                    self._query_unlocked("msenable?"), "msenable?"
                )
                self._require_motion_generation(generation)
                actual = self._read_all_actual_unlocked()
                try:
                    self._verify_actual_transition(previous_actual, actual)
                    self._verify_reduction_transition(
                        status, origin_state, previous_actual, actual
                    )
                    self._verify_actual_safety(status, previous_actual, actual)
                    self._verify_commanded_effect(expected_actual, actual)
                except BaseException:
                    self._publish_motion_step(
                        generation,
                        actual,
                        master_voltage=fresh_master,
                        master_enabled=observed_enabled,
                    )
                    raise
                self._publish_motion_step(
                    generation,
                    actual,
                    master_voltage=fresh_master,
                    master_enabled=observed_enabled,
                    confirmed_output_change=observed_enabled is expected,
                )
                if observed_enabled is not expected:
                    raise DeviceFault(
                        "MDT msenable? readback did not confirm requested "
                        f"state {int(expected)}"
                    )
                command_unconfirmed = False
                return self._complete_motion(generation, origin_state)
            except _MDTOperationCancelled:
                if command_unconfirmed:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                raise
            except BaseException as error:
                with self._lifecycle_lock:
                    active = self.state is DriverState.ACTIVE
                if (
                    not active
                    and not command_unconfirmed
                    and isinstance(error, InstrumentSafetyError)
                ):
                    raise
                if command_unconfirmed:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                if not getattr(error, "mdt_fault_published", False):
                    self._publish_operation_fault("msenable=", error)
                raise

    def get_master_scan_voltage(self) -> float:
        with self._getter_request("msvoltage?") as snapshot:
            value = self._parse_number(
                self._query_unlocked("msvoltage?"), "msvoltage?"
            )
            published = self._publish_request_success(
                snapshot,
                fault_context="msvoltage?",
                master_scan_voltage_v=value,
            )
            with self._lifecycle_lock:
                if self.status is not published:
                    raise _MDTOperationCancelled(
                        "MDT693B Master Scan query was cancelled during publication"
                    )
                self._master_scan_command_v = value
            return value

    def set_master_scan_voltage(
        self, target_v: float, *, confirm: bool = False
    ) -> MDTStatus:
        self._require_confirmation(confirm, "Master Scan voltage motion")
        target = self._motion_value(target_v, "Master Scan voltage target")
        capabilities = (
            "msvoltage=", "msenable?", "msvoltage?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        )

        def validate_static(
            status: MDTStatus,
            state: DriverState,
            commands: dict[Axis, float],
            master: float,
        ) -> None:
            # Enable state, current contribution, XYZ, and residuals are
            # dynamic and are validated only after locked fresh queries.
            return None

        status, state, generation, commands, master = self._motion_preflight_snapshot(
            *capabilities
        )
        validate_static(status, state, commands, master)

        def refresh_start(
            status: MDTStatus,
            origin_state: DriverState,
            axis_commands: dict[Axis, float],
            _: float,
            generation: int,
        ) -> tuple[MDTStatus, dict[Axis, float], float]:
            enabled = self._parse_bool(
                self._query_unlocked("msenable?"), "msenable?"
            )
            self._require_motion_generation(generation)
            fresh_master = self._parse_number(
                self._query_unlocked("msvoltage?"), "msvoltage?"
            )
            self._require_motion_generation(generation)
            if not enabled:
                self._publish_motion_step(
                    generation,
                    {axis: status.axes[axis].actual_v for axis in Axis},
                    master_voltage=fresh_master,
                    master_enabled=False,
                )
                self._complete_motion(generation, origin_state)
                raise InstrumentSafetyError(
                    "Master Scan voltage may ramp only while freshly confirmed enabled"
                )
            actual = self._read_all_actual_unlocked()
            self._require_motion_generation(generation)
            status = self._require_prewrite_actual_authority(
                generation, actual, context="Master Scan voltage motion"
            )
            restricted, evidence = self._classify_motion_actuals(status, actual)
            published = self._publish_motion_step(
                generation,
                actual,
                master_voltage=fresh_master,
                master_enabled=True,
                restricted=restricted,
                fault_evidence=evidence,
            )
            residuals = self._motion_residuals(
                actual, axis_commands, fresh_master, True
            )
            try:
                self._validate_motion_outputs(
                    published,
                    origin_state,
                    axis_commands,
                    fresh_master,
                    axis_commands,
                    target,
                    True,
                    frozenset(Axis),
                    changes_master_voltage=True,
                    residuals=residuals,
                )
            except InstrumentSafetyError:
                self._complete_motion(generation, origin_state)
                raise
            return published, axis_commands, fresh_master

        return self._run_ramp(
            generation=generation,
            capabilities=capabilities,
            context="msvoltage=",
            validator=validate_static,
            command_for_step=lambda value: f"msvoltage={self._wire_number(value)}",
            start_for=lambda _, master_voltage: master_voltage,
            target_v=target,
            axis_commands_for_step=lambda _, __: None,
            master_for_step=lambda value: value,
            refresh_start=refresh_start,
            confirm_master_property=True,
        )

    def _require_arrow_candidate(self, candidate: bytes) -> None:
        with self._lifecycle_lock:
            self._require_no_close_intent_unlocked()
            if not self._motion_controls_confirmed:
                raise InstrumentSafetyError(
                    "MDT arrow controls are blocked because motion certainty is lost"
                )
            if self._arrow_capabilities[candidate] is False:
                raise InstrumentCapabilityError(
                    "MDT arrow candidate is latched unsupported for this session"
                )

    def _set_arrow_capability(self, candidate: bytes, supported: bool) -> None:
        with self._lifecycle_lock:
            self._arrow_capabilities[candidate] = supported

    @classmethod
    def _parse_selected_channel(
        cls, lines: tuple[str, ...], context: str
    ) -> str:
        text = cls._single_response(lines, context).strip().upper()
        if text not in {"X", "Y", "Z"}:
            raise InstrumentCapabilityError(
                f"MDT {context} returned an unrecognized selected-channel response"
            )
        return text

    def _transact_arrow_unlocked(self, candidate: bytes) -> tuple[str, ...]:
        protocol = self._protocol
        if protocol is None:
            raise InstrumentConnectionError("MDT serial protocol is not connected")
        return protocol.transact_arrow(candidate)

    def _arrow_preflight(
        self, candidate: bytes, capability: str, *, motion: bool
    ) -> tuple[MDTStatus, DriverState, int, dict[Axis, float], float]:
        self._require_arrow_candidate(candidate)
        capabilities = (
            (capability, "dacstep?", "vlimit?", "xvoltage?", "yvoltage?", "zvoltage?")
            if motion
            else (capability, "xvoltage?", "yvoltage?", "zvoltage?")
        )
        return self._motion_preflight_snapshot(
            *capabilities, require_axis_authority=motion
        )

    def _translate_arrow_framing_failure(
        self, candidate: bytes, context: str, error: BaseException
    ) -> BaseException:
        if isinstance(error, InstrumentProtocolError):
            self._set_arrow_capability(candidate, False)
            translated = InstrumentCapabilityError(
                f"MDT {context} arrow candidate did not produce valid acknowledged framing"
            )
            translated.__cause__ = error
            return translated
        return error

    def _select_channel(self, candidate: bytes, capability: str) -> str:
        self._arrow_preflight(candidate, capability, motion=False)
        capabilities = (
            capability, "xvoltage?", "yvoltage?", "zvoltage?",
        )
        with self._request_lock:
            attempted = False
            generation = -1
            try:
                _, _, generation, _, _ = self._arrow_preflight(
                    candidate, capability, motion=False
                )
                status, origin_state, _, _ = self._revalidate_motion_start(
                    generation,
                    capabilities,
                    lambda *args: None,
                    require_axis_authority=False,
                )
                before = self._read_all_actual_unlocked()
                self._require_motion_generation(generation)
                attempted = True
                try:
                    lines = self._transact_arrow_unlocked(candidate)
                except _MDTArrowRejected:
                    attempted = False
                    self._set_arrow_capability(candidate, False)
                    self._complete_motion(generation, origin_state)
                    raise
                selection_error: InstrumentCapabilityError | None = None
                selected = ""
                try:
                    selected = self._parse_selected_channel(lines, capability)
                except (InstrumentProtocolError, InstrumentCapabilityError) as error:
                    selection_error = InstrumentCapabilityError(
                        f"MDT {capability} returned an unrecognized selected-channel response"
                    )
                    selection_error.__cause__ = error
                self._require_motion_generation(generation)
                after = self._read_all_actual_unlocked()
                if selection_error is not None:
                    self._publish_motion_step(generation, after)
                    raise selection_error
                self._set_arrow_capability(candidate, True)
                tolerance = self._MOTION_CONFIRMATION_TOLERANCE_V
                moved = tuple(
                    axis for axis in Axis
                    if abs(after[axis] - before[axis]) > tolerance
                )
                if moved:
                    self._publish_motion_step(generation, after)
                    names = ", ".join(axis.name for axis in moved)
                    raise DeviceFault(
                        f"MDT {capability} changed output axis/axes {names}"
                    )
                self._publish_motion_step(generation, after)
                attempted = False
                self._complete_motion(generation, origin_state)
                return selected
            except _MDTArrowRejected:
                raise
            except _MDTOperationCancelled:
                if attempted:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                raise
            except BaseException as error:
                if attempted and isinstance(error, InstrumentCapabilityError):
                    self._set_arrow_capability(candidate, False)
                translated = self._translate_arrow_framing_failure(
                    candidate, capability, error
                ) if attempted else error
                if attempted:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                if not getattr(translated, "mdt_fault_published", False):
                    self._publish_operation_fault(capability, translated)
                if translated is error:
                    raise
                raise translated from error

    @staticmethod
    def _arrow_candidate_violation(
        value: float, minimum: float, maximum: float
    ) -> tuple[float, float]:
        return max(minimum - value, 0.0), max(value - maximum, 0.0)

    def _validate_arrow_motion_start(
        self,
        status: MDTStatus,
        origin_state: DriverState,
        actual: Mapping[Axis, float],
        step_v: float,
        direction: int,
    ) -> None:
        reduction_only = status.restricted or origin_state is DriverState.FAULT
        if reduction_only:
            if not self._motion_controls_confirmed:
                raise InstrumentSafetyError(
                    "FAULT/restricted arrow motion is blocked because controls are uncertain"
                )
            if direction > 0:
                raise InstrumentSafetyError(
                    "Increment is not a provable reduction from restricted/FAULT state"
                )
        for axis in Axis:
            axis_state = status.axes[axis]
            ceiling = min(
                75.0,
                status.hardware_limit.volts,
                self.application_limits_v[axis],
                axis_state.maximum_v,
            )
            candidate = actual[axis] + direction * step_v
            current_low, current_high = self._arrow_candidate_violation(
                actual[axis], axis_state.minimum_v, ceiling
            )
            target_low, target_high = self._arrow_candidate_violation(
                candidate, axis_state.minimum_v, ceiling
            )
            if not reduction_only:
                if target_low > 0.0 or target_high > 0.0:
                    raise InstrumentSafetyError(
                        f"{axis.name} could leave its safe interval during arrow motion"
                    )
                continue
            if current_low > 0.0 and target_high > 0.0:
                raise InstrumentSafetyError(
                    f"{axis.name} arrow motion could cross to the opposite bound side"
                )
            if current_high > 0.0 and target_low > 0.0:
                raise InstrumentSafetyError(
                    f"{axis.name} arrow motion could cross to the opposite bound side"
                )
            current_violation = max(current_low, current_high)
            target_violation = max(target_low, target_high)
            if current_violation > 0.0:
                if target_violation >= current_violation:
                    raise InstrumentSafetyError(
                        f"{axis.name} arrow motion is not a strict violation reduction"
                    )
            elif target_violation > 0.0:
                raise InstrumentSafetyError(
                    f"{axis.name} could leave its safe interval during arrow motion"
                )

    def _run_selected_increment(
        self, candidate: bytes, capability: str, direction: int
    ) -> MDTStatus:
        self._arrow_preflight(candidate, capability, motion=True)
        capabilities = (
            capability, "dacstep?", "vlimit?",
            "xvoltage?", "yvoltage?", "zvoltage?",
        )
        with self._request_lock:
            attempted = False
            generation = -1
            try:
                _, _, generation, _, _ = self._arrow_preflight(
                    candidate, capability, motion=True
                )
                status, origin_state, axis_commands, master_voltage = (
                    self._revalidate_motion_start(
                        generation, capabilities, lambda *args: None
                    )
                )
                dac_step = self._parse_int(
                    self._query_unlocked("dacstep?"), "dacstep?"
                )
                if not 1 <= dac_step <= 1000:
                    raise InstrumentProtocolError(
                        "MDT dacstep? response is outside 1..1000"
                    )
                hardware_limit = self._parse_hardware_limit(
                    self._query_unlocked("vlimit?")
                )
                before = self._read_all_actual_unlocked()
                self._require_motion_generation(generation)
                status = self._require_prewrite_actual_authority(
                    generation, before, context="arrow motion"
                )
                fresh_axes = {
                    axis: AxisState(
                        before[axis],
                        status.axes[axis].minimum_v,
                        status.axes[axis].maximum_v,
                    )
                    for axis in Axis
                }
                fresh = replace(
                    status,
                    axes=fresh_axes,
                    hardware_limit=hardware_limit,
                    dac_step=dac_step,
                )
                restricted, evidence = self._classify_motion_actuals(fresh, before)
                fresh = replace(
                    fresh, restricted=restricted, fault_evidence=evidence
                )
                step_v = dac_step * hardware_limit.volts / 65535.0
                try:
                    if step_v > 0.1:
                        raise InstrumentSafetyError(
                            f"MDT conservative DAC arrow step {step_v:g} V exceeds 0.1 V"
                        )
                    self._validate_arrow_motion_start(
                        fresh, origin_state, before, step_v, direction
                    )
                except InstrumentSafetyError:
                    self._publish_motion_step(
                        generation,
                        before,
                        hardware_limit=hardware_limit,
                        dac_step=dac_step,
                        restricted=restricted,
                        fault_evidence=evidence,
                    )
                    self._complete_motion(generation, origin_state)
                    raise

                now = self._clock()
                last = self._last_arrow_motion_at
                if last is not None:
                    remaining = 0.050 - (now - last)
                    if remaining > 0.0:
                        self._sleep(remaining)
                        self._require_motion_generation(generation)
                self._last_arrow_motion_at = self._clock()
                attempted = True
                try:
                    lines = self._transact_arrow_unlocked(candidate)
                except _MDTArrowRejected:
                    attempted = False
                    self._set_arrow_capability(candidate, False)
                    self._complete_motion(generation, origin_state)
                    raise
                selection_error: InstrumentCapabilityError | None = None
                selected_name: str | None = None
                if lines:
                    try:
                        selected_name = self._parse_selected_channel(lines, capability)
                    except (InstrumentProtocolError, InstrumentCapabilityError) as error:
                        selection_error = InstrumentCapabilityError(
                            f"MDT {capability} returned an unrecognized selected-channel response"
                        )
                        selection_error.__cause__ = error
                self._require_motion_generation(generation)
                after = self._read_all_actual_unlocked()
                tolerance = self._MOTION_CONFIRMATION_TOLERANCE_V
                if selection_error is not None:
                    post_restricted, post_evidence = self._classify_motion_actuals(
                        fresh, after
                    )
                    self._publish_motion_step(
                        generation,
                        after,
                        hardware_limit=hardware_limit,
                        dac_step=dac_step,
                        restricted=post_restricted,
                        fault_evidence=post_evidence,
                    )
                    raise selection_error
                self._set_arrow_capability(candidate, True)
                changed_axes = tuple(
                    axis for axis in Axis
                    if abs(after[axis] - before[axis]) > tolerance
                )
                if selected_name is None:
                    if len(changed_axes) != 1:
                        post_restricted, post_evidence = self._classify_motion_actuals(
                            fresh, after
                        )
                        self._publish_motion_step(
                            generation,
                            after,
                            hardware_limit=hardware_limit,
                            dac_step=dac_step,
                            restricted=post_restricted,
                            fault_evidence=post_evidence,
                        )
                        raise DeviceFault(
                            f"MDT {capability} prompt-only acknowledgement did not identify "
                            "exactly one changed axis"
                        )
                    selected = changed_axes[0]
                else:
                    selected = Axis(selected_name.lower())
                delta = after[selected] - before[selected]
                try:
                    self._verify_actual_transition(before, after)
                    self._verify_reduction_transition(
                        fresh, origin_state, before, after
                    )
                    self._verify_actual_safety(fresh, before, after)
                    if direction * delta <= tolerance:
                        raise DeviceFault(
                            f"MDT {capability} did not move {selected.name} in the requested direction"
                        )
                    if not math.isclose(
                        abs(delta), step_v, rel_tol=0.0, abs_tol=tolerance
                    ):
                        raise DeviceFault(
                            f"MDT {capability} moved {selected.name} by {abs(delta):g} V; "
                            f"expected the fresh DAC-derived step {step_v:g} V"
                        )
                    if abs(delta) > 0.1 + tolerance:
                        raise DeviceFault(
                            f"MDT {capability} moved {selected.name} by {abs(delta):g} V"
                        )
                    for axis in Axis:
                        if axis is not selected and abs(after[axis] - before[axis]) > tolerance:
                            raise DeviceFault(
                                f"MDT {capability} changed unselected axis {axis.name}"
                            )
                except BaseException:
                    post_restricted, post_evidence = self._classify_motion_actuals(
                        fresh, after
                    )
                    self._publish_motion_step(
                        generation,
                        after,
                        hardware_limit=hardware_limit,
                        dac_step=dac_step,
                        restricted=post_restricted,
                        fault_evidence=post_evidence,
                    )
                    raise
                next_commands = dict(axis_commands)
                next_commands[selected] += delta
                post_restricted, post_evidence = self._classify_motion_actuals(
                    fresh, after
                )
                self._publish_motion_step(
                    generation,
                    after,
                    axis_commands=next_commands,
                    master_voltage=master_voltage,
                    hardware_limit=hardware_limit,
                    dac_step=dac_step,
                    restricted=post_restricted,
                    fault_evidence=post_evidence,
                    confirmed_output_change=True,
                )
                attempted = False
                return self._complete_motion(generation, origin_state)
            except _MDTArrowRejected:
                raise
            except _MDTOperationCancelled:
                if attempted:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                raise
            except BaseException as error:
                if attempted and isinstance(error, InstrumentCapabilityError):
                    self._set_arrow_capability(candidate, False)
                translated = self._translate_arrow_framing_failure(
                    candidate, capability, error
                ) if attempted else error
                if attempted:
                    with self._lifecycle_lock:
                        self._motion_controls_confirmed = False
                with self._lifecycle_lock:
                    active = self.state is DriverState.ACTIVE
                if not active and not attempted and isinstance(
                    translated, InstrumentSafetyError
                ):
                    raise
                if not getattr(translated, "mdt_fault_published", False):
                    self._publish_operation_fault(capability, translated)
                if translated is error:
                    raise
                raise translated from error

    def select_previous_channel(self) -> str:
        return self._select_channel(_ARROW_LEFT, "left")

    def select_next_channel(self) -> str:
        return self._select_channel(_ARROW_RIGHT, "right")

    def increment_selected(self) -> MDTStatus:
        return self._run_selected_increment(_ARROW_UP, "up", 1)

    def decrement_selected(self) -> MDTStatus:
        return self._run_selected_increment(_ARROW_DOWN, "down", -1)

    def get_axis_state(self, axis: Axis) -> AxisState:
        if not isinstance(axis, Axis):
            raise InstrumentProtocolError("axis must be an Axis value")
        voltage_query = f"{axis.value}voltage?"
        minimum_query = f"{axis.value}min?"
        maximum_query = f"{axis.value}max?"
        with self._getter_request(voltage_query, minimum_query, maximum_query) as snapshot:
            actual = self._parse_number(self._query_unlocked(voltage_query), voltage_query)
            minimum = self._parse_number(self._query_unlocked(minimum_query), minimum_query)
            maximum = self._parse_number(self._query_unlocked(maximum_query), maximum_query)
            if minimum > maximum:
                raise InstrumentProtocolError(f"MDT {axis.name} minimum exceeds its maximum")
            value = AxisState(actual, minimum, maximum)
            axes = dict(snapshot.axes)
            axes[axis] = value
            self._publish_request_success(
                snapshot,
                fault_context=voltage_query,
                actual_observation={axis: actual},
                axes=axes,
            )
            return value

    def _set_axis_limit(self, axis: Axis, value_v: float, *, minimum: bool) -> AxisState:
        if not isinstance(axis, Axis):
            raise InstrumentProtocolError("axis must be an Axis value")
        value = self._setter_float(value_v, f"{axis.name} axis limit")
        suffix = "min" if minimum else "max"
        setter_capability = f"{axis.value}{suffix}="
        query = f"{axis.value}{suffix}?"
        with self._connected_request(setter_capability, query) as snapshot:
            previous_axis = snapshot.axes[axis]
            ceiling = min(
                snapshot.hardware_limit.volts, 75.0, self.application_limits_v[axis]
            )
            if not 0.0 <= value <= ceiling:
                raise InstrumentProtocolError(
                    f"{axis.name} axis limit must be within 0..{ceiling:g} V"
                )
            if minimum:
                if value > previous_axis.maximum_v:
                    raise InstrumentProtocolError(
                        f"{axis.name} minimum cannot exceed its maximum"
                    )
                if value > previous_axis.actual_v:
                    raise InstrumentProtocolError(
                        f"{axis.name} minimum cannot exclude current actual voltage"
                    )
            else:
                if value < previous_axis.minimum_v:
                    raise InstrumentProtocolError(
                        f"{axis.name} maximum cannot be below its minimum"
                    )
                if value < previous_axis.actual_v:
                    raise InstrumentProtocolError(
                        f"{axis.name} maximum cannot exclude current actual voltage"
                    )
            command = f"{axis.value}{suffix}={self._wire_number(value)}"
            try:
                self._empty_response(self._query_unlocked(command), command)
                observed = self._parse_number(self._query_unlocked(query), query)
                if not math.isclose(observed, value, rel_tol=1e-9, abs_tol=1e-9):
                    raise DeviceFault(
                        f"MDT {query} readback {observed!r} did not confirm {value!r}"
                    )
                confirmed_axis = AxisState(
                    previous_axis.actual_v,
                    observed if minimum else previous_axis.minimum_v,
                    previous_axis.maximum_v if minimum else observed,
                )
                axes = dict(snapshot.axes)
                axes[axis] = confirmed_axis
                self._publish_request_success(
                    snapshot,
                    fault_context=command,
                    axes=axes,
                    fault_evidence=snapshot.fault_evidence,
                )
            except BaseException as error:
                if (
                    not isinstance(error, _MDTOperationCancelled)
                    and not getattr(error, "mdt_fault_published", False)
                ):
                    self._publish_operation_fault(command, error)
                raise
            return confirmed_axis

    def set_axis_minimum(self, axis: Axis, value_v: float) -> AxisState:
        return self._set_axis_limit(axis, value_v, minimum=True)

    def set_axis_maximum(self, axis: Axis, value_v: float) -> AxisState:
        return self._set_axis_limit(axis, value_v, minimum=False)

    def get_dac_step(self) -> int:
        with self._getter_request("dacstep?") as snapshot:
            value = self._parse_int(self._query_unlocked("dacstep?"), "dacstep?")
            if not 1 <= value <= 1000:
                raise InstrumentProtocolError("MDT dacstep? response is outside 1..1000")
            self._publish_request_success(
                snapshot, fault_context="dacstep?", dac_step=value
            )
            return value

    def set_dac_step(self, value: int) -> int:
        expected = self._setter_integer(value, "DAC step", 1, 1000)
        result = self._confirmed_scalar_setter(
            setter_capability="dacstep=", query="dacstep?",
            command=f"dacstep={expected}", expected=expected,
            parser=self._parse_int, field="dac_step",
        )
        return int(result)

    def get_compatibility_enabled(self) -> bool:
        with self._getter_request("cm?") as snapshot:
            value = self._parse_bool(self._query_unlocked("cm?"), "cm?")
            self._publish_request_success(
                snapshot, fault_context="cm?", compatibility_enabled=value
            )
            return value

    def set_compatibility_enabled(self, enabled: bool) -> bool:
        expected = _boolean(enabled, "compatibility enabled")
        result = self._confirmed_scalar_setter(
            setter_capability="cm=", query="cm?", command=f"cm={int(expected)}",
            expected=expected, parser=self._parse_bool, field="compatibility_enabled",
        )
        return bool(result)

    def get_rotary_mode(self) -> RotaryMode:
        with self._getter_request("rotarymode?") as snapshot:
            raw = self._parse_int(self._query_unlocked("rotarymode?"), "rotarymode?")
            try:
                value = RotaryMode(raw)
            except ValueError as error:
                raise InstrumentProtocolError("MDT rotarymode? returned an unknown mode") from error
            self._publish_request_success(
                snapshot, fault_context="rotarymode?", rotary_mode=value
            )
            return value

    @classmethod
    def _parse_rotary_mode(cls, lines: tuple[str, ...], command: str) -> RotaryMode:
        raw = cls._parse_int(lines, command)
        try:
            return RotaryMode(raw)
        except ValueError as error:
            raise InstrumentProtocolError("MDT rotarymode? returned an unknown mode") from error

    def set_rotary_mode(self, mode: RotaryMode) -> RotaryMode:
        if not isinstance(mode, RotaryMode):
            raise InstrumentProtocolError("rotary mode must be a RotaryMode value")
        result = self._confirmed_scalar_setter(
            setter_capability="rotarymode=", query="rotarymode?",
            command=f"rotarymode={mode.value}", expected=mode,
            parser=self._parse_rotary_mode, field="rotary_mode",
        )
        assert isinstance(result, RotaryMode)
        return result

    def get_push_to_adjust_disabled(self) -> bool:
        with self._getter_request("pushdisable?") as snapshot:
            value = self._parse_bool(self._query_unlocked("pushdisable?"), "pushdisable?")
            self._publish_request_success(
                snapshot,
                fault_context="pushdisable?",
                push_to_adjust_disabled=value,
            )
            return value

    def set_push_to_adjust_disabled(self, disabled: bool) -> bool:
        expected = _boolean(disabled, "push-to-adjust disabled")
        result = self._confirmed_scalar_setter(
            setter_capability="pushdisable=", query="pushdisable?",
            command=f"pushdisable={int(expected)}", expected=expected,
            parser=self._parse_bool, field="push_to_adjust_disabled",
        )
        return bool(result)

    def _restore_limit_evidence(self, snapshot: MDTStatus) -> str | None:
        violations: list[str] = []
        for axis in Axis:
            axis_state = snapshot.axes[axis]
            ceiling = min(
                75.0,
                snapshot.hardware_limit.volts,
                self.application_limits_v[axis],
                axis_state.maximum_v,
            )
            if axis_state.actual_v < axis_state.minimum_v:
                violations.append(
                    f"{axis.name}={axis_state.actual_v:g} V below "
                    f"{axis_state.minimum_v:g} V"
                )
            elif axis_state.actual_v > ceiling:
                violations.append(
                    f"{axis.name}={axis_state.actual_v:g} V exceeds {ceiling:g} V"
                )
        if not violations:
            return None
        return "MDT restore limit observation: " + ", ".join(violations)

    def restore_factory_defaults(
        self, *, confirm: bool = False
    ) -> MDTStatus:
        self._require_confirmation(confirm, "factory restore")
        with self._lifecycle_lock:
            self._require_no_close_intent_unlocked()
            status = self.status
            if self._emergency_in_progress:
                raise _MDTOperationCancelled(
                    "MDT factory restore was cancelled by the active emergency owner"
                )
            if (
                self.state not in {
                    DriverState.READY, DriverState.ACTIVE, DriverState.FAULT,
                }
                or status is None
                or self._serial is None
                or self._protocol is None
            ):
                raise InstrumentConnectionError(
                    f"MDT693B factory restore requires a connected state, not {self.state.name}"
                )
            if "restore" not in status.supported_commands:
                raise InstrumentCapabilityError(
                    "MDT firmware does not advertise the restore command"
                )
            if self._restore_in_progress:
                raise InstrumentConnectionError("MDT factory restore is already active")
            self._operation_generation += 1
            generation = self._operation_generation
            self._restore_in_progress = True
            self.state = DriverState.ACTIVE
            self._transition_axis_authority_unlocked(False)

        attempted = False
        with self._high_impact_request_lock("restore"):
            try:
                self._require_motion_generation(generation)
                attempted = True
                with self._lifecycle_lock:
                    self._master_scan_command_v = None
                    self._motion_controls_confirmed = False
                protocol = self._protocol
                if protocol is None:
                    raise _MDTOperationCancelled(
                        "MDT factory restore lost its serial protocol"
                    )
                self._empty_response(
                    protocol.transact_echo_transition("restore"), "restore"
                )
                self._require_motion_generation(generation)
                snapshot = self._snapshot_from_queries(
                    lambda: self._require_motion_generation(generation)
                )
                evidence = self._restore_limit_evidence(snapshot)
                if evidence is not None:
                    snapshot = replace(
                        snapshot,
                        restricted=True,
                        fault_evidence=evidence,
                    )
                if evidence is None:
                    self._restart_terminal_monitor(generation)
                with self._lifecycle_lock:
                    if (
                        generation != self._operation_generation
                        or self.state is not DriverState.ACTIVE
                        or self._serial is None
                        or self._protocol is None
                    ):
                        raise _MDTOperationCancelled(
                            f"MDT factory restore was cancelled while state is {self.state.name}"
                        )
                    self.status = snapshot
                    self._restriction_evidence = evidence
                    self._transition_axis_authority_unlocked(False)
                    self._master_scan_command_v = snapshot.master_scan_voltage_v
                    self._motion_controls_confirmed = True
                    self._operation_generation += 1
                    self._restore_in_progress = False
                    self.state = (
                        DriverState.FAULT if evidence is not None
                        else DriverState.READY
                    )
                    assert self.status is not None
                    return self.status
            except BaseException as error:
                with self._lifecycle_lock:
                    self._restore_in_progress = False
                    if attempted:
                        self._motion_controls_confirmed = False
                if not getattr(error, "mdt_fault_published", False):
                    self._publish_operation_fault("restore", error)
                raise

    def _restart_terminal_monitor(
        self,
        generation: int,
        *,
        allowed_states: frozenset[DriverState] | None = None,
    ) -> None:
        """Replace a terminally exited monitor before publishing READY."""
        states = allowed_states or frozenset({DriverState.ACTIVE})
        with self._lifecycle_condition:
            self._require_no_close_intent_unlocked()
            restart_required = self._monitor_restart_required
            monitor = self._monitor_thread

        if not restart_required:
            try:
                monitor_alive = monitor is not None and monitor.is_alive()
            except BaseException:
                raise
            with self._lifecycle_condition:
                self._require_no_close_intent_unlocked()
                if (
                    generation != self._operation_generation
                    or self.state not in states
                    or self._serial is None
                    or self._protocol is None
                    or self._monitor_thread is not monitor
                    or not monitor_alive
                    or not self._monitor_started.is_set()
                    or self._monitor_exited.is_set()
                    or self._monitor_restart_required
                ):
                    raise DeviceFault(
                        "MDT cannot publish READY without one live monitor"
                    )
            return

        timeout = self.io_timeout + self.monitor_interval_s + 1.0
        if not self._monitor_exited.wait(timeout):
            raise DeviceFault("MDT terminal monitor did not finish before restart")

        with self._lifecycle_condition:
            self._require_no_close_intent_unlocked()
            if (
                generation != self._operation_generation
                or self.state not in states
                or self._serial is None
                or self._protocol is None
                or not self._monitor_restart_required
            ):
                raise _MDTOperationCancelled(
                    f"MDT monitor restart was cancelled while state is {self.state.name}"
                )
            self._monitor_started.clear()
            self._monitor_exited.clear()
            monitor = threading.Thread(
                target=self._monitor_main,
                args=(True,),
                name="MDT693BMonitor",
                daemon=True,
            )
            self._monitor_thread = monitor
            try:
                monitor.start()
            except BaseException:
                self._monitor_exited.set()
                raise

        if not self._monitor_started.wait(timeout):
            raise DeviceFault("MDT replacement monitor did not start")
        try:
            monitor_alive = monitor.is_alive()
        except BaseException:
            raise
        with self._lifecycle_condition:
            self._require_no_close_intent_unlocked()
            if (
                generation != self._operation_generation
                or self.state not in states
                or self._monitor_thread is not monitor
                or not monitor_alive
                or self._monitor_exited.is_set()
            ):
                raise _MDTOperationCancelled(
                    f"MDT replacement monitor was cancelled while state is {self.state.name}"
                )
            self._monitor_restart_required = False

    def _publish_recovery_actual_evidence(
        self, actual: Mapping[Axis, float], primary: BaseException
    ) -> str | None:
        """Retain a complete XYZ observation without confirming partial controls."""
        with self._lifecycle_lock:
            previous = self.status
            if previous is None:
                return None
            axes = {
                axis: AxisState(
                    actual[axis],
                    previous.axes[axis].minimum_v,
                    previous.axes[axis].maximum_v,
                )
                for axis in Axis
            }
            violations: list[str] = []
            for axis in Axis:
                state = axes[axis]
                ceiling = min(
                    75.0,
                    previous.hardware_limit.volts,
                    self.application_limits_v[axis],
                    state.maximum_v,
                )
                if state.actual_v < state.minimum_v:
                    violations.append(
                        f"{axis.name}={state.actual_v:g} V below {state.minimum_v:g} V"
                    )
                elif state.actual_v > ceiling:
                    violations.append(
                        f"{axis.name}={state.actual_v:g} V exceeds {ceiling:g} V"
                    )
            evidence = (
                "MDT recover partial XYZ observation: " + ", ".join(violations)
                if violations
                else None
            )
            observed_at = previous.observed_at
            try:
                observed_at = self._clock()
            except BaseException as publication_error:
                self._attach_cleanup_error(primary, publication_error)
            try:
                self.status = replace(
                    previous,
                    axes=axes,
                    restricted=previous.restricted or bool(violations),
                    fault_evidence=(
                        evidence if evidence is not None else previous.fault_evidence
                    ),
                    observed_at=observed_at,
                )
            except BaseException as publication_error:
                self._attach_cleanup_error(primary, publication_error)
            return evidence

    def recover(self) -> MDTStatus:
        """Re-establish one coherent read-only snapshot after a fault."""
        with self._lifecycle_condition:
            self._require_no_close_intent_unlocked()
            status = self.status
            if self._emergency_in_progress:
                raise _MDTOperationCancelled(
                    "MDT recovery was cancelled by the active emergency owner"
                )
            if (
                self.state not in {DriverState.READY, DriverState.FAULT}
                or status is None
                or self._serial is None
                or self._protocol is None
            ):
                raise InstrumentConnectionError(
                    f"MDT693B recovery requires a connected state, not {self.state.name}"
                )
            self._operation_generation += 1
            generation = self._operation_generation
            self._cancel_event.set()
            self.state = DriverState.ACTIVE
            self._transition_axis_authority_unlocked(False)
            self._lifecycle_condition.notify_all()

        observed_actual: dict[Axis, float] | None = None

        def capture_actual(actual: Mapping[Axis, float]) -> None:
            nonlocal observed_actual
            observed_actual = dict(actual)

        with self._high_impact_request_lock("recover"):
            try:
                self._require_motion_generation(generation)
                snapshot = self._snapshot_from_queries(
                    lambda: self._require_motion_generation(generation),
                    actual_observer=capture_actual,
                )
                evidence = self._restore_limit_evidence(snapshot)
                if evidence is not None:
                    snapshot = replace(
                        snapshot, restricted=True, fault_evidence=evidence
                    )
                self._restart_terminal_monitor(generation)
                with self._lifecycle_condition:
                    if (
                        generation != self._operation_generation
                        or self.state is not DriverState.ACTIVE
                        or self._serial is None
                        or self._protocol is None
                    ):
                        raise _MDTOperationCancelled(
                            f"MDT recovery was cancelled while state is {self.state.name}"
                        )
                    self.status = snapshot
                    self._restriction_evidence = (
                        snapshot.fault_evidence if snapshot.restricted else None
                    )
                    self._transition_axis_authority_unlocked(False)
                    self._master_scan_command_v = snapshot.master_scan_voltage_v
                    self._motion_controls_confirmed = True
                    self._monitor_failure_count = 0
                    self._operation_generation += 1
                    self.state = DriverState.READY
                    self._cancel_event.clear()
                    self._lifecycle_condition.notify_all()
                    assert self.status is not None
                    return self.status
            except BaseException as error:
                with self._lifecycle_lock:
                    self._motion_controls_confirmed = False
                evidence = (
                    None
                    if observed_actual is None
                    else self._publish_recovery_actual_evidence(
                        observed_actual, error
                    )
                )
                if not getattr(error, "mdt_fault_published", False):
                    self._publish_operation_fault(
                        "recover", error, evidence_prefix=evidence
                    )
                raise

    def ramp_to_zero(self) -> MDTStatus:
        """Slew every commanded contribution to zero and verify XYZ actuals."""
        capabilities = (
            "msenable?", "msenable=", "msvoltage?", "msvoltage=",
            "allvoltage=", "xvoltage=", "yvoltage=", "zvoltage=",
            "xvoltage?", "yvoltage?", "zvoltage?",
        )
        with self._lifecycle_condition:
            self._require_no_close_intent_unlocked()
            status = self.status
            if self._emergency_in_progress:
                raise _MDTOperationCancelled(
                    "MDT zero ramp was cancelled by the active emergency owner"
                )
            if (
                self.state not in {DriverState.READY, DriverState.FAULT}
                or status is None
                or self._serial is None
                or self._protocol is None
            ):
                raise InstrumentConnectionError(
                    f"MDT693B zero ramp requires a connected state, not {self.state.name}"
                )
            if not self._motion_controls_confirmed:
                raise InstrumentSafetyError(
                    "MDT zero ramp is blocked because motion controls are uncertain"
                )
            if not status.axis_command_known or self._axis_commands_v is None:
                raise InstrumentSafetyError(
                    "MDT zero ramp is disarmed because the base-axis command is unknown"
                )
            if self._zero_in_progress:
                raise InstrumentConnectionError("MDT zero ramp is already active")
            missing = frozenset(capabilities).difference(status.supported_commands)
            if missing:
                names = ", ".join(sorted(missing))
                raise InstrumentCapabilityError(
                    f"MDT firmware does not advertise required command(s): {names}"
                )
            for axis in Axis:
                axis_state = status.axes[axis]
                ceiling = min(
                    75.0,
                    status.hardware_limit.volts,
                    self.application_limits_v[axis],
                    axis_state.maximum_v,
                )
                if axis_state.minimum_v > 0.0 or ceiling < 0.0:
                    raise InstrumentSafetyError(
                        f"{axis.name} configured limits do not include zero"
                    )
            self._operation_generation += 1
            generation = self._operation_generation
            self._zero_in_progress = True
            self._zero_write_attempted = False
            self._cancel_event.clear()
            self._lifecycle_condition.notify_all()

        with self._high_impact_request_lock("ramp_to_zero"):
            try:
                with self._lifecycle_lock:
                    if (
                        generation != self._operation_generation
                        or not self._zero_in_progress
                        or self.state not in {DriverState.READY, DriverState.FAULT}
                    ):
                        raise _MDTOperationCancelled(
                            f"MDT zero ramp was cancelled while state is {self.state.name}"
                        )
                    entry_state = self.state
                    assert self._axis_commands_v is not None
                    entry_axis_commands = dict(self._axis_commands_v)
                    entry_actual = (
                        None
                        if self._last_confirmed_actual_v is None
                        else dict(self._last_confirmed_actual_v)
                    )
                    entry_master_command = self._master_scan_command_v
                def require_zero_generation() -> None:
                    with self._lifecycle_lock:
                        if (
                            generation != self._operation_generation
                            or not self._zero_in_progress
                            or self.state not in {
                                DriverState.READY, DriverState.FAULT,
                            }
                        ):
                            raise _MDTOperationCancelled(
                                "MDT zero ramp was cancelled while state is "
                                f"{self.state.name}"
                            )

                fresh = self._snapshot_from_queries(require_zero_generation)
                missing = frozenset(capabilities).difference(
                    fresh.supported_commands
                )
                if missing:
                    names = ", ".join(sorted(missing))
                    raise InstrumentCapabilityError(
                        "MDT firmware no longer advertises required zero-ramp "
                        f"command(s): {names}"
                    )
                for axis in Axis:
                    axis_state = fresh.axes[axis]
                    ceiling = min(
                        75.0,
                        fresh.hardware_limit.volts,
                        self.application_limits_v[axis],
                        axis_state.maximum_v,
                    )
                    if axis_state.minimum_v > 0.0 or ceiling < 0.0:
                        raise InstrumentSafetyError(
                            f"{axis.name} freshly read limits do not include zero"
                        )
                with self._lifecycle_lock:
                    if (
                        generation != self._operation_generation
                        or not self._zero_in_progress
                        or self.state is not entry_state
                    ):
                        raise _MDTOperationCancelled(
                            f"MDT zero ramp was cancelled while state is {self.state.name}"
                        )
                    fresh_actual = {
                        axis: fresh.axes[axis].actual_v for axis in Axis
                    }
                    tolerance = self._MOTION_CONFIRMATION_TOLERANCE_V
                    deviations = () if entry_actual is None else tuple(
                        (axis, entry_actual[axis], fresh_actual[axis])
                        for axis in Axis
                        if not math.isclose(
                            entry_actual[axis],
                            fresh_actual[axis],
                            rel_tol=0.0,
                            abs_tol=tolerance,
                        )
                    )
                    master_changed = (
                        entry_master_command is None
                        or fresh.master_scan_enabled is not status.master_scan_enabled
                        or not math.isclose(
                            fresh.master_scan_voltage_v,
                            entry_master_command,
                            rel_tol=0.0,
                            abs_tol=tolerance,
                        )
                    )
                    self.status = fresh
                    self._restriction_evidence = (
                        fresh.fault_evidence if fresh.restricted else None
                    )
                    self._master_scan_command_v = entry_master_command
                    if deviations or master_changed:
                        details = ", ".join(
                            f"{axis.name} expected {wanted:g} V but observed {seen:g} V"
                            for axis, wanted, seen in deviations
                        ) or "Master Scan control changed"
                        evidence = (
                            "MDT axis-command authority invalidated before zero ramp: "
                            + details
                        )
                        self._transition_axis_authority_unlocked(
                            False, evidence=evidence, publish_evidence=True
                        )
                        raise DeviceFault(evidence)
                    self._transition_axis_authority_unlocked(
                        True,
                        commands=entry_axis_commands,
                        actual=fresh_actual,
                    )
                current = self.status
                assert current is not None
                master_was_ramped = False
                if current.master_scan_enabled and (
                    current.master_scan_voltage_v != 0.0
                ):
                    self.set_master_scan_voltage(0.0, confirm=True)
                    master_was_ramped = True
                if master_was_ramped:
                    self._sleep(self.ramp_interval_s)
                    require_zero_generation()
                self.set_all_voltages(0.0, confirm=True)
                current = self.status
                assert current is not None
                if current.master_scan_enabled:
                    self.set_master_scan_enabled(False, confirm=True)
                actual = self._read_all_actual_unlocked()
                current = self.status
                assert current is not None
                axes = {
                    axis: AxisState(
                        actual[axis],
                        current.axes[axis].minimum_v,
                        current.axes[axis].maximum_v,
                    )
                    for axis in Axis
                }
                with self._lifecycle_lock:
                    if (
                        generation != self._operation_generation
                        or not self._zero_in_progress
                        or self.state not in {DriverState.READY, DriverState.FAULT}
                    ):
                        raise _MDTOperationCancelled(
                            f"MDT zero ramp was cancelled while state is {self.state.name}"
                        )
                    self.status = replace(
                        current, axes=axes, observed_at=self._clock()
                    )
                    published = self.status
                residual = {
                    axis: actual[axis]
                    for axis in Axis
                    if abs(actual[axis]) > self._MOTION_CONFIRMATION_TOLERANCE_V
                }
                if residual:
                    details = ", ".join(
                        f"{axis.name}={value:g} V" for axis, value in residual.items()
                    )
                    with self._lifecycle_condition:
                        self._transition_axis_authority_unlocked(
                            False,
                            evidence=(
                                "MDT axis-command authority invalidated by "
                                f"zero-ramp residual: {details}"
                            ),
                            publish_evidence=True,
                        )
                    raise DeviceFault(
                        f"MDT zero ramp left nonzero actual output(s): {details}"
                    )
                with self._lifecycle_lock:
                    self._last_confirmed_actual_v = dict(actual)
                self._restart_terminal_monitor(
                    generation,
                    allowed_states=frozenset(
                        {DriverState.READY, DriverState.FAULT}
                    ),
                )
                with self._lifecycle_condition:
                    if (
                        generation != self._operation_generation
                        or not self._zero_in_progress
                        or self.state not in {DriverState.READY, DriverState.FAULT}
                    ):
                        raise _MDTOperationCancelled(
                            f"MDT zero ramp was cancelled while state is {self.state.name}"
                        )
                    self._operation_generation += 1
                    self._zero_in_progress = False
                    self._cancel_event.clear()
                    self._lifecycle_condition.notify_all()
                    return published
            except BaseException as error:
                with self._lifecycle_lock:
                    write_attempted = self._zero_write_attempted
                    if write_attempted:
                        self._motion_controls_confirmed = False
                if (
                    (write_attempted or not isinstance(error, InstrumentSafetyError))
                    and not getattr(error, "mdt_fault_published", False)
                ):
                    self._publish_operation_fault("ramp_to_zero", error)
                raise
            finally:
                with self._lifecycle_condition:
                    self._zero_in_progress = False
                    self._lifecycle_condition.notify_all()

    def emergency_zero(self, *, confirm: bool = False) -> MDTStatus:
        """Immediately issue the documented three-command emergency zero sequence."""
        self._require_confirmation(confirm, "emergency zero")
        capabilities = (
            "msvoltage=", "allvoltage=", "msenable=",
            "xvoltage?", "yvoltage?", "zvoltage?",
        )
        with self._lifecycle_condition:
            self._require_no_close_intent_unlocked()
            status = self.status
            if self._emergency_in_progress:
                raise _MDTOperationCancelled(
                    "MDT emergency zero is already active"
                )
            if (
                self.state not in {
                    DriverState.READY, DriverState.ACTIVE, DriverState.FAULT,
                }
                or status is None
                or self._serial is None
                or self._protocol is None
            ):
                raise InstrumentConnectionError(
                    f"MDT693B emergency zero requires a connected state, not {self.state.name}"
                )
            missing = frozenset(capabilities).difference(status.supported_commands)
            if missing:
                names = ", ".join(sorted(missing))
                raise InstrumentCapabilityError(
                    f"MDT firmware does not advertise required command(s): {names}"
                )
            self._operation_generation += 1
            generation = self._operation_generation
            self._cancel_event.set()
            self.state = DriverState.ACTIVE
            motion_confirmed_before = self._motion_controls_confirmed
            self._motion_controls_confirmed = False
            self._restore_in_progress = False
            self._emergency_in_progress = True
            self._transition_axis_authority_unlocked(False)
            self._lifecycle_condition.notify_all()

        attempted = False
        with self._high_impact_request_lock(
            "emergency_zero",
            motion_confirmed_before=motion_confirmed_before,
        ):
            try:
                self._require_motion_generation(generation)
                for command in ("msvoltage=0", "allvoltage=0", "msenable=0"):
                    attempted = True
                    self._empty_response(self._query_unlocked(command), command)
                    self._require_motion_generation(generation)
                actual = self._read_all_actual_unlocked()
                self._require_motion_generation(generation)
                residual = {
                    axis: actual[axis]
                    for axis in Axis
                    if abs(actual[axis]) > self._MOTION_CONFIRMATION_TOLERANCE_V
                }
                with self._lifecycle_condition:
                    if (
                        generation != self._operation_generation
                        or self.state is not DriverState.ACTIVE
                    ):
                        raise _MDTOperationCancelled(
                            f"MDT emergency zero was cancelled while state is {self.state.name}"
                        )
                    previous = self.status
                    if previous is None:
                        raise _MDTOperationCancelled(
                            "MDT emergency zero lost its status snapshot"
                        )
                    axes = {
                        axis: AxisState(
                            actual[axis],
                            previous.axes[axis].minimum_v,
                            previous.axes[axis].maximum_v,
                        )
                        for axis in Axis
                    }
                    self.status = replace(
                        previous,
                        axes=axes,
                        master_scan_enabled=False,
                        master_scan_voltage_v=0.0,
                        observed_at=self._clock(),
                    )
                if residual:
                    details = ", ".join(
                        f"{axis.name}={value:g} V" for axis, value in residual.items()
                    )
                    raise DeviceFault(
                        f"MDT emergency zero left nonzero actual output(s): {details}"
                    )
                self._restart_terminal_monitor(generation)
                with self._lifecycle_condition:
                    if (
                        generation != self._operation_generation
                        or self.state is not DriverState.ACTIVE
                        or self.status is None
                    ):
                        raise _MDTOperationCancelled(
                            f"MDT emergency zero was cancelled while state is {self.state.name}"
                        )
                    self._master_scan_command_v = 0.0
                    self._motion_controls_confirmed = True
                    self._restriction_evidence = None
                    self.status = replace(
                        self.status,
                        restricted=False,
                        fault_evidence=None,
                        observed_at=self._clock(),
                    )
                    zeros = {axis: 0.0 for axis in Axis}
                    self._transition_axis_authority_unlocked(
                        True, commands=zeros, actual=zeros
                    )
                    self._operation_generation += 1
                    self._emergency_in_progress = False
                    self.state = DriverState.READY
                    self._cancel_event.clear()
                    self._lifecycle_condition.notify_all()
                    return self.status
            except BaseException as error:
                with self._lifecycle_lock:
                    if attempted:
                        self._motion_controls_confirmed = False
                if not getattr(error, "mdt_fault_published", False):
                    self._publish_operation_fault("emergency_zero", error)
                with self._lifecycle_lock:
                    self._emergency_in_progress = False
                raise

    def _snapshot_from_queries(
        self,
        cancellation_check: Callable[[], None] | None = None,
        *,
        actual_observer: Callable[[Mapping[Axis, float]], None] | None = None,
    ) -> MDTStatus:
        def query(command: str) -> tuple[str, ...]:
            if cancellation_check is not None:
                cancellation_check()
            result = self._query_unlocked(command)
            if cancellation_check is not None:
                cancellation_check()
            return result

        # A no-echo command list may itself begin with the exact token ``?``,
        # which is indistinguishable from an echo, including during recovery.
        # Do not apply a previously learned mode to this ambiguous help frame.
        if self._protocol is not None:
            self._protocol._echo_enabled = None
        supported = self._normalize_commands(query("?")).union({"?"})
        # Re-learn from id?, whose product response cannot equal that command.
        if self._protocol is not None:
            self._protocol._echo_enabled = None
        missing = frozenset(self._CONNECT_QUERIES).difference(supported)
        if missing:
            names = ", ".join(sorted(missing))
            raise InstrumentCapabilityError(f"MDT firmware is missing required queries: {names}")

        product, firmware = self._parse_identity(query("id?"))
        if product.strip().upper() != "MDT693B":
            raise InstrumentConnectionError(f"Unexpected MDT product: {product!r}")
        serial_number = self._single_response(query("serial?"), "serial?")
        friendly_name = self._raw_single_response(
            query("friendly?"), "friendly?"
        )
        echo_enabled = self._parse_bool(query("echo?"), "echo?")
        if self._protocol is None or self._protocol.echo_enabled is not echo_enabled:
            raise InstrumentProtocolError("MDT observed echo does not match echo? response")
        hardware_limit = self._parse_hardware_limit(query("vlimit?"))
        intensity = self._parse_int(query("intensity?"), "intensity?")
        if not 0 <= intensity <= 15:
            raise InstrumentProtocolError("MDT intensity? response is outside 0..15")
        master_enabled = self._parse_bool(query("msenable?"), "msenable?")
        master_voltage = self._parse_number(query("msvoltage?"), "msvoltage?")

        actual = {
            axis: self._parse_number(query(f"{axis.value}voltage?"), f"{axis.value}voltage?")
            for axis in Axis
        }
        if actual_observer is not None:
            actual_observer(actual)
        minimum = {
            axis: self._parse_number(query(f"{axis.value}min?"), f"{axis.value}min?")
            for axis in Axis
        }
        maximum = {
            axis: self._parse_number(query(f"{axis.value}max?"), f"{axis.value}max?")
            for axis in Axis
        }
        axes: dict[Axis, AxisState] = {}
        for axis in Axis:
            if minimum[axis] > maximum[axis]:
                raise InstrumentProtocolError(f"MDT {axis.name} minimum exceeds its maximum")
            axes[axis] = AxisState(actual[axis], minimum[axis], maximum[axis])

        dac_step = self._parse_int(query("dacstep?"), "dacstep?")
        if not 1 <= dac_step <= 1000:
            raise InstrumentProtocolError("MDT dacstep? response is outside 1..1000")
        compatibility = self._parse_bool(query("cm?"), "cm?")
        rotary_value = self._parse_int(query("rotarymode?"), "rotarymode?")
        try:
            rotary_mode = RotaryMode(rotary_value)
        except ValueError as error:
            raise InstrumentProtocolError("MDT rotarymode? returned an unknown mode") from error
        push_disabled = self._parse_bool(query("pushdisable?"), "pushdisable?")
        over_limit = self._over_limit_axes(actual)
        evidence = self._over_limit_evidence(over_limit) if over_limit else None
        return MDTStatus(
            product=product,
            firmware=firmware,
            serial_number=serial_number,
            friendly_name=friendly_name,
            echo_enabled=echo_enabled,
            hardware_limit=hardware_limit,
            display_intensity=intensity,
            master_scan_enabled=master_enabled,
            master_scan_voltage_v=master_voltage,
            axes=axes,
            dac_step=dac_step,
            compatibility_enabled=compatibility,
            rotary_mode=rotary_mode,
            push_to_adjust_disabled=push_disabled,
            supported_commands=supported,
            restricted=bool(over_limit),
            fault_evidence=evidence,
            observed_at=self._clock(),
        )

    def _over_limit_axes(self, actual: Mapping[Axis, float]) -> tuple[tuple[Axis, float, float], ...]:
        return tuple(
            (axis, actual[axis], min(75.0, self.application_limits_v[axis]))
            for axis in Axis
            if actual[axis] > min(75.0, self.application_limits_v[axis])
        )

    @staticmethod
    def _over_limit_evidence(over_limit: tuple[tuple[Axis, float, float], ...]) -> str:
        details = ", ".join(
            f"{axis.name}={actual:g} V exceeds {limit:g} V" for axis, actual, limit in over_limit
        )
        return f"MDT over-limit observation: {details}"

    def connect(self) -> MDT693B:
        with self._lifecycle_condition:
            _probe_connect_guard(self)
            if self._request_lock_uncertain:
                raise InstrumentConnectionError(
                    "Cannot reconnect MDT693B after an unproven request-lock "
                    "release; construct a new driver instance"
                )
            if self.state is DriverState.READY:
                return self
            if self.state is not DriverState.DISCONNECTED:
                raise InstrumentConnectionError(
                    f"Cannot connect MDT693B while state is {self.state.name}"
                )
            self.state = DriverState.CONNECTING
            self.cleanup_error = None
            self._restriction_evidence = None
            self._close_requested.clear()
            self._connect_done.clear()
            self._monitor_stop.clear()
            self._monitor_started.clear()
            self._monitor_exited.set()
            self._cancel_event.clear()
            self._monitor_failure_count = 0
            self._monitor_restart_required = False
            self._emergency_in_progress = False
            self._lifecycle_condition.notify_all()

        try:
            resource = self._serial_factory(
                port=self.port,
                baudrate=115200,
                bytesize=serial.EIGHTBITS,
                parity=serial.PARITY_NONE,
                stopbits=serial.STOPBITS_ONE,
                xonxoff=False,
                rtscts=False,
                dsrdtr=False,
                timeout=self.io_timeout,
                write_timeout=self.io_timeout,
            )
            with self._lifecycle_lock:
                self._serial = resource
                self._protocol = _MDTProtocol(resource, echo_enabled=None)
                close_requested = self._close_requested.is_set()
            if close_requested:
                raise InstrumentConnectionError("MDT connect was cancelled by close")
            with self._request_lock:
                snapshot = self._snapshot_from_queries()
            with self._lifecycle_lock:
                if self._close_requested.is_set() or self.state is not DriverState.CONNECTING:
                    raise InstrumentConnectionError("MDT connect was cancelled by close")
                self.status = snapshot
                self._restriction_evidence = snapshot.fault_evidence if snapshot.restricted else None
                self._transition_axis_authority_unlocked(False)
                self._master_scan_command_v = snapshot.master_scan_voltage_v
                self._motion_controls_confirmed = True
                self._arrow_capabilities = {
                    candidate: None for candidate in _ARROW_CANDIDATES
                }
                self._last_arrow_motion_at = None
                self._restore_in_progress = False
                self._zero_in_progress = False
                self._cancel_event.clear()
                monitor = threading.Thread(
                    target=self._monitor_main,
                    name="MDT693BMonitor",
                    daemon=True,
                )
                self._monitor_thread = monitor
                self._monitor_started.set()
                self._monitor_exited.clear()
                monitor.start()
                self.state = DriverState.READY
                return self
        except BaseException as error:
            with self._lifecycle_lock:
                if self.state is not DriverState.CLOSING:
                    self.state = DriverState.FAULT
            cleanup_error = self._cleanup_failed_connect()
            if cleanup_error is not None:
                self.cleanup_error = cleanup_error
                try:
                    setattr(error, "mdt_cleanup_error", cleanup_error)
                except BaseException:
                    pass
            if not isinstance(error, Exception):
                raise
            if isinstance(error, DriverError):
                raise
            raise InstrumentConnectionError(f"Failed to connect MDT693B at {self.port}") from error
        finally:
            self._connect_done.set()

    def _cleanup_failed_connect(self) -> BaseException | None:
        self._monitor_stop.set()
        monitor = self._monitor_thread
        try:
            monitor_alive = monitor is not None and monitor.is_alive()
        except BaseException as error:
            return error
        if monitor_alive:
            try:
                monitor.join(self.monitor_interval_s + self.io_timeout * 3 + 0.5)
            except BaseException as error:
                return error
            try:
                monitor_alive = monitor.is_alive()
            except BaseException as error:
                return error
            if monitor_alive:
                return DeviceFault("MDT monitor did not terminate after failed connect")
        resource = self._serial
        if resource is not None:
            try:
                resource.close()
                if getattr(resource, "is_open", False):
                    raise DeviceFault(
                        "MDT serial close returned while the failed-connect "
                        "resource remained open"
                    )
            except BaseException as error:
                if not isinstance(error, Exception) or isinstance(error, DriverError):
                    return error
                translated = InstrumentConnectionError(
                    "MDT failed-connect serial cleanup failed"
                )
                translated.__cause__ = error
                return translated
        with self._lifecycle_lock:
            self._serial = None
            self._protocol = None
            self._monitor_thread = None
            self.state = DriverState.DISCONNECTED
        return None

    def _monitor_main(self, signal_start: bool = False) -> None:
        if signal_start:
            self._monitor_started.set()
        try:
            while not self._monitor_stop.is_set():
                cycle_started = self._clock()
                with self._request_lock:
                    try:
                        actual = {
                            axis: self._parse_number(
                                self._query_unlocked(f"{axis.value}voltage?"),
                                f"{axis.value}voltage?",
                            )
                            for axis in Axis
                        }
                    except Exception as error:
                        with self._lifecycle_lock:
                            self._monitor_failure_count += 1
                            failures = self._monitor_failure_count
                        if self._publish_monitor_failure(error, failures):
                            return
                    else:
                        with self._lifecycle_lock:
                            self._monitor_failure_count = 0
                        self._publish_monitor_success(actual)
                if self._monitor_stop.is_set():
                    return
                remaining = max(0.0, self.monitor_interval_s - (self._clock() - cycle_started))
                self._sleep(remaining)
        except BaseException as error:
            if not self._monitor_stop.is_set():
                with self._lifecycle_condition:
                    self._monitor_restart_required = True
                self._publish_operation_fault("monitor terminated unexpectedly", error)
        finally:
            with self._lifecycle_condition:
                self._monitor_exited.set()
                self._lifecycle_condition.notify_all()

    def _publish_monitor_failure(self, error: Exception, count: int) -> bool:
        evidence = (
            f"MDT monitor communication failure {count}/{self.monitor_failure_limit}: "
            f"{type(error).__name__}: {error}"
        )
        with self._lifecycle_condition:
            if self.state in {DriverState.CLOSING, DriverState.DISCONNECTED}:
                return False
            if self.status is not None:
                self.status = replace(
                    self.status, fault_evidence=evidence, observed_at=self._clock()
                )
            terminal = count >= self.monitor_failure_limit
            if terminal and self.state not in {DriverState.CLOSING, DriverState.DISCONNECTED}:
                self._monitor_restart_required = True
                self._operation_generation += 1
                self._cancel_event.set()
                self.state = DriverState.FAULT
                self._lifecycle_condition.notify_all()
            return terminal

    def _publish_monitor_success(self, actual: Mapping[Axis, float]) -> None:
        with self._lifecycle_condition:
            if self.state is not DriverState.READY:
                return
            previous = self.status
            if previous is None:
                return
            axes = {
                axis: AxisState(
                    actual[axis], previous.axes[axis].minimum_v, previous.axes[axis].maximum_v
                )
                for axis in Axis
            }
            over_limit = self._over_limit_axes(actual)
            restricted = previous.restricted or bool(over_limit)
            if over_limit:
                evidence = self._over_limit_evidence(over_limit)
                self._restriction_evidence = evidence
            elif previous.restricted:
                evidence = self._restriction_evidence
            else:
                evidence = None
            self.status = replace(
                previous,
                axes=axes,
                restricted=restricted,
                fault_evidence=evidence,
                observed_at=self._clock(),
            )
            if self.status.axis_command_known:
                self._invalidate_axis_authority_for_observation_unlocked(
                    actual, context="monitor observation"
                )

    @property
    def has_resource_responsibility(self) -> bool:
        with self._lifecycle_lock:
            return self.state is not DriverState.DISCONNECTED or self._serial is not None

    def probe_identity(self) -> ProbeReport:
        def read():
            status = self.status
            return {"model": status.product, "serial": status.serial_number,
                    "firmware": status.firmware}, {
                    "voltage_v": {axis.name: value.actual_v for axis, value in status.axes.items()},
                    "axis_command_known": status.axis_command_known,
                    "restricted": status.restricted}
        return _identity_probe(self, self.connect, read)

    def close(self) -> None:
        with self._lifecycle_condition:
            if (
                self.state is DriverState.DISCONNECTED
                and self._serial is None
                and self._monitor_thread is None
            ):
                return
            motion_in_progress = (
                self.state is DriverState.ACTIVE or self._zero_in_progress
            )
            self.state = DriverState.CLOSING
            self._operation_generation += 1
            self._cancel_event.set()
            self._close_requested.set()
            self._monitor_stop.set()
            monitor = self._monitor_thread
            resource_for_cancel = self._serial
            connect_in_progress = not self._connect_done.is_set()
            self._lifecycle_condition.notify_all()

        cancel_error: BaseException | None = None
        try:
            cancel_read = getattr(resource_for_cancel, "cancel_read", None)
        except BaseException as error:
            self._publish_cleanup_fault(error)
            raise
        if callable(cancel_read):
            try:
                cancel_read()
            except BaseException as error:
                cancel_error = self._translated_cancel_error(error)

        if connect_in_progress:
            try:
                connect_finished = self._connect_done.wait(
                    self.io_timeout + self.monitor_interval_s + 1.0
                )
            except BaseException as error:
                if cancel_error is not None:
                    self._attach_cleanup_error(cancel_error, error)
                    self._publish_cleanup_fault(cancel_error)
                    raise cancel_error
                self._publish_cleanup_fault(error)
                raise
            if not connect_finished:
                timeout_error = DeviceFault("MDT connect did not terminate during close")
                if cancel_error is not None:
                    self._attach_cleanup_error(cancel_error, timeout_error)
                    self._publish_cleanup_fault(cancel_error)
                    raise cancel_error
                self._publish_cleanup_fault(timeout_error)
                raise timeout_error
            with self._lifecycle_lock:
                monitor = self._monitor_thread

        try:
            monitor_alive = monitor is not None and monitor.is_alive()
        except BaseException as error:
            if cancel_error is not None:
                self._attach_cleanup_error(cancel_error, error)
                self._publish_cleanup_fault(cancel_error)
                raise cancel_error
            self._publish_cleanup_fault(error)
            raise
        if monitor_alive:
            try:
                monitor.join(self.io_timeout + self.monitor_interval_s + 1.0)
            except BaseException as error:
                if cancel_error is not None:
                    self._attach_cleanup_error(cancel_error, error)
                    self._publish_cleanup_fault(cancel_error)
                    raise cancel_error
                self._publish_cleanup_fault(error)
                raise
            try:
                monitor_alive = monitor.is_alive()
            except BaseException as error:
                if cancel_error is not None:
                    self._attach_cleanup_error(cancel_error, error)
                    self._publish_cleanup_fault(cancel_error)
                    raise cancel_error
                self._publish_cleanup_fault(error)
                raise
            if monitor_alive:
                error = DeviceFault("MDT monitor did not terminate during close")
                if cancel_error is not None:
                    self._attach_cleanup_error(cancel_error, error)
                    self._publish_cleanup_fault(cancel_error)
                    raise cancel_error
                self._publish_cleanup_fault(error)
                raise error

        request_timeout = self.io_timeout + self.monitor_interval_s + 1.0
        try:
            request_acquired = self._request_lock.acquire(timeout=request_timeout)
        except BaseException as error:
            if cancel_error is not None:
                self._attach_cleanup_error(cancel_error, error)
                self._publish_cleanup_fault(cancel_error)
                raise cancel_error
            self._publish_cleanup_fault(error)
            raise
        if not request_acquired:
            error = DeviceFault("MDT request lock did not become available during close")
            if cancel_error is not None:
                self._attach_cleanup_error(cancel_error, error)
                self._publish_cleanup_fault(cancel_error)
                raise cancel_error
            self._publish_cleanup_fault(error)
            raise error

        body_error: BaseException | None = None
        try:
            resource = self._serial
            if resource is not None:
                try:
                    resource.close()
                    if getattr(resource, "is_open", False):
                        raise DeviceFault(
                            "MDT serial close returned while the resource remained open"
                        )
                except BaseException as error:
                    if not isinstance(error, Exception) or isinstance(error, DriverError):
                        translated = error
                    else:
                        translated = InstrumentConnectionError("MDT serial close failed")
                        translated.__cause__ = error
                    if cancel_error is not None:
                        self._attach_cleanup_error(cancel_error, translated)
                        self._publish_cleanup_fault(cancel_error)
                        raise cancel_error
                    self._publish_cleanup_fault(translated)
                    if translated is error:
                        raise
                    raise translated from error
            with self._lifecycle_lock:
                self._serial = None
                self._protocol = None
                self._monitor_thread = None
                self.cleanup_error = cancel_error
                if cancel_error is not None and self.status is not None:
                    try:
                        self.status = replace(
                            self.status,
                            fault_evidence=(
                                f"MDT cleanup cancellation failure: "
                                f"{type(cancel_error).__name__}: {cancel_error}"
                            ),
                            observed_at=self._clock(),
                        )
                    except BaseException as publication_error:
                        self._attach_cleanup_error(cancel_error, publication_error)
                self.state = DriverState.DISCONNECTED
                self._lifecycle_condition.notify_all()
        except BaseException as error:
            body_error = error
        try:
            self._request_lock.release()
        except BaseException as release_error:
            primary = body_error or cancel_error or release_error
            if primary is not release_error:
                self._attach_cleanup_error(primary, release_error)
            self._publish_cleanup_fault(primary)
            raise primary
        if body_error is not None:
            raise body_error
        if cancel_error is not None:
            raise cancel_error

    @staticmethod
    def _attach_cleanup_error(primary: BaseException, secondary: BaseException) -> None:
        try:
            existing = getattr(primary, "mdt_cleanup_error", None)
            if existing is None:
                setattr(primary, "mdt_cleanup_error", secondary)
            else:
                setattr(primary, "mdt_additional_cleanup_error", secondary)
        except BaseException:
            pass

    @staticmethod
    def _translated_cancel_error(error: BaseException) -> BaseException:
        if not isinstance(error, Exception) or isinstance(error, InstrumentConnectionError):
            return error
        translated = InstrumentConnectionError("MDT serial read cancellation failed")
        translated.__cause__ = error
        return translated

    def _publish_cleanup_fault(self, error: BaseException) -> None:
        with self._lifecycle_condition:
            self.cleanup_error = error
            if self.status is not None:
                try:
                    self.status = replace(
                        self.status,
                        fault_evidence=(
                            f"MDT cleanup failure: {type(error).__name__}: {error}"
                        ),
                        observed_at=self._clock(),
                    )
                except BaseException as publication_error:
                    self._attach_cleanup_error(error, publication_error)
            self.state = DriverState.FAULT
            self._cancel_event.set()
            self._lifecycle_condition.notify_all()

    def __enter__(self) -> MDT693B:
        return self.connect()

    def __exit__(self, exc_type, exc_value, traceback) -> bool:
        try:
            self.close()
        except BaseException as cleanup_error:
            if exc_type is None:
                raise
            if isinstance(exc_value, BaseException):
                self._attach_cleanup_error(exc_value, cleanup_error)
        return False
