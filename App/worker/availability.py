"""Bounded, intent-only availability checks; no enumeration, connection or threads."""
import copy
import dataclasses
import math
import secrets
import time
from dataclasses import dataclass, replace
from threading import RLock
from Code.Utils.common import _freeze_probe
from .catalog import load_catalog
from .contracts import Observation, ProtocolError
from .verification import ProbeEvidence

@dataclass(frozen=True)
class CheckIntent:
    id: str
    boot_id: str
    domain: object
    context: object
    config_rev: int
    model_id: str
    profile_id: str
    probe_version: int
    mode: str
    identity: dict
    stage: str
    started_at: float

    def __post_init__(self):
        object.__setattr__(self,"identity",_freeze_probe(self.identity))

@dataclass(frozen=True)
class DeviceState:
    communication: str = "UNKNOWN"
    quality: str = "authorization_required"
    detected: bool | None = None
    last_success: float | None = None
    next_due: float = 0.0
    observed_age_s: float | None = None

class Availability:
    max_active_idle_probes=1
    def __init__(self,*,registry,boot_id,mode):
        self.registry,self.boot_id,self.mode=registry,boot_id,mode
        self._policies={}
        self._states={}
        self._active={}
        self._idle=None
        self._failures={}
        self._evidence_binding={}
        self._lock=RLock()
        self._last_now=None

    @property
    def active_count(self):
        with self._lock:return len(self._active)

    def authorize(self,ref,policy):
        if (type(policy) is not dict or set(policy)-{"interval_s","enumeration","readonly"}
                or type(policy.get("interval_s",30)) is not int or policy.get("interval_s",30)<10):
            raise ProtocolError("Check interval must be an integer of at least ten seconds")
        self.registry.get(ref)
        for stage in ("enumeration","readonly"):
            binding=policy.get(stage)
            if binding is not None and (type(binding) is not dict or set(binding)!={
                    "mode","identity","config_rev","profile_id","probe_version"}):
                raise ProtocolError("Check consent has an invalid binding")
        with self._lock:
            self._policies[ref]=copy.deepcopy(policy)
            index=next(index for index,state in enumerate(self.registry.states()) if state.config.domain==ref)
            self._states[ref]=DeviceState(next_due=index*0.5)
            self._failures[ref]=0

    def _matches(self,binding,configuration,profile):
        return (type(binding) is dict and type(binding.get("config_rev")) is int
            and type(binding.get("probe_version")) is int and binding=={"mode":self.mode,"identity":configuration.expected_identity,
            "config_rev":configuration.config_rev,"profile_id":configuration.profile_id,"probe_version":profile.probe_version})

    def tick(self,now):
        if type(now) not in (int,float) or not math.isfinite(now):
            raise ProtocolError("Availability requires a finite monotonic time")
        with self._lock:
            if self._last_now is not None and now<self._last_now:
                self._policies.clear()
                for ref,state in tuple(self._states.items()):
                    self._states[ref]=replace(state,quality="clock_discontinuity")
                self._last_now=now
                return []
            self._last_now=now
            intents=[]
            for domain in self.registry.states():
                configuration=domain.config;ref=configuration.domain
                if ref in self._active or ref not in self._policies or configuration.domain.kind!="device":continue
                profile=load_catalog().profile(configuration.model_id,configuration.profile_id)
                policy=self._policies[ref]
                for stage in ("enumeration","readonly"):
                    if policy.get(stage) is not None and not self._matches(policy[stage],configuration,profile):
                        policy[stage]=None
                eligible=[stage for stage in ("readonly","enumeration")
                    if self._matches(policy.get(stage),configuration,profile)
                    and (stage=="enumeration" or profile.probe_mode=="readonly" and profile.automatic_probe)]
                state=self._states[ref]
                if not eligible:
                    self._states[ref]=replace(state,quality="authorization_required")
                    continue
                if now<state.next_due:continue
                if domain.pending:
                    self._states[ref]=replace(state,quality="busy")
                    continue
                if domain.driver is not None or not domain.release_confirmed:
                    # The owned lane's existing observation cycle remains its sole reader.
                    self._states[ref]=replace(state,quality="owned_cache")
                    continue
                if self._idle is not None:continue
                intent=CheckIntent(secrets.token_hex(16),self.boot_id,ref,domain.context,configuration.config_rev,
                    configuration.model_id,configuration.profile_id,profile.probe_version,self.mode,
                    configuration.expected_identity,eligible[0],now)
                self._active[ref]=intent;self._idle=intent.id
                domain.active_probe=intent
                self._states[ref]=replace(state,quality="checking")
                intents.append(intent)
            return intents

    def complete(self,intent,evidence,*,now=None):
        now=time.monotonic() if now is None else now
        if type(now) not in (float,int) or not math.isfinite(now):raise ProtocolError("Invalid observation time")
        with self._lock:
            if self._active.get(intent.domain) is not intent:return None
            try:domain=self.registry.get(intent.domain)
            except ProtocolError:
                self._finish(intent);return None
            config=domain.config
            profile=load_catalog().profile(config.model_id,config.profile_id)
            policy=self._policies.get(intent.domain,{})
            if (intent.boot_id!=self.boot_id or intent.context!=domain.context or intent.config_rev!=config.config_rev
                    or intent.mode!=self.mode or intent.identity!=config.expected_identity
                    or intent.profile_id!=config.profile_id or intent.model_id!=config.model_id
                    or intent.probe_version!=profile.probe_version
                    or not self._matches(policy.get(intent.stage),config,profile)):
                self._finish(intent);return None
            state=self._states[intent.domain]
            success=False;quality="failed";sample_time=None
            if isinstance(evidence,ProbeEvidence):
                success=(evidence.domain==intent.domain and evidence.config_rev==intent.config_rev
                    and evidence.model_id==intent.model_id and evidence.profile_id==intent.profile_id
                    and evidence.mode==intent.mode and evidence.probe_version==intent.probe_version
                    and evidence.identity==intent.identity and evidence.release_confirmed
                    and not evidence.retained_session and intent.started_at<=evidence.issued_monotonic<=now)
                sample_time=evidence.issued_monotonic if success else None
            elif isinstance(evidence,Observation):
                stamp=evidence.status.get("observed_monotonic")
                success=(evidence.status.get("connected") is True and type(stamp) in (float,int)
                    and math.isfinite(stamp) and intent.started_at<=stamp<=now)
                sample_time=stamp if success else None
            elif type(evidence) is dict:
                if intent.stage=="enumeration" and type(evidence.get("detected")) is bool:
                    state=replace(state,detected=evidence["detected"])
                    success=True
                quality=evidence.get("quality","failed")
                if quality not in {"failed","busy","external_owner","cleaning","identity_mismatch"}:quality="failed"
                if evidence.get("release_confirmed") is False:
                    self._states[intent.domain]=replace(state,quality="cleanup_unresolved")
                    return self.state(intent.domain,now)
            if success:
                self._evidence_binding[intent.domain]=self._binding(domain)
                self._failures[intent.domain]=0
                state=replace(state,quality="fresh" if sample_time is not None else "detected",
                    communication="ONLINE" if sample_time is not None else state.communication,
                    last_success=sample_time if sample_time is not None else state.last_success,
                    next_due=now+policy.get("interval_s",30))
            else:
                count=self._failures[intent.domain]
                self._failures[intent.domain]=min(count+1,4)
                state=replace(state,quality=quality,next_due=now+(30,60,120,300,300)[min(count,4)])
            self._states[intent.domain]=state
            domain.communication_evidence=state
            self._finish(intent)
            return self.state(intent.domain,now)

    def _finish(self,intent):
        self._active.pop(intent.domain,None)
        if self._idle==intent.id:self._idle=None
        try:
            state=self.registry.get(intent.domain)
            if state.active_probe is intent:state.active_probe=None
        except ProtocolError:
            pass

    def _binding(self,domain):
        config=domain.config
        return (domain.context,config.config_rev,config.model_id,config.profile_id,
                copy.deepcopy(config.expected_identity),self.mode)

    def record_owned(self,ref,context,observation,*,now):
        from .observations import communication_time
        with self._lock:
            domain=self.registry.get(ref)
            if (domain.context!=context or domain.driver is None or not isinstance(observation,Observation)
                    or observation.status.get("connected") is not True):
                return None
            sample=communication_time(observation,now)
            if sample is None:return None
            previous=self._states.get(ref,DeviceState())
            if previous.last_success is not None and sample<previous.last_success:return None
            self._evidence_binding[ref]=self._binding(domain)
            self._states[ref]=replace(previous,communication="ONLINE",quality="fresh",last_success=sample)
            domain.communication_evidence=self._states[ref]
            return self.state(ref,now)

    def state(self,ref,now):
        with self._lock:
            domain=self.registry.get(ref)
            state=self._states.get(ref,DeviceState())
            if state.last_success is not None and self._evidence_binding.get(ref)!=self._binding(domain):
                return replace(state,communication="UNKNOWN",quality="context_changed",observed_age_s=None)
            age=None if state.last_success is None else now-state.last_success
            ttl=load_catalog().profile(domain.config.model_id,domain.config.profile_id).communication_ttl_s
            stale=age is not None and (age<0 or age>=ttl)
            return replace(state,communication="UNKNOWN" if stale else state.communication,
                observed_age_s=age,quality="stale" if stale and state.quality=="fresh" else state.quality)

