"""Separately authorized, bounded Tracking diagnostic through the reviewed driver.

No output/current/power/piezo/scan commands. Uncertain actions stop and hold;
only a fully successful sequence restores the original target and Tracking.
"""
import argparse
import dataclasses
import json
from pathlib import Path
import time

from Code.Utils.tlb6700 import TLB6700


def qualify(device, record, step=.01, restore_target=None, restore_local=False):
    def call(name, *args, **kwargs):
        began=time.monotonic()
        value=getattr(device,name)(*args,**kwargs)
        entry={'method':name,'elapsed_s':time.monotonic()-began}
        if dataclasses.is_dataclass(value):entry['sample']=dataclasses.asdict(value)
        elif isinstance(value,dict):entry['sample']=value
        record['steps'].append(entry)
        return value

    def arrived(target, require_tracking=True):
        deadline=time.monotonic()+15
        for _ in range(60):
            sample=call('read_motion')
            if sample['operation_complete']:
                if abs(sample['wavelength_nm']-baseline)>.02:
                    raise RuntimeError('Readback exceeded the approved excursion; holding state')
                if require_tracking and device.target_following_enabled is not True or abs(sample['wavelength_nm']-target)>.008:
                    raise RuntimeError('Target following did not reach the bounded target; holding state')
                return sample
            if time.monotonic()>deadline:break
            time.sleep(.15)
        raise RuntimeError('Motor did not finish in the diagnostic budget; holding state')

    initial=call('read_status')
    if not initial.operation_complete:raise RuntimeError('Controller is busy; no diagnostic writes sent')
    if device.identity['head_model']!='6722-P' or device.identity['serial']!='22500001':
        raise RuntimeError('Diagnostic identity differs from the approved controller/head')
    baseline=round(initial.wavelength_nm,3)
    original_target=initial.wavelength_setpoint_nm if restore_target is None else restore_target
    if abs(initial.wavelength_setpoint_nm-baseline)>.02:
        raise RuntimeError('Stored target is outside the approved excursion; no writes sent')
    bounds=device.operating_range_nm
    if not bounds[0]<=baseline<=bounds[1] or not bounds[0]<=initial.wavelength_setpoint_nm<=bounds[1]:
        raise RuntimeError('Initial wavelength/target is outside operating limits; no writes sent')
    if not bounds[0]<=original_target<=bounds[1] or abs(original_target-baseline)>.02:
        raise RuntimeError('Restoration target exceeds the approved excursion; no writes sent')
    if initial.remote and not restore_local:raise RuntimeError('Qualification requires initial Local mode; no writes sent')
    if initial.tracking:raise RuntimeError('Qualification requires initial Tracking Off; no writes sent')
    target=round(baseline+step if baseline+step<=bounds[1] else baseline-step,3)
    if not bounds[0]<=target<=bounds[1] or abs(target-baseline)<.009:
        raise RuntimeError('No bounded test target fits operating limits')
    record['baseline_nm']=baseline;record['test_target_nm']=target
    if initial.remote:call('set_remote',False,confirm=True)
    call('control_tracking',False,confirm=True)
    call('set_target_wavelength',baseline,confirm=True)
    call('control_tracking',True,confirm=True)
    arrived(baseline,require_tracking=False)
    # Public typed mode actions isolate the cause without raw SCPI or bypass.
    call('set_remote',True,confirm=True);call('control_tracking',True,confirm=True);arrived(baseline);call('read_status')
    call('set_remote',False,confirm=True);call('read_status')
    call('control_tracking',True,confirm=True);arrived(baseline)
    call('set_target_wavelength',target,confirm=True);arrived(target)
    call('set_target_wavelength',baseline,confirm=True);arrived(baseline)
    if not initial.tracking:call('control_tracking',False,confirm=True)
    call('set_target_wavelength',original_target,confirm=True)
    if initial.tracking:arrived(original_target)
    final=call('read_status')
    expected_remote=False if restore_local else initial.remote
    if final.remote!=expected_remote:
        call('set_remote',expected_remote,confirm=True);final=call('read_status')
    if (final.output_enabled!=initial.output_enabled or final.current_setpoint_ma!=initial.current_setpoint_ma
            or final.tracking!=initial.tracking or final.remote!=expected_remote
            or abs(final.wavelength_setpoint_nm-original_target)>.0001
            or abs(final.wavelength_nm-(original_target if initial.tracking else baseline))>.008):
        raise RuntimeError('Original state restoration was not verified')
    record['restored']=True


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--confirm-motion',action='store_true',help='Requires separate operator authorization')
    parser.add_argument('--device-key',required=True,choices=['6700 SN22500001'])
    parser.add_argument('--min-nm',type=float,required=True,help='Current registered operating minimum')
    parser.add_argument('--max-nm',type=float,required=True,help='Current registered operating maximum')
    parser.add_argument('--max-speed-nm-s',type=float,required=True,help='Current registered speed ceiling')
    parser.add_argument('--evidence',required=True)
    parser.add_argument('--restore-target-nm',type=float,help='Known original target from a previous fully acknowledged attempt')
    parser.add_argument('--restore-local',action='store_true',help='Restore the known original Local mode from a previous acknowledged attempt')
    args=parser.parse_args()
    if not args.confirm_motion:parser.error('No device opened: --confirm-motion requires separate operator authorization')
    path=Path(args.evidence)
    if path.exists():parser.error('Evidence already exists; choose a fresh path')
    device=TLB6700(device_key=args.device_key,control_limits={'min_nm':args.min_nm,'max_nm':args.max_nm,'max_speed_nm_s':args.max_speed_nm_s})
    record={'stage':'authorized_tracking_action','steps':[],'restored':False}
    try:
        device.connect();record['identity']=device.identity
        qualify(device,record,restore_target=args.restore_target_nm,restore_local=args.restore_local)
    except Exception as cause:
        record['error']=str(cause)
    finally:
        try:device.close()
        except Exception as cause:record['close_error']=str(cause)
        record['release_confirmed']=device.resources_released
        path.parent.mkdir(parents=True,exist_ok=True)
        path.write_text(json.dumps(record,indent=2),encoding='utf-8')
    print(json.dumps(record,indent=2))
    return 0 if record['restored'] and record['release_confirmed'] and not record.get('error') else 1


if __name__=='__main__':raise SystemExit(main())
