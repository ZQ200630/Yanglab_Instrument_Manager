"""Known OSA existing-trace/Host archive diagnostic. Never starts a sweep.

The Windows pipe is a Host control/data transport, not an instrument transport.
All hardware reads run through the packaged Code/Utils driver. This script has
no output-setting, raw SCPI, simulated backend or forced-process-exit route.
"""
from __future__ import annotations

import argparse
from collections import deque
import ctypes
from datetime import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import re
import struct
import subprocess
import sys
import threading
import time
import uuid

import numpy as np
from Code.Utils.osa_trace import MAX_TRACE_POINTS, TraceCapture, TraceContext

RESOURCE='GPIB0::4::INSTR'
SERIAL='901C12907'
MAX_FRAME=65536

def _need(condition, message):
    if not condition: raise ValueError(message)

def _fields(value, keys):
    return type(value) is dict and set(value)==set(keys)

def _id(value): return type(value) is str and re.fullmatch(r'[0-9a-f]{32}',value) is not None
def _hash(value): return type(value) is str and re.fullmatch(r'[0-9a-f]{64}',value) is not None
def _integer(value,low,high): return type(value) is int and low<=value<=high

def _json(raw):
    def pairs(items):
        result={}
        for key,value in items:
            _need(key not in result,'Duplicate JSON field');result[key]=value
        return result
    def constant(value): raise ValueError('Nonfinite JSON constant: '+value)
    return json.loads(raw,object_pairs_hook=pairs,parse_constant=constant)

class HostLink:
    def __init__(self,transport): self.transport,self.sequence=transport,0
    def __enter__(self): return self
    def __exit__(self,kind,error,tb):
        try:self.transport.close()
        except BaseException as cleanup:
            if error is None:raise
            setattr(error,'host_pipe_close_error',repr(cleanup))
    def call(self,method,params=None):
        _need(type(method) is str and type(params or {}) is dict,'Invalid Host request')
        self.sequence+=1;identifier='debug-'+str(self.sequence)
        body=json.dumps(dict(v=1,id=identifier,method=method,params=params or {}),allow_nan=False,separators=(',',':')).encode()
        _need(0<len(body)<=MAX_FRAME,'Host request exceeds frame limit')
        self.transport.write(struct.pack('<I',len(body))+body)
        header=self.transport.read(4);_need(len(header)==4,'Incomplete Host frame header')
        length,=struct.unpack('<I',header);_need(0<length<=MAX_FRAME,'Invalid Host frame length')
        data=self.transport.read(length);_need(len(data)==length,'Incomplete Host frame')
        reply=_json(data)
        _need(type(reply) is dict and type(reply.get('v')) is int and reply['v']==1 and reply.get('id')==identifier and type(reply.get('ok')) is bool,'Unbound Host reply')
        if not reply['ok']:raise RuntimeError('Host rejected request: '+json.dumps(reply.get('error'),ensure_ascii=True))
        _need('result' in reply,'Host result absent');return reply['result']

