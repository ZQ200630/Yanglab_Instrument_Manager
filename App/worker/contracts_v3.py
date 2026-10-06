"""Strict instance-addressed v3 contracts; no v2 fallback or hardware imports."""
from __future__ import annotations
import copy
from dataclasses import asdict, dataclass
import json
import math
import re
from typing import Any

from .catalog import CatalogError, load_catalog
from .contracts import ProtocolError, PHASES, _unique_object, _reject_constant

MAX_SEQUENCE = 9007199254740991
MAX_REQUEST_BYTES = 65536
FIVE_PHASES = tuple(sorted(PHASES))
METHODS = frozenset({"ping","status","activate","inventory","configure_domain","retire_domain","register_verified",
                     "probe","check_online","connect","disconnect","resume","action","shutdown",
                     "read_capture_chunk","ack_capture"})
DOMAIN_METHODS = frozenset({"probe","check_online","connect","disconnect","resume","action"})
DRIVER_KINDS = frozenset({"osa","voltage","gain","pm400","mdt","fiber"})

def valid_id(value) -> bool:
    return type(value) is str and re.fullmatch(r"[0-9a-f]{32}",value) is not None

def _fields(value, names, optional=()):
    if type(value) is not dict or not set(names) <= set(value) or set(value)-set(names)-set(optional):
        raise ProtocolError("Unexpected or missing v3 fields")
    return value

def _sequence(value, positive=False):
    if type(value) is not int or not (int(positive) <= value <= MAX_SEQUENCE):
        raise ProtocolError("Expected a JS-safe integer")
    return value

def _finite(value):
    pending = [(value,0)]
    while pending:
        item,depth = pending.pop()
        if depth > 32:
            return False
        if type(item) is float:
            if not math.isfinite(item): return False
        elif type(item) is int:
            if abs(item) > MAX_SEQUENCE: return False
        elif type(item) is list:
            pending.extend((child,depth+1) for child in item)
        elif type(item) is dict:
            if any(type(key) is not str for key in item): return False
            pending.extend((child,depth+1) for child in item.values())
        elif item is not None and type(item) not in {str,bool}:
            return False
    return True

@dataclass(frozen=True)
class DomainRef:
    kind: str
    id: str

@dataclass(frozen=True)
class ContextV3:
    session_id: str
    domain: DomainRef | None
    connection_id: str | None
    epoch: int

@dataclass(frozen=True)
class DomainConfig:
    domain: DomainRef
    config_rev: int
    driver_kind: str
    model_id: str
    profile_id: str | None
    params: dict[str,Any]
    expected_identity: dict[str,Any]
    members: tuple["DomainConfig",...]
    def __post_init__(self):
        object.__setattr__(self,"params",copy.deepcopy(self.params))
        object.__setattr__(self,"expected_identity",copy.deepcopy(self.expected_identity))

@dataclass(frozen=True)
class RequestV3:
    id: str
    method: str
    params: dict[str,Any]
    context: ContextV3 | None
    def __post_init__(self):
        object.__setattr__(self,"params",copy.deepcopy(self.params))

@dataclass(frozen=True)
class OutcomeV3:
    phase: str
    context: ContextV3 | None
    result: Any = None
    error: dict[str,str] | None = None
    def __post_init__(self):
        object.__setattr__(self,"result",copy.deepcopy(self.result))
        object.__setattr__(self,"error",copy.deepcopy(self.error))

def domain_ref(value) -> DomainRef:
    _fields(value,("kind","id"))
    if type(value["kind"]) is not str or value["kind"] not in {"device","setup"} or not valid_id(value["id"]):
        raise ProtocolError("Invalid domain identity")
    return DomainRef(value["kind"],value["id"])

def context_v3(value) -> ContextV3 | None:
    if value is None: return None
    _fields(value,("session_id","domain","connection_id","epoch"))
    if not valid_id(value["session_id"]) or (value["connection_id"] is not None and not valid_id(value["connection_id"])):
        raise ProtocolError("Invalid session or connection identity")
    domain = None if value["domain"] is None else domain_ref(value["domain"])
    epoch = _sequence(value["epoch"])
    if domain is None and (value["connection_id"] is not None or epoch != 0):
        raise ProtocolError("Global context requires null connection and epoch zero")
    return ContextV3(value["session_id"],domain,value["connection_id"],epoch)

