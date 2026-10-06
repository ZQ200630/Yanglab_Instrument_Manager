"""Pure ASCII protocol helpers for the Gain Chip Driver."""

from __future__ import annotations

import math
import re
import threading
import time
from dataclasses import dataclass, replace
from numbers import Integral, Real

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


_COMMAND = re.compile(r"^[A-Z][A-Z0-9]{2,3}$")
_FIELD = re.compile(r"^[A-Z]$")
_REPLY = re.compile(r"^READY;([A-Z])=([^;\r\n]+)\r\n$")


def _validate_command_name(name: str) -> None:
    if not isinstance(name, str) or not _COMMAND.fullmatch(name):
        raise InstrumentProtocolError("Gain command name must be 3-4 ASCII uppercase characters")
    try:
        name.encode("ascii")
    except UnicodeEncodeError as error:
        raise InstrumentProtocolError("Gain command name must be ASCII") from error


def _validate_field(field: str) -> None:
    if not isinstance(field, str) or not _FIELD.fullmatch(field):
        raise InstrumentProtocolError("Gain reply field must be one ASCII uppercase character")


def format_fixed(value: float, minimum: float, maximum: float, label: str) -> str:
    """Format a non-negative finite value in the device's six-digit milli-unit field."""
    if isinstance(value, bool):
        raise InstrumentSafetyError(f"{label} must be numeric, not bool")
    try:
        numeric = float(value)
    except (TypeError, ValueError) as error:
        raise InstrumentSafetyError(f"{label} must be numeric") from error
    if not math.isfinite(numeric):
        raise InstrumentSafetyError(f"{label} must be finite")
    if not isinstance(minimum, (int, float)) or isinstance(minimum, bool):
        raise ValueError("minimum must be numeric")
    if not isinstance(maximum, (int, float)) or isinstance(maximum, bool):
        raise ValueError("maximum must be numeric")
    if not math.isfinite(float(minimum)) or not math.isfinite(float(maximum)):
        raise ValueError("limits must be finite")
    if not minimum <= numeric <= maximum:
        raise InstrumentSafetyError(f"{label} {numeric!r} is outside {minimum}-{maximum}")
    scaled = int(round(numeric * 1000))
    if scaled < 0 or scaled > 999999:
        raise InstrumentSafetyError(f"{label} cannot fit the six-digit device field")
    return f"{scaled:06d}"


def build_command(
    name: str,
    value: float | bool | None = None,
    minimum: float | None = None,
    maximum: float | None = None,
    label: str = "value",
) -> bytes:
    """Build one strictly framed ASCII command."""
    _validate_command_name(name)
    if value is None:
        payload = name
    elif isinstance(value, bool):
        payload = f"{name}{int(value):06d}"
    else:
        if minimum is None or maximum is None:
            raise ValueError("numeric commands require minimum and maximum")
        payload = name + format_fixed(value, minimum, maximum, label)
    return (payload + "\r\n").encode("ascii")


def parse_reply(line: bytes, expected_field: str) -> str:
    """Parse a READY field reply with exact CRLF framing."""
    _validate_field(expected_field)
    if not isinstance(line, (bytes, bytearray)):
        raise InstrumentProtocolError("Gain reply must be bytes")
    try:
        text = bytes(line).decode("ascii")
    except UnicodeDecodeError as error:
        raise InstrumentProtocolError("Gain reply is not ASCII") from error
    match = _REPLY.fullmatch(text)
    if match is None:
        raise InstrumentProtocolError(f"Malformed Gain reply: {text!r}")
    field, value = match.groups()
    if field != expected_field:
        raise InstrumentProtocolError(f"Expected Gain reply field {expected_field}, received {field}")
    return value


def parse_ack(line: bytes) -> str:
    """Parse either the exact READY acknowledgement or a strict READY field reply."""
    if not isinstance(line, (bytes, bytearray)):
        raise InstrumentProtocolError("Gain acknowledgement must be bytes")
    try:
        text = bytes(line).decode("ascii")
    except UnicodeDecodeError as error:
        raise InstrumentProtocolError("Gain acknowledgement is not ASCII") from error
    if text == "READY\r\n":
        return "READY"
    if _REPLY.fullmatch(text) is not None:
        return text[:-2]
    raise InstrumentProtocolError(f"Malformed Gain acknowledgement: {text!r}")


CP210X_VID = 0x10C4
CP210X_PID = 0xEA60
DEFAULT_USB_SERIAL = "E42E432326A3ED118B3D99412981D5C7"
_CONSECUTIVE_SAMPLE_MAX_GAP = 1.5
_THERMAL_EVIDENCE_FRESHNESS = 1.5
_STABILITY_SECONDS = 5.0
_STABILITY_SAMPLES = 6
_FLOAT_REPLY_LIMITS = {
    "E": (15.0, 40.0),
    "C": (0.0, 200.0),
    "P": (0.0, 999.999),
    "I": (0.0, 999.999),
    # `D` is the TEC boolean in its state-command context; it is a PID
    # coefficient only when routed through the floating-point converter.
    "D": (0.0, 999.999),
}


@dataclass(frozen=True)
class GainStatus:
    """Confirmed, immutable snapshot of the Gain Chip Driver state."""

    temperature_c: float
    target_c: float
    tec_enabled: bool
    current_ma: float
    current_enabled: bool
    received_at: float