class NativePipe:
    """One reader; PeekNamedPipe bounds waits without a blocking empty read."""
    def __init__(self,endpoint,timeout=35):
        from ctypes import wintypes as w
        _need(os.name=='nt' and type(endpoint) is str and endpoint.startswith('\\\\.\\pipe\\YangLab-') and '\0' not in endpoint and len(endpoint)<=256,'Invalid local Host endpoint')
        _need(type(timeout) in (int,float) and math.isfinite(timeout) and 0<timeout<=45,'Invalid pipe deadline')
        self.api=ctypes.WinDLL('kernel32',use_last_error=True);self.timeout=timeout
        self.api.CreateFileW.argtypes=(w.LPCWSTR,w.DWORD,w.DWORD,w.LPVOID,w.DWORD,w.DWORD,w.HANDLE);self.api.CreateFileW.restype=w.HANDLE
        self.api.PeekNamedPipe.argtypes=(w.HANDLE,w.LPVOID,w.DWORD,ctypes.POINTER(w.DWORD),ctypes.POINTER(w.DWORD),ctypes.POINTER(w.DWORD))
        self.api.ReadFile.argtypes=(w.HANDLE,w.LPVOID,w.DWORD,ctypes.POINTER(w.DWORD),w.LPVOID)
        self.api.WriteFile.argtypes=self.api.ReadFile.argtypes;self.api.CloseHandle.argtypes=(w.HANDLE,)
        self.handle=self.api.CreateFileW(endpoint,0x00120183,0,None,3,0x00110000,None)
        if self.handle==w.HANDLE(-1).value:self.handle=None;raise ctypes.WinError(ctypes.get_last_error())
    def read(self,length):
        from ctypes import wintypes as w
        _need(_integer(length,1,MAX_FRAME),'Unbounded pipe read');deadline=time.monotonic()+self.timeout;result=bytearray()
        while len(result)<length:
            available=w.DWORD()
            if not self.api.PeekNamedPipe(self.handle,None,0,None,ctypes.byref(available),None):raise ctypes.WinError(ctypes.get_last_error())
            if not available.value:
                if time.monotonic()>=deadline:raise TimeoutError('Host pipe read deadline exceeded; no request replayed')
                time.sleep(.01);continue
            size=min(length-len(result),available.value);buffer=ctypes.create_string_buffer(size);count=w.DWORD()
            if not self.api.ReadFile(self.handle,buffer,size,ctypes.byref(count),None):raise ctypes.WinError(ctypes.get_last_error())
            _need(0<count.value<=size,'Invalid pipe read progress');result.extend(buffer.raw[:count.value])
        return bytes(result)
    def write(self,data):
        from ctypes import wintypes as w
        _need(type(data) is bytes and 4<len(data)<=MAX_FRAME+4,'Unbounded pipe write')
        buffer=ctypes.create_string_buffer(data);count=w.DWORD()
        if not self.api.WriteFile(self.handle,buffer,len(data),ctypes.byref(count),None):raise ctypes.WinError(ctypes.get_last_error())
        _need(count.value==len(data),'Partial Host write; no request replayed')
    def close(self):
        if self.handle is not None:
            if not self.api.CloseHandle(self.handle):raise ctypes.WinError(ctypes.get_last_error())
            self.handle=None

def _reference(ref,host,domain):
    _need(_fields(ref,['id','name','host_id','domain','byte_count','sample_count','sha256','metadata']) and _id(ref['id']) and _id(ref['host_id']) and type(ref['name']) is str and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_-]{0,39}',ref['name']) and _integer(ref['sample_count'],1,MAX_TRACE_POINTS) and type(ref['byte_count']) is int and ref['byte_count']==ref['sample_count']*16 and _hash(ref['sha256']),'Invalid archive reference')
    _need(_id(host) and _fields(domain,['kind','id']) and domain['kind']=='device' and _id(domain['id']) and ref['host_id']==host and ref['domain']==domain,'Archive Host/domain mismatch')

