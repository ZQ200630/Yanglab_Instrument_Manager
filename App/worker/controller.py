"""Single-process owner and typed public-driver action boundary."""

from __future__ import annotations

import dataclasses
import sys
from pathlib import Path
import copy
import datetime
import math
import threading
import time
import uuid
from collections.abc import Callable, Iterable, Mapping
from enum import Enum
from typing import Any
from serial.tools import list_ports
from pyvisa.rname import ResourceName

from Code.Utils.newport_usb import resources_released as newport_resources_released
from Code.Utils.tlb_native_bridge import NATIVE
from Code.Setups import FiberCouplingSetup, InstrumentSession
from Code.Utils import (AQ6370, GainDriver, MeasurementKind, PM400, VoltageSource, TLB6700,
                        DeviceFault, InstrumentSafetyError)

from .discovery import discover
from .pm400_ops import catalog as pm400_catalog
from .pm400_ops import execute as pm400_execute
from .pm400_ops import PM400InvocationError
from .contracts import Context, Observation, Request, Outcome, encode_v2
from .scheduler import CallbackFailure, PostReadback, Scheduler, _ReplyFuture
from .observations import AFFECTED, READERS, SWITCHES, EvidenceStore


ROLES = frozenset({"osa", "voltage", "gain", "pm400", "fiber", "laser"})
LIFECYCLE_ACK = frozenset({"osa", "voltage", "gain"})
VISA_ROLES = frozenset({"osa", "pm400"})
SERIAL_ROLES = frozenset({"voltage", "gain"})


class ConsoleError(RuntimeError):
    """A request is invalid or its device operation failed."""


class _LifecycleAdapter:
    """Keep Session's retry obligation aligned with the App's strict evidence."""

    def __init__(self, device, released):
        self._device, self._released = device, released

    @property
    def resources_released(self):
        return self._released(self._device)

    @property
    def zero_evidence(self):
        return self._device.zero_evidence

    def zero(self, *, emergency=False):
        return self._device.zero(emergency=emergency)

    def disable_current(self):
        return self._device.disable_current()

    def disable_tec(self):
        return self._device.disable_tec()

    def close(self):
        return self._device.close()


def _json_value(value: Any) -> Any:
    if value is None or type(value) in (bool, int, str):
        return value
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ConsoleError("device reported non-finite value")
        return value
    if isinstance(value, (datetime.datetime, datetime.date, datetime.time)):
        return value.isoformat()
    if isinstance(value, MeasurementKind):
        return value.name.lower()
    if isinstance(value, Enum):
        return _json_value(value.value)
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        return {field.name: _json_value(getattr(value, field.name))
                for field in dataclasses.fields(value)}
    if isinstance(value, Mapping):
        return {str(_json_value(key)): _json_value(item) for key, item in value.items()}
    if isinstance(value, (set, frozenset)):
        return [_json_value(item) for item in sorted(value, key=str)]
    if isinstance(value, (tuple, list)):
        return [_json_value(item) for item in value]
    if hasattr(value, "tolist") and callable(value.tolist):
        return _json_value(value.tolist())
    if hasattr(value, "__dict__"):
        return {name: _json_value(item) for name, item in vars(value).items()
                if not name.startswith("_")}
    raise ConsoleError(f"unsupported device status type: {type(value).__name__}")


def _state(device: object) -> str:
    state = getattr(device, "state", None)
    return state.name if hasattr(state, "name") else "UNKNOWN"


def _sample_age(sample: object) -> float | None:
    """Age of a host-observed sample, never a claim about physical output."""
    received_at = sample.get("received_at") if isinstance(sample, Mapping) else getattr(sample, "received_at", None)
    if type(received_at) not in (int, float) or not math.isfinite(received_at):
        return None
    return max(0.0, time.monotonic() - received_at)


def _serial_key(resource: str) -> str:
    """Treat ``COM6`` and ``\\\\.\\COM6`` as the same Windows port."""
    candidate = resource.strip().casefold()
    prefix = "\\\\.\\"
    if candidate.startswith(prefix):
        candidate = candidate[len(prefix):]
    number = candidate[3:] if candidate.startswith("com") else ""
    if number and number.isascii() and number.isdecimal():
        return candidate
    return resource.strip().casefold()