def domain_config(value) -> DomainConfig:
    _fields(value,("domain","config_rev","driver_kind","model_id","profile_id","params","expected_identity","members"))
    domain = domain_ref(value["domain"])
    revision = _sequence(value["config_rev"],positive=True)
    if type(value["driver_kind"]) is not str or value["driver_kind"] not in DRIVER_KINDS or (
        type(value["params"]) is not dict or type(value["expected_identity"]) is not dict
        or not _finite(value) or type(value["members"]) is not list
    ):
        raise ProtocolError("Invalid domain configuration")
    members = ()
    if domain.kind == "device":
        if value["members"] or value["driver_kind"] == "fiber":
            raise ProtocolError("Physical devices cannot contain setup members")
        try:
            model = load_catalog().model(value["model_id"])
            if model.driver_kind != value["driver_kind"]:
                raise ProtocolError("Trusted model and driver do not match")
            params = load_catalog().profile(value["model_id"],value["profile_id"]).validate(value["params"])
        except CatalogError as error:
            raise ProtocolError(str(error)) from error
    else:
        if value["driver_kind"] != "fiber" or value["model_id"] != "fiber-coupling" or value["profile_id"] is not None:
            raise ProtocolError("Unknown trusted setup binding")
        if not 1 <= len(value["members"]) <= 2:
            raise ProtocolError("Fiber setup requires one or two members")
        for member in value["members"]:
            if type(member) is not dict or member.get("driver_kind") != "mdt" or member.get("members") != []:
                raise ProtocolError("Fiber members must be physical MDT controllers")
        members = tuple(domain_config(member) for member in value["members"])
        if len({member.domain for member in members}) != len(members):
            raise ProtocolError("Duplicate setup member")
        serials = tuple(member.expected_identity.get("serial",member.expected_identity.get("transport_serial")) for member in members)
        if (any(serial not in ("2110148249-10", "160721175410") for serial in serials)
                or len(set(serials)) != len(serials)):
            raise ProtocolError("Fiber members require distinct registered transport serials")
        if set(value["params"]) - {"voltage_limit_v","toward_chip_limit_um","other_limit_um","serial_timeout_s"}:
            raise ProtocolError("Unreviewed setup parameters")
        params = copy.deepcopy(value["params"])
    return DomainConfig(domain,revision,value["driver_kind"],value["model_id"],value["profile_id"],
                        params,value["expected_identity"],members)

def _params(method,params):
    if method not in METHODS or type(params) is not dict or not _finite(params):
        raise ProtocolError("Unknown method or invalid v3 parameters")
    if method in {"ping","status","inventory","disconnect","shutdown"}:
        _fields(params,())
    elif method == "activate":
        _fields(params,("ownership_nonce",))
        if not valid_id(params["ownership_nonce"]): raise ProtocolError("Invalid ownership nonce")
    elif method in {'read_capture_chunk', 'ack_capture'}:
        extra = ('offset', 'length') if method == 'read_capture_chunk' else ('sha256',)
        _fields(params, ('ownership_nonce', 'capture_id', *extra))
        if not valid_id(params['ownership_nonce']) or not valid_id(params['capture_id']):
            raise ProtocolError('Invalid private capture ownership or identity')
        if method == 'read_capture_chunk':
            _sequence(params['offset']); _sequence(params['length'], positive=True)
            if params['length'] > 16384 or params['offset'] + params['length'] > 200001 * 16:
                raise ProtocolError('Capture chunk exceeds bounded native payload')
        elif type(params['sha256']) is not str or re.fullmatch(r'[0-9a-f]{64}', params['sha256']) is None:
            raise ProtocolError('Invalid capture acknowledgement hash')
    elif method == "configure_domain":
        _fields(params,("config",))
        domain_config(params["config"])
    elif method == "retire_domain":
        _fields(params,("domain","config_rev"))
        domain_ref(params["domain"]); _sequence(params["config_rev"],positive=True)
    elif method == 'register_verified':
        _fields(params,('domain','proof_id','config_digest','config_rev'))
        domain_ref(params['domain']);_sequence(params['config_rev'],positive=True)
        if not valid_id(params['proof_id']) or type(params['config_digest']) is not str or re.fullmatch(r'[0-9a-f]{64}',params['config_digest']) is None:
            raise ProtocolError('Invalid private verification publication')
    elif method == "connect":
        _fields(params,(),("acknowledge_lifecycle",))
        if "acknowledge_lifecycle" in params and type(params["acknowledge_lifecycle"]) is not bool:
            raise ProtocolError("Lifecycle acknowledgment must be Boolean")
    elif method == "resume":
        _fields(params,("confirm",))
        if params["confirm"] is not True: raise ProtocolError("Explicit resume confirmation is required")
    elif method in {"probe","check_online"}:
        _fields(params,("authorization",))
        if type(params["authorization"]) is not dict: raise ProtocolError("Probe authorization must be an object")
        if set(params["authorization"]) - {"stage","binding","accepted","supervised","retain_session"}:
            raise ProtocolError("Unknown probe authorization field")
    elif method == "action":
        _fields(params,("name","args"))
        if type(params["name"]) is not str or not params["name"] or len(params["name"]) > 64 or type(params["args"]) is not dict:
            raise ProtocolError("Invalid typed action")
        if set(params["args"]) & {"role","driver_kind","priority","domain","context"}:
            raise ProtocolError("Action cannot override domain or priority")

