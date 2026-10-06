"""Process-wide VISA manager leases and resource reservations."""
from __future__ import annotations

from collections.abc import Callable
from threading import RLock
import weakref

import pyvisa
from pyvisa.rname import ResourceName

from .common import InstrumentConnectionError


_registry_lock = RLock()
_managers: dict[int, _ManagerEntry] = {}
_resources: dict[str, object] = {}
# Values never refer back to their borrowed manager, even through an exception.
_borrowed: dict[int, weakref.ReferenceType] = {}
_acquisitions: set[object] = set()
_retired: dict[int, _RetiredManager] = {}


class _ManagerEntry:
    def __init__(self, manager: object):
        self.manager = manager
        self.tokens: set[object] = set()
        self.borrowed = False
        self.closing = False
        self.close_error: BaseException | None = None


class _RetiredManager:
    def __init__(self, manager: object, reference: weakref.ReferenceType | None):
        self.pending = _acquisitions.copy()
        # A pending factory can already hold this manager before returning it.
        # Retain its actual identity until every such acquisition has enrolled.
        self.manager = manager if self.pending else None
        self.reference = reference


def _retire_manager(manager: object) -> None:
    key = id(manager)

    def forget(ref):
        with _registry_lock:
            retired = _retired.get(key)
            if retired is not None and retired.reference is ref:
                del _retired[key]

    try:
        reference = weakref.ref(manager, forget)
    except TypeError:
        reference = None
    if reference is not None or _acquisitions:
        _retired[key] = _RetiredManager(manager, reference)


def _finish_acquisition(token: object) -> list[object]:
    """Detach completed strong ownership under lock; caller drops it outside."""
    _acquisitions.remove(token)
    cleanup: list[object] = []
    for key, retired in list(_retired.items()):
        retired.pending.discard(token)
        if not retired.pending:
            # Transfer before clearing: destructors may reenter this registry or
            # perform backend I/O, so they must not run in this critical section.
            if retired.manager is not None:
                cleanup.append(retired.manager)
            retired.manager = None
            if retired.reference is None:
                del _retired[key]
    return cleanup


def _remember_borrowed(manager: object) -> None:
    key = id(manager)

    def forget(ref):
        with _registry_lock:
            if _borrowed.get(key) is ref:
                del _borrowed[key]

    try:
        reference = weakref.ref(manager, forget)
    except TypeError as error:
        raise TypeError("Borrowed VISA managers must support weak references") from error
    _borrowed[key] = reference


def _canonical_resource_name(manager: object, name: str) -> str:
    if not isinstance(name, str) or not name.strip():
        raise InstrumentConnectionError("VISA resource name must be a nonempty string")
    name = name.strip()
    resource_info = getattr(manager, "resource_info", None)
    if callable(resource_info):
        try:
            expanded = resource_info(name).resource_name
        except Exception:
            # A syntactically valid direct name needs no alias service. An
            # unresolved alias will also fail the parser below and is rejected.
            pass
        else:
            if isinstance(expanded, str) and expanded.strip():
                name = expanded.strip()
    try:
        # ResourceName normalizes structural defaults without uppercasing USB
        # serial numbers, TCPIP device identifiers, or other sensitive fields.
        return str(ResourceName.from_string(name))
    except (ValueError, TypeError) as error:
        raise InstrumentConnectionError(f"Cannot resolve VISA resource {name!r}") from error


class VisaResourceReservation:
    """Exclusive process-wide claim; release only after resource I/O has ended."""

    def __init__(self, lease: VisaManagerLease, canonical_name: str):
        self.canonical_name = canonical_name
        self.released = False
        self._lease = lease
        self._token = object()
        _resources[canonical_name] = self._token
        lease._reservations.add(self._token)

    def release(self) -> None:
        """Idempotently relinquish the target without performing VISA I/O."""
        with _registry_lock:
            if self.released:
                return
            del _resources[self.canonical_name]
            self._lease._reservations.remove(self._token)
            self.released = True


