"""Authorized read-only TLB-6700 qualification through an owned native Host.

No output-setting command, hardware backend choice, retry or external Host stop.
"""
import argparse
from datetime import datetime
import hashlib
import json
import math
from pathlib import Path
import re
import sys
import time
import uuid

from Code.Debugs.check_osa_host import HostChild, LeasePulse, _id, _integer, _need, _save

ROOT = Path(__file__).resolve().parents[2]


def read_intent(snapshot, lease, domain, rev, action, sequence):
    _need(action in ('connect', 'read_status'), 'Diagnostic permits connect/read_status only')
    _need(type(domain) is dict and domain.get('kind') == 'device' and _id(domain.get('id')), 'Invalid laser domain')
    _need(_id(snapshot.get('boot_id')) and lease.get('boot_id') == snapshot['boot_id']
          and _id(lease.get('token')) and _integer(lease.get('control_epoch'), 0, 2**53-1)
          and _integer(sequence, 1, 2**53-1) and _integer(rev, 1, 2**53-1), 'Current authority absent')
    context = snapshot['domains']['device:'+domain['id']]['context']
    _need(context['domain'] == domain and _id(context['session_id'])
          and _integer(context['epoch'], 0, 2**53-1), 'Domain context mismatch')
    _need(context['connection_id'] is None if action == 'connect' else _id(context['connection_id']),
          'Connection context mismatch')
    return dict(domain=domain, lease_token=lease['token'], control_epoch=lease['control_epoch'],
                config_rev=rev, context=context, method='connect' if action == 'connect' else 'action',
                params={'acknowledge_lifecycle':True} if action == 'connect' else {'name':'read_status', 'args':{}},
                sequence=sequence, confirmation=None)


def execute_read(client, lease, domain, rev, action, sequence, timeout=45):
    """Submit once, then poll that receipt. Never replay an uncertain operation."""
    _need(type(timeout) in (int, float) and math.isfinite(timeout) and 0 < timeout <= 45, 'Invalid deadline')
    intent = read_intent(client.call('snapshot'), lease, domain, rev, action, sequence)
    intent['confirmation'] = client.call('prepare', {'intent':intent})['token']
    request_id = uuid.uuid4().hex
    admitted = client.call('execute', dict(request_id=request_id, intent=intent))
    _need(_id(admitted.get('operation_id')), 'Unbound operation admission')
    deadline = time.monotonic()+timeout
    while True:
        operation = client.call('operation', {'request_id':request_id})
        _need(operation.get('operation_id') == admitted['operation_id'], 'Operation receipt mismatch')
        if operation.get('status') != 'Accepted':
            if operation.get('phase') != 'completed':
                raise RuntimeError('Laser operation failed; no retry: '+json.dumps(operation, ensure_ascii=True))
            return operation
        if time.monotonic() >= deadline: raise TimeoutError('Laser operation deadline; no replay: '+request_id)
        time.sleep(.05)


def require_laser(record, device_key):
    identity = record.get('expected_identity', {})
    _need(record.get('model_id') == 'tlb6700' and record.get('profile_id') == 'newport-usb'
          and record.get('params') == {'device_key':device_key} and record.get('verified_mode') == 'real'
          and identity.get('model') == 'TLB-6700' and '6700 SN'+identity.get('serial', '') == device_key
          and identity.get('head_model') and identity.get('head_serial'), 'Laser controller/head identity mismatch')


def await_sample(client, domain, previous=None):
    """Wait for Host's bounded status cache to publish the completed acquisition."""
    deadline = time.monotonic()+10
    while True:
        status = client.call('snapshot')['domains']['device:'+domain['id']]
        device = status.get('device')
        sample = device.get('laser') if type(device) is dict else None
        if type(sample) is dict and sample.get('received_at') != previous and _id(status['context']['connection_id']):
            return device
        if time.monotonic() >= deadline: raise TimeoutError('New laser sample was not published; no acquisition replayed')
        time.sleep(.05)


