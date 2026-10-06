"""Host-local physical claims; independent of driver VISA leases and external owners.

Reservation never opens an interface. Only the owning lifecycle coordinator may
supply a completed immutable cleanup report after every old call has returned.
"""
from dataclasses import dataclass
import re
from threading import RLock
from pyvisa.rname import ResourceName
from Code.Setups.session import CleanupReport
from .contracts_v3 import DomainRef

class ResourceBusy(RuntimeError):
    pass

@dataclass(frozen=True)
class ResourceClaim:
    canonical_visa: str | None = None
    transport_identity: str | None = None
    serial_port: str | None = None
    instrument_identity: str | None = None

def instrument_identity(config):
    serial=config.expected_identity.get('serial')
    if serial is None and config.driver_kind=='mdt':
        serial=config.expected_identity.get('transport_serial')
    if type(serial) is str and serial.strip():
        return config.model_id+':'+serial.strip()
    return None

@dataclass(frozen=True)
class Reservation:
    domain: DomainRef
    claims: tuple[ResourceClaim,...]
    keys: frozenset[tuple[str,str]]
    token: object

def serial_key(port):
    if type(port) is not str or not port.strip():
        raise ResourceBusy("Invalid serial address")
    value=port.strip()
    if value.startswith("\\\\.\\"): value=value[4:]
    match=re.fullmatch(r"COM([0-9]+)",value,re.IGNORECASE)
    return "COM"+str(int(match[1])) if match else port.strip().casefold()

def keys_for(claim):
    if not isinstance(claim,ResourceClaim): raise ResourceBusy("Expected a physical claim")
    keys=set()
    if claim.serial_port is not None: keys.add(("serial",serial_key(claim.serial_port)))
    if claim.canonical_visa is not None:
        try:
            resource=ResourceName.from_string(claim.canonical_visa.strip())
        except Exception as error:
            raise ResourceBusy("VISA alias independence is unproven; resolve it before opening") from error
        canonical=str(resource)
        if resource.interface_type == "ASRL":
            keys.add(("serial",serial_key("COM"+str(resource.board))))
        else: keys.add(("visa",canonical))
    if claim.transport_identity is not None:
        if type(claim.transport_identity) is not str or not claim.transport_identity.strip():
            raise ResourceBusy("Invalid stable transport identity")
        keys.add(("identity",claim.transport_identity))
    if claim.instrument_identity is not None:
        if type(claim.instrument_identity) is not str or not claim.instrument_identity.strip():
            raise ResourceBusy('Invalid instrument identity')
        keys.add(('instrument',claim.instrument_identity))
    if not keys: raise ResourceBusy("Empty physical claim")
    return keys

class ResourceClaims:
    def __init__(self):
        self._lock=RLock()
        self._active={}

    def reserve(self,domain:DomainRef,claims:tuple[ResourceClaim,...]) -> Reservation:
        if not isinstance(domain,DomainRef) or type(claims) is not tuple or not claims:
            raise ResourceBusy("A domain and nonempty physical claims are required")
        keys=set()
        for claim in claims:
            part=keys_for(claim)
            if keys & part: raise ResourceBusy("Repeated physical member in a reservation")
            keys.update(part)
        with self._lock:
            if any(item.domain==domain or item.keys & keys for item in self._active.values()):
                raise ResourceBusy("Physical resource is owned by another responsibility")
            token=object()
            value=Reservation(domain,claims,frozenset(keys),token)
            self._active[token]=value
            return value

    def confirm_release(self,reservation:Reservation,evidence:CleanupReport) -> None:
        with self._lock:
            if not isinstance(reservation,Reservation) or self._active.get(reservation.token) is not reservation:
                raise ResourceBusy("Reservation does not belong to this coordinator")
            if not isinstance(evidence,CleanupReport) or evidence.unreleased or not evidence.steps:
                raise ResourceBusy("Cleanup has not confirmed resource release")
            del self._active[reservation.token]

    def reservations(self):
        with self._lock:return tuple(self._active.values())

