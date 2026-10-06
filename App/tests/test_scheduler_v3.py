"""Reuse the common safety core with bounded instance dispatch."""
from concurrent.futures import Future
import threading
import unittest
from App.worker.contracts import Observation
from App.worker.contracts_v3 import ContextV3, RequestV3
from App.worker.scheduler import PostReadback
from App.tests.test_domains import config, SESSION
try:
    from App.worker.domains import DomainRegistry, SchedulerV3
except ImportError:
    DomainRegistry=SchedulerV3=None

class SchedulerV3Tests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(SchedulerV3,"The shared instance scheduler is not implemented")
        self.registry=DomainRegistry(session_id=SESSION)
        self.calls=[]
        self.release=threading.Event()
        self.entered=threading.Event()
        def execute(request):
            self.calls.append((request.context.domain,request.method,request.params.get("name")))
            if request.method=="connect": return PostReadback({"connected":True})
            if request.method=="shutdown": return {"unreleased":[]}
            if request.method=="disconnect": return {"connected":False,"cleanup":{"unreleased":[]}}
            if request.params.get("name")=="wait_stable":
                self.entered.set()
                if not self.release.wait(3): raise RuntimeError("Fixture deadline")
            return PostReadback({"result":None})
        self.scheduler=SchedulerV3(execute,lambda ref,context:Observation({"state":"READY","connected":True}),
                                   registry=self.registry)
        self.addCleanup(self.cleanup)
        for number in (1,2): self.scheduler.configure(config(number))
    def cleanup(self):
        self.release.set()
        request=RequestV3("cleanup","shutdown",{},ContextV3(SESSION,None,None,0))
        outcome=self.scheduler.submit(request).result(4)
        self.assertEqual(outcome.phase,"completed")
        self.assertTrue(self.scheduler.join(2))
    def submit(self,number,method,params,ident):
        ref=config(number).domain
        return self.scheduler.submit(RequestV3(ident,method,params,self.scheduler.context(ref)))
    def connect(self,number):
        self.assertEqual(self.submit(number,"connect",{"acknowledge_lifecycle":True},f"connect-{number}").result(2).phase,"completed")

    def test_same_kind_action_and_stop_do_not_touch_other_instance(self):
        self.connect(1);self.connect(2)
        before=[call for call in self.calls if call[0]==config(2).domain]
        self.assertEqual(self.submit(1,"action",{"name":"disable_current","args":{}},"stop-one").result(2).phase,"completed")
        self.assertEqual([call for call in self.calls if call[0]==config(2).domain],before)
        self.assertNotEqual(self.scheduler.context(config(1).domain),self.scheduler.context(config(2).domain))
        self.assertEqual(self.scheduler.pending_normal_limit,31)
        self.assertEqual(self.scheduler.reply_slot_limit,226)

    def test_revoke_waits_for_final_cleanup_pass(self):
        self.connect(1);self.connect(2)
        ordinary=self.submit(1,"action",{"name":"wait_stable","args":{}},"old")
        self.assertTrue(self.entered.wait(1))
        stopped=self.submit(1,"action",{"name":"disable_current","args":{}},"stop")
        self.assertFalse(stopped.done())
        self.assertEqual(self.submit(2,"action",{"name":"set_current","args":{"current_ma":2}},"other").result(2).phase,"completed")
        self.release.set()
        ordinary.result(2)
        self.assertEqual(stopped.result(2).phase,"completed")
        safe=[call for call in self.calls if call[0]==config(1).domain and call[2]=="disable_current"]
        self.assertEqual(len(safe),2)

    def test_pre_recovery_status_cannot_clear_unknown_on_another_instance(self):
        self.connect(1);self.connect(2)
        old=self.scheduler.context(config(1).domain)
        self.submit(1,"action",{"name":"disable_current","args":{}},"stop").result(2)
        request=RequestV3("old-write","action",{"name":"set_current","args":{"current_ma":5}},old)
        self.assertEqual(self.scheduler.submit(request).result(1).phase,"rejected_before_call")
        self.assertEqual(self.scheduler.status()["domains"]["device:"+config(2).domain.id]["state"],"READY")

    def test_pool_size_does_not_scale_with_domain_count(self):
        before=self.scheduler.thread_limits
        for number in range(3,65):self.scheduler.configure(config(number))
        self.assertEqual(self.scheduler.thread_limits,before)
        self.assertEqual(before,{"normal":4,"observation":4,"safety":64,"management":1})

    def test_normal_saturation_cannot_consume_safety_or_global_slots(self):
        for number in range(3,65):self.scheduler.configure(config(number))
        for number in range(1,65):self.connect(number)
        ordinary=[]
        for number in range(1,32):
            ordinary.append(self.submit(number,"action",{"name":"wait_stable","args":{}},f"work-{number}"))
        self.assertTrue(self.entered.wait(1))
        rejected=self.submit(32,"action",{"name":"set_current","args":{"current_ma":2}},"overflow").result(1)
        self.assertEqual(rejected.phase,"rejected_before_call")
        safety=[self.submit(number,"action",{"name":"disable_current","args":{}},f"safe-{number}") for number in range(1,65)]
        self.assertTrue(all(not item.done() or item.result().phase=="completed" for item in safety))
        status=self.scheduler.submit(RequestV3("status-now","status",{},None)).result(1)
        self.assertEqual(status.phase,"completed")
        self.release.set()
        for item in ordinary: item.result(2)
        for item in safety:self.assertEqual(item.result(2).phase,"completed")

    def test_retire_waits_for_every_admitted_safety_terminal(self):
        self.connect(1)
        self.registry.get(config(1).domain).release_confirmed=True
        key='device:'+config(1).domain.id
        from App.worker.scheduler import _Work
        from App.worker.contracts import Request
        with self.scheduler.core._condition:
            lane=self.scheduler.core._roles[key]
            lane.context=type(lane.context)(SESSION,None,lane.context.epoch)
            admitted=Request('still-publishing','disconnect',{'role':key},lane.context)
            self.scheduler.core._active_ids[admitted.id]=_Work(admitted,Future())
        try:
            from App.worker.contracts import ProtocolError
            self.assertRaises(ProtocolError,self.scheduler.retire,config(1).domain)
        finally:
            with self.scheduler.core._condition:
                self.scheduler.core._active_ids.pop(admitted.id,None)
