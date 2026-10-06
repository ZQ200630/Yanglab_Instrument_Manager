from __future__ import annotations

import math
import threading
import time
from dataclasses import dataclass
from enum import Enum
from numbers import Integral, Real
from typing import Sequence

import serial
from serial.tools import list_ports

from .common import (
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

CHANNELS = 8
BOARD_SCALE_V = 28.0
DEFAULT_LIMIT_V = 14.0


class ZeroState(Enum):
    UNKNOWN = "unknown"
    COMMAND_SENT = "command_sent"
    MEASURED_ZERO = "measured_zero"


@dataclass(frozen=True)
class ZeroEvidence:
    """Host-observed telemetry after a write, not a firmware acknowledgement."""

    state: ZeroState
    sent_at: float | None
    observed_at: float | None
    voltage_v: tuple[float, ...] | None


@dataclass(frozen=True)
class VoltageStatus:
    voltage_v: tuple[float, ...]
    current_ma: tuple[float, ...]
    received_at: float


def encode_voltages(values: Sequence[float], limit: float = DEFAULT_LIMIT_V) -> bytes:
    if len(values) != CHANNELS:
        raise InstrumentSafetyError("Voltage command requires exactly eight channels")
    if isinstance(limit, bool):
        raise InstrumentSafetyError("Voltage limit must be a finite number")
    try:
        numeric_limit = float(limit)
    except (TypeError, ValueError):
        raise InstrumentSafetyError("Voltage limit must be a finite number") from None
    if not math.isfinite(numeric_limit) or not 0.0 <= numeric_limit <= DEFAULT_LIMIT_V:
        raise InstrumentSafetyError("Voltage limit must be within 0-14 V")

    payload = bytearray()
    for index, value in enumerate(values, start=1):
        if isinstance(value, bool):
            raise InstrumentSafetyError(f"Channel {index} voltage must be numeric, not bool")
        try:
            numeric = float(value)
        except (TypeError, ValueError):
            raise InstrumentSafetyError(f"Channel {index} voltage is not numeric") from None
        if not math.isfinite(numeric) or not 0.0 <= numeric <= numeric_limit:
            raise InstrumentSafetyError(
                f"Channel {index} voltage {numeric!r} is outside 0-{numeric_limit} V"
            )
        raw = int(numeric * 65535 / BOARD_SCALE_V)
        payload.extend(raw.to_bytes(2, "big", signed=False))
    return bytes(payload + b"\r\n")


def decode_telemetry(frame: bytes, received_at: float | None = None) -> VoltageStatus:
    if len(frame) != 34 or not frame.endswith(b"\r\n"):
        raise InstrumentProtocolError(f"Expected 34-byte voltage telemetry frame, got {len(frame)}")
    payload = frame[:-2]
    voltages = []
    currents = []
    for offset in range(0, 32, 4):
        voltages.append(int.from_bytes(payload[offset : offset + 2], "big") * 0.0016)
        currents.append(
            int.from_bytes(payload[offset + 2 : offset + 4], "big", signed=True) * 0.0025
        )
    timestamp = time.monotonic() if received_at is None else received_at
    return VoltageStatus(tuple(voltages), tuple(currents), timestamp)


class VoltageSource:
    """Eight-channel voltage source with telemetry-backed safe startup."""

    def __init__(
        self,
        port: str | None = None,
        limit: float = 14.0,
        ramp_step: float = 0.1,
        ramp_interval: float = 0.05,
        io_timeout: float = 0.5,
        startup_timeout: float = 3.0,
        telemetry_timeout: float = 1.0,
        communication_recovery_timeout: float = 3.0,
        communication_retry_interval: float = 0.1,
        write_attempts: int = 3,
        write_retry_interval: float = 0.1,
        serial_factory=serial.Serial,
        ports_provider=list_ports.comports,
        clock=time.monotonic,
        sleep=time.sleep,
    ) -> None:
        self.limit = self._positive_finite(limit, "limit", maximum=DEFAULT_LIMIT_V)
        self.ramp_step = self._positive_finite(
            ramp_step, "ramp_step", maximum=0.1
        )
        self.ramp_interval = self._positive_finite(
            ramp_interval, "ramp_interval", minimum=0.05
        )
        self.io_timeout = self._positive_finite(io_timeout, "io_timeout")
        self.startup_timeout = self._positive_finite(startup_timeout, "startup_timeout")
        self.telemetry_timeout = self._positive_finite(
            telemetry_timeout, "telemetry_timeout"
        )
        self.communication_recovery_timeout = self._positive_finite(
            communication_recovery_timeout, "communication_recovery_timeout"
        )
        self.communication_retry_interval = self._positive_finite(
            communication_retry_interval, "communication_retry_interval"
        )
        if isinstance(write_attempts, bool) or not isinstance(write_attempts, Integral):
            raise ValueError("write_attempts must be a positive integer")
        if write_attempts < 1:
            raise ValueError("write_attempts must be a positive integer")
        self.write_attempts = int(write_attempts)
        self.write_retry_interval = self._positive_finite(
            write_retry_interval, "write_retry_interval"
        )
        self.port = port
        self._serial_factory = serial_factory
        self._ports_provider = ports_provider
        self._clock = clock
        self._sleep = sleep
        # Lock order is lifecycle -> operation -> write.  State/telemetry lock is
        # never held while waiting between ramp frames or while joining a thread.
        self._lifecycle_lock = threading.RLock()
        self._operation_lock = threading.RLock()
        self._write_lock = threading.Lock()
        self._lock = threading.RLock()
        self._status_changed = threading.Condition(self._lock)
        self._stop_event = threading.Event()
        self._cancel_event = threading.Event()
        self._cancellation_generation = 0
        self._shutdown_requested = False
        self._zero_generation = 0
        self._zero_minimum_sequence = 0
        self._zero_evidence = ZeroEvidence(ZeroState.UNKNOWN, None, None, None)
        self._closing_reader_error: BaseException | None = None
        self._reader_thread: threading.Thread | None = None
        self._reader_exited = threading.Event()
        self._reader_exited.set()
        self._serial = None
        self._resource_open_pending = False
        self._latest_status: VoltageStatus | None = None
        self._status_sequence = 0
        self._last_command_sent_at: float | None = None
        self._malformed_frames = 0
        self._communication_failure_started: float | None = None
        self._communication_failure_count = 0
        self._commanded = [0.0] * CHANNELS
        self.cleanup_error: BaseException | None = None
        self.state = DriverState.DISCONNECTED
        self.log = get_logger("voltage")

    @property
    def zero_evidence(self) -> ZeroEvidence:
        """Fresh evidence only; firmware supplies no measurement ID or timestamp."""
        with self._status_changed:
            evidence = self._zero_evidence
            if (evidence.observed_at is not None
                    and self._clock() - evidence.observed_at > self.telemetry_timeout):
                self._invalidate_zero_locked()
            return self._zero_evidence

    @property
    def resources_released(self) -> bool:
        """Whether port and reader cleanup was confirmed; says nothing about voltage."""
        with self._status_changed:
            return (not self._resource_open_pending
                    and self._serial is None and self._reader_thread is None)

    def _invalidate_zero_locked(self) -> None:
        self._zero_evidence = ZeroEvidence(ZeroState.UNKNOWN, None, None, None)

    @staticmethod
    def _positive_finite(
        value: Real,
        name: str,
        *,
        maximum: float | None = None,
        minimum: float | None = None,
        allow_zero: bool = False,
    ) -> float:
        if isinstance(value, bool) or not isinstance(value, Real):
            raise ValueError(f"{name} must be finite and positive")
        numeric = float(value)
        invalid = not math.isfinite(numeric) or (
            numeric < 0.0 if allow_zero else numeric <= 0.0
        )
        if invalid:
            raise ValueError(f"{name} must be finite and {'non-negative' if allow_zero else 'positive'}")
        if maximum is not None and numeric > maximum:
            raise ValueError(f"{name} must be at most {maximum}")
        if minimum is not None and numeric < minimum:
            raise ValueError(f"{name} must be at least {minimum}")
        return numeric

    def connect(self) -> "VoltageSource":
        with self._lifecycle_lock:
            with self._status_changed:
                if self.state in {DriverState.READY, DriverState.ACTIVE}:
                    return self
                if self.state == DriverState.FAULT:
                    raise DeviceFault("Voltage source is faulted; close it before reconnecting")
                if self.state != DriverState.DISCONNECTED:
                    raise InstrumentSafetyError(
                        f"Cannot connect while driver state is {self.state.name}"
                    )
                if self._serial is not None or self._reader_thread is not None:
                    self.state = DriverState.FAULT
                    raise DeviceFault("Voltage source cleanup is incomplete; close it before reconnecting")
                self.state = DriverState.CONNECTING
                self._resource_open_pending = True
                self._stop_event.clear()
                self._cancel_event.clear()
                self._shutdown_requested = False
                self._zero_generation += 1
                self._invalidate_zero_locked()
                self._closing_reader_error = None
                self._latest_status = None
                self._status_sequence = 0
                self._last_command_sent_at = None
                self._malformed_frames = 0
                self._communication_failure_started = None
                self._communication_failure_count = 0
                self.cleanup_error = None
            try:
                if self.port is None:
                    self.port = find_serial_port(0x1A86, 0x7523, ports_provider=self._ports_provider)
                serial_port = self._serial_factory(
                    port=self.port, baudrate=115200, bytesize=8, parity="N", stopbits=1,
                    timeout=self.io_timeout, write_timeout=self.io_timeout,
                )
                with self._status_changed:
                    self._serial = serial_port
                    self._resource_open_pending = False
                if not getattr(serial_port, "is_open", True):
                    raise InstrumentConnectionError(f"Failed to open voltage source on {self.port}")
                if self._cancel_event.is_set():
                    raise InstrumentConnectionError("Voltage source connection was cancelled")
                self._start_reader()
                self._wait_for_status(after_sequence=-1, require_zero=False)
                with self._status_changed:
                    before_zero = self._status_sequence
                sent_at = self._clock()
                with self._write_lock:
                    self._write_frame_locked(encode_voltages([0.0] * CHANNELS, self.limit))
                self._wait_for_status(after_sequence=before_zero, require_zero=True, sent_at=sent_at)
                with self._status_changed:
                    if self._cancel_event.is_set() or self.state == DriverState.FAULT:
                        raise DeviceFault("Voltage source faulted during startup")
                    self.state = DriverState.READY
                self.log.info("Voltage source connected: port=%s", self.port)
                return self
            except BaseException as error:
                with self._status_changed:
                    self._resource_open_pending = False
                self.log.error("Voltage source connection failed: port=%s error=%s", self.port, type(error).__name__)
                self._cancel_event.set()
                try:
                    self._close_locked()
                except BaseException as cleanup_error:
                    self.cleanup_error = cleanup_error
                    self.log.error("Voltage source connection cleanup failed: port=%s error=%s", self.port, type(cleanup_error).__name__)
                if not isinstance(error, Exception):
                    raise
                if isinstance(error, DriverError):
                    raise
                raise InstrumentConnectionError(f"Failed to connect voltage source on {self.port}") from error

    def _start_reader(self) -> None:
        with self._lock:
            self._reader_exited = threading.Event()
            self._reader_thread = threading.Thread(
                target=self._run_reader,
                args=(self._reader_exited,),
                name="VoltageSourceReader",
                daemon=False,
            )
            self._reader_thread.start()

    def _run_reader(self, exited: threading.Event) -> None:
        try:
            self._reader_loop()
        finally:
            # Independent of Thread's bookkeeping: CPython 3.10 can mark a
            # still-running target stopped when KeyboardInterrupt breaks join.
            # This is the last action of the target; no serial access follows.
            exited.set()

    def _reader_loop(self) -> None:
        while not self._stop_event.is_set():
            with self._status_changed:
                read_generation = self._zero_generation
                read_started_at = self._clock()
                serial_port = self._serial
            try:
                frame = serial_port.read_until(b"\r\n")
            except (OSError, serial.SerialException) as error:
                if self._stop_event.is_set():
                    return
                failure = InstrumentConnectionError("Voltage telemetry read failed")
                failure.__cause__ = error
                if self._record_closing_reader_error(failure):
                    return
                if self._record_communication_failure(failure):
                    self._enter_fault(failure)
                    return
                if self._stop_event.wait(self.communication_retry_interval):
                    return
                continue
            if self._stop_event.is_set():
                return
            try:
                status = decode_telemetry(frame, received_at=self._clock())
            except InstrumentProtocolError as error:
                if self._stop_event.is_set():
                    return
                if self._record_closing_reader_error(error):
                    return
                self._malformed_frames += 1
                if self._record_communication_failure(error):
                    self._enter_fault(error)
                    return
                continue
            self._malformed_frames = 0
            with self._status_changed:
                if self._stop_event.is_set():
                    return
                recovery = self._clear_communication_failure_locked()
                self._latest_status = status
                self._status_sequence += 1
                evidence = self._zero_evidence
                if read_generation == self._zero_generation:
                    if not all(abs(value) < 0.05 for value in status.voltage_v):
                        # Keep the command boundary so later settled telemetry can
                        # confirm the same zero command, but revoke any observation.
                        self._zero_evidence = ZeroEvidence(
                            ZeroState.UNKNOWN, evidence.sent_at, None, None)
                    elif (evidence.sent_at is not None
                          and read_started_at >= evidence.sent_at
                          and self._status_sequence > self._zero_minimum_sequence
                          and 0 <= self._clock() - status.received_at <= self.telemetry_timeout):
                        self._zero_evidence = ZeroEvidence(
                            ZeroState.MEASURED_ZERO, evidence.sent_at,
                            status.received_at, status.voltage_v)
                self._status_changed.notify_all()
            if recovery is not None:
                count, elapsed = recovery
                self.log.info(
                    "Voltage telemetry recovered: failures=%d elapsed_s=%.3f",
                    count,
                    elapsed,
                )

    def _record_closing_reader_error(self, error: BaseException) -> bool:
        with self._status_changed:
            if self._stop_event.is_set():
                return True
            if self.state != DriverState.CLOSING:
                return False
            self._closing_reader_error = error
            self._invalidate_zero_locked()
            self._status_changed.notify_all()
            return True

    def _record_communication_failure(self, error: BaseException) -> bool:
        now = self._clock()
        with self._status_changed:
            self._invalidate_zero_locked()
            if self._communication_failure_started is None:
                self._communication_failure_started = now
            self._communication_failure_count += 1
            elapsed = now - self._communication_failure_started
            count = self._communication_failure_count
            exhausted = elapsed >= self.communication_recovery_timeout
            self._status_changed.notify_all()
        self.log.warning(
            "Voltage communication retry: failure=%d elapsed_s=%.3f "
            "deadline_s=%.3f error=%s detail=%s",
            count,
            elapsed,
            self.communication_recovery_timeout,
            type(error).__name__,
            self._error_detail(error),
        )
        return exhausted

    @staticmethod
    def _error_detail(error: BaseException) -> str:
        cause = error.__cause__
        if cause is None:
            return str(error)
        return f"{error}; caused by {type(cause).__name__}: {cause}"

    def _clear_communication_failure_locked(self) -> tuple[int, float] | None:
        if self._communication_failure_started is None:
            return None
        recovery = (
            self._communication_failure_count,
            self._clock() - self._communication_failure_started,
        )
        self._communication_failure_started = None
        self._communication_failure_count = 0
        return recovery

    def _wait_for_status(
        self,
        *,
        after_sequence: int,
        require_zero: bool,
        sent_at: float | None = None,
    ) -> VoltageStatus:
        deadline = self._clock() + self.startup_timeout
        with self._status_changed:
            while True:
                if self.state == DriverState.FAULT:
                    raise DeviceFault("Voltage source faulted while waiting for telemetry")
                if self._cancel_event.is_set() or self.state != DriverState.CONNECTING:
                    raise InstrumentConnectionError("Voltage source connection was cancelled")
                status = self._latest_status
                if (
                    status is not None
                    and self._status_sequence > after_sequence
                    and (sent_at is None or status.received_at >= sent_at)
                    and (not require_zero or self.zero_evidence.state == ZeroState.MEASURED_ZERO)
                ):
                    if self._cancel_event.is_set() or self.state != DriverState.CONNECTING:
                        raise InstrumentConnectionError("Voltage source connection was cancelled")
                    return status
                remaining = deadline - self._clock()
                if remaining <= 0.0:
                    raise InstrumentTimeoutError("Timed out waiting for voltage telemetry")
                self._status_changed.wait(timeout=remaining)

    def read_status(self, max_age: float | None = None) -> VoltageStatus:
        age_limit = self.telemetry_timeout if max_age is None else self._positive_finite(max_age, "max_age", allow_zero=True)
        with self._lock:
            require_state(self.state, {DriverState.READY, DriverState.ACTIVE}, "read voltage status")
            status = self._latest_status
            if status is None or self._clock() - status.received_at > age_limit:
                raise InstrumentTimeoutError("Voltage telemetry is unavailable or stale")
            return status

    def wait_for_status(self, timeout: float | None = None) -> VoltageStatus:
        """Wait for a telemetry frame received after this call begins."""
        wait_time = self.telemetry_timeout if timeout is None else self._positive_finite(timeout, "timeout")
        ordinary_deadline = self._clock() + wait_time
        with self._status_changed:
            require_state(self.state, {DriverState.READY, DriverState.ACTIVE}, "wait for voltage status")
            sequence = self._status_sequence
            while self._status_sequence <= sequence:
                if self.state == DriverState.FAULT:
                    raise DeviceFault("Voltage source faulted while waiting for telemetry")
                if self._cancel_event.is_set() or self.state not in {DriverState.READY, DriverState.ACTIVE}:
                    raise InstrumentConnectionError("Voltage source lifecycle is closing")
                deadline = ordinary_deadline
                if self._communication_failure_started is not None:
                    deadline = max(
                        deadline,
                        self._communication_failure_started
                        + self.communication_recovery_timeout,
                    )
                remaining = deadline - self._clock()
                if remaining <= 0:
                    raise InstrumentTimeoutError("Timed out waiting for new voltage telemetry")
                self._status_changed.wait(remaining)
            if self.state == DriverState.FAULT:
                raise DeviceFault("Voltage source faulted while waiting for telemetry")
            if self._cancel_event.is_set() or self.state not in {DriverState.READY, DriverState.ACTIVE}:
                raise InstrumentConnectionError("Voltage source lifecycle is closing")
            if self._latest_status is None:
                raise InstrumentTimeoutError("Voltage telemetry is unavailable")
            return self._latest_status

    def wait_for_channel(
        self,
        channel: int,
        target_v: float,
        *,
        timeout: float | None = None,
        tolerance_v: float = 0.1,
    ) -> VoltageStatus:
        """Wait until post-command telemetry confirms one channel's target."""

        if (
            isinstance(channel, bool)
            or not isinstance(channel, Integral)
            or not 1 <= channel <= CHANNELS
        ):
            raise InstrumentSafetyError(
                f"Channel number must be an integer within 1-{CHANNELS}"
            )
        encode_voltages([target_v] + [0.0] * (CHANNELS - 1), self.limit)
        target = float(target_v)
        tolerance = self._positive_finite(tolerance_v, "tolerance_v")
        wait_time = (
            self.telemetry_timeout
            if timeout is None
            else self._positive_finite(timeout, "timeout")
        )
        ordinary_deadline = self._clock() + wait_time
        with self._status_changed:
            require_state(
                self.state,
                {DriverState.READY, DriverState.ACTIVE},
                "wait for voltage target",
            )
            sent_at = self._last_command_sent_at
            if sent_at is None:
                raise InstrumentSafetyError(
                    "Cannot confirm a voltage target before a command is sent"
                )
            while True:
                if self.state == DriverState.FAULT:
                    raise DeviceFault("Voltage source faulted while confirming target")
                if (
                    self._cancel_event.is_set()
                    or self.state not in {DriverState.READY, DriverState.ACTIVE}
                ):
                    raise InstrumentConnectionError("Voltage source lifecycle is closing")
                status = self._latest_status
                if (
                    status is not None
                    and status.received_at >= sent_at
                    and abs(status.voltage_v[channel - 1] - target) < tolerance
                ):
                    return status
                deadline = ordinary_deadline
                if self._communication_failure_started is not None:
                    deadline = max(
                        deadline,
                        self._communication_failure_started
                        + self.communication_recovery_timeout,
                    )
                remaining = deadline - self._clock()
                if remaining <= 0.0:
                    raise InstrumentTimeoutError(
                        f"Timed out confirming channel {channel} at {target:.6f} V"
                    )
                self._status_changed.wait(timeout=remaining)

    def set_channel(self, channel: int, voltage: float) -> None:
        """Smoothly change one one-based channel while preserving the other targets."""
        operation_token = self._capture_operation_token()
        if isinstance(channel, bool) or not isinstance(channel, Integral) or not 1 <= channel <= CHANNELS:
            raise InstrumentSafetyError(f"Channel number must be an integer within 1-{CHANNELS}")
        encode_voltages([voltage] + [0.0] * (CHANNELS - 1), self.limit)
        with self._operation_lock:
            self._begin_normal_operation_locked(operation_token)
            with self._status_changed:
                target = list(self._commanded)
            target[channel - 1] = voltage
            self._ramp_to_locked(self._validated_target(target))

    def set_all(self, values: Sequence[float]) -> None:
        """Smoothly change every channel to its requested target."""
        operation_token = self._capture_operation_token()
        self._ramp_to_public(values, operation_token)

    def ramp_to(self, values: Sequence[float]) -> None:
        """Linearly ramp all channels to a fully validated target."""
        operation_token = self._capture_operation_token()
        self._ramp_to_public(values, operation_token)

    def _ramp_to_public(self, values: Sequence[float], operation_token: int) -> None:
        target = self._validated_target(values)

        with self._operation_lock:
            self._begin_normal_operation_locked(operation_token)
            self._ramp_to_locked(target)

    def _capture_operation_token(self) -> int:
        """Capture cancellation order before user validation or operation queuing."""
        with self._status_changed:
            return self._cancellation_generation

    def _begin_normal_operation_locked(self, operation_token: int) -> None:
        recovery_error: InstrumentTimeoutError | None = None
        with self._status_changed:
            require_state(self.state, {DriverState.READY, DriverState.ACTIVE}, "set voltage")
            if self._shutdown_requested:
                raise InstrumentConnectionError("Voltage source lifecycle is closing")
            if operation_token != self._cancellation_generation:
                raise DeviceFault("Voltage operation was cancelled before it acquired control")
            while self._communication_failure_started is not None:
                require_state(
                    self.state,
                    {DriverState.READY, DriverState.ACTIVE},
                    "wait for voltage communication recovery",
                )
                if self._shutdown_requested:
                    raise InstrumentConnectionError("Voltage source lifecycle is closing")
                remaining = (
                    self._communication_failure_started
                    + self.communication_recovery_timeout
                    - self._clock()
                )
                if remaining <= 0.0:
                    recovery_error = InstrumentTimeoutError(
                        "Voltage communication did not recover before the deadline"
                    )
                    break
                self._status_changed.wait(timeout=remaining)
            # A public emergency zero leaves cancellation set until the interrupted
            # operation releases this sequence lock.  The next explicit operation is
            # therefore the only safe point at which to reset it.
            if recovery_error is None:
                self._cancel_event.clear()
        if recovery_error is not None:
            self._enter_fault(recovery_error)
            raise DeviceFault("Voltage communication recovery failed") from recovery_error

    def _validated_target(self, values: Sequence[float]) -> tuple[float, ...]:
        try:
            target = tuple(values)
        except TypeError:
            raise InstrumentSafetyError("Voltage command requires exactly eight channels") from None
        encode_voltages(target, self.limit)
        return tuple(float(value) for value in target)

    def _ramp_to_locked(self, target: Sequence[float]) -> None:
        with self._status_changed:
            require_state(
                self.state,
                {DriverState.READY, DriverState.ACTIVE},
                "set voltage",
            )
            start = tuple(self._commanded)
        max_delta = max(abs(end - begin) for begin, end in zip(start, target, strict=True))
        steps = max(1, math.ceil(max_delta / self.ramp_step))
        for step in range(1, steps + 1):
            if self._cancel_event.is_set():
                raise DeviceFault("Voltage ramp was cancelled")
            fraction = step / steps
            frame_values = tuple(
                begin + (end - begin) * fraction
                for begin, end in zip(start, target, strict=True)
            )
            frame = encode_voltages(frame_values, self.limit)
            sent_at = self._clock()
            try:
                with self._write_lock:
                    if self._cancel_event.is_set():
                        raise DeviceFault("Voltage ramp was cancelled")
                    self._write_frame_locked(frame)
                    if self._cancel_event.is_set():
                        raise DeviceFault("Voltage ramp was cancelled")
                    self._commanded = list(frame_values)
                    self._last_command_sent_at = sent_at
            except DriverError as error:
                if not isinstance(error, DeviceFault):
                    self._enter_fault(error)
                raise
            if step < steps:
                if self._cancel_event.wait(self.ramp_interval):
                    raise DeviceFault("Voltage ramp was cancelled")
        with self._status_changed:
            self.state = DriverState.ACTIVE if any(self._commanded) else DriverState.READY

    def zero(self, emergency: bool = False) -> None:
        """Return every channel to zero, immediately only for emergency handling."""
        if not emergency:
            self.ramp_to([0.0] * CHANNELS)
            return

        # Do this before contending for the write lock so an in-flight ramp cannot
        # produce another nonzero frame after this zero is issued.
        self._advance_cancellation_generation()
        failure: BaseException | None = None
        completion_error: BaseException | None = None
        with self._write_lock:
            try:
                require_state(
                    self.state,
                    {DriverState.READY, DriverState.ACTIVE},
                    "emergency-zero voltage source",
                )
                self._write_immediate_zero_locked()
            except BaseException as error:
                failure = error
            # Keep the write path exclusive until the exact-write result, final
            # READY/FAULT state, and completion generation are published together.
            completion_error = self._publish_emergency_completion(failure)
        if completion_error is not None:
            raise completion_error

    def _write_immediate_zero_locked(self) -> None:
        self._write_frame_locked(encode_voltages([0.0] * CHANNELS, self.limit))
        self._commanded = [0.0] * CHANNELS

    def _write_frame_locked(self, frame: bytes) -> None:
        with self._status_changed:
            self._zero_generation += 1
            self._invalidate_zero_locked()
        serial_port = self._serial
        if serial_port is None or not getattr(serial_port, "is_open", True):
            raise InstrumentConnectionError("Voltage source serial port is not open")
        last_error: InstrumentConnectionError | None = None
        for attempt in range(1, self.write_attempts + 1):
            with self._status_changed:
                self._zero_generation += 1
                self._invalidate_zero_locked()
            try:
                count = serial_port.write(frame)
            except Exception as error:
                last_error = InstrumentConnectionError("Voltage command write failed")
                last_error.__cause__ = error
            else:
                if (
                    not isinstance(count, bool)
                    and isinstance(count, Integral)
                    and count == len(frame)
                ):
                    with self._status_changed:
                        # Start the observation generation only after write has
                        # returned. Reads begun inside write are ineligible even
                        # when clock resolution gives them the same timestamp.
                        self._zero_generation += 1
                        if frame[:16] == b"\x00" * 16:
                            self._zero_minimum_sequence = self._status_sequence
                            self._zero_evidence = ZeroEvidence(
                                ZeroState.COMMAND_SENT, self._clock(), None, None)
                        self._status_changed.notify_all()
                    if attempt > 1:
                        self.log.info(
                            "Voltage command write recovered: attempts=%d",
                            attempt,
                        )
                    break
                last_error = InstrumentConnectionError(
                    f"Voltage command write returned {count!r}; expected {len(frame)} bytes"
                )
            if attempt == self.write_attempts:
                assert last_error is not None
                raise last_error
            self.log.warning(
                "Voltage command write retry: attempt=%d/%d error=%s detail=%s",
                attempt,
                self.write_attempts,
                type(last_error).__name__,
                self._error_detail(last_error),
            )
            self._sleep(self.write_retry_interval)

    def _enter_fault(self, error: BaseException) -> None:
        if not self._begin_fault_transition(error):
            return
        try:
            with self._write_lock:
                self._write_immediate_zero_locked()
        except BaseException as zero_error:
            with self._status_changed:
                self.cleanup_error = zero_error
            self.log.exception("Voltage source emergency zero during fault failed")
        self.log.error("Voltage source fault: error=%s", type(error).__name__)

    def _begin_fault_transition(self, error: BaseException) -> bool:
        """Atomically publish fault cancellation, evidence, and terminal state."""
        with self._status_changed:
            if self._stop_event.is_set() or self.state in {
                DriverState.CLOSING,
                DriverState.DISCONNECTED,
                DriverState.FAULT,
            }:
                return False
            self._cancellation_generation += 1
            self._cancel_event.set()
            self.state = DriverState.FAULT
            self._invalidate_zero_locked()
            if self.cleanup_error is None:
                self.cleanup_error = error
            self._stop_event.set()
            self._status_changed.notify_all()
            return True

    def close(self) -> None:
        if not self._begin_close_transition():
            return
        # This is deliberately after terminal publication: connect can be
        # holding lifecycle ownership while waiting for its first frame, and
        # waiters must observe cancellation/CLOSING before close queues behind it.
        with self._lifecycle_lock:
            self._close_locked()

    def _begin_close_transition(self) -> bool:
        """Atomically publish a close request before lifecycle serialization."""
        with self._status_changed:
            self._shutdown_requested = True
            if self.state == DriverState.DISCONNECTED:
                self._status_changed.notify_all()
                return False
            self._cancellation_generation += 1
            self._cancel_event.set()
            self.state = DriverState.CLOSING
            self._status_changed.notify_all()
            return True

    def _advance_cancellation_generation(self) -> int:
        """Publish the non-terminal emergency start barrier for normal operations."""
        with self._status_changed:
            self._cancellation_generation += 1
            self._cancel_event.set()
            self._status_changed.notify_all()
            return self._cancellation_generation

    def _publish_emergency_completion(
        self, failure: BaseException | None
    ) -> BaseException | None:
        """Publish the terminal emergency result before another write can begin."""
        with self._status_changed:
            self._cancellation_generation += 1
            self._cancel_event.set()
            terminal_state = self.state in {
                DriverState.FAULT,
                DriverState.CLOSING,
                DriverState.DISCONNECTED,
            }
            if failure is None and not terminal_state and not self._stop_event.is_set():
                self.state = DriverState.READY
                completion_error: BaseException | None = None
            elif terminal_state or self._stop_event.is_set():
                completion_error = (
                    DeviceFault("Voltage source emergency zero lost a terminal-state race")
                    if self.state == DriverState.FAULT or self._stop_event.is_set()
                    else InstrumentConnectionError("Voltage source lifecycle is closing")
                )
            else:
                self.state = DriverState.FAULT
                if self.cleanup_error is None:
                    self.cleanup_error = failure
                self._stop_event.set()
                completion_error = failure
            self._status_changed.notify_all()
            return completion_error

    def _close_locked(self) -> None:
        with self._status_changed:
            if self.state == DriverState.DISCONNECTED:
                return
            self.state = DriverState.CLOSING
            serial_port = self._serial
            reader = self._reader_thread
            reader_exited = self._reader_exited
            self._closing_reader_error = None
            self._status_changed.notify_all()
        primary: BaseException | None = None
        errors: list[BaseException] = []
        zero_confirmed = False

        def remember(error: BaseException) -> None:
            nonlocal primary
            errors.append(error)
            if primary is None or (
                isinstance(error, (KeyboardInterrupt, SystemExit))
                and not isinstance(primary, (KeyboardInterrupt, SystemExit))
            ):
                primary = error

        try:
            if serial_port is not None and getattr(serial_port, "is_open", True):
                with self._write_lock:
                    self._write_immediate_zero_locked()
                # Use wall time for the bounded cleanup budget even if a caller's
                # injected telemetry clock is stationary. No output retries here.
                deadline = time.monotonic() + 1.0
                with self._status_changed:
                    while self.zero_evidence.state != ZeroState.MEASURED_ZERO:
                        if (self._stop_event.is_set() or reader is None
                                or reader is threading.current_thread()
                                or self._closing_reader_error is not None):
                            break
                        remaining = deadline - time.monotonic()
                        if remaining <= 0:
                            break
                        self._status_changed.wait(remaining)
            with self._status_changed:
                zero_confirmed = self.zero_evidence.state == ZeroState.MEASURED_ZERO
                self._stop_event.set()
        except BaseException as error:
            remember(error)
            self.log.exception("Voltage source zero confirmation during shutdown failed")
        with self._status_changed:
            self._stop_event.set()
            if not zero_confirmed:
                self._invalidate_zero_locked()
                unknown = DeviceFault("Voltage source shutdown output unknown: zero telemetry not confirmed")
                unknown.__cause__ = self._closing_reader_error
                remember(unknown)
            self._status_changed.notify_all()
        reader_released = reader is None
        if reader is threading.current_thread():
            remember(DeviceFault("Voltage source reader cannot close itself"))
        elif reader is not None:
            # Windows Serial.close destroys OVERLAPPED structures still used by
            # read_until. Cancel without destroying them, then join before close.
            # Transports without cancellation must honor their finite I/O timeout.
            try:
                cancel_read = getattr(serial_port, "cancel_read", None)
                if callable(cancel_read) and getattr(serial_port, "is_open", False):
                    cancel_read()
            except BaseException as error:
                remember(error)
            reader_deadline = time.monotonic() + self.io_timeout + 1.0
            try:
                reader.join(timeout=self.io_timeout + 1.0)
            except BaseException as error:
                remember(error)
            try:
                # Never trust is_alive(), including on later cleanup attempts:
                # a previously interrupted join may have poisoned it forever.
                reader_released = reader_exited.wait(
                    max(0.0, reader_deadline - time.monotonic())
                )
            except BaseException as error:
                remember(error)
            if not reader_released:
                remember(DeviceFault("Voltage source reader did not stop during shutdown"))
                self.log.error("Voltage source reader did not stop during shutdown")
        port_released = serial_port is None
        if serial_port is not None and reader_released:
            try:
                serial_port.close()
                port_released = not getattr(serial_port, "is_open", False)
            except BaseException as error:
                remember(error)
                self.log.exception("Voltage source serial close failed")
                # An exception may occur after the transport has actually closed.
                try:
                    port_released = getattr(serial_port, "is_open", True) is False
                except BaseException as status_error:
                    remember(status_error)
            if not port_released:
                remember(DeviceFault("Voltage source serial port release was not confirmed"))
        elif serial_port is not None:
            self.log.error("Voltage source serial port retained for its live reader")
        with self._status_changed:
            if port_released:
                self._serial = None
            if reader_released:
                self._reader_thread = None
            if primary is not None:
                self.state = DriverState.FAULT
                self.cleanup_error = primary
                primary.cleanup_errors = tuple(errors)
            else:
                self.state = DriverState.DISCONNECTED
                self.cleanup_error = None
            self._status_changed.notify_all()
        if primary is not None:
            raise primary
        self.log.info("Voltage source shutdown complete: port=%s", self.port)

    def __enter__(self) -> "VoltageSource":
        return self.connect()

    def __exit__(self, exc_type, exc, traceback) -> bool:
        if exc_type is None:
            self.close()
            return False
        try:
            self.close()
        except BaseException as cleanup_error:
            self.cleanup_error = cleanup_error
            self.log.error(
                "Voltage source context cleanup failed: port=%s error=%s",
                self.port,
                type(cleanup_error).__name__,
            )
        return False
