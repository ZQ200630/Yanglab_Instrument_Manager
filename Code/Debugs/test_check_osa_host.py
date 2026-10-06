"""Finite saved frames/bytes only, not GUI, network or physical acceptance."""
import copy
import hashlib
import json
from pathlib import Path
import struct
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO

import numpy as np
from Code.Debugs import check_osa_host as d

HOST='a'*32
DOMAIN={'kind':'device','id':'b'*32}
IDENTITY={'manufacturer':'YOKOGAWA','model':'AQ6370E','serial':'901C12907','firmware':'01.04'}

def saved_bytes(count=2, unit='W'):
    raw=b''.join(struct.pack('<dd',1500+i/1000, i*1e-9 if unit=='W' else -210+i/1000) for i in range(count))
    context=dict(transfer_format='ASCII',sample_count=count,spacing=int(unit=='W'),level_unit=int(unit=='W'),x_unit=0,trace_attribute=0,active_trace='TRA',center_m=1.5e-6,span_m=2e-9,resolution_m=2e-11,sweep_mode=1)
    metadata=dict(native_unit=unit,trace='A',identity='YOKOGAWA,AQ6370E,901C12907,01.04',read_started_at='2026-10-06T10:00:00+00:00',read_finished_at='2026-10-06T10:00:01+00:00',elapsed_s=1,consistency='unproven',context_before=context,context_after=context)
    ref=dict(id='c'*32,name='osa',host_id=HOST,domain=DOMAIN,byte_count=len(raw),sample_count=count,sha256=hashlib.sha256(raw).hexdigest(),metadata=metadata)
    manifest=dict(schema=1,source_kind='real',name='osa',archived_at_unix_ms=1,origin=dict(host_id=HOST,domain=DOMAIN,device_identity=IDENTITY,config_rev=1,operation_id=ref['id']),descriptor=dict(schema=1,kind='osa_trace',capture_id='d'*32,point_count=count,byte_count=len(raw),sha256=ref['sha256'],metadata=metadata))
    return ref,json.dumps(manifest,separators=(',',':')).encode(),raw

class Frames:
    def __init__(self,reply):
        body=json.dumps(reply).encode();self.raw=bytearray(struct.pack('<I',len(body))+body);self.writes=[];self.closed=False
    def read(self,n):
        result=bytes(self.raw[:n]);del self.raw[:n];return result
    def write(self,data):self.writes.append(data)
    def close(self):self.closed=True

class DataOnly:
    def __init__(self,ref,manifest,raw):self.ref,self.manifest,self.raw=ref,manifest,raw;self.calls=[];self.bad_offset=False
    def call(self,method,params):
        self.calls.append((method,params))
        if method=='archive_manifest_bytes':return dict(id=self.ref['id'],name='osa',byte_count=len(self.manifest),sha256=hashlib.sha256(self.manifest).hexdigest(),data_hex=self.manifest.hex())
        if method!='read_archive':raise AssertionError('No hardware/control method permitted')
        return dict(id=self.ref['id'],name='osa',offset=params['offset']+int(self.bad_offset),length=params['length'],sha256=self.ref['sha256'],data_hex=self.raw[params['offset']:params['offset']+params['length']].hex())