class VisaManagerLease:
    """One caller's ownership of an actual manager object."""

    def __init__(self, entry: _ManagerEntry):
        self.manager = entry.manager
        self._entry = entry
        self._token = object()
        self.released = False
        self._reservations: set[object] = set()
        self._resolving = 0
        entry.tokens.add(self._token)

    def _require_usable(self) -> None:
        if self.released:
            raise InstrumentConnectionError("VISA manager lease is released")
        if self._entry.closing:
            raise InstrumentConnectionError("VISA manager is already closing")
        if self._entry.close_error is not None:
            raise InstrumentConnectionError("VISA manager close failed; retry close explicitly")

    def reserve_resource(self, name: str) -> VisaResourceReservation:
        """Resolve a name read-only and claim it before the caller opens it."""
        with _registry_lock:
            self._require_usable()
            self._resolving += 1
        try:
            canonical = _canonical_resource_name(self.manager, name)
            with _registry_lock:
                self._require_usable()
                if canonical in _resources:
                    raise InstrumentConnectionError(f"VISA resource {canonical!r} is already reserved")
                return VisaResourceReservation(self, canonical)
        finally:
            with _registry_lock:
                self._resolving -= 1

    def close(self) -> None:
        """Release this lease, closing an owned manager only at its last lease.

        A failed close keeps ownership and must be explicitly retried. Callers
        must close their resource and release its reservation first.
        """
        entry = self._entry
        with _registry_lock:
            if self.released:
                return
            if entry.closing:
                raise InstrumentConnectionError("VISA manager is already closing")
            if self._reservations or self._resolving:
                raise InstrumentConnectionError("VISA lease still owns resources or resource-name calls")
            if len(entry.tokens) > 1 or entry.borrowed:
                entry.tokens.remove(self._token)
                self.released = True
                if not entry.tokens:
                    del _managers[id(self.manager)]
                return
            entry.closing = True
        # External backend I/O must never block unrelated registry operations.
        try:
            self.manager.close()
        except BaseException as error:
            with _registry_lock:
                entry.close_error = error
                entry.closing = False
            raise
        else:
            with _registry_lock:
                _retire_manager(self.manager)
                entry.tokens.remove(self._token)
                entry.close_error = None
                entry.closing = False
                self.released = True
                del _managers[id(self.manager)]


def acquire_visa_manager(
    *, resource_manager: object | None = None,
    factory: Callable[[], object] = pyvisa.ResourceManager,
) -> VisaManagerLease:
    """Acquire by manager identity; injected managers are never auto-closed.

    Borrowed managers must support weak references so external ownership can
    survive zero active leases without keeping the object alive indefinitely.
    Factory and backend calls execute outside the global registry lock. Closed
    manager identity is retained until overlapping acquisitions have finished;
    weak-referenceable managers also remain retired for their object lifetime.
    """
    token = object()
    with _registry_lock:
        _acquisitions.add(token)
    try:
        manager = resource_manager if resource_manager is not None else factory()
        with _registry_lock:
            retired = _retired.get(id(manager))
            if retired is not None and (
                retired.manager is manager
                or (retired.reference is not None and retired.reference() is manager)
            ):
                raise InstrumentConnectionError("VISA manager has already been closed")
            entry = _managers.get(id(manager))
            if entry is not None and (entry.closing or entry.close_error is not None):
                raise InstrumentConnectionError("VISA manager is closing or its close failed")
            if resource_manager is not None:
                _remember_borrowed(manager)
            if entry is None:
                entry = _ManagerEntry(manager)
                _managers[id(manager)] = entry
            reference = _borrowed.get(id(manager))
            if reference is not None and reference() is manager:
                entry.borrowed = True
            return VisaManagerLease(entry)
    finally:
        with _registry_lock:
            cleanup = _finish_acquisition(token)
        cleanup.clear()
