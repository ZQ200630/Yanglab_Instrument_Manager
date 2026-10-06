"""Draft-bound probe evidence; production dispatch remains gated by Host leases."""
import dataclasses
import math
import secrets
import time
from dataclasses import dataclass
from threading import RLock
from Code.Utils import AQ6370, PM400, MDT693B
from Code.Utils.common import ProbeReport, DriverState, _freeze_probe
from Code.Setups.session import CleanupReport, CleanupStep
from .catalog import load_catalog
from .contracts import ProtocolError
from .contracts_v3 import DomainConfig, domain_config
from .resources import ResourceClaim, instrument_identity

class StaleProof(ProtocolError):
    pass

@dataclass(frozen=True)
class ProbeEvidence:
    proof_id: str
    domain: object
    model_id: str
    profile_id: str
    config_digest: str
    config_rev: int
    identity: dict
    mode: str
    probe_version: int
    issued_monotonic: float
    release_confirmed: bool
    retained_session: bool = False
    controller: str | None = None
    observations: dict = None

    def __post_init__(self):
        object.__setattr__(self,"identity",_freeze_probe(self.identity))
        object.__setattr__(self,"observations",_freeze_probe(self.observations or {}))

    def valid_at(self,now):
        return (type(now) in (float,int) and math.isfinite(now)
                and 0 <= now-self.issued_monotonic < 60)

