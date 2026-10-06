"""Offline physical claims, alias fencing and explicit setup membership."""
import dataclasses
import unittest
from App.worker.contracts_v3 import DomainRef
from Code.Setups.session import CleanupReport, CleanupStep
from Code.Setups.fiber_coupling import FiberCouplingSetup, StageSide, StageConnectionError
from Code.Debugs.test_fiber_coupling import FakePort, FakeMDTFactory, make_mdt_status
from App.tests.driver_fixture import MDTFacade, factories, fiber_factory
try:
    from App.worker.resources import ResourceClaims, ResourceClaim, ResourceBusy
except ImportError:
    ResourceClaims = ResourceClaim = ResourceBusy = None

LEFT="2110148249-10"
RIGHT="160721175410"
A=DomainRef("device","1"*32)
B=DomainRef("device","2"*32)

class ResourceDomainTests(unittest.TestCase):
    def test_unknown_stage_address_does_not_invent_serial_identity(self):
        from App.worker.resources import instrument_identity
        from App.tests.domain_fixture import config
        for port in ('COM991', 'COM992'):
            cfg = dataclasses.replace(config(30, 'mdt'), params={'port': port, 'baudrate': 115200, 'io_timeout_s': .5})
            self.assertIsNone(instrument_identity(cfg))
            known = dataclasses.replace(cfg, expected_identity={'transport_serial': LEFT})
            self.assertEqual(instrument_identity(known), 'mdt693b:' + LEFT)
    def test_saved_controller_serial_is_accepted_as_explicit_fiber_identity(self):
        from App.tests.domain_fixture import config
        from App.worker.contracts_v3 import domain_config
        import json
        member=json.loads(json.dumps(dataclasses.asdict(dataclasses.replace(config(1,'mdt'),expected_identity={'model':'MDT693B','serial':LEFT}))))
        setup=domain_config(dict(domain={'kind':'setup','id':'f'*32},config_rev=1,driver_kind='fiber',model_id='fiber-coupling',profile_id=None,
            params={},expected_identity={},members=[member]))
        self.assertEqual(setup.members[0].expected_identity['serial'],LEFT)
    def setUp(self):
        self.assertIsNotNone(ResourceClaims,"Physical reservation registry is not implemented")
        self.claims=ResourceClaims()

    def test_serial_asrl_alias_is_reserved_once(self):
        reservation=self.claims.reserve(A,(ResourceClaim(serial_port="COM7"),))
        for alias in ("ASRL7::INSTR","ASRL7"):
            with self.assertRaises(ResourceBusy):
                self.claims.reserve(B,(ResourceClaim(canonical_visa=alias),))
        with self.assertRaises(ResourceBusy):
            self.claims.reserve(B,(ResourceClaim(serial_port="\\\\.\\COM7"),))
        self.assertEqual(reservation.domain,A)

    def test_unresolved_alias_independence_is_rejected(self):
        self.claims.reserve(A,(ResourceClaim(canonical_visa="GPIB0::4::INSTR"),))
        with self.assertRaises(ResourceBusy):
            self.claims.reserve(B,(ResourceClaim(canonical_visa="unresolved-osa-alias"),))

    def test_visa_case_is_preserved_and_different_resources_are_independent(self):
        self.claims.reserve(A,(ResourceClaim(canonical_visa="USB0::0x1313::0x8075::aBC::INSTR"),))
        self.claims.reserve(B,(ResourceClaim(canonical_visa="USB0::0x1313::0x8075::ABC::INSTR"),))
        self.assertEqual(len(self.claims.reservations()),2)

    def test_stable_identity_and_setup_reservation_are_atomic(self):
        raw=self.claims.reserve(A,(ResourceClaim(serial_port="COM9",transport_identity=LEFT),))
        with self.assertRaises(ResourceBusy):
            self.claims.reserve(B,(ResourceClaim(serial_port="COM2",transport_identity=RIGHT),
                                   ResourceClaim(serial_port="COM3",transport_identity=LEFT)))
        self.assertEqual(len(self.claims.reservations()),1)
        self.claims.confirm_release(raw,CleanupReport((CleanupStep("mdt","close",None),),None,()))
        self.claims.reserve(B,(ResourceClaim(serial_port="COM2",transport_identity=RIGHT),
                              ResourceClaim(serial_port="COM3",transport_identity=LEFT)))

    def test_failed_release_and_foreign_reservation_keep_claims(self):
        reservation=self.claims.reserve(A,(ResourceClaim(serial_port="COM7"),))
        failed=CleanupReport((CleanupStep("mdt","close",RuntimeError("held")),),None,(("mdt",object()),))
        with self.assertRaises(ResourceBusy): self.claims.confirm_release(reservation,failed)
        with self.assertRaises(ResourceBusy):
            self.claims.reserve(B,(ResourceClaim(serial_port="COM7"),))
        other=ResourceClaims()
        with self.assertRaises(ResourceBusy):
            other.confirm_release(reservation,CleanupReport((CleanupStep("mdt","close",None),),None,()))

    def test_one_side_setup_uses_serial_not_port_order(self):
        factory=FakeMDTFactory({"COM9":{"status":make_mdt_status(LEFT)},
                                "COM2":{"status":make_mdt_status(RIGHT)}})
        setup=FiberCouplingSetup.for_members((LEFT,),port_enumerator=lambda:[
            FakePort("COM2",RIGHT),FakePort("COM9",LEFT)],driver_factory=factory)
        self.addCleanup(setup.close)
        self.assertEqual(set(factory.instances),{"COM9"})
        self.assertEqual(setup.available_sides,frozenset({StageSide.LEFT}))
        self.assertEqual(setup.discovery.registered[StageSide.LEFT].serial_number,LEFT)
        self.assertEqual(setup.missing_sides,frozenset({StageSide.RIGHT}))

    def test_invalid_or_missing_explicit_members_open_nothing(self):
        factory=FakeMDTFactory({})
        for serials in ((),(LEFT,LEFT),("unknown",),(RIGHT,)):
            with self.subTest(serials=serials),self.assertRaises(StageConnectionError):
                FiberCouplingSetup.for_members(serials,port_enumerator=lambda:[
                    FakePort("COM9",LEFT)],driver_factory=factory)
        self.assertEqual(factory.instances,{})

