"""Private native TLB adapter. Python schedules; Rust alone owns SDK transactions.

No process starts at import time. No selectable backend or ctypes fallback.
A lost response keeps the exact pending exchange and ownership; setters never replay.
"""
from __future__ import annotations
import json
import math
import re
import queue
import subprocess
import threading
from pathlib import Path
from .common import InstrumentConnectionError, InstrumentProtocolError, InstrumentSafetyError

def native_path():
    root=Path(__file__).resolve().parents[2]
    candidates=(root/'drivers/newport/yang-lab-tlb.exe',root/'App/src-tauri/binaries/yang-lab-tlb.exe')
    for candidate in candidates:
        if candidate.is_file():
            from .newport_usb import validate_dll
            return validate_dll(candidate,bits=64)
    raise InstrumentConnectionError('Native TLB driver is missing. Rebuild or reinstall the App.')

def _launch(path):
    return subprocess.Popen([str(path),'--stdio'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,bufsize=0,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))

def _unique(pairs):
    result={}
    for key,value in pairs:
        if key in result:raise ValueError('Duplicate native reply field')
        result[key]=value
    return result

def validate_status(sample):
    numbers={'wavelength_nm','wavelength_setpoint_nm','power_mw','power_setpoint_mw','current_ma','current_setpoint_ma','piezo_percent','read_interval_s'}
    switches={'output_enabled','tracking','remote','constant_power','operation_complete'}
    if (type(sample) is not dict or set(sample)!=numbers|switches|{'status_byte'}
        or any(type(sample[k]) not in {int,float} or not math.isfinite(sample[k]) for k in numbers)
        or any(type(sample[k]) is not bool for k in switches)
        or type(sample['status_byte']) is not int or not 0<=sample['status_byte']<=255
        or any(sample[k]<0 for k in numbers-{'power_mw','current_ma'}) or sample['piezo_percent']>100):
        raise InstrumentProtocolError('Invalid native status payload')
    return sample

def _validate_result(operation,reply):
    value=reply.get('result')
    if type(value) is not dict:raise ValueError('Native result must be an object')
    method=operation['method']
    if method=='hello':
        if (set(value)!={'backend','protocol','pid'} or value['backend']!='rust'
            or type(value['protocol']) is not int or value['protocol']!=1 or type(value['pid']) is not int or value['pid']<=0):raise ValueError('Invalid native hello')
    elif method=='connect':
        if set(value)!={'identity','wavelength_range_nm'}:raise ValueError('Invalid native connection result')
        identity=value['identity']
        if (type(identity) is not dict or set(identity)!={'manufacturer','model','serial','firmware','head_model','head_serial'}
            or identity['manufacturer']!='New Focus' or identity['model']!='TLB-6700'
            or identity['serial']!=operation['key'][7:]
            or any(type(v) is not str or not v or len(v)>64 for v in identity.values())
            or not re.fullmatch(r'(?:TLB-)?[0-9]{4}(?:-[A-Za-z0-9]+)*',identity['head_model'])
            or not re.fullmatch(r'[A-Za-z0-9_-]{1,64}',identity['head_serial'])):raise ValueError('Invalid native identity')
        from .tlb6700 import RANGES
        head=identity['head_model'].removeprefix('TLB-')
        expected=RANGES.get(head)
        if value['wavelength_range_nm']!=(list(expected) if expected is not None else None):raise ValueError('Invalid native head limits')
    elif method=='status':validate_status(value)
    elif method=='action':
        if value!={'completed':True} or value['completed'] is not True:raise ValueError('Native action was not completed')
    elif method in {'disconnect','shutdown'}:
        if value!={'release_confirmed':True} or value['release_confirmed'] is not True:raise ValueError('Native release was not confirmed')
    elif method=='enumerate':
        from .newport_usb import device_key
        keys=value.get('controller_keys')
        if set(value)!={'controller_keys'} or type(keys) is not list or len(keys)>32:raise ValueError('Invalid native enumeration')
        for key in keys:device_key(key)
        if len(set(keys))!=len(keys):raise ValueError('Duplicate native enumeration')
    elif method=='resources':
        if set(value)!={'resources_released'} or type(value['resources_released']) is not bool or value['resources_released']!=reply['resources_released']:raise ValueError('Invalid native release metadata')
    if method in {'connect','status','action'} and reply['resources_released']:raise ValueError('Native active session falsely released')
    if method in {'hello','enumerate','shutdown'} and not reply['resources_released']:raise ValueError('Native temporary resources retained')

