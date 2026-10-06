from __future__ import annotations

import logging
from collections.abc import Callable, Iterable
from enum import Enum, auto
from typing import Collection

from serial.tools import list_ports


class DriverState(Enum):
    DISCONNECTED = auto()
    CONNECTING = auto()
    READY = auto()
    ACTIVE = auto()
    CLOSING = auto()
    FAULT = auto()


class DriverError(RuntimeError):
    """Base error for instrument-driver failures."""


class InstrumentConnectionError(DriverError):
    """A device could not be uniquely found, opened, or identified."""


class InstrumentProtocolError(DriverError):
    """A device frame or response violated its protocol."""


class InstrumentSafetyError(DriverError):
    """An operation violated a state, range, or interlock rule."""


class InstrumentCapabilityError(DriverError):
    """The connected sensor, model, or firmware cannot perform an operation."""


class InstrumentTimeoutError(DriverError):
    """A bounded device operation did not complete in time."""


class DeviceFault(DriverError):
    """A driver entered a latched fault state."""


def require_state(actual: DriverState, allowed: Collection[DriverState], action: str) -> None:
    if actual not in allowed:
        names = ", ".join(sorted(state.name for state in allowed))
        raise InstrumentSafetyError(
            f"Cannot {action} while driver state is {actual.name}; allowed states: {names}"
        )


def get_logger(name: str) -> logging.Logger:
    logger = logging.getLogger(f"sil.{name}")
    if not logger.handlers:
        logger.addHandler(logging.NullHandler())
    return logger


def find_serial_port(
    vid: int,
    pid: int,
    serial_number: str | None = None,
    ports_provider: Callable[[], Iterable[object]] = list_ports.comports,
) -> str:
    """Return the unique matching device; provider exceptions propagate unchanged."""
    matches = []
    for port in ports_provider():
        if getattr(port, "vid", None) != vid or getattr(port, "pid", None) != pid:
            continue
        if serial_number is not None and getattr(port, "serial_number", None) != serial_number:
            continue
        matches.append(str(port.device))
    if len(matches) != 1:
        detail = "none" if not matches else ", ".join(matches)
        selector = f"{vid:04X}:{pid:04X}"
        if serial_number is not None:
            selector += f" serial={serial_number}"
        raise InstrumentConnectionError(
            f"Expected exactly one USB serial device {selector}; found {detail}"
        )
    return matches[0]

def _freeze_probe(value):
    from collections.abc import Mapping
    from types import MappingProxyType
    if isinstance(value, Mapping):
        return MappingProxyType({key: _freeze_probe(item) for key, item in value.items()})
    if isinstance(value, (tuple, list)):
        return tuple(_freeze_probe(item) for item in value)
    return value


from dataclasses import dataclass

@dataclass(frozen=True)
class ProbeReport:
    """One immutable identity-only attempt, not validation of every function."""
    identity: dict
    observations: dict
    release_confirmed: bool

    def __post_init__(self):
        object.__setattr__(self, "identity", _freeze_probe(self.identity))
        object.__setattr__(self, "observations", _freeze_probe(self.observations))


def _probe_connect_guard(driver):
    from threading import get_ident
    owner = getattr(driver, "_probe_owner_thread", None)
    if owner is not None and owner != get_ident():
        raise InstrumentConnectionError("An identity probe owns this driver lifecycle")


def _identity_probe(driver, connect, read):
    """Keep failed ownership on the driver; finally close without replacing primary."""
    from threading import get_ident
    with driver._lifecycle_lock:
        if (driver.state is not DriverState.DISCONNECTED
                or driver.has_resource_responsibility
                or getattr(driver, "_probe_owner_thread", None) is not None):
            raise InstrumentConnectionError("Identity probe requires an unowned disconnected driver")
        driver._probe_owner_thread = get_ident()
    identity, observations, primary = {}, {}, None
    try:
        try:
            connect()
            identity, observations = read()
        except BaseException as error:
            primary = error
        finally:
            try:
                driver.close()
            except BaseException as error:
                if primary is None:
                    primary = error
        report = ProbeReport(identity, observations, not driver.has_resource_responsibility)
        driver.last_probe_report = report
        if primary is not None:
            for name, value in (("probe_report", report), ("driver", driver)):
                try: setattr(primary, name, value)
                except BaseException: pass
            raise primary
        return report
    finally:
        with driver._lifecycle_lock:
            driver._probe_owner_thread = None