def decode_archive(ref,manifest_bytes,raw,host,domain):
    _reference(ref,host,domain)
    _need(type(raw) is bytes and len(raw)==ref['byte_count'] and hashlib.sha256(raw).hexdigest()==ref['sha256'],'Incomplete or hash-failed native archive')
    _need(type(manifest_bytes) is bytes and 0<len(manifest_bytes)<=16384,'Unbounded manifest')
    manifest=_json(manifest_bytes)
    _need(_fields(manifest,['schema','source_kind','name','archived_at_unix_ms','origin','descriptor']) and type(manifest['schema']) is int and manifest['schema']==1 and manifest['source_kind']=='real' and manifest['name']==ref['name'] and _integer(manifest['archived_at_unix_ms'],1,2**53-1),'Invalid real archive manifest')
    origin,desc=manifest['origin'],manifest['descriptor']
    _need(_fields(origin,['host_id','domain','device_identity','config_rev','operation_id']) and origin['host_id']==host and origin['domain']==domain and origin['operation_id']==ref['id'] and _integer(origin['config_rev'],1,2**53-1) and type(origin['device_identity']) is dict and origin['device_identity'],'Archive provenance mismatch')
    _need(_fields(desc,['schema','kind','capture_id','point_count','byte_count','sha256','metadata']) and type(desc['schema']) is int and desc['schema']==1 and desc['kind']=='osa_trace' and _id(desc['capture_id']) and type(desc['point_count']) is int and desc['point_count']==ref['sample_count'] and type(desc['byte_count']) is int and desc['byte_count']==len(raw) and desc['sha256']==ref['sha256'] and desc['metadata']==ref['metadata'],'Archive descriptor mismatch')
    m=ref['metadata'];_need(_fields(m,['native_unit','trace','identity','read_started_at','read_finished_at','elapsed_s','consistency','context_before','context_after']) and len(json.dumps(m).encode())<=8192,'Invalid capture metadata')
    values=np.frombuffer(raw,dtype='<f8').reshape((-1,2))
    try:
        capture=TraceCapture(values[:,0],values[:,1],m['native_unit'],m['trace'],m['identity'],datetime.fromisoformat(m['read_started_at']),datetime.fromisoformat(m['read_finished_at']),m['elapsed_s'],TraceContext(**m['context_before']),TraceContext(**m['context_after']),m['consistency'])
    except (TypeError,KeyError,ValueError,RuntimeError) as error:raise ValueError('Invalid native capture: '+str(error)) from error
    _need(capture.context_before.active_trace=='TR'+capture.trace,'Capture active-trace mismatch')
    return capture

def _hex(value,length):
    _need(type(value) is str and len(value)==length*2 and re.fullmatch(r'[0-9a-f]+',value),'Invalid archive byte encoding');return bytes.fromhex(value)

def fetch_archive(client,ref,host,domain):
    _reference(ref,host,domain);access=dict(domain=domain,name=ref['name'],id=ref['id'])
    reply=client.call('archive_manifest_bytes',access)
    _need(_fields(reply,['id','name','byte_count','sha256','data_hex']) and reply['id']==ref['id'] and reply['name']==ref['name'] and _integer(reply['byte_count'],1,16384) and _hash(reply['sha256']),'Unbound manifest reply')
    manifest=_hex(reply['data_hex'],reply['byte_count']);_need(hashlib.sha256(manifest).hexdigest()==reply['sha256'],'Manifest SHA256 mismatch')
    raw=bytearray()
    while len(raw)<ref['byte_count']:
        offset=len(raw);length=min(16384,ref['byte_count']-offset);chunk=client.call('read_archive',dict(access,offset=offset,length=length))
        _need(_fields(chunk,['id','name','offset','length','sha256','data_hex']) and chunk['id']==ref['id'] and chunk['name']==ref['name'] and type(chunk['offset']) is int and chunk['offset']==offset and type(chunk['length']) is int and chunk['length']==length and chunk['sha256']==ref['sha256'],'Unbound archive chunk')
        raw.extend(_hex(chunk['data_hex'],length))
    raw=bytes(raw);return manifest,raw,decode_archive(ref,manifest,raw,host,domain)