class GainDriver:
    """Serialized request/response driver for the CP210x Gain Chip Driver."""

    def __init__(
        self,
        port: str | None = None,
        usb_serial: str | None = DEFAULT_USB_SERIAL,
        io_timeout: float = 1.0,
        poll_interval: float = 1.0,
        start_watchdog: bool = True,
        serial_factory=serial.Serial,
        ports_provider=list_ports.comports,
        clock=time.monotonic,
        sleep=time.sleep,
    ) -> None:
        self._configured_port = port
        self.port = port
        self.usb_serial = usb_serial
        self.io_timeout = self._positive_finite(io_timeout, "io_timeout")
        self.poll_interval = self._positive_finite(poll_interval, "poll_interval")
        if self.poll_interval != 1.0:
            raise ValueError("poll_interval must be exactly one second")
        if not isinstance(start_watchdog, bool):
            raise ValueError("start_watchdog must be bool")
        self.start_watchdog = start_watchdog
        self._serial_factory = serial_factory
        self._ports_provider = ports_provider
        self._clock = clock
        self._sleep = sleep
        self._request_lock = threading.RLock()
        self._lifecycle_lock = threading.RLock()
        self._watchdog_condition = threading.Condition(threading.RLock())
        self._latest_temperature: float | None = None
        self._latest_temperature_at: float | None = None
        self._stable_samples = 0
        self._stable_sample_times: list[float] = []
        self._last_stable_sample_at: float | None = None
        self._stability_epoch = 0
        self._moderate_deviation_samples = 0
        self._last_moderate_sample_at: float | None = None
        self._read_failures = 0
        self._last_read_failure_at: float | None = None
        self._stop_watchdog = threading.Event()
        self._watchdog_thread: threading.Thread | None = None
        self._serial = None
        self._shutdown_current_confirmed = False
        self._shutdown_tec_confirmed = False
        self._serial_closed = False
        self._connect_invariant_fault = False
        self._shutdown_error: BaseException | None = None
        self.status: GainStatus | None = None
        self.state = DriverState.DISCONNECTED
        self.cleanup_error: BaseException | None = None
        self.fault_error: BaseException | None = None
        self.log = get_logger("gain")

    def _clear_stability_locked(self) -> None:
        """Invalidate all thermal evidence while holding _watchdog_condition."""
        self._stability_epoch += 1
        self._stable_samples = 0
        self._stable_sample_times = []
        self._last_stable_sample_at = None

    def _stability_freshness_bound(self) -> float:
        return _THERMAL_EVIDENCE_FRESHNESS

    def _consecutive_sample_max_gap(self) -> float:
        """Largest interval that still represents adjacent one-second samples."""
        return _CONSECUTIVE_SAMPLE_MAX_GAP

    def _stability_ready_locked(self, now: float) -> bool:
        times = self._stable_sample_times
        if self._stable_samples < _STABILITY_SAMPLES or len(times) < _STABILITY_SAMPLES:
            return False
        if times[-1] - times[0] < _STABILITY_SECONDS:
            return False
        if self._latest_temperature_at is None or self._last_stable_sample_at is None:
            return False
        freshness = self._stability_freshness_bound()
        if now - self._latest_temperature_at > freshness:
            return False
        if now - self._last_stable_sample_at > freshness:
            return False
        return all(
            1.0 <= later - earlier <= self._consecutive_sample_max_gap()
            for earlier, later in zip(
                self._stable_sample_times, self._stable_sample_times[1:]
            )
        )

    @staticmethod
    def _positive_finite(value: Real, label: str) -> float:
        if isinstance(value, bool) or not isinstance(value, Real):
            raise ValueError(f"{label} must be finite and positive")
        numeric = float(value)
        if not math.isfinite(numeric) or numeric <= 0.0:
            raise ValueError(f"{label} must be finite and positive")
        return numeric

    def _discover_port(self) -> str:
        """Prefer the stable USB serial; only fall back when it is absent."""
        ports = [
            item
            for item in self._ports_provider()
            if getattr(item, "vid", None) == CP210X_VID
            and getattr(item, "pid", None) == CP210X_PID
        ]
        if self.usb_serial is not None:
            preferred = [
                item for item in ports
                if getattr(item, "serial_number", None) == self.usb_serial
            ]
            if len(preferred) == 1:
                return str(preferred[0].device)
            if len(preferred) > 1:
                devices = ", ".join(str(item.device) for item in preferred)
                raise InstrumentConnectionError(
                    f"Expected exactly one CP210x serial={self.usb_serial}; found {devices}"
                )
        # No matching preferred serial exists, so and only so select a unique VID/PID.
        return find_serial_port(
            CP210X_VID, CP210X_PID, ports_provider=lambda: ports
        )

    def connect(self) -> "GainDriver":
        with self._lifecycle_lock:
            if self.state in {DriverState.READY, DriverState.ACTIVE}:
                return self
            if self._watchdog_thread is not None and self._watchdog_thread.is_alive():
                raise DeviceFault("Cannot connect while a previous Gain watchdog is still alive")
            if self.state == DriverState.FAULT:
                raise InstrumentSafetyError("Gain driver is faulted; close it before reconnecting")
            if self.state != DriverState.DISCONNECTED:
                raise InstrumentSafetyError(
                    f"Cannot connect while Gain driver is {self.state.name}"
                )
            self.state = DriverState.CONNECTING
            self.status = None
            self.cleanup_error = None
            self.fault_error = None
            self._shutdown_current_confirmed = False
            self._shutdown_tec_confirmed = False
            self._serial_closed = False
            self._shutdown_error = None
            with self._watchdog_condition:
                self._latest_temperature = None
                self._latest_temperature_at = None
                self._clear_stability_locked()
                self._moderate_deviation_samples = 0
                self._last_moderate_sample_at = None
                self._read_failures = 0
                self._last_read_failure_at = None
                self._stop_watchdog.clear()
            try:
                self.port = (
                    self._configured_port
                    if self._configured_port is not None
                    else self._discover_port()
                )
                serial_port = self._serial_factory(
                    port=self.port,
                    baudrate=115200,
                    bytesize=8,
                    parity="N",
                    stopbits=1,
                    timeout=self.io_timeout,
                    write_timeout=self.io_timeout,
                )
                self._serial = serial_port
                if not getattr(serial_port, "is_open", True):
                    raise InstrumentConnectionError(f"Failed to open Gain driver on {self.port}")
                temperature = self._read_float("RDTA", "T")
                target = self._read_float("RDEA", "E")
                tec_enabled = self._read_bool("RDRA", "R")
                current = self._read_float("RDCA", "C")
                current_enabled = self._read_bool("RDQA", "Q")
                self.status = GainStatus(
                    temperature_c=temperature,
                    target_c=target,
                    tec_enabled=tec_enabled,
                    current_ma=current,
                    current_enabled=current_enabled,
                    received_at=self._clock(),
                )
                self.state = DriverState.ACTIVE if current_enabled else DriverState.READY
                with self._watchdog_condition:
                    self._latest_temperature = temperature
                    self._latest_temperature_at = self.status.received_at
                    self._watchdog_condition.notify_all()
                self._enforce_current_requires_tec(self.status, during_connect=True)
                if self.start_watchdog:
                    self._start_watchdog()
                self.log.info("Gain driver connected: port=%s", self.port)
                return self
            except BaseException as error:
                self.log.error("Gain driver connection failed: port=%s error=%s", self.port, type(error).__name__)
                if self._connect_invariant_fault:
                    # The invariant trip has already attempted Q=0 then D=0.
                    # It is still a failed connect, so release the transport
                    # now; retain FAULT only if that release is unconfirmed.
                    self._connect_invariant_fault = False
                    shutdown_confirmed = (
                        self._shutdown_current_confirmed
                        and self._shutdown_tec_confirmed
                    )
                    if shutdown_confirmed:
                        cleanup_error = self._close_serial()
                    else:
                        # Reissue both commands on an explicit retry.  A stale
                        # single confirmation cannot justify consuming only one
                        # half of the next fail-safe Q=0 -> D=0 sequence.
                        self._shutdown_current_confirmed = False
                        self._shutdown_tec_confirmed = False
                        cleanup_error = self._shutdown_error or DeviceFault(
                            "Gain invariant shutdown was not fully confirmed"
                        )
                    if cleanup_error is None:
                        self.status = None
                        self._serial = None
                        self.state = DriverState.DISCONNECTED
                    else:
                        self.cleanup_error = cleanup_error
                        self.fault_error = self.fault_error or cleanup_error
                        self.state = DriverState.FAULT
                else:
                    self._stop_watchdog.set()
                    cleanup_error = self._connect_cleanup_best_effort()
                    if cleanup_error is None:
                        self.status = None
                        self._serial = None
                        self.state = DriverState.DISCONNECTED
                    else:
                        self.cleanup_error = cleanup_error
                        self.fault_error = self.fault_error or cleanup_error
                        self.state = DriverState.FAULT
                if isinstance(error, (DriverError, KeyboardInterrupt, SystemExit)):
                    raise
                raise InstrumentConnectionError(f"Failed to connect Gain driver on {self.port}") from error

    def _request(self, command: bytes, field: str | None) -> str:
        """Write exactly one command and consume exactly its CRLF reply."""
        with self._request_lock:
            serial_port = self._serial
            if serial_port is None or not getattr(serial_port, "is_open", True):
                raise InstrumentConnectionError("Gain driver serial port is not open")
            try:
                count = serial_port.write(command)
            except Exception as error:
                raise InstrumentConnectionError("Gain command write failed") from error
            if isinstance(count, bool) or not isinstance(count, Integral) or count != len(command):
                raise InstrumentConnectionError(
                    f"Gain command write returned {count!r}; expected {len(command)} bytes"
                )
            try:
                line = serial_port.read_until(b"\r\n")
            except Exception as error:
                raise InstrumentConnectionError("Gain command read failed") from error
            return parse_ack(line) if field is None else parse_reply(line, field)

    @staticmethod
    def _float_value(value: str, field: str) -> float:
        if value != value.strip():
            raise InstrumentProtocolError(f"Gain reply field {field} contains surrounding whitespace")
        try:
            numeric = float(value)
        except (TypeError, ValueError) as error:
            raise InstrumentProtocolError(f"Gain reply field {field} is not numeric") from error
        if not math.isfinite(numeric):
            raise InstrumentProtocolError(f"Gain reply field {field} is not finite")
        limits = _FLOAT_REPLY_LIMITS.get(field)
        if limits is not None and not limits[0] <= numeric <= limits[1]:
            raise InstrumentProtocolError(
                f"Gain reply field {field} {numeric!r} is outside {limits[0]}-{limits[1]}"
            )
        return numeric

    @staticmethod
    def _bool_value(value: str, field: str) -> bool:
        if value == "0":
            return False
        if value == "1":
            return True
        raise InstrumentProtocolError(f"Gain reply field {field} must be literal 0 or 1")

    def _read_float(self, command: str, field: str) -> float:
        return self._float_value(self._request(build_command(command), field), field)

    def _read_bool(self, command: str, field: str) -> bool:
        return self._bool_value(self._request(build_command(command), field), field)

    def _require_ready(self, action: str) -> GainStatus:
        require_state(self.state, {DriverState.READY, DriverState.ACTIVE}, action)
        if self.status is None:
            raise InstrumentConnectionError("Gain driver has no confirmed status")
        return self.status

    def _enforce_current_requires_tec(
        self, status: GainStatus, *, during_connect: bool = False
    ) -> None:
        """Trip immediately whenever a confirmed snapshot says Q=1 while TEC is off."""
        if not status.current_enabled or status.tec_enabled:
            return
        cause = DeviceFault("Gain current output is enabled while TEC is disabled")
        if during_connect:
            self._connect_invariant_fault = True
        self._severe_trip(cause)
        raise cause

    def _sync_observed_output_state(
        self,
        status: GainStatus,
        *,
        tec_enabled: bool | None = None,
        current_enabled: bool | None = None,
    ) -> GainStatus:
        """Commit a confirmed D/Q observation and keep state and safety invariant aligned."""
        next_status = replace(
            status,
            tec_enabled=status.tec_enabled if tec_enabled is None else tec_enabled,
            current_enabled=status.current_enabled if current_enabled is None else current_enabled,
        )
        self.status = next_status
        if next_status.tec_enabled != status.tec_enabled:
            with self._watchdog_condition:
                self._clear_stability_locked()
                self._watchdog_condition.notify_all()
        self._enforce_current_requires_tec(next_status)
        if self.state in {DriverState.READY, DriverState.ACTIVE}:
            self.state = DriverState.ACTIVE if next_status.current_enabled else DriverState.READY
        return next_status

    def read_status(self) -> GainStatus:
        with self._request_lock:
            return self._require_ready("read Gain status")

    def read_temperature(self) -> float:
        with self._request_lock:
            status = self._require_ready("read Gain temperature")
            value = self._read_float("RDTA", "T")
            self.status = replace(status, temperature_c=value, received_at=self._clock())
            return value

    def read_target(self) -> float:
        with self._request_lock:
            status = self._require_ready("read Gain target temperature")
            value = self._read_float("RDEA", "E")
            self.status = replace(status, target_c=value)
            if value != status.target_c:
                with self._watchdog_condition:
                    self._clear_stability_locked()
                    self._watchdog_condition.notify_all()
            return value

    def read_tec_enabled(self) -> bool:
        with self._request_lock:
            status = self._require_ready("read Gain TEC state")
            value = self._read_bool("RDRA", "R")
            self._sync_observed_output_state(status, tec_enabled=value)
            return value

    def read_current(self) -> float:
        with self._request_lock:
            status = self._require_ready("read Gain current")
            value = self._read_float("RDCA", "C")
            self.status = replace(status, current_ma=value)
            return value

    def read_current_enabled(self) -> bool:
        with self._request_lock:
            status = self._require_ready("read Gain current-output state")
            value = self._read_bool("RDQA", "Q")
            self._sync_observed_output_state(status, current_enabled=value)
            return value

    def set_temperature(self, value: float) -> float:
        frame = build_command("STEA", value, 15.0, 40.0, "temperature")
        with self._request_lock:
            status = self._require_ready("set Gain target temperature")
            applied = self._float_value(self._request(frame, "E"), "E")
            self.status = replace(status, target_c=applied)
            with self._watchdog_condition:
                self._clear_stability_locked()
                self._watchdog_condition.notify_all()
            return applied

    def set_current(self, value: float) -> float:
        frame = build_command("STCA", value, 0.0, 200.0, "current")
        with self._request_lock:
            status = self._require_ready("set Gain current")
            applied = self._float_value(self._request(frame, "C"), "C")
            self.status = replace(status, current_ma=applied)
            return applied

    def _set_bool(self, command: str, field: str, enabled: bool, action: str) -> bool:
        with self._request_lock:
            status = self._require_ready(action)
            applied = self._bool_value(self._request(build_command(command, enabled), field), field)
            if applied is not enabled:
                raise InstrumentProtocolError(f"Gain reply field {field} did not confirm {int(enabled)}")
            if field == "D":
                self.status = replace(status, tec_enabled=applied)
                self._shutdown_tec_confirmed = not applied
                if applied != status.tec_enabled:
                    with self._watchdog_condition:
                        self._clear_stability_locked()
                        self._watchdog_condition.notify_all()
            else:
                self.status = replace(status, current_enabled=applied)
                self._shutdown_current_confirmed = not applied
                if applied and self.state == DriverState.READY:
                    self.state = DriverState.ACTIVE
                elif not applied and self.state == DriverState.ACTIVE:
                    self.state = DriverState.READY
            self._enforce_current_requires_tec(self.status)
            return applied

    def enable_tec(self) -> bool:
        return self._set_bool("STRA", "D", True, "enable Gain TEC")

    def disable_tec(self) -> bool:
        with self._request_lock:
            status = self._require_ready("disable Gain TEC")
            if status.current_enabled:
                try:
                    current_off = self._bool_value(
                        self._request(build_command("STQA", False), "Q"), "Q"
                    )
                    if current_off:
                        raise InstrumentProtocolError("Gain reply field Q did not confirm shutdown")
                    self._shutdown_current_confirmed = True
                    self.status = replace(status, current_enabled=False)
                    self.state = DriverState.READY
                except BaseException as error:
                    # Current state is uncertain: latch FAULT and still issue the
                    # complete Q=0 -> D=0 fail-safe sequence.
                    self._severe_trip(error)
                    raise
            # Keep this reentrant call inside the same request lock: no current
            # enable/recheck may interleave between confirmed Q=0 and D=0.
            return self._set_bool("STRA", "D", False, "disable Gain TEC")

    def disable_current(self) -> bool:
        return self._set_bool("STQA", "Q", False, "disable Gain current output")

    def wait_stable(self, timeout: float = 60.0) -> None:
        """Wait for six watchdog samples spanning five seconds within tolerance."""
        if isinstance(timeout, bool):
            raise ValueError("timeout must be finite and non-negative")
        try:
            duration = float(timeout)
        except (TypeError, ValueError) as error:
            raise ValueError("timeout must be finite and non-negative") from error
        if not math.isfinite(duration) or duration < 0.0:
            raise ValueError("timeout must be finite and non-negative")
        deadline = self._clock() + duration
        with self._watchdog_condition:
            while True:
                if self.state == DriverState.FAULT:
                    raise DeviceFault("Gain driver faulted while waiting for temperature stabilization") from self.fault_error
                if self.state not in {DriverState.READY, DriverState.ACTIVE}:
                    raise InstrumentSafetyError(
                        f"Cannot wait for Gain temperature stabilization while driver state is {self.state.name}"
                    )
                if self._stability_ready_locked(self._clock()):
                    return
                remaining = deadline - self._clock()
                if remaining <= 0.0:
                    raise InstrumentTimeoutError("Gain temperature did not stabilize before timeout")
                self._watchdog_condition.wait(remaining)

    def enable_current(self) -> bool:
        """Enable drive current only after watchdog-confirmed stabilization."""
        with self._request_lock:
            status = self.status
            if self.state == DriverState.ACTIVE:
                if status is not None and status.current_enabled:
                    return True
                raise InstrumentSafetyError("Gain ACTIVE state lacks confirmed current-output state")
            if self.state != DriverState.READY:
                raise InstrumentSafetyError(
                    f"Cannot enable Gain current output while driver state is {self.state.name}; allowed states: READY"
                )
            if status is None:
                raise InstrumentConnectionError("Gain driver has no confirmed status")
            with self._watchdog_condition:
                epoch = self._stability_epoch
                if not self._stability_ready_locked(self._clock()):
                    raise InstrumentSafetyError("Gain temperature stability evidence is stale or incomplete")
            tec_enabled = self._read_bool("RDRA", "R")
            current_ma = self._read_float("RDCA", "C")
            temperature = self._read_float("RDTA", "T")
            received_at = self._clock()
            with self._watchdog_condition:
                self._latest_temperature = temperature
                self._latest_temperature_at = received_at
                evidence_is_valid = (
                    self._stability_epoch == epoch
                    and self._stability_ready_locked(received_at)
                )
                # Keep the condition through the final output command and
                # commit.  Terminal paths publish state under this same lock
                # before they wait for _request_lock, so Q=1 has one clear
                # linearization point relative to close and severe faults.
                if self.state != DriverState.READY or self._stop_watchdog.is_set():
                    raise InstrumentSafetyError("Gain driver entered a terminal state during current enable")
                if self._stability_epoch != epoch or not evidence_is_valid:
                    raise InstrumentSafetyError("Gain temperature stability epoch changed or became stale")
                if not tec_enabled:
                    raise InstrumentSafetyError("Gain TEC must be enabled before enabling current")
                if not 0.0 <= current_ma <= 200.0:
                    raise InstrumentSafetyError("Gain current setpoint is outside 0-200 mA")
                if abs(temperature - status.target_c) > 0.2:
                    raise InstrumentSafetyError("Gain temperature is not within target +/-0.2 degC")
                try:
                    applied = self._bool_value(
                        self._request(build_command("STQA", True), "Q"), "Q"
                    )
                    if not applied:
                        raise InstrumentProtocolError("Gain reply field Q did not confirm 1")
                    # This controller resets its current setpoint to 3 mA when
                    # Q transitions on.  The pre-enable RDCA value is therefore
                    # not authoritative after STQA=1.
                    enabled_current_ma = self._read_float("RDCA", "C")
                except BaseException as error:
                    # Once STQA=1 has been attempted, a transport or protocol
                    # failure leaves output state unknown.  READY is no longer
                    # a truthful state; fail safe before surfacing the cause.
                    self._severe_trip(error)
                    raise
                self._shutdown_current_confirmed = False
                self.status = replace(
                    status,
                    temperature_c=temperature,
                    tec_enabled=tec_enabled,
                    current_ma=enabled_current_ma,
                    current_enabled=True,
                    received_at=received_at,
                )
                self.state = DriverState.ACTIVE
                return True

    def ramp_current(
        self,
        target_ma: float,
        *,
        step_ma: float = 1.0,
        interval_s: float = 0.1,
    ) -> float:
        """Ramp an enabled output to a target using bounded, confirmed steps."""

        if isinstance(target_ma, bool) or not isinstance(target_ma, Real):
            raise ValueError("target current must be finite and within 0-200 mA")
        target = float(target_ma)
        if not math.isfinite(target) or not 0.0 <= target <= 200.0:
            raise ValueError("target current must be finite and within 0-200 mA")
        step = self._positive_finite(step_ma, "current ramp step")
        if step > 1.0:
            raise ValueError("current ramp step must be no greater than 1.0 mA")
        interval = self._positive_finite(interval_s, "current ramp interval")
        if interval < 0.05:
            raise ValueError("current ramp interval must be at least 0.05 s")

        status = self.read_status()
        if self.state != DriverState.ACTIVE or not status.current_enabled:
            raise InstrumentSafetyError("Gain current output must be enabled before ramping")
        if not status.tec_enabled:
            raise InstrumentSafetyError("Gain TEC must remain enabled while ramping current")

        start = status.current_ma
        distance = abs(target - start)
        count = int(math.ceil(distance / step)) if distance else 0
        direction = 1.0 if target >= start else -1.0
        try:
            for index in range(1, count + 1):
                self._sleep(interval)
                status = self.read_status()
                if (
                    self.state != DriverState.ACTIVE
                    or not status.current_enabled
                    or not status.tec_enabled
                ):
                    raise InstrumentSafetyError(
                        "Gain output state changed during current ramp"
                    )
                requested = (
                    target
                    if index == count
                    else start + direction * min(distance, index * step)
                )
                self.set_current(requested)

            final_current = self.read_current()
            final_enabled = self.read_current_enabled()
            final_tec = self.read_tec_enabled()
            final_temperature = self.read_temperature()
            final_status = self.read_status()
            if not math.isclose(final_current, target, rel_tol=0.0, abs_tol=0.0005):
                raise InstrumentProtocolError(
                    f"Gain final current {final_current!r} did not confirm target {target!r}"
                )
            if not final_enabled or not final_tec:
                raise InstrumentSafetyError("Gain output state changed during final ramp verification")
            if abs(final_temperature - final_status.target_c) > 0.2:
                raise InstrumentSafetyError(
                    "Gain temperature left target +/-0.2 degC during current ramp"
                )
            return final_current
        except BaseException as error:
            self._severe_trip(error)
            raise

    def _start_watchdog(self) -> None:
        """Start one daemon watchdog after a successful connection."""
        if self._watchdog_thread is not None and self._watchdog_thread.is_alive():
            return
        self._stop_watchdog.clear()
        self._watchdog_thread = threading.Thread(
            target=self._watchdog_loop,
            name="gain-watchdog",
            daemon=True,
        )
        self._watchdog_thread.start()

    def _watchdog_loop(self) -> None:
        next_deadline = self._clock() + self.poll_interval
        while True:
            remaining = max(0.0, next_deadline - self._clock())
            if self._stop_watchdog.wait(remaining):
                return
            try:
                self._watchdog_iteration()
            except BaseException as error:
                # A watchdog must convert every unexpected polling failure into
                # a counted safety event rather than silently dying.
                self._record_watchdog_failure(error)
            if self._stop_watchdog.is_set():
                return
            next_deadline += self.poll_interval
            now = self._clock()
            if next_deadline <= now:
                missed = int((now - next_deadline) // self.poll_interval) + 1
                next_deadline += missed * self.poll_interval

    def _watchdog_stopping(self) -> bool:
        return self._stop_watchdog.is_set() or self.state in {
            DriverState.CLOSING,
            DriverState.DISCONNECTED,
        }

    def _watchdog_iteration(self) -> None:
        """Poll one temperature sample and apply the watchdog thresholds."""
        if self._watchdog_stopping():
            return
        try:
            # Keep one request lock from RDTA through condition processing.
            # Target/TEC readers and setters acquire the same lock before they
            # reset an epoch, so an old temperature cannot enter a new epoch.
            with self._request_lock:
                status = self._require_ready("watch Gain temperature")
                temperature = self._read_float("RDTA", "T")
                sample_at = self._clock()
                status = replace(status, temperature_c=temperature, received_at=sample_at)
                self.status = status
                deviation = abs(temperature - status.target_c)
                severe_cause: BaseException | None = None
                disable_current = False
                with self._watchdog_condition:
                    if self._watchdog_stopping():
                        return
                    self._latest_temperature = temperature
                    self._latest_temperature_at = sample_at
                    self._read_failures = 0
                    self._last_read_failure_at = None
                    if not status.tec_enabled or deviation > 0.2:
                        self._clear_stability_locked()
                    elif self._last_stable_sample_at is None:
                        self._stable_sample_times.append(sample_at)
                        self._last_stable_sample_at = sample_at
                        self._stable_sample_times = self._stable_sample_times[-_STABILITY_SAMPLES:]
                        self._stable_samples = len(self._stable_sample_times)
                    elif sample_at - self._last_stable_sample_at > self._consecutive_sample_max_gap():
                        self._clear_stability_locked()
                        self._stable_sample_times.append(sample_at)
                        self._last_stable_sample_at = sample_at
                        self._stable_samples = 1
                    elif sample_at - self._last_stable_sample_at >= 1.0:
                        self._stable_sample_times.append(sample_at)
                        self._last_stable_sample_at = sample_at
                        self._stable_sample_times = self._stable_sample_times[-_STABILITY_SAMPLES:]
                        self._stable_samples = len(self._stable_sample_times)
                    else:
                        self._stable_samples = len(self._stable_sample_times)
                    if deviation > 1.0:
                        if (
                            self._last_moderate_sample_at is None
                            or sample_at - self._last_moderate_sample_at > self._consecutive_sample_max_gap()
                        ):
                            self._moderate_deviation_samples = 1
                            self._last_moderate_sample_at = sample_at
                        elif sample_at - self._last_moderate_sample_at >= 1.0:
                            self._moderate_deviation_samples += 1
                            self._last_moderate_sample_at = sample_at
                    else:
                        self._moderate_deviation_samples = 0
                        self._last_moderate_sample_at = None
                    if deviation > 3.0:
                        severe_cause = DeviceFault(
                            f"Gain temperature deviation {deviation:.3f} degC exceeds 3.0 degC"
                        )
                    elif self._moderate_deviation_samples == 3:
                        disable_current = True
                    self._watchdog_condition.notify_all()
        except BaseException as error:
            if not self._watchdog_stopping():
                self._record_watchdog_failure(error)
            return
        if severe_cause is not None:
            self._severe_trip(severe_cause)
        elif disable_current:
            try:
                self.disable_current()
            except BaseException as error:
                self._severe_trip(error)

    def _record_watchdog_failure(self, error: BaseException) -> None:
        """Count one failed watchdog request and trip after three in a row."""
        if self._watchdog_stopping():
            return
        trip = False
        with self._watchdog_condition:
            if self._watchdog_stopping():
                return
            self._clear_stability_locked()
            # A failure is one watchdog request attempt.  Do not use thermal
            # sample timing here: serial timeouts and scheduling gaps must not
            # let a broken transport avoid the three-attempt severe trip.
            self._read_failures += 1
            self._last_read_failure_at = self._clock()
            trip = self._read_failures >= 3
            self._watchdog_condition.notify_all()
        self.log.error("Gain watchdog temperature read failed: %s", type(error).__name__)
        if trip:
            self._severe_trip(error)

    def _severe_trip(self, cause: BaseException) -> None:
        """Latch a fault and attempt both shutdown commands in safe order."""
        with self._watchdog_condition:
            if self._watchdog_stopping():
                return
            if self.state == DriverState.FAULT:
                return
            self.fault_error = cause
            self.state = DriverState.FAULT
            self._stop_watchdog.set()
            self._watchdog_condition.notify_all()
        self.log.error("Gain severe watchdog fault: %s", cause)
        with self._request_lock:
            for command, field, attribute in (
                (build_command("STQA", False), "Q", "current_enabled"),
                (build_command("STRA", False), "D", "tec_enabled"),
            ):
                try:
                    applied = self._bool_value(self._request(command, field), field)
                    if applied:
                        raise InstrumentProtocolError(
                            f"Gain reply field {field} did not confirm shutdown"
                        )
                    if attribute == "current_enabled":
                        self._shutdown_current_confirmed = True
                    else:
                        self._shutdown_tec_confirmed = True
                    if self.status is not None:
                        self.status = replace(self.status, **{attribute: False})
                except BaseException as error:
                    # Do not replace fault_error: it is the causal evidence for
                    # the latched fault.  Continue so TEC shutdown is attempted.
                    if self._shutdown_error is None:
                        self._shutdown_error = error
                    self.log.error("Gain severe shutdown command failed: %s", type(error).__name__)

    def read_pid(self) -> tuple[float, float, float]:
        with self._request_lock:
            self._require_ready("read Gain PID")
            return (
                self._read_float("RDPA", "P"),
                self._read_float("RDIA", "I"),
                self._read_float("RDDA", "D"),
            )

    def set_pid(self, p: float, i: float, d: float) -> tuple[float, float, float]:
        frames = (
            (build_command("STPA", p, 0.0, 999.999, "PID P"), "P"),
            (build_command("STIA", i, 0.0, 999.999, "PID I"), "I"),
            (build_command("STDA", d, 0.0, 999.999, "PID D"), "D"),
        )
        with self._request_lock:
            self._require_ready("set Gain PID")
            applied = tuple(
                self._float_value(self._request(frame, field), field)
                for frame, field in frames
            )
            return applied  # Status deliberately has no PID fields to replace.

    def reset_pid(self) -> tuple[float, float, float]:
        with self._request_lock:
            self._require_ready("reset Gain PID")
            self._request(build_command("RST"), None)
            return self.read_pid()

    def clear_integral(self) -> None:
        with self._request_lock:
            self._require_ready("clear Gain PID integral")
            self._request(build_command("CLR"), None)

    def _connect_cleanup_best_effort(self) -> BaseException | None:
        """Clean a partially opened connection without lying about a failed close."""
        if self._serial is None:
            return None
        output_error = self._close_output_steps()
        serial_error = self._close_serial()
        return output_error or serial_error

    def _shutdown_serial_best_effort(self) -> BaseException | None:
        """Try safety commands in current-before-TEC order, then always close."""
        primary: BaseException | None = None
        if self._serial is not None and getattr(self._serial, "is_open", True):
            for command, field in ((build_command("STQA", False), "Q"), (build_command("STRA", False), "D")):
                try:
                    self._request(command, field)
                except BaseException as error:
                    if primary is None:
                        primary = error
                    self.log.error("Gain shutdown command failed: %s", type(error).__name__)
            try:
                self._serial.close()
            except BaseException as error:
                if primary is None:
                    primary = error
                self.log.error("Gain serial close failed: %s", type(error).__name__)
        self._serial = None
        return primary

    @staticmethod
    def _typed_cleanup_error(error: BaseException, step: str) -> BaseException:
        """Preserve interruptions, but type ordinary transport cleanup failures."""
        if isinstance(error, (DriverError, KeyboardInterrupt, SystemExit)):
            return error
        wrapped = InstrumentConnectionError(f"Gain {step} failed")
        wrapped.__cause__ = error
        return wrapped

    def _close_output_steps(self) -> BaseException | None:
        """Confirm current-off then TEC-off without allowing one failure to skip the next."""
        primary: BaseException | None = None
        steps = (
            ("current shutdown", build_command("STQA", False), "Q", "current_enabled", "_shutdown_current_confirmed"),
            ("TEC shutdown", build_command("STRA", False), "D", "tec_enabled", "_shutdown_tec_confirmed"),
        )
        with self._request_lock:
            for name, command, field, attribute, confirmed_name in steps:
                if getattr(self, confirmed_name):
                    continue
                try:
                    applied = self._bool_value(self._request(command, field), field)
                    if applied:
                        raise InstrumentProtocolError(
                            f"Gain reply field {field} did not confirm shutdown"
                        )
                    setattr(self, confirmed_name, True)
                    if self.status is not None:
                        self.status = replace(self.status, **{attribute: False})
                except BaseException as error:
                    typed = self._typed_cleanup_error(error, name)
                    if primary is None:
                        primary = typed
                    self.log.error("Gain %s failed: %s", name, type(typed).__name__)
        return primary

    def _close_serial(self) -> BaseException | None:
        """Close the transport exactly once after output shutdown attempts."""
        if self._serial is None or self._serial_closed:
            return None
        try:
            self._serial.close()
        except BaseException as error:
            typed = self._typed_cleanup_error(error, "serial close")
            self.log.error("Gain serial close failed: %s", type(typed).__name__)
            return typed
        self._serial_closed = True
        return None

    def close(self) -> None:
        with self._lifecycle_lock:
            if self.state == DriverState.DISCONNECTED:
                return
            with self._watchdog_condition:
                # Publish CLOSING, cancellation, and the condition wake-up as
                # one transaction so wait_stable cannot miss teardown.
                self.state = DriverState.CLOSING
                self._stop_watchdog.set()
                self._watchdog_condition.notify_all()
            error = self._close_output_steps()
            watchdog = self._watchdog_thread
            if watchdog is threading.current_thread():
                watchdog_error: BaseException | None = DeviceFault(
                    "Gain watchdog cannot join itself during cleanup"
                )
            else:
                watchdog_error = None
                if watchdog is not None:
                    try:
                        watchdog.join(self.io_timeout + self.poll_interval + 1.0)
                        if watchdog.is_alive():
                            watchdog_error = DeviceFault(
                                "Gain watchdog did not stop during bounded cleanup"
                            )
                    except BaseException as join_error:
                        watchdog_error = self._typed_cleanup_error(
                            join_error, "watchdog join"
                        )
            if error is None and watchdog_error is not None:
                error = watchdog_error
            serial_error = self._close_serial()
            if error is None and serial_error is not None:
                error = serial_error
            if error is not None:
                self.cleanup_error = error
                self.fault_error = self.fault_error or error
                with self._watchdog_condition:
                    self.state = DriverState.FAULT
                    self._watchdog_condition.notify_all()
                raise error
            self.status = None
            self._serial = None
            self._watchdog_thread = None
            self.cleanup_error = None
            self.state = DriverState.DISCONNECTED
            self.log.info("Gain driver shutdown complete: port=%s", self.port)

    def __enter__(self) -> "GainDriver":
        return self.connect()

    def __exit__(self, exc_type, exc, traceback) -> bool:
        try:
            self.close()
        except BaseException as cleanup_error:
            self.cleanup_error = cleanup_error
            if exc_type is None:
                raise
        return False