class Verification:
    def __init__(self,*,registry,claims,factories=None,clock=time.monotonic,authority=None):
        self.registry,self.claims=registry,claims
        self.factories=dict(factories or {})
        self.clock,self.authority=clock,authority
        self._proofs={}
        self._consumed=set()
        self._lock=RLock()

    def run(self,config:DomainConfig,authorization:dict) -> ProbeEvidence:
        with self._lock:
            state=self.registry.get(config.domain)
            if state.config!=config: raise ProtocolError("Draft configuration changed")
            profile=load_catalog().profile(config.model_id,config.profile_id)
            mode="real"
            binding=authorization.get("binding",{}) if type(authorization) is dict else {}
            expected={"mode":mode,"domain":dataclasses.asdict(config.domain),
                "config_rev":config.config_rev,"model_id":config.model_id,"profile_id":config.profile_id}
            if (authorization.get("accepted") is not True or any(binding.get(key)!=value for key,value in expected.items())
                    or type(binding.get("config_digest")) is not str or len(binding["config_digest"])!=64):
                raise ProtocolError("Explicit connection consent must bind this draft and mode")
            retained=authorization.get("retain_session") is True
            if profile.probe_mode=="supervised":
                if (authorization.get("stage")!="supervised" or authorization.get("supervised") is not True
                        or not retained or self.authority is None
                        or self.authority.valid(config.domain) is not True or state.driver is None
                        or getattr(state.driver.state,'name',None) not in ('READY','ACTIVE')):
                    raise ProtocolError("Manual verification required: prepare an independently authorized controlled session")
                identity={"model":load_catalog().model(config.model_id).name,
                          "transport_serial":config.expected_identity.get("transport_serial"),
                          "operator_binding":"Operator-attested supervised session"}
                identity={key:value for key,value in identity.items() if value is not None}
                report=ProbeReport(identity,{"supervised":True},False)
                controller=str(self.authority.controller_id)
            else:
                if (profile.probe_mode!="readonly" or authorization.get("stage")!="readonly" or retained
                        or authorization.get("supervised") is not False):
                    raise ProtocolError("Read-only probe consent is required")
                if state.driver is not None or state.reservation is not None or not state.release_confirmed:
                    raise ProtocolError("Previous probe ownership is retained")
                address=config.params.get("port",config.params.get("resource"))
                identity=instrument_identity(config)
                claim=(ResourceClaim(canonical_visa=address,instrument_identity=identity) if profile.access=="visa" else
                       ResourceClaim(serial_port=address,transport_identity=config.expected_identity.get("transport_serial"),instrument_identity=identity))
                reservation=self.claims.reserve(config.domain,(claim,))
                state.reservation=reservation;state.release_confirmed=False
                kwargs={"resource_name":address} if profile.access=="visa" else {"port":address}
                factory=self.factories.get(config.driver_kind)
                if factory is None:
                    factory={
                        "osa":AQ6370,"pm400":PM400,"mdt":MDT693B}[config.driver_kind]
                try:
                    driver=factory(**kwargs)
                    state.driver=driver
                    report=driver.probe_identity()
                except BaseException as error:
                    report=getattr(error,"probe_report",None)
                    if isinstance(report,ProbeReport) and report.release_confirmed:
                        self._release(state,report)
                    state.cleanup_attempts.append({"probe":True,"released":state.release_confirmed,
                                                  "error":type(error).__name__})
                    raise
                if not isinstance(report,ProbeReport) or not report.release_confirmed:
                    raise ProtocolError("Probe release is unresolved")
                self._release(state,report)
                controller=None
            model=load_catalog().model(config.model_id)
            reported=report.identity.get("model","")
            if not (type(reported) is str and (reported.upper().startswith("AQ6370") if config.driver_kind=="osa"
                    else reported.casefold()==model.name.casefold())):
                raise ProtocolError("Probe identity does not match the trusted model")
            expected_serial=config.expected_identity.get("serial",config.expected_identity.get("transport_serial"))
            observed_serial=report.identity.get("serial",report.identity.get("transport_serial"))
            if expected_serial is not None and observed_serial!=expected_serial:
                raise ProtocolError("Probe serial identity mismatch")
            proof=ProbeEvidence(secrets.token_hex(16),config.domain,config.model_id,config.profile_id,
                binding["config_digest"],config.config_rev,report.identity,mode,profile.probe_version,
                self.clock(),report.release_confirmed,retained,controller,report.observations)
            for old_id,old in tuple(self._proofs.items()):
                if old.domain==config.domain:
                    del self._proofs[old_id]
                    self._consumed.discard(old_id)
            self._proofs[proof.proof_id]=proof
            return proof

    def _release(self,state,report):
        self.claims.confirm_release(state.reservation,CleanupReport((CleanupStep("probe","close",None),),None,()))
        state.driver=None;state.reservation=None;state.release_confirmed=True

    def validate(self,proof,config,*,mode,config_digest,consume=False):
        with self._lock:
            if (self._proofs.get(proof.proof_id) is not proof or proof.proof_id in self._consumed
                    or not proof.valid_at(self.clock()) or proof.domain!=config.domain
                    or proof.config_rev!=config.config_rev or proof.model_id!=config.model_id
                    or proof.profile_id!=config.profile_id or proof.mode!=mode
                    or proof.config_digest!=config_digest
                    or proof.probe_version!=load_catalog().profile(config.model_id,config.profile_id).probe_version):
                raise StaleProof("Verification expired, was consumed, or no longer matches this draft")
            if proof.retained_session and (self.authority is None or self.authority.valid(config.domain) is not True):
                raise StaleProof("Supervised controller authority ended")
            if consume:self._consumed.add(proof.proof_id)
            return proof

    def cancel(self,ref):
        with self._lock:
            state=self.registry.get(ref)
            if state.driver is not None:
                try:
                    state.driver.close()
                    if state.driver.has_resource_responsibility:
                        raise ProtocolError("Cancelled draft still holds resources")
                    if state.reservation is None:
                        raise ProtocolError("Supervised cancellation requires the control coordinator")
                    self._release(state,None)
                except BaseException as error:
                    raise ProtocolError("Draft cleanup remains retained") from error
            self.registry.retire(ref)
            for proof_id,proof in tuple(self._proofs.items()):
                if proof.domain==ref: del self._proofs[proof_id]

    def publish(self,proof_id,ref,config_digest,config_rev):
        with self._lock:
            proof=self._proofs.get(proof_id)
            if proof is None: raise StaleProof('Worker proof not found')
            state=self.registry.get(ref)
            self.validate(proof,state.config,mode='real',config_digest=config_digest)
            if config_rev!=state.config.config_rev: raise StaleProof('Configuration revision changed')
            # Refine identity only from this worker's evidence. Addresses, model,
            # profiles and outputs are unchanged; no arbitrary rebind is allowed.
            state.config=dataclasses.replace(state.config,expected_identity=dict(proof.identity))
            self._consumed.add(proof_id)
            return state.config

    def discard(self,proof_id):
        with self._lock:
            self._proofs.pop(proof_id,None)
            self._consumed.discard(proof_id)

    def close_residuals(self):
        from .domains import domain_key
        import uuid
        with self._lock:
            steps=[];unreleased=[]
            for state in self.registry.states():
                if state.release_confirmed or state.reservation is None:continue
                key=domain_key(state.config.domain)
                try:
                    if state.pending:raise ProtocolError('Probe call is still pending')
                    if state.driver is not None:
                        state.driver.close()
                        if state.driver.has_resource_responsibility:raise ProtocolError('Probe resource remains held')
                    self._release(state,None)
                    steps.append({'role':key,'action':'close_probe','error':None})
                except BaseException as error:
                    unreleased.append(key);steps.append({'role':key,'action':'close_probe','error':type(error).__name__})
            return {'attempt_id':uuid.uuid4().hex,'steps':steps,'unreleased':unreleased}

