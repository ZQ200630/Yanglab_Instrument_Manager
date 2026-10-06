"""Native pipe contracts, not GUI, hardware or remote-network acceptance."""
import time
from App.tests import test_local_host_process as f
class LocalHostAcceptanceTests(f.LocalHostProcessTests):
    def test_single_fiber_setup_holds_and_can_be_retired(self):
        self._fiber_case(1)
    def test_dual_fiber_setup_holds_and_can_be_retired(self):
        self._fiber_case(2)
    def _fiber_case(self,count):
        members=[]
        for index,port in enumerate(('COM991','COM992'),1):
            rev=self.client.call('snapshot')['result']['registry']['registry_rev']
            d=self.client.call('create_draft',dict(model_id='mdt693b',profile_id='serial',params={'port':port},name=f'Stage {index}',expected_rev=rev))['result']
            domain=dict(kind='device',id=d['device_id']);lease=self.client.call('acquire_control',dict(domain=domain))['result']
            consent=dict(accepted=True,mode='real',config_digest=d['config_digest'],open_effects=['DTR_RTS_reset_not_verified'],supervised=False,retain_session=False)
            proof=self.client.call('test_connection',dict(draft_id=d['device_id'],expected_rev=rev+1,consent=consent,lease_token=lease['token'],control_epoch=lease['control_epoch'],request_id=f'stage-{index}',sequence=index))
            self.assertTrue(proof['ok'],proof)
            saved=self.client.call('save_device',dict(draft_id=d['device_id'],proof_id=proof['result']['proof_id'],expected_rev=rev+1));self.assertTrue(saved['ok'],saved)
            self.assertEqual(saved['result']['verified_mode'],'real');members.append(domain['id'])
        for count in (count,):
            rev=self.client.call('snapshot')['result']['registry']['registry_rev']
            response=self.client.call('save_setup',dict(name='Fiber',members=members[:count],expected_rev=rev));self.assertTrue(response['ok'],response);setup=response['result']
            domain=dict(kind='setup',id=setup['setup_id'])
            self.assertEqual(self.client.call('acquire_control',dict(domain=dict(kind='device',id=members[0])))['error']['code'],'SetupOwned')
            lease=self.client.call('acquire_control',dict(domain=domain))['result']
            snapshot=self.client.call('snapshot')['result'];ctx=snapshot['domains']['setup:'+setup['setup_id']]['context']
            intent=dict(domain=domain,lease_token=lease['token'],control_epoch=lease['control_epoch'],config_rev=1,context=ctx,method='connect',params={},sequence=10+count,confirmation=None)
            intent['confirmation']=self.client.call('prepare',dict(intent=intent))['result']['token']
            receipt=self.client.call('execute',dict(request_id=f'fiber-{count}',intent=intent));self.assertTrue(receipt['ok'],receipt)
            deadline=time.monotonic()+5
            while True:
                op=self.client.call('operation',dict(request_id=f'fiber-{count}'))['result']
                if op['status']!='Accepted':break
                self.assertLess(time.monotonic(),deadline);time.sleep(.02)
            self.assertEqual(op['phase'],'completed',op)
            self.assertTrue(self.client.call('safe_stop',dict(domain=domain))['ok'])
            while self.client.call('snapshot')['result']['control']['setup:'+setup['setup_id']]['state']!='AVAILABLE':
                self.assertLess(time.monotonic(),deadline);time.sleep(.02)
            removed=self.client.call('retire_setup',dict(setup_id=setup['setup_id'],config_rev=1,expected_rev=rev+1));self.assertTrue(removed['ok'],removed)
            snapshot=self.client.call('snapshot')['result'];self.assertEqual(len(snapshot['registry']['setups']),0);self.assertEqual(len(snapshot['registry']['devices']),2)
            self.assertFalse(removed['result']['physical_zero_verified'])
            self.assertTrue(removed['result']['restart_required'])
    def test_host_status_remains_visible_after_gui_close(self):
        before=self.client.call('ping')['result']
        report=self.client.call('close_client')
        self.assertTrue(report['ok'],report);self.assertTrue(report['result']['released'])
        self.client.close();other=f.Pipe(self.endpoint);self.clients.append(other)
        after=other.call('ping')['result'];self.assertEqual(after['boot_id'],before['boot_id'])
        self.assertTrue(after['tray']['visible']);self.assertTrue(after['tray']['has_reopen_management'])

    def test_two_pipe_clients_multi_instance_contract(self):
        devices=[]
        for index in range(6):
            snap=self.client.call('snapshot')['result'];rev=snap['registry']['registry_rev']
            d=self.client.call('create_draft',dict(model_id='aq6370',profile_id='gpib-visa',params={'resource':f'GPIB0::{index+4}::INSTR'},name=f'OSA {index+1}',expected_rev=rev))['result']
            domain={'kind':'device','id':d['device_id']};lease=self.client.call('acquire_control',dict(domain=domain))['result']
            consent=dict(accepted=True,mode='real',config_digest=d['config_digest'],open_effects=[],supervised=False,retain_session=False)
            proof=self.client.call('test_connection',dict(draft_id=d['device_id'],expected_rev=rev+1,consent=consent,lease_token=lease['token'],control_epoch=lease['control_epoch'],request_id=f'probe-{index}',sequence=index+1))['result']
            self.assertTrue(self.client.call('save_device',dict(draft_id=d['device_id'],proof_id=proof['proof_id'],expected_rev=rev+1))['ok']);devices.append((domain,lease))
        other=f.Pipe(self.endpoint);self.clients.append(other);self.assertTrue(other.call('ping')['ok'])
        a,lease=devices[0];b,_=devices[1]
        self.assertFalse(other.call('acquire_control',dict(domain=a))['ok'])
        before=other.call('snapshot')['result']['domains']['device:'+b['id']]
        ctx=self.client.call('snapshot')['result']['domains']['device:'+a['id']]['context']
        intent=dict(domain=a,lease_token=lease['token'],control_epoch=lease['control_epoch'],config_rev=1,context=ctx,method='connect',params={'acknowledge_lifecycle':True},sequence=10,confirmation=None)
        intent['confirmation']=self.client.call('prepare',dict(intent=intent))['result']['token']
        receipt=self.client.call('execute',dict(request_id='one-open',intent=intent))['result']
        deadline=time.monotonic()+5
        while self.client.call('operation',dict(request_id='one-open'))['result']['status']=='Accepted':
            self.assertLess(time.monotonic(),deadline);time.sleep(.02)
        while True:
            seen=other.call('snapshot')['result'].get('operations',[])
            matching=[op for op in seen if op['operation_id']==receipt['operation_id'] and op['status']=='Terminal']
            if matching:break
            self.assertLess(time.monotonic(),deadline);time.sleep(.02)
        self.assertEqual(matching[0]['operation_id'],receipt['operation_id'])
        after=other.call('snapshot')['result']['domains']['device:'+b['id']]
        self.assertEqual(after['context'],before['context']);self.assertIsNone(after['context']['connection_id'])
        closed=self.client.call('close_client');self.assertTrue(closed['result']['released'],closed)
        self.assertTrue(other.call('ping')['ok']);self.assertEqual(len(other.call('snapshot')['result']['registry']['devices']),6)