class ConsoleController:
    def __init__(
        self, *,
        factories: Mapping[str, Callable[..., object]] | None = None,
        port_enumerator: Callable[[], Iterable[object]] = list_ports.comports,
        visa_resource_resolver: Callable[[str], str] | None = None,
        _domains=None,
    ) -> None:
        self._domains = _domains
        self._factories: dict[str, Callable[..., object]] = dict(factories or {})
        self._port_enumerator = port_enumerator
        self._visa_resource_resolver = visa_resource_resolver
        self._devices: dict[str, object] = {}
        self._resources: dict[str, str | None] = {}
        self._lock = threading.RLock()
        self._closed = False
        self._last_cleanup: dict[str, Any] | None = None
        # Last completed host command, not a physical voltage measurement.
        self._requested_voltage: list[float | None] | None = None
        self._ownership: dict[str, Context] = {}
        self._resource_keys: dict[str, str] = {}
        from .resources import ResourceClaims
        self._physical_claims = ResourceClaims()
        self._physical_reservations = {}
        self._release_evidence = {}
        self._stage_candidates: dict[str, set[str]] = {}
        self._stage_ports: set[str] = set()
        self._session_id = _domains.session_id if _domains is not None else uuid.uuid4().hex
        self._sessions = {}
        self._cleanup_attempts = []
        self._role_cleanup = {}
        self._role_cleanup_attempts = []
        self._gain_evidence = None
        self._gain_round = None
        self._gain_last_command = None
        self._management = None
        self._wire_identity = {
            "python_executable": str(Path(sys.executable).resolve()),
            "project_root": str(Path(__file__).resolve().parents[2]),
            "environment_name": Path(sys.prefix).name, "protocol_version": 2,
        }
        if _domains is None:
            self._scheduler = Scheduler(self._execute, self._observe, session_id=self._session_id)
        else:
            from .domains import SchedulerV3
            self._v3scheduler = SchedulerV3(self._execute_v3,self._observe_v3,registry=_domains)
            self._scheduler = self._v3scheduler.core
        self._scheduler._release_role = self._release_role
        self._scheduler._invalidate_role = self._invalidate_evidence
        self._scheduler._status_overlay = self._evidence_overlay
        self._scheduler._prepare_disconnect = self._prepare_disconnect

    def _factory(self, role: str) -> Callable[..., object]:
        if role in self._factories:
            return self._factories[role]
        kind = self._kind(role)
        if kind in self._factories:
            return self._factories[kind]
        if kind == 'mdt':
            from Code.Utils import MDT693B
            return MDT693B
        return {"osa": AQ6370, "voltage": VoltageSource, "gain": GainDriver,
                "pm400": PM400, "fiber": FiberCouplingSetup.connect, "laser": TLB6700}[kind]

    def _kind(self, role):
        return self._domains.get(self._v3scheduler._refs[role]).config.driver_kind if self._domains is not None else role

    def _domain_state(self, role):
        return self._domains.get(self._v3scheduler._refs[role])

    def _gain_get(self, role, field):
        if self._domains is not None:
            return getattr(self._domain_state(role).evidence,field)
        legacy={'store':'_gain_evidence','context':'_gain_evidence_context','round':'_gain_round','last_command':'_gain_last_command'}
        return getattr(self,legacy[field],None)

    def _gain_set(self, role, field, value):
        if self._domains is not None:
            setattr(self._domain_state(role).evidence,field,value)
        else:
            legacy={'store':'_gain_evidence','context':'_gain_evidence_context','round':'_gain_round','last_command':'_gain_last_command'}
            setattr(self,legacy[field],value)

    def _snapshot(self, role: str, device: object) -> dict[str, Any]:
        kind = self._kind(role)
        with self._lock:
            resource = self._resources.get(role)
            requested = copy.deepcopy(self._domain_state(role).requested_voltage if self._domains is not None else self._requested_voltage)
        result: dict[str, Any] = {
            "connected": _state(device) in {"READY", "ACTIVE"},
            "state": _state(device), "resource": resource,
        }
        if kind == "osa":
            result["identity"] = getattr(device, "identity", None)
        elif kind == "voltage":
            sample = device.read_status()
            result.update(_json_value(sample))
            result["sample_age_s"] = _sample_age(sample)
            result["zero_evidence"] = _json_value(device.zero_evidence)
            result["requested_voltage_v"] = requested
        elif kind == "pm400":
            result["instrument"] = _json_value(getattr(device, "instrument_info", None))
            result["sensor"] = _json_value(getattr(device, "sensor_info", None))
            result["last_preexisting_errors"] = _json_value(
                getattr(device, "last_preexisting_errors", ())
            )
            result["catalog"] = pm400_catalog(device)
        elif kind == "mdt":
            result['controller'] = _json_value(device.status)
        elif kind == "laser":
            sample = device.read_status()
            result.update(identity=_json_value(device.identity), laser=_json_value(sample),
                          wavelength_range_nm=_json_value(device.wavelength_range_nm),
                          sample_age_s=_sample_age(sample))
        elif kind == "fiber":
            result["left"] = _json_value(device.left.status)
            result["right"] = _json_value(device.right.status)
        return result

    def _status(self) -> dict[str, Any]:
        return self.cached_status()

    def cached_status(self) -> dict[str, Any]:
        status = self._scheduler.status()
        with self._lock:
            cleanup = copy.deepcopy(self._last_cleanup)
            role_cleanup_attempts = copy.deepcopy(self._role_cleanup_attempts)
            owned = set(self._resources)
        status["devices"] = {role: value for role, value in status["devices"].items() if role in owned}
        return {**status, "mode": "real",
                "last_cleanup": cleanup,
                "role_cleanup_attempts": role_cleanup_attempts,
                "observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}

    def context(self, role: str) -> Context:
        return self._scheduler.context(role)

    def submit(self, request: Request):
        published = _ReplyFuture(lambda error: self._scheduler._record_callback_error(
            getattr(request, "id", "invalid"), error))
        published.set_running_or_notify_cancel()
        def publish(completed):
            outcome = None
            try:
                outcome = completed.result()
                # Scheduler rejections already carry validated terminal evidence.
                # Their original input need not be a Request or have a wire ID.
                if outcome.phase == "completed":
                    outcome = self._wire_outcome(request, outcome)
                    encode_v2(request.id, outcome)
            except BaseException as error:
                context = (outcome.context if isinstance(outcome, Outcome)
                           else Context(self._session_id, None, 0))
                outcome = Outcome("completed_readback_failed", context,
                    error={"type": type(error).__name__, "message": "terminal publication failed"})
            published.set_result(outcome)
        self._scheduler.submit(request).add_done_callback(publish)
        return published

    def _wire_outcome(self, request, outcome):
        if outcome.phase != "completed" or request.method not in {"ping", "status"}:
            return outcome
        status = self.cached_status()
        if request.method == "ping":
            status = {**status, **self._wire_identity, "connected": bool(status["devices"])}
        return Outcome(outcome.phase, outcome.context, status, outcome.error)

    def _observe(self, role: str, context: Context) -> Observation:
        if self._kind(role) == 'gain':
            return self._observe_gain(context,role)
        with self._lock:
            owner = self._ownership.get(role)
            device = self._devices.get(role)
        if device is None or owner is None or owner.connection_id != context.connection_id:
            raise ConsoleError("observation has no matching owned device")
        return Observation(self._snapshot(role, device))

    def _evidence_overlay(self, role, context, status):
        # Called with Scheduler condition held. Never acquire it under _lock.
        if self._kind(role) != 'gain':
            return status
        with self._lock:
            store = self._gain_get(role,'store')
            owner = self._ownership.get(role)
            if store is None or owner is None or store.connection_id != context.connection_id:
                return status
            if owner.session_id != context.session_id or owner.connection_id != context.connection_id:
                return status
            fields = store.snapshot()
            if self._gain_get(role,'context') != context or self._scheduler._roles[role].epoch_exhausted:
                for field in fields.values():
                    field['quality'] = 'unknown'
            return {**(status or {}), 'fields': fields,
                    **{name: field['value'] for name, field in fields.items()},
                    'last_command': copy.deepcopy(self._gain_get(role,'last_command'))}

    def _invalidate_evidence(self, role, context):
        if self._kind(role) != 'gain':
            return
        with self._lock:
            store = self._gain_get(role,'store')
            if store is not None and store.connection_id == context.connection_id:
                self._gain_set(role,'context',context)
                store.invalidate(SWITCHES, 'stop_requested')
                self._gain_set(role,'round',None)

    def _gain_command_started(self, name, context, role='gain'):
        with self._scheduler._condition:
            with self._lock:
                store = self._gain_get(role,'store')
                if store is not None and store.connection_id == context.connection_id:
                    store.invalidate(AFFECTED.get(name, ()), 'command_started')
                    self._gain_set(role,'round',None)
                    self._scheduler._roles[role].healthy_context = None

    def _observe_gain(self, context, role='gain'):
        scheduler = self._scheduler
        with scheduler._condition:
            lane = scheduler._roles[role]
            admission = lane.observation_revision
            required = lane.readback
            if context != lane.context or admission != lane.revision or lane.epoch_exhausted:
                return Observation({}, more=True)
            with self._lock:
                store, device = self._gain_get(role,'store'), self._devices.get(role)
                if store is None or device is None or store.connection_id != context.connection_id:
                    raise ConsoleError('observation has no matching owned Gain device')
                round_ = self._gain_get(role,'round')
                if round_ is None or round_['context'] != context:
                    round_ = {'context': context, 'index': 0, 'error': None}
                    self._gain_set(role,'round',round_)
                name = tuple(READERS)[round_['index']]
                revision = store._next_revision()
                started = scheduler._clock()
        error = None
        fault = False
        try:
            value = getattr(device, READERS[name])()
            if _state(device) == 'FAULT':
                raise DeviceFault('Gain reports FAULT during observation')
        except BaseException as cause:
            error = {'type': type(cause).__name__, 'message': str(cause) or type(cause).__name__}
            fault = isinstance(cause, (DeviceFault, InstrumentSafetyError)) or _state(device) == 'FAULT'
        with scheduler._condition:
            with self._lock:
                # Atomic with stop epoch and command-start fences. Reject here,
                # before changing the live store, not just the aggregate cache.
                required_error = error is not None and required is not None and lane.readback is required
                if (context != lane.context or (admission != lane.revision and not required_error) or lane.epoch_exhausted
                        or store is not self._gain_get(role,'store') or round_ is not self._gain_get(role,'round')):
                    if round_ is self._gain_get(role,'round'):
                        self._gain_set(role,'round',None)
                    return Observation({}, more=True)
                if error is None:
                    try:
                        store.record(name, value, started, revision)
                    except (ValueError, TypeError) as cause:
                        error = {'type': type(cause).__name__, 'message': str(cause)}
                if fault:
                    store.invalidate(SWITCHES, 'interlock_fault')
                if error:
                    store.fail(name, error['message'])
                    round_['error'] = round_['error'] or error
                elif name == 'tec_enabled' and value is False:
                    store.invalidate(('current_enabled',), 'tec_observed_off')
                round_['index'] += 1
                # Fail the round at the first error, retaining earlier fields.
                # A queued write must not turn a real required-readback failure
                # into mere supersession and bypass the role fault barrier.
                more = error is None and round_['index'] < len(READERS)
                status = {'connected': _state(device) in {'READY', 'ACTIVE'},
                          'state': _state(device), 'resource': self._resources.get(role)}
                if round_['error']:
                    status['observation_error'] = copy.deepcopy(round_['error'])
                status = self._evidence_overlay(role, context, status)
                if not more:
                    self._gain_set(role,'round',None)
                return Observation(status, more=more)

    def _reserve(self, role, resource, context) -> None:
        """Pure registry transaction; caller holds the short ownership lock."""
        if role in self._resources:
            raise ConsoleError(f"{role} is already connected or retained after a fault")
        stage_ports = self._stage_candidates.get(context.connection_id, set())
        kind = self._kind(role)
        if kind in SERIAL_ROLES | {'mdt'}:
            key = _serial_key(resource)
            if kind != 'mdt' and key in stage_ports | self._stage_ports:
                raise ConsoleError("serial resource belongs to a registered MDT693B stage")
            if any(self._kind(other) in SERIAL_ROLES | {'mdt'} and existing == key for other, existing in self._resource_keys.items()):
                raise ConsoleError("this serial resource is already assigned to another role")
        elif kind in VISA_ROLES:
            # ResourceName normalizes interface/default fields, preserving serial
            # numbers and case-sensitive address components. Never casefold VISA.
            key = resource
            if any(self._kind(other) in VISA_ROLES and existing == key for other, existing in self._resource_keys.items()):
                raise ConsoleError("this VISA resource is already assigned to another role")
        elif kind == "laser":
            key = 'newport:' + resource
            if key in self._resource_keys.values():
                raise ConsoleError('This Newport controller already has a session')
        else:
            key = "fiber"
            if any(self._kind(other) in SERIAL_ROLES | {'mdt'} and existing in stage_ports
                   for other, existing in self._resource_keys.items()):
                raise ConsoleError("fiber stage port is already assigned to a serial role")
        if self._domains is not None:
            from .resources import ResourceClaim, instrument_identity
            configuration = self._domain_state(role).config
            members = configuration.members if kind == "fiber" else (configuration,)
            claims = []
            for member in members:
                address = member.params.get("device_key", member.params.get("port", member.params.get("resource")))
                identity = member.expected_identity.get("transport_serial")
                physical = instrument_identity(member)
                if member.driver_kind in VISA_ROLES:
                    claims.append(ResourceClaim(canonical_visa=address, transport_identity=identity,instrument_identity=physical))
                elif member.driver_kind == 'laser':
                    claims.append(ResourceClaim(transport_identity='newport:' + address, instrument_identity=physical))
                else:
                    claims.append(ResourceClaim(serial_port=address, transport_identity=identity,instrument_identity=physical))
            reservation = self._physical_claims.reserve(configuration.domain, tuple(claims))
            self._physical_reservations[role] = reservation
            state = self._domain_state(role)
            state.reservation = reservation
            state.release_confirmed = False
        self._resources[role] = resource
        self._resource_keys[role] = key
        self._ownership[role] = context
        if kind == "fiber":
            self._stage_ports = set(stage_ports)

    def _construct(self, role, resource):
        factory = self._factory(role)
        kind = self._kind(role)
        if kind == 'laser':
            return factory(device_key=resource)
        if kind == "fiber":
            if self._domains is not None:
                serials = tuple(member.expected_identity.get("serial",member.expected_identity.get("transport_serial"))
                                for member in self._domain_state(role).config.members)
                if "fiber" not in self._factories and role not in self._factories:
                    from types import SimpleNamespace
                    # _connect validated these exact serial/address pairs before
                    # reservation. Never discover a different address during open.
                    ports=tuple(SimpleNamespace(device=member.params['port'],serial_number=serial)
                                for member,serial in zip(self._domain_state(role).config.members,serials))
                    return FiberCouplingSetup.for_members(serials, port_enumerator=lambda:ports)
                return factory(member_serials=serials)
            return factory()
        kwargs = {"resource_name": resource} if kind in VISA_ROLES else {"port": resource}
        if self._domains is not None:
            configuration=self._domain_state(role).config
            if kind in VISA_ROLES:
                kwargs['timeout']=configuration.params['timeout_s']
            else:
                kwargs['io_timeout']=configuration.params['io_timeout_s']
                if kind == 'gain':
                    kwargs['usb_serial']=configuration.expected_identity.get('transport_serial')
        return factory(**kwargs)

    def _retain(self, role, device, context) -> None:
        if self._ownership.get(role) != context:
            raise ConsoleError("device construction lost its ownership reservation")
        self._devices[role] = device
        kind = self._kind(role)
        session_role = 'pm400' if kind in {'mdt', 'laser'} else kind
        self._sessions[role] = InstrumentSession(**{session_role: _LifecycleAdapter(device, self._release_confirmed)})
        if self._domains is not None:
            state=self._domain_state(role)
            state.driver=device
            state.session=self._sessions[role]
            state.release_confirmed=False
            state.resources=(self._resource_keys[role],)
        self._role_cleanup.pop(role, None)
        if kind == 'gain':
            self._gain_set(role,'store',EvidenceStore(context.connection_id,clock=self._scheduler._clock))
            self._gain_set(role,'context',context)
            self._gain_set(role,'round',None)
            self._gain_set(role,'last_command',None)

    def _forget(self, role) -> None:
        kind = self._kind(role)
        if role in self._physical_reservations:
            self._physical_claims.confirm_release(self._physical_reservations[role],
                                                 self._release_evidence.get(role))
            self._physical_reservations.pop(role)
            self._release_evidence.pop(role, None)
        self._devices.pop(role, None)
        self._resources.pop(role, None)
        self._resource_keys.pop(role, None)
        self._ownership.pop(role, None)
        self._sessions.pop(role, None)
        if self._domains is not None:
            state=self._domain_state(role)
            state.driver=None
            state.session=None
            state.resources=()
            state.reservation=None
            state.release_confirmed=True
        if kind == 'gain':
            self._gain_set(role,'store',None)
            self._gain_set(role,'round',None)
            self._gain_set(role,'last_command',None)
        if kind == "fiber":
            self._stage_ports.clear()

    def _connect(self, params: dict[str, Any], context: Context) -> dict[str, Any]:
        role = params.get("role")
        kind = self._kind(role)
        if kind not in ROLES | {'mdt'}:
            raise ConsoleError("unknown instrument role")
        if kind in LIFECYCLE_ACK and params.get("acknowledge_lifecycle") is not True:
            raise ConsoleError(f"{role} connection requires lifecycle-effects acknowledgement")
        resource = params.get("resource")
        if kind != "fiber" and (type(resource) is not str or not resource.strip()):
            raise ConsoleError(f"{role} resource must be selected")
        if kind == "fiber" and resource is not None:
            raise ConsoleError("fiber side binding is fixed by serial number")
        stage_ports = set()
        if kind in SERIAL_ROLES | {"fiber", "mdt"}:
            try:
                stages = FiberCouplingSetup.enumerate(
                    port_enumerator=self._port_enumerator,
                )
            except Exception as error:
                raise ConsoleError(
                    f"cannot verify MDT693B port reservation: {error}"
                ) from error
            stage_ports = {_serial_key(info.resource)
                           for info in stages.registered.values()}
            stage_ports.update(_serial_key(info.resource)
                               for info in stages.unknown_devices)
            if kind == "fiber" and self._domains is not None:
                discovered = {info.serial_number: info for info in stages.registered.values()}
                for member in self._domain_state(role).config.members:
                    serial = member.expected_identity.get("serial",member.expected_identity.get("transport_serial"))
                    info = discovered.get(serial)
                    if info is None or _serial_key(info.resource) != _serial_key(member.params["port"]):
                        raise ConsoleError("Selected fiber identity/address must be reverified")
                stage_ports = {_serial_key(discovered[member.expected_identity.get("serial",member.expected_identity.get("transport_serial"))].resource)
                               for member in self._domain_state(role).config.members}
        if kind in VISA_ROLES:
            resource = resource.strip()
            if self._visa_resource_resolver:
                resource = self._visa_resource_resolver(resource)
            try:
                resource = str(ResourceName.from_string(resource))
            except Exception as error:
                raise ConsoleError(f"select a canonical VISA resource or supply an alias resolver: {error}") from error
        with self._lock:
            self._stage_candidates[context.connection_id] = stage_ports
            try:
                self._reserve(role, resource, context)
            finally:
                self._stage_candidates.pop(context.connection_id, None)
        device = None
        try:
            device = self._construct(role, resource)
            with self._lock:
                self._retain(role, device, context)
            if kind != "fiber":
                device.connect()
                if self._domains is not None:
                    self._verify_configured_identity(role,device)
        except Exception as error:
            cleanup = "no object returned; resource release is unknown"
            released = False
            if device is not None:
                try:
                    cleanup_result = self._cleanup_role(role)
                    released = cleanup_result['connected'] is False
                    cleanup = "resource release confirmed" if released else "cleanup returned without confirmed release"
                except Exception as cleanup_error:
                    cleanup = f"cleanup failed: {cleanup_error}"
            raise CallbackFailure(f"{role} connect failed: {error}; {cleanup}",
                phase="failed_after_call_started", release_confirmed=released,
                status={"connected": False, "resource": resource, "ownership_error": cleanup}) from error
        if kind == "voltage":
            # A successful VoltageSource.connect() performs its startup zero.
            self._publish_voltage(context, [0.0] * 8,role=role)
        return PostReadback({"connected": True, "status": {"resource": resource}})

    def _verify_configured_identity(self,role,device):
        expected=self._domain_state(role).config.expected_identity
        kind=self._kind(role)
        if kind=='osa':
            fields=str(device.identity).split(',')
            observed=dict(zip(('manufacturer','model','serial','firmware'),(part.strip() for part in fields)))
        elif kind=='pm400':
            info=device.instrument_info
            observed={'manufacturer':info.manufacturer,'model':info.model,'serial':info.serial_number,'firmware':info.firmware}
        elif kind=='mdt':
            status=_json_value(device.status)
            observed={'model':status.get('product'),'serial':status.get('serial_number'),'firmware':status.get('firmware')}
        elif kind=='laser':
            observed=dict(device.identity)
        else:
            return # supervised operator-bound identities are not invented serial readback
        for key in ('model','serial','firmware','manufacturer','head_model','head_serial'):
            value=expected.get(key)
            if value is None: continue
            actual=observed.get(key)
            matches=(str(actual).upper().startswith('AQ6370') if key=='model' and kind=='osa'
                     else str(actual).casefold()==value.casefold() if key in {'manufacturer','model'} else actual==value)
            if not matches: raise ConsoleError('Connected instrument identity differs from registered '+key)

    @staticmethod
    def _release_confirmed(device):
        missing = object()
        release_flag = getattr(device, "resources_released", missing)
        open_flag = getattr(device, "is_open", missing)
        return (_state(device) == "DISCONNECTED"
                and (release_flag is missing or release_flag is True)
                and (open_flag is missing or open_flag is False))

    def _disconnect(self, params: dict[str, Any]) -> dict[str, Any]:
        result = self._cleanup_role(params['role'])
        if result['connected']:
            errors = '; '.join(step['error'] for step in result['cleanup']['steps'] if step['error'])
            raise ConsoleError(f"{params['role']} close returned without confirmed release: {errors}")
        return result

    def _prepare_disconnect(self, role):
        """Immediate safety actions only; retain resources until old calls exit."""
        with self._lock:
            device = self._devices.get(role)
        if device is None:
            raise CallbackFailure('Disconnect preparation has no retained device',
                                  phase='failed_after_call_started')
        steps = []
        try:
            kind = self._kind(role)
            if kind == 'voltage':
                device.zero(emergency=True)
                steps.append('zero_all_channels')
            elif kind == 'gain':
                device.disable_current()
                steps.append('disable_current')
                device.disable_tec()
                steps.append('disable_tec')
            else:
                raise ValueError('This kind must wait for quiescence before close')
        except BaseException as error:
            # A final close is still needed even when the initial off call fails.
            return {'connected': True, 'deferred_release': True,
                    'safe_steps': steps, 'preparation_error': str(error)}
        return {'connected': True, 'deferred_release': True, 'safe_steps': steps}

    def _cleanup_role(self, role):
        with self._lock:
            session = self._sessions.get(role)
            reserved = role in self._resources
            if not reserved and session is None:
                previous = copy.deepcopy(self._role_cleanup.get(role))
                return {'role': role, 'connected': False, 'cleanup': previous}
        report = None
        error = None
        if session is not None:
            try:
                report = session.close()
            except BaseException as caught:
                report = getattr(caught, 'cleanup_report', None)
                error = str(caught)
        unresolved = reserved and (report is None or bool(report.unreleased))
        # Laser sessions borrow the helper's close-only PM400 lifecycle slot.
        # Attribute the public evidence to the actual device, leaving the
        # immutable helper report and its release evidence unchanged.
        cleanup_kind = self._kind(role)
        result = {'attempt_id': uuid.uuid4().hex,
                  'steps': [{'role': 'laser' if cleanup_kind == 'laser' and step.role == 'pm400' else step.role,
                             'action': step.action, 'ok': step.error is None,
                             'error': None if step.error is None else str(step.error)}
                            for step in report.steps] if report else [],
                  'unreleased': [role] if unresolved else [],
                  'voltage_zero': _json_value(report.voltage_zero) if report else None}
        if report is None and reserved:
            result['steps'].append({'role': role, 'action': 'close', 'ok': False,
                'error': error or 'no device object; resource release is unknown'})
        with self._lock:
            if report is not None:
                self._release_evidence[role] = report
            if self._kind(role) == 'voltage':
                if self._domains is None:
                    self._requested_voltage = None
                else:
                    self._domain_state(role).requested_voltage=None
            self._role_cleanup[role] = copy.deepcopy(result)
            # Preserve every role report before disconnect can raise or a retry
            # replaces the latest report. Reconnection only clears that latest
            # pointer, never this independent evidence for the old connection.
            owner = self._ownership.get(role)
            self._role_cleanup_attempts.append({
                'role': role, 'context': dataclasses.asdict(owner) if owner else None,
                **copy.deepcopy(result)})
            if self._domains is not None:
                state=self._domain_state(role)
                state.immutable_cleanup=copy.deepcopy(result)
                state.cleanup_attempts.append(copy.deepcopy(self._role_cleanup_attempts[-1]))
        return {'role': role, 'connected': bool(unresolved), 'cleanup': result}

    def _release_role(self, role, context):
        """Registry-only finalization after all role calls have exited."""
        with self._lock:
            owner = self._ownership.get(role)
            if owner is None and role not in self._resources:
                return
            if owner is None or owner.connection_id != context.connection_id:
                raise ConsoleError('cleanup finalization lost its connection ownership')
            self._forget(role)

    def _publish_voltage(self, context, values, *, channel=None,role='voltage'):
        # Hold the dispatch gate through bookkeeping so stop cannot land between
        # checking identity and publishing an older command's requested target.
        with self._scheduler._condition:
            lane = self._scheduler._roles[role]
            if lane.context != context or lane.epoch_exhausted:
                return
            with self._lock:
                if self._domains is not None:
                    state=self._domain_state(role)
                    if channel is None:
                        state.requested_voltage=copy.deepcopy(values)
                    else:
                        requested=list(state.requested_voltage or [None]*8)
                        requested[channel-1]=values
                        state.requested_voltage=requested
                    return
                if channel is None:
                    self._requested_voltage = values
                else:
                    requested = list(self._requested_voltage or [None] * 8)
                    requested[channel - 1] = values
                    self._requested_voltage = requested

    def _action(self, params: dict[str, Any], context: Context) -> dict[str, Any]:
        role = params.get("role")
        kind = self._kind(role)
        name = params.get("name")
        with self._lock:
            device = self._devices.get(role)
        if device is None:
            raise ConsoleError("role is not connected")
        voltage_command_started = False
        driver_call_started = False
        try:
            if kind == 'gain':
                self._gain_command_started(name, context,role)
            if kind == "osa" and (name == "acquire" or
                                  (name == "read_trace" and self._domains is not None)):
                trace = params.get("trace", "A")
                if trace not in ("A", "B", "C", "D", "E", "F", "G"):
                    raise ConsoleError("unsupported OSA trace")
                if self._domains is not None and self._capture_spool is None:
                    raise ConsoleError("Host-owned capture staging is not configured")
                driver_call_started = True
                outcome = (device.read_trace(trace=trace) if name == "read_trace" else
                           device.acquire_trace(trace=trace) if self._domains is not None else
                           device.acquire(trace=trace))
            elif kind == "voltage" and name == "set_channel":
                channel, voltage = params["channel"], params["voltage"]
                voltage_command_started = True
                driver_call_started = True
                device.set_channel(channel, voltage)
                self._publish_voltage(context, float(voltage), channel=channel,role=role)
                outcome = None
            elif kind == "voltage" and name == "set_all":
                values = params["values"]
                voltage_command_started = True
                driver_call_started = True
                device.set_all(values)
                self._publish_voltage(context, [float(value) for value in values],role=role)
                outcome = None
            elif kind == "voltage" and name == "zero":
                voltage_command_started = True
                driver_call_started = True
                device.zero(emergency=True)
                self._publish_voltage(context, [0.0] * 8,role=role)
                outcome = None
            elif kind == "gain" and name == "set_temperature":
                temperature = params["temperature_c"]
                driver_call_started = True
                outcome = device.set_temperature(temperature)
            elif kind == "gain" and name == "set_current":
                current = params["current_ma"]
                driver_call_started = True
                outcome = device.set_current(current)
            elif kind == "gain" and name == "enable_tec":
                driver_call_started = True
                outcome = device.enable_tec()
            elif kind == "gain" and name == "disable_tec":
                driver_call_started = True
                outcome = device.disable_tec()
            elif kind == "gain" and name == "wait_stable":
                timeout = params.get("timeout_s", 60.0)
                driver_call_started = True
                device.wait_stable(timeout=timeout)
                outcome = None
            elif kind == "gain" and name == "enable_current":
                driver_call_started = True
                outcome = device.enable_current()
            elif kind == "gain" and name == "disable_current":
                driver_call_started = True
                outcome = device.disable_current()
            elif kind == "pm400" and name == "measure_power":
                driver_call_started = True
                outcome = device.measure_power()
            elif kind == "pm400" and name in {"measure", "read", "write", "command"}:
                arguments = {key: value for key, value in params.items()
                             if key not in {"role", "name"}}
                outcome = pm400_execute(device, name, arguments)
            elif kind=="pm400" and name in {"measure_kind","read_setting","write_setting","run_maintenance"}:
                arguments={key:value for key,value in params.items() if key not in {"role","name"}}
                outcome=pm400_execute(device,{'measure_kind':'measure','read_setting':'read','write_setting':'write','run_maintenance':'command'}[name],arguments)
            elif kind == "mdt" and name == "read_status":
                outcome = device.status
            elif kind == 'laser':
                if name == 'read_status':
                    # The ordered PostReadback observation below acquires and
                    # publishes one fresh sample. Do not query all 13 values twice.
                    outcome = None
                elif name in {'set_remote', 'set_output', 'set_tracking', 'set_wavelength', 'set_piezo'}:
                    field = {'set_remote':'remote', 'set_output':'enabled', 'set_tracking':'enabled',
                             'set_wavelength':'wavelength_nm', 'set_piezo':'percent'}[name]
                    driver_call_started = True
                    outcome = getattr(device, name)(params[field], confirm=params.get('confirm', False))
                else:
                    raise ConsoleError('Unreviewed laser action')
            elif kind == "fiber" and name in {"adopt_baseline", "move"}:
                side = params.get("side")
                if side not in {"left", "right"}:
                    raise ConsoleError("fiber side must be left or right")
                stage = device.left if side == "left" else device.right
                if name == "adopt_baseline":
                    if params.get("confirm") is not True:
                        raise ConsoleError("baseline adoption requires explicit confirmation")
                    driver_call_started = True
                    outcome = stage.adopt_baseline(
                        confirm=True, allow_nominal=params.get("allow_nominal") is True,
                    )
                else:
                    driver_call_started = True
                    outcome = stage.move_by_um(
                        dx=params.get("dx", 0.0), dy=params.get("dy", 0.0),
                        dz=params.get("dz", 0.0),
                    )
            else:
                raise ConsoleError("action is not exposed for this role")
        except Exception as error:
            if kind == 'gain' and (_state(device) == 'FAULT' or isinstance(error, (DeviceFault, InstrumentSafetyError))):
                with self._scheduler._condition:
                    with self._lock:
                        store = self._gain_get(role,'store')
                        if store is not None:
                            store.invalidate(SWITCHES, 'driver_fault')
            if voltage_command_started:
                self._publish_voltage(context, None,role=role)
            entered = driver_call_started or isinstance(error, PM400InvocationError)
            message = (f"{role} driver call failed: {type(error).__name__}: {error}; "
                       "device outcome is unknown; inspect before retrying") if entered else f"invalid {role} action: {error}"
            raise CallbackFailure(message,
                phase="failed_after_call_started" if entered else "rejected_before_call",
                status={"requested_voltage_v": None} if voltage_command_started else {}) from error
        try:
            if kind == 'osa' and self._domains is not None:
                def cancelled():
                    with self._scheduler._condition:
                        lane = self._scheduler._roles.get(role)
                        return (self._scheduler._closing or lane is None or
                                lane.context != context or lane.epoch_exhausted)
                # No scheduler/controller lock is held while writing files.
                encoded_outcome = self._capture_spool.put(uuid.uuid4().hex, outcome, cancelled=cancelled)
            else:
                encoded_outcome = _json_value(outcome)
        except BaseException as error:
            message = (
                f"{role} command completed, but capture staging failed: {type(error).__name__}: {error}; "
                "no capture published; do not repeat automatically"
                if kind == 'osa' and self._domains is not None else
                f"{role} command completed, but result encoding failed: {type(error).__name__}: {error}; "
                "inspect the device before retrying")
            raise CallbackFailure(
                message,
                phase="completed_readback_failed",
            ) from error
        if kind == 'gain':
            with self._scheduler._condition:
                lane = self._scheduler._roles[role]
                if lane.context == context and not lane.epoch_exhausted:
                    with self._lock:
                        self._gain_set(role,'last_command',{'name': name, 'result': encoded_outcome})
        return PostReadback({"result": encoded_outcome})

    def handle(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        if type(params) is not dict:
            raise ConsoleError("params must be an object")
        role = params.get("role") if method in {"connect", "disconnect", "action", "resume"} else None
        context = self.context(role) if role in ROLES else Context(self._session_id, None, 0)
        outcome = self.submit(Request(uuid.uuid4().hex, method, params, context)).result()
        if outcome.phase != "completed":
            raise ConsoleError(outcome.error["message"])
        if method == "status":
            return self.cached_status()
        if method == "ping":
            with self._lock:
                owned = bool(self._resources)
            return {"mode": "real",
                    "connected": owned}
        return outcome.result

    def _execute(self, request: Request) -> dict:
        method, params = request.method, request.params
        if method in {"settings_get", "settings_save"}:
            if self._management is None:
                raise CallbackFailure("settings management unavailable")
            return self._management(request)
        if method == "inventory":
            return discover()
        if method == "scan_lasers":
            from .discovery import scan_lasers
            return scan_lasers()
        if method == "connect":
            try:
                return self._connect(params, request.context)
            except CallbackFailure:
                raise
            except Exception as error:
                raise CallbackFailure(str(error), release_confirmed=True) from error
        if method == "disconnect":
            try:
                return self._disconnect(params)
            except Exception as error:
                with self._lock:
                    device = self._devices.get(params["role"])
                status = {}
                if device is not None:
                    try:
                        status["state"] = _state(device)
                    except Exception:
                        status["state"] = "UNKNOWN"
                if self._kind(params["role"]) == "voltage":
                    status["requested_voltage_v"] = None
                raise CallbackFailure(str(error), phase="failed_after_call_started", status=status) from error
        if method == "action":
            return self._action(params, request.context)
        if method == "shutdown":
            return self._close_compat()
        raise CallbackFailure("unknown controller method")

    def close(self) -> dict[str, Any]:
        result = self.handle("shutdown", {})
        self._scheduler.join(1.0)
        return result

    def _close_compat(self, *, _additional_report=None) -> dict[str, Any]:
        # All role/global coordinators have settled. Aggregation performs no device I/O.
        additional = copy.deepcopy(_additional_report or {})
        with self._lock:
            reports = copy.deepcopy(self._role_cleanup)
            self._requested_voltage = None
            self._last_cleanup = {
                'attempt_id': uuid.uuid4().hex,
                'steps': [step for report in reports.values() for step in report['steps']] + additional.get('steps', []),
                'unreleased': sorted(set(self._resources) | set(additional.get('unreleased', []))),
                'voltage_zero': reports.get('voltage', {}).get('voltage_zero'),
                'roles': reports,
            }
            if 'probe_cleanup' in additional:
                self._last_cleanup['probe_cleanup'] = additional['probe_cleanup']
            self._cleanup_attempts.append(copy.deepcopy(self._last_cleanup))
            self._closed = not self._last_cleanup['unreleased']
            return copy.deepcopy(self._last_cleanup)


class DomainController(ConsoleController):
    """One owner, many adapters. No controller or dispatch threads per device."""
    def __init__(self, *,session_id,host_verification=False,capture_spool=None,ownership_nonce=None,**options):
        from .domains import DomainRegistry
        from .contracts_v3 import valid_id
        from .captures import CaptureSpool
        if capture_spool is not None and (not isinstance(capture_spool, CaptureSpool) or
                not valid_id(ownership_nonce) or capture_spool.root.name != ownership_nonce):
            raise ValueError('Capture staging requires its exact Host ownership nonce')
        self._capture_spool = capture_spool
        self._capture_nonce = ownership_nonce
        self.registry=DomainRegistry(session_id=session_id)
        super().__init__(_domains=self.registry,**options)
        self._wire_identity["protocol_version"]=3
        self._v3scheduler._host_probe_enabled=host_verification
        if host_verification:
            from .verification import Verification
            self._verification=Verification(registry=self.registry,claims=self._physical_claims,
                                            factories=self._factories)
        self._unpublished_verification=set()

    def configure(self,config):
        return self._v3scheduler.configure(config)

    def context(self,ref):
        return self._v3scheduler.context(ref)

    def device(self,ref):
        return self.registry.get(ref).driver

    def _execute_v3(self,request):
        from .domains import domain_key
        if request.method in {'read_capture_chunk', 'ack_capture'}:
            if (self._capture_spool is None or request.params['ownership_nonce'] != self._capture_nonce):
                raise CallbackFailure('Host-owned capture staging authorization required', phase='rejected_before_call')
            params = request.params
            if request.method == 'read_capture_chunk':
                return self._capture_spool.read(params['capture_id'], params['offset'], params['length'])
            self._capture_spool.acknowledge(params['capture_id'], params['sha256'])
            return {'acknowledged': True}
        if request.method=='register_verified':
            from .contracts_v3 import domain_ref
            params=request.params;ref=domain_ref(params['domain'])
            self._verification.publish(params['proof_id'],ref,params['config_digest'],params['config_rev'])
            self._unpublished_verification.discard(ref)
            return {'identity_bound':True}
        if request.method in {'probe','check_online'}:
            if request.context!=self.context(request.context.domain):
                raise CallbackFailure('Probe context changed before driver entry',phase='rejected_before_call')
            authorization=request.params['authorization']
            if request.method=='check_online':
                from .catalog import load_catalog
                configuration=self.registry.get(request.context.domain).config
                profile=load_catalog().profile(configuration.model_id,configuration.profile_id)
                if profile.probe_mode!='readonly' or not profile.automatic_probe or profile.open_effects:
                    raise CallbackFailure('Profile forbids automatic active checks',phase='rejected_before_call')
                proof=self._verification.run(configuration,authorization)
                self._verification.discard(proof.proof_id)
                return {'identity':dict(proof.identity),'release_confirmed':proof.release_confirmed,'observations':dict(proof.observations)}
            if authorization.get('supervised') is True:
                from types import SimpleNamespace
                controller=authorization.get('binding',{}).get('controller')
                from .contracts_v3 import valid_id
                if not valid_id(controller): raise CallbackFailure('Host controller binding required',phase='rejected_before_call')
                self._verification.authority=SimpleNamespace(controller_id=controller,
                    valid=lambda ref:ref==request.context.domain and self.context(ref)==request.context)
            proof=self._verification.run(self.registry.get(request.context.domain).config,authorization)
            self._unpublished_verification.add(request.context.domain)
            return {'identity':dict(proof.identity),'probe_version':proof.probe_version,
                    'release_confirmed':proof.release_confirmed,'retained_session':proof.retained_session,'controller':proof.controller,'proof_id':proof.proof_id}
        if request.context.domain is None:
            return self._execute(Request(request.id,request.method,request.params,
                Context(request.context.session_id,None,0)))
        key=domain_key(request.context.domain)
        if request.context.domain in self._unpublished_verification and request.method in {'connect','resume','action'} and request.params.get('name') not in {'zero','disable_current','disable_tec'}:
            raise CallbackFailure('Verified identity must be durably registered before ordinary control',phase='rejected_before_call')
        config=self.registry.get(request.context.domain).config
        params=copy.deepcopy(request.params)
        if request.method=="action":
            from .catalog import load_catalog
            name=params["name"]
            allowed=("adopt_baseline","move") if config.driver_kind=="fiber" else load_catalog().model(config.model_id).operations
            if name not in allowed:
                raise CallbackFailure("Action is not exposed for this instrument")
            args=params["args"]
            schemas={
                ("osa","acquire"):((),("trace",)),
                ("osa","read_trace"):((),("trace",)),
                ("voltage","set_channel"):(("channel","voltage"),()),
                ("voltage","set_all"):(("values",),()),
                ("voltage","zero"):((),()),
                ("gain","set_temperature"):(("temperature_c",),()),
                ("gain","set_current"):(("current_ma",),()),
                ("gain","wait_stable"):((),("timeout_s",)),
                ("gain","enable_tec"):((),()),("gain","disable_tec"):((),()),
                ("gain","enable_current"):((),()),("gain","disable_current"):((),()),
                ("pm400","measure_power"):((),()),("mdt","read_status"):((),()),
                ("laser","read_status"):((),()),
                ("laser","set_remote"):(("remote","confirm"),()),
                ("laser","set_output"):(("enabled","confirm"),()),
                ("laser","set_tracking"):(("enabled","confirm"),()),
                ("laser","set_wavelength"):(("wavelength_nm","confirm"),()),
                ("laser","set_piezo"):(("percent","confirm"),()),
                ("pm400","measure_kind"):(("kind",),()),
                ("pm400","read_setting"):(("setting",),("group","selector")),
                ("pm400","write_setting"):(("setting",),("value","group","selector","confirm")),
                ("pm400","run_maintenance"):(("command",),("confirm",)),
                ("fiber","move"):(("side",),("dx","dy","dz")),
                ("fiber","adopt_baseline"):(("side","confirm"),("allow_nominal",)),
            }
            schema=schemas.get((config.driver_kind,name))
            if schema is not None:
                required,optional=schema
                if not set(required)<=set(args) or set(args)-set(required)-set(optional):
                    raise CallbackFailure("Unexpected or missing typed action arguments")
            params={"role":key,"name":name,**args}
        else:
            params={"role":key,**params}
        if request.method=="connect":
            params["resource"]=config.params.get("device_key",config.params.get("port",config.params.get("resource")))
        return self._execute(Request(request.id,request.method,params,
            Context(request.context.session_id,request.context.connection_id,request.context.epoch)))

    def _observe_v3(self,ref,context):
        from .domains import domain_key
        return self._observe(domain_key(ref),Context(context.session_id,context.connection_id,context.epoch))

    def submit(self,request):
        from .contracts_v3 import OutcomeV3
        published=_ReplyFuture(lambda error:self._scheduler._record_callback_error(getattr(request,"id","invalid"),error))
        published.set_running_or_notify_cancel()
        def complete(done):
            outcome=done.result()
            if outcome.phase=="completed" and request.method in {"ping","status"}:
                outcome=OutcomeV3(outcome.phase,outcome.context,self.cached_status())
            published.set_result(outcome)
        self._v3scheduler.submit(request).add_done_callback(complete)
        return published

    def cached_status(self):
        status=self._v3scheduler.status()
        for state in self.registry.states():
            from .domains import domain_key
            status['domains'][domain_key(state.config.domain)]['probe_responsibility']=state.reservation is not None and not state.release_confirmed
        with self._lock:
            owned=set(self._resources)
            cleanup=copy.deepcopy(self._last_cleanup)
            attempts=copy.deepcopy(self._role_cleanup_attempts)
        status["devices"]={key:value for key,value in status["devices"].items() if key in owned}
        # Rebase copied snapshots on acquisition time even while the next read
        # is blocked. Publishing cache age must never issue a hardware getter.
        for device in status["devices"].values():
            if "laser" in device:
                device["sample_age_s"] = _sample_age(device["laser"])
        return {**status,**self._wire_identity,"mode":"real",
                "connected":bool(owned),"last_cleanup":cleanup,"domain_cleanup_attempts":attempts,
                "capture_staging_configured":self._capture_spool is not None,
                "newport_resources_released":newport_resources_released()}

    def _close_compat(self):
        additional={'steps':[],'unreleased':[]}
        if hasattr(self,'_verification'):
            probe=self._verification.close_residuals()
            additional['probe_cleanup']=probe
            additional['steps']+=probe['steps']
            additional['unreleased']+=probe['unreleased']
        if NATIVE.needs_cleanup:
            # Global discovery can own SDK handles without a published domain driver.
            error=None
            try:NATIVE.cleanup_unowned()
            except Exception as failure:error=str(failure)[:512]
            additional['steps'].append({'role':'newport:native','action':'preserving_close','ok':error is None,'error':error})
            if error is not None:additional['unreleased'].append('newport:native')
        # Freeze one complete attempt; the reply, cache and history carry identical evidence.
        return super()._close_compat(_additional_report=additional)

    def close(self):
        from .contracts_v3 import ContextV3,RequestV3
        outcome=self.submit(RequestV3(uuid.uuid4().hex,"shutdown",{},ContextV3(self._session_id,None,None,0))).result()
        if outcome.phase!="completed":
            raise ConsoleError(outcome.error["message"])
        self._scheduler.join(1)
        return outcome.result
