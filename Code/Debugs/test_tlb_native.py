"""Native driver adapter boundaries, using finite injected process/reply doubles."""
import importlib.util
import unittest
from unittest.mock import patch, PropertyMock
from Code.Utils.common import InstrumentConnectionError, InstrumentSafetyError, InstrumentProtocolError, DriverState

class Native:
    def __init__(self):self.opened=False;self.actions=[]
    @property
    def is_open(self):return self.opened
    def connect(self,key):
        self.opened=True
        return {'identity':{'manufacturer':'New Focus','model':'TLB-6700','serial':'1012','firmware':'2.4','head_model':'6722-P','head_serial':'P1001'},'wavelength_range_nm':[1045.,1085.]}
    def status(self):
        return dict(wavelength_nm=1060.01,wavelength_setpoint_nm=1060.,power_mw=0.,power_setpoint_mw=10.,current_ma=0.,current_setpoint_ma=20.,piezo_percent=50.,output_enabled=False,tracking=True,remote=False,constant_power=False,operation_complete=True,status_byte=0,read_interval_s=.01)
    def action(self,name,value,confirm):
        self.actions.append((name,value,confirm))
        raise InstrumentSafetyError('head is read-only')
    def close(self):self.opened=False

class NativeAdapterTests(unittest.TestCase):
    def test_invalid_motion_faults_without_inventing_full_status(self):
        from Code.Utils.tlb6700 import TLB6700
        native=Native()
        valid=dict(wavelength_nm=1060.,wavelength_setpoint_nm=1061.,tracking=False,operation_complete=False,read_interval_s=.01)
        for sample in [dict(valid,power_mw=1),dict(valid,tracking=1),dict(valid,wavelength_nm=float('nan'))]:
            native.motion=lambda:sample
            with patch('Code.Utils.tlb6700.NativeLaser',return_value=native):
                d=TLB6700(device_key='6700 SN1012');d.connect()
                with self.assertRaises(InstrumentProtocolError):d.read_motion()
                self.assertIs(d.state,DriverState.FAULT);d.close()
    def test_production_driver_uses_native_semantics_without_ctypes(self):
        from Code.Utils.tlb6700 import TLB6700
        self.assertTrue(hasattr(__import__('Code.Utils.tlb6700',fromlist=['NativeLaser']),'NativeLaser'),'Production native adapter missing')
        native=Native()
        with patch('Code.Utils.tlb6700.NativeLaser',return_value=native),patch('Code.Utils.newport_usb._NativeUSB',side_effect=AssertionError('ctypes must not open hardware')):
            d=TLB6700(device_key='6700 SN1012')
            d.connect();self.assertEqual(d.identity['head_serial'],'P1001');self.assertEqual(d.read_status().wavelength_nm,1060.01)
            with self.assertRaises(InstrumentSafetyError):d.set_output(True,confirm=True)
            self.assertEqual(native.actions,[('output',True,True)])
            self.assertIs(d.state,DriverState.READY);d.close();self.assertTrue(d.resources_released)
    def test_lost_connect_keeps_responsibility_until_explicit_close(self):
        from Code.Utils.tlb6700 import TLB6700
        self.assertTrue(hasattr(__import__('Code.Utils.tlb6700',fromlist=['NativeLaser']),'NativeLaser'),'Production native adapter missing')
        native=Native()
        def lost(key):native.opened=True;raise InstrumentConnectionError('outcome unknown')
        native.connect=lost
        def failed_close():raise InstrumentConnectionError('release unknown')
        native.close=failed_close
        with patch('Code.Utils.tlb6700.NativeLaser',return_value=native):
            d=TLB6700(device_key='6700 SN1012')
            with self.assertRaises(InstrumentConnectionError):d.connect()
            self.assertTrue(d.has_resource_responsibility)
            native.close=lambda:setattr(native,'opened',False)
            d.close();self.assertTrue(d.resources_released)
    def test_invalid_status_payload_faults_the_driver(self):
        from Code.Utils.tlb6700 import TLB6700
        native=Native();native.status=lambda:{'wavelength_nm':1060}
        with patch('Code.Utils.tlb6700.NativeLaser',return_value=native):
            d=TLB6700(device_key='6700 SN1012');d.connect()
            with self.assertRaises(InstrumentProtocolError):d.read_status()
            self.assertIs(d.state,DriverState.FAULT);d.close()
    def test_global_sdk_release_includes_native_pending_owner(self):
        self.assertIsNotNone(importlib.util.find_spec('Code.Utils.tlb_native_bridge'),'Native bridge missing')
        from Code.Utils import newport_usb,tlb_native_bridge
        with patch.object(type(tlb_native_bridge.NATIVE),'resources_released',new_callable=PropertyMock,return_value=False):
            self.assertFalse(newport_usb.resources_released())

