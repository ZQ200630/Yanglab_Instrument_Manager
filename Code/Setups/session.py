"""Coordinate delivered instruments using only their public lifecycle APIs.

Construction transfers lifecycle responsibility without connecting anything.
Fiber setups must already be connected by the caller. A session never restores
outputs, adopts a piezo baseline, or reconnects after shutdown. Health is cached
host evidence; drivers remain responsible for freshness and output interlocks.
"""

from __future__ import annotations

import inspect
import threading
from dataclasses import dataclass

from Code.Utils.common import DeviceFault, DriverState, InstrumentSafetyError
from Code.Utils.voltage import ZeroEvidence, ZeroState


@dataclass(frozen=True)
class CleanupStep:
    role: str
    action: str
    error: BaseException | None


@dataclass(frozen=True)
class CleanupReport:
    steps: tuple[CleanupStep, ...]
    voltage_zero: ZeroEvidence | None
    unreleased: tuple[tuple[str, object], ...]

    @property
    def ok(self) -> bool:
        """No reported failure; None evidence does not confirm physical zero."""
        return (not self.unreleased and all(step.error is None for step in self.steps)
                and (self.voltage_zero is None
                     or self.voltage_zero.state is ZeroState.MEASURED_ZERO))


@dataclass(frozen=True)
class DeviceHealth:
    role: str
    state: DriverState | None
    cached: bool
    observed_at: float | None


@dataclass
class _CleanupAttempt:
    """One reserved cleanup outcome, shared by its overlapping callers."""

    running: bool = False
    report: CleanupReport | None = None


_MISSING = object()
_ROLES = ("osa", "voltage", "gain", "pm400", "fiber")
_CLEANUP = (("voltage", "zero", "zero"),
            ("gain", "current_off", "disable_current"),
            ("gain", "tec_off", "disable_tec"),
            ("gain", "close", "close"),
            ("voltage", "close", "close"),
            ("osa", "close", "close"),
            ("pm400", "close", "close"),
            ("fiber", "close", "close"))


def _public_member(device: object, name: str) -> object:
    # Do not mistake a failing property for an absent API.
    if inspect.getattr_static(device, name, _MISSING) is _MISSING:
        return _MISSING
    return getattr(device, name)


def _released(device: object, close_error: BaseException | None) -> bool:
    released = _public_member(device, "resources_released")
    if released is not _MISSING:
        return released is True
    is_open = _public_member(device, "is_open")
    if is_open is not _MISSING and bool(is_open):
        return False
    state = _public_member(device, "state")
    if isinstance(state, DriverState):
        return state is DriverState.DISCONNECTED
    # Older duck-typed drivers supply no lifecycle evidence. A successful close
    # is their public release contract; an exception retains ownership.
    return close_error is None


def _attach_report(error: BaseException, report: CleanupReport) -> None:
    try:
        error.cleanup_report = report
    except BaseException:
        # An unusual exception with a rejecting descriptor must not replace an
        # operation's original KeyboardInterrupt/SystemExit or other failure.
        pass


def _raise_cleanup(report: CleanupReport) -> None:
    first = next((step.error for step in report.steps if step.error is not None), None)
    if first is None and not report.ok:
        first = InstrumentSafetyError("Cleanup did not confirm resource release or voltage zero")
    if first is not None:
        _attach_report(first, report)
        raise first


