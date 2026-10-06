"""Availability schedules authorized intents only; never opens a transport."""
import dataclasses
import unittest
from App.tests.domain_fixture import config
from App.worker.domains import DomainRegistry
from App.worker.contracts_v3 import ContextV3
from App.worker.verification import ProbeEvidence
try:
    from App.worker.availability import Availability
except ImportError:
    Availability=None

class AvailabilityTests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(Availability,"Availability intent coordinator is missing")
        self.registry=DomainRegistry(session_id="a"*32)
        self.cfg=dataclasses.replace(config(1,"osa"),expected_identity={"model":"AQ6370D","serial":"SN"})
        self.registry.configure(self.cfg)
        self.availability=Availability(registry=self.registry,boot_id="b"*32,mode="real")

    def policy(self,stage="readonly"):
        return {"interval_s":30,stage:{"mode":"real","identity":self.cfg.expected_identity,
            "config_rev":1,"profile_id":"gpib-visa","probe_version":1}}

    def success(self,intent):
        return ProbeEvidence("c"*32,self.cfg.domain,"aq6370","gpib-visa","d"*64,1,
            self.cfg.expected_identity,"real",1,intent.started_at,True)

    def test_default_start_does_not_open_ports(self):
        self.assertEqual(self.availability.tick(0),[])
        self.assertEqual(self.availability.state(self.cfg.domain,0).communication,"UNKNOWN")

    def test_busy_probe_deferred_without_offline_claim(self):
        self.availability.authorize(self.cfg.domain,self.policy())
        self.registry.get(self.cfg.domain).pending=True
        self.assertEqual(self.availability.tick(0),[])
        state=self.availability.state(self.cfg.domain,0)
        self.assertEqual(state.communication,"UNKNOWN")
        self.assertEqual(state.quality,"busy")
        self.registry.get(self.cfg.domain).pending=False
        self.assertEqual(len(self.availability.tick(1)),1)

    def test_late_success_cannot_mark_rebound_device_online(self):
        self.availability.authorize(self.cfg.domain,self.policy())
        intent=self.availability.tick(0)[0]
        self.registry.get(self.cfg.domain)._context=ContextV3("a"*32,self.cfg.domain,"e"*32,1)
        self.assertIsNone(self.availability.complete(intent,self.success(intent),now=1))
        self.assertEqual(self.availability.state(self.cfg.domain,1).communication,"UNKNOWN")

    def test_backoff_is_bounded_and_success_restores_interval(self):
        self.availability.authorize(self.cfg.domain,self.policy())
        now=0;delays=[]
        for expected in (30,60,120,300,300):
            intent=self.availability.tick(now)[0]
            state=self.availability.complete(intent,{"quality":"failed","release_confirmed":True},now=now)
            delays.append(state.next_due-now)
            now+=expected
        self.assertEqual(delays,[30,60,120,300,300])
        intent=self.availability.tick(now)[0]
        state=self.availability.complete(intent,self.success(intent),now=now)
        self.assertEqual(state.next_due-now,30)
        self.assertEqual(state.communication,"ONLINE")
        self.assertEqual(self.availability.state(self.cfg.domain,now+90).communication,"UNKNOWN")

    def test_detected_resource_is_not_online_and_stages_have_separate_consent(self):
        self.availability.authorize(self.cfg.domain,self.policy("enumeration"))
        intent=self.availability.tick(0)[0]
        self.assertEqual(intent.stage,"enumeration")
        state=self.availability.complete(intent,{"detected":True},now=0)
        self.assertTrue(state.detected)
        self.assertEqual(state.communication,"UNKNOWN")

    def test_changed_profile_or_mode_revokes_previous_permission(self):
        self.availability.authorize(self.cfg.domain,self.policy())
        self.registry.get(self.cfg.domain).config=dataclasses.replace(self.cfg,config_rev=2)
        self.assertEqual(self.availability.tick(0),[])
        self.assertEqual(self.availability.state(self.cfg.domain,0).quality,"authorization_required")
        self.registry.get(self.cfg.domain).config=self.cfg
        self.availability.authorize(self.cfg.domain,{**self.policy(),"readonly":dict(
            self.policy()["readonly"],mode="simulate")})
        self.assertEqual(self.availability.tick(0),[])

    def test_source_gain_and_unproven_dtr_profiles_never_idle_open(self):
        for number,kind in ((2,"gain"),(3,"voltage"),(4,"mdt")):
            cfg=config(number,kind);self.registry.configure(cfg)
            policy={"interval_s":30,"readonly":{"mode":"real","identity":cfg.expected_identity,
                "config_rev":1,"profile_id":cfg.profile_id,"probe_version":1}}
            self.availability.authorize(cfg.domain,policy)
        self.assertEqual(self.availability.tick(0),[])

    def test_sixty_four_devices_have_one_idle_probe_and_no_queue_growth(self):
        for number in range(2,65):
            cfg=dataclasses.replace(config(number,"osa"),expected_identity=self.cfg.expected_identity)
            self.registry.configure(cfg)
            policy={"interval_s":30,"readonly":dict(self.policy()["readonly"])}
            self.availability.authorize(cfg.domain,policy)
        self.availability.authorize(self.cfg.domain,self.policy())
        self.assertEqual(len(self.availability.tick(100)),1)
        self.assertEqual(self.availability.tick(100),[])
        self.assertEqual(self.availability.max_active_idle_probes,1)
        self.assertEqual(self.availability.active_count,1)

    def test_explicit_port_busy_keeps_previous_success_but_does_not_claim_off(self):
        self.availability.authorize(self.cfg.domain,self.policy())
        intent=self.availability.tick(0)[0]
        self.availability.complete(intent,self.success(intent),now=0)
        intent=self.availability.tick(30)[0]
        state=self.availability.complete(intent,{"quality":"external_owner"},now=30)
        self.assertEqual(state.quality,"external_owner")
        self.assertEqual(state.communication,"ONLINE")
        self.assertEqual(state.last_success,0)

    def test_configuration_edit_revokes_cached_online_without_another_tick(self):
        self.availability.authorize(self.cfg.domain,self.policy())
        intent=self.availability.tick(0)[0]
        self.availability.complete(intent,self.success(intent),now=0)
        self.registry.get(self.cfg.domain).config=dataclasses.replace(self.cfg,config_rev=2)
        self.assertEqual(self.availability.state(self.cfg.domain,1).communication,"UNKNOWN")

    def test_owned_cache_does_not_open_or_rejuvenate_old_evidence(self):
        from App.worker.contracts import Observation
        self.availability.authorize(self.cfg.domain,self.policy())
        state=self.registry.get(self.cfg.domain)
        state.driver=object()
        self.assertEqual(self.availability.tick(0),[])
        observed=Observation({"connected":True,"observed_monotonic":0.0})
        self.availability.record_owned(self.cfg.domain,state.context,observed,now=0)
        self.availability.record_owned(self.cfg.domain,state.context,observed,now=89)
        self.assertEqual(self.availability.state(self.cfg.domain,90).communication,"UNKNOWN")
        self.assertEqual(self.availability.active_count,0)

    def test_unreleased_probe_keeps_single_responsibility_until_explicit_release(self):
        self.availability.authorize(self.cfg.domain,self.policy())
        intent=self.availability.tick(0)[0]
        result=self.availability.complete(intent,{"quality":"cleaning","release_confirmed":False},now=1)
        self.assertEqual(result.quality,"cleanup_unresolved")
        self.assertEqual(self.availability.active_count,1)
        from App.worker.contracts import ProtocolError
        with self.assertRaises(ProtocolError): self.registry.retire(self.cfg.domain)
        self.assertEqual(self.availability.tick(100),[])
        self.availability.complete(intent,{"quality":"failed","release_confirmed":True},now=100)
        self.assertEqual(self.availability.active_count,0)