import io, json, queue
class Output:
    def __init__(self):self.q=queue.Queue()
    def readline(self,limit):return self.q.get()
    def close(self):self.q.put(b'')
class Input:
    def __init__(self,process):self.p=process
    def write(self,line):
        request=json.loads(line);self.p.requests.append(request);op=request['operation']['method'];released=op in {'hello','disconnect','shutdown','resources','enumerate','discover'}
        result={'backend':'rust','protocol':1,'pid':100} if op=='hello' else {'identity':Native().connect('x')['identity'],'wavelength_range_nm':[1045.,1085.]} if op=='connect' else {'controller_keys':['6700 SN1012']} if op=='enumerate' else {'release_confirmed':True}
        result=self.p.bad_results.get(op,result)
        reply=json.dumps(dict(v=1,id=request['id'],ok=True,result=result,resources_released=released)).encode()+b'\n'
        if self.p.drop==op:self.p.late=reply
        else:self.p.stdout.q.put(reply)
        if op=='shutdown':self.p.exited=True
        return len(line)
    def flush(self):pass
    def close(self):self.p.stdout.q.put(b'');self.p.exited=True
class Process:
    def __init__(self,drop=None):self.bad_results={};self.drop=drop;self.stdout=Output();self.stdin=Input(self);self.stderr=io.BytesIO();self.requests=[];self.exited=False;self.late=None
    def poll(self):return 0 if self.exited else None
    def wait(self,timeout=None):return 0 if self.exited else (_ for _ in ()).throw(TimeoutError('still owns process'))