class InstrumentSession:
    def __init__(self, *, osa=None, voltage=None, gain=None, pm400=None, fiber=None, log=None):
        self._devices = {role: device for role, device in zip(
            _ROLES, (osa, voltage, gain, pm400, fiber)) if device is not None}
        self._pending = dict(self._devices)
        self._log = log
        self._condition = threading.Condition()
        self._stop_requested = False
        self._connecting = False
        self._connected = False
        self._attempt: _CleanupAttempt | None = None
        self._report: CleanupReport | None = None

    def request_stop(self) -> None:
        """Publish stop intent only; this method performs no driver I/O."""
        with self._condition:
            self._stop_requested = True
            self._condition.notify_all()

    def _require_running(self) -> None:
        if self._stop_requested:
            raise InstrumentSafetyError("Instrument session stop requested")

    def connect(self) -> InstrumentSession:
        with self._condition:
            self._require_running()
            if self._connecting:
                raise InstrumentSafetyError("Instrument session connection is already in progress")
            if self._connected:
                return self
            self._connecting = True
        primary = None
        attempt = None
        try:
            for role in _ROLES[:-1]:
                with self._condition:
                    self._require_running()
                device = self._devices.get(role)
                if device is not None:
                    device.connect()
            with self._condition:
                self._require_running()
                self._connected = True
        except BaseException as error:
            primary = error
        finally:
            with self._condition:
                if primary is not None:
                    self._stop_requested = True
                    # Bind failed connect to the same attempt as close callers
                    # already waiting for it, before making them runnable.
                    attempt = self._reserve_cleanup_locked()
                self._connecting = False
                self._condition.notify_all()
        if primary is not None:
            self._cleanup_preserving(primary, attempt=attempt)
            raise primary
        return self

    def _reserve_cleanup_locked(self) -> _CleanupAttempt:
        if self._attempt is None or self._attempt.report is not None:
            self._attempt = _CleanupAttempt()
        return self._attempt

    def close(self) -> CleanupReport:
        """Close in safety order, retaining failed resources for explicit retry.

        Overlapping close calls share one attempt. The condition is released
        while waiting and while drivers perform I/O, so stop remains responsive.
        After all resources are released, close returns the last report without
        repeating actions (including when that report records an output error).
        """
        with self._condition:
            self._stop_requested = True
            if self._report is not None and not self._pending:
                return self._report
            # Reserve before waiting for connect; a late wake must consume this
            # exact attempt even if another caller has already completed it.
            attempt = self._reserve_cleanup_locked()
        return self._close_attempt(attempt)

    def _close_attempt(self, attempt: _CleanupAttempt) -> CleanupReport:
        with self._condition:
            self._condition.wait_for(lambda: not self._connecting)
            if attempt.running:
                self._condition.wait_for(lambda: attempt.report is not None)
            report = attempt.report
            if report is None:
                attempt.running = True
            pending = dict(self._pending)
            evidence = self._report.voltage_zero if self._report is not None else None
        if report is not None:
            _raise_cleanup(report)
            return report
        steps = []
        remaining = dict(pending)
        try:
            for role, action, method in _CLEANUP:
                device = pending.get(role)
                if device is None:
                    continue
                error = None
                try:
                    if action == "zero":
                        device.zero(emergency=True)
                    else:
                        getattr(device, method)()
                except BaseException as caught:
                    error = caught
                if action == "close":
                    try:
                        if _released(device, error):
                            remaining.pop(role)
                        elif error is None:
                            error = InstrumentSafetyError(f"{role} close did not release its resource")
                    except BaseException as caught:
                        if error is None:
                            error = caught
                steps.append(CleanupStep(role, action, error))
            if "voltage" in pending:
                try:
                    value = _public_member(pending["voltage"], "zero_evidence")
                    if value is _MISSING or value is None:
                        evidence = None
                    elif isinstance(value, ZeroEvidence):
                        evidence = value
                    else:
                        raise InstrumentSafetyError("Unsupported voltage zero evidence")
                except BaseException as caught:
                    evidence = None
                    steps.append(CleanupStep("voltage", "zero_evidence", caught))
            report = CleanupReport(tuple(steps), evidence, tuple(remaining.items()))
            with self._condition:
                self._pending = remaining
                self._report = report
                attempt.report = report
        finally:
            with self._condition:
                attempt.running = False
                self._condition.notify_all()
        # Logging happens only after safety actions and report publication.
        for step in report.steps:
            if step.error is not None and self._log is not None:
                try:
                    self._log.error("Cleanup step failed: role=%s action=%s error=%s",
                                    step.role, step.action, step.error)
                except BaseException:
                    pass
        _raise_cleanup(report)
        return report

    def check_health(self) -> tuple[DeviceHealth, ...]:
        """Read public cached state only; unknown state is explicitly rejected."""
        with self._condition:
            self._require_running()
            devices = tuple(self._devices.items())
        snapshots = []
        first = None
        for role, device in devices:
            state = None
            observed_at = None
            try:
                candidate = _public_member(device, "state")
                state = candidate if isinstance(candidate, DriverState) else None
                if state is DriverState.FAULT:
                    raise DeviceFault(f"{role} driver is in FAULT")
                if state not in (DriverState.READY, DriverState.ACTIVE):
                    raise InstrumentSafetyError(f"{role} has unhealthy or unknown driver state: {state}")
                if role == "gain":
                    snapshot = _public_member(device, "status")
                    observed_at = getattr(snapshot, "received_at", None)
                elif role == "voltage":
                    read_status = _public_member(device, "read_status")
                    if read_status is not _MISSING:
                        observed_at = getattr(read_status(), "received_at", None)
            except BaseException as error:
                if first is None:
                    first = error
            snapshots.append(DeviceHealth(role, state, state is not None, observed_at))
        result = tuple(snapshots)
        with self._condition:
            self._require_running()
        if first is not None:
            first.device_health = result
            raise first
        return result

    def _cleanup_preserving(
        self, primary: BaseException, *, attempt: _CleanupAttempt | None = None,
    ) -> None:
        try:
            report = self.close() if attempt is None else self._close_attempt(attempt)
        except BaseException as cleanup:
            report = getattr(cleanup, "cleanup_report", self._report)
        if report is not None:
            _attach_report(primary, report)

    def __enter__(self) -> InstrumentSession:
        return self.connect()

    def __exit__(self, exc_type, exc_value, traceback) -> bool:
        if exc_value is None:
            self.close()
        else:
            self._cleanup_preserving(exc_value)
        return False


def safe_shutdown(voltage, gain, osa, log, *, pm400=None, fiber=None) -> None:
    """Compatibility entry point for the original four positional arguments."""
    InstrumentSession(voltage=voltage, gain=gain, osa=osa, log=log,
                      pm400=pm400, fiber=fiber).close()
