"""Instance registry and v3 facade over the existing lifecycle/safety core."""
from __future__ import annotations
import copy
from concurrent.futures import Future
from dataclasses import asdict, dataclass, field
from threading import RLock
import json
from .contracts import Context, Request, Observation, ProtocolError
from .contracts_v3 import ContextV3, DomainConfig, DomainRef, OutcomeV3, RequestV3, domain_config, parse_v3, valid_id
from .observations import EvidenceStore
from .scheduler import Scheduler, _ReplyFuture

def domain_key(ref:DomainRef) -> str:
    return ref.kind+":"+ref.id

@dataclass
class EvidenceState:
    store: EvidenceStore | None = None
    context: Context | None = None
    round: dict | None = None
    last_command: dict | None = None

@dataclass
class DomainState:
    config: DomainConfig
    _context: ContextV3
    driver: object | None = None
    resources: tuple[str,...] = ()
    reservation: object | None = None
    communication_evidence: object | None = None
    active_probe: object | None = None
    session: object | None = None
    evidence: EvidenceState = field(default_factory=EvidenceState)
    requested_voltage: list | None = None
    fiber_ports: set = field(default_factory=set)
    cleanup_attempts: list = field(default_factory=list)
    immutable_cleanup: dict | None = None
    release_confirmed: bool = True
    pending: bool = False
    retired: bool = False
    context_provider: object | None = None

    @property
    def context(self):
        return self.context_provider() if self.context_provider is not None else self._context

class DomainRegistry:
    def __init__(self, *,session_id:str):
        if not valid_id(session_id): raise ProtocolError("Invalid worker session")
        self.session_id=session_id
        self.lock=RLock()
        self._states={}
        self._retired=set()

    def configure(self,config:DomainConfig) -> ContextV3:
        if not isinstance(config,DomainConfig): raise ProtocolError("Expected validated domain configuration")
        config=domain_config(json.loads(json.dumps(asdict(config),allow_nan=False)))
        with self.lock:
            if config.domain in self._retired: raise ProtocolError("Retired domain identities cannot be reused")
            previous=self._states.get(config.domain)
            if previous is not None:
                if previous.driver is not None or not previous.release_confirmed or previous.pending or previous.active_probe is not None:
                    raise ProtocolError("Domain responsibility is retained")
                if config.config_rev<=previous.config.config_rev:
                    raise ProtocolError("Configuration revision must increase")
                if any(getattr(config,name)!=getattr(previous.config,name) for name in
                       ("driver_kind","model_id","profile_id","params","expected_identity","members")):
                    raise ProtocolError("Physical rebinding requires a new domain identity")
                previous.config=config
                previous.evidence=EvidenceState()
                previous.communication_evidence=None
                return previous.context
            if len(self._states)>=64: raise ProtocolError("Execution-domain capacity is 64")
            context=ContextV3(self.session_id,config.domain,None,0)
            self._states[config.domain]=DomainState(config,context)
            return context

    def get(self,ref:DomainRef) -> DomainState:
        with self.lock:
            try:return self._states[ref]
            except (KeyError,TypeError) as error: raise ProtocolError("Unknown domain") from error

    def states(self):
        with self.lock:return tuple(self._states.values())

    def retire(self,ref:DomainRef):
        with self.lock:
            state=self.get(ref)
            if state.pending or state.driver is not None or not state.release_confirmed or state.active_probe is not None:
                raise ProtocolError("Confirmed release and quiescence are required")
            state.retired=True
            self._retired.add(ref)
            del self._states[ref]

