"""Registration evidence never substitutes enumeration or loses a failed owner."""
import dataclasses
import unittest
from unittest.mock import Mock
from App.tests.domain_fixture import config
from App.worker.contracts import ProtocolError
from App.worker.domains import DomainRegistry
from App.worker.resources import ResourceClaims
from Code.Utils.common import ProbeReport, DriverState, InstrumentConnectionError
try:
    from App.worker.verification import Verification, StaleProof
except ImportError:
    Verification=StaleProof=None

class FakeProbe:
    def __init__(self,resource_name=None,port=None,identity=None,close_error=None):
        self.identity=identity or {"model":"AQ6370D","serial":"SN"}
        self.close_error=close_error
        self.has_resource_responsibility=False
        self.calls=[]
    def probe_identity(self):
        self.calls.append("probe")
        self.has_resource_responsibility=bool(self.close_error)
        report=ProbeReport(self.identity,{},not self.has_resource_responsibility)
        if self.close_error:
            error=InstrumentConnectionError("probe close incomplete")
            error.probe_report=report
            raise error
        return report
    def close(self):
        self.calls.append("close")
        if self.close_error: raise self.close_error
        self.has_resource_responsibility=False

class RegistrationProbeTests(unittest.TestCase):
    def test_residual_probe_cleanup_reports_each_attempt_and_retains_failures(self):
        self.driver.close_error=OSError('held')
        with self.assertRaises(InstrumentConnectionError):self.verifier.run(self.cfg,self.authorization())
        first=self.verifier.close_residuals()
        self.assertEqual(first['unreleased'],['device:'+self.cfg.domain.id])
        self.driver.close_error=None
        second=self.verifier.close_residuals()
        self.assertEqual(second['unreleased'],[])
        self.assertEqual(first['unreleased'],['device:'+self.cfg.domain.id])
        self.assertTrue(self.registry.get(self.cfg.domain).release_confirmed)
    def setUp(self):
        self.assertIsNotNone(Verification,"Draft verification coordinator is missing")
        self.registry=DomainRegistry(session_id="a"*32)
        self.now=0.0
        self.driver=FakeProbe()
        self.verifier=Verification(registry=self.registry,claims=ResourceClaims(),
            clock=lambda:self.now,factories={"osa":lambda **kw:self.driver})
        self.cfg=config(1,"osa")
        self.registry.configure(self.cfg)

    def authorization(self):
        return {"stage":"readonly","accepted":True,"supervised":False,"retain_session":False,
                "binding":{"mode":"real","domain":dataclasses.asdict(self.cfg.domain),
                    "config_rev":self.cfg.config_rev,"model_id":self.cfg.model_id,
                    "profile_id":self.cfg.profile_id,"config_digest":"d"*64}}

    def test_probe_proof_expires_at_60_seconds(self):
        proof=self.verifier.run(self.cfg,self.authorization())
        self.assertTrue(proof.valid_at(59.999))
        self.assertFalse(proof.valid_at(60.0))
        self.assertFalse(proof.valid_at(-1))
        self.assertTrue(proof.release_confirmed)

    def test_publish_binds_only_worker_issued_identity_without_transport_rebinding(self):
        proof=self.verifier.run(self.cfg,self.authorization())
        bound=self.verifier.publish(proof.proof_id,self.cfg.domain,'d'*64,1)
        self.assertEqual(bound.expected_identity['serial'],'SN')
        self.assertEqual(bound.params,self.cfg.params)
        with self.assertRaises(StaleProof):
            self.verifier.publish(proof.proof_id,self.cfg.domain,'d'*64,1)

    def test_real_proof_cannot_validate_obsolete_mode(self):
        proof=self.verifier.run(self.cfg,self.authorization())
        with self.assertRaises(StaleProof):
            self.verifier.validate(proof,self.cfg,mode="simulate",config_digest="d"*64)
        self.assertEqual(proof.mode,"real")

    def test_configuration_change_and_proof_reuse_are_rejected(self):
        proof=self.verifier.run(self.cfg,self.authorization())
        changed=dataclasses.replace(self.cfg,config_rev=2)
        with self.assertRaises(StaleProof):
            self.verifier.validate(proof,changed,mode="real",config_digest="d"*64)
        self.verifier.validate(proof,self.cfg,mode="real",config_digest="d"*64,consume=True)
        with self.assertRaises(StaleProof):
            self.verifier.validate(proof,self.cfg,mode="real",config_digest="d"*64)

    def test_close_failure_retains_driver_and_blocks_new_attempt(self):
        self.driver.close_error=OSError("still held")
        with self.assertRaises(InstrumentConnectionError):
            self.verifier.run(self.cfg,self.authorization())
        state=self.registry.get(self.cfg.domain)
        self.assertIs(state.driver,self.driver)
        self.assertFalse(state.release_confirmed)
        with self.assertRaises(ProtocolError): self.verifier.run(self.cfg,self.authorization())
        with self.assertRaises(ProtocolError): self.verifier.cancel(self.cfg.domain)
        self.assertIs(state.driver,self.driver)
        self.driver.close_error=None
        self.verifier.cancel(self.cfg.domain)
        with self.assertRaises(ProtocolError): self.registry.configure(self.cfg)

    def test_identity_mismatch_cannot_issue_proof(self):
        self.driver.identity={"model":"other","serial":"SN"}
        with self.assertRaises(ProtocolError): self.verifier.run(self.cfg,self.authorization())
        self.assertTrue(self.registry.get(self.cfg.domain).release_confirmed)

    def test_source_and_gain_are_not_implicitly_connected(self):
        for number,kind in ((2,"gain"),(3,"voltage")):
            cfg=config(number,kind);self.registry.configure(cfg)
            auth=self.authorization()
            auth["binding"].update(domain=dataclasses.asdict(cfg.domain),model_id=cfg.model_id,
                                    profile_id=cfg.profile_id)
            with self.assertRaises(ProtocolError): self.verifier.run(cfg,auth)
        self.assertEqual(self.driver.calls,[])

    def test_save_failure_keeps_supervised_session(self):
        cfg=config(2,"gain")
        self.registry.configure(cfg)
        state=self.registry.get(cfg.domain)
        owner=Mock()
        owner.state=DriverState.READY
        state.driver=owner;state.release_confirmed=False
        authority=Mock()
        authority.valid.return_value=True
        self.verifier.authority=authority
        auth=self.authorization()
        auth.update(stage="supervised",supervised=True,retain_session=True)
        auth["binding"].update(domain=dataclasses.asdict(cfg.domain),model_id=cfg.model_id,
                                profile_id=cfg.profile_id)
        proof=self.verifier.run(cfg,auth)
        def failed_save():
            self.verifier.validate(proof,cfg,mode="real",config_digest="d"*64)
            raise OSError("Host durable configuration write failed")
        with self.assertRaises(OSError): failed_save()
        self.verifier.validate(proof,cfg,mode="real",config_digest="d"*64)
        self.assertFalse(proof.release_confirmed)
        self.assertTrue(proof.retained_session)
        self.assertIs(state.driver,owner)
        self.assertFalse(state.release_confirmed)

