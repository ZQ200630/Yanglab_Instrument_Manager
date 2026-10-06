"""V3 ingress retains the full 225 liabilities, separate from device calls."""
import io
import json
from concurrent.futures import Future
import threading
import unittest
from App.worker.main import _Replies, _read_lines
from App.worker.contracts import ProtocolError
from App.worker.contracts_v3 import ContextV3, OutcomeV3, RequestV3
from App.tests.test_domains import config, SESSION

class IngressV3Tests(unittest.TestCase):
    def test_every_domain_safety_chain_has_reserved_terminal_capacity(self):
        entered,release=threading.Event(),threading.Event()
        frames=[]
        class Output:
            def write(self,line):
                entered.set()
                if not release.wait(3):raise TimeoutError("Fixture output deadline")
                frames.append(json.loads(line))
            def flush(self):pass
        configurations={config(number).domain:config(number) for number in range(1,65)}
        replies=_Replies(Output(),version=3,config_resolver=configurations.__getitem__)
        def accept(id,method,params,context):
            request=RequestV3(id,method,params,context)
            self.assertIsNone(replies.reserve(request))
            completed=Future()
            completed.set_result(OutcomeV3("completed",context,{"unreleased":[]} if method=="shutdown" else {}))
            replies.publish(request,completed)
        try:
            for number in range(1,32):
                context=ContextV3(SESSION,config(number).domain,"c"*32,0)
                accept(f"normal-{number}","action",{"name":"set_current","args":{"current_ma":1}},context)
            self.assertTrue(entered.wait(1))
            for number in range(1,65):
                context=ContextV3(SESSION,config(number).domain,"c"*32,0)
                accept(f"off-{number}","action",{"name":"disable_current","args":{}},context)
                accept(f"tec-{number}","action",{"name":"disable_tec","args":{}},context)
                accept(f"close-{number}","disconnect",{},context)
            global_context=ContextV3(SESSION,None,None,0)
            accept("query","status",{},global_context)
            accept("shutdown","shutdown",{},global_context)
            self.assertEqual(len(replies.active),225)
            self.assertEqual(replies.queue.maxsize,226)
            replies.reject("invalid",None,"Rejected bounded input")
            self.assertEqual(replies.queue.qsize(),225)
        finally:
            release.set()
            replies.closed.set()
            replies.thread.join(3)
        self.assertFalse(replies.thread.is_alive())
        self.assertEqual(len(frames),226)
        self.assertEqual(len({frame["id"] for frame in frames}),226)
        self.assertTrue(all(frame["v"]==3 for frame in frames))

    def test_reader_rejects_oversized_terminated_line_before_queueing(self):
        import os
        import queue
        read_fd,write_fd=os.pipe()
        source=os.fdopen(read_fd,"r",encoding="utf-8")
        incoming=queue.Queue()
        stopped=threading.Event()
        reader=threading.Thread(target=_read_lines,args=(source,incoming,stopped,65536))
        reader.start()
        try:
            os.write(write_fd,b"x"*65537+b"\n")
            os.close(write_fd);write_fd=None
            reader.join(2)
            result=incoming.get(timeout=1)
            self.assertTrue(isinstance(result,ProtocolError),'Oversized input must be rejected before queueing')
            self.assertNotIsInstance(result,str)
        finally:
            stopped.set()
            if write_fd is not None:os.close(write_fd)
            reader.join(2)
            source.close()