def read_intent(snapshot,lease,domain,rev,action,sequence,trace='A'):
    _need(action in ('connect','read_trace') and trace in tuple('ABCDEFG'),'Diagnostic permits connect/existing trace only')
    _need(_id(snapshot.get('boot_id')) and lease.get('boot_id')==snapshot['boot_id'] and _id(lease.get('token')) and _integer(lease.get('control_epoch'),0,2**53-1) and _integer(sequence,1,2**53-1) and _integer(rev,1,2**53-1),'Current authority absent')
    context=snapshot['domains']['device:'+domain['id']]['context'];_need(context['domain']==domain and _id(context['session_id']) and _integer(context['epoch'],0,2**53-1),'Domain context mismatch')
    _need(context['connection_id'] is None if action=='connect' else _id(context['connection_id']),'Connection context mismatch')
    return dict(domain=domain,lease_token=lease['token'],control_epoch=lease['control_epoch'],config_rev=rev,context=context,method='connect' if action=='connect' else 'action',params={'acknowledge_lifecycle':True} if action=='connect' else {'name':'read_trace','args':{'trace':trace,'archive_name':'osa'}},sequence=sequence,confirmation=None)

def execute_read(client,lease,domain,rev,action,sequence,trace='A',timeout=45):
    """Submit once, then poll only that receipt; failure never starts another read."""
    _need(type(timeout) in (int,float) and math.isfinite(timeout) and 0<timeout<=45,'Invalid operation deadline')
    intent=read_intent(client.call('snapshot'),lease,domain,rev,action,sequence,trace)
    intent['confirmation']=client.call('prepare',{'intent':intent})['token']
    request_id=uuid.uuid4().hex
    admitted=client.call('execute',dict(request_id=request_id,intent=intent))
    _need(_id(admitted.get('operation_id')),'Unbound operation admission')
    deadline=time.monotonic()+timeout
    while True:
        operation=client.call('operation',{'request_id':request_id})
        _need(operation.get('operation_id')==admitted['operation_id'],'Operation receipt mismatch')
        if operation.get('status')!='Accepted':
            if operation.get('phase')!='completed':
                raise RuntimeError('OSA operation did not complete; no retry: '+json.dumps(operation,ensure_ascii=True))
            return operation
        if time.monotonic()>=deadline:raise TimeoutError('OSA operation deadline; no request replayed: '+request_id)
        time.sleep(.05)

def require_known_osa(record):
    identity=record.get('expected_identity',{})
    _need(record.get('model_id')=='aq6370' and record.get('profile_id')=='gpib-visa' and record.get('params')=={'resource':RESOURCE,'backend':'system','timeout_s':30} and record.get('verified_mode')=='real' and identity.get('manufacturer')=='YOKOGAWA' and identity.get('model')=='AQ6370E' and identity.get('serial')==SERIAL,'Not the previously identified local AQ6370E; no full trace read permitted')

def validate_stop(report):
    exit_report=report.get('process_exit')
    if report.get('resource_released') is not True or type(exit_report) is not dict or exit_report.get('confirmed') is not True or exit_report.get('success') is not True:
        raise RuntimeError('Resource release and successful worker exit are separate required evidence')

class HostChild:
    """Only its own Popen is owned. Never terminate a hardware-owning Host."""
    def __init__(self,binary,root,records):
        self.binary,self.root,self.records=map(Path,(binary,root,records))
        self.child=None;self.endpoint=None;self.stop_attempted=False;self.stderr=deque(maxlen=32)

    def start(self):
        _need(self.child is None,'Diagnostic Host already started')
        self.records.mkdir(parents=True,exist_ok=True)
        self.child=subprocess.Popen([str(self.binary),'--real','--root',str(self.root),'--record-dir',str(self.records),'--python',sys.executable],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,creationflags=subprocess.CREATE_NO_WINDOW)
        def errors():
            try:
                while True:
                    line=self.child.stderr.readline(4096)
                    if not line:return
                    self.stderr.append(line.rstrip())
            except (OSError,ValueError):return
        threading.Thread(target=errors,daemon=True).start()
        output=queue.Queue(maxsize=1)
        threading.Thread(target=lambda:output.put(self.child.stdout.readline(MAX_FRAME+1)),daemon=True).start()
        try:
            line=output.get(timeout=12)
            _need(0<len(line)<=MAX_FRAME,'Host startup returned no bounded endpoint')
            result=_json(line);_need(_fields(result,['endpoint','host']) and result['host']=='Yang LAB INSTRUMENT CONSOLE','Unexpected diagnostic Host startup')
            self.endpoint=result['endpoint']
        except BaseException:
            if self.child.poll() is not None:self.close_streams()
            raise
        return self

    def link(self):
        _need(self.endpoint is not None,'No owned Host endpoint')
        deadline=time.monotonic()+3
        while True:
            try:return HostLink(NativePipe(self.endpoint))
            except OSError:
                if time.monotonic()>=deadline or self.child.poll() is not None:raise
                time.sleep(.02)

    def close_streams(self):
        if self.child is not None and self.child.poll() is not None:
            self.child.stdout.close();self.child.stderr.close()

    def stop(self,client):
        _need(not self.stop_attempted,'Stop already attempted; no automatic replay')
        self.stop_attempted=True
        report=client.call('stop',{'confirm':True});validate_stop(report)
        code=self.child.wait(timeout=8)
        report={**report,'host_exit_code':code};self.close_streams()
        _need(code==0,'Owned Host exit was unsuccessful');return report