class NativeManager:
    def __init__(self,*,_launch=_launch,_path=native_path,_timeout=90):
        self._launch,self._path,self._timeout=_launch,_path,_timeout
        self._lock=threading.RLock()
        self._process=None
        self._replies=None
        self._pending=None
        self._owners={}
        self._sequence=0
        self._released=True
        self._broken=False
        self._shutdown_requested=False

    @property
    def resources_released(self):
        # Cached evidence only. Do not block Host status behind a vendor call.
        if not self._lock.acquire(blocking=False):return False
        try:return self._released and not self._pending and not self._broken and not self._owners
        finally:self._lock.release()

    def owns(self,key,owner):
        with self._lock:return self._owners.get(key) is owner

    def _ensure(self):
        if self._process is not None:return
        path=self._path()
        try:self._process=self._launch(path)
        except OSError as error:raise InstrumentConnectionError('Could not start native TLB driver') from error
        self._released=False
        self._replies=queue.Queue(maxsize=2)
        output,replies=self._process.stdout,self._replies
        def read():
            try:
                while True:
                    line=output.readline(4097)
                    if not line:replies.put(InstrumentConnectionError('Native driver ended; outcome unknown'));return
                    if len(line)>4096 or not line.endswith(b'\n'):raise ValueError('Native reply exceeds capacity')
                    replies.put(json.loads(line,object_pairs_hook=_unique,parse_constant=lambda v:(_ for _ in ()).throw(ValueError(v))))
            except Exception:
                replies.put(InstrumentConnectionError('Native driver reply is invalid; outcome unknown'))
        threading.Thread(target=read,name='NativeTLBReplies',daemon=True).start()
        stderr=self._process.stderr
        def drain_errors():
            try:
                while stderr.read(4096):pass
            except (OSError,ValueError):pass
        threading.Thread(target=drain_errors,name='NativeTLBErrors',daemon=True).start()
        try:
            hello=self._send({'method':'hello'})
            if hello.get('backend')!='rust' or hello.get('protocol')!=1:
                raise InstrumentConnectionError('Native driver startup identity mismatch')
        except Exception:
            # The fixed native program cannot open hardware before a post-handshake operation.
            # EOF is normal cleanup, never termination. If exit is uncertain, retain this owner.
            try:
                self._process.stdin.close()
                self._process.wait(timeout=3)
            except Exception:
                self._broken=True
            else:
                self._detach()
                self._released=True
                self._broken=False
            raise

    def _receive(self):
        try:reply=self._replies.get(timeout=self._timeout)
        except queue.Empty as error:
            raise InstrumentConnectionError('Native driver reply timed out; outcome unknown. Disconnect to settle the original operation.') from error
        if isinstance(reply,Exception):
            self._broken=True
            raise reply
        expected={'v','id','ok','resources_released','result' if reply.get('ok') is True else 'error'} if isinstance(reply,dict) else set()
        if (not isinstance(reply,dict) or set(reply)!=expected or reply.get('v')!=1
            or type(reply.get('v')) is not int or type(reply.get('id')) is not int or reply['id']!=self._pending['id']
            or type(reply.get('ok')) is not bool or type(reply.get('resources_released')) is not bool):
            self._broken=True
            raise InstrumentConnectionError('Native reply identity is invalid; outcome unknown')
        try:
            if reply['ok']:_validate_result(self._pending['operation'],reply)
            elif (type(reply['error']) is not dict or set(reply['error'])!={'kind','message'}
                or reply['error']['kind'] not in {'connection','protocol','safety','read_failure'}
                or type(reply['error']['message']) is not str):raise ValueError('Invalid native error')
        except Exception as error:
            self._broken=True
            raise InstrumentConnectionError('Native method acknowledgement is invalid; outcome unknown') from error
        self._released=reply['resources_released']
        if self._pending['operation']['method']=='shutdown' and not reply['ok']:self._shutdown_requested=False
        self._pending=None
        return reply

    @staticmethod
    def _result(reply):
        if reply['ok']:
            if type(reply['result']) is not dict:raise InstrumentProtocolError('Invalid native result')
            return reply['result']
        error=reply['error']
        if type(error) is not dict or set(error)!={'kind','message'} or type(error['message']) is not str:
            raise InstrumentProtocolError('Invalid native error')
        kind={'safety':InstrumentSafetyError,'protocol':InstrumentProtocolError}.get(error.get('kind'),InstrumentConnectionError)
        raise kind(error['message'][:512])

    def _send(self,operation):
        if self._pending is not None or self._broken:
            raise InstrumentConnectionError('A native operation remains unresolved; disconnect before continuing')
        if self._process.poll() is not None:
            self._broken=True
            raise InstrumentConnectionError('Native owner exited; connection outcome unknown')
        self._sequence+=1
        request={'id':self._sequence,'operation':operation}
        line=(json.dumps(request,separators=(',',':'),allow_nan=False)+'\n').encode('ascii')
        if len(line)>4096:raise InstrumentProtocolError('Native request exceeds capacity')
        self._pending=request
        self._released=False
        try:
            # PIPE_BUF is not assumed; raw Windows writes may be partial.
            sent=0
            while sent<len(line):
                n=self._process.stdin.write(line[sent:])
                if not n:raise OSError('Native input closed')
                sent+=n
            self._process.stdin.flush()
        except OSError as error:
            self._broken=True
            raise InstrumentConnectionError('Native request dispatch failed; outcome unknown') from error
        return self._result(self._receive())

    def call(self,operation):
        with self._lock:
            self._ensure()
            return self._send(operation)

    def connect(self,key,owner):
        with self._lock:
            if key in self._owners:raise InstrumentConnectionError('This controller already has a session')
            self._owners[key]=owner
            # Keep the reservation on every error until explicit preserving close.
            return self.call({'method':'connect','key':key})

    def operation(self,key,owner,method,**params):
        with self._lock:
            if self._owners.get(key) is not owner:raise InstrumentConnectionError('Native controller has no current owner')
            return self.call({'method':method,'key':key,**params})

    def _stop_released(self):
        if self._process is None:return
        if self._pending is not None:self._receive() # settle, never replay the original request
        if self._broken:raise InstrumentConnectionError('Native ownership remains unresolved')
        if not self._released or self._owners:return
        if not self._shutdown_requested:
            self._shutdown_requested=True
            self._send({'method':'shutdown'})
        if not self._released:raise InstrumentConnectionError('Native SDK release is unconfirmed')
        try:exit_code=self._process.wait(timeout=3)
        except Exception as error:raise InstrumentConnectionError('Native SDK released; owner exit remains pending') from error
        if exit_code!=0:raise InstrumentConnectionError('Native owner exit failed; inspect cleanup')
        self._detach()

    def _detach(self):
        for stream in (self._process.stdin,self._process.stdout,self._process.stderr):stream.close()
        self._process=None
        self._replies=None
        self._pending=None
        self._shutdown_requested=False

    def disconnect(self,key,owner):
        with self._lock:
            if self._owners.get(key) is not owner:return
            if self._process is not None:
                settled_disconnect=False
                if self._pending is not None:
                    previous=self._pending['operation']
                    reply=self._receive()
                    settled_disconnect=previous=={'method':'disconnect','key':key} and reply['ok']
                if not self._shutdown_requested and not settled_disconnect:
                    reply=self._send({'method':'disconnect','key':key})
                    if reply.get('release_confirmed') is not True:raise InstrumentConnectionError('Native controller release is unconfirmed')
            del self._owners[key]
            try:self._stop_released()
            except Exception:
                # Restore the claim until the original shutdown acknowledgement and owner exit settle.
                self._owners[key]=owner
                raise

    @property
    def needs_cleanup(self):
        with self._lock:return self._process is not None or bool(self._owners) or self._pending is not None

    def cleanup_unowned(self):
        """Preserving settlement of a scan/owner without inventing a controller key."""
        with self._lock:
            if self._owners:raise InstrumentConnectionError('Native controllers still retain responsibility')
            if self._pending is not None:self._receive()
            if self._broken:raise InstrumentConnectionError('Native ownership remains unresolved')
            if self._process is not None and not self._released and not self._shutdown_requested:
                self._shutdown_requested=True
                self._send({'method':'shutdown'})
            self._stop_released()

    def enumerate(self):
        with self._lock:
            if self._owners:raise InstrumentConnectionError('Disconnect Newport controllers before refreshing the device list.')
            if self._pending is not None or self._shutdown_requested:
                # This explicit refresh settles the old scan first. Discard its old keys;
                # the new user request gets a fresh enumeration from a released owner.
                self.cleanup_unowned()
            try:
                result=self.call({'method':'enumerate'})
                keys=result.get('controller_keys')
                from .newport_usb import device_key
                if type(keys) is not list or len(keys)>32 or len(set(keys))!=len(keys):raise InstrumentProtocolError('Invalid native controller enumeration')
                return tuple(device_key(key) for key in keys)
            finally:
                if self._released and not self._pending and not self._broken:self._stop_released()

NATIVE=NativeManager()

class NativeLaser:
    def __init__(self,*,_manager=NATIVE):self._manager,self._key,self._owner=_manager,None,object()
    @property
    def is_open(self):return self._key is not None and self._manager.owns(self._key,self._owner)
    def connect(self,key):
        self._key=key
        return self._manager.connect(key,self._owner)
    def status(self):return self._manager.operation(self._key,self._owner,'status')
    def action(self,name,value,confirm):
        return self._manager.operation(self._key,self._owner,'action',action={'name':name,'value':value},confirm=confirm)
    def close(self):
        self._manager.disconnect(self._key,self._owner)
        self._key=None
