"""Durable native Host ledger contracts with bounded transport injection."""
import json
from pathlib import Path
import time
from App.tests import test_local_leases as fixtures

class OperationLedgerTests(fixtures.LocalLeaseTests):
    def operation(self, method='connect', params=None, sequence=1):
        domain={'kind':'device','id':f'{1:032x}'}
        lease=self.client.call('acquire_control',{'domain':domain})['result']
        cached=self.client.call('worker_status')
        self.assertTrue(cached['ok'],cached)
        return dict(domain=domain,lease_token=lease['token'],control_epoch=lease['control_epoch'],
            config_rev=1,context=cached['result']['domains']['device:'+domain['id']]['context'],
            method=method,params=params if params is not None else {'acknowledge_lifecycle':True},sequence=sequence,confirmation=None)

    def wait_operation(self, request_id):
        deadline=time.monotonic()+4
        while True:
            reply=self.client.call('operation',{'request_id':request_id})
            self.assertTrue(reply['ok'],reply)
            if reply['result']['status']!='Accepted': return reply['result']
            self.assertLess(time.monotonic(),deadline)
            time.sleep(.02)

    def test_duplicate_executes_connect_once_and_changed_args_conflict(self):
        intent=self.operation()
        prepared=self.client.call('prepare',{'intent':intent})
        self.assertTrue(prepared['ok'],prepared)
        intent['confirmation']=prepared['result']['token']
        first=self.client.call('execute',{'request_id':'lost-receipt','intent':intent})
        self.assertTrue(first['ok'],first)
        finished=self.wait_operation('lost-receipt')
        self.assertEqual(finished['phase'],'completed',finished)
        duplicate=self.client.call('execute',{'request_id':'lost-receipt','intent':intent})
        self.assertTrue(duplicate['ok'],duplicate)
        self.assertEqual(duplicate['result']['operation_id'],first['result']['operation_id'])
        intent['params']={}
        changed=self.client.call('execute',{'request_id':'lost-receipt','intent':intent})
        self.assertEqual(changed['error']['code'],'RequestConflict')
        state=self.client.call('worker_status')['result']['domains']['device:'+intent['domain']['id']]
        self.assertEqual(state['context']['connection_id'],finished['result']['context']['connection_id'])

    def test_journal_failure_rejects_before_call_but_safe_stop_still_runs(self):
        intent=self.operation();proof=self.client.call('prepare',{'intent':intent})
        self.assertTrue(proof['ok'],proof);intent['confirmation']=proof['result']['token']
        journal=Path(self.directory.name)/'operations.json'
        journal.unlink();journal.mkdir()
        failed=self.client.call('execute',{'request_id':'disk-full','intent':intent})
        self.assertEqual(failed['error']['code'],'OperationStorage')
        state=self.client.call('worker_status')['result']['domains']['device:'+intent['domain']['id']]
        self.assertIsNone(state['context']['connection_id'])
        (Path(self.directory.name)/'safety-audit.json').mkdir()
        safe=self.client.call('safe_stop',{'domain':intent['domain']})
        self.assertTrue(safe['ok'],safe)
        self.assertIsNotNone(safe['result']['audit_error'])