class LeasePulse:
    """Renew only the existing lease on a separately attached heartbeat channel."""
    def __init__(self,endpoint,attach_token,lease):
        self.endpoint,self.attach_token,self.lease=endpoint,attach_token,lease
        self.link=None;self.thread=None;self.done=threading.Event();self.error=None;self.renewals=0

    def renew(self):
        renewed=self.link.call('renew_control',{'token':self.lease['token']})
        _need(renewed.get('token')==self.lease['token'] and renewed.get('boot_id')==self.lease['boot_id'] and renewed.get('control_epoch')==self.lease['control_epoch'],'Heartbeat authority mismatch')
        self.renewals+=1

    def __enter__(self):
        self.link=HostLink(NativePipe(self.endpoint,timeout=3))
        try:
            hello=self.link.call('ping',{'attach_token':self.attach_token,'channel':'heartbeat'})
            _need(hello['boot_id']==self.lease['boot_id'],'Heartbeat boot mismatch')
            self.renew()
        except BaseException:
            self.link.__exit__(*sys.exc_info());raise
        def pulse():
            while not self.done.wait(2):
                try:self.renew()
                except BaseException as error:self.error=error;return
        self.thread=threading.Thread(target=pulse,daemon=True);self.thread.start();return self

    def check(self):
        if self.error is not None:raise RuntimeError('Lease heartbeat failed; no new action: '+repr(self.error)) from self.error

    def __exit__(self,kind,error,tb):
        self.done.set();self.thread.join(4)
        if self.thread.is_alive():self.error=TimeoutError('Heartbeat did not release its channel')
        else:self.link.__exit__(kind,error,tb)
        if error is None:self.check()
        elif self.error is not None:setattr(error,'heartbeat_error',repr(self.error))

def _save(path,content):
    with path.open('xb') as stream:stream.write(content);stream.flush();os.fsync(stream.fileno())

def export_archive(target,ref,manifest,raw,host,domain):
    capture=decode_archive(ref,manifest,raw,host,domain);target=Path(target);target.mkdir(exist_ok=False)
    _save(target/'manifest.json',manifest);_save(target/'native.bin',raw)
    csv=('wavelength_nm,power_'+capture.native_unit+'\n'+''.join(f'{float(x)!r},{float(y)!r}\n' for x,y in zip(capture.wavelength_nm,capture.native_values))).encode()
    _save(target/'spectrum.csv',csv)
    seal=dict(schema=1,kind='diagnostic_native_export',native_sha256=ref['sha256'],manifest_sha256=hashlib.sha256(manifest).hexdigest(),csv_sha256=hashlib.sha256(csv).hexdigest(),physical_state='not_measured')
    _save(target/'complete.json',json.dumps(seal,allow_nan=False,indent=2).encode());return seal

