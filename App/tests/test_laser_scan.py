"""Read-only enumeration contracts with a finite enumerator, never hardware."""
import unittest
from App.worker import discovery
from App.worker.contracts_v3 import parse_v3, classify_v3
import json
import io
import threading
from unittest.mock import patch
from App.worker import main

class LaserScanTests(unittest.TestCase):
    def test_two_controllers_keep_exact_serials_without_opening_a_driver(self):
        self.assertTrue(callable(getattr(discovery, 'scan_lasers', None)))
        reply = discovery.scan_lasers(enumerator=lambda: ('6700 SN1012', '6700 SN1020'))
        self.assertEqual(reply, {'controllers': [
            {'device_key':'6700 SN1012', 'serial':'1012'},
            {'device_key':'6700 SN1020', 'serial':'1020'}]})

    def test_unknown_or_duplicate_identity_is_rejected_and_empty_scan_is_valid(self):
        self.assertTrue(callable(getattr(discovery, 'scan_lasers', None)))
        self.assertEqual(discovery.scan_lasers(enumerator=lambda: ()), {'controllers':[]})
        for values in [('6700 SN1','6700 SN1'), ('USB1',), tuple('6700 SN'+str(x) for x in range(33))]:
            with self.assertRaises(ValueError): discovery.scan_lasers(enumerator=lambda: values)

    def test_real_worker_ingress_dispatches_scan_through_the_query_scheduler(self):
        bootstrap = main._bootstrap_v3(ownership_nonce='b'*32)
        context = dict(session_id=bootstrap.session_id,domain=None,connection_id=None,epoch=0)
        lines = [dict(v=3,id='activate',method='activate',params={'ownership_nonce':'b'*32},context=context),
                 dict(v=3,id='scan',method='scan_lasers',params={},context=context),
                 dict(v=3,id='stop',method='shutdown',params={},context=context)]
        scan_flushed=threading.Event()
        class SequentialInput:
            def __init__(self):self.index=0
            def readline(self,maximum):
                if self.index==2 and not scan_flushed.wait(5):raise AssertionError('scan reply was not flushed')
                if self.index>=len(lines):return ''
                line=json.dumps(lines[self.index])+'\n';self.index+=1;return line
        class Output(io.StringIO):
            def write(self,line):
                result=super().write(line)
                if json.loads(line)['id']=='scan':scan_flushed.set()
                return result
        output = Output()
        with patch('Code.Utils.tlb6700.TLB6700.enumerate',return_value=('6700 SN1012',)) as enumeration:
            self.assertEqual(main.run_v3(bootstrap,input_stream=SequentialInput(),output_stream=output),0)
        replies = {x['id']:x for x in map(json.loads,output.getvalue().splitlines())}
        self.assertTrue(replies['scan']['ok'],replies['scan'])
        self.assertEqual(replies['scan']['result'],{'controllers':[{'device_key':'6700 SN1012','serial':'1012'}]})
        enumeration.assert_called_once_with()
        self.assertEqual(replies['stop']['result']['unreleased'],[])

    def test_scan_requires_global_context_and_has_no_selector_or_command_arguments(self):
        base=dict(v=3,id='scan',method='scan_lasers',params={},context=dict(session_id='a'*32,domain=None,connection_id=None,epoch=0))
        self.assertEqual(classify_v3(parse_v3(json.dumps(base)),None),'query')
        base['params']={'command':'*RST'}
        with self.assertRaises(ValueError): parse_v3(json.dumps(base))

if __name__=='__main__': unittest.main()
