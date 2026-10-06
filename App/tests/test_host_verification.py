"""Native Host registration with explicitly injected driver transports."""
import time
from App.tests import test_local_host_process as fixtures

class HostVerificationTests(fixtures.LocalHostProcessTests):
    def test_authorized_idle_readonly_checks_do_not_connect_or_grant_control(self):
        draft=self.client.call('create_draft',dict(model_id='aq6370',profile_id='gpib-visa',params={'resource':'GPIB0::4::INSTR'},name='OSA',expected_rev=0))['result']
        domain=dict(kind='device',id=draft['device_id']);lease=self.client.call('acquire_control',dict(domain=domain))['result']
        consent=dict(accepted=True,mode='real',config_digest=draft['config_digest'],open_effects=[],supervised=False,retain_session=False)
        proof=self.client.call('test_connection',dict(draft_id=domain['id'],expected_rev=1,consent=consent,lease_token=lease['token'],control_epoch=lease['control_epoch'],request_id='verify',sequence=1))['result']
        self.assertTrue(self.client.call('save_device',dict(draft_id=domain['id'],proof_id=proof['proof_id'],expected_rev=1))['ok'])
        self.assertTrue(self.client.call('safe_stop',dict(domain=domain))['ok'])
        deadline=time.monotonic()+5
        while self.client.call('snapshot')['result']['control']['device:'+domain['id']]['state']!='AVAILABLE':
            self.assertLess(time.monotonic(),deadline);time.sleep(.05)
        policy=self.client.call('save_check_policy',dict(device_id=domain['id'],config_rev=1,expected_rev=2,interval_s=10,enumeration=False,readonly=True))
        self.assertTrue(policy['ok'],policy)
        while True:
            state=self.client.call('snapshot')['result']['domains']['device:'+domain['id']]
            if state.get('availability',{}).get('communication')=='ONLINE':break
            self.assertLess(time.monotonic(),deadline,state);time.sleep(.05)
        self.assertIsNone(state['context']['connection_id'])
        self.assertEqual(self.client.call('snapshot')['result']['control']['device:'+domain['id']]['state'],'AVAILABLE')
    def test_settings_and_cancel_remove_software_authority_without_outputs(self):
        settings=self.client.call('save_settings',dict(expected_rev=0,settings=dict(python_path=str(fixtures.sys.executable),host_name='Test Host')))
        self.assertTrue(settings['ok'],settings)
        draft=self.client.call('create_draft',dict(model_id='aq6370',profile_id='gpib-visa',params={'resource':'GPIB0::4::INSTR'},name='Draft',expected_rev=1))['result']
        domain=dict(kind='device',id=draft['device_id'])
        self.assertTrue(self.client.call('acquire_control',dict(domain=domain))['ok'])
        cancelled=self.client.call('cancel_draft',dict(draft_id=draft['device_id'],expected_rev=2))
        self.assertTrue(cancelled['ok'],cancelled)
        self.assertFalse(self.client.call('acquire_control',dict(domain=domain))['ok'])
    def test_draft_probe_save_and_second_identical_model(self):
        for index in (4,5):
            snap=self.client.call('snapshot')['result']['registry']
            draft=self.client.call('create_draft',dict(model_id='aq6370',profile_id='gpib-visa',
                params={'resource':f'GPIB0::{index}::INSTR'},name=f'OSA {index}',expected_rev=snap['registry_rev']))
            self.assertTrue(draft['ok'],draft);draft=draft['result']
            domain={'kind':'device','id':draft['device_id']}
            lease=self.client.call('acquire_control',{'domain':domain})
            self.assertTrue(lease['ok'],lease)
            consent=dict(accepted=True,mode='real',config_digest=draft['config_digest'],open_effects=[],supervised=False,retain_session=False)
            op=self.client.call('test_connection',dict(draft_id=draft['device_id'],expected_rev=self.client.call('snapshot')['result']['registry']['registry_rev'],
                consent=consent,lease_token=lease['result']['token'],control_epoch=lease['result']['control_epoch'],request_id=f'probe-{index}',sequence=index))
            self.assertTrue(op['ok'],op)
            saved=self.client.call('save_device',dict(draft_id=draft['device_id'],proof_id=op['result']['proof_id'],
                expected_rev=self.client.call('snapshot')['result']['registry']['registry_rev']))
            self.assertTrue(saved['ok'],saved)
            self.assertEqual(saved['result']['device_id'],draft['device_id'])
            self.assertFalse(self.client.call('save_device',dict(draft_id=draft['device_id'],proof_id=op['result']['proof_id'],expected_rev=999))['ok'])
        self.assertEqual(len(self.client.call('snapshot')['result']['registry']['devices']),2)

    def test_source_is_not_implicitly_connected_by_testing(self):
        draft=self.client.call('create_draft',dict(model_id='voltage',profile_id='ch340-serial',params={'port':'COM7'},name='Source',expected_rev=0))
        self.assertTrue(draft['ok'],draft);draft=draft['result'];domain=dict(kind='device',id=draft['device_id'])
        lease=self.client.call('acquire_control',dict(domain=domain))['result']
        consent=dict(accepted=True,mode='real',config_digest=draft['config_digest'],open_effects=['DTR_RTS_reset_not_verified'],supervised=True,retain_session=True)
        failed=self.client.call('test_connection',dict(draft_id=draft['device_id'],expected_rev=1,consent=consent,
            lease_token=lease['token'],control_epoch=lease['control_epoch'],request_id='source-probe',sequence=1))
        self.assertFalse(failed['ok'])
        state=self.client.call('worker_status')['result']['domains']['device:'+draft['device_id']]
        self.assertIsNone(state['context']['connection_id'])

    def test_supervised_source_requires_separate_confirmed_connect_then_retains_owner(self):
        draft=self.client.call('create_draft',dict(model_id='voltage',profile_id='ch340-serial',params={'port':'COM7'},name='Source',expected_rev=0))['result']
        domain=dict(kind='device',id=draft['device_id']);lease=self.client.call('acquire_control',dict(domain=domain))['result']
        context=self.client.call('worker_status')['result']['domains']['device:'+domain['id']]['context']
        intent=dict(domain=domain,lease_token=lease['token'],control_epoch=lease['control_epoch'],config_rev=1,context=context,
            method='connect',params={'acknowledge_lifecycle':True},sequence=1,confirmation=None)
        proof=self.client.call('prepare',dict(intent=intent))
        self.assertTrue(proof['ok'],proof)
        intent['confirmation']=proof['result']['token']
        receipt=self.client.call('execute',dict(request_id='prepare-source',intent=intent));self.assertTrue(receipt['ok'],receipt)
        deadline=time.monotonic()+3
        while self.client.call('operation',dict(request_id='prepare-source'))['result']['status']=='Accepted':
            self.assertLess(time.monotonic(),deadline);time.sleep(.02)
        opened=self.client.call('snapshot')['result']['domains']['device:'+domain['id']]['context']
        self.assertIsNotNone(opened['connection_id'])
        consent=dict(accepted=True,mode='real',config_digest=draft['config_digest'],open_effects=['DTR_RTS_reset_not_verified'],supervised=True,retain_session=True)
        probe=self.client.call('test_connection',dict(draft_id=draft['device_id'],expected_rev=1,consent=consent,
            lease_token=lease['token'],control_epoch=lease['control_epoch'],request_id='probe-source',sequence=2))
        self.assertTrue(probe['ok'],probe)
        self.assertEqual(self.client.call('snapshot')['result']['domains']['device:'+domain['id']]['context'],opened)
        saved=self.client.call('save_device',dict(draft_id=draft['device_id'],proof_id=probe['result']['proof_id'],expected_rev=1))
        self.assertTrue(saved['ok'],saved)
        self.assertEqual(saved['result']['identity_strength'],'operator_bound')
        closed=self.client.call('close_client')
        self.assertTrue(closed['result']['released'],closed)
