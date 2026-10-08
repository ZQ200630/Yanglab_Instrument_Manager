"""Staged TLB-6700 checks. No output-setting or emission-enable CLI exists.

--inventory: Windows/SDK metadata, no instrument opening.
--list-controllers: separately authorized active read-only SDK identity discovery.
--device-key ... --identity/--status: authorized read-only connection and release.
"""
import argparse
import dataclasses
import json
import logging
import uuid

from Code.Utils.newport_usb import inventory
from Code.Utils.tlb6700 import TLB6700


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    stage = parser.add_mutually_exclusive_group(required=True)
    stage.add_argument('--inventory', action='store_true')
    stage.add_argument('--list-controllers', action='store_true')
    stage.add_argument('--identity', action='store_true')
    stage.add_argument('--status', action='store_true')
    stage.add_argument('--worker-status', action='store_true', help='Read through the real v3 worker and preserving close')
    parser.add_argument('--device-key')
    parser.add_argument('--verbose', action='store_true')
    parser.add_argument('--samples', type=int, choices=range(1, 6), default=1)
    args = parser.parse_args(argv)
    if args.verbose:
        logging.basicConfig(level=logging.DEBUG)
    if args.inventory or args.list_controllers:
        if args.device_key:
            parser.error('--device-key applies only to connection checks')
        result = inventory() if args.inventory else {'controller_keys': TLB6700.enumerate(), 'stage':'readonly_identity_discovery'}
    else:
        if not args.device_key:
            parser.error('--device-key is required; never select a controller by order')
        if args.worker_status:
            from App.worker.controller import DomainController
            from App.worker.contracts_v3 import domain_config, RequestV3
            controller = DomainController(session_id=uuid.uuid4().hex)
            config = domain_config(dict(domain={'kind':'device','id':uuid.uuid4().hex},config_rev=1,
                driver_kind='laser',model_id='tlb6700',profile_id='newport-usb',params={'device_key':args.device_key},
                expected_identity={},members=[]))
            def call(method, params):
                outcome = controller.submit(RequestV3(uuid.uuid4().hex, method, params, controller.context(config.domain))).result(30)
                if outcome.phase != 'completed':
                    raise RuntimeError(f'Worker read-only check failed: {outcome.error}')
                return outcome.result
            samples = []
            try:
                controller.configure(config)
                call('connect', {})
                device = controller.device(config.domain)
                identity = dict(device.identity)
                for index in range(args.samples):
                    if index: call('action', {'name':'read_status','args':{}})
                    snapshot = controller.cached_status()['devices']['device:' + config.domain.id]
                    samples.append(snapshot)
                call('disconnect', {})
            finally:
                report = controller.close()
            result = {'identity':identity,'pipeline':'real v3 worker','samples':samples,
                      'release_confirmed':not report['unreleased']}
            print(json.dumps(result, indent=2, ensure_ascii=False))
            return 0
        device = TLB6700(device_key=args.device_key)
        try:
            device.connect()
            result = {'identity': device.identity, 'wavelength_range_nm': device.wavelength_range_nm,
                'range_source':'published standard model envelope; not a measured spectrum'}
            if args.status:
                samples = []
                for _ in range(args.samples):
                    sample = dataclasses.asdict(device.read_status())
                    sample.pop('received_at')
                    samples.append(sample)
                result['readback' if len(samples)==1 else 'readbacks'] = samples[0] if len(samples)==1 else samples
        finally:
            device.close()
        result['release_confirmed'] = device.resources_released
    print(json.dumps(result, indent=2, ensure_ascii=False))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