class OSAHostDiagnosticTests(unittest.TestCase):
    def test_cli_without_explicit_known_read_acknowledgement_never_starts(self):
        with redirect_stderr(StringIO()),self.assertRaises(SystemExit) as failure:
            d.main([])
        self.assertEqual(failure.exception.code,2)
        with redirect_stdout(StringIO()),self.assertRaises(SystemExit) as help_exit:
            d.main(['--help'])
        self.assertEqual(help_exit.exception.code,0)
    def test_execute_existing_trace_polls_original_operation_once_and_never_sweeps(self):
        class OperationWire:
            def __init__(self): self.requests=[];self.polls=0
            def call(self,method,params=None):
                self.requests.append((method,params))
                if method=='snapshot':return dict(boot_id='1'*32,domains={'device:'+DOMAIN['id']:{'context':dict(session_id='e'*32,domain=DOMAIN,connection_id='f'*32,epoch=1)}})
                if method=='prepare':return {'token':'approved'}
                if method=='execute':return {'operation_id':'c'*32,'status':'Accepted'}
                if method=='operation':
                    self.polls+=1
                    return dict(operation_id='c'*32,status='Accepted' if self.polls==1 else 'Completed',phase='admitted' if self.polls==1 else 'completed',result={'result':{'native_unit':'W'}})
                raise AssertionError(method)
        client=OperationWire();lease=dict(token='2'*32,boot_id='1'*32,control_epoch=3)
        operation=d.execute_read(client,lease,DOMAIN,1,'read_trace',4,'A',timeout=1)
        self.assertEqual(operation['result'],{'result':{'native_unit':'W'}})
        execution=[p for m,p in client.requests if m=='execute'];self.assertEqual(len(execution),1)
        self.assertEqual(execution[0]['intent']['params'],{'name':'read_trace','args':{'trace':'A','archive_name':'osa'}})
        self.assertEqual([p['request_id'] for m,p in client.requests if m=='operation'],[execution[0]['request_id']]*2)

    def test_failed_read_or_deadline_never_reexecutes(self):
        class FailureWire:
            def __init__(self,phase):self.phase=phase;self.executions=0
            def call(self,method,params=None):
                if method=='snapshot':return dict(boot_id='1'*32,domains={'device:'+DOMAIN['id']:{'context':dict(session_id='e'*32,domain=DOMAIN,connection_id='f'*32,epoch=1)}})
                if method=='prepare':return {'token':'approved'}
                if method=='execute':self.executions+=1;return {'operation_id':'c'*32,'status':'Accepted'}
                if method=='operation':return dict(operation_id='c'*32,status='Accepted' if self.phase=='waiting' else 'Completed',phase=self.phase,result={})
                raise AssertionError(method)
        lease=dict(token='2'*32,boot_id='1'*32,control_epoch=3)
        for phase,error in [('completed_readback_failed',RuntimeError),('OutcomeUnknown',RuntimeError),('waiting',TimeoutError)]:
            client=FailureWire(phase)
            with self.assertRaises(error):d.execute_read(client,lease,DOMAIN,1,'read_trace',4,'A',timeout=.05)
            self.assertEqual(client.executions,1)

    def test_known_osa_identity_gate_blocks_other_models_and_serials(self):
        record=dict(model_id='aq6370',profile_id='gpib-visa',params={'resource':'GPIB0::4::INSTR','backend':'system','timeout_s':30},expected_identity=IDENTITY,verified_mode='real')
        d.require_known_osa(record)
        for identity in [{**IDENTITY,'serial':'unknown'},{**IDENTITY,'model':'AQ6370D'},{**IDENTITY,'manufacturer':'other'}]:
            with self.assertRaises(ValueError):d.require_known_osa({**record,'expected_identity':identity})
        with self.assertRaises(ValueError):d.require_known_osa({**record,'params':{'resource':'GPIB0::5::INSTR'}})
        with self.assertRaises(ValueError):d.require_known_osa({**record,'params':{**record['params'],'backend':'other'}})

    def test_stop_requires_release_and_separate_successful_worker_exit(self):
        d.validate_stop(dict(resource_released=True,process_exit=dict(confirmed=True,success=True)))
        for report in [dict(resource_released=False,process_exit=dict(confirmed=True,success=True)),dict(resource_released=True,process_exit=None),dict(resource_released=True,process_exit=dict(confirmed=True,success=False)),dict(resource_released=True,process_exit=dict(confirmed=1,success=True))]:
            with self.assertRaises(RuntimeError):d.validate_stop(report)

    def test_host_link_binds_small_reply_and_releases_transport(self):
        wire=Frames(dict(v=1,id='debug-1',ok=True,result={'source':'literal'}))
        with d.HostLink(wire) as client:self.assertEqual(client.call('ping'),{'source':'literal'})
        self.assertTrue(wire.closed)
        request=b''.join(wire.writes);length,=struct.unpack('<I',request[:4]);self.assertEqual(length,len(request)-4)
        self.assertEqual(json.loads(request[4:]),dict(v=1,id='debug-1',method='ping',params={}))

    def test_host_link_rejects_bad_frames_without_resending(self):
        for reply in [dict(v=1,id='other',ok=True,result={}),dict(v=2,id='debug-1',ok=True,result={}),dict(v=1,id='debug-1',ok=1,result={}),dict(v=1,id='debug-1',ok=False,error={'message':'Denied'})]:
            wire=Frames(reply)
            with d.HostLink(wire) as client:
                with self.assertRaises((ValueError,RuntimeError)):client.call('ping')
            self.assertEqual(len(wire.writes),1)
        wire=Frames({});wire.raw=bytearray(struct.pack('<I',65537))
        with d.HostLink(wire) as client:
            with self.assertRaises(ValueError):client.call('ping')

    def test_oversized_request_never_reaches_transport(self):
        wire=Frames({})
        with d.HostLink(wire) as client:
            with self.assertRaises(ValueError):client.call('ping',{'data':'x'*65536})
        self.assertEqual(wire.writes,[])

    def test_saved_native_w_is_validated_without_conversion(self):
        ref,manifest,raw=saved_bytes();capture=d.decode_archive(ref,manifest,raw,HOST,DOMAIN)
        self.assertEqual(capture.native_unit,'W');self.assertEqual(capture.native_values.tolist(),[0,1e-9]);self.assertIsNone(capture.power_dbm)
        self.assertEqual(capture.consistency,'unproven');self.assertFalse(capture.native_values.flags.writeable)

    def test_wrong_scope_hash_partial_or_unit_has_no_complete_capture(self):
        ref,manifest,raw=saved_bytes()
        with self.assertRaises(ValueError):d.decode_archive(ref,manifest,raw,'e'*32,DOMAIN)
        for value in [raw[:-1],raw[:-1]+bytes([raw[-1]^1])]:
            with self.assertRaises(ValueError):d.decode_archive(ref,manifest,value,HOST,DOMAIN)
        wrong=copy.deepcopy(ref);wrong['metadata']['native_unit']='V'
        with self.assertRaises((ValueError,RuntimeError)):d.decode_archive(wrong,manifest,raw,HOST,DOMAIN)

    def test_data_only_fetch_is_bounded_and_scoped(self):
        ref,manifest,raw=saved_bytes(2000,'dBm');client=DataOnly(ref,manifest,raw)
        actual_manifest,actual_raw,capture=d.fetch_archive(client,ref,HOST,DOMAIN)
        self.assertEqual(actual_manifest,manifest);self.assertEqual(actual_raw,raw);self.assertEqual(len(capture.native_values),2000)
        self.assertEqual([m for m,p in client.calls],['archive_manifest_bytes','read_archive','read_archive'])
        for method,p in client.calls:
            self.assertEqual(p['domain'],DOMAIN);self.assertNotIn('lease_token',p)
            if method=='read_archive':self.assertLessEqual(p['length'],16384)
        client.bad_offset=True
        with self.assertRaises(ValueError):d.fetch_archive(client,ref,HOST,DOMAIN)

    def test_read_only_intent_retains_current_authority_and_refuses_output_actions(self):
        context=dict(session_id='e'*32,domain=DOMAIN,connection_id='f'*32,epoch=1)
        snapshot=dict(boot_id='1'*32,domains={'device:'+DOMAIN['id']:{'context':context}})
        lease=dict(token='2'*32,control_epoch=3,boot_id='1'*32)
        intent=d.read_intent(snapshot,lease,DOMAIN,1,'read_trace',4,'A')
        self.assertEqual(intent,dict(domain=DOMAIN,lease_token='2'*32,control_epoch=3,config_rev=1,context=context,method='action',params={'name':'read_trace','args':{'trace':'A','archive_name':'osa'}},sequence=4,confirmation=None))
        for action in ['acquire','zero','set_channel','enable_tec']:
            with self.assertRaises(ValueError):d.read_intent(snapshot,lease,DOMAIN,1,action,4,'A')
        with self.assertRaises(ValueError):d.read_intent(snapshot,{**lease,'boot_id':'3'*32},DOMAIN,1,'read_trace',4,'A')

    def test_diagnostic_export_preserves_original_manifest_and_float64_values(self):
        ref,manifest,raw=saved_bytes()
        with tempfile.TemporaryDirectory() as root:
            target=Path(root)/'export';d.export_archive(target,ref,manifest,raw,HOST,DOMAIN)
            self.assertEqual((target/'manifest.json').read_bytes(),manifest);self.assertEqual((target/'native.bin').read_bytes(),raw)
            text=(target/'spectrum.csv').read_text();self.assertEqual(text.splitlines()[0],'wavelength_nm,power_W')
            values=np.loadtxt(target/'spectrum.csv',delimiter=',',skiprows=1);self.assertEqual(values[:,1].tolist(),[0,1e-9])
            seal=json.loads((target/'complete.json').read_text());self.assertEqual(seal['native_sha256'],ref['sha256'])
            with self.assertRaises(FileExistsError):d.export_archive(target,ref,manifest,raw,HOST,DOMAIN)

if __name__=='__main__':unittest.main()