class VerificationModeAcceptanceTests(f.LocalHostProcessTests):
    def seed_registry(self,directory):
        from App.tests.test_local_leases import LocalLeaseTests
        import json
        from pathlib import Path
        LocalLeaseTests.seed_registry(self,directory)
        path=Path(directory)/'devices.json';data=json.loads(path.read_text());data['devices'][0]['verified_mode']='simulate';data['devices'][1].pop('verified_mode');path.write_text(json.dumps(data))
    def test_foreign_or_unstamped_evidence_is_metadata_only(self):
        snapshot=self.client.call('snapshot')['result'];self.assertEqual(snapshot['domains'],{})
        self.assertEqual(snapshot['registry']['devices'], [])
        self.assertEqual(len(snapshot['registry']['drafts']), 2)
        for d in snapshot['registry']['drafts']:
            self.assertIsNone(d.get('identity'))
            self.assertIsNone(d.get('proof_id'))
            denied=self.client.call('acquire_control',dict(domain=dict(kind='device',id=d['device_id'])))
            self.assertFalse(denied['ok'], denied)
            removed=self.client.call('cancel_draft',dict(draft_id=d['device_id'],expected_rev=self.client.call('snapshot')['result']['registry']['registry_rev']))
            self.assertTrue(removed['ok'],removed)
