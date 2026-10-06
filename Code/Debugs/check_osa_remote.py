"""Real native Host TLS diagnostic; link-only opens no instrument resource.

Optional OSA stage uses owning Host drivers, never raw VISA/SCPI. No scan is started.
Credentials stay in memory or the Host's DPAPI file and are excluded from evidence.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import socket
import ssl
import struct
import sys
import threading
import time
import uuid

from .check_osa_host import (HostChild, HostLink, LeasePulse, MAX_FRAME, NativePipe, RESOURCE,
    _json, _need, _save, execute_read, fetch_archive, export_archive, require_known_osa)


class TlsTransport:
    def __init__(self, endpoint, fingerprint, timeout=35):
        address, port = endpoint.rsplit(':', 1)
        _need(address == '127.0.0.1' and 0 < int(port) <= 65535, 'Diagnostic uses loopback only')
        self.socket = None
        tcp = socket.create_connection((address, int(port)), timeout=timeout)
        try:
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE
            context.minimum_version = ssl.TLSVersion.TLSv1_3
            self.socket = context.wrap_socket(tcp, server_hostname='yang-lab')
            _need(hashlib.sha256(self.socket.getpeercert(binary_form=True)).hexdigest() == fingerprint,
                  'Host certificate fingerprint mismatch; no application frame sent')
        except BaseException:
            if self.socket is not None: self.socket.close()
            else: tcp.close()
            raise

    def read(self, length):
        _need(type(length) is int and 0 < length <= MAX_FRAME, 'Unbounded TLS read')
        result = bytearray()
        while len(result) < length:
            data = self.socket.recv(length - len(result))
            _need(bool(data), 'Remote stream closed')
            result.extend(data)
        return bytes(result)

    def write(self, data):
        _need(type(data) is bytes and 4 < len(data) <= MAX_FRAME + 4, 'Unbounded TLS write')
        self.socket.sendall(data)

    def close(self):
        if self.socket is not None:
            self.socket.close()
            self.socket = None


def event(client):
    raw = bytearray()
    expected = None
    while True:
        length, = struct.unpack('<I', client.transport.read(4))
        _need(0 < length <= MAX_FRAME, 'Unbounded event frame')
        chunk = _json(client.transport.read(length))
        _need(chunk.get('event_chunk') is True and type(chunk.get('size')) is int and 0 < chunk['size'] <= 1024*1024,
              'Unbounded event')
        binding = (chunk.get('seq'),chunk['size'],chunk.get('checksum'))
        _need(chunk.get('offset') == len(raw) and (expected is None or binding == expected), 'Event chunk mismatch')
        expected = binding
        text = chunk.get('data_hex')
        _need(type(text) is str and 0 < len(text) <= 56000 and len(text)%2 == 0 and all(c in '0123456789abcdef' for c in text), 'Invalid event bytes')
        data = bytes.fromhex(text)
        _need(len(raw)+len(data) <= chunk['size'], 'Event overflow')
        raw.extend(data)
        if len(raw) == chunk['size']:
            _need(hashlib.sha256(raw).hexdigest() == binding[2], 'Event hash mismatch')
            return _json(bytes(raw))


def wait(check, timeout=8):
    deadline = time.monotonic() + timeout
    while True:
        value = check()
        if value: return value
        _need(time.monotonic() < deadline, 'Diagnostic deadline expired')
        time.sleep(.05)


class RemotePulse:
    def __init__(self, open_channel, lease):
        self.open_channel, self.lease = open_channel, lease
        self.done = threading.Event()
        self.error = None
        self.client = None
    def __enter__(self):
        self.client = self.open_channel('heartbeat')
        def pulse():
            while not self.done.is_set():
                try:
                    renewed = self.client.call('renew_control', {'token':self.lease['token']})
                    _need(renewed['token'] == self.lease['token'] and renewed['boot_id'] == self.lease['boot_id'], 'Remote lease changed')
                except BaseException as error:
                    self.error = error
                    return
                self.done.wait(2)
        self.thread = threading.Thread(target=pulse, daemon=True)
        self.thread.start()
        return self
    def __exit__(self, kind, error, tb):
        self.done.set()
        self.thread.join(4)
        if self.thread.is_alive(): self.error = TimeoutError('Remote heartbeat did not settle')
        else: self.client.__exit__(kind, error, tb)
        if error is None and self.error is not None: raise self.error


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument('--host', type=Path, required=True, help='Explicit freshly built native Host binary')
    result.add_argument('--out', type=Path, required=True, help='New evidence directory')
    result.add_argument('--read-osa', action='store_true')
    result.add_argument('--confirm-known-osa-read', action='store_true')
    result.add_argument('--trace', choices=tuple('ABCDEFG'), default='A')
    return result


def main(argv=None):
    args = parser().parse_args(argv)
    _need(os.name == 'nt' and Path(sys.executable).parent.name == 'VISA', 'Run in Anaconda VISA on Windows')
    _need(not args.read_osa or args.confirm_known_osa_read, 'Explicit OSA read-stage authorization required')
    root = Path(__file__).resolve().parents[2]
    binary = args.host.resolve(strict=True)
    out = args.out.absolute()
    out.mkdir(parents=True, exist_ok=False)
    report = dict(schema=1, transport='native_rust_tls13', link='loopback', real_two_pc='pending', native_two_app='pending',
                  hardware_stage='existing_osa_trace' if args.read_osa else 'none', host_sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
    owned = HostChild(binary, root, out/'host')
    remote = None
    owner = None
    peer_id = uuid.uuid4().hex
    try:
        owned.start()
        owner = owned.link()
        hello = owner.call('ping')
        snapshot = owner.call('snapshot')
        _need(snapshot['startup_error'] is None and snapshot['registry']['devices'] == [], 'Fresh empty Host required')
        report['host_id'] = snapshot['host_id']
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            endpoint = '127.0.0.1:' + str(probe.getsockname()[1])
        owner.call('remote_listener', {'endpoint':endpoint})
        status = wait(lambda: (s if (s := owner.call('remote_status'))['transport']['state'] == 'LISTENING' else None))
        fingerprint = status['fingerprint']
        try:
            TlsTransport(endpoint, '0'*64)
            raise AssertionError('Wrong certificate accepted')
        except ValueError: report['wrong_pin_rejected'] = True
        with HostLink(TlsTransport(endpoint, fingerprint)) as rejected:
            try:
                rejected.call('remote_auth', {'peer_id':peer_id,'credential':'0'*64,'join':{}})
                raise AssertionError('Unpaired client admitted')
            except RuntimeError: report['unpaired_rejected'] = True
        code = owner.call('remote_pair_begin')['code']
        paired = queue.Queue(maxsize=1)
        def pair():
            try:
                with HostLink(TlsTransport(endpoint, fingerprint)) as link:
                    response = link.call('remote_pair', {'peer_id':peer_id,'name':'TLS diagnostic','code':code})
                    paired.put(response)
            except BaseException as error: paired.put(error)
        thread = threading.Thread(target=pair, daemon=True)
        thread.start()
        pending = wait(lambda: owner.call('remote_status')['pending'])
        owner.call('remote_approve', {'id':pending[0]['id']})
        response = paired.get(timeout=10)
        if isinstance(response, BaseException): raise response
        thread.join(2)
        _need(response['host_id'] == snapshot['host_id'], 'Pairing Host mismatch')
        credential = response['credential']  # never included in reports
        remote = HostLink(TlsTransport(endpoint, fingerprint))
        auth = remote.call('remote_auth', {'peer_id':peer_id,'credential':credential,'join':{}})
        _need(auth['host_id'] == snapshot['host_id'] and auth['worker_protocol'] == 3, 'Remote identity mismatch')
        attach = auth['attach_token']
        old_release = {key:auth[key] for key in ('boot_id','client_session_id','release_token')}
        def channel(name):
            link = HostLink(TlsTransport(endpoint, fingerprint, timeout=8))
            try:
                reply = link.call('remote_auth', {'peer_id':peer_id,'credential':credential,'join':{'attach_token':attach,'channel':name}})
                _need(reply['client_session_id'] == auth['client_session_id'], 'Remote channel session mismatch')
                return link
            except BaseException:
                link.__exit__(*sys.exc_info())
                raise
        _need(remote.call('catalog') == owner.call('catalog'), 'Authoritative catalog mismatch')
        _need(remote.call('snapshot')['host_id'] == snapshot['host_id'], 'Remote snapshot belongs to another Host')
        try:
            remote.call('save_settings', {'settings':snapshot['registry']['settings'],'expected_rev':0})
            raise AssertionError('Remote configuration write admitted')
        except RuntimeError: report['remote_configuration_rejected'] = True
        with channel('events') as events:
            events.call('subscribe')
            first = event(events)
            _need(first['host_id'] == snapshot['host_id'], 'Remote event identity mismatch')
            receipt = remote.call('request_snapshot')
            wait(lambda: (e if (e := event(events)).get('type') == 'snapshot' and e['seq'] >= receipt['seq'] else None))
            report['remote_event_stream'] = True
        # Closing events revokes that peer session; create a fresh explicit session, never reclaim a lease.
        remote.__exit__(None, None, None)
        remote = HostLink(TlsTransport(endpoint, fingerprint))
        auth = remote.call('remote_auth', {'peer_id':peer_id,'credential':credential,'join':{}})
        attach = auth['attach_token']
        recovered = remote.call('reconcile_client', old_release)
        _need(recovered['released'] is True and recovered['host_id'] == snapshot['host_id'] and recovered['client_session_id'] == old_release['client_session_id'], 'Old-session release was not reconciled')
        try:
            remote.call('reconcile_client', {**old_release,'release_token':'0'*64})
            raise AssertionError('Foreign release capability accepted')
        except RuntimeError: report['foreign_release_proof_rejected'] = True
        report['old_session_reconciled'] = True
        if args.read_osa:
            from App.worker.discovery import discover
            inventory = discover()
            _need(RESOURCE in inventory['visa'], 'Known OSA not enumerated')
            registry = owner.call('snapshot')['registry']
            draft = owner.call('create_draft', dict(model_id='aq6370',profile_id='gpib-visa',params={'resource':RESOURCE},name='OSA',expected_rev=registry['registry_rev']))
            domain = dict(kind='device',id=draft['device_id'])
            local_lease = owner.call('acquire_control', {'domain':domain})
            with LeasePulse(owned.endpoint, hello['attach_token'], local_lease):
                proof = owner.call('test_connection',dict(draft_id=domain['id'],expected_rev=owner.call('snapshot')['registry']['registry_rev'],consent=dict(accepted=True,mode='real',config_digest=draft['config_digest'],open_effects=[],supervised=False,retain_session=False),lease_token=local_lease['token'],control_epoch=local_lease['control_epoch'],request_id=uuid.uuid4().hex,sequence=1))
                record = owner.call('save_device',dict(draft_id=domain['id'],proof_id=proof['proof_id'],expected_rev=owner.call('snapshot')['registry']['registry_rev']))
                require_known_osa(record)
            # Closing the attached heartbeat fences the old local session and schedules cleanup.
            owner.__exit__(None,None,None);owner=owned.link();hello=owner.call('ping')
            wait(lambda: owner.call('snapshot')['control']['device:'+domain['id']]['state']=='AVAILABLE')
            lease = remote.call('acquire_control',{'domain':domain})
            try:
                owner.call('acquire_control',{'domain':domain})
                raise AssertionError('Local GUI bypassed remote lease')
            except RuntimeError: report['local_remote_contention'] = True
            with RemotePulse(channel,lease):
                report['connect'] = execute_read(remote,lease,domain,record['config_rev'],'connect',1,args.trace)
                operation = execute_read(remote,lease,domain,record['config_rev'],'read_trace',2,args.trace)
                ref = operation['result']['result']['archive_ref']
                manifest,raw,capture = fetch_archive(remote,ref,snapshot['host_id'],domain)
                other_manifest,other_raw,_ = fetch_archive(owner,ref,snapshot['host_id'],domain)
                _need(manifest == other_manifest and raw == other_raw, 'Local/remote samples differ')
                report['osa'] = dict(operation_id=operation['operation_id'],sample_count=len(capture.native_values),native_unit=capture.native_unit,same_native_bytes=True,sha256=ref['sha256'],consistency=capture.consistency)
                report['export'] = export_archive(out/'export',ref,manifest,raw,snapshot['host_id'],domain)
        old_release = {key:auth[key] for key in ('boot_id','client_session_id','release_token')}
        report['first_stop'] = owned.stop(owner)
        remote.__exit__(None,None,None);remote=None
        owner.__exit__(None,None,None);owner=None
        owned=HostChild(binary,root,out/'host');owned.start();owner=owned.link();owner.call('ping')
        status=wait(lambda:(s if (s:=owner.call('remote_status'))['transport']['state']=='LISTENING' else None))
        _need(status['fingerprint']==fingerprint and status['host_id']==snapshot['host_id'],'Restarted Host identity changed')
        remote=HostLink(TlsTransport(endpoint,fingerprint))
        after=remote.call('remote_auth',{'peer_id':peer_id,'credential':credential,'join':{}})
        _need(after['boot_id']!=old_release['boot_id'],'Host boot did not change')
        recovered=remote.call('reconcile_client',old_release)
        _need(recovered['released'] is True and recovered['boot_id']==old_release['boot_id'] and recovered['client_session_id']==old_release['client_session_id'] and recovered['host_stopped'] is True,'Verified old-boot release receipt unavailable')
        report['verified_restart_release'] = True
        owner.call('remote_revoke',{'id':peer_id})
        with HostLink(TlsTransport(endpoint,fingerprint)) as revoked:
            try:
                revoked.call('remote_auth',{'peer_id':peer_id,'credential':credential,'join':{}})
                raise AssertionError('Revoked peer admitted')
            except RuntimeError: report['revoked_rejected'] = True
        owner.call('remote_listener',{'endpoint':None})
        report['stop'] = owned.stop(owner)
        report['status'] = 'passed'
    except BaseException as error:
        report['status'] = 'failed'
        report['error'] = dict(type=type(error).__name__,message=str(error)[:1024])
        raise
    finally:
        if remote is not None: remote.__exit__(None,None,None)
        if owned.child is not None and owned.child.poll() is None and not owned.stop_attempted:
            try:
                with owned.link() as cleanup:
                    cleanup.call('ping')
                    cleanup.call('remote_revoke',{'id':peer_id})
                    report['cleanup_stop'] = owned.stop(cleanup)
            except BaseException as error: report['cleanup_error'] = type(error).__name__
        if owner is not None: owner.__exit__(None,None,None)
        owned.close_streams()
        report['host_alive'] = owned.child is not None and owned.child.poll() is None
        _save(out/'run.json',json.dumps(report,allow_nan=False,indent=2).encode())
    print(json.dumps(dict(status=report['status'],report=str(out/'run.json'),hardware_stage=report['hardware_stage'])))
    return 0


if __name__ == '__main__': raise SystemExit(main())
