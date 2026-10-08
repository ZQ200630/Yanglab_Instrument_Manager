"""Read-only Windows USB serial driver metadata, including driverless devices.

No serial port is opened. An absent device is never evidence of a missing driver.
"""
from __future__ import annotations

import re

from .newport_usb import windows_devices

DRIVERS = {
    'ch340': {'ids': {(0x1A86, 0x7523), (0x1A86, 0x5523)},
              'services': {'CH341SER', 'CH341SER_A', 'CH341SER_A64', 'CH341SER_M64'}},
    'cp210x': {'ids': {(0x10C4, pid) for pid in (0xEA60, 0xEA63, 0xEA70, 0xEA71, 0xEA7A, 0xEA7B)},
               'services': {'SILABSER'}},
}


def inventory(*, _records=None):
    """Group present adapters by driver family; only problem 28 means missing."""
    records = windows_devices(set().union(*(d['ids'] for d in DRIVERS.values()))) if _records is None else _records
    result = {}
    for name, driver in DRIVERS.items():
        devices = []
        for record in records:
            identity = re.match(r'USB\\VID_([0-9A-F]{4})&PID_([0-9A-F]{4})(?:[\\&]|$)',
                                str(record.get('instance_id', '')).upper())
            if not identity or (int(identity[1], 16), int(identity[2], 16)) not in driver['ids']:
                continue
            # The vendor INF binds MI interfaces, not the healthy USBCCGP parent.
            if name == 'cp210x' and int(identity[2], 16) in (0xEA70, 0xEA71, 0xEA7A, 0xEA7B):
                if not re.match(r'USB\\VID_10C4&PID_[0-9A-F]{4}&MI_[0-9A-F]{2}\\', str(record['instance_id']).upper()):
                    continue
            code, service = record.get('problem_code'), str(record.get('service', '')).upper()
            state = 'missing' if code == 28 else 'ready' if code == 0 and service in driver['services'] else 'unavailable'
            devices.append({**record, 'driver_state': state})
        states = {d['driver_state'] for d in devices}
        state = ('not_detected' if not devices else 'missing' if 'missing' in states
                 else 'unavailable' if 'unavailable' in states else 'ready')
        result[name] = {'state': state, 'devices': devices}
    return result