def qualify(client, endpoint, device_key, samples, *, hello=None, evidence=None):
    """Production admission/verification path; callers own and stop the Host."""
    _need(type(device_key) is str and re.fullmatch(r'6700 SN\d{1,16}', device_key), 'Invalid exact controller key')
    _need(type(samples) is int and 1 <= samples <= 5, 'Unbounded sample count')
    progress = {} if evidence is None else evidence
    _need(type(progress) is dict and not progress, 'Use an empty evidence record for this attempt')
    hello = client.call('ping') if hello is None else hello
    _need(type(hello) is dict and _id(hello.get('boot_id')) and _id(hello.get('attach_token')),
          'Retain the initial Host handshake before any metadata request')
    snapshot = client.call('snapshot')
    _need(snapshot['startup_error'] is None and snapshot['registry']['devices'] == [],
          'Diagnostic registry must start empty and healthy')
    draft = client.call('create_draft', dict(model_id='tlb6700', profile_id='newport-usb',
        params={'device_key':device_key}, name='Laser', expected_rev=snapshot['registry']['registry_rev']))
    domain = dict(kind='device', id=draft['device_id'])
    lease = client.call('acquire_control', {'domain':domain})
    with LeasePulse(endpoint, hello['attach_token'], lease) as pulse:
        proof = client.call('test_connection', dict(draft_id=domain['id'],
            expected_rev=client.call('snapshot')['registry']['registry_rev'],
            consent=dict(accepted=True, mode='real', config_digest=draft['config_digest'],
                         open_effects=[], supervised=False, retain_session=False),
            lease_token=lease['token'], control_epoch=lease['control_epoch'],
            request_id=uuid.uuid4().hex, sequence=1))
        pulse.check()
        record = client.call('save_device', dict(draft_id=domain['id'], proof_id=proof['proof_id'],
            expected_rev=client.call('snapshot')['registry']['registry_rev']))
        require_laser(record, device_key)
        progress.update(record=record, operations=[], samples=[])
        operations, readings = progress['operations'], progress['samples']
        operations.append(execute_read(client, lease, domain, record['config_rev'], 'connect', 2))
        for index in range(samples):
            if index: operations.append(execute_read(client, lease, domain, record['config_rev'], 'read_status', index+2))
            pulse.check()
            device = await_sample(client, domain, readings[-1]['laser']['received_at'] if readings else None)
            _need(device['identity'] == record['expected_identity'] and type(device.get('laser')) is dict,
                  'Identity-bound laser sample absent')
            readings.append(device)
    return progress


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host-binary', required=True, type=Path)
    parser.add_argument('--device-key', required=True)
    parser.add_argument('--out', required=True, type=Path, help='New directory below Result')
    parser.add_argument('--samples', choices=range(1, 6), type=int, default=3)
    parser.add_argument('--confirm-readonly', action='store_true')
    args = parser.parse_args(argv)
    if not args.confirm_readonly: parser.error('Separate operator read-only authorization is required')
    if not re.fullmatch(r'6700 SN\d{1,16}', args.device_key): parser.error('Use an exact controller key')
    if not args.host_binary.is_absolute() or not args.host_binary.is_file(): parser.error('Host binary must be an existing absolute path')
    if sys.version_info[:3] != (3, 10, 16) or Path(sys.prefix).name.casefold() != 'visa':
        parser.error('Run source diagnostics in Anaconda VISA Python3.10.16')
    out = args.out.resolve()
    if not out.is_relative_to(ROOT/'Result') or out == ROOT/'Result' or out.exists():
        parser.error('Use a new output directory below this repository Result directory')
    out.mkdir(parents=True)
    report = dict(started_at=datetime.now().astimezone().isoformat(), pipeline='native Host / real v3 worker',
                  stage='read-only', host_sha256=hashlib.sha256(args.host_binary.read_bytes()).hexdigest(),
                  python=sys.executable, physical_state='not independently measured', status='failed')
    owned = HostChild(args.host_binary, ROOT, out/'records')
    try:
        owned.start()
        with owned.link() as client:
            hello = client.call('ping')
            report['driver_status'] = client.call('driver_status')
            report['qualification'] = {}
            qualify(client, owned.endpoint, args.device_key, args.samples, hello=hello, evidence=report['qualification'])
            report['stop'] = owned.stop(client)
        report['status'] = 'passed'
    except BaseException as error:
        report['error'] = dict(type=type(error).__name__, message=str(error)[:8192])
        raise
    finally:
        if owned.child is not None and owned.child.poll() is None and not owned.stop_attempted and owned.endpoint is not None:
            try:
                with owned.link() as cleanup:
                    cleanup.call('ping')
                    report['stop'] = owned.stop(cleanup)
            except BaseException as error: report['cleanup_error'] = repr(error)
        if owned.child is not None:
            report['host_after_cleanup'] = dict(pid=owned.child.pid, alive=owned.child.poll() is None,
                                               exit_code=owned.child.poll(), stderr=list(owned.stderr))
            owned.close_streams()
        report['finished_at'] = datetime.now().astimezone().isoformat()
        _save(out/'run.json', json.dumps(report, allow_nan=False, indent=2).encode())
    print(json.dumps({'status':report['status'], 'report':str(out/'run.json')}))
    return 0


if __name__ == '__main__': raise SystemExit(main())