class SchedulerV3:
    pending_normal_limit=31
    reply_slot_limit=226
    responsibility_limit=225
    thread_limits={"normal":4,"observation":4,"safety":64,"management":1}

    def __init__(self,execute,observe,*,registry:DomainRegistry,clock=None):
        self.registry=registry
        self._execute=execute
        self._observe=observe
        self._refs={}
        self._management_requests={}
        self._host_probe_enabled=False
        options={} if clock is None else {"clock":clock}
        self.core=Scheduler(self._execute_internal,self._observe_internal,session_id=registry.session_id,
                            role_kinds={},fixed_pools=True,**options)
        self._release_domain=lambda ref,context:None
        self._invalidate_domain=lambda ref,context:None
        self._overlay_domain=lambda ref,context,status:status
        self.core._release_role=self._release_internal
        self.core._invalidate_role=self._invalidate_internal
        self.core._status_overlay=self._overlay_internal

    def _context(self,key,context):
        return ContextV3(context.session_id,self._refs.get(key),context.connection_id,context.epoch)

    def configure(self,config:DomainConfig):
        key=domain_key(config.domain)
        with self.core._condition:
            if self.core._closing:
                raise ProtocolError('Cannot configure a closing worker')
            lane=self.core._roles.get(key)
            if lane is not None and (lane.context.connection_id is not None or lane.active or lane.queue or
                    lane.observing or lane.readback or lane.safety.active or
                    any(work.request.params.get('role')==key for work in self.core._active_ids.values())):
                raise ProtocolError("Cannot configure an owned or pending domain")
            self.registry.configure(config)
            if lane is None:
                self._refs[key]=config.domain
                self.core.add_lane(key,config.driver_kind)
            self.registry.get(config.domain).context_provider=lambda k=key:self.context(self._refs[k])
            return self.context(config.domain)

    def context(self,ref):
        with self.core._condition:
            self.registry.get(ref)
            return self._context(domain_key(ref),self.core.context(domain_key(ref)))

    def retire(self,ref):
        key=domain_key(ref)
        with self.core._condition:
            lane=self.core._roles[key]
            if lane.context.connection_id is not None or lane.active or lane.queue or lane.observing or lane.readback or lane.safety.active or any(
                    work.request.params.get('role')==key for work in self.core._active_ids.values()):
                raise ProtocolError("Cannot retire a live responsibility")
            self.registry.retire(ref)
            del self.core._roles[key]
            del self.core._role_kinds[key]
            del self._refs[key]
            self.core._condition.notify_all()

    def _release_internal(self,key,context):
        ref=self._refs[key]
        self._release_domain(ref,self._context(key,context))
        state=self.registry.get(ref)
        state.release_confirmed=True

    def _invalidate_internal(self,key,context):
        self._invalidate_domain(self._refs[key],self._context(key,context))

    def _overlay_internal(self,key,context,status):
        return self._overlay_domain(self._refs[key],self._context(key,context),status)

    def _execute_internal(self,request):
        key=request.params.get("role")
        if request.id in self._management_requests:
            original=self._management_requests.pop(request.id)
            if original.method=='configure_domain':
                config=domain_config(original.params['config'])
                return {'context':asdict(self.configure(config)),'config_rev':config.config_rev}
            if original.method=='retire_domain':
                from .contracts_v3 import domain_ref
                ref=domain_ref(original.params['domain'])
                if self.registry.get(ref).config.config_rev!=original.params['config_rev']:
                    raise ProtocolError('Configuration revision changed')
                self.retire(ref)
                return {'retired':True}
            return self._execute(original)
        params={key_:value for key_,value in request.params.items() if key_ not in {"role","name"}}
        if request.method=="action":
            params={"name":request.params["name"],"args":params}
        elif request.method=="connect":
            params.pop("resource",None)
        context=self._context(key,request.context)
        return self._execute(RequestV3(request.id,request.method,params,context))

    def _observe_internal(self,key,context):
        return self._observe(self._refs[key],self._context(key,context))

    def submit(self,request:RequestV3) -> Future:
        published=_ReplyFuture(lambda error:self.core._record_callback_error(getattr(request,"id","invalid"),error))
        published.set_running_or_notify_cancel()
        try:
            request=parse_v3(json.dumps({"v":3,"id":request.id,"method":request.method,"params":request.params,
                "context":None if request.context is None else asdict(request.context)},allow_nan=False))
            if request.method=='activate' or (request.method in {'probe','check_online'} and not self._host_probe_enabled):
                raise ProtocolError("Internal management or verification requires its Host coordinator")
            ref=request.context.domain if request.context is not None else None
            key=None if ref is None else domain_key(ref)
            params=copy.deepcopy(request.params)
            if ref is not None:
                state=self.registry.get(ref)
                params={"role":key,**(params.get("args",{}) if request.method=="action" else params)}
                if request.method=="action":
                    params["name"]=request.params["name"]
                if request.method=="connect":
                    params["resource"]=state.config.params.get("port",state.config.params.get("resource"))
            context=None if request.context is None else Context(request.context.session_id,request.context.connection_id,request.context.epoch)
            internal=Request(request.id,request.method,params,context)
            if request.method in {'configure_domain','retire_domain','probe','check_online','register_verified',
                                  'read_capture_chunk','ack_capture'}:
                if request.id in self._management_requests:
                    raise ProtocolError('Management request ID already exists')
                self._management_requests[request.id]=request
                internal=Request(request.id,'settings_save',{},Context(context.session_id,None,0))
        except (ProtocolError,ValueError,TypeError,AttributeError) as error:
            published.set_result(OutcomeV3("rejected_before_call",ContextV3(self.registry.session_id,None,None,0),
                error={"type":type(error).__name__,"message":str(error)}))
            return published
        if ref is not None:
            state.pending=True
        def complete(done):
            if self._management_requests.get(request.id) is request:
                self._management_requests.pop(request.id,None)
            outcome=done.result()
            if ref is not None:
                with self.core._condition:
                    lane=self.core._roles.get(key)
                    state.pending=bool(lane and (lane.active or lane.queue or lane.readback or lane.safety.active))
            terminal_context=ContextV3(outcome.context.session_id,ref,outcome.context.connection_id,outcome.context.epoch)
            if request.method in {'probe','check_online'}:
                # The serialized management lane is global, but a probe owns
                # this domain. Preserve its actual live/held connection context.
                terminal_context=self.context(ref)
            if request.method=='disconnect' and outcome.phase=='completed' and outcome.result.get('connected') is False and not outcome.result.get('cleanup',{}).get('unreleased',['unknown']):
                terminal_context=self.context(ref)
            published.set_result(OutcomeV3(outcome.phase,terminal_context,outcome.result,outcome.error))
        self.core.submit(internal).add_done_callback(complete)
        return published

    def status(self):
        with self.core._condition:
            status=self.core.status()
            roles=status.pop("roles")
            status["domains"]={key:{**value,"context":asdict(self._context(key,self.core.context(key)))}
                               for key,value in roles.items()}
            return status

    def join(self,timeout):
        return self.core.join(timeout)