class ControllerResourceTests(unittest.TestCase):
    def test_same_reported_serial_blocks_duplicate_address(self):
        from App.tests.domain_fixture import config
        one=dataclasses.replace(config(21,'osa'),expected_identity={'model':'AQ6370','serial':'UNITTEST'})
        two=dataclasses.replace(config(22,'osa'),expected_identity={'model':'AQ6370','serial':'UNITTEST'})
        self.controller.configure(one);self.controller.configure(two)
        self.assertEqual(self.call(one,params={'acknowledge_lifecycle':True}).phase,'completed')
        self.assertNotEqual(self.call(two,params={'acknowledge_lifecycle':True}).phase,'completed')
    def test_saved_mdt_serial_blocks_duplicate_address_before_factory(self):
        from App.tests.domain_fixture import config
        opened=[]
        def factory(port, **kwargs):
            opened.append(port);driver=MDTFacade(port, serial=LEFT);return driver
        self.controller._factories['mdt']=factory
        one=dataclasses.replace(config(1,'mdt'),expected_identity={'model':'MDT693B','serial':LEFT})
        two=dataclasses.replace(config(2,'mdt'),expected_identity={'model':'MDT693B','serial':LEFT})
        self.controller.configure(one);self.controller.configure(two)
        self.assertEqual(self.call(one).phase,'completed')
        self.assertNotEqual(self.call(two).phase,'completed')
        self.assertEqual(opened,['COM1'])

    def test_fiber_construction_never_opens_second_discovery_unreserved_port(self):
        from unittest.mock import patch
        from App.worker.controller import DomainController
        from App.tests.domain_fixture import config
        from App.worker.contracts_v3 import DomainConfig,RequestV3
        calls=[]
        def enumerate_ports():
            calls.append(1);return [FakePort('COM1' if len(calls)==1 else 'COM2',LEFT)]
        factory=FakeMDTFactory({'COM1':{'status':make_mdt_status(LEFT)},'COM2':{'status':make_mdt_status(LEFT)}})
        member=dataclasses.replace(config(1,'mdt'),expected_identity={'model':'MDT693B','serial':LEFT})
        setup=DomainConfig(DomainRef('setup','3'*32),1,'fiber','fiber-coupling',None,{}, {},(member,))
        real_for_members=FiberCouplingSetup.for_members
        c=DomainController(session_id='b'*32,port_enumerator=enumerate_ports);self.addCleanup(c.close);c.configure(setup)
        with patch.object(FiberCouplingSetup,'for_members',side_effect=lambda serials,**kwargs:real_for_members(serials,driver_factory=factory,**kwargs)):
            result=c.submit(RequestV3('fiber-open','connect',{},c.context(setup.domain))).result(4)
        self.assertEqual(result.phase,'completed',result)
        self.assertEqual(set(factory.instances),{'COM1'},'construction opened outside the validated/reserved address map')
        self.assertIn(('serial','COM1'),c._physical_claims.reservations()[0].keys)

    def setUp(self):
        from App.worker.controller import DomainController
        ports = [FakePort('COM1', LEFT), FakePort('COM2', RIGHT)]
        sources = factories()
        sources['fiber'] = fiber_factory(ports)
        self.controller=DomainController(session_id="a"*32, factories=sources, port_enumerator=lambda: ports)
        self.addCleanup(self.controller.close)
        self.next_id=0

    def call(self,config,method="connect",params=None):
        from App.worker.contracts_v3 import RequestV3
        self.next_id+=1
        return self.controller.submit(RequestV3(str(self.next_id),method,params or {},
            self.controller.context(config.domain))).result(4)

    def test_transport_identity_blocks_replugged_same_mdt_before_second_open(self):
        from App.tests.domain_fixture import config
        opened=[]
        def factory(port, **kwargs):
            opened.append(port)
            return MDTFacade(port)
        self.controller._factories["mdt"]=factory
        one=dataclasses.replace(config(1,"mdt"),expected_identity={"transport_serial":LEFT})
        two=dataclasses.replace(config(2,"mdt"),expected_identity={"transport_serial":LEFT})
        self.controller.configure(one);self.controller.configure(two)
        self.assertEqual(self.call(one).phase,"completed")
        self.assertNotEqual(self.call(two).phase,"completed")
        self.assertEqual(opened,["COM1"])
        self.assertEqual(self.call(one,"disconnect").phase,"completed")
        self.assertEqual(self.call(two).phase,"completed")

    def test_setup_blocks_raw_member_and_other_setup_then_releases_all_members(self):
        from App.tests.domain_fixture import config
        from App.worker.contracts_v3 import DomainConfig
        left=dataclasses.replace(config(1,"mdt"),expected_identity={"transport_serial":LEFT})
        right=dataclasses.replace(config(2,"mdt"),expected_identity={"transport_serial":RIGHT})
        setup=DomainConfig(DomainRef("setup","3"*32),1,"fiber","fiber-coupling",None,{}, {},(left,))
        other=dataclasses.replace(setup,domain=DomainRef("setup","4"*32),members=(left,right))
        self.controller.configure(left);self.controller.configure(setup);self.controller.configure(other)
        self.assertEqual(self.call(setup).phase,"completed")
        self.assertNotEqual(self.call(left).phase,"completed")
        self.assertNotEqual(self.call(other).phase,"completed")
        driver=self.controller.device(setup.domain)
        self.assertTrue(driver.left.status.available)
        self.assertFalse(driver.right.status.available)
        self.assertNotEqual(self.call(setup,"action",{"name":"adopt_baseline","args":{
            "side":"right","confirm":True,"allow_nominal":True}}).phase,"completed")
        self.assertEqual(self.call(setup,"disconnect").phase,"completed")
        self.assertEqual(self.call(other).phase,"completed")

    def test_invalid_setup_identity_is_rejected_before_factory(self):
        from App.tests.domain_fixture import config
        from App.worker.contracts_v3 import DomainConfig
        from App.worker.contracts import ProtocolError
        member=config(1,"mdt")
        setup=DomainConfig(DomainRef("setup","3"*32),1,"fiber","fiber-coupling",None,{}, {},(member,))
        with self.assertRaises(ProtocolError): self.controller.configure(setup)


