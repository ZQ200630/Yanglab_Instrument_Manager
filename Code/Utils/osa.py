from __future__ import annotations

import math
from dataclasses import dataclass
from datetime import datetime, timezone
from numbers import Integral, Real
from threading import Event, Lock, RLock, Thread
from time import monotonic
from typing import Callable, Sequence

import numpy as np
import pyvisa
from pymeasure.instruments.yokogawa.aq6370series import AQ6370Series
from pyvisa.constants import StatusCode
from pyvisa.errors import InvalidSession, VisaIOError

from .common import (
    DriverError,
    DriverState,
    InstrumentConnectionError,
    InstrumentProtocolError,
    InstrumentTimeoutError,
    get_logger,
    require_state,
    ProbeReport, _identity_probe, _probe_connect_guard,
)
from .visa import acquire_visa_manager
from .osa_trace import (TraceCapture, TraceContext, decode_trace_reply,
                        TRACE_CHUNK_POINTS, MAX_TRACE_REPLY_BYTES)


@dataclass(frozen=True)
class Spectrum:
    wavelength_nm: np.ndarray
    power_dbm: np.ndarray
    trace: str
    acquired_at: datetime
    identity: str

    def __post_init__(self) -> None:
        try:
            wavelength = np.array(self.wavelength_nm, dtype=float, copy=True)
            power = np.array(self.power_dbm, dtype=float, copy=True)
        except (TypeError, ValueError) as error:
            raise InstrumentProtocolError("OSA trace contains non-numeric data") from error
        if wavelength.ndim != 1 or power.ndim != 1 or not len(wavelength):
            raise InstrumentProtocolError("OSA trace arrays must be non-empty and one-dimensional")
        if wavelength.shape != power.shape:
            raise InstrumentProtocolError("OSA wavelength and power arrays have different lengths")
        if not np.all(np.isfinite(wavelength)) or not np.all(np.isfinite(power)):
            raise InstrumentProtocolError("OSA trace contains non-finite values")
        if len(wavelength) > 1 and not np.all(np.diff(wavelength) > 0):
            raise InstrumentProtocolError("OSA wavelengths are not strictly increasing")
        immutable_wavelength = np.frombuffer(wavelength.tobytes(), dtype=wavelength.dtype).reshape(
            wavelength.shape
        )
        immutable_power = np.frombuffer(power.tobytes(), dtype=power.dtype).reshape(power.shape)
        object.__setattr__(self, "wavelength_nm", immutable_wavelength)
        object.__setattr__(self, "power_dbm", immutable_power)

    @classmethod
    def create(
        cls,
        wavelength_nm: Sequence[float],
        power_dbm: Sequence[float],
        trace: str,
        identity: str,
    ) -> "Spectrum":
        return cls(wavelength_nm, power_dbm, trace, datetime.now(timezone.utc), identity)


class _CleanupHelper:
    """Retained ownership of one backend call, even after its caller times out."""

    def __init__(self, kind: str, subject: object, action: Callable[[], object]):
        self.kind = kind
        self.subject = subject
        self.done = Event()
        self.error: BaseException | None = None
        self.thread = Thread(target=self._run, args=(action,), name=f"osa-{kind}", daemon=True)

    def _run(self, action: Callable[[], object]) -> None:
        try:
            action()
        except BaseException as error:
            self.error = error
        finally:
            self.done.set()