def parse_v3(line: str) -> RequestV3:
    if type(line) is not str or not line:
        raise ProtocolError("Expected a nonempty JSON line")
    try:
        size = len(line.encode("utf-8"))
    except UnicodeError as error: raise ProtocolError("Invalid UTF-8 request") from error
    if size > MAX_REQUEST_BYTES: raise ProtocolError("V3 request exceeds 64 KiB")
    try:
        value = json.loads(line,object_pairs_hook=_unique_object,parse_constant=_reject_constant)
    except (ValueError,TypeError,RecursionError) as error:
        raise ProtocolError("Invalid v3 JSON: "+str(error)) from error
    _fields(value,("v","id","method","params","context"))
    if type(value["v"]) is not int or value["v"] != 3:
        raise ProtocolError("Unsupported protocol version")
    if type(value["id"]) is not str or not 1 <= len(value["id"]) <= 64 or any(ord(char)<32 for char in value["id"]):
        raise ProtocolError("Invalid v3 request ID")
    method = value["method"]
    if type(method) is not str: raise ProtocolError("Invalid method")
    _params(method,value["params"])
    context = context_v3(value["context"])
    if context is None and method not in {"ping","status"}: raise ProtocolError("Context is required")
    if method in DOMAIN_METHODS and (context is None or context.domain is None):
        raise ProtocolError("Device request requires its domain")
    if method not in DOMAIN_METHODS and context is not None and context.domain is not None:
        raise ProtocolError("Management requires global context")
    return RequestV3(value["id"],method,value["params"],context)

def classify_v3(request: RequestV3, domain_config: DomainConfig | None) -> str:
    if not isinstance(request,RequestV3): raise ProtocolError("Expected a v3 request")
    _params(request.method,request.params)
    if request.method in DOMAIN_METHODS:
        if domain_config is None or request.context is None or request.context.domain != domain_config.domain:
            raise ProtocolError("Request does not match its configured domain")
    if request.method == "action":
        return {("voltage","zero"):"zero",("gain","disable_current"):"current_off",
                ("gain","disable_tec"):"tec_off"}.get((domain_config.driver_kind,request.params["name"]),"normal")
    if request.method in {"ping","status","inventory"}: return "query"
    if request.method in {"disconnect","shutdown","resume"}: return request.method
    return "normal"

def encode_v3(request_id: str, outcome: OutcomeV3) -> str:
    if type(request_id) is not str or not 1 <= len(request_id) <= 64 or not isinstance(outcome,OutcomeV3) or outcome.phase not in PHASES:
        raise ProtocolError("Invalid v3 terminal")
    context = None if outcome.context is None else asdict(outcome.context)
    context_v3(context)
    success = outcome.phase == "completed"
    if success != (outcome.error is None) or (not success and outcome.result is not None):
        raise ProtocolError("Terminal result and error disagree")
    if outcome.error is not None:
        _fields(outcome.error,("type","message"),("attempt_id",))
        if any(type(value) is not str or not value.strip() for value in outcome.error.values()):
            raise ProtocolError("Invalid terminal error")
    result = dict(v=3,id=request_id,ok=success,phase=outcome.phase,context=context)
    result["result" if success else "error"] = outcome.result if success else outcome.error
    try:
        return json.dumps(result,separators=(",",":"),ensure_ascii=False,allow_nan=False)+"\n"
    except (TypeError,ValueError) as error: raise ProtocolError("Invalid v3 terminal value") from error