def main(argv=None):
    parser=argparse.ArgumentParser(description='Read only the known AQ6370E existing trace through a packaged real Host; archive, restart and diagnostic-export its exact native samples. No sweep or output changes.')
    parser.add_argument('--root',type=Path,required=True,help='Verified isolated installed candidate root')
    parser.add_argument('--out',type=Path,required=True,help='New output directory, never overwritten')
    parser.add_argument('--trace',choices=tuple('ABCDEFG'),default='A')
    parser.add_argument('--confirm-known-osa-read',required=True,action='store_true',help='Explicitly acknowledge identity probe, existing-trace read and normal close on GPIB0::4::INSTR only')
    args=parser.parse_args(argv)
    root=args.root.resolve(strict=True);out=args.out.absolute();binary=root/'yang-lab-host.exe'
    _need(os.name=='nt' and Path(sys.executable).parent.name=='VISA','Run in the Anaconda VISA environment on Windows')
    _need(not out.exists() and root.is_dir() and not root.is_symlink(),'New output and ordinary packaged root required')
    binary_hash=hashlib.sha256(binary.read_bytes()).hexdigest()
    _need(binary_hash=='9e04be9b7b06e06ec1ea828a45af3359343172eb243f227b0a93749851edecb4','Not the verified diagnostic Host binary')
    sources={name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in ('Code/Utils/osa.py','Code/Utils/osa_trace.py','App/worker/main.py','App/worker/domains.py')}
    out.mkdir(parents=True,exist_ok=False)
    report=dict(schema=1,source_kind='real',resource=RESOURCE,expected_serial=SERIAL,packaged_root=str(root),host_sha256=binary_hash,source_sha256=sources,started_at=datetime.now().astimezone().isoformat(),physical_state='not_measured',native_gui_acceptance='pending',stages=[],stops=[])
    owned=None
    def start():
        nonlocal owned
        owned=HostChild(binary,root,out/'host');owned.start()
        report['stages'].append({'stage':'host_started','pid':owned.child.pid})
        return owned.link()
    def stop(client):
        evidence=owned.stop(client);report['stops'].append(evidence)
    try:
        from App.worker.discovery import discover
        inventory=discover();report['inventory']=inventory
        _need(RESOURCE in inventory['visa'] and 'visa_close' not in inventory['errors'],'Known OSA not enumerated or inventory manager did not close')
        # Configure a new diagnostic registry only; installed operator settings
        # and existing acquisitions are never edited or adopted.
        with start() as client:
            client.call('ping');snapshot=client.call('snapshot')
            _need(snapshot['startup_error'] is None and snapshot['registry']['devices']==[],'Diagnostic registry must start empty and healthy')
            settings={**snapshot['registry']['settings'],'python_path':sys.executable,'data_root':str(out/'archive')}
            client.call('save_settings',dict(settings=settings,expected_rev=snapshot['registry']['registry_rev']))
            stop(client)
        with start() as client:
            hello=client.call('ping');snapshot=client.call('snapshot');host=snapshot['host_id'];before=hello['boot_id']
            _need(snapshot['archive']['available'] is True and Path(snapshot['archive']['active_root'])==out/'archive','Selected archive root not active')
            draft=client.call('create_draft',dict(model_id='aq6370',profile_id='gpib-visa',params={'resource':RESOURCE},name='OSA',expected_rev=snapshot['registry']['registry_rev']))
            domain=dict(kind='device',id=draft['device_id']);lease=client.call('acquire_control',{'domain':domain})
            with LeasePulse(owned.endpoint,hello['attach_token'],lease) as pulse:
                proof=client.call('test_connection',dict(draft_id=domain['id'],expected_rev=client.call('snapshot')['registry']['registry_rev'],consent=dict(accepted=True,mode='real',config_digest=draft['config_digest'],open_effects=[],supervised=False,retain_session=False),lease_token=lease['token'],control_epoch=lease['control_epoch'],request_id=uuid.uuid4().hex,sequence=1))
                pulse.check();report['identity_proof']=proof
                record=client.call('save_device',dict(draft_id=domain['id'],proof_id=proof['proof_id'],expected_rev=client.call('snapshot')['registry']['registry_rev']))
                report['device']=record;require_known_osa(record)
                report['connect']=execute_read(client,lease,domain,record['config_rev'],'connect',2,args.trace);pulse.check()
                operation=execute_read(client,lease,domain,record['config_rev'],'read_trace',3,args.trace);pulse.check()
                report['read_operation']=operation;ref=operation['result']['result']['archive_ref']
                _need(ref['id']==operation['operation_id'] and operation['result']['result'].get('staging_release_confirmed') is True,'Native staging provenance/release incomplete')
                manifest,raw,capture=fetch_archive(client,ref,host,domain)
                with owned.link() as observer:
                    observer.call('ping');other_manifest,other_raw,_=fetch_archive(observer,ref,host,domain)
                    _need(other_manifest==manifest and other_raw==raw,'Second observer archive differs')
                report['observer']={'same_native_bytes':True,'same_manifest_bytes':True,'acquired_control':False}
            stop(client)
        # Restart proves historical reads without reconnecting the instrument
        # or granting a lease. Old boot/context/lease is never reused.
        with start() as client:
            hello=client.call('ping');snapshot=client.call('snapshot')
            _need(hello['boot_id']!=before and snapshot['host_id']==host,'Host restart identity mismatch')
            key='device:'+domain['id']
            _need(snapshot['domains'][key]['context']['connection_id'] is None and snapshot['control'][key]['state']=='AVAILABLE','Restart retained old instrument authority')
            history=client.call('list_archives',dict(domain=domain,offset=0,limit=4))
            _need(any(entry.get('state')=='complete' and entry.get('reference')==ref for entry in history['entries']),'Original archive absent after restart')
            reopened_manifest,reopened_raw,_=fetch_archive(client,ref,host,domain)
            _need(reopened_manifest==manifest and reopened_raw==raw,'Restart changed original archive bytes')
            report['restart']=dict(previous_boot=before,current_boot=hello['boot_id'],same_host=True,no_connection=True,no_lease=True,same_native_bytes=True,same_manifest_bytes=True)
            report['export']=export_archive(out/'export',ref,manifest,raw,host,domain)
            report['capture']=dict(sample_count=len(capture.native_values),native_unit=capture.native_unit,trace=capture.trace,consistency=capture.consistency,sha256=ref['sha256'],manifest_sha256=hashlib.sha256(manifest).hexdigest())
            stop(client)
        report['status']='passed'
    except BaseException as error:
        report['status']='failed';report['error']=dict(type=type(error).__name__,message=str(error)[:8192])
        if owned is not None and owned.child is not None:
            report['owned_host']=dict(pid=owned.child.pid,alive=owned.child.poll() is None,stderr=list(owned.stderr))
        raise
    finally:
        if owned is not None and owned.child is not None and owned.child.poll() is None and not owned.stop_attempted and owned.endpoint is not None:
            try:
                with owned.link() as cleanup:
                    cleanup.call('ping');stop(cleanup)
            except BaseException as error:report['cleanup_error']=repr(error)
        if owned is not None and owned.child is not None:
            report['owned_host_after_cleanup']=dict(pid=owned.child.pid,alive=owned.child.poll() is None,exit_code=owned.child.poll())
            owned.close_streams()
        report['finished_at']=datetime.now().astimezone().isoformat()
        _save(out/'run.json',json.dumps(report,allow_nan=False,indent=2).encode())
    print(json.dumps({'status':report['status'],'report':str(out/'run.json'),'capture':report['capture']},allow_nan=False))
    return 0

if __name__=='__main__':raise SystemExit(main())