class AQ6370:
    """Acquire without changing front-panel measurement settings.

    close_timeout bounds the entire close call, not the backend's lifetime.
    Failed or unfinished cleanup retains ownership in FAULT; cleanup_error
    records its cause and a later close explicitly retries or reaps it.
    A failed retry abort stops sweeps; the propagated operation exception
    carries abort_error while retaining the existing timeout/transport cause.
    """

    def __init__(
        self,
        resource_name: str = "GPIB0::4::INSTR",
        timeout: float = 30.0,
        retries: int = 2,
        resource_manager_factory: Callable[[], object] = pyvisa.ResourceManager,
        instrument_factory: Callable[[object], object] = AQ6370Series,
        *,
        resource_manager: object | None = None,
        close_timeout: float = 2.0,
    ) -> None:
        for name, value in (("timeout", timeout), ("close_timeout", close_timeout)):
            if (isinstance(value, bool) or not isinstance(value, Real)
                    or not math.isfinite(value) or value <= 0):
                raise ValueError(f"{name} must be finite and positive")
        if isinstance(retries, bool) or not isinstance(retries, Integral) or retries < 0:
            raise ValueError("retries must be a non-negative integer")
        self.resource_name = resource_name
        self.timeout = float(timeout)
        self.close_timeout = float(close_timeout)
        self.retries = int(retries)
        self._resource_manager_factory = resource_manager_factory
        self._borrowed_manager = resource_manager
        self._instrument_factory = instrument_factory
        self._manager = None
        self._lease = None
        self._reservation = None
        self._resource = None
        self._instrument = None
        self._lifecycle_lock = RLock()
        self._transaction_lock = Lock()
        self._abort_lock = Lock()
        self._close_lock = Lock()
        self._close_callers = 0
        self._connect_done = Event()
        self._connect_done.set()
        self._acquire_done = Event()
        self._acquire_done.set()
        self._cancel_generation = 0
        self._closing = False
        self._sweep_owned = False
        self._cleanup_helpers: dict[str, _CleanupHelper] = {}
        self.cleanup_error: BaseException | None = None
        self.state = DriverState.DISCONNECTED
        self.identity = ""
        self.log = get_logger("osa")

    def _check_generation(self, generation: int) -> None:
        with self._lifecycle_lock:
            if self._closing or generation != self._cancel_generation:
                raise InstrumentConnectionError("OSA operation was cancelled")

    def connect(self) -> "AQ6370":
        return self._connect()

    @property
    def has_resource_responsibility(self) -> bool:
        with self._lifecycle_lock:
            return (self.state is not DriverState.DISCONNECTED or self._resource is not None
                    or self._lease is not None or self._reservation is not None
                    or bool(self._cleanup_helpers))

    def probe_identity(self) -> ProbeReport:
        def read():
            fields = self.identity.split(",")
            if len(fields) != 4:
                raise InstrumentProtocolError("OSA identity must contain four fields")
            return dict(zip(("manufacturer", "model", "serial", "firmware"),
                            (field.strip() for field in fields))), {}
        return _identity_probe(self, lambda: self._connect(identity_only=True), read)

    def _connect(self, *, identity_only=False) -> "AQ6370":
        with self._lifecycle_lock:
            _probe_connect_guard(self)
            if self._closing:
                raise InstrumentConnectionError("OSA cleanup must finish before connecting")
            if self.state in {DriverState.READY, DriverState.ACTIVE}:
                return self
            require_state(self.state, {DriverState.DISCONNECTED}, "connect OSA")
            self.state = DriverState.CONNECTING
            self._sweep_owned = False
            generation = self._cancel_generation
            self._connect_done.clear()
        try:
            try:
                lease = acquire_visa_manager(
                    resource_manager=self._borrowed_manager,
                    factory=self._resource_manager_factory,
                )
                with self._lifecycle_lock:
                    self._lease = lease
                    self._manager = lease.manager
                self._check_generation(generation)
                reservation = lease.reserve_resource(self.resource_name)
                with self._lifecycle_lock:
                    self._reservation = reservation
                self._check_generation(generation)
                resource = lease.manager.open_resource(reservation.canonical_name)
                with self._lifecycle_lock:
                    self._resource = resource
                self._check_generation(generation)
                resource.timeout = max(1, math.ceil(self.timeout * 1000))
                self._check_generation(generation)
                identity = str(resource.query("*IDN?")).strip()
                if "AQ6370" not in identity.upper():
                    raise InstrumentConnectionError(f"Unexpected OSA identity: {identity!r}")
                self._check_generation(generation)
                instrument = None if identity_only else self._instrument_factory(resource)
                with self._lifecycle_lock:
                    self._instrument = instrument
                    self._check_generation(generation)
                    self.identity = identity
                    self.state = DriverState.READY
                    self.log.info("OSA connected: resource=%s identity=%s", self.resource_name, identity)
                    return self
            finally:
                with self._lifecycle_lock:
                    self._connect_done.set()
        except BaseException as error:
            cleanup_error = self._cleanup_operation_failure(generation)
            self._log_failure("OSA connection failed: resource=%s error=%s",
                              self.resource_name, type(error).__name__)
            if not isinstance(error, Exception) or isinstance(error, InstrumentConnectionError):
                self._attach_evidence(error, "cleanup_error", cleanup_error)
                raise
            selected_error = InstrumentConnectionError(f"Failed to connect to {self.resource_name}")
            self._attach_evidence(selected_error, "cleanup_error", cleanup_error)
            raise selected_error from error

    @staticmethod
    def _attach_evidence(error: BaseException, name: str, evidence: BaseException | None) -> None:
        if evidence is not None:
            try:
                setattr(error, name, evidence)
            except BaseException:
                pass

    def _log_failure(self, message: str, *args: object) -> None:
        try:
            self.log.error(message, *args)
        except BaseException:
            pass

    def _cleanup_operation_failure(self, generation: int) -> BaseException | None:
        # A close caller already owns cancelled work. A late operation must not
        # overwrite its state or silently retry cleanup that requires an operator.
        with self._lifecycle_lock:
            if generation != self._cancel_generation or self._closing:
                return self.cleanup_error
            self.state = DriverState.FAULT
        try:
            self._close(expected_generation=generation)
        except BaseException as error:
            self._log_failure("OSA fault cleanup failed: error=%s", type(error).__name__)
            return error.__cause__ if error.__cause__ is not None else error
        return None

    @staticmethod
    def _remaining(deadline: float) -> float:
        return max(0.0, deadline - monotonic())

    def _wait_helper(self, kind: str, subject: object, action: Callable[[], object],
                     deadline: float) -> None:
        helper = self._cleanup_helpers.get(kind)
        if helper is None:
            if self._remaining(deadline) <= 0:
                raise InstrumentTimeoutError(f"OSA {kind} has no close budget remaining")
            helper = _CleanupHelper(kind, subject, action)
            self._cleanup_helpers[kind] = helper
            try:
                helper.thread.start()
            except RuntimeError:
                # A fresh CPython Thread reports native launch failure with
                # RuntimeError before it has an ident. Interruptions may occur
                # after launch but before ident publication: retain those, and
                # retain any thread with an ident even if start() raised.
                if helper.thread.ident is None:
                    del self._cleanup_helpers[kind]
                raise
        if not helper.done.wait(self._remaining(deadline)):
            raise InstrumentTimeoutError(f"OSA {kind} exceeded close deadline")
        helper.thread.join(self._remaining(deadline))
        if helper.thread.is_alive():
            raise InstrumentTimeoutError(f"OSA {kind} helper has not finished")
        if helper.error is not None:
            raise helper.error

    def _abort(self, instrument: object) -> None:
        # Retry cleanup and close may meet while the backend is blocked. Only
        # one abort command may access this instrument at a time.
        with self._abort_lock:
            with self._lifecycle_lock:
                if not self._sweep_owned or instrument is not self._instrument:
                    return
            instrument.abort()
            with self._lifecycle_lock:
                self._sweep_owned = False

    def _initiate_sweep(self, instrument: object, generation: int) -> None:
        # Close's abort cannot pass an in-flight INIT or precede a late INIT.
        with self._abort_lock:
            with self._lifecycle_lock:
                self._check_generation(generation)
                self._sweep_owned = True
            instrument.initiate_sweep()

    def _query_bytes(self, command: str, deadline: float, generation: int,
                     maximum: int) -> bytes:
        """A typed internal query with a hard response bound and whole-call budget."""
        resource = self._resource
        self._check_generation(generation)
        remaining = self._remaining(deadline)
        if remaining <= 0:
            raise InstrumentTimeoutError("OSA trace read deadline expired")
        resource.timeout = max(1, math.ceil(remaining * 1000))
        resource.write(command)
        result = bytearray()
        while True:
            self._check_generation(generation)
            remaining = self._remaining(deadline)
            if remaining <= 0:
                raise InstrumentTimeoutError("OSA trace read deadline expired")
            resource.timeout = max(1, math.ceil(remaining * 1000))
            count = min(4096, maximum + 1 - len(result))
            data, status = resource.visalib.read(resource.session, count)
            self._check_generation(generation)
            if self._remaining(deadline) <= 0:
                raise InstrumentTimeoutError("OSA trace read deadline expired")
            if not isinstance(data, bytes) or len(data) > count:
                raise InstrumentProtocolError("OSA backend returned an invalid read fragment")
            result.extend(data)
            if len(result) > maximum:
                raise InstrumentProtocolError("OSA response exceeds the bounded transfer size")
            if status in (StatusCode.success, StatusCode.success_termination_character_read):
                return bytes(result)
            if status != StatusCode.success_max_count_read:
                raise InstrumentConnectionError("OSA backend reported an unsuccessful read")
            if not data:
                raise InstrumentProtocolError("OSA transfer made no progress")

    def _active_trace(self, trace_name: str, deadline: float, generation: int) -> str:
        reply = self._query_bytes(":TRACe:ACTive?", deadline, generation, 32)
        try:
            active = reply.decode("ascii").strip().upper()
        except UnicodeDecodeError as error:
            raise InstrumentProtocolError("OSA active trace is not ASCII") from error
        if active != f"TR{trace_name}":
            raise InstrumentProtocolError(
                "Select the requested trace on the OSA panel first; software never changes active trace"
            )
        return active

    def _sweep_mode(self, deadline: float, generation: int) -> int:
        reply = self._query_bytes(":INITiate:SMODe?", deadline, generation, 32)
        try:
            mode = int(reply.decode("ascii").strip())
        except (ValueError, UnicodeDecodeError) as error:
            raise InstrumentProtocolError("OSA sweep mode is malformed") from error
        if mode not in (1, 2, 3):
            raise InstrumentProtocolError("OSA sweep mode is unsupported")
        return mode

    def _trace_context(self, trace_name: str, deadline: float,
                       generation: int) -> TraceContext:
        def text(command):
            reply = self._query_bytes(command, deadline, generation, 256)
            try:
                return reply.decode("ascii").strip()
            except UnicodeDecodeError as error:
                raise InstrumentProtocolError("OSA metadata is not ASCII") from error

        def integer(command):
            value = text(command)
            if not value or value.lstrip("+-").isdigit() is False:
                raise InstrumentProtocolError("OSA metadata is not an integer")
            try:
                return int(value)
            except ValueError as error:
                raise InstrumentProtocolError("OSA metadata is not an integer") from error

        def number(command):
            try:
                return float(text(command))
            except ValueError as error:
                raise InstrumentProtocolError("OSA metadata is not numeric") from error

        # Every instrument command is a query. Local VISA timeout/termination
        # changes affect this session, never the panel's format or sweep mode.
        active = self._active_trace(trace_name, deadline, generation)
        x_unit = integer(":UNIT:X?")
        if x_unit != 0:
            raise InstrumentProtocolError(
                "OSA frequency-mode context is not validated; select wavelength units on the panel"
            )
        return TraceContext(
            transfer_format=text(":FORMat:DATA?"),
            sample_count=integer(f":TRACe:DATA:SNUMber? TR{trace_name}"),
            spacing=integer(":DISPlay:TRACe:Y1:SCALe:SPACing?"),
            level_unit=integer(":DISPlay:TRACe:Y1:SCALe:UNIT?"),
            x_unit=x_unit,
            # Explicit trace-name attribute commands may select that trace.
            # Omit the name so even that documented side effect cannot switch it.
            trace_attribute=integer(":TRACe:ATTRibute?"),
            active_trace=active,
            center_m=number(":SENSe:WAVelength:CENTer?"),
            span_m=number(":SENSe:WAVelength:SPAN?"),
            resolution_m=number(":SENSe:BWIDth:RESolution?"),
            sweep_mode=integer(":INITiate:SMODe?"),
        )

    def _read_trace_locked(self, trace_name: str, deadline: float,
                           generation: int) -> TraceCapture:
        resource = self._resource
        previous_timeout, previous_termination = resource.timeout, resource.read_termination
        started, started_utc = monotonic(), datetime.now(timezone.utc)
        primary_error = None
        try:
            # A binary payload may itself contain LF; only VISA completion status
            # terminates these reads. Never issue FORMat or another panel setter.
            resource.read_termination = None
            before = self._trace_context(trace_name, deadline, generation)
            unit = before.native_unit
            x_parts, y_parts = [], []
            for first in range(1, before.sample_count + 1, TRACE_CHUNK_POINTS):
                last = min(first + TRACE_CHUNK_POINTS - 1, before.sample_count)
                for axis, parts in (("X", x_parts), ("Y", y_parts)):
                    reply = self._query_bytes(f":TRACe:{axis}? TR{trace_name},{first},{last}",
                                              deadline, generation, MAX_TRACE_REPLY_BYTES)
                    parts.append(decode_trace_reply(reply, before.transfer_format, last - first + 1))
            after = self._trace_context(trace_name, deadline, generation)
            self._check_generation(generation)
            return TraceCapture(
                wavelength_nm=np.concatenate(x_parts) * 1e9,
                native_values=np.concatenate(y_parts), native_unit=unit,
                trace=trace_name, identity=self.identity, read_started_at=started_utc,
                read_finished_at=datetime.now(timezone.utc), elapsed_s=monotonic() - started,
                context_before=before, context_after=after,
            )
        except BaseException as error:
            primary_error = error
            raise
        finally:
            restore_errors = []
            for name, value in (("read_termination", previous_termination),
                                ("timeout", previous_timeout)):
                try:
                    setattr(resource, name, value)
                except BaseException as error:
                    restore_errors.append(error)
            if restore_errors:
                selected = primary_error if primary_error is not None else restore_errors[0]
                self._attach_evidence(selected, "local_restore_errors", tuple(restore_errors))
                if primary_error is None:
                    raise selected

    def read_trace(self, trace: str = "A", timeout: float | None = None) -> TraceCapture:
        """Read the current trace; no sweep, abort, setting write or read retry."""
        if not isinstance(trace, str):
            raise ValueError("trace must identify AQ6370 trace A through G")
        trace_name = trace.strip().upper()
        if len(trace_name) == 3 and trace_name.startswith("TR"):
            trace_name = trace_name[2:]
        if len(trace_name) != 1 or trace_name not in "ABCDEFG":
            raise ValueError("trace must identify AQ6370 trace A through G")
        wait_seconds = self.timeout if timeout is None else timeout
        if (isinstance(wait_seconds, bool) or not isinstance(wait_seconds, Real)
                or not math.isfinite(wait_seconds) or wait_seconds <= 0):
            raise ValueError("timeout must be finite and positive")
        deadline = monotonic() + float(wait_seconds)
        with self._lifecycle_lock:
            require_state(self.state, {DriverState.READY}, "read an OSA trace")
            generation = self._cancel_generation
            self._check_generation(generation)
            if not self._transaction_lock.acquire(blocking=False):
                raise InstrumentConnectionError("OSA transaction is already running")
            self._acquire_done.clear()
            self.state = DriverState.ACTIVE
        try:
            try:
                capture = self._read_trace_locked(trace_name, deadline, generation)
                with self._lifecycle_lock:
                    self._check_generation(generation)
                    self.state = DriverState.READY
                    return capture
            finally:
                with self._lifecycle_lock:
                    self._acquire_done.set()
                    self._transaction_lock.release()
        except BaseException as error:
            cleanup_error = self._cleanup_operation_failure(generation)
            selected_error = error
            if not isinstance(error, DriverError):
                if isinstance(error, TimeoutError) or (isinstance(error, VisaIOError)
                        and error.error_code == StatusCode.error_timeout):
                    selected_error = InstrumentTimeoutError("OSA existing-trace read timed out")
                elif isinstance(error, (VisaIOError, InvalidSession, OSError)):
                    selected_error = InstrumentConnectionError("OSA existing-trace connection failed")
            self._attach_evidence(selected_error, "cleanup_error", cleanup_error)
            self._log_failure("OSA existing-trace read failed: trace=%s error=%s",
                              trace_name, type(error).__name__)
            if selected_error is error:
                raise
            raise selected_error from error

    def close(self) -> None:
        self._close()

    def _close(self, expected_generation: int | None = None) -> None:
        deadline = monotonic() + self.close_timeout
        with self._lifecycle_lock:
            if expected_generation is not None and expected_generation != self._cancel_generation:
                return
            self._cancel_generation += 1
            self._close_callers += 1
            self._closing = True
            self.state = DriverState.CLOSING
        acquired = False
        errors: list[BaseException] = []
        try:
            acquired = self._close_lock.acquire(timeout=self._remaining(deadline))
            if not acquired:
                raise InstrumentTimeoutError("OSA cleanup is still running")
            # Only a later explicit close may retry a completed failed helper.
            for kind, helper in list(self._cleanup_helpers.items()):
                if helper.done.is_set() and not helper.thread.is_alive() and helper.error is not None:
                    del self._cleanup_helpers[kind]
            if not self._connect_done.wait(self._remaining(deadline)):
                raise InstrumentTimeoutError("OSA connection has not finished")
            with self._lifecycle_lock:
                instrument, resource = self._instrument, self._resource
                lease, reservation = self._lease, self._reservation
                abort_needed = self._sweep_owned or "abort" in self._cleanup_helpers
            if instrument is not None and abort_needed:
                try:
                    self._wait_helper("abort", instrument, lambda: self._abort(instrument), deadline)
                except BaseException as error:
                    errors.append(error)
                    helper = self._cleanup_helpers.get("abort")
                    if helper is not None and not helper.done.is_set():
                        raise
            if not self._acquire_done.wait(self._remaining(deadline)):
                raise InstrumentTimeoutError("OSA acquisition has not finished")
            if resource is not None:
                # A failed close does not prove the handle is closed. Keep both
                # its reservation and manager lease until an explicit retry.
                self._wait_helper("resource-close", resource, resource.close, deadline)
                with self._lifecycle_lock:
                    self._resource = None
                    self._instrument = None
            if reservation is not None:
                reservation.release()
                with self._lifecycle_lock:
                    self._reservation = None
            if lease is not None:
                self._wait_helper("lease-release", lease, lease.close, deadline)
                with self._lifecycle_lock:
                    self._lease = None
                    self._manager = None
            if errors:
                raise errors[0]
            with self._lifecycle_lock:
                self._cleanup_helpers.clear()
                self.cleanup_error = None
                self._closing = self._close_callers > 1
                self.state = DriverState.CLOSING if self._closing else DriverState.DISCONNECTED
            self.log.info("OSA shutdown complete: resource=%s", self.resource_name)
        except BaseException as error:
            with self._lifecycle_lock:
                self.cleanup_error = error
                self.state = DriverState.FAULT
                self._closing = True
            raise InstrumentConnectionError("OSA cleanup incomplete; call close() to retry") from error
        finally:
            with self._lifecycle_lock:
                self._close_callers -= 1
            if acquired:
                self._close_lock.release()

    def acquire(self, trace: str = "A", timeout: float | None = None) -> Spectrum:
        return self._acquire(trace, timeout, native=False)

    def acquire_trace(self, trace: str = "A", timeout: float | None = None) -> TraceCapture:
        """Explicit bounded sweep, retaining exact native values and read context."""
        return self._acquire(trace, timeout, native=True)

    def _acquire(self, trace: str, timeout: float | None, *, native: bool) -> Spectrum | TraceCapture:
        if not isinstance(trace, str):
            raise ValueError("trace must identify AQ6370 trace A through G")
        trace_name = trace.strip().upper()
        if len(trace_name) == 3 and trace_name.startswith("TR"):
            trace_name = trace_name[2:]
        if trace_name not in "ABCDEFG" or len(trace_name) != 1:
            raise ValueError("trace must identify AQ6370 trace A through G")
        wait_seconds = self.timeout if timeout is None else timeout
        if (isinstance(wait_seconds, bool) or not isinstance(wait_seconds, Real)
                or not math.isfinite(wait_seconds) or wait_seconds <= 0):
            raise ValueError("timeout must be finite and positive")
        with self._lifecycle_lock:
            require_state(self.state, {DriverState.READY}, "acquire an OSA spectrum")
            generation = self._cancel_generation
            self._check_generation(generation)
            if not self._transaction_lock.acquire(blocking=False):
                raise InstrumentConnectionError("OSA acquisition is already running")
            self._acquire_done.clear()
            self.state = DriverState.ACTIVE
            resource, instrument, identity = self._resource, self._instrument, self.identity
        try:
            try:
                for attempt in range(self.retries + 1):
                    self._check_generation(generation)
                    self.log.info("OSA acquisition attempt %d/%d: trace=%s",
                                  attempt + 1, self.retries + 1, trace_name)
                    sweep_attempted, sweep_completed, completion_acknowledged = False, False, False
                    deadline = monotonic() + float(wait_seconds)
                    try:
                        resource.timeout = max(1, math.ceil(wait_seconds * 1000))
                        self._check_generation(generation)
                        self._active_trace(trace_name, deadline, generation)
                        mode = self._sweep_mode(deadline, generation)
                        if mode not in (1, 3):
                            raise InstrumentProtocolError(
                                "OSA new sweep requires panel SINGLE or AUTO; use read_trace in REPEAT"
                            )
                        sweep_attempted = True
                        self._initiate_sweep(instrument, generation)
                        self._check_generation(generation)
                        if str(resource.query("*OPC?")).strip() != "1":
                            raise InstrumentProtocolError("OSA did not acknowledge sweep completion")
                        completion_acknowledged = True
                        # OPC in REPEAT acknowledges initiation, not completion.
                        # A changed/malformed mode keeps ownership for cleanup and
                        # must never cause an automatic replacement sweep.
                        if self._sweep_mode(deadline, generation) != mode:
                            raise InstrumentProtocolError("OSA sweep mode changed after initiation")
                        sweep_completed = True
                        with self._lifecycle_lock:
                            self._check_generation(generation)
                            self._sweep_owned = False
                        self._check_generation(generation)
                        capture = self._read_trace_locked(trace_name, deadline, generation)
                        result = capture
                        if not native:
                            power_dbm = capture.power_dbm
                            if power_dbm is None:
                                raise InstrumentProtocolError("OSA native values have no finite dBm compatibility")
                            result = Spectrum.create(capture.wavelength_nm, power_dbm, trace_name, identity)
                        with self._lifecycle_lock:
                            self._check_generation(generation)
                            self.state = DriverState.READY
                            return result
                    except Exception as error:
                        abort_error = None
                        is_protocol = isinstance(error, InstrumentProtocolError)
                        is_timeout = isinstance(error, TimeoutError) or (
                            isinstance(error, VisaIOError) and error.error_code == StatusCode.error_timeout
                        )
                        if (sweep_attempted and not sweep_completed and not completion_acknowledged
                                and (is_protocol or is_timeout)
                                and attempt < self.retries):
                            self._check_generation(generation)
                            try:
                                self._abort(instrument)
                            except Exception as failure:
                                abort_error = failure
                            else:
                                self._check_generation(generation)
                                self.log.warning("OSA acquisition retry: completed=%d error=%s",
                                                 attempt + 1, type(error).__name__)
                                continue
                        if is_timeout:
                            selected_error = InstrumentTimeoutError(
                                f"OSA acquisition failed after {attempt + 1} attempts"
                            )
                        elif isinstance(error, (VisaIOError, InvalidSession, OSError)):
                            selected_error = InstrumentConnectionError("OSA acquisition connection failed")
                        else:
                            selected_error = error
                        if abort_error is not None:
                            # Keep sweep failure/cause compatible while exposing
                            # why no further sweep could safely be attempted.
                            self._attach_evidence(selected_error, "abort_error", abort_error)
                        if selected_error is error:
                            raise
                        raise selected_error from error
            finally:
                with self._lifecycle_lock:
                    self._acquire_done.set()
                    self._transaction_lock.release()
        except BaseException as error:
            cleanup_error = self._cleanup_operation_failure(generation)
            self._attach_evidence(error, "cleanup_error", cleanup_error)
            self._log_failure("OSA acquisition final fault: trace=%s", trace_name)
            raise
        raise AssertionError("unreachable OSA acquisition retry state")

    def __enter__(self) -> "AQ6370":
        return self.connect()

    def __exit__(self, exc_type, exc, traceback) -> None:
        try:
            self.close()
        except BaseException as cleanup_error:
            if exc_type is None:
                raise
            evidence = cleanup_error.__cause__ if cleanup_error.__cause__ is not None else cleanup_error
            self._attach_evidence(exc, "cleanup_error", evidence)
            self._log_failure("OSA context cleanup did not replace body exception: error=%s",
                              type(cleanup_error).__name__)
