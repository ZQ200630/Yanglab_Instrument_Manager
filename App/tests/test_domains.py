"""Pure registry state is per domain, including same-kind devices."""
import dataclasses
import unittest
import threading
import tempfile
from pathlib import Path
from App.worker.captures import CaptureSpool
from App.worker.contracts import ProtocolError
from App.worker.contracts_v3 import ContextV3, DomainRef, domain_config
from App.worker.contracts_v3 import RequestV3
from App.worker import controller as controller_api
from App.tests.driver_fixture import WireOSA, factories
from App.tests.domain_fixture import config
try:
    from App.worker.domains import DomainRegistry
except ImportError:
    DomainRegistry = None

SESSION="a"*32

class DomainTests(unittest.TestCase):
    def test_failed_connect_with_confirmed_release_reports_new_disconnected_context_and_allows_retry(self):
        instances=[]
        class FirstIdentityMismatch(WireOSA):
            def __init__(self, **kwargs):
                super().__init__(**kwargs);instances.append(self)
                if len(instances)==1:self.test_wire.replies['*IDN?']='YOKOGAWA,AQ6370E,OTHER,1.0'
        cfg=dataclasses.replace(config(26,'osa'),expected_identity={'model':'AQ6370E','serial':'UNITTEST'})
        c=controller_api.DomainController(session_id=SESSION,port_enumerator=lambda:(),factories={'osa':FirstIdentityMismatch})
        self.addCleanup(c.close);c.configure(cfg);before=c.context(cfg.domain)
        out=c.submit(RequestV3('failed-open','connect',{'acknowledge_lifecycle':True},before)).result(4)
        self.assertEqual(out.phase,'failed_after_call_started')
        self.assertIsNone(out.context.connection_id)
        self.assertEqual(out.context,c.context(cfg.domain))
        self.assertGreater(out.context.epoch,before.epoch)
        self.assertEqual(c.cached_status()['domains']['device:'+cfg.domain.id]['state'],'DISCONNECTED')
        self.assertIsNone(c.device(cfg.domain));self.assertEqual(len(instances),1)
        stale=c.submit(RequestV3('old-open','connect',{'acknowledge_lifecycle':True},before)).result(4)
        self.assertEqual(stale.phase,'rejected_before_call');self.assertEqual(len(instances),1)
        retry=c.submit(RequestV3('retry-open','connect',{'acknowledge_lifecycle':True},c.context(cfg.domain))).result(4)
        self.assertEqual(retry.phase,'completed',retry);self.assertEqual(len(instances),2)

    def test_supervised_probe_terminal_preserves_the_live_connection_context(self):
        c=controller_api.DomainController(session_id=SESSION,port_enumerator=lambda: (),factories=factories(),host_verification=True);self.addCleanup(c.close)
        cfg=dataclasses.replace(config(25,'voltage'),expected_identity={});c.configure(cfg)
        self.assertEqual(c.submit(RequestV3('open','connect',{'acknowledge_lifecycle':True},c.context(cfg.domain))).result(4).phase,'completed')
        context=c.context(cfg.domain);self.assertIsNotNone(context.connection_id)
        authorization=dict(accepted=True,stage='supervised',supervised=True,retain_session=True,binding=dict(mode='real',domain=dataclasses.asdict(cfg.domain),config_rev=1,model_id=cfg.model_id,profile_id=cfg.profile_id,config_digest='d'*64,controller='c'*32))
        out=c.submit(RequestV3('probe','probe',{'authorization':authorization},context)).result(4)
        self.assertEqual(out.phase,'completed',out);self.assertEqual(out.context,context)
        self.assertEqual(c.context(cfg.domain),context)

    def test_disconnect_terminal_reports_verified_released_context(self):
        c=controller_api.DomainController(session_id=SESSION,port_enumerator=lambda: (),factories=factories());self.addCleanup(c.close);cfg=config(24,'osa');c.configure(cfg)
        self.assertEqual(c.submit(RequestV3('open','connect',{'acknowledge_lifecycle':True},c.context(cfg.domain))).result(4).phase,'completed')
        out=c.submit(RequestV3('close','disconnect',{},c.context(cfg.domain))).result(4)
        self.assertEqual(out.phase,'completed');self.assertIsNone(out.context.connection_id);self.assertEqual(out.context,c.context(cfg.domain))
    def test_private_idle_check_reads_closes_without_registration_or_control(self):
        controller=controller_api.DomainController(session_id=SESSION,port_enumerator=lambda: (),factories=factories(),host_verification=True)
        self.addCleanup(controller.close)
        for kind in ('osa','gain','mdt'):
            cfg=dataclasses.replace(config({'osa':21,'gain':22,'mdt':23}[kind],kind),expected_identity={})
            controller.configure(cfg)
            authorization=dict(accepted=True,stage='readonly',supervised=False,retain_session=False,
                binding=dict(mode='real',domain=dataclasses.asdict(cfg.domain),config_rev=1,model_id=cfg.model_id,profile_id=cfg.profile_id,config_digest='d'*64))
            result=controller.submit(RequestV3('check-'+kind,'check_online',{'authorization':authorization},controller.context(cfg.domain))).result(4)
            self.assertEqual(result.phase,'completed' if kind=='osa' else 'rejected_before_call',result)
            self.assertIsNone(controller.device(cfg.domain))
            self.assertNotIn(cfg.domain,controller._unpublished_verification)
    def test_v3_pm_settings_are_finite_typed_operations_not_raw_transport(self):
        cfg=domain_config(dict(domain={'kind':'device','id':'e'*32},config_rev=1,driver_kind='pm400',model_id='pm400',profile_id='usb-visa',
            params={'resource':'USB0::0x1313::0x8075::UNITTEST::INSTR'},expected_identity={},members=[]))
        controller=controller_api.DomainController(session_id=SESSION,port_enumerator=lambda: (),factories=factories());self.addCleanup(controller.close);controller.configure(cfg)
        self.assertEqual(controller.submit(RequestV3('open','connect',{},controller.context(cfg.domain))).result(4).phase,'completed')
        result=controller.submit(RequestV3('set','action',{'name':'write_setting','args':{'setting':'sense.wavelength_nm','value':780.0}},controller.context(cfg.domain))).result(4)
        self.assertEqual(result.phase,'completed',result)
        denied=controller.submit(RequestV3('raw','action',{'name':'write','args':{'scpi':'*RST'}},controller.context(cfg.domain))).result(4)
        self.assertEqual(denied.phase,'rejected_before_call')
    def setUp(self):
        self.assertIsNotNone(DomainRegistry,"The instance registry is not implemented")
        self.registry=DomainRegistry(session_id=SESSION)

    def test_registered_identity_mismatch_closes_without_granting_control(self):
        cfg=dataclasses.replace(config(1,'osa'),expected_identity={'model':'AQ6370','serial':'EXPECTED'})
        class WrongOSA(WireOSA):
            def __init__(self,**kwargs):
                super().__init__(**kwargs);self.identity='YOKOGAWA,AQ6370,OTHER,0'
        controller=controller_api.DomainController(session_id=SESSION,port_enumerator=lambda: (),factories={'osa':WrongOSA})
        self.addCleanup(controller.close);controller.configure(cfg)
        result=controller.submit(RequestV3('connect','connect',{'acknowledge_lifecycle':True},controller.context(cfg.domain))).result(3)
        self.assertNotEqual(result.phase,'completed')
        self.assertIsNone(controller.device(cfg.domain))

    def test_same_kind_devices_have_independent_context_evidence_and_cleanup(self):
        for kind in ("gain","voltage","osa"):
            one,two=config(1,kind),config(2,kind)
            registry=DomainRegistry(session_id=SESSION)
            self.assertNotEqual(registry.configure(one),registry.configure(two))
            first,second=registry.get(one.domain),registry.get(two.domain)
            first.evidence.last_command={"name":"disable_current"}
            first.cleanup_attempts.append({"attempt_id":"test"})
            first.requested_voltage=[1.0]*8
            self.assertIsNone(second.evidence.last_command)
            self.assertEqual(second.cleanup_attempts,[])
            self.assertIsNone(second.requested_voltage)
            self.assertIsNot(first.evidence,second.evidence)

    def test_reconfigure_or_retire_cannot_discard_owned_or_pending_responsibility(self):
        value=config()
        self.registry.configure(value)
        state=self.registry.get(value.domain)
        state.driver=object()
        state.release_confirmed=False
        self.assertRaises(ProtocolError,self.registry.configure,config(revision=2))
        self.assertRaises(ProtocolError,self.registry.retire,value.domain)
        state.driver=None
        state.release_confirmed=True
        state.pending=True
        self.assertRaises(ProtocolError,self.registry.retire,value.domain)
        state.pending=False
        self.registry.retire(value.domain)
        self.assertRaises(ProtocolError,self.registry.configure,value)

    def test_maximum_is_execution_domains_not_instrument_kinds(self):
        for number in range(1,65): self.registry.configure(config(number))
        self.assertRaises(ProtocolError,self.registry.configure,config(65))
        self.assertEqual(len(self.registry.states()),64)


class DriverInstancesTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(hasattr(controller_api,"DomainController"),"Per-instance public-driver adapter is not implemented")
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        nonce = 'b' * 32
        root = Path(temporary.name) / nonce
        root.mkdir()
        self.controller=controller_api.DomainController(session_id=SESSION,port_enumerator=lambda: (),factories=factories(),
            capture_spool=CaptureSpool(root, nonce), ownership_nonce=nonce)
        self.addCleanup(self.controller.close)
        self.next_id=0

    def call(self,configuration,method,params):
        self.next_id+=1
        request=RequestV3(str(self.next_id),method,params,self.controller.context(configuration.domain))
        return self.controller.submit(request).result(4)

    def test_two_of_each_supported_kind_keep_commands_and_readbacks_separate(self):
        for index,kind in enumerate(("osa","gain","voltage")):
            one,two=config(index*2+1,kind),config(index*2+2,kind)
            self.controller.configure(one);self.controller.configure(two)
            self.assertEqual(self.call(one,"connect",{"acknowledge_lifecycle":True}).phase,"completed")
            self.assertEqual(self.call(two,"connect",{"acknowledge_lifecycle":True}).phase,"completed")
            before=self.controller.device(two.domain)
            if kind=="gain":
                self.assertEqual(self.call(one,"action",{"name":"set_current","args":{"current_ma":60}}).phase,"completed")
                self.assertEqual(before.status.current_ma,0)
                state=self.controller.registry.get(one.domain)
                self.assertEqual(state.evidence.last_command["name"],"set_current")
            elif kind=="voltage":
                self.assertEqual(self.call(one,"action",{"name":"set_channel","args":{"channel":1,"voltage":1.0}}).phase,"completed")
                self.assertEqual(before.read_status().voltage_v[0],0)
                self.assertEqual(self.controller.registry.get(two.domain).requested_voltage,[0.0]*8)
            else:
                self.assertEqual(self.call(one,"action",{"name":"acquire","args":{}}).phase,"completed")
            self.assertEqual(self.call(one,"disconnect",{}).phase,"completed")
            self.assertEqual(before.state.name,"READY")

    def test_unknown_instance_or_mdt_motion_never_reaches_a_driver(self):
        unknown=RequestV3("unknown","connect",{},ContextV3(SESSION,config(99).domain,None,0))
        self.assertEqual(self.controller.submit(unknown).result(1).phase,"rejected_before_call")
        value=config(1,"mdt")
        self.controller.configure(value)
        self.assertEqual(self.call(value,"connect",{}).phase,"completed")
        device=self.controller.device(value.domain)
        before=dict(device.status.axes)
        rejected=self.call(value,"action",{"name":"set_voltage","args":{"axis":"X","voltage":0}})
        self.assertEqual(rejected.phase,"rejected_before_call")
        self.assertEqual(dict(device.status.axes),before)
        self.assertEqual(self.call(value,"disconnect",{}).phase,"completed")
        self.assertEqual(dict(device.status.axes),before)

    def test_gain_required_readback_failure_faults_only_its_instance(self):
        first,second=config(1),config(2)
        self.controller.configure(first);self.controller.configure(second)
        self.call(first,"connect",{"acknowledge_lifecycle":True})
        self.call(second,"connect",{"acknowledge_lifecycle":True})
        driver=self.controller.device(first.domain)
        original=driver.read_temperature
        def failed(): raise ValueError('Injected readback failure')
        driver.read_temperature=failed
        outcome=self.call(first,"action",{"name":"set_current","args":{"current_ma":3}})
        self.assertEqual(outcome.phase,"completed_readback_failed")
        status=self.controller.cached_status()["domains"]
        self.assertEqual(status["device:"+first.domain.id]["state"],"FAULT")
        self.assertEqual(status["device:"+second.domain.id]["state"],"READY")
        driver.read_temperature=original
        self.assertEqual(self.call(first,"action",{"name":"disable_current","args":{}}).phase,"completed")
        scheduler = self.controller._scheduler
        key = 'device:' + first.domain.id
        with scheduler._condition:
            self.assertTrue(scheduler._condition.wait_for(lambda:
                scheduler._roles[key].healthy_context == scheduler._roles[key].context
                and not scheduler._roles[key].observing
                and not scheduler._roles[key].readback, timeout=2))
            resumed = self.controller.submit(RequestV3('resume-after-readback', 'resume',
                {'confirm': True}, self.controller.context(first.domain)))
        self.assertEqual(resumed.result(2).phase, 'completed')
        self.assertEqual(self.call(second,"action",{"name":"set_current","args":{"current_ma":4}}).phase,"completed")

    def test_old_on_read_cannot_refresh_recovered_or_other_instance_evidence(self):
        first,second=config(1),config(2)
        self.controller.configure(first);self.controller.configure(second)
        self.call(first,"connect",{"acknowledge_lifecycle":True})
        self.call(second,"connect",{"acknowledge_lifecycle":True})
        read_entered,read_release,off_entered,off_release=(threading.Event() for _ in range(4))
        self.addCleanup(read_release.set);self.addCleanup(off_release.set)
        device=self.controller.device(first.domain)
        original_read=device.read_current_enabled
        original_off=device.disable_current
        def read():
            if not read_release.is_set():
                read_entered.set()
                if not read_release.wait(3):raise TimeoutError('Fixture reader deadline')
                return True
            return original_read()
        def off():
            off_entered.set()
            if not off_release.wait(3):raise TimeoutError('Fixture stop deadline')
            return original_off()
        device.read_current_enabled=read;device.disable_current=off
        scheduler=self.controller._scheduler
        key='device:'+first.domain.id
        with scheduler._condition:
            scheduler._roles[key].refresh=True
            scheduler._condition.notify_all()
        self.assertTrue(read_entered.wait(1))
        stopped=self.controller.submit(RequestV3('stop-during-read','action',{'name':'disable_current','args':{}},self.controller.context(first.domain)))
        self.assertTrue(off_entered.wait(1))
        read_release.set()
        with scheduler._condition:
            self.assertTrue(scheduler._condition.wait_for(lambda:not scheduler._roles[key].observing,1))
        fields=self.controller.cached_status()['devices'][key]['fields']
        self.assertEqual(fields['current_enabled']['quality'],'unknown')
        self.assertIs(fields['current_enabled']['value'],False)
        other=self.controller.cached_status()['devices']['device:'+second.domain.id]['fields']
        self.assertEqual(other['current_enabled']['quality'],'fresh')
        self.assertIs(other['current_enabled']['value'],False)
        off_release.set()
        self.assertEqual(stopped.result(2).phase,'completed')