class ManagerTests(unittest.TestCase):
    def test_head_discovery_validates_and_releases_one_temporary_owner(self):
        p=Process();p.bad_results['discover']={'identities':[Native().connect('x')['identity']]};m=self.manager(p)
        heads=m.discover();self.assertEqual(heads[0]['head_model'],'6722-P');self.assertTrue(m.resources_released)
        self.assertEqual([r['operation']['method'] for r in p.requests],['hello','discover','shutdown'])
    def test_discovery_reply_capacity_handles_32_bounded_heads(self):
        p=Process();heads=[dict(Native().connect('x')['identity'],serial=str(i),firmware='f'*64,head_model='6722-'+'X'*59,head_serial='S'*64) for i in range(32)]
        p.bad_results['discover']={'identities':heads};m=self.manager(p)
        self.assertGreater(len(json.dumps(p.bad_results['discover'])),4096)
        self.assertEqual(len(m.discover()),32);self.assertTrue(m.resources_released)
    def test_discovery_timeout_settles_original_request_without_replay(self):
        p=Process(drop='discover');p.bad_results['discover']={'identities':[Native().connect('x')['identity']]};m=self.manager(p)
        with self.assertRaises(InstrumentConnectionError):m.discover()
        self.assertFalse(m.resources_released);p.stdout.q.put(p.late);m.cleanup_unowned()
        self.assertTrue(m.resources_released);self.assertEqual(sum(r['operation']['method']=='discover' for r in p.requests),1)
    def test_malformed_or_duplicate_head_acknowledgement_retains_owner(self):
        head=Native().connect('x')['identity']
        for identities in [[head,head],[dict(head,head_model='')],[dict(head,serial='USB0')]]:
            p=Process();p.bad_results['discover']={'identities':identities};m=self.manager(p)
            with self.assertRaises(InstrumentConnectionError):m.discover()
            self.assertFalse(m.resources_released);p.stdout.close()
    def manager(self,process):
        self.assertIsNotNone(importlib.util.find_spec('Code.Utils.tlb_native_bridge'),'Native bridge missing')
        from Code.Utils.tlb_native_bridge import NativeManager
        return NativeManager(_launch=lambda path:process,_path=lambda:'finite-only.exe',_timeout=.01)
    def test_duplicate_claim_and_last_release_close_process(self):
        p=Process();m=self.manager(p);a,b=object(),object();m.connect('6700 SN1012',a)
        with self.assertRaises(InstrumentConnectionError):m.connect('6700 SN1012',b)
        self.assertFalse(m.resources_released);m.disconnect('6700 SN1012',a);self.assertTrue(m.resources_released)
        self.assertEqual([r['operation']['method'] for r in p.requests],['hello','connect','disconnect','shutdown'])
    def test_lost_connect_never_replayed_and_close_settles_original_reply(self):
        p=Process(drop='connect');m=self.manager(p);owner=object()
        with self.assertRaises(InstrumentConnectionError):m.connect('6700 SN1012',owner)
        self.assertFalse(m.resources_released);self.assertTrue(m.owns('6700 SN1012',owner))
        with self.assertRaises(InstrumentConnectionError):m.call({'method':'enumerate'})
        p.stdout.q.put(p.late);m.disconnect('6700 SN1012',owner)
        self.assertTrue(m.resources_released);self.assertEqual(sum(r['operation']['method']=='connect' for r in p.requests),1)
    def test_lost_shutdown_retains_claim_and_retry_waits_original_exit(self):
        p=Process(drop='shutdown');m=self.manager(p);owner=object();m.connect('6700 SN1012',owner)
        with self.assertRaises(InstrumentConnectionError):m.disconnect('6700 SN1012',owner)
        self.assertTrue(m.owns('6700 SN1012',owner));self.assertFalse(m.resources_released)
        p.stdout.q.put(p.late);m.disconnect('6700 SN1012',owner)
        self.assertTrue(m.resources_released)
        self.assertEqual(sum(r['operation']['method']=='shutdown' for r in p.requests),1)
    def test_failed_metadata_handshake_reaps_without_hardware_request(self):
        p=Process(drop='hello');m=self.manager(p);owner=object()
        with self.assertRaises(InstrumentConnectionError):m.connect('6700 SN1012',owner)
        m.disconnect('6700 SN1012',owner)
        self.assertTrue(m.resources_released);self.assertEqual(len(p.requests),1)
    def test_late_disconnect_finishes_without_second_disconnect(self):
        p=Process(drop='disconnect');m=self.manager(p);owner=object();m.connect('6700 SN1012',owner)
        with self.assertRaises(InstrumentConnectionError):m.disconnect('6700 SN1012',owner)
        p.stdout.q.put(p.late);m.disconnect('6700 SN1012',owner)
        self.assertTrue(m.resources_released)
        self.assertEqual(sum(r['operation']['method']=='disconnect' for r in p.requests),1)
    def test_pending_scan_can_be_settled_and_closed_without_a_key(self):
        p=Process(drop='enumerate');m=self.manager(p)
        with self.assertRaises(InstrumentConnectionError):m.enumerate()
        self.assertFalse(m.resources_released);p.stdout.q.put(p.late)
        m.cleanup_unowned();self.assertTrue(m.resources_released)
        self.assertEqual(sum(r['operation']['method']=='enumerate' for r in p.requests),1)
    def test_action_false_and_malformed_status_never_count_as_success(self):
        for method, result in [('action',{'completed':False}),('control',{'completed':False}),('control',{'completed':1}),('limits',{'completed':True,'raw':'*RST'}),('status',{'wavelength_nm':1060})]:
            with self.subTest(method=method):
                p=Process();m=self.manager(p);owner=object();m.connect('6700 SN1012',owner);p.bad_results[method]=result
                with self.assertRaises(InstrumentConnectionError):m.operation('6700 SN1012',owner,method)
                self.assertFalse(m.resources_released);self.assertTrue(m.owns('6700 SN1012',owner))
                p.stdout.close()
    def test_refresh_after_late_scan_releases_old_owner_and_gets_fresh_keys(self):
        from Code.Utils.tlb_native_bridge import NativeManager
        old,new=Process(drop='enumerate'),Process()
        new.bad_results['enumerate']={'controller_keys':['6700 SN1013']}
        processes=iter([old,new])
        m=NativeManager(_launch=lambda path:next(processes),_path=lambda:'finite.exe',_timeout=.01)
        with self.assertRaises(InstrumentConnectionError):m.enumerate()
        old.stdout.q.put(old.late)
        self.assertEqual(m.enumerate(),('6700 SN1013',));self.assertTrue(m.resources_released)
        self.assertEqual(sum(r['operation']['method']=='enumerate' for r in old.requests),1)
        self.assertEqual(sum(r['operation']['method']=='enumerate' for r in new.requests),1)
    def test_broken_reply_does_not_claim_release_or_replay(self):
        p=Process(drop='connect');m=self.manager(p);owner=object()
        with self.assertRaises(InstrumentConnectionError):m.connect('6700 SN1012',owner)
        p.stdout.q.put(b'{"id":999,"ok":true}\n')
        with self.assertRaises(InstrumentConnectionError):m.disconnect('6700 SN1012',owner)
        self.assertFalse(m.resources_released);self.assertEqual(sum(r['operation']['method']=='connect' for r in p.requests),1)
        p.stdout.close()
class WorkerNativeCleanupTests(unittest.TestCase):
    def test_global_native_scan_release_is_part_of_shutdown_evidence(self):
        import uuid
        from App.worker.controller import DomainController
        class Pending:
            needs_cleanup=True
            def cleanup_unowned(self):raise InstrumentConnectionError('pending native scan')
        with patch('App.worker.controller.NATIVE',Pending(),create=True):
            controller=DomainController(session_id=uuid.uuid4().hex)
            report=controller.close()
            self.assertIn('newport:native',report['unreleased'])
            self.assertFalse(controller._closed)
            self.assertEqual(controller.cached_status()['last_cleanup'],report)
            self.assertEqual(controller._cleanup_attempts[-1],report)
if __name__=='__main__':unittest.main()
